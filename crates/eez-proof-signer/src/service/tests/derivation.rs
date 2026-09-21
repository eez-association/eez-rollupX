//! Cold v3 authorization through the real RPC, execution and attester boundary.

use super::*;
use eez_payload_codec::derivable::{Environment, Payload, Record, RecordData};
use reth_ethereum_primitives::Block;

mod captured;
mod synthetic;

pub(super) fn chain_config() -> alloy_genesis::ChainConfig {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/stateless-block-13/chain-config.json"
    ))
    .unwrap()
}

fn state() -> Arc<ServiceState> {
    Arc::new(
        ServiceState::new(
            Validator::stateless_for_test(chain_config(), TEST_SYSTEM_ADDRESS),
            expected_rollup_id(1),
            test_attester(),
            test_system_transaction_key(),
            crate::testkit::l1_chain_spec(),
        )
        .unwrap(),
    )
}

/// The captured empty block has unchanged state. Subsequent blocks use its
/// independently specified empty transition and the same authenticated absence
/// proofs. No new helper or Reth assembler generates these expected headers;
/// the real backend still validates every proposed block and witness.
pub(super) fn blocks(count: usize) -> Vec<BlockWitness> {
    let mut block: Block = alloy_rlp::decode_exact(include_bytes!(
        "../../../tests/fixtures/stateless-block-13/block-13.rlp"
    ))
    .unwrap();
    let mut witness: alloy_rpc_types_debug::ExecutionWitness = serde_json::from_str(include_str!(
        "../../../tests/fixtures/stateless-block-13/witness-13.json"
    ))
    .unwrap();
    let parent: alloy_consensus::Header =
        alloy_rlp::decode_exact(witness.headers.last().unwrap().as_ref()).unwrap();
    assert!(block.body.transactions.is_empty());
    assert_eq!(parent.state_root, block.header.state_root);
    let mut result = Vec::new();
    for _ in 0..count {
        result.push(BlockWitness {
            number: block.header.number,
            hash: block.header.hash_slow().to_vec(),
            parent_hash: block.header.parent_hash.to_vec(),
            rlp: alloy_rlp::encode(&block),
            witness: Some(ExecutionWitness {
                state: witness.state.iter().map(|b| b.to_vec()).collect(),
                codes: witness.codes.iter().map(|b| b.to_vec()).collect(),
                keys: witness.keys.iter().map(|b| b.to_vec()).collect(),
                headers: witness.headers.iter().map(|b| b.to_vec()).collect(),
            }),
        });
        witness.headers = vec![alloy_rlp::encode(&block.header).into()];
        block.header.parent_hash = block.header.hash_slow();
        block.header.number += 1;
        block.header.timestamp += 1;
        block.header.base_fee_per_gas = block.header.base_fee_per_gas.map(|fee| fee - fee / 8);
    }
    result
}

pub(super) fn environment(blocks: &[BlockWitness]) -> Environment {
    let block: Block = alloy_rlp::decode_exact(&blocks[0].rlp).unwrap();
    Environment {
        timestamp_step_seconds: 1,
        fee_recipient: block.header.beneficiary,
        prev_randao: block.header.mix_hash,
        gas_limit: block.header.gas_limit,
        extra_data: block.header.extra_data,
        cancun_parent_beacon_root: block.header.parent_beacon_block_root.unwrap(),
    }
}

pub(super) fn payload(blocks: &[BlockWitness], env: Environment, records: Vec<Record>) -> Payload {
    Payload::new(
        blocks.len() as u64 - 1,
        env,
        records,
        blocks.last().unwrap().rlp.clone(),
        vec![],
        vec![],
    )
    .unwrap()
}

fn preparation(
    blocks: Vec<BlockWitness>,
    identity: B256,
) -> Vec<eez_control_rpc::v1::PrepareChunk> {
    use eez_control_rpc::v1::{PrepareChunk, PrepareHeader, prepare_chunk};
    let first = blocks.first().unwrap().number;
    let last = blocks.last().unwrap().number;
    std::iter::once(PrepareChunk {
        kind: Some(prepare_chunk::Kind::Header(PrepareHeader {
            rollup_id: 1,
            expected_validation_identity: identity.to_vec(),
            from_block: first,
            to_block: last,
            lease_id: Vec::new(),
        })),
    })
    .chain(blocks.into_iter().map(|block| PrepareChunk {
        kind: Some(prepare_chunk::Kind::Block(block)),
    }))
    .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_preparation_replays_exact_blocks_caches_facts_and_never_signs() {
    let recorder = eez_telemetry::native::prometheus_builder().build_recorder();
    let handle = recorder.handle();
    let _local = metrics::set_default_local_recorder(&recorder);
    eez_telemetry::operational::enable();
    eez_telemetry::operational::initialize_signer_metrics();
    let state = state();
    let identity = state.validation_identity;
    let server = TestServer::new(Arc::clone(&state)).await;
    let blocks = blocks(3);
    let expected_first = blocks.first().unwrap().hash.clone();
    let expected_last = blocks.last().unwrap().hash.clone();
    let response = server
        .client()
        .await
        .prepare_ordinary(tokio_stream::iter(preparation(blocks.clone(), identity)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.validation_identity, identity.to_vec());
    assert_eq!(response.prepared_count, 3);
    assert_eq!(response.first_hash, expected_first);
    assert_eq!(response.last_hash, expected_last);
    assert_eq!(response.lease_id.len(), 32);
    assert_eq!(state.attester.signing_calls(), 0);
    assert_eq!(server.state.attester.signing_calls(), 0);
    assert_eq!(server.state.expected_rollup_id.get(), 1);

    let usage = server
        .client()
        .await
        .capabilities(eez_control_rpc::v1::CapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(usage.preparation_max_blocks, 64);

    let mut renewal = preparation(blocks[..1].to_vec(), identity);
    let Some(eez_control_rpc::v1::prepare_chunk::Kind::Header(header)) = renewal[0].kind.as_mut()
    else {
        unreachable!()
    };
    header.lease_id = response.lease_id.clone();
    let renewed = server
        .client()
        .await
        .prepare_ordinary(tokio_stream::iter(renewal))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(renewed.lease_id, response.lease_id);

    let range = eez_control_rpc::v1::CachedOrdinaryRange {
        validation_identity: identity.to_vec(),
        first_number: blocks[0].number,
        first_hash: blocks[0].hash.clone(),
        last_number: blocks[0].number,
        last_hash: blocks[0].hash.clone(),
    };
    let retained = server
        .client()
        .await
        .retain_ordinary_ranges(eez_control_rpc::v1::RetainOrdinaryRangesRequest {
            validation_identity: identity.to_vec(),
            lease_id: response.lease_id.clone(),
            ranges: vec![range],
            release: false,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(retained.retained_count, 1);
    let released = server
        .client()
        .await
        .retain_ordinary_ranges(eez_control_rpc::v1::RetainOrdinaryRangesRequest {
            validation_identity: identity.to_vec(),
            lease_id: response.lease_id,
            ranges: Vec::new(),
            release: true,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(released.retained_count, 0);
    assert_eq!(state.attester.signing_calls(), 0);
    let rendered = handle.render();
    assert!(rendered.lines().any(|line| line ==
        "eez_stage_duration_seconds_count{stage=\"signer_preparation\",outcome=\"success\"} 2"),
        "{rendered}");
    for outcome in ["success", "internal", "deadline", "panicked", "cancelled"] {
        assert!(
            rendered.lines().any(|line| line
                == format!("eez_signer_authorized_requests_total{{outcome=\"{outcome}\"}} 0")),
            "{rendered}"
        );
    }
    assert!(
        rendered
            .lines()
            .any(|line| line == "eez_signer_requests_total{outcome=\"success\"} 0"),
        "{rendered}"
    );
}

#[tokio::test]
async fn preparation_identity_and_witness_failures_never_publish_or_sign() {
    let state = state();
    let server = TestServer::new(Arc::clone(&state)).await;
    let valid = blocks(1);
    for chunks in [preparation(valid.clone(), B256::repeat_byte(0x42)), {
        let mut invalid = valid.clone();
        invalid[0].witness.as_mut().unwrap().state.clear();
        preparation(invalid, state.validation_identity)
    }] {
        let status = server
            .client()
            .await
            .prepare_ordinary(tokio_stream::iter(chunks))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::FailedPrecondition);
        assert_eq!(state.attester.signing_calls(), 0);
    }
}

pub(super) fn request(blocks: Vec<BlockWitness>, operations: Vec<u8>) -> Vec<ProveChunk> {
    let mut batch = anchor_batch();
    batch.entries[0].rootUpdates[0].currentRoot = B256::from_slice(&blocks[0].parent_hash);
    batch.entries[0].rootUpdates[0].newRoot = B256::from_slice(&blocks.last().unwrap().hash);
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    batch.callData = operations.into();
    let header = ProveHeader {
        expected_validation_identity: Validator::stub(Vec::new())
            .validation_identity(expected_rollup_id(1))
            .to_vec(),
        rollup_id: 1,
        from_block: blocks[0].number,
        to_block: blocks.last().unwrap().number,
        post_batch: Some(public_input_post_batch_for(batch)),
    };
    std::iter::once(ProveChunk {
        kind: Some(prove_chunk::Kind::Header(header)),
    })
    .chain(blocks.into_iter().map(|block| ProveChunk {
        kind: Some(prove_chunk::Kind::Block(block)),
    }))
    .collect()
}

#[tokio::test]
async fn cold_v3_empty_history_and_terminal_only_reach_the_attester() {
    for count in [1, 3] {
        let blocks = blocks(count);
        let payload = payload(&blocks, environment(&blocks), vec![]);
        let state = state();
        let expected_signer = state.attester.address();
        let server = TestServer::new(state.clone()).await;
        let chunks = request(blocks, payload.encode().unwrap());
        let expected = recompute_window_public_inputs_hash(&chunks);
        let response = server.attest(chunks).await;
        assert_attestation(&response, expected, expected_signer);
        assert_eq!(state.attester.signing_calls(), 1);
    }
}

#[tokio::test]
async fn prepared_ordinary_range_matches_cold_digest_with_fresh_terminal_validation() {
    use eez_control_rpc::v1::CachedOrdinaryRange;

    let blocks = blocks(3);
    let encoded = payload(&blocks, environment(&blocks), vec![])
        .encode()
        .unwrap();
    let cold = request(blocks.clone(), encoded);
    let expected = recompute_window_public_inputs_hash(&cold);
    let state = state();
    let server = TestServer::new(Arc::clone(&state)).await;

    let preparation = preparation(blocks[..2].to_vec(), state.validation_identity);
    server
        .client()
        .await
        .prepare_ordinary(tokio_stream::iter(preparation))
        .await
        .unwrap();
    assert_eq!(state.attester.signing_calls(), 0);

    let mut cached = vec![cold[0].clone()];
    cached.push(ProveChunk {
        kind: Some(prove_chunk::Kind::CachedRange(CachedOrdinaryRange {
            validation_identity: state.validation_identity.to_vec(),
            first_number: blocks[0].number,
            first_hash: blocks[0].hash.clone(),
            last_number: blocks[1].number,
            last_hash: blocks[1].hash.clone(),
        })),
    });
    cached.push(cold.last().unwrap().clone());
    let warm = server.attest(cached).await;
    assert_attestation(&warm, expected, state.attester.address());
    assert_eq!(state.attester.signing_calls(), 1);

    let mut mixed = vec![cold[0].clone()];
    mixed.push(ProveChunk {
        kind: Some(prove_chunk::Kind::CachedRange(CachedOrdinaryRange {
            validation_identity: state.validation_identity.to_vec(),
            first_number: blocks[0].number,
            first_hash: blocks[0].hash.clone(),
            last_number: blocks[0].number,
            last_hash: blocks[0].hash.clone(),
        })),
    });
    mixed.extend_from_slice(&cold[2..]);
    let mixed_response = server.attest(mixed).await;
    assert_eq!(mixed_response.public_inputs_hash, warm.public_inputs_hash);

    let cold_response = server.attest(cold).await;
    assert_eq!(warm.public_inputs_hash, cold_response.public_inputs_hash);
    assert_eq!(warm.signature, cold_response.signature);
    assert_eq!(state.attester.signing_calls(), 3);
}

#[tokio::test]
async fn cache_miss_is_structured_and_cached_terminal_is_never_accepted() {
    use eez_control_rpc::v1::{CachedOrdinaryRange, prove_failure};

    let blocks = blocks(3);
    let encoded = payload(&blocks, environment(&blocks), vec![])
        .encode()
        .unwrap();
    let cold = request(blocks.clone(), encoded);
    let state = state();
    let server = TestServer::new(Arc::clone(&state)).await;
    let range = |last: usize| ProveChunk {
        kind: Some(prove_chunk::Kind::CachedRange(CachedOrdinaryRange {
            validation_identity: state.validation_identity.to_vec(),
            first_number: blocks[0].number,
            first_hash: blocks[0].hash.clone(),
            last_number: blocks[last].number,
            last_hash: blocks[last].hash.clone(),
        })),
    };

    let status = server
        .prove(vec![cold[0].clone(), range(1), cold[3].clone()])
        .await;
    assert_eq!(status.code(), Code::FailedPrecondition);
    let failure = eez_control_rpc::decode_prove_failure(status.details()).unwrap();
    let Some(prove_failure::ActionableFailure::CacheMiss(miss)) = failure.actionable_failure else {
        panic!("dedicated cache miss detail")
    };
    assert_eq!(miss.validation_identity, state.validation_identity.to_vec());
    assert_eq!(miss.block_number, blocks[0].number);
    assert_eq!(miss.block_hash, blocks[0].hash);
    assert_eq!(state.attester.signing_calls(), 0);

    let terminal_status = server.prove(vec![cold[0].clone(), range(2)]).await;
    assert_eq!(terminal_status.code(), Code::InvalidArgument);
    assert!(terminal_status.message().contains("terminal block"));
    assert!(terminal_status.details().is_empty());
    assert_eq!(state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn cached_execution_facts_do_not_authorize_a_different_published_environment() {
    use eez_control_rpc::v1::CachedOrdinaryRange;

    let blocks = blocks(3);
    let state = state();
    let server = TestServer::new(Arc::clone(&state)).await;
    server
        .client()
        .await
        .prepare_ordinary(tokio_stream::iter(preparation(
            blocks[..2].to_vec(),
            state.validation_identity,
        )))
        .await
        .unwrap();
    let mut altered = environment(&blocks);
    altered.fee_recipient = Address::repeat_byte(0x77);
    let cold = request(
        blocks.clone(),
        payload(&blocks, altered, vec![]).encode().unwrap(),
    );
    let chunks = vec![
        cold[0].clone(),
        ProveChunk {
            kind: Some(prove_chunk::Kind::CachedRange(CachedOrdinaryRange {
                validation_identity: state.validation_identity.to_vec(),
                first_number: blocks[0].number,
                first_hash: blocks[0].hash.clone(),
                last_number: blocks[1].number,
                last_hash: blocks[1].hash.clone(),
            })),
        },
        cold.last().unwrap().clone(),
    ];
    let status = server.prove(chunks).await;
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn cold_v3_full_ordinary_record_is_still_exactly_bound() {
    let blocks = blocks(3);
    let record = Record {
        empty_gap: 0,
        data: RecordData::FullBlock(blocks[0].rlp.clone().into()),
    };
    let p = payload(&blocks, environment(&blocks), vec![record]);
    let state = state();
    let server = TestServer::new(state.clone()).await;
    let _response = server.attest(request(blocks, p.encode().unwrap())).await;
    assert_eq!(state.attester.signing_calls(), 1);
}

#[tokio::test]
async fn new_network_rejects_every_previous_operations_tag_before_signing() {
    let blocks = blocks(2);
    let previous = [vec![0], vec![1], vec![2]];
    // This uses the shipping service path, without the separate old-fixture
    // oracle selected by the original regression-test helpers.
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for operations in previous {
        assert_eq!(
            server
                .prove(request(blocks.clone(), operations))
                .await
                .code(),
            Code::InvalidArgument
        );
        assert_eq!(state.attester.signing_calls(), 0);
    }
}

#[tokio::test]
async fn every_published_environment_mismatch_is_rejected_before_signing() {
    let blocks = blocks(2);
    let env = environment(&blocks);
    let mut mutations = vec![env; 6];
    mutations[0].timestamp_step_seconds += 1;
    mutations[1].fee_recipient = Address::repeat_byte(0xa1);
    mutations[2].prev_randao = B256::repeat_byte(0xa2);
    mutations[3].gas_limit += 1;
    mutations[4].extra_data = Bytes::from_static(b"changed");
    mutations[5].cancun_parent_beacon_root = B256::repeat_byte(0xa3);
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for env in mutations {
        let p = payload(&blocks, env, vec![]);
        assert_eq!(
            server
                .prove(request(blocks.clone(), p.encode().unwrap()))
                .await
                .code(),
            Code::FailedPrecondition
        );
        assert_eq!(state.attester.signing_calls(), 0);
    }
}

#[tokio::test]
async fn changed_count_transactions_terminal_and_witness_never_sign() {
    let blocks = blocks(3);
    let env = environment(&blocks);
    let mut bad_terminal: Block = alloy_rlp::decode_exact(&blocks[2].rlp).unwrap();
    bad_terminal.header.gas_limit += 1;
    let variants = [
        Payload::new(
            1,
            env.clone(),
            vec![],
            blocks[2].rlp.clone(),
            vec![],
            vec![],
        )
        .unwrap(),
        payload(
            &blocks,
            env.clone(),
            vec![Record {
                empty_gap: 1,
                data: RecordData::Transactions(vec![
                    hex::decode(crate::testkit::SYSTEM_TX).unwrap().into(),
                ]),
            }],
        ),
        Payload::new(
            2,
            env.clone(),
            vec![],
            alloy_rlp::encode(bad_terminal),
            vec![],
            vec![],
        )
        .unwrap(),
        Payload::new(
            2,
            env.clone(),
            vec![],
            blocks[2].rlp.clone(),
            vec![vec![1]],
            vec![],
        )
        .unwrap(),
        Payload::new(
            2,
            env.clone(),
            vec![],
            blocks[2].rlp.clone(),
            vec![],
            vec![1],
        )
        .unwrap(),
    ];
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for p in variants {
        assert_eq!(
            server
                .prove(request(blocks.clone(), p.encode().unwrap()))
                .await
                .code(),
            Code::FailedPrecondition
        );
        assert_eq!(state.attester.signing_calls(), 0);
    }
    let p = payload(&blocks, env, vec![]).encode().unwrap();
    let mut invalid = blocks;
    invalid[0].witness.as_mut().unwrap().state.clear();
    assert_eq!(
        server.prove(request(invalid, p)).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn successful_backend_without_reconstruction_cannot_authorize_v3() {
    let blocks = blocks(2);
    let encoded = payload(&blocks, environment(&blocks), vec![])
        .encode()
        .unwrap();
    let admitted = blocks
        .iter()
        .map(|block| AdmittedBlock {
            declared_number: block.number,
            claimed_hash: B256::from_slice(&block.hash),
            claimed_parent_hash: B256::from_slice(&block.parent_hash),
            rlp: block.rlp.clone(),
            witness: Default::default(),
        })
        .collect::<Vec<_>>();
    let state = Arc::new(
        ServiceState::new(
            Validator::stub(vec![Ok(backend_output_for(&admitted))]),
            expected_rollup_id(1),
            test_attester(),
            test_system_transaction_key(),
            crate::testkit::l1_chain_spec(),
        )
        .unwrap(),
    );
    let server = TestServer::new(state.clone()).await;
    assert_eq!(
        server.prove(request(blocks, encoded)).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 0);
}

/// Early feasibility measurement only: this uses the captured empty-state
/// workload and exact codec counts, not a legal native Sync-grid settlement or
/// a deployment capacity claim. Run explicitly before designing a cache.
#[tokio::test]
#[ignore = "explicit long-span cold Prove feasibility measurement"]
async fn cold_span_feasibility() {
    let limits = ServiceLimits::new(ServiceLimitsParams {
        max_window_blocks: nz(65_536),
        max_window_bytes: nz(512 * 1024 * 1024),
        max_window_witness_items: nz(1_000_000),
        stream_idle_timeout: Duration::from_secs(120),
        request_timeout: Duration::from_secs(600),
    })
    .unwrap();
    let state = state();
    let server = TestServer::with_limits(state.clone(), limits).await;
    for count in [512, 1_024, 4_096, 10_000, 43_200, 65_536] {
        let build_started = std::time::Instant::now();
        let blocks = blocks(count);
        let input_hash = keccak256(
            blocks
                .iter()
                .flat_map(|block| block.hash.iter().copied())
                .collect::<Vec<_>>(),
        );
        let witness_items = blocks
            .iter()
            .map(|block| {
                let witness = block.witness.as_ref().unwrap();
                witness.state.len()
                    + witness.codes.len()
                    + witness.keys.len()
                    + witness.headers.len()
            })
            .sum::<usize>();
        let payload = payload(&blocks, environment(&blocks), vec![])
            .encode()
            .unwrap();
        let operations_bytes = payload.len();
        let chunks = request(blocks, payload);
        let request_bytes = chunks
            .iter()
            .map(prost::Message::encoded_len)
            .sum::<usize>();
        assert!(request_bytes <= limits.window_limits().payload_bytes);
        assert!(witness_items <= limits.window_limits().witness_items);
        let Some(prove_chunk::Kind::Header(header)) = &chunks[0].kind else {
            unreachable!()
        };
        let post_batch = header.post_batch.clone().unwrap();
        let input_build_ms = build_started.elapsed().as_secs_f64() * 1_000.0;
        let started = std::time::Instant::now();
        let response = server.attest(chunks).await;
        let cold_rpc_ms = started.elapsed().as_secs_f64() * 1_000.0;
        // Do not initialize KZG in the client-side oracle before timing the
        // process's first signer request. Only the small header is retained.
        let expected_hash = recompute_test_public_inputs_hash(&post_batch);
        assert_attestation(&response, expected_hash, state.attester.address());
        let process_peak_rss =
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|status| {
                    status
                        .lines()
                        .find(|line| line.starts_with("VmHWM:"))
                        .map(str::to_owned)
                });
        eprintln!(
            "{}",
            serde_json::json!({
            "measurement": "cold_prove_empty_fixture", "logical_blocks": count,
            "first_request_in_process": count == 512,
            "physical_blobs": post_batch.blobs.len(),
                "operations_bytes": operations_bytes, "request_bytes": request_bytes,
                "witness_items": witness_items, "input_hash": input_hash.to_string(),
                "input_build_ms": input_build_ms, "cold_rpc_ms": cold_rpc_ms,
                "combined_harness_process_peak_rss": process_peak_rss,
                "native_grid_settlement": false,
            })
        );
    }
}
