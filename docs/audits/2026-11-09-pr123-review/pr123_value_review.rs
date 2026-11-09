//! Review-only reproductions; do not merge without an implementation fix.
use alloy_primitives::{Address, B256, Bytes, I256, U256};
use eez_protocol::abi::{EtherDeltaSol, RootUpdateSol};
use eez_protocol::entries::{
    NestedPairDirection, entry_flat_value_sum, finalize_l1_entry_traces, verify_nested_group,
};
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId, materialize_actions};

fn assert_inbound_projection_matches_verified_effect(depth: usize) {
    let l2 = RollupId(1);
    let calls = (0..depth)
        .map(|index| {
            let (source, target) = if index % 2 == 0 {
                (RollupId::MAINNET, l2)
            } else {
                (l2, RollupId::MAINNET)
            };
            ExecutedAction {
                parent_index: index.checked_sub(1),
                call_mode: CallMode::Mutable,
                source_rollup_id: source,
                target_rollup_id: target,
                source_address: Address::with_last_byte(source.0 as u8 + 1),
                target_address: Address::with_last_byte(target.0 as u8 + 1),
                data: Bytes::from(vec![index as u8]),
                value: if index == depth - 1 {
                    U256::from(7)
                } else {
                    U256::ZERO
                },
                outcome: ExecutionOutcome::Resolved {
                    success: index != 1,
                    return_data: if index == 1 {
                        vec![0xde, 0xad]
                    } else {
                        vec![0x77]
                    },
                    gas_used: 21_000,
                },
                revert_spans: vec![],
            }
        })
        .collect();
    // The root succeeds after catching node 1's revert. The last transfer is
    // inside node 1's frame and therefore must disappear on both chains.
    let mut composition = materialize_actions(RollupId::MAINNET, calls).unwrap();
    let source = &mut composition.source;
    source.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
        currentRoot: B256::with_last_byte(1),
        newRoot: B256::with_last_byte(2),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(&mut source.batch, &source.entry_traces).unwrap();
    let l1_entry = &composition.source.batch.entries[0];
    let l2_entries = &composition.targets[0].batch.entries;
    let verified =
        verify_nested_group(l1_entry, l2_entries, NestedPairDirection::Inbound, l2).unwrap();
    assert_eq!(
        verified.ether_delta,
        I256::ZERO,
        "independent traversal must discard the failed ancestor's descendants"
    );
    let outflow = entry_flat_value_sum(l1_entry).unwrap();
    let inflow = entry_flat_value_sum(&l2_entries[0]).unwrap();
    let projected = I256::try_from(inflow).unwrap() - I256::try_from(outflow).unwrap();
    assert_eq!(
        projected, verified.ether_delta,
        "Composer's settlement projection disagrees: depth={depth}, inflow={inflow}, outflow={outflow}"
    );
}

#[test]
fn naturally_failed_ancestor_discards_successful_descendant_withdrawal() {
    assert_inbound_projection_matches_verified_effect(4);
}

#[test]
fn naturally_failed_ancestor_discards_successful_descendant_deposit() {
    assert_inbound_projection_matches_verified_effect(5);
}
