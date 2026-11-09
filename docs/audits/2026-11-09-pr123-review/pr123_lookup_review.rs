use alloy_primitives::{Address, B256, Bytes, U256};
use eez_protocol::abi::{EtherDeltaSol, RootUpdateSol};
use eez_protocol::entries::finalize_l1_entry_traces;
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId, materialize_actions};
#[test]
fn source_reverted_successful_lookup_has_unforced_callback_without_delivery() {
    let actions = (0..2)
        .map(|i| ExecutedAction {
            parent_index: if i == 0 { None } else { Some(0) },
            call_mode: CallMode::Mutable,
            source_rollup_id: RollupId(i),
            target_rollup_id: RollupId(1 - i),
            source_address: Address::with_last_byte(1 + i as u8),
            target_address: Address::with_last_byte(2 - i as u8),
            data: Bytes::from_static(b"call"),
            value: U256::ZERO,
            outcome: ExecutionOutcome::Resolved {
                success: true,
                return_data: vec![],
                gas_used: 21000,
            },
            revert_spans: if i == 0 { vec![2] } else { vec![] },
        })
        .collect();
    let mut c = materialize_actions(RollupId::MAINNET, actions).unwrap();
    assert!(c.targets.is_empty());
    c.source.batch.entries[0].rootUpdates = vec![RootUpdateSol {
        rollupId: 1,
        currentRoot: B256::ZERO,
        newRoot: B256::ZERO,
        etherDelta: EtherDeltaSol::ZERO,
    }];
    finalize_l1_entry_traces(&mut c.source.batch, &c.source.entry_traces).unwrap();
    let entry = &c.source.batch.entries[0];
    assert!(eez_protocol::settlement::is_lookup_only_inbound_entry(
        entry, 1
    ));
    assert!(entry.success);
    assert_eq!(entry.l2ToL1Calls.len(), 1);
    assert_eq!(entry.l2ToL1Calls[0].revertNextNCalls, 0);
}
