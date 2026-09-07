use eez_protocol::blob_semantics::{
    SemanticTransaction, decode_semantic_transactions, encode_semantic_transaction,
};
use eez_protocol::{ExecutedAction, ExecutionOutcome};

use super::*;
use crate::settlement::{SemanticBindingError, verify_native_semantics};
use crate::validate::{ValidatedSettlingBlock, ValidatedWindow};

fn semantic_action(
    source_rollup_id: RollupId,
    target_rollup_id: RollupId,
    source_address: Address,
    target_address: Address,
    value: U256,
    data: Bytes,
    return_data: Bytes,
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
            success: true,
        },
        revert_span: None,
    }
}

fn validated_window(settling_rlp: Vec<u8>) -> ValidatedWindow {
    let settling = validated_block(41, settling_rlp, Vec::new());
    ValidatedWindow::for_test(
        B256::ZERO,
        B256::ZERO,
        B256::ZERO,
        Vec::new(),
        ValidatedSettlingBlock::for_test(settling, Vec::new(), Vec::new()),
    )
}

fn authorized_outbound_fixture() -> (
    SemanticTransaction,
    ValidatedWindow,
    AuthorizedOutboundEffects,
    CanonicalPostBatch,
) {
    let roots = [B256::ZERO; 3];
    let mut batch = effect_batch(&roots, &[ClaimedEntryShape::Outbound]);
    batch.immediateEntryCount = U256::from(batch.entries.len());
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
    let user = user_transaction(7);
    let (_, mut encoded_user) = block_and_payload_transactions(vec![user]);
    let raw_user = Bytes::from(encoded_user.pop().unwrap());
    let pairs = eez_protocol::system_tx::build_cross_chain_sync_pairs(
        &[(sidecar, raw_user.clone())],
        &[],
        &system_transaction_context(),
        11,
    )
    .unwrap();
    let transactions = eez_protocol::system_tx::interleave_sync_block_txs(&pairs)
        .into_iter()
        .map(|raw| alloy_rlp::decode_exact(raw.as_ref()).unwrap())
        .collect();
    let window = validated_window(block_rlp(transactions));
    let call = batch.entries[1].l2ToL1Calls[0].clone();
    let semantic = SemanticTransaction {
        origin_chain: RollupId(1),
        tx_data: raw_user,
        actions: vec![semantic_action(
            RollupId(1),
            RollupId::MAINNET,
            call.sourceAddress,
            call.targetAddress,
            call.value,
            call.data,
            batch.entries[1].returnData.clone(),
        )],
    };
    (semantic, window, outbound, batch)
}

#[test]
fn native_outbound_semantics_bind_exact_transaction_and_tables() {
    let (semantic, window, outbound, batch) = authorized_outbound_fixture();
    assert_eq!(
        verify_native_semantics(
            &[semantic],
            &window,
            &outbound,
            &AuthorizedInboundEffects::default(),
            &batch,
            expected_rollup_id(),
        ),
        Ok(())
    );
}

#[test]
fn native_semantics_reject_an_unproved_root_only_immediate_entry() {
    let (semantic, window, outbound, mut batch) = authorized_outbound_fixture();
    let live_root = batch.entries.last().unwrap().rootUpdates[0].newRoot;
    batch.entries.push(state_entry(1, live_root, live_root));
    batch.immediateEntryCount = U256::from(batch.entries.len());

    assert!(matches!(
        verify_native_semantics(
            &[semantic],
            &window,
            &outbound,
            &AuthorizedInboundEffects::default(),
            &batch,
            expected_rollup_id(),
        ),
        Err(SemanticBindingError::FifoRootSchedule { reason, .. })
            if reason.contains("expected one anchor plus 1 outbound settlement entries")
    ));
}

#[test]
fn native_outbound_semantics_reject_tx_data_and_call_mutations() {
    let (semantic, window, outbound, batch) = authorized_outbound_fixture();
    let verify = |semantic: SemanticTransaction| {
        verify_native_semantics(
            &[semantic],
            &window,
            &outbound,
            &AuthorizedInboundEffects::default(),
            &batch,
            expected_rollup_id(),
        )
    };

    let mut wrong_tx = semantic.clone();
    let mut mutated_tx_data = wrong_tx.tx_data.to_vec();
    mutated_tx_data[0] ^= 1;
    wrong_tx.tx_data = mutated_tx_data.into();
    assert!(matches!(
        verify(wrong_tx),
        Err(SemanticBindingError::OutboundTransactionData { .. })
    ));

    let mut wrong_call = semantic;
    let mut mutated_call_data = wrong_call.actions[0].data.to_vec();
    mutated_call_data[0] ^= 1;
    wrong_call.actions[0].data = mutated_call_data.into();
    assert!(matches!(
        verify(wrong_call),
        Err(SemanticBindingError::OutboundSidecars { .. }
            | SemanticBindingError::OutboundSettlement { .. })
    ));
}

#[test]
fn native_inbound_semantics_bind_both_sides_and_reject_result_mutation() {
    let value = U256::from(7);
    let settling = SettlingBlockObservations::for_test(
        vec![true],
        vec![observed_inbound_candidate(0, value, true)],
        Vec::new(),
    );
    let batch = bindable_inbound_batch(&settling);
    let plan = effect_plan(&batch, &settling);
    let inbound = verify_inbound_effect_entries(&plan).unwrap();
    let semantic = SemanticTransaction {
        origin_chain: RollupId::MAINNET,
        tx_data: Bytes::from_static(b"chain-defined L1 transaction"),
        actions: vec![semantic_action(
            RollupId::MAINNET,
            RollupId(1),
            address!("00000000000000000000000000000000000000bb"),
            address!("00000000000000000000000000000000000000aa"),
            value,
            Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            Bytes::from_static(&[0x01, 0x02]),
        )],
    };
    let window = validated_window(block_rlp(Vec::new()));
    assert_eq!(
        verify_native_semantics(
            std::slice::from_ref(&semantic),
            &window,
            &AuthorizedOutboundEffects::default(),
            &inbound,
            &batch,
            expected_rollup_id(),
        ),
        Ok(())
    );

    let mut mutated = semantic;
    mutated.actions[0].outcome = ExecutionOutcome::Resolved {
        return_data: vec![0xff],
        gas_used: 0,
        success: true,
    };
    assert!(matches!(
        verify_native_semantics(
            &[mutated],
            &window,
            &AuthorizedOutboundEffects::default(),
            &inbound,
            &batch,
            expected_rollup_id(),
        ),
        Err(SemanticBindingError::InboundSettlement | SemanticBindingError::InboundSidecars)
    ));
}

#[test]
fn native_lookup_semantics_bind_results_without_an_l2_effect() {
    let root = B256::ZERO;
    let lookup_cases = [
        (false, None, Bytes::from_static(b"destination revert")),
        (true, Some(1), Bytes::from_static(b"source-frame result")),
    ];

    for (success, revert_span, return_data) in lookup_cases {
        let mut action = semantic_action(
            RollupId::MAINNET,
            RollupId(1),
            Address::with_last_byte(0x31),
            Address::with_last_byte(0x41),
            U256::ZERO,
            Bytes::from_static(&[0xaa, 0xbb]),
            return_data.clone(),
        );
        action.outcome = ExecutionOutcome::Resolved {
            return_data: return_data.to_vec(),
            gas_used: 0,
            success,
        };
        action.revert_span = revert_span;
        let semantic = SemanticTransaction {
            origin_chain: RollupId::MAINNET,
            tx_data: Bytes::from_static(b"chain-defined lookup transaction"),
            actions: vec![action],
        };

        let mut materialized = semantic.materialize().expect("materialize lookup");
        assert!(
            materialized.targets.is_empty(),
            "lookup-only semantics must not materialize an L2 delivery"
        );
        materialized.source.batch.entries[0].rootUpdates = vec![RootUpdateSol {
            rollupId: 1,
            currentRoot: root,
            newRoot: root,
            etherDelta: EtherDeltaSol::ZERO,
        }];
        eez_protocol::entries::finalize_l1_entry_traces(
            &mut materialized.source.batch,
            &materialized.source.entry_traces,
        )
        .expect("finalize lookup");
        let lookup = materialized.source.batch.entries.remove(0);
        assert!(eez_protocol::settlement::is_lookup_only_inbound_entry(
            &lookup, 1
        ));

        let mut batch = state_chain(&[root, root]);
        batch.entries.push(lookup);
        batch.immediateEntryCount = U256::from(1);
        let settling = settling_with_system_flags(Vec::new());
        let bound = verify_effect_prefix(&batch, root, &[], &settling)
            .expect("lookup-only batch has no execution effect");
        assert_eq!((bound.inbound_count(), bound.outbound_count()), (0, 0));

        let window = validated_window(block_rlp(Vec::new()));
        assert_eq!(
            verify_native_semantics(
                std::slice::from_ref(&semantic),
                &window,
                &AuthorizedOutboundEffects::default(),
                &AuthorizedInboundEffects::default(),
                &batch,
                expected_rollup_id(),
            ),
            Ok(())
        );

        let mut mutated = semantic;
        mutated.actions[0].outcome = ExecutionOutcome::Resolved {
            return_data: b"different result".to_vec(),
            gas_used: 0,
            success,
        };
        assert_eq!(
            verify_native_semantics(
                &[mutated],
                &window,
                &AuthorizedOutboundEffects::default(),
                &AuthorizedInboundEffects::default(),
                &batch,
                expected_rollup_id(),
            ),
            Err(SemanticBindingError::InboundSettlement)
        );
    }
}

#[test]
fn source_reverted_lookup_with_cross_chain_descendants_is_rejected() {
    let root = B256::ZERO;
    let mut outer = semantic_action(
        RollupId::MAINNET,
        RollupId(1),
        Address::with_last_byte(0x31),
        Address::with_last_byte(0x41),
        U256::from(7),
        Bytes::from_static(b"outer call"),
        Bytes::from_static(b"outer result"),
    );
    outer.revert_span = Some(2);
    let mut nested = semantic_action(
        RollupId(1),
        RollupId::MAINNET,
        Address::with_last_byte(0x42),
        Address::with_last_byte(0x32),
        U256::from(3),
        Bytes::from_static(b"nested call"),
        Bytes::from_static(b"nested result"),
    );
    nested.parent_index = Some(0);
    let semantic = SemanticTransaction {
        origin_chain: RollupId::MAINNET,
        tx_data: Bytes::from_static(b"source-reverted L1 transaction"),
        actions: vec![outer, nested],
    };

    let messages = encode_semantic_transaction(&semantic).expect("encode nested lookup semantics");
    let decoded = decode_semantic_transactions(&messages).expect("decode nested lookup semantics");
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].origin_chain, semantic.origin_chain);
    assert_eq!(decoded[0].tx_data, semantic.tx_data);
    assert_eq!(decoded[0].actions.len(), 2);
    assert_eq!(decoded[0].actions[0].revert_span, Some(2));
    assert_eq!(
        decoded[0].actions[0].data,
        Bytes::from_static(b"outer call")
    );
    assert_eq!(decoded[0].actions[1].parent_index, Some(0));
    assert_eq!(
        decoded[0].actions[1].data,
        Bytes::from_static(b"nested call")
    );

    let mut materialized = semantic.materialize().expect("materialize nested lookup");
    assert!(
        materialized.targets.is_empty(),
        "source-reverted nested work must not materialize an L2 delivery"
    );
    materialized.source.batch.entries[0].rootUpdates = vec![RootUpdateSol {
        rollupId: 1,
        currentRoot: root,
        newRoot: root,
        etherDelta: EtherDeltaSol::ZERO,
    }];
    eez_protocol::entries::finalize_l1_entry_traces(
        &mut materialized.source.batch,
        &materialized.source.entry_traces,
    )
    .expect("finalize nested lookup");
    let lookup = materialized.source.batch.entries.remove(0);
    assert!(eez_protocol::settlement::is_lookup_only_inbound_entry(
        &lookup, 1
    ));

    let mut batch = state_chain(&[root, root]);
    batch.entries.push(lookup);
    batch.immediateEntryCount = U256::from(1);
    let error = verify_native_semantics(
        &decoded,
        &validated_window(block_rlp(Vec::new())),
        &AuthorizedOutboundEffects::default(),
        &AuthorizedInboundEffects::default(),
        &batch,
        expected_rollup_id(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        SemanticBindingError::FifoRootSchedule { reason, .. }
            if reason.contains("multi-chain observation replay is unsupported")
    ));
}

#[test]
fn native_inbound_semantics_finalize_root_dependent_nested_fields() {
    let (mut batch, calldata) = super::inbound::nested_inbound_fixture();
    batch.immediateEntryCount = U256::from(1);
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
    let inbound = verify_inbound_effect_entries(&effect_plan(&batch, &settling)).unwrap();

    let mut actions = (0u8..4)
        .map(|index| {
            let source_rollup_id = if index % 2 == 0 {
                RollupId::MAINNET
            } else {
                RollupId(1)
            };
            let target_rollup_id = if source_rollup_id.is_mainnet() {
                RollupId(1)
            } else {
                RollupId::MAINNET
            };
            semantic_action(
                source_rollup_id,
                target_rollup_id,
                Address::with_last_byte(0x10 + index),
                Address::with_last_byte(0x20 + index),
                U256::ZERO,
                Bytes::from(vec![0x30 + index]),
                Bytes::from(vec![0x40 + index]),
            )
        })
        .collect::<Vec<_>>();
    for (index, action) in actions.iter_mut().enumerate().skip(1) {
        action.parent_index = Some(index - 1);
    }
    let semantic = SemanticTransaction {
        origin_chain: RollupId::MAINNET,
        tx_data: Bytes::from_static(b"chain-defined nested L1 transaction"),
        actions,
    };

    assert_eq!(
        verify_native_semantics(
            &[semantic],
            &validated_window(block_rlp(Vec::new())),
            &AuthorizedOutboundEffects::default(),
            &inbound,
            &batch,
            expected_rollup_id(),
        ),
        Ok(())
    );
}

#[test]
fn native_l1_static_semantics_bind_exact_pinned_static_entry() {
    let mut action = semantic_action(
        RollupId::MAINNET,
        RollupId(1),
        Address::with_last_byte(0x31),
        Address::with_last_byte(0x41),
        U256::ZERO,
        Bytes::from_static(&[0xaa, 0xbb]),
        Bytes::from_static(&[0x01, 0x02]),
    );
    action.call_mode = CallMode::Static;
    let semantic = SemanticTransaction {
        origin_chain: RollupId::MAINNET,
        tx_data: Bytes::from_static(b"chain-defined L1 static transaction"),
        actions: vec![action],
    };
    let mut expected = semantic
        .materialize()
        .expect("materialize static semantic")
        .source
        .batch
        .staticEntries
        .pop()
        .expect("one static source entry");
    expected.expectedRoots = vec![ExpectedRootPerRollupSol {
        rollupId: 1,
        root: B256::with_last_byte(0x99),
    }];
    let window = validated_window(block_rlp(Vec::new()));
    let root = expected.expectedRoots[0].root;
    let mut batch = state_chain(&[root, root]);
    batch.immediateEntryCount = U256::from(1);
    batch.staticEntries.push(expected.clone());

    assert_eq!(
        verify_native_semantics(
            std::slice::from_ref(&semantic),
            &window,
            &AuthorizedOutboundEffects::default(),
            &AuthorizedInboundEffects::default(),
            &batch,
            expected_rollup_id(),
        ),
        Ok(())
    );

    expected.returnData = Bytes::from_static(&[0xff]);
    batch.staticEntries = vec![expected];
    assert_eq!(
        verify_native_semantics(
            &[semantic],
            &window,
            &AuthorizedOutboundEffects::default(),
            &AuthorizedInboundEffects::default(),
            &batch,
            expected_rollup_id(),
        ),
        Err(SemanticBindingError::InboundStaticSettlement)
    );
}

#[test]
fn native_semantics_reject_missing_or_wrong_direction_transactions() {
    let (mut semantic, window, outbound, batch) = authorized_outbound_fixture();
    assert!(matches!(
        verify_native_semantics(
            &[],
            &window,
            &outbound,
            &AuthorizedInboundEffects::default(),
            &batch,
            expected_rollup_id(),
        ),
        Err(SemanticBindingError::TransactionCount {
            minimum: 1,
            actual: 0
        })
    ));
    semantic.origin_chain = RollupId::MAINNET;
    assert!(matches!(
        verify_native_semantics(
            &[semantic],
            &window,
            &outbound,
            &AuthorizedInboundEffects::default(),
            &batch,
            expected_rollup_id(),
        ),
        Err(SemanticBindingError::Origin {
            expected: 1,
            actual: 0,
            ..
        })
    ));
}
