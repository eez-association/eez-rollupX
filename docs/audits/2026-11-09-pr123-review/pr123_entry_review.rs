//! Copy into crates/eez-protocol/tests/review_root_retry.rs of an isolated head snapshot.
use alloy_primitives::{Address, B256, Bytes, U256};
use eez_protocol::abi::{EtherDeltaSol, RootUpdateSol};
use eez_protocol::entries::{finalize_l1_entry_traces, verify_outbound_source_group};
use eez_protocol::system_tx::OutboundSourceEntry;
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId, materialize_actions};

fn action(parent: Option<usize>, source: RollupId, target: RollupId, suffix: u8) -> ExecutedAction {
    ExecutedAction {
        parent_index: parent,
        call_mode: CallMode::Mutable,
        source_rollup_id: source,
        target_rollup_id: target,
        source_address: Address::with_last_byte(0x10 + suffix),
        target_address: Address::with_last_byte(0x20 + suffix),
        data: Bytes::from(vec![0x30 + suffix]),
        value: U256::ZERO,
        outcome: ExecutionOutcome::Resolved {
            success: true,
            return_data: vec![0x40 + suffix],
            gas_used: 21_000,
        },
        revert_spans: vec![],
    }
}

fn verify_retry(natural_failure: bool, retry: bool) {
    let l2 = RollupId(1);
    let mut calls = vec![
        action(None, l2, RollupId::MAINNET, 0),
        action(Some(0), RollupId::MAINNET, l2, 1),
        action(None, l2, RollupId::MAINNET, 0),
        action(Some(2), RollupId::MAINNET, l2, 1),
    ];
    if natural_failure {
        for i in [0, 2] {
            calls[i].outcome = ExecutionOutcome::Resolved {
                success: false,
                return_data: vec![0xde, 0xad],
                gas_used: 21_000,
            };
        }
    } else {
        calls[0].revert_spans = vec![2];
    }
    if !retry {
        calls.truncate(2);
    }
    let mut composition = materialize_actions(l2, calls).expect("representable retry");
    let host = &mut composition.targets[0];
    host.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: 1,
        currentRoot: B256::with_last_byte(1),
        newRoot: B256::with_last_byte(2),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).unwrap();
    let sources = composition
        .source
        .batch
        .entries
        .iter()
        .cloned()
        .map(OutboundSourceEntry::Mutable)
        .collect::<Vec<_>>();
    eprintln!(
        "natural_failure={natural_failure}, slots={}, callbacks={}",
        sources.len(),
        composition.source.batch.entries[0].l2ToL1Calls.len()
    );
    let result = verify_outbound_source_group(&host.batch.entries[0], &sources, l2);
    eprintln!("verification: {result:?}");
    assert!(result.is_ok(), "valid identical retry rejected: {result:?}");
    assert_eq!(composition.source.batch.entries.len(), 1);
    assert_eq!(
        composition.source.batch.entries[0].l2ToL1Calls.len(),
        1,
        "Each source-slot invocation must reuse its one callback, not concatenate both attempts"
    );
}

#[test]
fn nonretry_callback_control() {
    verify_retry(false, false);
}
#[test]
fn reverted_root_retry_callback_program() {
    verify_retry(false, true);
}
#[test]
fn naturally_failed_root_retry_callback_program() {
    verify_retry(true, true);
}

#[test]
fn naturally_failed_root_retry_accepts_one_reused_callback_program_control() {
    let l2 = RollupId(1);
    let mut root = action(None, l2, RollupId::MAINNET, 0);
    root.outcome = ExecutionOutcome::Resolved {
        success: false,
        return_data: vec![0xde, 0xad],
        gas_used: 21_000,
    };
    let callback = action(Some(0), RollupId::MAINNET, l2, 1);
    let mut retry_callback = callback.clone();
    retry_callback.parent_index = Some(2);
    // Preserve the host for both complete attempts; replace only its defective
    // concatenated L2 source slot with the exact single-invocation program.
    let mut repeated = materialize_actions(
        l2,
        vec![root.clone(), callback.clone(), root.clone(), retry_callback],
    )
    .unwrap();
    let single = materialize_actions(l2, vec![root, callback]).unwrap();
    let host = &mut repeated.targets[0];
    host.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: 1,
        currentRoot: B256::with_last_byte(1),
        newRoot: B256::with_last_byte(2),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).unwrap();
    assert_eq!(host.batch.entries[0].l2ToL1Calls.len(), 2);
    assert_eq!(repeated.source.batch.entries[0].l2ToL1Calls.len(), 2);
    assert_eq!(single.source.batch.entries[0].l2ToL1Calls.len(), 1);
    let sources = single
        .source
        .batch
        .entries
        .into_iter()
        .map(OutboundSourceEntry::Mutable)
        .collect::<Vec<_>>();
    let facts = verify_outbound_source_group(&host.batch.entries[0], &sources, l2)
        .expect("both failed root attempts reuse the same complete callback program");
    assert!(facts.l2_outbound_hashes.is_empty());
    assert_eq!(facts.ether_delta, alloy_primitives::I256::ZERO);
}
