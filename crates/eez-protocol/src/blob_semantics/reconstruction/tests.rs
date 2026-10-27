use super::*;
use crate::abi::{EtherDeltaSol, RootUpdateSol};
use crate::{CallMode, EntryRollingHash, ExecutedAction, ExecutionOutcome};
use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};

fn action(source: u64, target: u64, marker: u8) -> ExecutedAction {
    ExecutedAction {
        parent_index: None,
        call_mode: CallMode::Mutable,
        target_address: Address::with_last_byte(marker),
        target_rollup_id: RollupId(target),
        source_rollup_id: RollupId(source),
        source_address: Address::with_last_byte(marker + 1),
        data: Bytes::from(vec![marker]),
        value: U256::from(marker),
        outcome: ExecutionOutcome::Resolved {
            return_data: vec![marker + 2],
            gas_used: 0,
            success: true,
        },
        revert_span: None,
    }
}

fn signed() -> Bytes {
    TxEnvelope::from(
        TxLegacy {
            chain_id: Some(1),
            gas_limit: 100_000,
            gas_price: 1,
            to: TxKind::Call(Address::with_last_byte(1)),
            ..Default::default()
        }
        .into_signed(Signature::new(U256::from(1), U256::from(1), false)),
    )
    .encoded_2718()
    .into()
}

fn semantic(source: u64, actions: Vec<ExecutedAction>) -> SemanticTransaction {
    SemanticTransaction {
        origin_chain: RollupId(source),
        tx_data: signed(),
        actions,
    }
}

fn decode(
    transactions: &[SemanticTransaction],
) -> Result<ReconstructionActions, ReconstructionError> {
    ReconstructionActions::from_semantics(transactions, RollupId(1), 1)
}

#[test]
fn source_positions_count_original_mutable_rows_and_preserve_failed_gaps() {
    let mut failed = action(0, 1, 4);
    failed.outcome = ExecutionOutcome::Resolved {
        return_data: vec![6],
        gas_used: 0,
        success: false,
    };
    let mut lookup = action(0, 1, 5);
    lookup.call_mode = CallMode::Static;
    lookup.value = U256::ZERO;
    let delivery = action(0, 1, 6);
    let stream = [
        semantic(1, vec![action(1, 0, 3)]),
        semantic(0, vec![failed, lookup, delivery.clone(), delivery]),
    ];
    let actions = decode(&stream).unwrap();
    assert_eq!(actions.outbound_entry_indices, [1]);
    assert_eq!(actions.inbound_entry_indices, [3, 4]);
    let live = LiveSourceContext::new(RollupId(1), B256::repeat_byte(8)).unwrap();
    let sources = actions.source_entries(&live).unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| source.index())
            .collect::<Vec<_>>(),
        [1, 3, 4]
    );
    let prefix = B256::repeat_byte(9);
    assert_ne!(sources[1].hash(prefix), sources[2].hash(prefix));
    assert_eq!(sources[1], live.entry(3).unwrap());
    // Selecting the original suffix retains rows, while discarding the original
    // outbound host renumbers the mutable table and cannot be called a resume.
    let renumbered = decode(&stream[1..]).unwrap().source_entries(&live).unwrap();
    assert_ne!(sources[1..], renumbered);
    let mut appended = stream.to_vec();
    appended.push(semantic(0, vec![action(0, 1, 7)]));
    let appended = decode(&appended).unwrap().source_entries(&live).unwrap();
    assert_eq!(&appended[..sources.len()], sources);
}

#[test]
fn static_only_outbound_host_counts_once_but_l1_statics_do_not_shift_rows() {
    let mut out = action(1, 0, 3);
    out.call_mode = CallMode::Static;
    out.value = U256::ZERO;
    let mut lookup = action(0, 1, 4);
    lookup.call_mode = CallMode::Static;
    lookup.value = U256::ZERO;
    let stream = [
        semantic(1, vec![out.clone(), out]),
        semantic(0, vec![lookup]),
        semantic(0, vec![action(0, 1, 5)]),
    ];
    let actions = decode(&stream).unwrap();
    assert_eq!(actions.outbound().len(), 1);
    assert_eq!(actions.outbound()[0].effects.len(), 2);
    let live = LiveSourceContext::new(RollupId(1), B256::repeat_byte(8)).unwrap();
    assert_eq!(
        actions
            .source_entries(&live)
            .unwrap()
            .iter()
            .map(|source| source.index())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn failed_foreign_root_is_not_filtered_from_the_semantic_profile() {
    let mut failed = action(0, 42, 4);
    failed.outcome = ExecutionOutcome::Resolved {
        return_data: vec![],
        gas_used: 0,
        success: false,
    };
    assert!(matches!(
        decode(&[semantic(0, vec![failed])]),
        Err(ReconstructionError::Table("foreign action chain"))
    ));
}

#[test]
fn semantic_reconstruction_retains_whole_groups_checked_inbound_and_source_identity() {
    let mut callback = action(0, 1, 4);
    callback.parent_index = Some(0);
    let transactions = [
        semantic(1, vec![action(1, 0, 3), callback, action(1, 0, 5)]),
        semantic(0, vec![action(0, 1, 6), action(0, 1, 7)]),
    ];
    let actions = decode(&transactions).unwrap();
    assert_eq!(actions.outbound().len(), 1);
    assert_eq!(actions.outbound()[0].effects.len(), 2);
    assert_eq!(actions.outbound()[0].user_tx, transactions[0].tx_data);
    assert_eq!(actions.inbound().len(), 2);
    assert_eq!(actions.inbound()[0].committed_value(), U256::from(6));
    assert_eq!(actions.inbound()[1].committed_value(), U256::from(7));
    let cfg = crate::system_tx::SystemTxContext {
        eezl2_address: eez_primitives::EEZL2_ADDRESS,
        l2_chain_id: 1,
        this_rollup_id: 1,
    };
    let pairs = crate::system_tx::build_cross_chain_sync_groups(
        actions.outbound(),
        actions.inbound(),
        &cfg,
    )
    .unwrap();
    assert_eq!(
        pairs.len(),
        3,
        "one load for the complete outbound group and two deliveries"
    );
    let live = LiveSourceContext::new(RollupId(1), B256::repeat_byte(8)).unwrap();
    let sources = actions.source_entries(&live).unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| source.index())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    for (pair, source) in pairs.iter().zip(sources) {
        let tx = pair
            .system_tx
            .at_position(crate::l2::GAS_LIMIT, 0, source.hash(B256::repeat_byte(9)))
            .unwrap();
        assert_eq!(tx.version, eez_primitives::SYSTEM_TX_VERSION);
        assert_ne!(tx.source_hash, B256::ZERO);
    }
}

#[test]
fn failed_inbound_and_static_lookups_do_not_become_native_deliveries() {
    let mut failed = action(0, 1, 1);
    failed.outcome = ExecutionOutcome::Resolved {
        return_data: vec![8],
        gas_used: 0,
        success: false,
    };
    let mut read = action(0, 1, 2);
    read.call_mode = CallMode::Static;
    read.value = U256::ZERO;
    let actions = decode(&[semantic(0, vec![failed, read])]).unwrap();
    assert!(actions.outbound().is_empty());
    assert!(actions.inbound().is_empty());
    assert_eq!(actions.compositions[0].source.batch.entries.len(), 1);
    assert_eq!(actions.compositions[0].source.batch.staticEntries.len(), 1);
}

#[test]
fn rejects_foreign_origins_destinations_order_and_noncanonical_origin_bytes() {
    assert_eq!(
        decode(&[semantic(2, vec![action(2, 1, 1)])]).unwrap_err(),
        ReconstructionError::Origin(2)
    );
    assert_eq!(
        decode(&[semantic(0, vec![action(0, 2, 1)])]).unwrap_err(),
        ReconstructionError::Table("foreign action chain")
    );
    assert_eq!(
        decode(&[
            semantic(0, vec![action(0, 1, 1)]),
            semantic(1, vec![action(1, 0, 2)])
        ])
        .unwrap_err(),
        ReconstructionError::DirectionOrder
    );
    assert_eq!(
        decode(&[semantic(0, vec![])]).unwrap_err(),
        ReconstructionError::EmptyTransaction
    );
    let mut tx = semantic(1, vec![action(1, 0, 1)]);
    tx.tx_data = [tx.tx_data.as_ref(), &[0]].concat().into();
    assert_eq!(
        decode(&[tx.clone()]).unwrap_err(),
        ReconstructionError::OriginTransaction
    );
    tx.tx_data = eez_primitives::SystemTransaction {
        chain_id: 1,
        version: eez_primitives::SYSTEM_TX_VERSION,
        source_hash: B256::repeat_byte(1),
        gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
        to: eez_primitives::EEZL2_ADDRESS,
        value: U256::ZERO,
        input: Bytes::new(),
    }
    .encoded_2718()
    .into();
    assert_eq!(
        decode(&[tx]).unwrap_err(),
        ReconstructionError::OriginTransaction
    );
    assert_eq!(
        ReconstructionActions::from_semantics(
            &[semantic(1, vec![action(1, 0, 1)])],
            RollupId(1),
            2
        )
        .unwrap_err(),
        ReconstructionError::OriginTransaction
    );
}

fn update(marker: u8) -> RootUpdateSol {
    RootUpdateSol {
        rollupId: 1,
        etherDelta: EtherDeltaSol::ZERO,
        currentRoot: B256::with_last_byte(marker),
        newRoot: B256::with_last_byte(marker + 1),
    }
}

fn settlement(transactions: &[SemanticTransaction]) -> EvmBatch {
    let root = update(1);
    let mut batch = EvmBatch {
        entries: vec![ExecutionEntrySol {
            destinationRollupId: 1,
            rollingHash: EntryRollingHash::seed_for_l1([(1, root.currentRoot)], B256::ZERO)
                .current(),
            rootUpdates: vec![root],
            success: true,
            ..Default::default()
        }],
        immediateEntryCount: U256::from(1),
        ..Default::default()
    };
    for tx in transactions {
        let mut c = tx.materialize().unwrap();
        if tx.origin_chain == RollupId(1) {
            for t in c.targets.iter_mut().filter(|t| t.rollup_id.is_mainnet()) {
                for e in &mut t.batch.entries {
                    e.rootUpdates = vec![update(2)];
                }
                finalize_l1_entry_traces(&mut t.batch, &t.entry_traces).unwrap();
                batch.immediateEntryCount += U256::from(t.batch.entries.len());
                batch.entries.extend(t.batch.entries.clone());
            }
        } else {
            for e in &mut c.source.batch.entries {
                e.rootUpdates = vec![update(3)];
            }
            finalize_l1_entry_traces(&mut c.source.batch, &c.source.entry_traces).unwrap();
            batch.entries.extend(c.source.batch.entries);
            for e in &mut c.source.batch.staticEntries {
                e.expectedRoots = vec![crate::abi::ExpectedRootPerRollupSol {
                    rollupId: 1,
                    root: B256::with_last_byte(4),
                }];
            }
            batch.staticEntries.extend(c.source.batch.staticEntries);
        }
    }
    batch
}

#[test]
fn semantic_binding_rejects_substitution_mutations_and_unused_suffixes() {
    let mut read = action(0, 1, 3);
    read.call_mode = CallMode::Static;
    read.value = U256::ZERO;
    let transactions = [
        semantic(1, vec![action(1, 0, 1)]),
        semantic(0, vec![action(0, 1, 2), read]),
    ];
    let actions = decode(&transactions).unwrap();
    let batch = settlement(&transactions);
    actions.bind_settlement(&batch).unwrap();
    let changes: &[fn(&mut EvmBatch)] = &[
        |b| b.entries[1].l2ToL1Calls[0].sourceRollupId = 2,
        |b| b.entries[2].destinationRollupId = 2,
        |b| b.entries[2].proxyEntryHash = B256::ZERO,
        |b| b.entries[2].rollingHash = B256::ZERO,
        |b| b.entries[2].rootUpdates[0].rollupId = 2,
        |b| b.entries.push(b.entries[2].clone()),
        |b| {
            b.entries.pop();
        },
        |b| b.entries.swap(1, 2),
        |b| b.staticEntries[0].returnData = Bytes::from_static(b"forged"),
        |b| b.staticEntries[0].expectedRoots[0].rollupId = 2,
        |b| b.staticEntries.push(b.staticEntries[0].clone()),
        |b| b.immediateEntryCount += U256::from(1),
    ];
    for change in changes {
        let mut bad = batch.clone();
        change(&mut bad);
        assert!(actions.bind_settlement(&bad).is_err());
    }
    assert!(
        InboundSidecar::for_rollup(&batch.entries[2], RollupId(1)).is_err(),
        "settlement ABI is never an inbound reconstruction input"
    );
}
