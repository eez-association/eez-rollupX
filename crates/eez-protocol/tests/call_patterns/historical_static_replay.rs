use alloy_consensus::{SignableTransaction, TxEnvelope, TxLegacy};
use alloy_eips::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};
use alloy_sol_types::SolValue;
use eez_protocol::abi::{EtherDeltaSol, ExecutionEntrySol, RootUpdateSol};
use eez_protocol::blob_semantics::{ReconstructionActions, SemanticTransaction};
use eez_protocol::entries::{finalize_l1_entry_traces, verify_outbound_source_group};
use eez_protocol::system_tx::OutboundSourceEntry;
use eez_protocol::{
    CallMode, EntryRollingHash, EvmBatch, ExecutedAction, ExecutionOutcome, RollupId,
};
fn call(parent: Option<usize>, source: u64, target: u64, mode: CallMode) -> ExecutedAction {
    ExecutedAction {
        parent_index: parent,
        call_mode: mode,
        source_rollup_id: RollupId(source),
        target_rollup_id: RollupId(target),
        source_address: Address::with_last_byte(0x10 + source as u8),
        target_address: Address::with_last_byte(0x20 + target as u8),
        data: Bytes::from_static(&[0x42]),
        value: U256::ZERO,
        outcome: ExecutionOutcome::Resolved {
            success: true,
            return_data: vec![0x77],
            gas_used: 21000,
        },
        revert_spans: vec![],
    }
}
fn update(marker: u8) -> RootUpdateSol {
    RootUpdateSol {
        rollupId: 1,
        currentRoot: B256::with_last_byte(marker),
        newRoot: B256::with_last_byte(marker + 1),
        etherDelta: EtherDeltaSol::ZERO,
    }
}
#[test]
fn repeated_static_replay_compatibility() {
    let tx = SemanticTransaction {
        operations_version: eez_protocol::blob_semantics::OperationsVersion::V0,
        origin_chain: RollupId(1),
        tx_data: TxEnvelope::from(
            TxLegacy {
                chain_id: Some(1),
                gas_limit: 100000,
                gas_price: 1,
                to: TxKind::Call(Address::with_last_byte(1)),
                ..Default::default()
            }
            .into_signed(Signature::new(U256::from(1), U256::from(1), false)),
        )
        .encoded_2718()
        .into(),
        actions: vec![
            call(None, 1, 0, CallMode::Mutable),
            call(Some(0), 0, 1, CallMode::Static),
            call(Some(0), 0, 1, CallMode::Static),
        ],
    };
    let mut c = tx.materialize().unwrap();
    let host = &mut c.targets[0];
    host.batch.entries[0].rootUpdates = vec![update(2)];
    finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).unwrap();
    let sources = c
        .source
        .batch
        .entries
        .iter()
        .cloned()
        .map(OutboundSourceEntry::Mutable)
        .collect::<Vec<_>>();
    let verified = verify_outbound_source_group(&host.batch.entries[0], &sources, RollupId(1));
    eprintln!(
        "rows={} verification={verified:?}",
        host.batch.entries[0].expectedL1ToL2Calls.len()
    );
    assert!(verified.is_ok());
    let root = update(1);
    let batch = EvmBatch {
        entries: vec![
            ExecutionEntrySol {
                destinationRollupId: 1,
                rollingHash: EntryRollingHash::seed_for_l1([(1, root.currentRoot)], B256::ZERO)
                    .current(),
                rootUpdates: vec![root],
                success: true,
                ..Default::default()
            },
            host.batch.entries[0].clone(),
        ],
        immediateEntryCount: U256::from(2),
        ..Default::default()
    };
    let reconstruction =
        ReconstructionActions::from_semantics(std::slice::from_ref(&tx), RollupId(1), 1).unwrap();
    reconstruction.bind_settlement(&batch).unwrap();
    // Frozen from c702f90da19cc149890d0a17a4ef6c200dcb6a21, protocol
    // 84a5b5207880f192a6a5896cbfe0bac5249db47f. Baseline verification and
    // native settlement binding both accepted these exact bytes.
    let bytes =
        alloy_primitives::hex::decode(include_str!("fixtures/baseline_static_batch.hex").trim())
            .unwrap();
    let frozen = EvmBatch::abi_decode(&bytes).unwrap();
    reconstruction
        .bind_settlement(&frozen)
        .expect("replay baseline-supported static rows");
    let mut modern_tx = tx.clone();
    modern_tx.operations_version = eez_protocol::blob_semantics::OperationsVersion::V1;
    let mut modern = modern_tx.materialize().unwrap();
    let modern_host = &mut modern.targets[0];
    assert_eq!(modern_host.batch.entries[0].expectedL1ToL2Calls.len(), 1);
    modern_host.batch.entries[0].rootUpdates = vec![update(2)];
    finalize_l1_entry_traces(&mut modern_host.batch, &modern_host.entry_traces).unwrap();
    let mut modern_batch = batch.clone();
    modern_batch.entries[1] = modern_host.batch.entries[0].clone();
    let modern_reconstruction =
        ReconstructionActions::from_semantics(&[modern_tx], RollupId(1), 1).unwrap();
    modern_reconstruction
        .bind_settlement(&modern_batch)
        .unwrap();
    assert!(
        modern_reconstruction.bind_settlement(&frozen).is_err(),
        "V1 must not silently accept V0 construction"
    );
    assert!(
        reconstruction.bind_settlement(&modern_batch).is_err(),
        "V0 must not silently accept V1 construction"
    );
}

#[test]
fn terminal_failed_retries_preserve_baseline_source_slots() {
    let mut tx = SemanticTransaction {
        operations_version: eez_protocol::blob_semantics::OperationsVersion::V0,
        origin_chain: RollupId(1),
        tx_data: TxEnvelope::from(
            TxLegacy {
                chain_id: Some(1),
                gas_limit: 100000,
                gas_price: 1,
                to: TxKind::Call(Address::with_last_byte(1)),
                ..Default::default()
            }
            .into_signed(Signature::new(U256::from(1), U256::from(1), false)),
        )
        .encoded_2718()
        .into(),
        actions: vec![
            call(None, 1, 0, CallMode::Mutable),
            call(None, 1, 0, CallMode::Mutable),
        ],
    };
    for action in &mut tx.actions {
        if let ExecutionOutcome::Resolved { success, .. } = &mut action.outcome {
            *success = false;
        }
    }
    let mut c = tx.materialize().unwrap();
    let host = &mut c.targets[0];
    host.batch.entries[0].rootUpdates = vec![update(2)];
    finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).unwrap();
    let sources = c
        .source
        .batch
        .entries
        .iter()
        .cloned()
        .map(OutboundSourceEntry::Mutable)
        .collect::<Vec<_>>();
    let verified = verify_outbound_source_group(&host.batch.entries[0], &sources, RollupId(1));
    eprintln!(
        "rows={} verification={verified:?}",
        host.batch.entries[0].expectedL1ToL2Calls.len()
    );
    assert!(verified.is_ok());
    let root = update(1);
    let batch = EvmBatch {
        entries: vec![
            ExecutionEntrySol {
                destinationRollupId: 1,
                rollingHash: EntryRollingHash::seed_for_l1([(1, root.currentRoot)], B256::ZERO)
                    .current(),
                rootUpdates: vec![root],
                success: true,
                ..Default::default()
            },
            host.batch.entries[0].clone(),
        ],
        immediateEntryCount: U256::from(2),
        ..Default::default()
    };
    let reconstruction =
        ReconstructionActions::from_semantics(std::slice::from_ref(&tx), RollupId(1), 1).unwrap();
    reconstruction.bind_settlement(&batch).unwrap();

    let frozen = EvmBatch::abi_decode(
        &alloy_primitives::hex::decode(
            include_str!("fixtures/baseline_terminal_source.hex").trim(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        c.source.batch.abi_encode(),
        frozen.abi_encode(),
        "historical native load bytes must not change"
    );
    let mut extra = sources.clone();
    extra.push(sources[0].clone());
    assert!(
        verify_outbound_source_group(&host.batch.entries[0], &extra, RollupId(1)).is_err(),
        "an extra duplicate slot has no observed invocation"
    );
}

#[test]
fn l2_only_static_compaction_requires_the_authenticated_version() {
    use eez_protocol::blob_semantics::OperationsVersion;
    let mut transaction = SemanticTransaction {
        operations_version: OperationsVersion::V0,
        origin_chain: RollupId::MAINNET,
        tx_data: Bytes::new(),
        actions: vec![
            call(None, 0, 1, CallMode::Mutable),
            call(Some(0), 1, 0, CallMode::Static),
            call(Some(0), 1, 0, CallMode::Static),
        ],
    };
    let legacy = transaction.materialize().unwrap();
    let legacy_replay =
        ReconstructionActions::from_semantics(std::slice::from_ref(&transaction), RollupId(1), 1)
            .unwrap();
    transaction.operations_version = OperationsVersion::V1;
    let compact = transaction.materialize().unwrap();
    let compact_replay =
        ReconstructionActions::from_semantics(&[transaction], RollupId(1), 1).unwrap();
    assert_eq!(
        legacy.source.batch.abi_encode(),
        compact.source.batch.abi_encode(),
        "L1 tables alone cannot identify the L2 reconstruction policy"
    );
    assert_eq!(
        legacy.targets[0].batch.entries[0].expectedL1ToL2Calls.len(),
        2
    );
    assert_eq!(
        compact.targets[0].batch.entries[0]
            .expectedL1ToL2Calls
            .len(),
        1
    );
    assert_ne!(
        legacy_replay.inbound()[0].as_abi_entry().abi_encode(),
        compact_replay.inbound()[0].as_abi_entry().abi_encode(),
        "version-selected replay preserves the exact native transaction bytes"
    );
}
