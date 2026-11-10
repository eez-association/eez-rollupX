use super::*;

fn nested_position_key(call_hash: B256, rolling_hash: B256) -> B256 {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(call_hash.as_slice());
    input[32..].copy_from_slice(rolling_hash.as_slice());
    alloy_primitives::keccak256(input)
}

pub(super) fn nested_inbound_fixture() -> (CanonicalPostBatch, Vec<u8>) {
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
        make_call(0, 0),
        make_call(1, 1),
        make_call(2, 0),
        make_call(3, 1),
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
    let root_hash = common_hash(&calls[0], RollupId(1));

    let mut batch = effect_batch(
        &[B256::ZERO, B256::ZERO, B256::with_last_byte(1)],
        &[ClaimedEntryShape::Inbound],
    );
    let update = batch.entries[1].rootUpdates.clone();
    let mut l1_rolling = EntryRollingHash::seed_for_l1([(1, B256::ZERO)], root_hash);
    l1_rolling.call_begin(common_hash(&calls[1], RollupId::MAINNET));
    let l1_row_key = nested_position_key(common_hash(&calls[2], RollupId(1)), l1_rolling.current());
    l1_rolling.nested_begin(common_hash(&calls[2], RollupId(1)));
    l1_rolling.call_begin(common_hash(&calls[3], RollupId::MAINNET));
    l1_rolling.call_end(true, &returns[3]);
    l1_rolling.nested_end();
    l1_rolling.call_end(true, &returns[1]);
    batch.entries[1] = ExecutionEntrySol {
        rootUpdates: update,
        proxyEntryHash: root_hash,
        l2ToL1Calls: vec![calls[1].clone()],
        expectedL1ToL2Calls: vec![ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: l1_row_key,
            l2ToL1Calls: vec![calls[3].clone()],
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: returns[2].clone(),
        }],
        rollingHash: l1_rolling.current(),
        destinationRollupId: 1,
        success: true,
        returnData: returns[0].clone(),
    };

    let mut l2_rolling = EntryRollingHash::seed_for_l2(root_hash);
    l2_rolling.call_begin(root_hash);
    let row0_key = nested_position_key(l2_hash(&calls[1]), l2_rolling.current());
    l2_rolling.nested_begin(l2_hash(&calls[1]));
    l2_rolling.call_begin(common_hash(&calls[2], RollupId(1)));
    let row1_key = nested_position_key(l2_hash(&calls[3]), l2_rolling.current());
    l2_rolling.nested_begin(l2_hash(&calls[3]));
    l2_rolling.nested_end();
    l2_rolling.call_end(true, &returns[2]);
    l2_rolling.nested_end();
    l2_rolling.call_end(true, &returns[0]);
    let target_entry = ExecutionEntrySol {
        proxyEntryHash: root_hash,
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
        rollingHash: l2_rolling.current(),
        destinationRollupId: 1,
        success: true,
        returnData: returns[0].clone(),
        ..Default::default()
    };
    let calldata =
        encode_execute_incoming(eez_protocol::system_tx::lower_entry_to_l2(&target_entry));
    (batch, calldata)
}

#[test]
fn inbound_authorizer_accepts_four_hop_nested_tables_and_rejects_row_tampering() {
    let (batch, calldata) = nested_inbound_fixture();
    let observation = inspect_inbound_candidate(U256::ZERO, &calldata, true, expected_rollup_id());
    let settling = SettlingBlockObservations::for_test(
        vec![true],
        vec![InboundCandidate {
            transaction_index: 0,
            inspection: observation,
        }],
        vec![
            observed_outbound_call(0, 0, &batch.entries[1].l2ToL1Calls[0]),
            observed_outbound_call(
                0,
                1,
                &batch.entries[1].expectedL1ToL2Calls[0].l2ToL1Calls[0],
            ),
        ],
    );
    let plan = effect_plan(&batch, &settling);
    assert_eq!(
        verify_inbound_effect_entries(&plan)
            .expect("authorize nested inbound pair")
            .len(),
        1
    );

    let mut tampered = batch.clone();
    tampered.entries[1].expectedL1ToL2Calls[0].expectedL1toL2Hash = B256::repeat_byte(0xee);
    let plan = effect_plan(&tampered, &settling);
    assert!(matches!(
        verify_inbound_effect_entries(&plan),
        Err(InboundEffectError::ExecutionTableMismatch { entry_index: 1, .. })
    ));

    let mut wrong_event_settling = settling;
    wrong_event_settling.outbound_event_candidates_mut_for_test()[1] =
        observed_outbound_call(0, 1, &batch.entries[1].l2ToL1Calls[0]);
    let plan = effect_plan(&batch, &wrong_event_settling);
    assert!(matches!(
        verify_inbound_effect_entries(&plan),
        Err(InboundEffectError::NestedCallHashMismatch {
            entry_index: 1,
            event_index: 1,
            ..
        })
    ));
}

#[test]
fn only_a_reverted_positioned_inbound_identifies_an_entry() {
    assert_eq!(
        InboundEffectError::InvalidObservation {
            entry_index: 4,
            transaction_index: 7,
            source: InboundObservationError::RevertedTransaction,
        }
        .poisoned_entry_index(),
        Some(4)
    );
    assert_eq!(
        InboundEffectError::InvalidObservation {
            entry_index: 4,
            transaction_index: 7,
            source: InboundObservationError::NonCanonicalAbi,
        }
        .poisoned_entry_index(),
        None
    );
}

#[test]
fn inbound_candidate_requires_the_exact_l2_shape_and_rolling_hash() {
    use eez_protocol::abi::{
        ExpectedOutgoingCrossChainCallSol, L2StaticExecutionEntrySol,
        executeIncomingCrossChainCallCall,
    };

    let value = U256::from(5);
    let calldata = strict_inbound_calldata(value, true);
    let valid = inspect_inbound_candidate(value, &calldata, true, expected_rollup_id()).unwrap();
    let sidecar = valid.derived_da_entry.as_entry();
    let [sidecar_call] = sidecar.l2ToL1Calls.as_slice() else {
        panic!("derived inbound sidecar must contain one call");
    };
    assert!(sidecar.rootUpdates.is_empty());
    assert_eq!(sidecar.destinationRollupId, 1);
    assert!(sidecar.success);
    assert_eq!(sidecar_call.sourceRollupId, 0);
    assert_eq!(sidecar_call.revertNextNCalls, 0);
    assert!(!sidecar_call.isStatic);
    assert_eq!(sidecar_call.gas, 0);

    let call = executeIncomingCrossChainCallCall::abi_decode(&calldata).unwrap();
    let entry = &call._entries[0];
    assert!(entry.success);
    assert!(entry.expectedOutgoingCalls.is_empty());
    assert!(sidecar.expectedL1ToL2Calls.is_empty());
    let mut expected_rolling =
        eez_protocol::rolling_hash::EntryRollingHash::seed_for_l2(entry.proxyEntryHash);
    expected_rolling.call_begin(entry.proxyEntryHash);
    expected_rolling.call_end(true, &entry.returnData);
    assert_eq!(entry.rollingHash, expected_rolling.current());

    let inspect = |call: &executeIncomingCrossChainCallCall| {
        inspect_inbound_candidate(value, &call.abi_encode(), true, expected_rollup_id()).err()
    };

    let mut with_static = call.clone();
    with_static
        ._staticEntries
        .push(L2StaticExecutionEntrySol::default());
    assert_eq!(
        inspect(&with_static),
        Some(InboundObservationError::StaticEntryCount { actual: 1 })
    );

    let shape_mutations: [(fn(&mut executeIncomingCrossChainCallCall), &str); 2] = [
        (
            |call| call._entries[0].incomingCalls[0].isStatic = true,
            "isStatic",
        ),
        (|call| call._entries[0].incomingCalls[0].gas = 1, "gas"),
    ];
    for (mutate, field) in shape_mutations {
        let mut malformed = call.clone();
        mutate(&mut malformed);
        assert_eq!(
            inspect(&malformed),
            Some(InboundObservationError::InvalidEntryShape { field })
        );
    }

    let mut failed = call.clone();
    failed._entries[0].success = false;
    assert_eq!(
        inspect(&failed),
        Some(InboundObservationError::InvalidEntryShape {
            field: "executionTable"
        }),
        "failed outer deliveries are rejected at the checked conversion"
    );

    let mut reverted = call.clone();
    reverted._entries[0].incomingCalls[0].revertNextNCalls = 1;
    assert_eq!(inspect(&reverted), None);

    // Nested rows and their final rolling hash require the complementary L1
    // table for validation, so candidate inspection retains them for the
    // pair authorizer instead of rejecting them at the calldata boundary.
    let mut nested = call.clone();
    nested._entries[0]
        .expectedOutgoingCalls
        .push(ExpectedOutgoingCrossChainCallSol {
            expectedOutgoingHash: B256::ZERO,
            incomingCalls: Vec::new(),
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::new(),
        });
    assert!(
        inspect_inbound_candidate(value, &nested.abi_encode(), true, expected_rollup_id()).is_ok()
    );

    let mut invalid_flat_rolling_hash = call.clone();
    invalid_flat_rolling_hash._entries[0].rollingHash = B256::repeat_byte(0xaa);
    assert!(
        inspect_inbound_candidate(
            value,
            &invalid_flat_rolling_hash.abi_encode(),
            true,
            expected_rollup_id()
        )
        .is_err()
    );

    let mut wrong_source_rollup = call;
    wrong_source_rollup._entries[0].incomingCalls[0].sourceRollupId = 2;
    assert_eq!(
        inspect(&wrong_source_rollup),
        Some(InboundObservationError::SourceRollup { actual: 2 })
    );
}

#[test]
fn inbound_effect_prefix_rejects_unsupported_target_shapes() {
    let settling = SettlingBlockObservations::for_test(
        vec![true],
        vec![observed_inbound_candidate(0, U256::ZERO, true)],
        Vec::new(),
    );
    let valid = bindable_inbound_batch(&settling);
    let checkpoints = [checkpoint(0, valid.entries[1].rootUpdates[0].newRoot)];
    let shape_mutations: [fn(&mut ExecutionEntrySol); 1] = [|entry| {
        entry.success = false;
        entry.expectedL1ToL2Calls.push(expected_call());
    }];

    for mutate in shape_mutations {
        let mut batch = valid.clone();
        mutate(&mut batch.entries[1]);
        assert_eq!(
            verify_effect_prefix(&batch, B256::ZERO, &checkpoints, &settling).err(),
            Some(EffectPrefixError::InvalidEntry { entry_index: 1 })
        );
    }

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
fn inbound_effect_entries_bind_observations_positionally_and_preserve_duplicates() {
    let mut settling = SettlingBlockObservations::for_test(
        vec![true, true],
        vec![
            observed_inbound_candidate(0, U256::from(1), true),
            observed_inbound_candidate(1, U256::from(2), true),
        ],
        Vec::new(),
    );
    let batch = bindable_inbound_batch(&settling);
    {
        let plan = effect_plan(&batch, &settling);
        assert!(verify_inbound_effect_entries(&plan).is_ok());

        let mut reordered = batch.clone();
        let first_updates = reordered.entries[1].rootUpdates.clone();
        let second_updates = reordered.entries[2].rootUpdates.clone();
        reordered.entries.swap(1, 2);
        reordered.entries[1].rootUpdates = first_updates;
        reordered.entries[2].rootUpdates = second_updates;
        for entry in &mut reordered.entries[1..] {
            let update = &entry.rootUpdates[0];
            entry.rollingHash = EntryRollingHash::seed_for_l1(
                [(update.rollupId, update.currentRoot)],
                entry.proxyEntryHash,
            )
            .current();
        }
        let reordered_plan = effect_plan(&reordered, &settling);
        assert!(matches!(
            verify_inbound_effect_entries(&reordered_plan),
            Err(InboundEffectError::CallHashMismatch { entry_index: 1, .. })
        ));
    }

    let first = observed_inbound_candidate(0, U256::from(3), true);
    let second = observed_inbound_candidate(1, U256::from(3), true);
    *settling.inbound_candidates_mut_for_test() = vec![first, second];
    let duplicates = bindable_inbound_batch(&settling);
    let duplicate_plan = effect_plan(&duplicates, &settling);
    assert!(verify_inbound_effect_entries(&duplicate_plan).is_ok());
}

#[test]
fn inbound_effect_entries_handle_mixed_effect_positions_without_authorizing_outbound() {
    // A system/user pair ends at the user transaction (outbound), followed by
    // a terminal system transaction (inbound).
    let observation = observed_inbound_candidate(2, U256::from(4), true);
    let mut settling =
        SettlingBlockObservations::for_test(vec![true, false, true], vec![observation], Vec::new());
    let mut batch = effect_batch(
        &[B256::ZERO; 4],
        &[ClaimedEntryShape::Outbound, ClaimedEntryShape::Inbound],
    );
    let observation = settling.inbound_candidates()[0]
        .inspection
        .as_ref()
        .unwrap();
    let inbound = &mut batch.entries[2];
    inbound.proxyEntryHash = observation.recomputed_call_hash;
    inbound.returnData = observation.return_data.clone();
    inbound.rootUpdates[0].etherDelta = EtherDeltaSol::from(observation.value);
    inbound.rollingHash = eez_protocol::rolling_hash::EntryRollingHash::seed_for_l1(
        [(
            inbound.rootUpdates[0].rollupId,
            inbound.rootUpdates[0].currentRoot,
        )],
        inbound.proxyEntryHash,
    )
    .current();

    settling
        .outbound_load_candidates_mut_for_test()
        .push(OutboundLoadCandidate::decoded_for_test(
            0,
            flat_outbound_sidecar(&batch.entries[1]),
        ));
    let plan = effect_plan(&batch, &settling);
    assert!(verify_inbound_effect_entries(&plan).is_ok());
}

#[test]
fn inbound_effect_entries_reject_missing_extra_or_invalid_observations() {
    let missing = SettlingBlockObservations::for_test(vec![true], Vec::new(), Vec::new());
    let inbound_batch = effect_batch(
        &[B256::ZERO, B256::ZERO, B256::repeat_byte(1)],
        &[ClaimedEntryShape::Inbound],
    );
    assert_eq!(
        verify_effect_prefix(&inbound_batch, B256::ZERO, &[], &missing).err(),
        Some(EffectPrefixError::EffectCountMismatch {
            claimed: 1,
            observed: 0
        })
    );

    let hidden = SettlingBlockObservations::for_test(
        vec![true, false],
        vec![observed_inbound_candidate(0, U256::ZERO, true)],
        Vec::new(),
    );
    let outbound_batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    assert_eq!(
        verify_effect_prefix(&outbound_batch, B256::ZERO, &[], &hidden).err(),
        Some(EffectPrefixError::EffectCountMismatch {
            claimed: 0,
            observed: 1
        })
    );

    let conflicting = SettlingBlockObservations::for_test(
        vec![false],
        vec![observed_inbound_candidate(0, U256::ZERO, true)],
        Vec::new(),
    );
    assert_eq!(
        verify_effect_prefix(&outbound_batch, B256::ZERO, &[], &conflicting).err(),
        Some(EffectPrefixError::EffectCountMismatch {
            claimed: 0,
            observed: 1
        })
    );

    let invalid = SettlingBlockObservations::for_test(
        vec![true],
        vec![invalid_inbound_candidate(0)],
        Vec::new(),
    );
    let invalid_plan = effect_plan(&inbound_batch, &invalid);
    assert!(matches!(
        verify_inbound_effect_entries(&invalid_plan),
        Err(InboundEffectError::InvalidObservation {
            entry_index: 1,
            transaction_index: 0,
            ..
        })
    ));

    let failed = SettlingBlockObservations::for_test(
        vec![true],
        vec![observed_inbound_candidate(0, U256::ZERO, false)],
        Vec::new(),
    );
    let failed_plan = effect_plan(&inbound_batch, &failed);
    let failed_error = verify_inbound_effect_entries(&failed_plan)
        .err()
        .expect("an uncommitted destination failure must be rejected");
    assert!(
        matches!(
            failed_error,
            InboundEffectError::InvalidObservation {
                entry_index: 1,
                source: InboundObservationError::InvalidEntryShape {
                    field: "executionTable"
                },
                ..
            }
        ),
        "unexpected error: {failed_error:?}"
    );
}

#[test]
fn inbound_effect_entries_require_the_canonical_deferred_shape() {
    let settling = SettlingBlockObservations::for_test(
        vec![true],
        vec![observed_inbound_candidate(0, U256::from(5), true)],
        Vec::new(),
    );
    let valid = bindable_inbound_batch(&settling);

    let mut wrong_rolling_hash = valid.clone();
    wrong_rolling_hash.entries[1].rollingHash = B256::repeat_byte(0xaa);
    let plan = effect_plan(&wrong_rolling_hash, &settling);
    assert!(matches!(
        verify_inbound_effect_entries(&plan),
        Err(InboundEffectError::ExecutionTableMismatch { entry_index: 1, .. })
    ));

    let entry = &valid.entries[1];
    let update = &entry.rootUpdates[0];
    assert_eq!(
        entry.rollingHash,
        eez_protocol::rolling_hash::EntryRollingHash::seed_for_l1(
            [(update.rollupId, update.currentRoot)],
            entry.proxyEntryHash,
        )
        .current(),
        "deferred L1 entries use the exact root-update plus proxy seed",
    );

    let mut wrong_rollup = valid.clone();
    wrong_rollup.entries[1].destinationRollupId = 2;
    let plan = effect_plan(&wrong_rollup, &settling);
    assert_eq!(
        verify_inbound_effect_entries(&plan).err(),
        Some(InboundEffectError::DestinationRollupMismatch {
            entry_index: 1,
            expected: 1,
            actual: 2,
        })
    );

    let mut wrong_hash = valid.clone();
    wrong_hash.entries[1].proxyEntryHash = B256::repeat_byte(0xbb);
    let plan = effect_plan(&wrong_hash, &settling);
    assert!(matches!(
        verify_inbound_effect_entries(&plan),
        Err(InboundEffectError::CallHashMismatch { entry_index: 1, .. })
    ));

    let mut wrong_return = valid.clone();
    wrong_return.entries[1].returnData = Bytes::from_static(&[0xff]);
    let plan = effect_plan(&wrong_return, &settling);
    assert_eq!(
        verify_inbound_effect_entries(&plan).err(),
        Some(InboundEffectError::ReturnDataMismatch { entry_index: 1 })
    );

    let mut wrong_delta = valid;
    wrong_delta.entries[1].rootUpdates[0].etherDelta = EtherDeltaSol::ZERO;
    let plan = effect_plan(&wrong_delta, &settling);
    assert_eq!(
        verify_inbound_effect_entries(&plan).err(),
        Some(InboundEffectError::EtherDeltaMismatch {
            entry_index: 1,
            expected: I256::from_raw(U256::from(5)),
            actual: I256::ZERO,
        })
    );
}

#[test]
fn inbound_effect_entries_accept_the_int192_maximum_and_reject_the_next_value() {
    let max_value = (U256::from(1) << 191) - U256::from(1);
    let max_settling = SettlingBlockObservations::for_test(
        vec![true],
        vec![observed_inbound_candidate(0, max_value, true)],
        Vec::new(),
    );
    let max_batch = bindable_inbound_batch(&max_settling);
    let max_plan = effect_plan(&max_batch, &max_settling);
    assert!(verify_inbound_effect_entries(&max_plan).is_ok());
    assert_eq!(
        max_batch.entries[1].rootUpdates[0].etherDelta,
        EtherDeltaSol::MAX
    );

    let value = U256::from(1) << 191;
    let candidate = observed_inbound_candidate(0, value, true);
    let observation = candidate.inspection.as_ref().unwrap();
    let call_hash = observation.recomputed_call_hash;
    let return_data = observation.return_data.clone();
    let settling = SettlingBlockObservations::for_test(vec![true], vec![candidate], Vec::new());
    let mut batch = effect_batch(
        &[B256::ZERO, B256::ZERO, B256::repeat_byte(1)],
        &[ClaimedEntryShape::Inbound],
    );
    batch.entries[1].proxyEntryHash = call_hash;
    batch.entries[1].returnData = return_data;
    let update = &batch.entries[1].rootUpdates[0];
    let rolling_seed = (update.rollupId, update.currentRoot);
    let proxy_entry_hash = batch.entries[1].proxyEntryHash;
    batch.entries[1].rollingHash =
        eez_protocol::rolling_hash::EntryRollingHash::seed_for_l1([rolling_seed], proxy_entry_hash)
            .current();
    let plan = effect_plan(&batch, &settling);

    assert_eq!(
        verify_inbound_effect_entries(&plan).err(),
        Some(InboundEffectError::ValueOutOfRange {
            entry_index: 1,
            transaction_index: 0,
            value,
        })
    );
}
