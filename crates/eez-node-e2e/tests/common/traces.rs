//! Owned OTLP/HTTP receiver for native-process propagation assertions. Capture
//! bounds are independent of the runtime exporter and failures remain observable.
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Request, Response,
    body::{Bytes, Incoming},
};
use hyper_util::rt::TokioIo;
use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest,
    common::v1::{KeyValue, any_value::Value},
    trace::v1::Span,
};
use prost::Message;

#[derive(Clone, Debug)]
pub struct CapturedSpan {
    pub resource: BTreeMap<String, String>,
    pub span: Span,
}

#[derive(Default)]
struct Capture {
    spans: Vec<CapturedSpan>,
    bytes: usize,
    requests: usize,
    error: Option<String>,
    directory: Option<std::path::PathBuf>,
}

pub struct Collector {
    pub address: SocketAddr,
    capture: Arc<Mutex<Capture>>,
    task: tokio::task::JoinHandle<()>,
}

impl Collector {
    pub async fn start() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let directory = std::env::var_os("EEZ_TEST_LOG_DIR").map(|directory| {
            std::path::PathBuf::from(directory).join(format!("native-traces-{}", address.port()))
        });
        if let Some(directory) = &directory {
            std::fs::create_dir_all(directory)?;
        }
        let capture = Arc::new(Mutex::new(Capture {
            directory,
            ..Capture::default()
        }));
        let state = capture.clone();
        let task = tokio::spawn(async move {
            loop {
                let accepted = listener.accept().await;
                let result = match accepted {
                    Ok((stream, _)) => {
                        let state = state.clone();
                        let service = hyper::service::service_fn(move |request| {
                            receive(request, state.clone())
                        });
                        tokio::time::timeout(
                            Duration::from_secs(2),
                            hyper::server::conn::http1::Builder::new()
                                .keep_alive(false)
                                .serve_connection(TokioIo::new(stream), service),
                        )
                        .await
                        .context("OTLP receiver connection deadline")
                        .and_then(|result| result.map_err(anyhow::Error::from))
                    }
                    Err(error) => Err(error.into()),
                };
                if let Err(error) = result {
                    state.lock().unwrap().error = Some(format!("{error:#}"));
                    break;
                }
            }
        });
        Ok(Self {
            address,
            capture,
            task,
        })
    }

    pub fn spans(&self) -> Result<Vec<CapturedSpan>> {
        let capture = self.capture.lock().unwrap();
        if let Some(error) = &capture.error {
            bail!("OTLP receiver failed: {error}");
        }
        Ok(capture.spans.clone())
    }

    pub fn report(&self) -> Result<()> {
        let spans = self.spans()?;
        let mut counts = BTreeMap::new();
        for captured in &spans {
            *counts
                .entry(format!(
                    "{}/{}",
                    captured.resource["service.name"], captured.span.name
                ))
                .or_insert(0_usize) += 1;
        }
        let capture = self.capture.lock().unwrap();
        println!(
            "native OTLP pilot: {}",
            serde_json::json!({
                "requests": capture.requests, "protobuf_bytes": capture.bytes,
                "spans": spans.len(), "span_counts": counts,
            })
        );
        Ok(())
    }

    pub fn retried_batch(
        &self,
        spool: &std::path::Path,
        hashes: &[alloy_primitives::TxHash],
    ) -> Result<bool> {
        let captured = self.spans()?;
        Ok(lifecycle_payloads(spool)?.iter().any(|payload| {
            let eez_telemetry::schema::Event::PreparationStarted { members, .. } = &payload.event
            else {
                return false;
            };
            let Some(context) = payload.trace else {
                return false;
            };
            hashes.iter().all(|hash| {
                members
                    .iter()
                    .any(|member| member.source.transaction_hash.0 == hash.0)
            }) && captured.iter().any(|entry| {
                // Ended spans are exported only after the preparation closes.
                // Keeping the outage active until then deterministically tests
                // cutoff/requeue recovery instead of racing the next retry.
                entry.span.name == "composer_preparation"
                    && entry.span.trace_id == context.trace_id.0
                    && entry.span.span_id == context.span_id.0
            }) && captured.iter().any(|entry| {
                entry.span.name == "prover_attempt"
                    && entry.span.trace_id == context.trace_id.0
                    && integer_value(&entry.span.attributes, "retry_attempt")
                        .is_some_and(|attempt| attempt >= 2)
            })
        }))
    }

    /// A successful protocol run alone cannot establish propagation: require
    /// exported parent IDs through every production stage, then join the native
    /// lifecycle preparation and exact admission/source links to those spans.
    pub fn connected_sources(
        &self,
        spool: &std::path::Path,
        hashes: &[alloy_primitives::TxHash],
    ) -> Result<Option<Vec<String>>> {
        connected_sources(&self.spans()?, &lifecycle_payloads(spool)?, hashes)
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn ancestor<'a>(
    captured: &'a [CapturedSpan],
    mut child: &'a CapturedSpan,
    names: &[&str],
) -> Result<Option<&'a CapturedSpan>> {
    for expected in names {
        let Some(parent) = captured.iter().find(|entry| {
            entry.span.trace_id == child.span.trace_id
                && entry.span.span_id == child.span.parent_span_id
        }) else {
            return Ok(None);
        };
        anyhow::ensure!(
            parent.span.name == *expected,
            "unexpected parent of {}: {}",
            child.span.name,
            parent.span.name
        );
        anyhow::ensure!(parent.resource["service.name"] == "eez-composer");
        anyhow::ensure!(parent.span.span_id != child.span.span_id);
        if *expected == "prover_attempt" {
            anyhow::ensure!(
                integer_value(&parent.span.attributes, "retry_attempt")
                    .is_some_and(|attempt| attempt >= 1)
            );
        }
        child = parent;
    }
    Ok(Some(child))
}

fn linked_members(
    captured: &[CapturedSpan],
    events: &[eez_telemetry::schema::Payload],
    preparation: &CapturedSpan,
    hashes: &[alloy_primitives::TxHash],
) -> Option<Vec<eez_telemetry::schema::Member>> {
    use eez_telemetry::schema::{Direction, Event};
    events.iter().find_map(|payload| {
        let Event::PreparationStarted { members, .. } = &payload.event else {
            return None;
        };
        let context = payload.trace?;
        if context.trace_id.0.as_slice() != preparation.span.trace_id
            || context.span_id.0.as_slice() != preparation.span.span_id
        {
            return None;
        }
        hashes
            .iter()
            .map(|hash| {
                members
                    .iter()
                    .find(|member| {
                        member.source.transaction_hash.0 == hash.0
                            && captured.iter().any(|source| {
                                source.span.name == "batch_member"
                                    && source.resource["service.name"] == "eez-composer"
                                    && string_value(&source.span.attributes, "source_hash")
                                        == Some(hex_id(&hash.0).as_str())
                                    && string_value(&source.span.attributes, "admission_epoch")
                                        == Some(hex_id(&member.admission.epoch.0).as_str())
                                    && integer_value(&source.span.attributes, "admission_sequence")
                                        == i64::try_from(member.admission.sequence).ok()
                                    && integer_value(&source.span.attributes, "chain_id")
                                        == i64::try_from(member.source.chain_id).ok()
                                    && integer_value(&source.span.attributes, "rollup_id")
                                        == i64::try_from(member.source.rollup_id).ok()
                                    && string_value(&source.span.attributes, "direction")
                                        == Some(match member.source.direction {
                                            Direction::Inbound => "Inbound",
                                            Direction::Outbound => "Outbound",
                                        })
                                    && string_value(&source.span.attributes, "genesis_hash")
                                        == Some(hex_id(&member.source.genesis_hash.0).as_str())
                                    && preparation.span.links.iter().any(|link| {
                                        link.trace_id == source.span.trace_id
                                            && link.span_id == source.span.span_id
                                    })
                            })
                    })
                    .copied()
            })
            .collect()
    })
}

// Preparation may be background prewarming, and capability/lease calls may
// occur outside a user batch. Bind every observed server call to its finite
// client RPC span without inventing lifecycle admission links for those calls.
fn control_rpc_ancestry(captured: &[CapturedSpan]) -> Result<bool> {
    for server in captured {
        let client = match server.span.name.as_str() {
            "capabilities" => "prover_capabilities",
            "prepare_ordinary" => "prover_prepare_ordinary",
            "retain_ordinary_ranges" => "prover_retain_ordinary",
            _ => continue,
        };
        anyhow::ensure!(server.resource["service.name"] == "eez-proof-signer");
        anyhow::ensure!(
            server.span.parent_span_id.len() == 8,
            "{} lost its remote client context",
            server.span.name
        );
        if ancestor(captured, server, &[client])?.is_none() {
            // Client and server exporters are independent: await a parent that
            // has not arrived rather than accepting an incomplete graph.
            return Ok(false);
        }
    }
    Ok(true)
}

fn preparation_succeeded(events: &[eez_telemetry::schema::Payload], span: &CapturedSpan) -> bool {
    use eez_telemetry::schema::{Event, PreparationOutcome};
    let Some(epoch) = string_value(&span.span.attributes, "preparation_epoch") else {
        return false;
    };
    let Some(sequence) = integer_value(&span.span.attributes, "preparation_sequence")
        .and_then(|value| u64::try_from(value).ok())
    else {
        return false;
    };
    let mut finishes = events.iter().filter_map(|payload| {
        let Event::PreparationFinished {
            preparation,
            outcome,
            ..
        } = &payload.event
        else {
            return None;
        };
        let context = payload.trace?;
        (hex_id(&preparation.epoch.0) == epoch
            && preparation.sequence == sequence
            && context.trace_id.0.as_slice() == span.span.trace_id
            && context.span_id.0.as_slice() == span.span.span_id)
            .then_some(*outcome)
    });
    finishes.next() == Some(PreparationOutcome::Prepared) && finishes.next().is_none()
}

fn connected_sources(
    captured: &[CapturedSpan],
    events: &[eez_telemetry::schema::Payload],
    hashes: &[alloy_primitives::TxHash],
) -> Result<Option<Vec<String>>> {
    anyhow::ensure!(!hashes.is_empty(), "empty native source population");
    for entry in captured {
        let service = entry
            .resource
            .get("service.name")
            .context("missing service identity")?;
        anyhow::ensure!(matches!(
            service.as_str(),
            "eez-composer" | "eez-proof-signer"
        ));
        for (key, expected) in [
            ("deployment.environment.name", "devnet"),
            ("eez.deployment", "native-trace-pilot"),
            ("service.instance.id", "fixture"),
        ] {
            anyhow::ensure!(entry.resource.get(key).map(String::as_str) == Some(expected));
        }
        anyhow::ensure!(entry.span.trace_id.len() == 16 && entry.span.span_id.len() == 8);
        anyhow::ensure!(entry.span.end_time_unix_nano >= entry.span.start_time_unix_nano);
        anyhow::ensure!(
            entry.span.dropped_attributes_count == 0 && entry.span.dropped_links_count == 0
        );
        anyhow::ensure!(
            entry.span.events.is_empty(),
            "arbitrary diagnostic events exported"
        );
    }

    if !control_rpc_ancestry(captured)? {
        return Ok(None);
    }

    // A transport outage can exhaust the exact-target cutoff. The selected
    // admissions then move to a new preparation (RPC SPEC section 6.3). Require
    // complete retry and successful-signing graphs joined by the same complete
    // admission identities, rather than requiring the failed window to sign.
    let mut retried = Vec::new();
    for client in captured
        .iter()
        .filter(|entry| entry.span.name == "prover_client")
    {
        anyhow::ensure!(client.resource["service.name"] == "eez-composer");
        let Some(attempt) = ancestor(captured, client, &["prover_attempt"])? else {
            continue;
        };
        if integer_value(&attempt.span.attributes, "retry_attempt").is_none_or(|n| n <= 1) {
            continue;
        }
        let Some(preparation) = ancestor(
            captured,
            attempt,
            &["composer_proving", "composer_preparation"],
        )?
        else {
            continue;
        };
        let first = captured.iter().find(|earlier| {
            earlier.span.name == "prover_attempt"
                && earlier.resource["service.name"] == "eez-composer"
                && earlier.span.trace_id == attempt.span.trace_id
                && earlier.span.parent_span_id == attempt.span.parent_span_id
                && earlier.span.span_id != attempt.span.span_id
                && integer_value(&earlier.span.attributes, "retry_attempt") == Some(1)
                && captured.iter().any(|client| {
                    client.span.name == "prover_client"
                        && client.resource["service.name"] == "eez-composer"
                        && client.span.trace_id == earlier.span.trace_id
                        && client.span.parent_span_id == earlier.span.span_id
                })
        });
        if first.is_some()
            && let Some(members) = linked_members(captured, events, preparation, hashes)
        {
            retried.push((preparation, members));
        }
    }
    for signer in captured.iter().filter(|entry| entry.span.name == "prove") {
        anyhow::ensure!(signer.resource["service.name"] == "eez-proof-signer");
        let Some(preparation) = ancestor(
            captured,
            signer,
            &[
                "prover_client",
                "prover_attempt",
                "composer_proving",
                "composer_preparation",
            ],
        )?
        else {
            continue;
        };
        // A connected server span can finish after the Composer cancels its
        // request. Only this exact successful preparation can supply the
        // attestation/log correlation checked by the caller.
        if !preparation_succeeded(events, preparation) {
            continue;
        }
        let Some(members) = linked_members(captured, events, preparation, hashes) else {
            continue;
        };
        if retried.iter().any(|(retry, retry_members)| {
            *retry_members == members
                && retry.span.start_time_unix_nano <= preparation.span.start_time_unix_nano
        }) {
            return Ok(Some(vec![hex_id(&preparation.span.trace_id); hashes.len()]));
        }
    }
    Ok(None)
}

async fn receive(
    request: Request<Incoming>,
    capture: Arc<Mutex<Capture>>,
) -> Result<Response<Full<Bytes>>> {
    if request.method() != hyper::Method::POST || request.uri().path() != "/v1/traces" {
        bail!("unexpected OTLP method/path");
    }
    let bytes = Limited::new(request.into_body(), 2 * 1024 * 1024)
        .collect()
        .await
        .map_err(|error| anyhow::anyhow!("OTLP body: {error}"))?
        .to_bytes();
    let decoded = ExportTraceServiceRequest::decode(bytes.as_ref())?;
    let mut capture = capture.lock().unwrap();
    capture.bytes += bytes.len();
    capture.requests += 1;
    if capture.bytes > 16 * 1024 * 1024 {
        bail!("OTLP pilot exceeded 16 MiB capture bound");
    }
    // Persist exact payloads only in the explicitly selected local test log
    // directory; no hashes, addresses or bulk traces enter repository evidence.
    if let Some(directory) = &capture.directory {
        let path = directory.join(format!("{}.pb", capture.requests));
        std::fs::write(path, &bytes)?;
    }
    for group in decoded.resource_spans {
        let attributes = group.resource.context("missing OTLP resource")?.attributes;
        let resource: BTreeMap<_, _> = attributes
            .iter()
            .filter_map(|attribute| {
                string_value(&attributes, &attribute.key)
                    .map(|value| (attribute.key.clone(), value.to_owned()))
            })
            .collect();
        for scope in group.scope_spans {
            for span in scope.spans {
                if capture.spans.len() >= 8192 {
                    bail!("OTLP pilot exceeded 8192 span bound");
                }
                capture.spans.push(CapturedSpan {
                    resource: resource.clone(),
                    span,
                });
            }
        }
    }
    Ok(Response::new(Full::new(Bytes::new())))
}

pub fn string_value<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.value.as_ref())
        .and_then(|value| match value {
            Value::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}

fn integer_value(attributes: &[KeyValue], key: &str) -> Option<i64> {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.value.as_ref())
        .and_then(|value| match value {
            Value::IntValue(value) => Some(*value),
            // The native Reth subscriber's field layer records these reviewed
            // tracing numeric fields as strings; the isolated bridge emits ints.
            Value::StringValue(value) => value.parse().ok(),
            _ => None,
        })
}

fn hex_id(bytes: &[u8]) -> String {
    alloy_primitives::hex::encode(bytes)
}

fn lifecycle_payloads(directory: &std::path::Path) -> Result<Vec<eez_telemetry::schema::Payload>> {
    use eez_telemetry::{reader, schema::Record};
    let mut chunks = BTreeMap::new();
    for id in reader::inventory(directory)? {
        let mut offset = 0;
        let mut complete = false;
        for _ in 0..64 {
            let batch = reader::read_segment(directory, id, offset)?;
            for line in batch.lines {
                if let Record::Data(record) = serde_json::from_slice(&line.bytes)? {
                    chunks
                        .entry((record.epoch, record.sequence))
                        .or_insert_with(Vec::new)
                        .push(record);
                }
            }
            offset = batch.next_offset;
            if batch.snapshot_complete {
                complete = true;
                break;
            }
        }
        anyhow::ensure!(complete, "native pilot lifecycle exceeded bounded read");
    }
    chunks
        .values()
        .filter(|chunks| chunks.len() == chunks[0].chunk_count as usize)
        .map(|chunks| eez_telemetry::assemble(chunks).map_err(anyhow::Error::msg))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::TxHash;
    use eez_telemetry::schema::{
        AdmissionId, BlockRef, Direction, Epoch, Event, Hash32, Member, Payload, PreparationId,
        PreparationOutcome, Source, SpanId, TraceContext, TraceId,
    };
    use opentelemetry_proto::tonic::{common::v1::AnyValue, trace::v1::span::Link};

    fn attribute(key: &str, value: impl ToString) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(value.to_string())),
            }),
        }
    }

    fn span(name: &str, trace: u8, id: u8, parent: u8) -> CapturedSpan {
        CapturedSpan {
            resource: [
                (
                    "service.name",
                    if matches!(
                        name,
                        "prove" | "capabilities" | "prepare_ordinary" | "retain_ordinary_ranges"
                    ) {
                        "eez-proof-signer"
                    } else {
                        "eez-composer"
                    },
                ),
                ("deployment.environment.name", "devnet"),
                ("eez.deployment", "native-trace-pilot"),
                ("service.instance.id", "fixture"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
            span: Span {
                name: name.into(),
                trace_id: vec![trace; 16],
                span_id: vec![id; 8],
                parent_span_id: if parent == 0 { vec![] } else { vec![parent; 8] },
                start_time_unix_nano: u64::from(trace) * 100,
                end_time_unix_nano: u64::from(trace) * 100 + 50,
                ..Default::default()
            },
        }
    }

    // Both windows carry the same two exact admissions. A second window models
    // the retained failure: the first preparation exhausted its cutoff, then a
    // new preparation signed the requeued sources on its first RPC attempt.
    fn fixture(rollover: bool) -> (Vec<CapturedSpan>, Vec<Payload>, [TxHash; 2]) {
        let hashes = [TxHash::repeat_byte(7), TxHash::repeat_byte(8)];
        let members: Vec<_> = hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| Member {
                source: Source {
                    chain_id: 1,
                    genesis_hash: Hash32([9; 32]),
                    rollup_id: 1,
                    transaction_hash: Hash32(hash.0),
                    direction: if i == 0 {
                        Direction::Inbound
                    } else {
                        Direction::Outbound
                    },
                },
                admission: AdmissionId {
                    epoch: Epoch([10; 16]),
                    sequence: i as u64 + 1,
                },
            })
            .collect();
        let mut spans = Vec::new();
        let mut events = Vec::new();
        let mut finishes = Vec::new();
        for trace in 1..=if rollover { 2 } else { 1 } {
            let mut preparation = span("composer_preparation", trace, 1, 0);
            preparation.span.attributes = vec![
                attribute("preparation_epoch", hex_id(&[10; 16])),
                attribute("preparation_sequence", trace),
            ];
            for (i, member) in members.iter().enumerate() {
                let mut source = span("batch_member", trace + 10, i as u8 + 20, 0);
                source.span.attributes = vec![
                    attribute("source_hash", hex_id(&member.source.transaction_hash.0)),
                    attribute("admission_epoch", hex_id(&member.admission.epoch.0)),
                    attribute("admission_sequence", member.admission.sequence),
                    attribute("chain_id", member.source.chain_id),
                    attribute("rollup_id", member.source.rollup_id),
                    attribute("direction", if i == 0 { "Inbound" } else { "Outbound" }),
                    attribute("genesis_hash", hex_id(&member.source.genesis_hash.0)),
                ];
                preparation.span.links.push(Link {
                    trace_id: source.span.trace_id.clone(),
                    span_id: source.span.span_id.clone(),
                    ..Default::default()
                });
                spans.push(source);
            }
            events.push(Payload {
                observed_at_unix_ms: 0,
                trace: Some(TraceContext {
                    trace_id: TraceId([trace; 16]),
                    span_id: SpanId([1; 8]),
                }),
                event: Event::PreparationStarted {
                    l2_chain_id: 1,
                    l2_genesis_hash: Hash32([9; 32]),
                    rollup_id: 1,
                    sync: BlockRef {
                        number: u64::from(trace) * 5,
                        hash: Hash32([trace; 32]),
                    },
                    members: members.clone(),
                },
            });
            finishes.push(Payload {
                observed_at_unix_ms: 0,
                trace: Some(TraceContext {
                    trace_id: TraceId([trace; 16]),
                    span_id: SpanId([1; 8]),
                }),
                event: Event::PreparationFinished {
                    preparation: PreparationId {
                        epoch: Epoch([10; 16]),
                        sequence: u64::from(trace),
                    },
                    outcome: if !rollover || trace == 2 {
                        PreparationOutcome::Prepared
                    } else {
                        PreparationOutcome::ProverFailed
                    },
                    elapsed_ns: 50,
                },
            });
            spans.push(preparation);
            spans.push(span("composer_proving", trace, 2, 1));
            let attempts = if trace == 1 { 2 } else { 1 };
            for attempt in 1..=attempts {
                let id = 2 + attempt * 2;
                let mut a = span("prover_attempt", trace, id, 2);
                a.span.attributes.push(attribute("retry_attempt", attempt));
                spans.push(a);
                spans.push(span("prover_client", trace, id + 1, id));
            }
            if !rollover || trace == 2 {
                spans.push(span("prove", trace, 9, 3 + attempts * 2));
            }
        }
        events.extend(finishes);
        (spans, events, hashes)
    }

    #[test]
    fn earlier_connected_failed_signer_does_not_replace_later_success() {
        let (mut spans, mut events, hashes) = fixture(true);
        // A timed-out server call still exports a fully connected graph.
        spans.insert(0, span("prove", 1, 9, 7));
        let expected = Some(vec![hex_id(&[2; 16]); 2]);
        assert_eq!(
            connected_sources(&spans, &events, &hashes).unwrap(),
            expected
        );
        spans.reverse();
        events.reverse();
        assert_eq!(
            connected_sources(&spans, &events, &hashes).unwrap(),
            expected
        );
    }

    #[test]
    fn success_requires_exact_finished_preparation_identity_and_context() {
        for mutation in [
            "missing",
            "epoch",
            "sequence",
            "trace",
            "span",
            "context",
            "duplicate",
        ] {
            let (spans, mut events, hashes) = fixture(true);
            let index = events
                .iter()
                .position(|event| {
                    matches!(
                        event.event,
                        Event::PreparationFinished {
                            outcome: PreparationOutcome::Prepared,
                            ..
                        }
                    )
                })
                .unwrap();
            if mutation == "missing" {
                events.remove(index);
            } else if mutation == "duplicate" {
                events.push(events[index].clone());
            } else {
                let finished = &mut events[index];
                let Event::PreparationFinished { preparation, .. } = &mut finished.event else {
                    unreachable!()
                };
                match mutation {
                    "epoch" => preparation.epoch = Epoch([99; 16]),
                    "sequence" => preparation.sequence = 99,
                    "trace" => finished.trace.as_mut().unwrap().trace_id = TraceId([99; 16]),
                    "span" => finished.trace.as_mut().unwrap().span_id = SpanId([99; 8]),
                    "context" => finished.trace = None,
                    _ => unreachable!(),
                }
            }
            assert!(
                connected_sources(&spans, &events, &hashes)
                    .unwrap()
                    .is_none(),
                "{mutation}"
            );
        }
    }

    #[test]
    fn failed_cancelled_or_panicked_preparation_never_qualifies_as_signed_success() {
        for failure in [
            PreparationOutcome::ConstructionFailed,
            PreparationOutcome::CapacityLimited,
            PreparationOutcome::ProverFailed,
            PreparationOutcome::ValidationRejected,
            PreparationOutcome::Cancelled,
            PreparationOutcome::Panicked,
        ] {
            let (spans, mut events, hashes) = fixture(false);
            let finished = events
                .iter_mut()
                .find(|event| matches!(event.event, Event::PreparationFinished { .. }))
                .unwrap();
            let Event::PreparationFinished { outcome, .. } = &mut finished.event else {
                unreachable!()
            };
            *outcome = failure;
            assert!(
                connected_sources(&spans, &events, &hashes)
                    .unwrap()
                    .is_none(),
                "{failure:?}"
            );
        }
    }

    #[test]
    fn control_rpc_ancestry_is_exact_without_requiring_user_batch_members() {
        assert!(control_rpc_ancestry(&[]).unwrap());
        for (server, client) in [
            ("capabilities", "prover_capabilities"),
            ("prepare_ordinary", "prover_prepare_ordinary"),
            ("retain_ordinary_ranges", "prover_retain_ordinary"),
        ] {
            let client_span = span(client, 30, 1, 0);
            let server_span = span(server, 30, 2, 1);
            assert!(control_rpc_ancestry(&[server_span.clone(), client_span.clone()]).unwrap());
            assert!(!control_rpc_ancestry(std::slice::from_ref(&server_span)).unwrap());
            let mut lost_context = server_span.clone();
            lost_context.span.parent_span_id.clear();
            assert!(control_rpc_ancestry(&[lost_context, client_span.clone()]).is_err());
            let mut wrong_trace = client_span.clone();
            wrong_trace.span.trace_id = vec![31; 16];
            assert!(!control_rpc_ancestry(&[server_span.clone(), wrong_trace]).unwrap());
            let mut wrong_parent = client_span.clone();
            wrong_parent.span.name = "composer_preparation".into();
            assert!(control_rpc_ancestry(&[server_span.clone(), wrong_parent]).is_err());
            let mut wrong_service = client_span;
            wrong_service
                .resource
                .insert("service.name".into(), "eez-proof-signer".into());
            assert!(control_rpc_ancestry(&[server_span, wrong_service]).is_err());
        }
    }

    #[test]
    fn incomplete_control_rpc_graph_cannot_hide_behind_a_complete_user_proof() {
        let (mut spans, events, hashes) = fixture(true);
        spans.push(span("prepare_ordinary", 30, 2, 1));
        assert!(
            connected_sources(&spans, &events, &hashes)
                .unwrap()
                .is_none()
        );
        spans.push(span("prover_prepare_ordinary", 30, 1, 0));
        assert!(
            connected_sources(&spans, &events, &hashes)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn signing_can_follow_retry_in_same_or_later_window() {
        for rollover in [false, true] {
            let (mut spans, mut events, hashes) = fixture(rollover);
            let expected = vec![hex_id(&[if rollover { 2 } else { 1 }; 16]); 2];
            assert_eq!(
                connected_sources(&spans, &events, &hashes).unwrap(),
                Some(expected.clone())
            );
            spans.reverse();
            events.reverse();
            assert_eq!(
                connected_sources(&spans, &events, &hashes).unwrap(),
                Some(expected)
            );
        }
    }

    #[test]
    fn complete_retry_and_signer_parent_chains_are_required() {
        for (trace, name, id) in [
            (1, "prover_attempt", 4),
            (1, "prover_client", 5),
            (1, "composer_proving", 2),
            (1, "composer_preparation", 1),
            (2, "prover_client", 5),
            (2, "prover_attempt", 4),
            (2, "composer_proving", 2),
            (2, "composer_preparation", 1),
            (2, "prove", 9),
        ] {
            let (mut spans, events, hashes) = fixture(true);
            spans.retain(|s| {
                !(s.span.trace_id == vec![trace; 16]
                    && s.span.name == name
                    && s.span.span_id == vec![id; 8])
            });
            assert!(
                connected_sources(&spans, &events, &hashes)
                    .unwrap()
                    .is_none(),
                "{trace}/{name}"
            );
        }
        let (mut spans, events, hashes) = fixture(true);
        spans
            .iter_mut()
            .find(|s| s.span.name == "prove")
            .unwrap()
            .span
            .parent_span_id = vec![2; 8];
        assert!(connected_sources(&spans, &events, &hashes).is_err());
    }

    #[test]
    fn both_windows_require_exact_lifecycle_members_and_exported_links() {
        for index in 0..2 {
            let (spans, mut events, hashes) = fixture(true);
            events.remove(index);
            assert!(
                connected_sources(&spans, &events, &hashes)
                    .unwrap()
                    .is_none()
            );
        }
        for trace in [1, 2] {
            for field in [
                "source_hash",
                "admission_epoch",
                "admission_sequence",
                "chain_id",
                "rollup_id",
                "direction",
                "genesis_hash",
            ] {
                let (mut spans, events, hashes) = fixture(true);
                spans
                    .iter_mut()
                    .find(|s| {
                        s.span.name == "batch_member" && s.span.trace_id == vec![trace + 10; 16]
                    })
                    .unwrap()
                    .span
                    .attributes
                    .retain(|a| a.key != field);
                assert!(
                    connected_sources(&spans, &events, &hashes)
                        .unwrap()
                        .is_none(),
                    "{trace}/{field}"
                );
            }
            let (mut spans, events, hashes) = fixture(true);
            spans
                .iter_mut()
                .find(|s| {
                    s.span.name == "composer_preparation" && s.span.trace_id == vec![trace; 16]
                })
                .unwrap()
                .span
                .links
                .pop();
            assert!(
                connected_sources(&spans, &events, &hashes)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn same_source_hash_on_a_new_admission_does_not_join_old_retries() {
        let (mut spans, mut events, hashes) = fixture(true);
        let Event::PreparationStarted { members, .. } = &mut events[1].event else {
            unreachable!()
        };
        members[0].admission.sequence = 99;
        let source = spans
            .iter_mut()
            .find(|s| s.span.name == "batch_member" && s.span.trace_id == vec![12; 16])
            .unwrap();
        source
            .span
            .attributes
            .retain(|a| a.key != "admission_sequence");
        source
            .span
            .attributes
            .push(attribute("admission_sequence", 99));
        assert!(
            connected_sources(&spans, &events, &hashes)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn retries_after_the_successful_window_do_not_prove_recovery() {
        let (mut spans, events, hashes) = fixture(true);
        for entry in &mut spans {
            if entry.span.trace_id == vec![1; 16] {
                entry.span.start_time_unix_nano += 200;
                entry.span.end_time_unix_nano += 200;
            }
        }
        assert!(
            connected_sources(&spans, &events, &hashes)
                .unwrap()
                .is_none()
        );
    }
}
