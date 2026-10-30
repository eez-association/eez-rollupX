//! Protocol call-pattern regressions. Network scenarios remain the execution
//! oracle; these fixtures isolate table construction and independent binding.

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use eez_protocol::abi::{EtherDeltaSol, RootUpdateSol};
use eez_protocol::entries::{finalize_l1_entry_traces, verify_outbound_source_group};
use eez_protocol::system_tx::OutboundSourceEntry;
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId, materialize_actions};

fn static_call(parent: Option<usize>, source: RollupId, target: RollupId) -> ExecutedAction {
    ExecutedAction {
        parent_index: parent,
        call_mode: CallMode::Static,
        source_rollup_id: source,
        target_rollup_id: target,
        source_address: Address::with_last_byte(source.0 as u8 + 1),
        target_address: Address::with_last_byte(target.0 as u8 + 1),
        data: Bytes::from_static(&[0x12, 0x34, 0x56, 0x78]),
        value: U256::ZERO,
        outcome: ExecutionOutcome::Resolved {
            success: true,
            return_data: vec![0x77],
            gas_used: 21_000,
        },
        revert_spans: vec![],
    }
}

#[test]
fn static_failed_callback_preserves_revert_bytes_and_binds_success_bit() {
    // staticReadHandlesFailedCallbackL2 / staticReadPropagatesFailedCallbackL2:
    // L2 reads L1; L1's terminal read back to L2 fails. The L1 application may
    // catch that failure or propagate it. Both are valid STATICCALL outcomes.
    for source_success in [true, false] {
        let l2 = RollupId(1);
        let mut read = static_call(None, l2, RollupId::MAINNET);
        read.outcome = ExecutionOutcome::Resolved {
            success: source_success,
            return_data: vec![0x77],
            gas_used: 21_000,
        };
        let mut callback = static_call(Some(0), RollupId::MAINNET, l2);
        callback.outcome = ExecutionOutcome::Resolved {
            success: false,
            return_data: vec![0xde, 0xad],
            gas_used: 21_000,
        };
        let mut composition = materialize_actions(l2, vec![read, callback])
            .expect("the pinned protocol represents failed static callbacks");
        let host = &mut composition.targets[0];
        host.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(1),
            newRoot: B256::with_last_byte(2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).unwrap();
        let source = &composition.source.l2_static_entries[0];
        // Independent encoding of keccak256(zeroHash || false || revertData).
        let mut preimage = vec![0u8; 33];
        preimage.extend_from_slice(&[0xde, 0xad]);
        assert_eq!(source.rollingHash, keccak256(preimage));
        assert_eq!(source.success, source_success);
        assert!(composition.source.batch.entries.is_empty());
        let sources = [OutboundSourceEntry::Static(source.clone())];
        verify_outbound_source_group(&host.batch.entries[0], &sources, l2)
            .expect("independent verifier accepts the exact failed callback");

        let mut forged = host.batch.entries[0].clone();
        forged.expectedL1ToL2Calls[0].success = true;
        let error = verify_outbound_source_group(&forged, &sources, l2)
            .expect_err("changing callback success cannot retain the signed static outcome");
        assert!(error.to_string().contains("static"), "{error}");
    }
}

#[test]
fn nested_rollback_roots_scan_forward_after_restoring_the_source_cursor() {
    let l2 = RollupId(1);
    let mut calls = (0..3)
        .map(|index| {
            let mut call = static_call(None, l2, RollupId::MAINNET);
            call.call_mode = CallMode::Mutable;
            call.data = Bytes::from(vec![index]);
            call
        })
        .collect::<Vec<_>>();
    calls[0].revert_spans = vec![2, 1];
    let mut composition = materialize_actions(l2, calls).unwrap();
    let host = &mut composition.targets[0];
    host.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
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
    assert_eq!(sources.len(), 3);
    assert_eq!(
        host.batch.entries[0]
            .l2ToL1Calls
            .iter()
            .map(|call| call.revertNextNCalls)
            .collect::<Vec<_>>(),
        vec![1, 1, 0]
    );
    verify_outbound_source_group(&host.batch.entries[0], &sources, l2)
        .expect("distinct calls after rollback scan past the earlier candidate");
    let mut omitted = sources.clone();
    omitted.remove(1);
    assert!(verify_outbound_source_group(&host.batch.entries[0], &omitted, l2).is_err());
}

#[test]
fn rollback_closes_after_the_complete_sibling_subtree() {
    let l2 = RollupId(1);
    let mut calls = vec![
        static_call(None, RollupId::MAINNET, l2),
        static_call(Some(0), l2, RollupId::MAINNET),
        static_call(Some(1), RollupId::MAINNET, l2),
    ];
    for call in &mut calls {
        call.call_mode = CallMode::Mutable;
    }
    calls[1].revert_spans = vec![2];
    let root = &calls[0];
    let hash =
        eez_protocol::action::common_cross_chain_call_hash(eez_protocol::action::CallHashInput {
            call_mode: root.call_mode,
            source_address: root.source_address,
            source_rollup_id: root.source_rollup_id,
            target_address: root.target_address,
            target_rollup_id: root.target_rollup_id,
            value: root.value,
            data: &root.data,
        });
    let mut expected = eez_protocol::rolling_hash::EntryRollingHash::seed_for_l2(hash);
    expected.call_begin(hash);
    expected.call_end(true, &[0x77]);
    let composition = materialize_actions(RollupId::MAINNET, calls).unwrap();
    assert_eq!(
        composition.targets[0].batch.entries[0].rollingHash,
        expected.current(),
        "the whole nested frame, including NESTED_END, must roll back before the root ends"
    );
}

#[test]
fn reentrant_lookup_scans_past_rows_after_rollback() {
    for fail_second in [false, true] {
        let l2 = RollupId(1);
        let mut calls = vec![
            static_call(None, RollupId::MAINNET, l2),
            static_call(Some(0), l2, RollupId::MAINNET),
            static_call(Some(0), l2, RollupId::MAINNET),
        ];
        for call in &mut calls {
            call.call_mode = CallMode::Mutable;
        }
        calls[1].revert_spans = vec![1];
        calls[2].data = Bytes::from_static(&[0x99]);
        if fail_second {
            calls[2].outcome = ExecutionOutcome::Resolved {
                success: false,
                return_data: vec![0xde, 0xad],
                gas_used: 21_000,
            };
            let mut retry = calls[1].clone();
            retry.revert_spans.clear();
            calls.push(retry);
        }
        let mut composition = materialize_actions(RollupId::MAINNET, calls)
            .expect("a different call after rollback can use a later reentrant row");
        let source = &mut composition.source;
        source.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(1),
            newRoot: B256::with_last_byte(2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut source.batch, &source.entry_traces).unwrap();
        let target = &composition.targets[0].batch.entries;
        assert_eq!(target[0].expectedL1ToL2Calls.len(), 2);
        eez_protocol::entries::verify_nested_group(
            &source.batch.entries[0],
            target,
            eez_protocol::entries::NestedPairDirection::Inbound,
            l2,
        )
        .unwrap();
        let mut forged = target.clone();
        forged[0].expectedL1ToL2Calls[0].expectedL1toL2Hash = B256::repeat_byte(0x99);
        assert!(
            eez_protocol::entries::verify_nested_group(
                &source.batch.entries[0],
                &forged,
                eez_protocol::entries::NestedPairDirection::Inbound,
                l2
            )
            .is_err()
        );
    }
}

#[test]
fn static_read_does_not_consume_a_failed_mutable_retry_slot() {
    let l2 = RollupId(1);
    let mut root = static_call(None, RollupId::MAINNET, l2);
    root.call_mode = CallMode::Mutable;
    let mut failed = static_call(Some(0), l2, RollupId::MAINNET);
    failed.call_mode = CallMode::Mutable;
    failed.outcome = ExecutionOutcome::Resolved {
        success: false,
        return_data: vec![0xde, 0xad],
        gas_used: 21_000,
    };
    let read = static_call(Some(0), l2, RollupId::MAINNET);
    let mut calls = vec![root, failed.clone(), read, failed];
    let mut composition = materialize_actions(RollupId::MAINNET, calls.clone()).unwrap();
    assert_eq!(
        composition.targets[0].batch.entries[0]
            .expectedL1ToL2Calls
            .len(),
        2,
        "the static read cannot move the mutable lookup cursor past the failed row"
    );
    let source = &mut composition.source;
    source.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
        currentRoot: B256::with_last_byte(1),
        newRoot: B256::with_last_byte(2),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(&mut source.batch, &source.entry_traces).unwrap();
    eez_protocol::entries::verify_nested_group(
        &source.batch.entries[0],
        &composition.targets[0].batch.entries,
        eez_protocol::entries::NestedPairDirection::Inbound,
        l2,
    )
    .unwrap();
    calls[3].outcome = ExecutionOutcome::Resolved {
        success: false,
        return_data: vec![0xff],
        gas_used: 21_000,
    };
    assert!(
        materialize_actions(RollupId::MAINNET, calls).is_err(),
        "an intervening read cannot authorize a later conflicting mutable candidate"
    );
}

#[test]
fn failed_inbound_root_retains_source_callbacks_without_canonical_delivery() {
    let l2 = RollupId(1);
    let mut calls = vec![
        static_call(None, RollupId::MAINNET, l2),
        static_call(Some(0), l2, RollupId::MAINNET),
        static_call(Some(1), RollupId::MAINNET, l2),
    ];
    for call in &mut calls {
        call.call_mode = CallMode::Mutable;
    }
    calls[0].outcome = ExecutionOutcome::Resolved {
        success: false,
        return_data: vec![0xde, 0xad],
        gas_used: 21_000,
    };
    let mut composition = materialize_actions(RollupId::MAINNET, calls).unwrap();
    assert!(
        composition.targets.is_empty(),
        "a failed source root authorizes no canonical delivery"
    );
    let source = &composition.source.batch.entries[0];
    assert!(!source.success);
    assert_eq!(
        source.l2ToL1Calls.len(),
        1,
        "the source resolver must execute the callback before its terminal revert"
    );
    assert_eq!(
        source.expectedL1ToL2Calls.len(),
        1,
        "the callback's nested read/write remains bound to the source frame"
    );
    let entry = &mut composition.source.batch.entries[0];
    entry.rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
        currentRoot: B256::with_last_byte(7),
        newRoot: B256::with_last_byte(7),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(
        &mut composition.source.batch,
        &composition.source.entry_traces,
    )
    .unwrap();
    let entry = &composition.source.batch.entries[0];
    assert!(eez_protocol::settlement::is_lookup_only_inbound_entry(
        entry, l2.0
    ));
    assert_eq!(
        eez_protocol::entries::entry_flat_value_sum(entry),
        Some(U256::ZERO)
    );
}

#[test]
fn failed_mutable_frame_binds_descendants_before_restoring_its_state() {
    let l2 = RollupId(1);
    for depth in [3usize, 4, 5] {
        let mut calls = (0..depth)
            .map(|index| {
                let (source, target) = if index % 2 == 0 {
                    (RollupId::MAINNET, l2)
                } else {
                    (l2, RollupId::MAINNET)
                };
                let mut call = static_call(index.checked_sub(1), source, target);
                call.call_mode = CallMode::Mutable;
                call.data = Bytes::from(vec![index as u8]);
                call
            })
            .collect::<Vec<_>>();
        calls[1].outcome = ExecutionOutcome::Resolved {
            success: false,
            return_data: vec![0xde, 0xad],
            gas_used: 21_000,
        };
        let mut composition = materialize_actions(RollupId::MAINNET, calls)
            .expect("a failed mutable frame may have executed successful descendants");
        let source = &mut composition.source;
        source.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(1),
            newRoot: B256::with_last_byte(2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut source.batch, &source.entry_traces).unwrap();
        let target = &composition.targets[0].batch.entries;
        eez_protocol::entries::verify_nested_group(
            &source.batch.entries[0],
            target,
            eez_protocol::entries::NestedPairDirection::Inbound,
            l2,
        )
        .unwrap();
        let mut forged = target.clone();
        forged[0].expectedL1ToL2Calls[0].revertedOrStaticRollingHash = B256::ZERO;
        let error = eez_protocol::entries::verify_nested_group(
            &source.batch.entries[0],
            &forged,
            eez_protocol::entries::NestedPairDirection::Inbound,
            l2,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("reverted rolling hash"),
            "{error}"
        );
    }
}

#[test]
fn nested_rollback_windows_restore_inner_state_before_continuing_outer_scope() {
    let l2 = RollupId(1);
    let mut calls = vec![static_call(None, RollupId::MAINNET, l2)];
    for index in 1..=3 {
        let mut call = static_call(Some(0), l2, RollupId::MAINNET);
        call.data = Bytes::from(vec![index]);
        calls.push(call);
    }
    for call in &mut calls {
        call.call_mode = CallMode::Mutable;
    }
    calls[1].revert_spans = vec![3];
    calls[2].revert_spans = vec![1];
    let mut composition = materialize_actions(RollupId::MAINNET, calls).unwrap();
    let source = &mut composition.source;
    source.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
        currentRoot: B256::with_last_byte(1),
        newRoot: B256::with_last_byte(2),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(&mut source.batch, &source.entry_traces).unwrap();
    assert_eq!(
        source.batch.entries[0]
            .l2ToL1Calls
            .iter()
            .map(|call| call.revertNextNCalls)
            .collect::<Vec<_>>(),
        vec![3, 1, 0]
    );
    eez_protocol::entries::verify_nested_group(
        &source.batch.entries[0],
        &composition.targets[0].batch.entries,
        eez_protocol::entries::NestedPairDirection::Inbound,
        l2,
    )
    .unwrap();
}

#[test]
fn rollback_scope_can_start_with_a_static_read_and_restore_its_source_cursor() {
    for failed_write in [false, true] {
        let l2 = RollupId(1);
        let mut read = static_call(None, l2, RollupId::MAINNET);
        read.revert_spans = if failed_write { vec![] } else { vec![2] };
        let mut write = static_call(None, l2, RollupId::MAINNET);
        write.call_mode = CallMode::Mutable;
        if failed_write {
            write.outcome = ExecutionOutcome::Resolved {
                success: false,
                return_data: vec![0x77],
                gas_used: 21_000,
            };
        }
        let final_read = static_call(None, l2, RollupId::MAINNET);
        let mut composition = materialize_actions(l2, vec![read, write, final_read]).unwrap();
        let target = &mut composition.targets[0];
        target.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(1),
            newRoot: B256::with_last_byte(2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut target.batch, &target.entry_traces).unwrap();
        let mut sources = composition
            .source
            .batch
            .entries
            .iter()
            .cloned()
            .map(OutboundSourceEntry::Mutable)
            .chain(
                composition
                    .source
                    .l2_static_entries
                    .iter()
                    .cloned()
                    .map(OutboundSourceEntry::Static),
            )
            .collect::<Vec<_>>();
        let facts = verify_outbound_source_group(&target.batch.entries[0], &sources, l2).unwrap();
        assert!(facts.l2_outbound_hashes.is_empty());
        let OutboundSourceEntry::Static(last) = sources.last_mut().unwrap() else {
            unreachable!()
        };
        assert_eq!(last.expectedEntryIndex, U256::ZERO);
        last.expectedEntryIndex = U256::from(1);
        assert!(verify_outbound_source_group(&target.batch.entries[0], &sources, l2).is_err());
    }
}

#[test]
fn failed_root_retry_keeps_one_complete_source_group() {
    let l2 = RollupId(1);
    let mut call = static_call(None, l2, RollupId::MAINNET);
    call.call_mode = CallMode::Mutable;
    call.outcome = ExecutionOutcome::Resolved {
        success: false,
        return_data: vec![0xde, 0xad],
        gas_used: 21_000,
    };
    let mut composition = materialize_actions(l2, vec![call.clone(), call]).unwrap();
    assert_eq!(composition.source.batch.entries.len(), 1);
    let host = &mut composition.targets[0];
    host.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
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
    let facts = verify_outbound_source_group(&host.batch.entries[0], &sources, l2).unwrap();
    assert!(facts.l2_outbound_hashes.is_empty());
    assert_eq!(
        eez_protocol::system_tx::outbound_settlement_source_counts(
            &host.batch.entries,
            &[(sources.len(), false)]
        )
        .unwrap(),
        [1]
    );
}

#[test]
fn recursive_static_reads_use_the_innermost_static_frame() {
    let l2 = RollupId(1);
    let calls = vec![
        static_call(None, l2, RollupId::MAINNET),
        static_call(Some(0), RollupId::MAINNET, l2),
        static_call(Some(1), l2, RollupId::MAINNET),
        static_call(Some(2), RollupId::MAINNET, l2),
    ];
    let mut composition =
        materialize_actions(l2, calls).expect("recursive static protocol support");
    let target = &mut composition.targets[0];
    target.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
        currentRoot: B256::with_last_byte(1),
        newRoot: B256::with_last_byte(2),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(&mut target.batch, &target.entry_traces).unwrap();
    // TableGenerator._staticCall keeps a separate frame for each pending read.
    // The third call is a second source pool lookup, while its callback belongs
    // to that lookup rather than to the first read's callback list.
    let sources = &composition.source.l2_static_entries;
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].incomingCalls.len(), 1);
    assert_eq!(sources[1].incomingCalls.len(), 1);
    let host = &composition.targets[0].batch.entries[0];
    assert_eq!(
        eez_protocol::system_tx::outbound_settlement_source_counts(
            std::slice::from_ref(host),
            &[(2, true)],
        )
        .unwrap(),
        vec![2],
        "both recursive pool entries belong to the single host checkpoint",
    );
    assert_eq!(host.l2ToL1Calls.len(), 1);
    assert_eq!(host.expectedL1ToL2Calls.len(), 2);
    assert_eq!(host.expectedL1ToL2Calls[0].l2ToL1Calls.len(), 1);
    assert!(host.expectedL1ToL2Calls[1].l2ToL1Calls.is_empty());
    let sources = sources
        .iter()
        .cloned()
        .map(OutboundSourceEntry::Static)
        .collect::<Vec<_>>();
    verify_outbound_source_group(host, &sources, l2).expect("independent recursive static binding");
    let mut forged = host.clone();
    forged.expectedL1ToL2Calls[0].revertedOrStaticRollingHash = B256::ZERO;
    let error = verify_outbound_source_group(&forged, &sources, l2)
        .expect_err("a callback cannot omit its own descendant commitment");
    assert!(
        error.to_string().contains("static row callback commitment"),
        "{error}"
    );
}

#[test]
fn excessive_static_depth_is_rejected_before_recursive_materialization() {
    let l2 = RollupId(1);
    let calls = (0usize..1026)
        .map(|index| {
            let (source, target) = if index % 2 == 0 {
                (l2, RollupId::MAINNET)
            } else {
                (RollupId::MAINNET, l2)
            };
            static_call(index.checked_sub(1), source, target)
        })
        .collect();
    let error =
        materialize_actions(l2, calls).expect_err("untrusted depth is bounded before recursion");
    assert!(
        error
            .to_string()
            .contains("depth exceeds the materializer limit"),
        "{error}"
    );
}

#[test]
fn inbound_recursive_reads_pin_and_observe_every_source_pool_entry() {
    use eez_protocol::settlement::{FifoTransactionRootPlan, fifo_observation_claims};
    let l2 = RollupId(1);
    let calls = vec![
        static_call(None, RollupId::MAINNET, l2),
        static_call(Some(0), l2, RollupId::MAINNET),
        static_call(Some(1), RollupId::MAINNET, l2),
        static_call(Some(2), l2, RollupId::MAINNET),
    ];
    let composition = materialize_actions(RollupId::MAINNET, calls).unwrap();
    let root = B256::with_last_byte(0x42);
    let plan = FifoTransactionRootPlan {
        mutable_new_roots: Vec::new(),
        static_roots: vec![root, root],
        endpoint: root,
    };
    let claims = fifo_observation_claims(&composition, &plan).unwrap();
    assert_eq!(composition.source.batch.staticEntries.len(), 2);
    assert!(composition.targets.is_empty());
    assert_eq!(claims.len(), 2);
    for claim in &claims {
        assert_eq!(claim.pinned_block_hash, root);
        assert_eq!(
            claim.callbacks.len(),
            1,
            "only the immediate callback is supplied"
        );
    }
    let mut omitted = plan;
    omitted.static_roots.pop();
    let error = fifo_observation_claims(&composition, &omitted)
        .expect_err("a nested pool lookup still requires an independently witnessed observation");
    assert!(error.contains("expected 0/2"), "{error}");
}

#[test]
fn static_callbacks_inside_mutable_hosts_bind_both_row_stacks() {
    let l2 = RollupId(1);
    for read_success in [true, false] {
        let mut root = static_call(None, l2, RollupId::MAINNET);
        root.call_mode = CallMode::Mutable;
        let mut read = static_call(Some(0), RollupId::MAINNET, l2);
        read.outcome = ExecutionOutcome::Resolved {
            success: read_success,
            return_data: vec![0x77],
            gas_used: 21_000,
        };
        let calls = vec![
            root,
            read,
            static_call(Some(1), l2, RollupId::MAINNET),
            static_call(Some(2), RollupId::MAINNET, l2),
        ];
        let mut composition = materialize_actions(l2, calls).unwrap();
        assert!(composition.source.l2_static_entries.is_empty());
        let target = &mut composition.targets[0];
        target.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(1),
            newRoot: B256::with_last_byte(2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut target.batch, &target.entry_traces).unwrap();
        let sources = [OutboundSourceEntry::Mutable(
            composition.source.batch.entries[0].clone(),
        )];
        let host = &target.batch.entries[0];
        verify_outbound_source_group(host, &sources, l2).expect("nested static row commitments");
        let mut forged = host.clone();
        forged.expectedL1ToL2Calls[0].revertedOrStaticRollingHash = B256::with_last_byte(0x99);
        let error = verify_outbound_source_group(&forged, &sources, l2)
            .expect_err("a caught static failure still binds all callbacks");
        assert!(
            error.to_string().contains("nested static callback results"),
            "{error}"
        );
    }
}

#[test]
fn static_candidates_distinguish_local_state_by_callback_commitment() {
    let l2 = RollupId(1);
    let mut calls = Vec::new();
    for value in [1, 2, 1] {
        let root_index = calls.len();
        let mut read = static_call(None, l2, RollupId::MAINNET);
        let mut callback = static_call(Some(root_index), RollupId::MAINNET, l2);
        for call in [&mut read, &mut callback] {
            call.outcome = ExecutionOutcome::Resolved {
                success: true,
                return_data: vec![value],
                gas_used: 21_000,
            };
        }
        calls.extend([read, callback]);
    }
    let mut composition = materialize_actions(l2, calls).unwrap();
    let target = &mut composition.targets[0];
    target.batch.entries[0].rootUpdates.push(RootUpdateSol {
        rollupId: l2.0,
        currentRoot: B256::with_last_byte(1),
        newRoot: B256::with_last_byte(2),
        etherDelta: EtherDeltaSol::ZERO,
    });
    finalize_l1_entry_traces(&mut target.batch, &target.entry_traces).unwrap();
    let mut sources = composition
        .source
        .l2_static_entries
        .iter()
        .cloned()
        .map(OutboundSourceEntry::Static)
        .collect::<Vec<_>>();
    verify_outbound_source_group(&target.batch.entries[0], &sources, l2)
        .expect("same cursor and call key can distinguish different callback state");
    let OutboundSourceEntry::Static(last) = &mut sources[2] else {
        unreachable!()
    };
    last.returnData = Bytes::from_static(&[9]);
    let error = verify_outbound_source_group(&target.batch.entries[0], &sources, l2)
        .expect_err("same complete candidate commitment cannot authorize different outputs");
    assert!(error.to_string().contains("conflicting results"), "{error}");
}
