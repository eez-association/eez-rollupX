//! Cold V0 authorization through the real RPC, execution and attester boundary.

use super::*;
use eez_payload_codec::v0::{self, BlockRun, Payload};
use eez_primitives::Block;

mod captured;
mod history_observations;
mod synthetic;

fn genesis() -> alloy_genesis::Genesis {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/native-offline-bidirectional/l2-genesis.json"
    ))
    .unwrap()
}

fn state() -> Arc<ServiceState> {
    Arc::new(
        configured_test_state(
            Address::repeat_byte(0x33),
            Address::repeat_byte(0x44),
            Validator::stateless_for_test(genesis(), TEST_SYSTEM_ADDRESS),
            expected_rollup_id(1),
            Arc::new(reth_chainspec::ChainSpec::from(
                serde_json::from_slice::<alloy_genesis::Genesis>(
                    &std::fs::read(
                        crate::testkit::manifest_dir()
                            .join("tests/fixtures/native-offline-bidirectional/l1-genesis.json"),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
        )
        .unwrap(),
    )
}

/// Complete Sync ranges from a native capture. The terminal carries every
/// remaining bridge header to the captured anchor, even for an early L2 prefix.
fn blocks(sync_intervals: usize) -> Vec<BlockWitness> {
    use eez_control_rpc::v2::client_frame;
    let mut captured = crate::testkit::native_frames("native-offline-bidirectional")
        .into_iter()
        .filter_map(|frame| match frame.kind {
            Some(client_frame::Kind::Block(block)) => block.data,
            _ => None,
        })
        .collect::<Vec<_>>();
    let count = sync_intervals * 5;
    assert!(count > 0 && count <= captured.len());
    let bridge = captured[count..]
        .iter()
        .flat_map(|block| {
            block
                .witness
                .as_ref()
                .unwrap()
                .l1_headers_rlp
                .iter()
                .cloned()
        })
        .collect::<Vec<_>>();
    captured.truncate(count);
    captured
        .last_mut()
        .unwrap()
        .witness
        .as_mut()
        .unwrap()
        .l1_headers_rlp
        .extend(bridge);
    captured
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

fn request(
    state: &ServiceState,
    blocks: Vec<BlockWitness>,
    operations: Vec<u8>,
) -> Vec<WindowChunk> {
    let mut batch = anchor_batch();
    let parent = B256::from_slice(&blocks[0].parent_hash);
    batch.entries[0].rootUpdates[0].currentRoot = parent;
    batch.entries[0].rootUpdates[0].newRoot = B256::from_slice(&blocks.last().unwrap().hash);
    batch.expectedRootPerRollup[0].root = parent;
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    batch.callData = operations.into();
    let endpoint: alloy_consensus::Header = blocks
        .iter()
        .rev()
        .find_map(|block| {
            block
                .witness
                .as_ref()?
                .l1_headers_rlp
                .last()
                .map(|raw| alloy_rlp::decode_exact(raw).unwrap())
        })
        .unwrap();
    let mut post = public_input_post_batch_for(batch);
    post.posting_context = Some(eez_control_rpc::v2::L1PostingContext {
        chain_id: 1337,
        parent_number: endpoint.number,
        parent_hash: endpoint.hash_slow().to_vec(),
        target_timestamp: endpoint.timestamp
            + state.validator.schedule().unwrap().l1_slot_seconds(),
    });
    let header = WindowMetadata {
        parent_header_rlp: blocks[0]
            .witness
            .as_ref()
            .unwrap()
            .headers
            .last()
            .unwrap()
            .clone(),
        rollup_id: 1,
        from_block: blocks[0].number,
        to_block: blocks.last().unwrap().number,
        post_batch: Some(post),
    };
    std::iter::once(WindowChunk {
        kind: Some(WindowPart::Metadata(header)),
    })
    .chain(blocks.into_iter().map(|block| WindowChunk {
        kind: Some(WindowPart::Block(block)),
    }))
    .collect()
}

#[tokio::test]
async fn cold_v0_sync_aligned_ranges_reach_the_attester() {
    for count in [1, 3] {
        let blocks = blocks(count);
        let payload = payload(&blocks);
        let state = state();
        let expected_signer = state.attester.address();
        let server = TestServer::new(state.clone()).await;
        let chunks = request(&state, blocks, payload.clone());
        let expected = recompute_window_public_inputs_for_state(&state, &chunks);
        let response = server.attest(chunks).await;
        assert_attestation(&response, expected, expected_signer);
        assert_eq!(state.attester.signing_calls(), 1);
    }
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
            .prove(request(&state, blocks, encode_published(&published)))
            .await
            .code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn unsupported_operations_tags_are_rejected_before_signing() {
    let blocks = blocks(2);
    let unsupported = [vec![1], vec![2], vec![3], vec![0xff]];
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for operations in unsupported {
        assert_eq!(
            server
                .prove(request(&state, blocks.clone(), operations))
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
    assert_eq!(&canonical[..2], &[0, 15]);
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
        let operations = [vec![0, 15], counts, canonical[3..].to_vec()].concat();
        assert!(Payload::decode(&operations).is_err());
        assert_eq!(
            server
                .prove(request(&state, blocks.clone(), operations))
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
                    .prove(request(
                        &state,
                        blocks.clone(),
                        encode_published(&published)
                    ))
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
    terminal_change.last_mut().unwrap().transactions = tx_change[1].transactions.clone();
    let state = state();
    let server = TestServer::new(state.clone()).await;
    for operations in [
        payload(&blocks[..2]),
        encode_published(&tx_change),
        encode_published(&terminal_change),
    ] {
        assert_eq!(
            server
                .prove(request(&state, blocks.clone(), operations))
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
        server.prove(request(&state, invalid, p)).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 0);
}

#[test]
fn successful_backend_without_reconstruction_is_rejected_by_da_validation() {
    let blocks = blocks(2);
    let encoded = payload(&blocks);
    let admitted = blocks
        .iter()
        .cloned()
        .map(|block| AdmittedBlock::from_wire(block, &mut 0, limits().window_limits()).unwrap())
        .collect::<Vec<_>>();
    let mut output = backend_output_for(&admitted);
    for block in &mut output.blocks {
        block.reconstruction = None;
    }
    let validated = Validator::stub(vec![Ok(output)])
        .validate(&admitted)
        .unwrap();
    let request = request(&state(), blocks, encoded);
    let Some(WindowPart::Metadata(header)) = &request[0].kind else {
        unreachable!()
    };
    let post = header.post_batch.as_ref().unwrap();
    let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
    let context = crate::testkit::proof_context(
        &batch,
        crate::testkit::target_from_wire(post.posting_context.as_ref().unwrap()),
    );
    let error = run_settlement(SettlementInput {
        canonical_batch: batch,
        posting_context: context,
        submitted_blobs: &post.blobs,
        validated_window: &validated,
        expected_rollup_id: expected_rollup_id(1),
        expected_l2_system_address: TEST_SYSTEM_ADDRESS,
        proof_system_vkey: test_proof_system_vkey(),
        expected_proof_system: test_proof_system(),
        system_transaction_reconstructor: &test_system_transaction_reconstructor(
            expected_rollup_id(1),
        ),
        cancellation: &CancellationToken::default(),
    })
    .unwrap_err();
    assert!(
        matches!(
            error,
            SettlementPipelineError::DaPayload(
                settlement::DaPayloadError::MissingReconstruction { .. }
            )
        ),
        "{error:?}"
    );
}
