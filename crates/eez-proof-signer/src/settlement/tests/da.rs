use super::*;

/// Refresh the exact L1 commitment after a test mutates a claimed entry.
fn refresh_l1_rolling_hash(entry: &mut ExecutionEntrySol) {
    let mut rolling_hash = EntryRollingHash::seed_for_l1(
        entry
            .rootUpdates
            .iter()
            .map(|update| (update.rollupId, update.currentRoot)),
        entry.proxyEntryHash,
    );
    for call in &entry.l2ToL1Calls {
        let mode = if call.isStatic {
            CallMode::Static
        } else {
            CallMode::Mutable
        };
        let call_hash = common_cross_chain_call_hash(CallHashInput {
            call_mode: mode,
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

// Sidecar-array encoding is obsolete: V0 publishes ordinary prefixes only.
// Typed sidecar/settlement substitution, source/destination/hash, semantic user
// bytes and group partition checks live in inbound, outbound and captured service
// tests. These tests exercise the actual V0 DA gate and its exact native suffix.

#[test]
fn da_payload_binds_inbound_sidecars_and_complete_reconstructed_transactions() {
    let settling = SettlingBlockObservations::for_test(
        vec![true, true],
        vec![
            observed_inbound_candidate(0, U256::from(7), true),
            observed_inbound_candidate(1, U256::from(9), true),
        ],
        Vec::new(),
    );
    let batch = bindable_inbound_batch(&settling);
    let plan = effect_plan(&batch, &settling);
    let inbound = verify_inbound_effect_entries(&plan).unwrap();
    let sidecars = settling
        .inbound_candidates()
        .iter()
        .map(|candidate| {
            candidate
                .inspection
                .as_ref()
                .unwrap()
                .derived_da_entry
                .encoded()
        })
        .collect::<Vec<_>>();
    let entries = sidecars
        .iter()
        .map(|encoded| ExecutionEntrySol::abi_decode(encoded).unwrap())
        .collect::<Vec<_>>();
    let transactions = build_inbound_transactions(&entries, &system_transaction_context(), 11);
    let (intermediate_rlp, intermediate_transactions) =
        block_and_payload_transactions(vec![transaction(CREATE_TX)]);
    let (settling_rlp, _) = block_and_payload_transactions(transactions.clone());
    let verifier = system_transactions();
    let outbound = AuthorizedOutboundEffects::default();
    let verify = |payload: &[u8], blocks: [(u64, &[u8]); 2]| {
        let (settling, intermediates) = blocks.split_last().unwrap();
        verify_da_payload(
            payload,
            intermediates.iter().copied(),
            *settling,
            &outbound,
            &inbound,
            &verifier,
        )
    };
    let blocks = [
        (40, intermediate_rlp.as_slice()),
        (41, settling_rlp.as_slice()),
    ];

    // Ordinary transactions remain in DA for intermediate blocks. Only the
    // two bound inbound transactions are omitted from the terminal block.
    let payload = encode_da_payload(&[intermediate_transactions, Vec::new()]);
    assert_eq!(verify(&payload, blocks), Ok(()));

    let mut noncanonical_context = system_transaction_context();
    noncanonical_context.l2_chain_id += 1;
    let noncanonical_transactions = build_inbound_transactions(&entries, &noncanonical_context, 11);
    let mut mutated_second_transaction = transactions;
    mutated_second_transaction[1] = noncanonical_transactions[1].clone();
    let wrong_block = block_rlp(mutated_second_transaction);
    assert_eq!(
        verify(
            &payload,
            [
                (40, intermediate_rlp.as_slice()),
                (41, wrong_block.as_slice()),
            ],
        ),
        Err(DaPayloadError::SyncBlockTransactionMismatch {
            transaction_index: 1,
        })
    );
}

#[test]
fn da_payload_binds_outbound_sidecars_users_and_system_loads() {
    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    let value = U256::from(7);
    batch.entries[1].l2ToL1Calls[0].value = value;
    batch.entries[1].rootUpdates[0].etherDelta = -EtherDeltaSol::from(value);
    refresh_l1_rolling_hash(&mut batch.entries[1]);
    let mut settling = settling_with_outbound_pairs(&batch);
    settling
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(
            1,
            0,
            &batch.entries[1].l2ToL1Calls[0],
        ));
    let plan = effect_plan(&batch, &settling);
    let outbound = authorize_outbound_effects(&plan).unwrap();
    let sidecar = outbound.iter().next().unwrap().derived_da_entry().clone();

    let (_, mut user_payload) = block_and_payload_transactions(vec![user_transaction(7)]);
    let user = user_payload.pop().unwrap();
    let pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &[(sidecar.clone(), Bytes::from(user.clone()))],
        &[],
        &system_transaction_context(),
        11,
    )
    .unwrap();
    let raw_transactions = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);
    let transactions = raw_transactions
        .iter()
        .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
        .collect::<Vec<_>>();
    let settling_rlp = block_rlp(transactions);
    let verifier = system_transactions();
    let payload = encode_da_payload(&[Vec::new()]);
    let inbound = AuthorizedInboundEffects::default();
    let verify = |payload: &[u8], blocks: [(u64, &[u8]); 1]| {
        let (settling, intermediates) = blocks.split_last().unwrap();
        verify_da_payload(
            payload,
            intermediates.iter().copied(),
            *settling,
            &outbound,
            &inbound,
            &verifier,
        )
    };

    assert_eq!(verify(&payload, [(41, settling_rlp.as_slice())]), Ok(()));

    let (_, mut extra_payload) = block_and_payload_transactions(vec![transaction(CREATE_TX)]);
    let extra = extra_payload.pop().unwrap();
    let mut extra_transactions = raw_transactions;
    extra_transactions.push(Bytes::from(extra.clone()));
    let extra_block = block_rlp(
        extra_transactions
            .into_iter()
            .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
            .collect(),
    );
    let extra_transaction = encode_da_payload(&[Vec::new()]);
    assert_eq!(
        verify(&extra_transaction, [(41, extra_block.as_slice())]),
        Err(DaPayloadError::SyncBlockTransactionCount {
            expected: 2,
            actual: 3,
        })
    );

    let mut noncanonical_context = system_transaction_context();
    noncanonical_context.l2_chain_id += 1;
    let wrong_pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &[(sidecar.clone(), Bytes::from(user))],
        &[],
        &noncanonical_context,
        11,
    )
    .unwrap();
    let wrong_transactions = eez_protocol::system_tx::interleave_sync_block_txs(&wrong_pairs)
        .into_iter()
        .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
        .collect::<Vec<_>>();
    let wrong_block = block_rlp(wrong_transactions);
    assert_eq!(
        verify(&payload, [(41, wrong_block.as_slice())]),
        Err(DaPayloadError::SyncBlockTransactionMismatch {
            transaction_index: 0,
        })
    );
}

#[test]
fn da_payload_binds_multiple_outbound_pairs_and_system_nonce_progression() {
    let mut batch = effect_batch(
        &[B256::ZERO; 4],
        &[ClaimedEntryShape::Outbound, ClaimedEntryShape::Outbound],
    );
    batch.entries[2].l2ToL1Calls[0].data = Bytes::from_static(&[0x02]);
    refresh_l1_rolling_hash(&mut batch.entries[2]);
    assert_eq!(
        batch.entries[2].rollingHash,
        b256!("408ea7ecec3dcd31191a40d774c3eaa13b49ae9ca8d82f4862151519fdfa37eb")
    );
    let mut settling = settling_with_outbound_pairs(&batch);
    *settling.outbound_event_candidates_mut_for_test() = vec![
        observed_outbound_call(1, 0, &batch.entries[1].l2ToL1Calls[0]),
        observed_outbound_call(3, 0, &batch.entries[2].l2ToL1Calls[0]),
    ];
    let plan = effect_plan(&batch, &settling);
    let outbound = authorize_outbound_effects(&plan).unwrap();
    let sidecars = outbound
        .iter()
        .map(|binding| binding.derived_da_entry().clone())
        .collect::<Vec<_>>();
    let (_, users) = block_and_payload_transactions(vec![user_transaction(7), user_transaction(8)]);
    let outbound_inputs = sidecars
        .iter()
        .cloned()
        .zip(users.iter().cloned().map(Bytes::from))
        .collect::<Vec<_>>();
    let pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &outbound_inputs,
        &[],
        &system_transaction_context(),
        11,
    )
    .unwrap();
    let raw_transactions = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);
    let first_load: TransactionSigned =
        TransactionSigned::decode_2718_exact(raw_transactions[0].as_ref()).unwrap();
    let second_load: TransactionSigned =
        TransactionSigned::decode_2718_exact(raw_transactions[2].as_ref()).unwrap();
    assert_eq!((first_load.nonce(), second_load.nonce()), (11, 12));
    let settling_rlp = block_rlp(
        raw_transactions
            .iter()
            .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
            .collect(),
    );
    let payload = encode_da_payload(&[Vec::new()]);
    let verifier = system_transactions();
    let inbound = AuthorizedInboundEffects::default();
    let verify = |payload: &[u8], blocks: [(u64, &[u8]); 1]| {
        let (settling, intermediates) = blocks.split_last().unwrap();
        verify_da_payload(
            payload,
            intermediates.iter().copied(),
            *settling,
            &outbound,
            &inbound,
            &verifier,
        )
    };

    assert_eq!(verify(&payload, [(41, settling_rlp.as_slice())]), Ok(()));

    let mut noncanonical_context = system_transaction_context();
    noncanonical_context.l2_chain_id += 1;
    let wrong_pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &outbound_inputs,
        &[],
        &noncanonical_context,
        11,
    )
    .unwrap();
    let wrong_transactions = eez_protocol::system_tx::interleave_sync_block_txs(&wrong_pairs);
    let mut mutated_second_load = raw_transactions;
    mutated_second_load[2] = wrong_transactions[2].clone();
    let wrong_block = block_rlp(
        mutated_second_load
            .into_iter()
            .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
            .collect(),
    );
    assert_eq!(
        verify(&payload, [(41, wrong_block.as_slice())]),
        Err(DaPayloadError::SyncBlockTransactionMismatch {
            transaction_index: 2,
        })
    );
}

#[test]
fn da_payload_reconstructs_whole_grouped_roots() {
    let mut batch = effect_batch(&[B256::ZERO; 3], &[ClaimedEntryShape::Outbound]);
    batch.entries[1].l2ToL1Calls[0].data = Bytes::from_static(&[0x11]);
    let mut second = batch.entries[1].l2ToL1Calls[0].clone();
    second.data = Bytes::from_static(&[0x22]);
    batch.entries[1].l2ToL1Calls.push(second);
    refresh_l1_rolling_hash(&mut batch.entries[1]);

    let source_entries = batch.entries[1]
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
    settling.outbound_load_candidates_mut_for_test().push(
        OutboundLoadCandidate::decoded_many_for_test(0, source_entries.clone()),
    );
    *settling.outbound_event_candidates_mut_for_test() = batch.entries[1]
        .l2ToL1Calls
        .iter()
        .enumerate()
        .map(|(receipt_log_index, call)| observed_outbound_call(1, receipt_log_index, call))
        .collect();

    let plan = effect_plan(&batch, &settling);
    let outbound = authorize_outbound_effects(&plan).unwrap();
    assert_eq!(outbound.groups().map(<[_]>::len).collect::<Vec<_>>(), [2]);

    let (_, mut user_payload) = block_and_payload_transactions(vec![user_transaction(7)]);
    let user = user_payload.pop().unwrap();
    let group = eez_protocol::system_tx::OutboundSyncGroup {
        effects: source_entries
            .iter()
            .cloned()
            .map(|source_entry| eez_protocol::system_tx::OutboundSyncEffect {
                source_entry: eez_protocol::system_tx::OutboundSourceEntry::Mutable(source_entry),
                callback_entries: Vec::new(),
            })
            .collect(),
        user_tx: Bytes::from(user.clone()),
    };
    let pairs = eez_protocol::system_tx::build_cross_chain_sync_groups(
        &[group],
        &[],
        &system_transaction_context(),
        11,
    )
    .unwrap();
    let raw_transactions = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);
    let settling_rlp = block_rlp(
        raw_transactions
            .iter()
            .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
            .collect(),
    );
    let verify = |payload: &[u8]| {
        verify_da_payload(
            payload,
            std::iter::empty(),
            (41, settling_rlp.as_slice()),
            &outbound,
            &AuthorizedInboundEffects::default(),
            &system_transactions(),
        )
    };

    let canonical = encode_da_payload(&[Vec::new()]);
    assert_eq!(verify(&canonical), Ok(()));
    // The whole group is semantic input, never split into ordinary DA rows.
    let smuggled = encode_da_payload(&[vec![user]]);
    assert!(matches!(
        verify(&smuggled),
        Err(DaPayloadError::ProjectedTransactionCount { .. })
    ));
}

#[test]
fn da_payload_binds_the_complete_mixed_sync_sequence_and_action_order() {
    let mut settling = SettlingBlockObservations::for_test(
        vec![true, false, true],
        vec![observed_inbound_candidate(2, U256::from(9), true)],
        Vec::new(),
    );
    let mut batch = effect_batch(
        &[B256::ZERO; 4],
        &[ClaimedEntryShape::Outbound, ClaimedEntryShape::Inbound],
    );
    let inbound_observation = settling.inbound_candidates()[0]
        .inspection
        .as_ref()
        .unwrap();
    batch.entries[2].proxyEntryHash = inbound_observation.recomputed_call_hash;
    batch.entries[2].returnData = inbound_observation.return_data.clone();
    batch.entries[2].rootUpdates[0].etherDelta = EtherDeltaSol::from(inbound_observation.value);
    refresh_l1_rolling_hash(&mut batch.entries[2]);
    settling
        .outbound_event_candidates_mut_for_test()
        .push(observed_outbound_call(
            1,
            0,
            &batch.entries[1].l2ToL1Calls[0],
        ));
    settling
        .outbound_load_candidates_mut_for_test()
        .push(OutboundLoadCandidate::decoded_for_test(
            0,
            flat_outbound_sidecar(&batch.entries[1]),
        ));

    let plan = effect_plan(&batch, &settling);
    let outbound = authorize_outbound_effects(&plan).unwrap();
    let inbound = verify_inbound_effect_entries(&plan).unwrap();
    let outbound_sidecar = outbound.iter().next().unwrap().derived_da_entry().clone();
    let inbound_sidecar = settling.inbound_candidates()[0]
        .inspection
        .as_ref()
        .unwrap()
        .derived_da_entry
        .as_entry()
        .clone();
    let (_, mut user_payload) = block_and_payload_transactions(vec![user_transaction(7)]);
    let user = user_payload.pop().unwrap();
    let pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &[(outbound_sidecar.clone(), Bytes::from(user.clone()))],
        &[eez_protocol::entries::InboundSidecar::try_from(&inbound_sidecar).unwrap()],
        &system_transaction_context(),
        11,
    )
    .unwrap();
    let raw_transactions = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);
    let transactions = raw_transactions
        .iter()
        .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
        .collect::<Vec<_>>();
    let settling_rlp = block_rlp(transactions);
    let payload = encode_da_payload(&[Vec::new()]);
    let verifier = system_transactions();
    let verify = |payload: &[u8], blocks: [(u64, &[u8]); 1]| {
        let (settling, intermediates) = blocks.split_last().unwrap();
        verify_da_payload(
            payload,
            intermediates.iter().copied(),
            *settling,
            &outbound,
            &inbound,
            &verifier,
        )
    };

    assert_eq!(verify(&payload, [(41, settling_rlp.as_slice())]), Ok(()));

    let mut reordered = raw_transactions;
    reordered.swap(0, 2);
    let reordered_block = block_rlp(
        reordered
            .into_iter()
            .map(|raw| TransactionSigned::decode_2718_exact(raw.as_ref()).unwrap())
            .collect(),
    );
    assert_eq!(
        verify(&payload, [(41, reordered_block.as_slice())]),
        Err(DaPayloadError::SyncBlockTransactionMismatch {
            transaction_index: 0,
        })
    );
}

#[test]
fn da_payload_matches_exact_transactions_in_every_validated_block() {
    let (first_rlp, first_transactions) =
        block_and_payload_transactions(vec![transaction(CREATE_TX)]);
    let (second_rlp, second_transactions) =
        block_and_payload_transactions(vec![transaction(EIP1559_SYSTEM_TX)]);
    let payload = encode_da_payload(&[first_transactions, second_transactions]);

    assert_eq!(
        verify_anchor_only_da_payload(
            &payload,
            [(41, first_rlp.as_slice()), (42, second_rlp.as_slice())],
        ),
        Ok(())
    );
}

#[test]
fn da_payload_rejects_transactions_assigned_to_the_wrong_block() {
    let (first_rlp, first_transactions) =
        block_and_payload_transactions(vec![transaction(CREATE_TX)]);
    let (second_rlp, _) = block_and_payload_transactions(Vec::new());
    let payload = encode_da_payload(&[Vec::new(), first_transactions]);

    assert_eq!(
        verify_anchor_only_da_payload(
            &payload,
            [(41, first_rlp.as_slice()), (42, second_rlp.as_slice())],
        ),
        Err(DaPayloadError::ProjectedTransactionCount {
            block_number: 41,
            submitted: 0,
            expected: 1,
        })
    );
}

#[test]
fn da_payload_rejects_transactions_reordered_within_one_block() {
    let (block_rlp, _) = block_and_payload_transactions(vec![
        transaction(CREATE_TX),
        transaction(EIP1559_SYSTEM_TX),
    ]);
    let (_, reordered) = block_and_payload_transactions(vec![
        transaction(EIP1559_SYSTEM_TX),
        transaction(CREATE_TX),
    ]);
    let payload = encode_da_payload(&[reordered]);

    assert_eq!(
        verify_anchor_only_da_payload(&payload, [(41, block_rlp.as_slice())]),
        Err(DaPayloadError::TransactionMismatch {
            block_number: 41,
            transaction_index: 0,
        })
    );
}

#[test]
fn da_payload_distinguishes_malformed_payloads_from_invalid_validated_blocks() {
    let valid_block = block_rlp(Vec::new());
    let malformed = verify_anchor_only_da_payload(&[], [(41, valid_block.as_slice())]);
    assert!(matches!(malformed, Err(DaPayloadError::Decode { .. })));

    let payload = encode_da_payload(&[Vec::new()]);
    assert!(matches!(
        verify_anchor_only_da_payload(&payload, [(41, [0xff].as_slice())]),
        Err(DaPayloadError::InvalidBlockRlp {
            block_number: 41,
            ..
        })
    ));
}

#[test]
fn v0_da_rejects_noncanonical_bytes_and_wrong_block_count() {
    let rlp = block_rlp(Vec::new());
    let canonical = encode_da_payload(&[Vec::new()]);
    assert_eq!(canonical[0], 0);
    let mut trailing = canonical.clone();
    trailing.push(0);
    let mut version = canonical.clone();
    version[0] = 0x80;
    let mut noncanonical_count = canonical.clone();
    noncanonical_count.splice(1..2, [0x81, 0]);
    for malformed in [
        trailing,
        version,
        noncanonical_count,
        canonical[..2].to_vec(),
    ] {
        assert!(matches!(
            verify_anchor_only_da_payload(&malformed, [(41, rlp.as_slice())]),
            Err(DaPayloadError::Decode { .. })
        ));
    }
    let two = encode_da_payload(&[Vec::new(), Vec::new()]);
    assert_eq!(
        verify_anchor_only_da_payload(&two, [(41, rlp.as_slice())]),
        Err(DaPayloadError::BlockCount {
            expected: 1,
            actual: 2
        })
    );
    assert_eq!(
        verify_anchor_only_da_payload(&canonical, [(41, rlp.as_slice()), (42, rlp.as_slice())]),
        Err(DaPayloadError::BlockCount {
            expected: 2,
            actual: 1
        })
    );
}

#[test]
fn v0_da_binds_transaction_bytes_and_published_metadata() {
    use alloy_eips::Encodable2718 as _;
    let block = block_rlp(vec![user_transaction(7)]);
    let different = encode_da_payload(&[vec![user_transaction(8).encoded_2718()]]);
    assert_eq!(
        verify_anchor_only_da_payload(&different, [(41, block.as_slice())]),
        Err(DaPayloadError::TransactionMismatch {
            block_number: 41,
            transaction_index: 0
        })
    );
    let transactions = [Bytes::from(user_transaction(7).encoded_2718())];
    for (beneficiary, extra_data) in [
        (Address::repeat_byte(1), &[][..]),
        (Address::ZERO, &[1][..]),
    ] {
        let payload = eez_payload_codec::v0::encode(&[eez_payload_codec::v0::BlockRun {
            blocks: 1,
            beneficiary,
            extra_data,
            transactions: &transactions,
        }])
        .unwrap();
        assert_eq!(
            verify_anchor_only_da_payload(&payload, [(41, block.as_slice())]),
            Err(DaPayloadError::EnvironmentMismatch { block_number: 41 })
        );
    }
}
