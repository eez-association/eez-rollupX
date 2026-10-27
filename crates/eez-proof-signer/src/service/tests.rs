//! Shared fixtures and the in-process tonic `Prove` harness for the service
//! test submodules.

mod derivation;
mod pipeline;
mod runtime;
mod settlement_checks;

use alloy_consensus::{SignableTransaction as _, Transaction as _};
use alloy_primitives::{B256, Bytes, Signature, U256, address, b256, keccak256};
use alloy_sol_types::SolValue as _;
use eez_control_rpc::v2::prover_client::ProverClient;
use eez_control_rpc::v2::{BlockWitness, ExecutionWitness, prove_failure};
use eez_control_rpc::v2::{Proof, SettlementInput as WireSettlement};
use eez_protocol::abi::{EtherDeltaSol, ExecutionEntrySol, L2ToL1CallSol, RootUpdateSol};
use eez_protocol::blob_semantics::SemanticTransaction;
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId};
use reth_primitives_traits::BlockBody as _;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Status};

use super::settlement_job::{
    PipelineError, SettlementInput, SettlementPipelineError, run_settlement,
};
use super::worker::WorkerGuard;
use super::*;
use crate::cancel::CancellationToken;
use crate::testkit::{
    TEST_SYSTEM_ADDRESS, checkpoint, system_transaction_context, test_proof_system_vkey,
};
use crate::validate::Validator;
use crate::validate::testing::backend_output_for;
use crate::window::AdmittedBlock;

fn stub_block(number: u64, parent: u8, hash: u8) -> AdmittedBlock {
    let mut block = AdmittedBlock::test(number, parent, hash);
    block.rlp = alloy_rlp::encode(eez_primitives::Block::default());
    block
}

fn nz(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

fn expected_rollup_id(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}

fn test_proof_system() -> Address {
    address!("00000000000000000000000000000000000000aa")
}

fn test_attester() -> crate::attest::Attester {
    test_attester_for(test_proof_system_vkey(), test_proof_system())
}

fn test_attester_for(
    proof_system_vkey: crate::attest::NonZeroProofSystemVkey,
    proof_system: Address,
) -> crate::attest::Attester {
    // Anvil account #1. Test-only and intentionally public.
    let private_key = b256!("59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d");
    crate::attest::Attester::new(
        private_key,
        proof_system_vkey,
        proof_system,
        TEST_SYSTEM_ADDRESS,
    )
    .unwrap()
}

fn test_system_transaction_reconstructor(
    rollup_id: NonZeroU64,
) -> settlement::SystemTransactionReconstructor {
    settlement::SystemTransactionReconstructor::new(1, rollup_id)
}

#[test]
fn posting_rules_use_the_trusted_profile_and_exact_target_fork() {
    let mut genesis = reth_chainspec::MAINNET.genesis.clone();
    genesis.config.cancun_time = Some(10);
    genesis.config.prague_time = Some(20);
    genesis.config.osaka_time = Some(1_000);
    let state = ServiceState::new(
        Address::repeat_byte(0x33),
        Address::repeat_byte(0x44),
        Validator::stub(Vec::new()),
        expected_rollup_id(1),
        test_attester(),
        Arc::new(reth_chainspec::ChainSpec::from(genesis)),
    )
    .unwrap();
    assert_eq!(state.posting_blob_limit(999).unwrap(), 9);
    assert_eq!(state.posting_blob_limit(1_000).unwrap(), 6);
    for invalid in [0, 9] {
        assert_eq!(
            state.posting_blob_limit(invalid).unwrap_err().code(),
            Code::InvalidArgument
        );
    }
}

#[test]
fn first_unsupported_inclusion_rejects_even_with_a_supported_parent() {
    let mut genesis = reth_chainspec::MAINNET.genesis.clone();
    genesis.config.cancun_time = Some(10);
    genesis.config.prague_time = Some(20);
    genesis.config.osaka_time = Some(1_000);
    genesis.config.amsterdam_time = Some(2_000);
    let state = ServiceState::new(
        Address::repeat_byte(0x33),
        Address::repeat_byte(0x44),
        Validator::stub(Vec::new()),
        expected_rollup_id(1),
        test_attester(),
        Arc::new(reth_chainspec::ChainSpec::from(genesis)),
    )
    .unwrap();
    assert_eq!(state.posting_blob_limit(1_999).unwrap(), 6);
    for target in [2_000, 2_001] {
        let error = state.posting_blob_limit(target).unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        assert_eq!(
            error.message(),
            "unsupported L1 rule period at posting target"
        );
    }
}

#[test]
fn service_state_rejects_a_validator_for_another_system_identity() {
    let validator = Validator::stateless_for_test(
        alloy_genesis::Genesis::default(),
        Address::repeat_byte(0xbb),
    );
    let error = ServiceState::new(
        Address::repeat_byte(0x33),
        Address::repeat_byte(0x44),
        validator,
        expected_rollup_id(1),
        test_attester(),
        crate::testkit::l1_chain_spec(),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "validator does not use the native L2 system address"
    );
}

#[test]
fn service_state_rejects_an_attester_bound_to_another_system_identity() {
    let attester = crate::attest::Attester::new(
        b256!("59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"),
        test_proof_system_vkey(),
        test_proof_system(),
        Address::repeat_byte(0xbb),
    )
    .unwrap();

    let error = ServiceState::new(
        Address::repeat_byte(0x33),
        Address::repeat_byte(0x44),
        Validator::stub(Vec::new()),
        expected_rollup_id(1),
        attester,
        crate::testkit::l1_chain_spec(),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "attester does not use the native L2 system address"
    );
}

fn anchor_batch() -> eez_protocol::EvmBatch {
    anchor_batch_for(1)
}

fn anchor_batch_for(rollup_id: u64) -> eez_protocol::EvmBatch {
    let mut batch = eez_protocol::EvmBatch {
        blockNumber: u64::MAX,
        expectedRootPerRollup: vec![eez_protocol::abi::ExpectedRootPerRollupSol {
            rollupId: rollup_id,
            root: B256::repeat_byte(0x04),
        }],
        ..Default::default()
    };
    batch.entries.push(ExecutionEntrySol {
        rootUpdates: vec![RootUpdateSol {
            rollupId: rollup_id,
            currentRoot: B256::ZERO,
            newRoot: B256::ZERO,
            etherDelta: EtherDeltaSol::ZERO,
        }],
        proxyEntryHash: B256::ZERO,
        destinationRollupId: rollup_id,
        l2ToL1Calls: Vec::new(),
        expectedL1ToL2Calls: Vec::new(),
        success: true,
        returnData: Bytes::new(),
        rollingHash: B256::ZERO,
    });
    batch.immediateEntryCount = U256::from(1);
    batch.proofSystems = vec![test_proof_system()];
    batch.rollupIdsWithProofSystems = vec![eez_protocol::abi::RollupIdWithProofSystemsSol {
        rollupId: rollup_id,
        proofSystemIndexes: vec![0],
    }];
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    batch
}

fn outbound_batch(
    anchor_root: B256,
    pre_settling_root: B256,
    final_root: B256,
) -> eez_protocol::EvmBatch {
    let mut batch = anchor_batch();
    let anchor = &mut batch.entries[0];
    anchor.rootUpdates[0].currentRoot = anchor_root;
    anchor.rootUpdates[0].newRoot = pre_settling_root;

    let mut effect = anchor.clone();
    effect.rootUpdates[0].currentRoot = pre_settling_root;
    effect.rootUpdates[0].newRoot = final_root;
    effect.l2ToL1Calls.push(l2_to_l1_call());
    batch.entries.push(effect);
    batch.immediateEntryCount = U256::from(2);
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    batch
}

fn l2_to_l1_call() -> L2ToL1CallSol {
    L2ToL1CallSol {
        revertNextNCalls: 0,
        isStatic: false,
        gas: 0,
        sourceAddress: Address::ZERO,
        sourceRollupId: 1,
        targetAddress: Address::ZERO,
        value: U256::ZERO,
        data: Bytes::new(),
    }
}

fn flat_outbound_sidecar(entry: &ExecutionEntrySol) -> ExecutionEntrySol {
    let root = &entry.l2ToL1Calls[0];
    let proxy_entry_hash = eez_protocol::l2_outbound_call_hash(
        eez_protocol::CallHashInput {
            call_mode: eez_protocol::CallMode::Mutable,
            source_address: root.sourceAddress,
            source_rollup_id: eez_protocol::RollupId(root.sourceRollupId),
            target_address: root.targetAddress,
            target_rollup_id: eez_protocol::RollupId::MAINNET,
            value: root.value,
            data: &root.data,
        },
        0,
    );
    ExecutionEntrySol {
        proxyEntryHash: proxy_entry_hash,
        rollingHash: eez_protocol::EntryRollingHash::seed_for_l2(proxy_entry_hash).current(),
        destinationRollupId: eez_protocol::RollupId::MAINNET.0,
        success: entry.success,
        returnData: entry.returnData.clone(),
        ..Default::default()
    }
}

/// Canonical one-pair Sync block and the batch that commits to it.
fn canonical_outbound_case() -> (eez_protocol::EvmBatch, Vec<u8>, Vec<u8>, B256) {
    outbound_case(U256::ZERO)
}

fn outbound_case(value: U256) -> (eez_protocol::EvmBatch, Vec<u8>, Vec<u8>, B256) {
    let parent_hash = B256::repeat_byte(0x04);
    let block_hash = B256::repeat_byte(0x05);
    let mut batch = outbound_batch(parent_hash, parent_hash, block_hash);
    batch.entries[1].l2ToL1Calls[0].value = value;
    batch.entries[1].rootUpdates[0].etherDelta = -EtherDeltaSol::from(value);
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    let call = &batch.entries[1].l2ToL1Calls[0];
    let call_hash = eez_protocol::l2_outbound_call_hash(
        eez_protocol::CallHashInput {
            call_mode: eez_protocol::CallMode::Mutable,
            source_address: call.sourceAddress,
            source_rollup_id: eez_protocol::RollupId(1),
            target_address: call.targetAddress,
            target_rollup_id: eez_protocol::RollupId::MAINNET,
            value: call.value,
            data: &call.data,
        },
        0,
    );
    let user_body: eez_primitives::BlockBody = alloy_consensus::BlockBody {
        transactions: vec![non_system_transaction()],
        ..Default::default()
    };
    let user = user_body.encoded_2718_transactions_iter().next().unwrap();
    let sidecar = flat_outbound_sidecar(&batch.entries[1]);
    let pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &[(sidecar.clone(), Bytes::from(user.clone()))],
        &[],
        &system_transaction_context(),
    )
    .unwrap();
    let transactions = crate::testkit::resolve_synthetic_plans(
        &pairs,
        &[crate::testkit::native_source_context()
            .entry(1)
            .unwrap()
            .hash(B256::repeat_byte(0x04))],
    )
    .into_iter()
    .map(|raw| {
        <TestTransaction as alloy_eips::Decodable2718>::decode_2718_exact(raw.as_ref()).unwrap()
    })
    .collect();
    let body: eez_primitives::BlockBody = alloy_consensus::BlockBody {
        transactions,
        ..Default::default()
    };
    let block = eez_primitives::Block::new(
        alloy_consensus::Header {
            gas_limit: crate::testkit::SYNTHETIC_BLOCK_GAS_LIMIT,
            ..Default::default()
        },
        body,
    );
    batch.callData = settlement::encode_da_payload(&[Vec::new()]).into();
    (batch, alloy_rlp::encode(block), user, call_hash)
}

fn outbound_backend_output() -> validate::BackendWindowOutput {
    let inputs = [stub_block(5, 0x04, 0x05)];
    let mut backend_output = backend_output_for(&inputs);
    backend_output.blocks[0].set_transaction_results_for_test(vec![true, true]);
    backend_output.blocks[0].transaction_state_checkpoints =
        vec![checkpoint(1, B256::repeat_byte(0x05))];
    backend_output.blocks[0]
        .settlement_evidence
        .set_system_sender_flags_for_test(vec![true, false]);
    backend_output
}

fn outbound_evidence(call_hash: B256) -> validate::SettlementBlockEvidence {
    validate::SettlementBlockEvidence::for_test(
        vec![true, false],
        vec![validate::OutboundEventObservation::decoded_for_test(
            1, 0, call_hash, 0,
        )],
    )
    .with_ordinary_prefix_for_test(B256::repeat_byte(0x04))
}

fn mixed_outbound_inbound_case() -> (eez_protocol::EvmBatch, Vec<u8>, B256) {
    let (mut batch, _outbound_block, user, outbound_call_hash) = canonical_outbound_case();
    let outbound_prefix_hash = B256::repeat_byte(0x44);
    let final_block_hash = B256::repeat_byte(0x05);
    let value = U256::from(9);
    let (_unused_standalone_tx, inbound_call_hash, return_data, inbound_sidecar) =
        strict_inbound_transaction(value);

    let mut inbound_entry = batch.entries[0].clone();
    batch.entries[1].rootUpdates[0].newRoot = outbound_prefix_hash;
    inbound_entry.rootUpdates[0].currentRoot = outbound_prefix_hash;
    inbound_entry.rootUpdates[0].newRoot = final_block_hash;
    inbound_entry.rootUpdates[0].etherDelta = EtherDeltaSol::from(value);
    inbound_entry.proxyEntryHash = inbound_call_hash;
    inbound_entry.returnData = return_data;
    batch.entries.push(inbound_entry);
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();

    let outbound_sidecar = flat_outbound_sidecar(&batch.entries[1]);
    let pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &[(outbound_sidecar.clone(), Bytes::from(user.clone()))],
        &[eez_protocol::entries::InboundSidecar::try_from(&inbound_sidecar).unwrap()],
        &system_transaction_context(),
    )
    .unwrap();
    let transactions = crate::testkit::resolve_synthetic_plans(
        &pairs,
        &[
            crate::testkit::native_source_context()
                .entry(1)
                .unwrap()
                .hash(B256::repeat_byte(0x04)),
            crate::testkit::native_source_context()
                .entry(2)
                .unwrap()
                .hash(outbound_prefix_hash),
        ],
    )
    .into_iter()
    .map(|raw| {
        <TestTransaction as alloy_eips::Decodable2718>::decode_2718_exact(raw.as_ref()).unwrap()
    })
    .collect();
    let body: eez_primitives::BlockBody = alloy_consensus::BlockBody {
        transactions,
        ..Default::default()
    };
    let block = eez_primitives::Block::new(
        alloy_consensus::Header {
            gas_limit: crate::testkit::SYNTHETIC_BLOCK_GAS_LIMIT,
            ..Default::default()
        },
        body,
    );
    batch.callData = settlement::encode_da_payload(&[Vec::new()]).into();

    (batch, alloy_rlp::encode(block), outbound_call_hash)
}

fn mixed_backend_output() -> validate::BackendWindowOutput {
    let inputs = [stub_block(5, 0x04, 0x05)];
    let mut backend_output = backend_output_for(&inputs);
    backend_output.blocks[0].set_transaction_results_for_test(vec![true, true, true]);
    backend_output.blocks[0].transaction_state_checkpoints = vec![
        checkpoint(1, B256::repeat_byte(0x44)),
        checkpoint(2, B256::repeat_byte(0x05)),
    ];
    backend_output
}

fn mixed_evidence(outbound_call_hash: B256) -> validate::SettlementBlockEvidence {
    validate::SettlementBlockEvidence::for_test(
        vec![true, false, true],
        vec![validate::OutboundEventObservation::decoded_for_test(
            1,
            0,
            outbound_call_hash,
            0,
        )],
    )
    .with_ordinary_prefix_for_test(B256::repeat_byte(0x04))
}

fn single_block_settlement_window(
    batch: eez_protocol::EvmBatch,
    block_rlp: Vec<u8>,
) -> Vec<WindowChunk> {
    let semantic_transactions = fixture_semantics(&batch, &block_rlp);
    let mut block = block_chunk(5, 0x04, 0x05);
    block_mut(&mut block).rlp = block_rlp;
    let mut window = vec![header_chunk(5, 5), block];
    replace_post_batch(
        &mut window,
        public_input_post_batch_for_semantics(batch, &semantic_transactions),
    );
    window
}

// Mutable case data shared by the execution and pure settlement regressions.
// TestServer emits only actual v2 frames; these types have no wire codec.
#[derive(Clone)]
struct WindowChunk {
    kind: Option<WindowPart>,
}

#[derive(Clone)]
enum WindowPart {
    Metadata(WindowMetadata),
    Block(BlockWitness),
}

#[derive(Clone)]
struct WindowMetadata {
    rollup_id: u64,
    from_block: u64,
    to_block: u64,
    parent_header_rlp: Vec<u8>,
    post_batch: Option<WireSettlement>,
}

fn header_chunk(from: u64, to: u64) -> WindowChunk {
    let block_count = to
        .checked_sub(from)
        .and_then(|distance| distance.checked_add(1))
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or_default();
    WindowChunk {
        kind: Some(WindowPart::Metadata(WindowMetadata {
            parent_header_rlp: alloy_rlp::encode(alloy_consensus::Header::default()),
            rollup_id: 1,
            from_block: from,
            to_block: to,
            post_batch: Some(public_input_post_batch_for_empty_blocks(
                anchor_batch(),
                block_count,
            )),
        })),
    }
}

fn public_input_post_batch() -> WireSettlement {
    public_input_post_batch_for_empty_blocks(anchor_batch(), 3)
}

fn public_input_post_batch_for(batch: eez_protocol::EvmBatch) -> WireSettlement {
    public_input_post_batch_for_semantics(batch, &[])
}

fn public_input_post_batch_for_semantics(
    mut batch: eez_protocol::EvmBatch,
    semantic_transactions: &[SemanticTransaction],
) -> WireSettlement {
    let rollup_id = batch
        .rollupIdsWithProofSystems
        .first()
        .map_or(1, |assignment| assignment.rollupId);
    let operations = std::mem::take(&mut batch.callData);
    let semantic_messages = semantic_transactions
        .iter()
        .map(eez_protocol::blob_semantics::encode_semantic_transaction)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .flatten()
        .collect();
    let blobs =
        eez_blob_codec::encode_native_payload(rollup_id, operations, semantic_messages).unwrap();
    batch.blobIndices = (0..blobs.len()).map(U256::from).collect();
    WireSettlement {
        abi_calldata: eez_protocol::entries::encode_postbatch(&batch),
        blobs: blobs
            .into_iter()
            .map(|blob| blob.into_bytes().into_vec())
            .collect(),
        posting_context: Some(crate::testkit::posting_context()),
    }
}

fn public_input_post_batch_for_empty_blocks(
    mut batch: eez_protocol::EvmBatch,
    block_count: usize,
) -> WireSettlement {
    bind_anchor_commitments(&mut batch, block_count);
    batch.callData = settlement::encode_da_payload(&vec![Vec::new(); block_count.max(1)]).into();
    public_input_post_batch_for(batch)
}

/// Bind an anchor-only fixture to the synthetic window hashes used by
/// `block_chunk`: parent `0x04..04`, followed by `block_count` sequential
/// block hashes. State roots in the stub backend deliberately do not
/// participate in this commitment.
fn bind_anchor_commitments(batch: &mut eez_protocol::EvmBatch, block_count: usize) {
    assert_eq!(
        batch.entries.len(),
        1,
        "fixture must contain one anchor entry"
    );
    let final_hash_byte = u8::try_from(4usize.checked_add(block_count).unwrap()).unwrap();
    let update = &mut batch.entries[0].rootUpdates[0];
    update.currentRoot = B256::repeat_byte(0x04);
    update.newRoot = B256::repeat_byte(final_hash_byte);
    eez_protocol::entries::finalize_l1_rolling_hashes(batch).unwrap();
}

fn recompute_test_public_inputs_hash(post_batch: &WireSettlement) -> B256 {
    let batch = settlement::decode_canonical_post_batch(post_batch.abi_calldata.clone()).unwrap();
    let verified_da = settlement::verify_da_carrier(&batch, &post_batch.blobs, 1).unwrap();
    settlement::CheckedPublicInputProfile::new(&batch, expected_rollup_id(1), test_proof_system())
        .unwrap()
        .recompute_public_inputs_hash(
            test_proof_system_vkey(),
            verified_da.transaction_blob_hashes(),
            &crate::testkit::offline_context(
                &batch,
                crate::testkit::target_from_wire(post_batch.posting_context.as_ref().unwrap()),
            ),
        )
        .unwrap()
        .into_inner()
}

fn recompute_window_public_inputs_hash(window: &[WindowChunk]) -> B256 {
    let Some(WindowPart::Metadata(header)) = &window[0].kind else {
        unreachable!("fixture starts with a header")
    };
    recompute_test_public_inputs_hash(header.post_batch.as_ref().unwrap())
}

fn recompute_window_public_inputs_for_state(state: &ServiceState, window: &[WindowChunk]) -> B256 {
    let Some(WindowPart::Metadata(header)) = &window[0].kind else {
        panic!("fixture header")
    };
    let post = header.post_batch.as_ref().unwrap();
    let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
    let endpoint: alloy_consensus::Header = window[1..]
        .iter()
        .rev()
        .find_map(|chunk| {
            let Some(WindowPart::Block(block)) = &chunk.kind else {
                return None;
            };
            block
                .witness
                .as_ref()?
                .l1_headers_rlp
                .last()
                .map(|raw| alloy_rlp::decode_exact(raw).unwrap())
        })
        .unwrap();
    let anchor = match &post.posting_context {
        Some(posting) => eez_protocol::l1_history::EvidenceAnchor::ActualParent {
            number: posting.parent_number,
            hash: B256::from_slice(&posting.parent_hash),
            target_timestamp: posting.target_timestamp,
        },
        None => eez_protocol::l1_history::EvidenceAnchor::Historical {
            number: batch.blockNumber,
            hash: endpoint.hash_slow(),
        },
    };
    let context = eez_protocol::l1_history::OfflineContext::new(
        &state.deployment.profile,
        state.deployment.registry,
        state.deployment.manager,
        header.rollup_id,
        batch.expectedRootPerRollup[0].root,
        anchor,
        endpoint.timestamp,
    )
    .unwrap();
    let carrier =
        settlement::verify_da_carrier_with_limit(&batch, &post.blobs, header.rollup_id, 16)
            .unwrap();
    settlement::CheckedPublicInputProfile::new(
        &batch,
        state.expected_rollup_id,
        state.attester.expected_proof_system(),
    )
    .unwrap()
    .recompute_public_inputs_hash(
        state.attester.proof_system_vkey(),
        carrier.transaction_blob_hashes(),
        &context,
    )
    .unwrap()
    .into_inner()
}

fn fixture_action(
    source_rollup_id: RollupId,
    target_rollup_id: RollupId,
    source_address: Address,
    target_address: Address,
    value: U256,
    data: Bytes,
    return_data: Bytes,
    success: bool,
) -> ExecutedAction {
    ExecutedAction {
        parent_index: None,
        call_mode: CallMode::Mutable,
        target_address,
        target_rollup_id,
        source_rollup_id,
        source_address,
        data,
        value,
        outcome: ExecutionOutcome::Resolved {
            return_data: return_data.to_vec(),
            gas_used: 0,
            success,
        },
        revert_span: None,
    }
}

fn fixture_semantics(batch: &eez_protocol::EvmBatch, block_rlp: &[u8]) -> Vec<SemanticTransaction> {
    let block: eez_primitives::Block = alloy_rlp::decode_exact(block_rlp).unwrap();
    let encoded_transactions = block
        .body
        .encoded_2718_transactions_iter()
        .collect::<Vec<_>>();
    let immediate_count = usize::try_from(batch.immediateEntryCount).unwrap();
    let outbound_entries = batch.entries[1..immediate_count]
        .iter()
        .filter(|entry| !entry.l2ToL1Calls.is_empty())
        .collect::<Vec<_>>();
    assert!(
        outbound_entries.len() <= 1,
        "service fixture synthesizer supports one outbound transaction"
    );

    let mut semantic = Vec::new();
    if let Some(entry) = outbound_entries.first() {
        let raw_user = encoded_transactions
            .get(1)
            .expect("outbound fixture has [load, user]");
        semantic.push(SemanticTransaction {
            origin_chain: RollupId(1),
            tx_data: Bytes::copy_from_slice(raw_user),
            actions: entry
                .l2ToL1Calls
                .iter()
                .map(|call| {
                    fixture_action(
                        RollupId(call.sourceRollupId),
                        RollupId::MAINNET,
                        call.sourceAddress,
                        call.targetAddress,
                        call.value,
                        call.data.clone(),
                        entry.returnData.clone(),
                        entry.success,
                    )
                })
                .collect(),
        });
    }

    for transaction in &block.body.transactions {
        let Some(decoded) = eez_protocol::entries::decode_inbound(transaction.input()) else {
            continue;
        };
        semantic.push(SemanticTransaction {
            origin_chain: RollupId::MAINNET,
            tx_data: Bytes::new(),
            actions: vec![fixture_action(
                RollupId::MAINNET,
                RollupId(1),
                decoded.source,
                decoded.target,
                decoded.value,
                decoded.data,
                decoded.return_data,
                decoded.success,
            )],
        });
    }
    semantic
}

#[track_caller]
fn assert_attestation(response: &Proof, expected_hash: B256, expected_signer: Address) {
    assert_eq!(response.public_inputs_hash, expected_hash.as_slice());
    assert_eq!(response.proof.len(), 65);
    let signature = Signature::try_from(response.proof.as_slice()).unwrap();
    assert_eq!(
        signature
            .recover_address_from_prehash(&expected_hash)
            .unwrap(),
        expected_signer
    );
}

fn replace_post_batch(window: &mut [WindowChunk], post_batch: WireSettlement) {
    header_mut(&mut window[0]).post_batch = Some(post_batch);
}

#[track_caller]
fn header_mut(chunk: &mut WindowChunk) -> &mut WindowMetadata {
    let Some(WindowPart::Metadata(header)) = &mut chunk.kind else {
        unreachable!("test chunk must be a header");
    };
    header
}

#[track_caller]
fn block_mut(chunk: &mut WindowChunk) -> &mut BlockWitness {
    let Some(WindowPart::Block(block)) = &mut chunk.kind else {
        unreachable!("test chunk must be a block");
    };
    block
}

fn da_payload_for_window(window: &[WindowChunk]) -> Vec<u8> {
    let blocks = window
        .iter()
        .skip(1)
        .map(|chunk| {
            let Some(WindowPart::Block(block)) = &chunk.kind else {
                panic!("test window contains a non-block chunk after its header");
            };
            let block = alloy_rlp::decode_exact::<eez_primitives::Block>(&block.rlp).unwrap();
            block.body.encoded_2718_transactions_iter().collect()
        })
        .collect::<Vec<_>>();
    settlement::encode_da_payload(&blocks)
}

fn replace_batch_bound_to_window(window: &mut [WindowChunk], mut batch: eez_protocol::EvmBatch) {
    bind_anchor_commitments(&mut batch, window.len() - 1);
    batch.callData = da_payload_for_window(window).into();
    replace_post_batch(window, public_input_post_batch_for(batch));
}

fn block_chunk(number: u64, parent: u8, hash: u8) -> WindowChunk {
    let block =
        eez_primitives::Block::new(Default::default(), eez_primitives::BlockBody::default());
    WindowChunk {
        kind: Some(WindowPart::Block(BlockWitness {
            number,
            hash: vec![hash; 32],
            parent_hash: vec![parent; 32],
            rlp: alloy_rlp::encode(block),
            witness: Some(ExecutionWitness {
                state: vec![vec![number as u8]],
                ..ExecutionWitness::default()
            }),
        })),
    }
}

/// A chained happy-path window: header(5..=7) + blocks 5, 6, 7.
fn happy_window() -> Vec<WindowChunk> {
    vec![
        header_chunk(5, 7),
        block_chunk(5, 0x04, 0x05),
        block_chunk(6, 0x05, 0x06),
        block_chunk(7, 0x06, 0x07),
    ]
}

fn stateless_window() -> Vec<WindowChunk> {
    let consensus_header = alloy_consensus::Header {
        number: 5,
        parent_hash: alloy_primitives::B256::repeat_byte(0x04),
        ..Default::default()
    };
    let hash = consensus_header.hash_slow();
    let consensus_block = alloy_consensus::Block::<
        alloy_consensus::EthereumTxEnvelope<alloy_consensus::TxEip4844>,
    >::new(consensus_header, Default::default());
    let mut chunk = block_chunk(5, 0x04, 0x05);
    let block = block_mut(&mut chunk);
    block.hash = hash.to_vec();
    block.rlp = alloy_rlp::encode(consensus_block);
    vec![header_chunk(5, 5), chunk]
}

type TestTransaction = eez_primitives::EezTxEnvelope;

fn single_non_system_transaction_window() -> Vec<WindowChunk> {
    single_transaction_window(non_system_transaction())
}

fn non_system_transaction() -> TestTransaction {
    alloy_consensus::TxLegacy::default()
        .into_signed(alloy_primitives::Signature::test_signature())
        .into()
}

fn single_system_transaction_window() -> Vec<WindowChunk> {
    single_transaction_window(system_transaction())
}

fn system_transaction() -> TestTransaction {
    alloy_rlp::decode_exact(hex::decode(crate::testkit::SYSTEM_TX).unwrap()).unwrap()
}

fn strict_inbound_transaction(value: U256) -> (TestTransaction, B256, Bytes, ExecutionEntrySol) {
    let target = alloy_primitives::address!("00000000000000000000000000000000000000aa");
    let source = alloy_primitives::address!("00000000000000000000000000000000000000bb");
    let data = Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]);
    let return_data = Bytes::from_static(&[0x01, 0x02]);
    let entry =
        eez_protocol::entries::build_l2_incoming_entry(eez_protocol::entries::IncomingEntry {
            target,
            source,
            value,
            data: data.clone(),
            source_rollup_id: eez_protocol::RollupId(0),
            l2_rollup_id: eez_protocol::RollupId(1),
            return_data: return_data.clone(),
            success: true,
        })
        .unwrap();
    let call_hash = entry.proxyEntryHash;
    let sidecar = ExecutionEntrySol {
        rootUpdates: Vec::new(),
        proxyEntryHash: call_hash,
        l2ToL1Calls: vec![L2ToL1CallSol {
            revertNextNCalls: 0,
            isStatic: false,
            gas: 0,
            sourceAddress: source,
            sourceRollupId: 0,
            targetAddress: target,
            value,
            data: data.clone(),
        }],
        expectedL1ToL2Calls: Vec::new(),
        rollingHash: entry.rollingHash,
        destinationRollupId: 1,
        success: true,
        returnData: return_data.clone(),
    };
    let input = eez_protocol::entries::encode_execute_incoming(entry);
    let context = system_transaction_context();
    let raw = eez_protocol::system_tx::build_inbound_system_txs(
        &[eez_protocol::entries::InboundSidecar::try_from(&sidecar).unwrap()],
        &context,
    )
    .unwrap()
    .remove(0)
    .at_position(
        crate::testkit::SYNTHETIC_BLOCK_GAS_LIMIT,
        0,
        crate::testkit::native_source_context()
            .entry(1)
            .unwrap()
            .hash(B256::repeat_byte(0x04)),
    )
    .unwrap();
    let transaction = TestTransaction::from(raw);
    assert_eq!(transaction.input(), input.as_slice());
    (transaction, call_hash, return_data, sidecar)
}

fn single_transaction_window(transaction: TestTransaction) -> Vec<WindowChunk> {
    vec![
        header_chunk(5, 5),
        transaction_block_chunk(5, 0x04, 0x05, transaction),
    ]
}

fn two_block_transaction_window(
    first: TestTransaction,
    settling: TestTransaction,
) -> Vec<WindowChunk> {
    vec![
        header_chunk(5, 6),
        transaction_block_chunk(5, 0x04, 0x05, first),
        transaction_block_chunk(6, 0x05, 0x06, settling),
    ]
}

fn transaction_block_chunk(
    number: u64,
    parent: u8,
    hash: u8,
    transaction: TestTransaction,
) -> WindowChunk {
    transactions_block_chunk(number, parent, hash, vec![transaction])
}

fn transactions_block_chunk(
    number: u64,
    parent: u8,
    hash: u8,
    transactions: Vec<TestTransaction>,
) -> WindowChunk {
    let body: eez_primitives::BlockBody = alloy_consensus::BlockBody {
        transactions,
        ..Default::default()
    };
    let consensus_block = eez_primitives::Block::new(
        alloy_consensus::Header {
            gas_limit: crate::testkit::SYNTHETIC_BLOCK_GAS_LIMIT,
            ..Default::default()
        },
        body,
    );
    let mut chunk = block_chunk(number, parent, hash);
    block_mut(&mut chunk).rlp = alloy_rlp::encode(consensus_block);
    chunk
}

/// The admitted-block shape of [`happy_window`], for stub backend-output construction.
fn happy_block_inputs() -> Vec<AdmittedBlock> {
    [(5u64, 0x04u8, 0x05u8), (6, 0x05, 0x06), (7, 0x06, 0x07)]
        .into_iter()
        .map(|(number, parent_hash, hash)| stub_block(number, parent_hash, hash))
        .collect()
}

fn limits() -> ServiceLimits {
    limits_with(
        16,
        1024 * 1024,
        Duration::from_secs(5),
        Duration::from_secs(30),
    )
}

fn limits_with(
    max_blocks: usize,
    max_bytes: usize,
    idle_timeout: Duration,
    request_timeout: Duration,
) -> ServiceLimits {
    ServiceLimits::new(ServiceLimitsParams {
        max_window_blocks: nz(max_blocks),
        max_window_bytes: nz(max_bytes),
        max_window_witness_items: nz(1024),
        stream_idle_timeout: idle_timeout,
        request_timeout,
    })
    .unwrap()
}

fn request_frames(chunks: &[WindowChunk]) -> Vec<eez_control_rpc::v2::client_frame::Kind> {
    use eez_control_rpc::v2::{self, client_frame};
    let Some(WindowPart::Metadata(header)) = &chunks[0].kind else {
        panic!("fixture has no range metadata");
    };
    let blocks = chunks[1..]
        .iter()
        .map(|chunk| {
            let Some(WindowPart::Block(block)) = &chunk.kind else {
                panic!("offline fixture must supply every block");
            };
            block.clone()
        })
        .collect::<Vec<_>>();
    let parent: alloy_consensus::Header =
        alloy_rlp::decode_exact(&header.parent_header_rlp).unwrap();
    let post = header.post_batch.as_ref().unwrap();
    let mut frames = vec![client_frame::Kind::Begin(v2::Begin {
        rollup_id: header.rollup_id,
        anchor_number: parent.number,
        anchor_hash: parent.hash_slow().to_vec(),
        anchor_state_root: parent.state_root.to_vec(),
    })];
    frames.extend(
        blocks
            .iter()
            .cloned()
            .map(|data| client_frame::Kind::Block(v2::Block { data: Some(data) })),
    );
    frames.push(client_frame::Kind::Finalize(v2::Finalize {
        from_block: header.from_block,
        to_block: header.to_block,
        terminal_hash: blocks.last().unwrap().hash.clone(),
        settlement: Some(v2::SettlementInput {
            abi_calldata: post.abi_calldata.clone(),
            blobs: post.blobs.clone(),
            posting_context: post
                .posting_context
                .as_ref()
                .map(|context| v2::L1PostingContext {
                    chain_id: context.chain_id,
                    target_timestamp: context.target_timestamp,
                    parent_number: context.parent_number,
                    parent_hash: context.parent_hash.clone(),
                }),
        }),
    }));
    frames
}

/// One in-process server with shared client setup and drop-triggered shutdown.
struct TestServer {
    state: Arc<ServiceState>,
    endpoint: String,
    _shutdown: oneshot::Sender<()>,
}

impl TestServer {
    async fn new(state: Arc<ServiceState>) -> Self {
        Self::with_limits(state, limits()).await
    }

    async fn with_limits(state: Arc<ServiceState>, limits: ServiceLimits) -> Self {
        Self::with_service(ProveSvc::new(state, limits)).await
    }

    async fn with_service(svc: ProveSvc) -> Self {
        let state = Arc::clone(&svc.state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(svc.into_server())
                .serve_with_incoming_shutdown(
                    tokio_stream::wrappers::TcpListenerStream::new(listener),
                    async {
                        let _ = shutdown_rx.await;
                    },
                )
                .await
                .expect("test Prove server failed");
        });
        Self {
            state,
            endpoint: format!("http://{addr}"),
            _shutdown: shutdown,
        }
    }

    async fn client(&self) -> ProverClient<tonic::transport::Channel> {
        ProverClient::connect(self.endpoint.clone()).await.unwrap()
    }

    async fn exchange(&self, chunks: Vec<WindowChunk>) -> Result<Proof, Status> {
        use eez_control_rpc::v2::{self, server_frame};
        let frames = request_frames(&chunks);
        let mut client = v2::prover_client::ProverClient::connect(self.endpoint.clone())
            .await
            .unwrap()
            .max_encoding_message_size(MAX_DECODING_MESSAGE_BYTES)
            .max_decoding_message_size(MAX_DECODING_MESSAGE_BYTES);
        let (tx, rx) = mpsc::channel(1);
        let mut request = tonic::Request::new(ReceiverStream::new(rx));
        request.metadata_mut().insert_bin(
            "eez-validation-identity-bin",
            tonic::metadata::MetadataValue::from_bytes(self.state.validation_identity.as_slice()),
        );
        let mut stream = client.prove_stream(request).await?.into_inner();
        let mut session_id = Vec::new();
        let mut epoch = 0;
        for (index, kind) in frames.into_iter().enumerate() {
            let request_id = index as u64 + 1;
            tx.send(v2::ClientFrame {
                session_id: session_id.clone(),
                epoch,
                request_id,
                kind: Some(kind),
            })
            .await
            .map_err(|_| Status::cancelled("test stream closed before send"))?;
            let response = stream
                .message()
                .await?
                .ok_or_else(|| Status::internal("stream ended without response"))?;
            assert_eq!(response.request_id, request_id);
            match response.kind.unwrap() {
                server_frame::Kind::Ready(_) => {
                    session_id = response.session_id;
                    epoch = response.epoch;
                }
                server_frame::Kind::Validated(_) => {}
                server_frame::Kind::Proof(proof) => {
                    return Ok(Proof {
                        public_inputs_hash: proof.public_inputs_hash,
                        proof: proof.proof,
                    });
                }
                server_frame::Kind::Rejected(error) => {
                    return Err(Status::with_details(
                        Code::from_i32(error.code),
                        error.message,
                        error.details.into(),
                    ));
                }
                other => panic!("unexpected fixture response {other:?}"),
            }
        }
        Err(Status::internal("fixture did not finish with Proof"))
    }

    async fn prove(&self, chunks: Vec<WindowChunk>) -> Status {
        let before = self.state.attester.signing_calls();
        let status = self
            .exchange(chunks)
            .await
            .expect_err("request must be rejected");
        assert_eq!(
            self.state.attester.signing_calls(),
            before,
            "rejected request invoked the attester"
        );
        status
    }

    async fn attest(&self, chunks: Vec<WindowChunk>) -> Proof {
        let before = self.state.attester.signing_calls();
        let response = self
            .exchange(chunks)
            .await
            .expect("valid window must be attested");
        assert_eq!(self.state.attester.signing_calls(), before + 1);
        response
    }
}

fn unused_validator() -> Arc<ServiceState> {
    inner(Validator::stub(Vec::new()))
}

fn one_accepting_validator() -> Arc<ServiceState> {
    inner(Validator::stub(vec![Ok(backend_output_for(
        &happy_block_inputs(),
    ))]))
}

fn single_block_validator_with_execution_evidence(
    receipt_successes: Vec<bool>,
    system_sender_flags: Vec<bool>,
) -> Arc<ServiceState> {
    let mut backend_output = backend_output_for(&[stub_block(5, 0x04, 0x05)]);
    backend_output.blocks[0].set_transaction_results_for_test(receipt_successes);
    backend_output.blocks[0]
        .settlement_evidence
        .set_system_sender_flags_for_test(system_sender_flags);
    inner(Validator::stub(vec![Ok(backend_output)]))
}

fn two_block_validator_with_execution_evidence(
    first_receipt_successes: Vec<bool>,
    first_system_sender_flags: Vec<bool>,
    settling_receipt_successes: Vec<bool>,
    settling_system_sender_flags: Vec<bool>,
) -> Arc<ServiceState> {
    let mut backend_output =
        backend_output_for(&[stub_block(5, 0x04, 0x05), stub_block(6, 0x05, 0x06)]);
    backend_output.blocks[0].set_transaction_results_for_test(first_receipt_successes);
    backend_output.blocks[0]
        .settlement_evidence
        .set_system_sender_flags_for_test(first_system_sender_flags);
    backend_output.blocks[1].set_transaction_results_for_test(settling_receipt_successes);
    backend_output.blocks[1]
        .settlement_evidence
        .set_system_sender_flags_for_test(settling_system_sender_flags);
    inner(Validator::stub(vec![Ok(backend_output)]))
}

fn inner(validator: Validator) -> Arc<ServiceState> {
    inner_with_rollup(validator, expected_rollup_id(1))
}

fn native_validator_state(rollup_id: NonZeroU64) -> Arc<ServiceState> {
    let directory =
        crate::testkit::manifest_dir().join("tests/fixtures/native-offline-bidirectional");
    Arc::new(
        ServiceState::new(
            Address::repeat_byte(0x33),
            Address::repeat_byte(0x44),
            Validator::stateless(&directory.join("l2-genesis.json"), TEST_SYSTEM_ADDRESS).unwrap(),
            rollup_id,
            test_attester(),
            Arc::new(reth_chainspec::ChainSpec::from(
                serde_json::from_slice::<alloy_genesis::Genesis>(
                    &std::fs::read(directory.join("l1-genesis.json")).unwrap(),
                )
                .unwrap(),
            )),
        )
        .unwrap(),
    )
}

fn inner_with_rollup(validator: Validator, expected_rollup_id: NonZeroU64) -> Arc<ServiceState> {
    Arc::new(
        ServiceState::new(
            Address::repeat_byte(0x33),
            Address::repeat_byte(0x44),
            validator,
            expected_rollup_id,
            test_attester(),
            crate::testkit::l1_chain_spec(),
        )
        .unwrap(),
    )
}
