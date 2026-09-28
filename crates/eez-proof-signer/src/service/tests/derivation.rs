//! Cold V0 authorization through the real RPC, execution and attester boundary.

use super::*;
use eez_payload_codec::v0::{self, BlockRun, Payload};
use eez_primitives::Block;

mod captured;
mod synthetic;

fn genesis() -> alloy_genesis::Genesis {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/native-osaka-empty/l2-genesis.json"
    ))
    .unwrap()
}

fn state() -> Arc<ServiceState> {
    Arc::new(
        ServiceState::new(
            crate::service::l1::L1Authority::Synthetic,
            Validator::stateless_for_test(genesis(), TEST_SYSTEM_ADDRESS),
            expected_rollup_id(1),
            test_attester(),
            crate::testkit::l1_chain_spec(),
        )
        .unwrap(),
    )
}

/// Consecutive source-executed empty blocks, each with its own authenticated
/// history-write transition. Repeating an old witness would erase EIP-2935.
fn blocks(count: usize) -> Vec<BlockWitness> {
    let captured = captured_blocks(include_bytes!(
        "../../../tests/fixtures/native-osaka-empty/blocks.protobuf"
    ));
    assert!(
        count > 0 && count <= captured.len(),
        "requested span exceeds captured fixture"
    );
    captured.into_iter().take(count).collect()
}

fn captured_blocks(mut encoded: &[u8]) -> Vec<BlockWitness> {
    use prost::Message as _;
    let mut blocks = Vec::new();
    while !encoded.is_empty() {
        let chunk = ProveChunk::decode_length_delimited(&mut encoded).unwrap();
        let Some(prove_chunk::Kind::Block(block)) = chunk.kind else {
            panic!("expected a block-only capture")
        };
        blocks.push(block);
    }
    blocks
}

#[derive(Clone)]
struct PublishedBlock {
    beneficiary: Address,
    extra_data: Bytes,
    transactions: Vec<Bytes>,
}

fn published_blocks(blocks: &[BlockWitness]) -> Vec<PublishedBlock> {
    use alloy_eips::Encodable2718;
    blocks
        .iter()
        .map(|witness| {
            let block: Block = alloy_rlp::decode_exact(&witness.rlp).unwrap();
            PublishedBlock {
                beneficiary: block.header.beneficiary,
                extra_data: block.header.extra_data.clone(),
                transactions: eez_driver::derivation::ordinary_prefix(&block)
                    .iter()
                    .map(|tx| tx.encoded_2718().into())
                    .collect(),
            }
        })
        .collect()
}

fn encode_published(blocks: &[PublishedBlock]) -> Vec<u8> {
    v0::encode(
        &blocks
            .iter()
            .map(|block| BlockRun {
                blocks: 1,
                beneficiary: block.beneficiary,
                extra_data: &block.extra_data,
                transactions: &block.transactions,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn payload(blocks: &[BlockWitness]) -> Vec<u8> {
    encode_published(&published_blocks(blocks))
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

#[tokio::test]
async fn ordinary_preparation_replays_exact_blocks_caches_facts_and_never_signs() {
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

fn request(blocks: Vec<BlockWitness>, operations: Vec<u8>) -> Vec<ProveChunk> {
    let mut batch = anchor_batch();
    batch.entries[0].rootUpdates[0].currentRoot = B256::from_slice(&blocks[0].parent_hash);
    batch.entries[0].rootUpdates[0].newRoot = B256::from_slice(&blocks.last().unwrap().hash);
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    batch.callData = operations.into();
    let header = ProveHeader {
        parent_provenance: Some(eez_control_rpc::v1::ParentProvenance {
            parent_header_rlp: alloy_rlp::encode(alloy_consensus::Header::default()),
            accepted_l1_number: 0,
            live_slot_endpoint_headers_rlp: Vec::new(),
        }),
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
async fn cold_v0_empty_history_and_terminal_only_reach_the_attester() {
    for count in [1, 3] {
        let blocks = blocks(count);
        let payload = payload(&blocks);
        let state = state();
        let expected_signer = state.attester.address();
        let server = TestServer::new(state.clone()).await;
        let chunks = request(blocks, payload.clone());
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
    let encoded = payload(&blocks);
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
    let encoded = payload(&blocks);
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
    let mut altered = published_blocks(&blocks);
    altered[0].beneficiary = Address::repeat_byte(0x77);
    let cold = request(blocks.clone(), encode_published(&altered));
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
async fn terminal_metadata_is_published_and_independently_bound() {
    let blocks = blocks(3);
    let mut published = published_blocks(&blocks);
    published.last_mut().unwrap().extra_data = Bytes::from_static(b"forged terminal");
    let state = state();
    let server = TestServer::new(state.clone()).await;
    assert_eq!(
        server
            .prove(request(blocks, encode_published(&published)))
            .await
            .code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn new_network_rejects_every_previous_operations_tag_before_signing() {
    let blocks = blocks(2);
    // Historical tag-0 projected shape is not the new RollupX V0 either.
    let previous = [vec![0, 0xc3, 0xc0, 0xc0, 0xc0], vec![1], vec![2], vec![3]];
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
async fn noncanonical_count_columns_never_reach_the_attester() {
    let blocks = blocks(3);
    let canonical = payload(&blocks);
    assert_eq!(&canonical[..3], &[0, 3, 0xc1]);
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for counts in [
        vec![0],
        vec![0xc2],
        vec![0, 0xc0],
        vec![0xff, 3],
        vec![0x80, 1],
        vec![0xff, 0xff, 0xff, 0xff, 0xff, 15],
    ] {
        let operations = [vec![0, 3], counts, canonical[3..].to_vec()].concat();
        assert!(Payload::decode(&operations).is_err());
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
async fn every_published_metadata_mismatch_is_rejected_before_signing() {
    let blocks = blocks(2);
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for position in 0..blocks.len() {
        for field in 0..2 {
            let mut published = published_blocks(&blocks);
            if field == 0 {
                published[position].beneficiary = Address::repeat_byte(0xa1);
            } else {
                published[position].extra_data = Bytes::from_static(b"changed");
            }
            assert_eq!(
                server
                    .prove(request(blocks.clone(), encode_published(&published)))
                    .await
                    .code(),
                Code::FailedPrecondition
            );
            assert_eq!(state.attester.signing_calls(), 0);
        }
    }
}

#[tokio::test]
async fn changed_count_transactions_and_witness_never_sign() {
    let blocks = blocks(3);
    let mut tx_change = published_blocks(&blocks);
    tx_change[1]
        .transactions
        .push(hex::decode(crate::testkit::LEGACY_USER_TX).unwrap().into());
    let mut terminal_change = published_blocks(&blocks);
    terminal_change[2].transactions = tx_change[1].transactions.clone();
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for operations in [
        payload(&blocks[..2]),
        encode_published(&tx_change),
        encode_published(&terminal_change),
    ] {
        assert_eq!(
            server
                .prove(request(blocks.clone(), operations))
                .await
                .code(),
            Code::FailedPrecondition
        );
        assert_eq!(state.attester.signing_calls(), 0);
    }
    let p = payload(&blocks);
    let mut invalid = blocks;
    invalid[0].witness.as_mut().unwrap().state.clear();
    assert_eq!(
        server.prove(request(invalid, p)).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn successful_backend_without_reconstruction_cannot_authorize_v0() {
    let blocks = blocks(2);
    let encoded = payload(&blocks);
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
    let mut output = backend_output_for(&admitted);
    for block in &mut output.blocks {
        block.reconstruction = None;
    }
    let state = Arc::new(
        ServiceState::new(
            crate::service::l1::L1Authority::Synthetic,
            Validator::stub(vec![Ok(output)]),
            expected_rollup_id(1),
            test_attester(),
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

/// Explicit cold-replay feasibility measurement. Supply a current-profile,
/// block-only witness stream through EEZ_TEST_EMPTY_SPAN_FIXTURE. Every position
/// must retain its actual history-write transition; a repeated witness is invalid.
/// This is not a live Sync-grid settlement or deployment-capacity claim.
#[tokio::test]
#[ignore = "explicit long-span cold Prove feasibility measurement"]
async fn cold_span_feasibility() {
    let limits = ServiceLimits::new(ServiceLimitsParams {
        max_window_blocks: nz(65_536),
        max_window_bytes: nz(512 * 1024 * 1024),
        max_window_witness_items: nz(1_000_000),
        stream_idle_timeout: Duration::from_mins(2),
        request_timeout: Duration::from_mins(10),
    })
    .unwrap();
    let state = state();
    let server = TestServer::with_limits(state.clone(), limits).await;
    let path = std::env::var("EEZ_TEST_EMPTY_SPAN_FIXTURE")
        .expect("provide a block-only capture of at least 512 consecutive empty blocks");
    let captured = captured_blocks(&std::fs::read(path).unwrap());
    let counts = [512, 1_024, 4_096, 10_000, 43_200, 65_536]
        .into_iter()
        .filter(|count| *count <= captured.len())
        .collect::<Vec<_>>();
    assert!(
        !counts.is_empty(),
        "fixture does not cover any intended feasibility span"
    );
    for count in counts {
        let build_started = std::time::Instant::now();
        let blocks = captured[..count].to_vec();
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
        let payload = payload(&blocks);
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
