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
        use eez_telemetry::schema::Event;
        let captured = self.spans()?;
        for entry in &captured {
            let service = entry
                .resource
                .get("service.name")
                .context("missing service identity")?;
            anyhow::ensure!(matches!(
                service.as_str(),
                "eez-composer" | "eez-proof-signer"
            ));
            anyhow::ensure!(
                entry
                    .resource
                    .get("deployment.environment.name")
                    .map(String::as_str)
                    == Some("devnet")
            );
            anyhow::ensure!(
                entry.resource.get("eez.deployment").map(String::as_str)
                    == Some("native-trace-pilot")
            );
            anyhow::ensure!(
                entry
                    .resource
                    .get("service.instance.id")
                    .map(String::as_str)
                    == Some("fixture")
            );
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
        let mut connected = Vec::new();
        for signer in captured.iter().filter(|entry| entry.span.name == "prove") {
            anyhow::ensure!(signer.resource["service.name"] == "eez-proof-signer");
            let mut child = signer;
            let mut complete = true;
            let mut retried = false;
            for expected in [
                "prover_client",
                "prover_attempt",
                "composer_proving",
                "composer_preparation",
            ] {
                let Some(parent) = captured.iter().find(|entry| {
                    entry.span.trace_id == child.span.trace_id
                        && entry.span.span_id == child.span.parent_span_id
                }) else {
                    complete = false;
                    break;
                };
                anyhow::ensure!(
                    parent.span.name == expected,
                    "unexpected parent of {}: {}",
                    child.span.name,
                    parent.span.name
                );
                anyhow::ensure!(parent.resource["service.name"] == "eez-composer");
                anyhow::ensure!(parent.span.span_id != child.span.span_id);
                if expected == "prover_attempt" {
                    anyhow::ensure!(
                        integer_value(&parent.span.attributes, "retry_attempt")
                            .is_some_and(|attempt| attempt >= 1)
                    );
                    retried = integer_value(&parent.span.attributes, "retry_attempt")
                        .is_some_and(|attempt| attempt > 1)
                        && captured.iter().any(|earlier| {
                            earlier.span.name == "prover_attempt"
                                && earlier.span.trace_id == parent.span.trace_id
                                && earlier.span.parent_span_id == parent.span.parent_span_id
                                && earlier.span.span_id != parent.span.span_id
                                && integer_value(&earlier.span.attributes, "retry_attempt")
                                    == Some(1)
                        });
                }
                child = parent;
            }
            if complete && retried {
                connected.push(child);
            }
        }
        let events = lifecycle_payloads(spool)?;
        let mut trace_ids = Vec::new();
        for hash in hashes {
            let hash_hex = hex_id(&hash.0);
            let matched = connected.iter().find(|preparation| {
                events.iter().any(|payload| {
                    let Event::PreparationStarted { members, .. } = &payload.event else {
                        return false;
                    };
                    let Some(context) = payload.trace else {
                        return false;
                    };
                    if context.trace_id.0.as_slice() != preparation.span.trace_id
                        || context.span_id.0.as_slice() != preparation.span.span_id
                    {
                        return false;
                    }
                    members
                        .iter()
                        .filter(|member| member.source.transaction_hash.0 == hash.0)
                        .any(|member| {
                            captured.iter().any(|source| {
                                source.span.name == "batch_member"
                                    && string_value(&source.span.attributes, "source_hash")
                                        == Some(hash_hex.as_str())
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
                                            eez_telemetry::schema::Direction::Inbound => "Inbound",
                                            eez_telemetry::schema::Direction::Outbound => {
                                                "Outbound"
                                            }
                                        })
                                    && string_value(&source.span.attributes, "genesis_hash")
                                        == Some(hex_id(&member.source.genesis_hash.0).as_str())
                                    && preparation.span.links.iter().any(|link| {
                                        link.trace_id == source.span.trace_id
                                            && link.span_id == source.span.span_id
                                    })
                            })
                        })
                })
            });
            let Some(preparation) = matched else {
                return Ok(None);
            };
            trace_ids.push(hex_id(&preparation.span.trace_id));
        }
        if trace_ids.windows(2).any(|ids| ids[0] != ids[1]) {
            return Ok(None);
        }
        Ok(Some(trace_ids))
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.task.abort();
    }
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
