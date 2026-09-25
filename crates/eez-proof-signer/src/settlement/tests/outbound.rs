use super::*;

fn nested_position_key(call_hash: B256, rolling_hash: B256) -> B256 {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(call_hash.as_slice());
    input[32..].copy_from_slice(rolling_hash.as_slice());
    alloy_primitives::keccak256(input)
}

fn nested_outbound_fixture() -> (CanonicalPostBatch, ExecutionEntrySol, Vec<B256>) {
    let make_call = |index: u8, source_rollup_id: u64| L2ToL1CallSol {
        revertNextNCalls: 0,
        isStatic: false,
        gas: 0,
        sourceAddress: Address::with_last_byte(0x10 + index),
        sourceRollupId: source_rollup_id,
        targetAddress: Address::with_last_byte(0x20 + index),
        value: U256::ZERO,
        data: Bytes::from(vec![0x30 + index]),
    };
    let calls = [
        make_call(0, 1),
        make_call(1, 0),
        make_call(2, 1),
        make_call(3, 0),
    ];
    let returns = [
        Bytes::from_static(&[0x40]),
        Bytes::from_static(&[0x41]),
        Bytes::from_static(&[0x42]),
        Bytes::from_static(&[0x43]),
    ];
    let common_hash = |call: &L2ToL1CallSol, target: RollupId| {
        common_cross_chain_call_hash(CallHashInput {
            call_mode: CallMode::Mutable,
            source_address: call.sourceAddress,
            source_rollup_id: RollupId(call.sourceRollupId),
            target_address: call.targetAddress,
            target_rollup_id: target,
            value: call.value,
            data: &call.data,
        })
    };
    let l2_hash = |call: &L2ToL1CallSol| {
        l2_outbound_call_hash(
            CallHashInput {
                call_mode: CallMode::Mutable,
                source_address: call.sourceAddress,
                source_rollup_id: RollupId(1),
                target_address: call.targetAddress,
                target_rollup_id: RollupId::MAINNET,
                value: call.value,
                data: &call.data,
            },
            0,
        )
    };
    let l2_outbound_hashes = vec![l2_hash(&calls[0]), l2_hash(&calls[2])];

    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let update = batch.entries[1].rootUpdates.clone();
    let mut l1_rolling = EntryRollingHash::seed_for_l1([(1, B256::ZERO)], B256::ZERO);
    l1_rolling.call_begin(common_hash(&calls[0], RollupId::MAINNET));
    let row0_key = nested_position_key(common_hash(&calls[1], RollupId(1)), l1_rolling.current());
    l1_rolling.nested_begin(common_hash(&calls[1], RollupId(1)));
    l1_rolling.call_begin(common_hash(&calls[2], RollupId::MAINNET));
    let row1_key = nested_position_key(common_hash(&calls[3], RollupId(1)), l1_rolling.current());
    l1_rolling.nested_begin(common_hash(&calls[3], RollupId(1)));
    l1_rolling.nested_end();
    l1_rolling.call_end(true, &returns[2]);
    l1_rolling.nested_end();
    l1_rolling.call_end(true, &returns[0]);
    batch.entries[1] = ExecutionEntrySol {
        rootUpdates: update,
        proxyEntryHash: B256::ZERO,
        l2ToL1Calls: vec![calls[0].clone()],
        expectedL1ToL2Calls: vec![
            ExpectedL1ToL2CallSol {
                expectedL1toL2Hash: row0_key,
                l2ToL1Calls: vec![calls[2].clone()],
                revertedOrStaticRollingHash: B256::ZERO,
                success: true,
                returnData: returns[1].clone(),
            },
            ExpectedL1ToL2CallSol {
                expectedL1toL2Hash: row1_key,
                l2ToL1Calls: Vec::new(),
                revertedOrStaticRollingHash: B256::ZERO,
                success: true,
                returnData: returns[3].clone(),
            },
        ],
        rollingHash: l1_rolling.current(),
        destinationRollupId: 1,
        success: true,
        returnData: Bytes::new(),
    };

    let mut l2_rolling = EntryRollingHash::seed_for_l2(l2_outbound_hashes[0]);
    l2_rolling.call_begin(common_hash(&calls[1], RollupId(1)));
    let l2_row_key = nested_position_key(l2_outbound_hashes[1], l2_rolling.current());
    l2_rolling.nested_begin(l2_outbound_hashes[1]);
    l2_rolling.call_begin(common_hash(&calls[3], RollupId(1)));
    l2_rolling.call_end(true, &returns[3]);
    l2_rolling.nested_end();
    l2_rolling.call_end(true, &returns[1]);
    let source_entry = ExecutionEntrySol {
        proxyEntryHash: l2_outbound_hashes[0],
        l2ToL1Calls: vec![calls[1].clone()],
        expectedL1ToL2Calls: vec![ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: l2_row_key,
            l2ToL1Calls: vec![calls[3].clone()],
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: returns[2].clone(),
        }],
        rollingHash: l2_rolling.current(),
        destinationRollupId: RollupId::MAINNET.0,
        success: true,
        returnData: returns[0].clone(),
        ..Default::default()
    };
    (batch, source_entry, l2_outbound_hashes)
}

#[test]
fn outbound_authorizer_accepts_four_hop_nested_tables_and_rejects_row_tampering() {
    let (batch, source_entry, event_hashes) = nested_outbound_fixture();
    let make_settling = |source_entry: ExecutionEntrySol| {
        let mut settling = SettlingBlockObservations::for_test(
            vec![true, false],
            Vec::new(),
            event_hashes
                .iter()
                .enumerate()
                .map(|(log_index, hash)| {
                    OutboundEventObservation::decoded_for_test(1, log_index, *hash, 0)
                })
                .collect(),
        );
        settling
            .outbound_load_candidates_mut_for_test()
            .push(OutboundLoadCandidate::decoded_for_test(0, source_entry));
        settling
    };

    let settling = make_settling(source_entry.clone());
    let plan = effect_plan(&batch, &settling);
    let authorized = authorize_outbound_effects(&plan).expect("authorize nested pair");
    assert_eq!(authorized.len(), 1);
    assert_eq!(
        authorized
            .iter()
            .next()
            .unwrap()
            .derived_da_entry()
            .expectedL1ToL2Calls
            .len(),
        1
    );
    assert_eq!(
        authorized
            .iter()
            .next()
            .unwrap()
            .derived_callback_entries()
            .len(),
        0
    );

    let mut tampered_source = source_entry;
    tampered_source.expectedL1ToL2Calls[0].expectedL1toL2Hash = B256::repeat_byte(0xee);
    let settling = make_settling(tampered_source);
    let plan = effect_plan(&batch, &settling);
    assert!(matches!(
        authorize_outbound_effects(&plan),
        Err(OutboundEffectError::ExecutionTableMismatch { entry_index: 1, .. })
    ));
}

#[test]
fn only_positioned_outbound_observation_failures_identify_a_user_transaction() {
    assert_eq!(
        OutboundEffectError::MissingObservation {
            entry_index: 1,
            transaction_index: 3,
        }
        .poisoned_transaction_index(),
        Some(3)
    );
    assert_eq!(
        OutboundEffectError::UnexpectedObservation {
            transaction_index: 2,
            receipt_log_index: 0,
        }
        .poisoned_transaction_index(),
        None
    );
    assert_eq!(
        OutboundEffectError::ExecutionTableMismatch {
            entry_index: 1,
            reason: "mismatched source table".to_string(),
        }
        .poisoned_transaction_index(),
        None
    );
    assert_eq!(
        OutboundEffectError::NestedCallHashMismatch {
            entry_index: 1,
            event_index: 0,
            transaction_index: 3,
            recomputed: B256::ZERO,
            observed: B256::repeat_byte(0x22),
        }
        .poisoned_transaction_index(),
        None
    );
}

fn refresh_outbound_rolling_hash(entry: &mut ExecutionEntrySol) {
    let [update] = entry.rootUpdates.as_slice() else {
        panic!("outbound fixture must contain one root update");
    };
    let mut rolling_hash = eez_protocol::rolling_hash::EntryRollingHash::seed_for_l1(
        [(update.rollupId, update.currentRoot)],
        entry.proxyEntryHash,
    );
    for call in &entry.l2ToL1Calls {
        let call_hash = eez_protocol::common_cross_chain_call_hash(CallHashInput {
            call_mode: eez_protocol::CallMode::Mutable,
            source_address: call.sourceAddress,
            source_rollup_id: RollupId(call.sourceRollupId),
            target_address: call.targetAddress,
            target_rollup_id: RollupId::MAINNET,
            value: call.value,
            data: &call.data,
        });
        rolling_hash.call_begin(call_hash);
        rolling_hash.call_end(entry.success, &entry.returnData);
    }
    entry.rollingHash = rolling_hash.current();
}

#[test]
fn outbound_event_and_l1_rolling_hash_share_the_zero_gas_identity() {
    let batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let entry = &batch.entries[1];
    let call = &entry.l2ToL1Calls[0];
    let input = CallHashInput {
        call_mode: CallMode::Mutable,
        source_address: call.sourceAddress,
        source_rollup_id: RollupId(call.sourceRollupId),
        target_address: call.targetAddress,
        target_rollup_id: RollupId::MAINNET,
        value: call.value,
        data: &call.data,
    };
    let event_hash = eez_protocol::l2_outbound_call_hash(input, 0);
    let l1_call_hash = eez_protocol::common_cross_chain_call_hash(input);

    assert_eq!(event_hash, l1_call_hash);
    let mut expected_rolling = eez_protocol::rolling_hash::EntryRollingHash::seed_for_l1(
        [(
            entry.rootUpdates[0].rollupId,
            entry.rootUpdates[0].currentRoot,
        )],
        entry.proxyEntryHash,
    );
    expected_rolling.call_begin(event_hash);
    expected_rolling.call_end(entry.success, &entry.returnData);
    assert_eq!(entry.rollingHash, expected_rolling.current());
}

#[test]
fn outbound_events_bind_by_transaction_and_preserve_duplicate_hashes() {
    let mut batch = effect_batch(
        &[B256::ZERO; 4],
        &[ClaimedEntryShape::Outbound, ClaimedEntryShape::Outbound],
    );
    batch.entries[1].l2ToL1Calls[0].data = Bytes::from_static(&[0x01]);
    batch.entries[2].l2ToL1Calls[0].data = Bytes::from_static(&[0x02]);
    refresh_outbound_rolling_hash(&mut batch.entries[1]);
    refresh_outbound_rolling_hash(&mut batch.entries[2]);
    let mut settling = settling_with_outbound_pairs(&batch);
    *settling.outbound_event_candidates_mut_for_test() = vec![
        observed_outbound_call(1, 0, &batch.entries[1].l2ToL1Calls[0]),
        observed_outbound_call(3, 0, &batch.entries[2].l2ToL1Calls[0]),
    ];
    let plan = effect_plan(&batch, &settling);
    let authorized = authorize_outbound_effects(&plan).unwrap();
    for binding in authorized.iter() {
        assert!(binding.derived_da_entry().rootUpdates.is_empty());
        assert_ne!(binding.derived_da_entry().proxyEntryHash, B256::ZERO);
        assert_eq!(
            binding.derived_da_entry().rollingHash,
            EntryRollingHash::seed_for_l2(binding.derived_da_entry().proxyEntryHash).current()
        );
    }

    let duplicate_batch = effect_batch(
        &[B256::ZERO; 4],
        &[ClaimedEntryShape::Outbound, ClaimedEntryShape::Outbound],
    );
    let call = &duplicate_batch.entries[1].l2ToL1Calls[0];
    let mut duplicates = settling_with_outbound_pairs(&duplicate_batch);
    *duplicates.outbound_event_candidates_mut_for_test() = vec![
        observed_outbound_call(1, 0, call),
        observed_outbound_call(3, 0, call),
    ];
    let duplicate_plan = effect_plan(&duplicate_batch, &duplicates);
    assert!(authorize_outbound_effects(&duplicate_plan).is_ok());

    let mut reordered = settling_with_outbound_pairs(&batch);
    *reordered.outbound_event_candidates_mut_for_test() = vec![
        observed_outbound_call(1, 0, &batch.entries[2].l2ToL1Calls[0]),
        observed_outbound_call(3, 0, &batch.entries[1].l2ToL1Calls[0]),
    ];
    let reordered_plan = effect_plan(&batch, &reordered);
    assert!(matches!(
        authorize_outbound_effects(&reordered_plan),
        Err(OutboundEffectError::NestedCallHashMismatch {
            entry_index: 1,
            transaction_index: 1,
            ..
        })
    ));
}

#[test]
fn two_outbound_roots_bind_to_one_load_and_one_user_transaction() {
    let mut batch = effect_batch(
        &[B256::ZERO; 4],
        &[ClaimedEntryShape::Outbound, ClaimedEntryShape::Outbound],
    );
    batch.entries[1].l2ToL1Calls[0].data = Bytes::from_static(&[0x11]);
    batch.entries[2].l2ToL1Calls[0].data = Bytes::from_static(&[0x22]);
    refresh_outbound_rolling_hash(&mut batch.entries[1]);
    refresh_outbound_rolling_hash(&mut batch.entries[2]);

    let mut settling = settling_with_system_flags(vec![true, false]);
    settling.outbound_load_candidates_mut_for_test().push(
        OutboundLoadCandidate::decoded_many_for_test(
            0,
            vec![
                flat_outbound_sidecar(&batch.entries[1]),
                flat_outbound_sidecar(&batch.entries[2]),
            ],
        ),
    );
    *settling.outbound_event_candidates_mut_for_test() = vec![
        observed_outbound_call(1, 0, &batch.entries[1].l2ToL1Calls[0]),
        observed_outbound_call(1, 1, &batch.entries[2].l2ToL1Calls[0]),
    ];

    let plan = effect_plan(&batch, &settling);
    let authorized = authorize_outbound_effects(&plan).unwrap();
    assert_eq!(authorized.len(), 2);
    assert_eq!(authorized.groups().map(<[_]>::len).collect::<Vec<_>>(), [2]);
    assert!(
        authorized
            .iter()
            .all(|binding| binding.load_transaction_index() == 0)
    );
    assert!(
        authorized
            .iter()
            .all(|binding| binding.transaction_index() == 1)
    );
}

#[test]
fn one_settlement_host_authorizes_two_flat_source_roots() {
    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let mut second = batch.entries[1].l2ToL1Calls[0].clone();
    second.data = Bytes::from_static(&[0x22]);
    batch.entries[1].l2ToL1Calls[0].data = Bytes::from_static(&[0x11]);
    batch.entries[1].l2ToL1Calls.push(second);
    let total_value = batch.entries[1]
        .l2ToL1Calls
        .iter()
        .fold(U256::ZERO, |total, call| total + call.value);
    batch.entries[1].rootUpdates[0].etherDelta = -EtherDeltaSol::from(total_value);
    refresh_outbound_rolling_hash(&mut batch.entries[1]);

    let sources = batch.entries[1]
        .l2ToL1Calls
        .iter()
        .map(|call| {
            flat_outbound_sidecar_for_call(
                call,
                batch.entries[1].success,
                batch.entries[1].returnData.clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut settling = settling_with_system_flags(vec![true, false]);
    settling
        .outbound_load_candidates_mut_for_test()
        .push(OutboundLoadCandidate::decoded_many_for_test(0, sources));
    *settling.outbound_event_candidates_mut_for_test() = batch.entries[1]
        .l2ToL1Calls
        .iter()
        .enumerate()
        .map(|(log_index, call)| observed_outbound_call(1, log_index, call))
        .collect();

    let plan = effect_plan(&batch, &settling);
    let authorized = authorize_outbound_effects(&plan).expect("authorize grouped flat host");
    assert_eq!(authorized.len(), 2);
    assert_eq!(authorized.groups().map(<[_]>::len).collect::<Vec<_>>(), [2]);
}

#[test]
fn root_revert_retry_reuses_one_loaded_source_and_one_surviving_event() {
    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let mut retry = batch.entries[1].l2ToL1Calls[0].clone();
    batch.entries[1].l2ToL1Calls[0].revertNextNCalls = 1;
    retry.revertNextNCalls = 0;
    batch.entries[1].l2ToL1Calls.push(retry);
    refresh_outbound_rolling_hash(&mut batch.entries[1]);

    let mut settling = settling_with_system_flags(vec![true, false]);
    settling
        .outbound_load_candidates_mut_for_test()
        .push(OutboundLoadCandidate::decoded_for_test(
            0,
            flat_outbound_sidecar(&batch.entries[1]),
        ));
    settling
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(
            1,
            0,
            &batch.entries[1].l2ToL1Calls[1],
        ));

    let plan = effect_plan(&batch, &settling);
    let authorized = authorize_outbound_effects(&plan)
        .expect("the retry reuses its restored source cursor slot");
    assert_eq!(authorized.len(), 1);
    assert_eq!(authorized.groups().map(<[_]>::len).collect::<Vec<_>>(), [1]);
}

#[test]
fn outbound_bindings_require_a_preceding_load_and_outbound_first_order() {
    let outbound_batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let mut missing_load = settling_with_system_flags(vec![false]);
    missing_load
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(
            0,
            0,
            &outbound_batch.entries[1].l2ToL1Calls[0],
        ));
    let missing_load_plan = effect_plan(&outbound_batch, &missing_load);
    assert_eq!(
        authorize_outbound_effects(&missing_load_plan).err(),
        Some(OutboundEffectError::MissingPrecedingSystemTransaction {
            entry_index: 1,
            transaction_index: 0,
        })
    );

    let mut mixed_batch = effect_batch(
        &[B256::ZERO; 4],
        &[ClaimedEntryShape::Inbound, ClaimedEntryShape::Outbound],
    );
    mixed_batch.entries[1].rootUpdates[0].etherDelta = EtherDeltaSol::ONE;
    let mut inbound_first = settling_with_system_flags(vec![true, true, false]);
    inbound_first
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(
            2,
            0,
            &mixed_batch.entries[2].l2ToL1Calls[0],
        ));
    let inbound_first_plan = effect_plan(&mixed_batch, &inbound_first);
    assert_eq!(
        authorize_outbound_effects(&inbound_first_plan).err(),
        Some(OutboundEffectError::NonCanonicalEffectOrder {
            entry_index: 2,
            transaction_index: 2,
        })
    );
}

#[test]
fn outbound_events_reject_missing_extra_multiple_and_malformed_observations() {
    let outbound_batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let call = &outbound_batch.entries[1].l2ToL1Calls[0];

    let missing = settling_with_outbound_pairs(&outbound_batch);
    let missing_plan = effect_plan(&outbound_batch, &missing);
    assert_eq!(
        authorize_outbound_effects(&missing_plan).err(),
        Some(OutboundEffectError::MissingObservation {
            entry_index: 1,
            transaction_index: 1,
        })
    );

    let mut malformed = settling_with_outbound_pairs(&outbound_batch);
    malformed
        .outbound_event_candidates_mut_for_test()
        .push(OutboundEventObservation::malformed_for_test(1, 3));
    let malformed_plan = effect_plan(&outbound_batch, &malformed);
    assert_eq!(
        authorize_outbound_effects(&malformed_plan).err(),
        Some(OutboundEffectError::MalformedObservation {
            transaction_index: 1,
            receipt_log_index: 3,
        })
    );

    let observation = observed_outbound_call(1, 0, call);
    let decoded_event = observation.decoded_event().unwrap();
    let mut multiple = settling_with_outbound_pairs(&outbound_batch);
    *multiple.outbound_event_candidates_mut_for_test() = vec![
        observation,
        OutboundEventObservation::decoded_for_test(
            observation.transaction_index(),
            1,
            decoded_event.call_hash(),
            decoded_event.call_gas(),
        ),
    ];
    let multiple_plan = effect_plan(&outbound_batch, &multiple);
    assert_eq!(
        authorize_outbound_effects(&multiple_plan).err(),
        Some(OutboundEffectError::ObservationCountMismatch {
            entry_index: 1,
            transaction_index: 1,
            expected: 1,
            actual: 2,
        })
    );

    let mut extra = settling_with_outbound_pairs(&outbound_batch);
    *extra.outbound_event_candidates_mut_for_test() = vec![
        observation,
        OutboundEventObservation::decoded_for_test(
            2,
            0,
            decoded_event.call_hash(),
            decoded_event.call_gas(),
        ),
    ];
    let extra_plan = effect_plan(&outbound_batch, &extra);
    assert_eq!(
        authorize_outbound_effects(&extra_plan).err(),
        Some(OutboundEffectError::UnexpectedObservation {
            transaction_index: 2,
            receipt_log_index: 0,
        })
    );

    // The system/user pair makes transaction 1 the outbound effect position;
    // an otherwise matching event from the preceding transaction cannot be
    // reassociated with it.
    let mut early = settling_with_system_flags(vec![true, false]);
    early
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(0, 0, call));
    let early_plan = effect_plan(&outbound_batch, &early);
    assert_eq!(
        authorize_outbound_effects(&early_plan).err(),
        Some(OutboundEffectError::UnexpectedObservation {
            transaction_index: 0,
            receipt_log_index: 0,
        })
    );

    let mut inbound_batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Inbound]);
    inbound_batch.entries[1].rootUpdates[0].etherDelta = EtherDeltaSol::ONE;
    let mut inbound = settling_with_system_flags(vec![true]);
    inbound
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(0, 0, call));
    let inbound_plan = effect_plan(&inbound_batch, &inbound);
    assert!(
        authorize_outbound_effects(&inbound_plan).is_ok(),
        "events emitted by inbound nested calls belong to the inbound authorizer"
    );
}

#[test]
fn fully_reverted_outbound_root_is_authorized_without_an_impossible_event() {
    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    batch.entries[1].l2ToL1Calls[0].revertNextNCalls = 1;
    // A root-level force-revert suppresses the receipt event and destination
    // state, but EEZ preserves the call fold in the host entry's rolling hash.
    refresh_outbound_rolling_hash(&mut batch.entries[1]);
    let settling = settling_with_outbound_pairs(&batch);
    let plan = effect_plan(&batch, &settling);

    let authorized = authorize_outbound_effects(&plan)
        .expect("the rollback marker authorizes a call whose receipt event reverted");
    assert_eq!(authorized.len(), 1);
}

#[test]
fn naturally_reverted_outbound_root_is_authorized_without_an_impossible_event() {
    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let failed_return = Bytes::from_static(b"L1 target reverted");
    let root = batch.entries[1].l2ToL1Calls[0].clone();
    let update = &batch.entries[1].rootUpdates[0];
    let mut rolling = EntryRollingHash::seed_for_l1(
        [(update.rollupId, update.currentRoot)],
        batch.entries[1].proxyEntryHash,
    );
    rolling.call_begin(common_cross_chain_call_hash(CallHashInput {
        call_mode: CallMode::Mutable,
        source_address: root.sourceAddress,
        source_rollup_id: RollupId(root.sourceRollupId),
        target_address: root.targetAddress,
        target_rollup_id: RollupId::MAINNET,
        value: root.value,
        data: &root.data,
    }));
    rolling.call_end(false, &failed_return);
    batch.entries[1].rollingHash = rolling.current();
    batch.entries[1].rootUpdates[0].etherDelta = EtherDeltaSol::ZERO;

    let mut settling = settling_with_system_flags(vec![true, false]);
    settling
        .outbound_load_candidates_mut_for_test()
        .push(OutboundLoadCandidate::decoded_for_test(
            0,
            flat_outbound_sidecar_for_call(&root, false, failed_return),
        ));
    let plan = effect_plan(&batch, &settling);

    let authorized = authorize_outbound_effects(&plan)
        .expect("a naturally reverting proxy frame cannot retain its receipt event");
    assert_eq!(authorized.len(), 1);
}

#[test]
fn outbound_events_reject_nonzero_manager_entry_gas() {
    let batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let call = &batch.entries[1].l2ToL1Calls[0];
    let call_gas = 1;
    let mut settling = settling_with_outbound_pairs(&batch);
    settling
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call_with_gas(1, 2, call, call_gas));

    assert_eq!(
        authorize_outbound_effects(&effect_plan(&batch, &settling)).err(),
        Some(OutboundEffectError::UnsupportedCallGas {
            transaction_index: 1,
            receipt_log_index: 2,
            actual: call_gas,
        })
    );
}

#[test]
fn outbound_events_bind_the_single_call_and_expected_rollup() {
    let valid = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let observation = observed_outbound_call(1, 0, &valid.entries[1].l2ToL1Calls[0]);

    let verify = |batch: &CanonicalPostBatch| {
        let mut settling = settling_with_outbound_pairs(batch);
        settling
            .outbound_event_candidates_mut_for_test()
            .push(observation);
        let plan = effect_plan(batch, &settling);
        authorize_outbound_effects(&plan).err()
    };

    let mut wrong_destination = valid.clone();
    wrong_destination.entries[1].destinationRollupId = 2;
    assert_eq!(
        verify(&wrong_destination),
        Some(OutboundEffectError::DestinationRollupMismatch {
            entry_index: 1,
            expected: 1,
            actual: 2,
        })
    );

    let mut wrong_source = valid.clone();
    wrong_source.entries[1].l2ToL1Calls[0].sourceRollupId = 2;
    assert_eq!(
        verify(&wrong_source),
        Some(OutboundEffectError::SourceRollupMismatch {
            entry_index: 1,
            expected: 1,
            actual: 2,
        })
    );

    let hash_mutations: [fn(&mut L2ToL1CallSol); 4] = [
        |call| call.targetAddress = Address::repeat_byte(0xcc),
        |call| call.value = U256::from(8),
        |call| call.data = Bytes::from_static(&[0xff]),
        |call| call.sourceAddress = Address::repeat_byte(0xdd),
    ];
    for mutate in hash_mutations {
        let mut wrong_call = valid.clone();
        mutate(&mut wrong_call.entries[1].l2ToL1Calls[0]);
        assert!(matches!(
            verify(&wrong_call),
            Some(OutboundEffectError::ExecutionTableMismatch { entry_index: 1, .. })
        ));
    }
}

#[test]
fn outbound_effect_prefix_rejects_unsupported_target_shapes() {
    let valid = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let settling = settling_with_outbound_pairs(&valid);
    let checkpoints = [checkpoint(1, B256::ZERO)];
    let shape_mutations: [fn(&mut ExecutionEntrySol); 2] = [
        |entry| entry.success = false,
        |entry| entry.l2ToL1Calls[0].gas = 1,
    ];
    for mutate in shape_mutations {
        let mut batch = valid.clone();
        mutate(&mut batch.entries[1]);
        assert_eq!(
            verify_effect_prefix(&batch, B256::ZERO, &checkpoints, &settling).err(),
            Some(EffectPrefixError::InvalidEntry { entry_index: 1 })
        );
    }

    let mut static_call = valid.clone();
    static_call.entries[1].l2ToL1Calls[0].isStatic = true;
    assert!(verify_effect_prefix(&static_call, B256::ZERO, &checkpoints, &settling).is_ok());

    let mut reverted = valid.clone();
    reverted.entries[1].l2ToL1Calls[0].revertNextNCalls = 1;
    assert!(verify_effect_prefix(&reverted, B256::ZERO, &checkpoints, &settling).is_ok());

    for enrich in [
        |entry: &mut ExecutionEntrySol| entry.expectedL1ToL2Calls.push(expected_call()),
        |entry: &mut ExecutionEntrySol| entry.l2ToL1Calls.push(l2_to_l1_call()),
    ] {
        let mut batch = valid.clone();
        enrich(&mut batch.entries[1]);
        assert!(verify_effect_prefix(&batch, B256::ZERO, &checkpoints, &settling).is_ok());
    }
}

#[test]
fn outbound_authorizer_rejects_wrong_l1_rolling_hash() {
    let valid = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let verify = |batch: &CanonicalPostBatch| {
        let mut settling = settling_with_outbound_pairs(batch);
        settling
            .outbound_event_candidates_mut_for_test()
            .push(observed_outbound_call(
                1,
                0,
                &batch.entries[1].l2ToL1Calls[0],
            ));
        let plan = effect_plan(batch, &settling);
        authorize_outbound_effects(&plan).err()
    };

    let mut wrong_rolling_hash = valid;
    wrong_rolling_hash.entries[1].rollingHash = B256::repeat_byte(2);
    assert!(matches!(
        verify(&wrong_rolling_hash),
        Some(OutboundEffectError::ExecutionTableMismatch { entry_index: 1, .. })
    ));
}

#[test]
fn outbound_effects_require_a_successful_outcome_and_exact_value_accounting() {
    let valid = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let verify = |batch: &CanonicalPostBatch| {
        let mut settling = settling_with_outbound_pairs(batch);
        settling
            .outbound_event_candidates_mut_for_test()
            .push(observed_outbound_call(
                1,
                0,
                &batch.entries[1].l2ToL1Calls[0],
            ));
        let plan = effect_plan(batch, &settling);
        authorize_outbound_effects(&plan).err()
    };

    let mut noncanonical_host_return_data = valid.clone();
    noncanonical_host_return_data.entries[1].returnData = Bytes::from_static(&[0xca, 0xfe]);
    refresh_outbound_rolling_hash(&mut noncanonical_host_return_data.entries[1]);
    assert!(matches!(
        verify(&noncanonical_host_return_data),
        Some(OutboundEffectError::ExecutionTableMismatch { entry_index: 1, .. })
    ));

    let mut wrong_delta = valid.clone();
    wrong_delta.entries[1].rootUpdates[0].etherDelta = EtherDeltaSol::ONE;
    assert_eq!(
        verify(&wrong_delta),
        Some(OutboundEffectError::EtherDeltaMismatch {
            entry_index: 1,
            expected: I256::ZERO,
            actual: I256::ONE,
        })
    );

    for value in [U256::from(1), U256::from(1) << 191] {
        let mut value_bearing = valid.clone();
        value_bearing.entries[1].l2ToL1Calls[0].value = value;
        value_bearing.entries[1].rootUpdates[0].etherDelta = if value == U256::from(1) << 191 {
            EtherDeltaSol::MIN
        } else {
            -EtherDeltaSol::from(value)
        };
        refresh_outbound_rolling_hash(&mut value_bearing.entries[1]);
        assert_eq!(verify(&value_bearing), None);

        value_bearing.entries[1].rootUpdates[0].etherDelta += EtherDeltaSol::ONE;
        assert_eq!(
            verify(&value_bearing),
            Some(OutboundEffectError::EtherDeltaMismatch {
                entry_index: 1,
                expected: -I256::try_from(value).unwrap(),
                actual: -I256::try_from(value).unwrap() + I256::ONE,
            })
        );
    }

    for value in [(U256::from(1) << 191) + U256::from(1), U256::MAX] {
        let mut out_of_range = valid.clone();
        out_of_range.entries[1].l2ToL1Calls[0].value = value;
        refresh_outbound_rolling_hash(&mut out_of_range.entries[1]);
        assert_eq!(
            verify(&out_of_range),
            Some(OutboundEffectError::ValueOutOfRange {
                entry_index: 1,
                value,
            })
        );
    }

    let mut reserved_source = valid;
    reserved_source.entries[1].l2ToL1Calls[0].sourceAddress = TEST_SYSTEM_ADDRESS;
    refresh_outbound_rolling_hash(&mut reserved_source.entries[1]);
    assert_eq!(
        verify(&reserved_source),
        Some(OutboundEffectError::ReservedSourceAddress { entry_index: 1 })
    );

    let mut settling = settling_with_outbound_pairs(&reserved_source);
    settling
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(
            1,
            0,
            &reserved_source.entries[1].l2ToL1Calls[0],
        ));
    let plan = effect_plan(&reserved_source, &settling);
    assert!(
        authorize_outbound_effects_for_rollup(
            &plan,
            expected_rollup_id(),
            Address::repeat_byte(0xbb),
        )
        .is_ok()
    );
}

/// Construct actual canonical load calldata; the tests exercise the strict
/// decoder as well as effect binding and outbound authorization.
fn mixed_outbound_fixture(kinds: &[bool]) -> (CanonicalPostBatch, SettlingBlockObservations) {
    mixed_outbound_fixture_with_results(kinds, false)
}

fn mixed_outbound_fixture_with_results(
    kinds: &[bool],
    changing: bool,
) -> (CanonicalPostBatch, SettlingBlockObservations) {
    use eez_protocol::abi::loadExecutionTableCall;
    use eez_protocol::system_tx::{lower_entry_to_l2, lower_static_entry_to_l2};
    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let template = batch.entries[1].l2ToL1Calls[0].clone();
    let host = &mut batch.entries[1];
    host.l2ToL1Calls.clear();
    host.rootUpdates[0].etherDelta = EtherDeltaSol::ZERO;
    let mut rolling = EntryRollingHash::seed_for_l1([(1, B256::ZERO)], B256::ZERO);
    let mut mutable = Vec::new();
    let mut statics = Vec::new();
    let mut settling = settling_with_system_flags(vec![true, false]);
    for (index, is_static) in kinds.iter().copied().enumerate() {
        let mut call = template.clone();
        call.isStatic = is_static;
        call.value = U256::ZERO;
        call.data = Bytes::from(vec![if is_static {
            0x10 + (statics.len() % 2) as u8
        } else {
            0x22
        }]);
        let hash = common_cross_chain_call_hash(CallHashInput {
            call_mode: CallMode::from_is_static(is_static),
            source_address: call.sourceAddress,
            source_rollup_id: RollupId(1),
            target_address: call.targetAddress,
            target_rollup_id: RollupId::MAINNET,
            value: U256::ZERO,
            data: &call.data,
        });
        rolling.call_begin(hash);
        let result = if changing && is_static {
            Bytes::from(vec![mutable.len() as u8])
        } else {
            Bytes::new()
        };
        rolling.call_end(true, &result);
        if is_static {
            statics.push(lower_static_entry_to_l2(
                &StaticExecutionEntrySol {
                    proxyEntryHash: hash,
                    destinationRollupId: 0,
                    l2ToL1Calls: Vec::new(),
                    rollingHash: B256::ZERO,
                    success: true,
                    returnData: result,
                    expectedRoots: Vec::new(),
                },
                U256::from(mutable.len()),
            ));
        } else {
            mutable.push(lower_entry_to_l2(&flat_outbound_sidecar_for_call(
                &call,
                true,
                Bytes::new(),
            )));
            settling
                .outbound_event_candidates_mut_for_test()
                .push(observed_outbound_call(1, index, &call));
        }
        host.l2ToL1Calls.push(call);
    }
    host.rollingHash = rolling.current();
    let calldata = loadExecutionTableCall {
        _entries: mutable,
        _staticEntries: statics,
    }
    .abi_encode();
    settling
        .outbound_load_candidates_mut_for_test()
        .push(OutboundLoadCandidate {
            transaction_index: 0,
            inspection: super::super::outbound::inspect_outbound_load_candidate(
                U256::ZERO,
                &calldata,
                true,
            ),
        });
    (batch, settling)
}

#[test]
fn mixed_outbound_authorizes_reads_before_and_after_write() {
    for kinds in [
        vec![true, true, false, true, true],
        vec![false, true, true],
        vec![true, true, false],
        vec![true, true],
        vec![true],
        vec![false, true, false, true],
    ] {
        let (batch, settling) = mixed_outbound_fixture(&kinds);
        let plan = effect_plan(&batch, &settling);
        let authorized = authorize_outbound_effects(&plan).expect("authorize complete mixed host");
        assert_eq!(authorized.len(), kinds.len());
        assert_eq!(
            authorized.groups().map(<[_]>::len).collect::<Vec<_>>(),
            [kinds.len()]
        );
    }
}

#[test]
fn mixed_outbound_rejects_changed_host_outcome() {
    let (mut batch, settling) = mixed_outbound_fixture(&[true, true, false, true, true]);
    batch.entries[1].rollingHash = B256::repeat_byte(0x99);
    assert!(matches!(
        authorize_outbound_effects(&effect_plan(&batch, &settling)),
        Err(OutboundEffectError::ExecutionTableMismatch { .. })
    ));
}

#[test]
fn mixed_outbound_rejects_missing_mutable_event() {
    let (batch, mut settling) = mixed_outbound_fixture(&[true, true, false, true, true]);
    settling.outbound_event_candidates_mut_for_test().clear();
    assert!(matches!(
        authorize_outbound_effects(&effect_plan(&batch, &settling)),
        Err(OutboundEffectError::MissingObservation { .. })
    ));
}

#[test]
fn mixed_outbound_rejects_conflicting_cached_results() {
    use eez_protocol::abi::loadExecutionTableCall;
    use eez_protocol::system_tx::{OutboundSourceEntry, lower_entry_to_l2};
    let (batch, mut settling) = mixed_outbound_fixture(&[true, true, true, false]);
    let load = &mut settling.outbound_load_candidates_mut_for_test()[0];
    let sources = load.inspection.as_ref().unwrap().as_entries();
    let mut call = loadExecutionTableCall {
        _entries: Vec::new(),
        _staticEntries: Vec::new(),
    };
    for source in sources {
        match source {
            OutboundSourceEntry::Mutable(entry) => call._entries.push(lower_entry_to_l2(entry)),
            OutboundSourceEntry::Static(entry) => call._staticEntries.push(entry.clone()),
        }
    }
    call._staticEntries[2].returnData = Bytes::from_static(&[1]);
    load.inspection = super::super::outbound::inspect_outbound_load_candidate(
        U256::ZERO,
        &call.abi_encode(),
        true,
    );
    match authorize_outbound_effects(&effect_plan(&batch, &settling)) {
        Err(OutboundEffectError::ExecutionTableMismatch { reason, .. }) => {
            assert!(reason.contains("conflicting results"))
        }
        _ => panic!("expected conflicting static lookup rejection"),
    }
}

#[test]
fn mixed_outbound_rejects_extra_mutable_event() {
    let (batch, mut settling) = mixed_outbound_fixture(&[true, false, true]);
    settling
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(
            1,
            3,
            &batch.entries[1].l2ToL1Calls[1],
        ));
    assert!(matches!(
        authorize_outbound_effects(&effect_plan(&batch, &settling)),
        Err(OutboundEffectError::ObservationCountMismatch { .. })
    ));
}

#[test]
fn mixed_outbound_rejects_wrong_static_call_identity() {
    let (mut batch, settling) = mixed_outbound_fixture(&[true, false, true]);
    batch.entries[1].l2ToL1Calls[0].data = Bytes::from_static(&[0xfe]);
    match authorize_outbound_effects(&effect_plan(&batch, &settling)) {
        Err(OutboundEffectError::ExecutionTableMismatch { reason, .. }) => {
            assert!(reason.contains("source lookup"))
        }
        _ => panic!("expected static identity rejection"),
    }
}

#[test]
fn mixed_outbound_rejects_partitioning_the_static_pool_between_hosts() {
    for split in [1, 2] {
        let (mut batch, settling) = mixed_outbound_fixture(&[false, true, true, true, true]);
        let mut second = batch.entries[1].clone();
        second.l2ToL1Calls = batch.entries[1].l2ToL1Calls.split_off(split);
        batch.entries.push(second);
        batch.immediateEntryCount += U256::from(1);
        for host in &mut batch.entries[1..] {
            let mut rolling = EntryRollingHash::seed_for_l1([(1, B256::ZERO)], B256::ZERO);
            for call in &host.l2ToL1Calls {
                rolling.call_begin(common_cross_chain_call_hash(CallHashInput {
                    call_mode: CallMode::from_is_static(call.isStatic),
                    source_address: call.sourceAddress,
                    source_rollup_id: RollupId(1),
                    target_address: call.targetAddress,
                    target_rollup_id: RollupId::MAINNET,
                    value: call.value,
                    data: &call.data,
                }));
                rolling.call_end(true, &[]);
            }
            host.rollingHash = rolling.current();
        }
        match authorize_outbound_effects(&effect_plan(&batch, &settling)) {
            Err(OutboundEffectError::ExecutionTableMismatch { reason, .. }) => {
                assert!(reason.contains("complete source load"))
            }
            _ => panic!("expected complete static pool rejection"),
        }
    }
}

#[test]
fn mixed_outbound_authorizes_changing_results_at_distinct_positions() {
    let (batch, settling) =
        mixed_outbound_fixture_with_results(&[true, true, false, true, true], true);
    let authorized = authorize_outbound_effects(&effect_plan(&batch, &settling))
        .expect("position-bound changing reads");
    assert_eq!(authorized.len(), 5);
}

#[test]
fn mixed_outbound_rejects_static_position_tampering() {
    use eez_protocol::abi::loadExecutionTableCall;
    use eez_protocol::system_tx::{OutboundSourceEntry, lower_entry_to_l2};
    for wrong in [U256::ZERO, U256::from(2), U256::MAX] {
        let (batch, mut settling) = mixed_outbound_fixture(&[true, true, false, true, true]);
        let load = &mut settling.outbound_load_candidates_mut_for_test()[0];
        let mut call = loadExecutionTableCall {
            _entries: Vec::new(),
            _staticEntries: Vec::new(),
        };
        for source in load.inspection.as_ref().unwrap().as_entries() {
            match source {
                OutboundSourceEntry::Mutable(entry) => call._entries.push(lower_entry_to_l2(entry)),
                OutboundSourceEntry::Static(entry) => call._staticEntries.push(entry.clone()),
            }
        }
        call._staticEntries[2].expectedEntryIndex = wrong;
        load.inspection = super::super::outbound::inspect_outbound_load_candidate(
            U256::ZERO,
            &call.abi_encode(),
            true,
        );
        assert!(matches!(
            authorize_outbound_effects(&effect_plan(&batch, &settling)),
            Err(OutboundEffectError::ExecutionTableMismatch { .. })
        ));
    }
}

#[test]
fn outbound_load_rejects_the_previous_protocol_selector() {
    use super::super::outbound::{OutboundLoadObservationError, inspect_outbound_load_candidate};
    use eez_protocol::abi::loadExecutionTableCall;
    let mut calldata = loadExecutionTableCall {
        _entries: Vec::new(),
        _staticEntries: Vec::new(),
    }
    .abi_encode();
    calldata[..4].copy_from_slice(&[0xb3, 0x01, 0xbc, 0x80]);
    assert!(matches!(
        inspect_outbound_load_candidate(U256::ZERO, &calldata, true),
        Err(OutboundLoadObservationError::InvalidAbi { .. }
            | OutboundLoadObservationError::NonCanonicalAbi)
    ));
}
