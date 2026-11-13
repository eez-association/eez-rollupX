//! Bounded native workload driver. Host eligibility is enforced by the external
//! quiet-window runner; ordinary correctness execution is never timing evidence.
use alloy_primitives::{Address, B256, Bytes, Signature, U256, hex, keccak256};
use eez_control_rpc::v2::{self, client_frame, server_frame};
use eez_protocol::l1_history::{EvidenceAnchor, ProofContext};
use eez_prover::{BlockWitness, Prover, ProvingContext};
use eez_prover_client::RemoteProver;
use eez_testkit::{ANVIL_ATTESTER_KEY, ProofSignerConfig, ProofSignerHandle};
use prost::Message as _;
use serde_json::{Value, json};
use std::{
    io::Write as _,
    path::PathBuf,
    time::{Duration, Instant},
};

const FIXTURES: [&str; 3] = [
    "native-offline-bidirectional",
    "native-osaka-lookup",
    "native-osaka-changing-static",
];
const SCENARIOS: [&str; 4] = ["cold", "prepared", "disconnected", "lost_session"];

struct Fixture {
    chain_config: PathBuf,
    l1_chain_config: PathBuf,
    identity: Value,
    context: ProvingContext,
    digest: B256,
}

impl Fixture {
    fn load(name: &str) -> Self {
        let directory = eez_testkit::repo_root()
            .join("crates/eez-proof-signer/tests/fixtures")
            .join(name);
        let read = |file: &str| std::fs::read(directory.join(file)).unwrap();
        let identity: Value = serde_json::from_slice(&read("identity.json")).unwrap();
        let expected: Value = serde_json::from_slice(&read("response.json")).unwrap();
        let bytes = read("request.v2.protobuf");
        assert_eq!(
            bytes.len() as u64,
            expected["request_bytes"].as_u64().unwrap()
        );
        assert_eq!(
            keccak256(&bytes),
            expected["request_hash"]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap()
        );
        let mut input = bytes.as_slice();
        let mut begin = None;
        let mut finalize = None;
        let mut blocks = Vec::new();
        let mut l1_tip: Option<Bytes> = None;
        while !input.is_empty() {
            let frame = v2::ClientFrame::decode_length_delimited(&mut input).unwrap();
            match frame.kind.unwrap() {
                client_frame::Kind::Begin(b) => assert!(begin.replace(b).is_none()),
                client_frame::Kind::Finalize(f) => assert!(finalize.replace(f).is_none()),
                client_frame::Kind::Block(b) => {
                    let b = b.data.unwrap();
                    let w = b.witness.unwrap();
                    // Captures contain appended segments. The production client accepts
                    // per-block durable witnesses with their shared endpoint restored.
                    let mut headers = l1_tip.clone().into_iter().collect::<Vec<_>>();
                    headers.extend(w.l1_headers_rlp.into_iter().map(Bytes::from));
                    l1_tip = headers.last().cloned();
                    blocks.push(BlockWitness {
                        number: b.number,
                        hash: B256::from_slice(&b.hash),
                        parent_hash: B256::from_slice(&b.parent_hash),
                        rlp: b.rlp.into(),
                        witness: alloy_rpc_types_debug::ExecutionWitness {
                            state: w.state.into_iter().map(Into::into).collect(),
                            codes: w.codes.into_iter().map(Into::into).collect(),
                            keys: w.keys.into_iter().map(Into::into).collect(),
                            headers: w.headers.into_iter().map(Into::into).collect(),
                        },
                        l1_headers_rlp: headers,
                    });
                }
                _ => panic!("capture is not a cold exchange"),
            }
        }
        let begin = begin.unwrap();
        let finalize = finalize.unwrap();
        let settlement = finalize.settlement.unwrap();
        let parent = blocks[0]
            .witness
            .headers
            .iter()
            .find_map(|rlp| {
                let header: alloy_consensus::Header = alloy_rlp::decode_exact(rlp).unwrap();
                (header.hash_slow() == blocks[0].parent_hash).then_some(header)
            })
            .unwrap();
        assert_eq!(parent.hash_slow().as_slice(), begin.anchor_hash);
        assert_eq!(parent.state_root.as_slice(), begin.anchor_state_root);
        let posting = settlement.posting_context.unwrap();
        let endpoint: alloy_consensus::Header =
            alloy_rlp::decode_exact(l1_tip.as_ref().unwrap()).unwrap();
        let genesis: alloy_genesis::Genesis =
            serde_json::from_slice(&read("l2-genesis.json")).unwrap();
        let proof_context = ProofContext::new(
            U256::from(genesis.config.chain_id),
            begin.rollup_id,
            EvidenceAnchor::ActualParent {
                number: posting.parent_number,
                hash: B256::from_slice(&posting.parent_hash),
                target_timestamp: posting.target_timestamp,
            },
            endpoint.timestamp,
        )
        .unwrap();
        Self {
            chain_config: directory.join("l2-genesis.json"),
            l1_chain_config: directory.join("l1-genesis.json"),
            identity,
            digest: B256::from_slice(
                &hex::decode(expected["public_inputs_hash"].as_str().unwrap()).unwrap(),
            ),
            context: ProvingContext {
                rollup_id: begin.rollup_id,
                from_block: finalize.from_block,
                to_block: finalize.to_block,
                batch: eez_protocol::entries::decode_postbatch(&settlement.abi_calldata).unwrap(),
                blobs: settlement
                    .blobs
                    .iter()
                    .enumerate()
                    .map(|(i, b)| eez_blob_codec::Blob::try_from_slice(i, b).unwrap())
                    .collect(),
                blocks: blocks.into(),
                l1_posting: Some(eez_prover::L1PostingContext {
                    chain_id: posting.chain_id,
                    parent_number: posting.parent_number,
                    parent_hash: B256::from_slice(&posting.parent_hash),
                    target_timestamp: posting.target_timestamp,
                }),
                proof_context,
                parent,
            },
        }
    }

    fn config(&self) -> ProofSignerConfig<'_> {
        let address = |key: &str| {
            self.identity[key]
                .as_str()
                .unwrap()
                .parse::<Address>()
                .unwrap()
        };
        ProofSignerConfig {
            registry_address: address("registry_address"),
            rollup_manager_address: address("rollup_manager_address"),
            chain_config: &self.chain_config,
            l1_chain_config: self.l1_chain_config.to_str().unwrap().into(),
            rollup_id: self.context.rollup_id,
            signer_key: ANVIL_ATTESTER_KEY,
            vkey: self.identity["vkey"].as_str().unwrap().parse().unwrap(),
            proof_system: address("proof_system"),
            max_request_blocks: None,
        }
    }
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

async fn sample(name: &str, scenario: &str) -> Value {
    let started = Instant::now();
    let (fixture, expected_identity) = tokio::task::spawn_blocking({
        let name = name.to_owned();
        move || {
            let fixture = Fixture::load(&name);
            let identity = fixture.config().validation_identity().unwrap();
            (fixture, identity)
        }
    })
    .await
    .unwrap();
    let load_ms = ms(started);
    let cfg = fixture.config();
    let startup = Instant::now();
    let mut signer = ProofSignerHandle::spawn_network_denied(&cfg).await.unwrap();
    let startup_ms = ms(startup);
    let proxy = eez_testkit::ProverProxyHandle::observe(signer.endpoint().to_owned())
        .await
        .unwrap();
    let attester = eez_testkit::signer_address(ANVIL_ATTESTER_KEY).unwrap();
    let prover = RemoteProver::new(proxy.endpoint(), attester, expected_identity, cfg.vkey);
    let entire = Instant::now();
    let capability = Instant::now();
    let capabilities = prover.capabilities().await.unwrap();
    let capability_ms = ms(capability);
    assert_eq!(capabilities.operations_tag, 1);
    assert_eq!(capabilities.profile_id, 2);
    assert_eq!(
        capabilities.validation_identity,
        expected_identity.as_slice()
    );
    assert_eq!(
        (
            capabilities.max_window_blocks,
            capabilities.max_request_bytes,
            capabilities.max_request_witness_items
        ),
        (65_536, 536_870_912, 1_000_000)
    );
    assert_eq!(
        (
            capabilities.stream_idle_timeout_millis,
            capabilities.request_timeout_millis
        ),
        (120_000, 600_000)
    );
    assert_eq!(capabilities.max_message_bytes, 268_435_456);
    let prefix = match scenario {
        "cold" => 0,
        "prepared" => fixture.context.blocks.len(),
        "disconnected" | "lost_session" => 15,
        _ => panic!("unknown scenario"),
    };
    let preparation_ms = if prefix != 0 {
        let preparation = Instant::now();
        prover
            .prevalidate_range(cfg.rollup_id, &fixture.context.blocks[..prefix])
            .await
            .unwrap();
        ms(preparation)
    } else {
        0.0
    };
    assert_eq!(
        signer.streaming_attestations().unwrap(),
        0,
        "prevalidation signed"
    );
    let restart_ms = if scenario == "lost_session" {
        let recovery = Instant::now();
        tokio::task::spawn_blocking(move || drop(signer))
            .await
            .unwrap();
        signer = ProofSignerHandle::spawn_network_denied(&cfg).await.unwrap();
        proxy.set_upstream(signer.endpoint().to_owned()).unwrap();
        ms(recovery)
    } else {
        0.0
    };
    let proving = Instant::now();
    let signature = prover.prove(fixture.context.clone()).await.unwrap();
    let proving_ms = ms(proving);
    let entire_ms = ms(entire);
    assert_eq!(
        Signature::try_from(signature.as_ref())
            .unwrap()
            .recover_address_from_prehash(&fixture.digest)
            .unwrap(),
        attester
    );
    assert_eq!(signer.streaming_attestations().unwrap(), 1);
    signer.assert_alive();
    let events = proxy.observations().unwrap();
    let client = events
        .iter()
        .filter(|e| e["direction"] == "client")
        .collect::<Vec<_>>();
    let count = |kind: &str| client.iter().filter(|e| e["kind"] == kind).count();
    let n = fixture.context.blocks.len();
    assert_eq!(
        count("Block"),
        match scenario {
            "prepared" => n + 1,
            "lost_session" => n + 15,
            _ => n,
        }
    );
    assert_eq!(
        count("Begin"),
        if scenario == "lost_session" { 2 } else { 1 }
    );
    assert_eq!(count("Resume"), usize::from(prefix != 0));
    assert_eq!(count("Rewind"), usize::from(scenario == "prepared"));
    let header_charge = |index: usize| {
        let segment = &fixture.context.blocks[index].l1_headers_rlp[usize::from(index != 0)..];
        (
            segment.len() as u64,
            segment.iter().map(|h| h.len() as u64).sum::<u64>(),
        )
    };
    let sent_headers = client
        .iter()
        .map(|e| e["l1_headers"].as_u64().unwrap())
        .sum::<u64>();
    let sent_header_bytes = client
        .iter()
        .map(|e| e["l1_header_bytes"].as_u64().unwrap())
        .sum::<u64>();
    let mut indices = (0..n).collect::<Vec<_>>();
    if scenario == "prepared" {
        indices.push(n - 1);
    }
    if scenario == "lost_session" {
        indices.extend(0..15);
    }
    let expected_headers = indices
        .into_iter()
        .map(header_charge)
        .fold((0, 0), |(count, bytes), (next_count, next_bytes)| {
            (count + next_count, bytes + next_bytes)
        });
    assert_eq!((sent_headers, sent_header_bytes), expected_headers);
    let mut proofs = 0;
    let mut proof_transport = None;
    let mut not_found = 0;
    for event in events.iter().filter(|e| e["direction"] == "server") {
        let frame = v2::ServerFrame::decode(
            hex::decode(event["protobuf_hex"].as_str().unwrap())
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        match frame.kind.unwrap() {
            server_frame::Kind::Validated(v) => {
                let b = &fixture.context.blocks[(v.number - 1) as usize];
                let decoded: eez_primitives::Block = alloy_rlp::decode_exact(&b.rlp).unwrap();
                assert_eq!(v.hash, b.hash.as_slice());
                assert_eq!(v.post_state_root, decoded.header.state_root.as_slice());
            }
            server_frame::Kind::Proof(p) => {
                assert_eq!(p.public_inputs_hash, fixture.digest.as_slice());
                proofs += 1;
                proof_transport = Some(event["transport"].clone());
            }
            server_frame::Kind::Rejected(r) => {
                assert_eq!(r.code, tonic::Code::NotFound as i32);
                not_found += 1;
            }
            server_frame::Kind::Ready(_) => {}
            other => panic!("unexpected response: {other:?}"),
        }
    }
    assert_eq!(proofs, 1);
    assert_eq!(not_found, usize::from(scenario == "lost_session"));
    assert!(
        events
            .iter()
            .any(|e| e["direction"] == "eof" && Some(e["transport"].clone()) == proof_transport)
    );
    json!({"fixture": name, "scenario": scenario, "fixture_load_ms": load_ms,
        "startup_readiness_ms": startup_ms, "capability_ms": capability_ms,
        "preparation_ms": preparation_ms, "restart_readiness_ms": restart_ms,
        "prove_verify_eof_ms": proving_ms, "complete_attempt_ms": entire_ms,
        "validation_identity": expected_identity, "attester": attester, "vkey": cfg.vkey,
        "blocks_sent": count("Block"), "client_protobuf_bytes": client.iter().map(|e| e["bytes"].as_u64().unwrap()).sum::<u64>(),
        "l1_headers_sent": sent_headers,
        "l1_header_bytes_sent": sent_header_bytes,
        "capabilities": {"profile_id": capabilities.profile_id, "operations_tag": capabilities.operations_tag,
            "max_window_blocks": capabilities.max_window_blocks, "max_request_bytes": capabilities.max_request_bytes,
            "max_request_witness_items": capabilities.max_request_witness_items, "max_message_bytes": capabilities.max_message_bytes,
            "stream_idle_timeout_millis": capabilities.stream_idle_timeout_millis, "request_timeout_millis": capabilities.request_timeout_millis},
        "semantic_checks": "ACK identities/roots, exact request proof digest, public attester, one Proof/EOF, preparation never signs, scenario-specific backfill and controls",
        "events": events})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_workload_preparation_and_recovery_are_checked() {
    // The source harness's first spawn builds all shipping entrypoints on a
    // blocking worker. Complete that setup before bounding native exchanges.
    let fixture = tokio::task::spawn_blocking(|| Fixture::load(FIXTURES[0]))
        .await
        .unwrap();
    let ready = ProofSignerHandle::spawn_network_denied(&fixture.config())
        .await
        .unwrap();
    assert_eq!(ready.streaming_attestations().unwrap(), 0);
    drop(ready);
    for name in FIXTURES {
        for scenario in SCENARIOS {
            tokio::time::timeout(Duration::from_mins(3), sample(name, scenario))
                .await
                .unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit measurement utility; run only through the quiet-window runner after correctness checks"]
async fn characterize_native_streaming_workload() {
    assert_eq!(
        std::env::var("EEZ_TEST_PREBUILT").as_deref(),
        Ok("1"),
        "measurements must never trigger a source build"
    );
    let path = std::env::var_os("EEZ_STREAMING_TIMINGS_OUTPUT")
        .expect("quiet-window runner must supply a new output path");
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    for name in FIXTURES {
        for scenario in SCENARIOS {
            let group = Instant::now();
            for repetition in 0..23 {
                assert!(
                    group.elapsed() < Duration::from_mins(15),
                    "partial group exceeds its budget; retain evidence"
                );
                writeln!(output, "{}", json!({"event": "attempt_start", "fixture": name, "scenario": scenario, "repetition": repetition})).unwrap();
                output.flush().unwrap();
                let started = Instant::now();
                let mut work = tokio::spawn(sample(name, scenario));
                let budget = Duration::from_mins(15)
                    .saturating_sub(group.elapsed())
                    .min(Duration::from_mins(3));
                let result = tokio::time::timeout(budget, &mut work).await;
                let (success, result) = match result {
                    Ok(Ok(result)) => (true, result),
                    Ok(Err(error)) => (
                        false,
                        json!({"fixture": name, "scenario": scenario, "error": error.to_string(), "failed_after_ms": ms(started)}),
                    ),
                    Err(_) => {
                        work.abort();
                        let _ = work.await;
                        (
                            false,
                            json!({"fixture": name, "scenario": scenario, "error": "attempt or group timeout", "failed_after_ms": ms(started)}),
                        )
                    }
                };
                writeln!(
                    output,
                    "{}",
                    json!({"event": "attempt_result", "success": success, "warmup": repetition < 3, "repetition": repetition, "result": result})
                )
                .unwrap();
                output.flush().unwrap();
                assert!(
                    success,
                    "retain the failed attempt and diagnostic log before retrying"
                );
            }
        }
    }
}
