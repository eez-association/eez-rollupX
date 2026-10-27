use alloy_consensus::{Header, SignableTransaction as _, TxLegacy};
use alloy_primitives::{Log, Signature, b256};
use eez_primitives::{EezTxEnvelope as TransactionSigned, Receipt as EthereumReceipt};
use eez_protocol::CallMode;
use reth_primitives_traits::SignerRecoverable as _;

use super::*;
use crate::testkit::{LEGACY_USER_TX, TEST_SYSTEM_ADDRESS};
use crate::validate::Validator;

fn history_fixture() -> (Genesis, AdmittedBlock) {
    use eez_control_rpc::v2::client_frame;
    let name = "native-offline-bidirectional";
    let index = crate::testkit::native_frames(name)
        .into_iter()
        .filter_map(|frame| match frame.kind {
            Some(client_frame::Kind::Block(block)) => block.data,
            _ => None,
        })
        .position(|block| {
            let decoded: Block = alloy_rlp::decode_exact(&block.rlp).unwrap();
            matches!(decoded.body.transactions.as_slice(), [tx] if tx.is_history())
        })
        .expect("capture must contain a nonempty scheduled history-only block");
    crate::testkit::native_block(name, Some(index))
}

fn fixture_genesis() -> Genesis {
    history_fixture().0
}

fn fixture_input() -> AdmittedBlock {
    // Select the recorded state transition by its shape: eligible timestamps
    // may omit updates when new evidence was not ready during capture.
    history_fixture().1
}

#[test]
fn reconstruction_fact_is_built_only_from_exact_replay_outputs() {
    let input = fixture_input();
    let backend = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS);
    let mut backend = backend;
    backend.evm_config = backend
        .evm_config
        .clone()
        .with_l1_history(Arc::new(eez_protocol::l1_history::WitnessHistory(
            crate::testkit::checked_history(std::slice::from_ref(&input)).unwrap(),
        )))
        .unwrap();
    let recovered = decode_match_and_recover_signers(&input, &backend.chain_spec).unwrap();
    let block = recovered.clone().into_sealed_block().into_block();
    let mut output = stateless_validation_recovered(
        recovered,
        input.witness.clone(),
        backend.chain_spec.clone(),
        backend.evm_config.clone(),
    )
    .unwrap();
    let parent = input.witness.headers.last().map(AsRef::as_ref);
    let fact =
        reconstruction::reconstruct(&backend.evm_config, &block, &input.rlp, parent, &output)
            .unwrap();
    assert_eq!(fact.block_hash(), input.claimed_hash);
    let valid_post = output.post_state_root;
    output.post_state_root = B256::repeat_byte(0xa1);
    assert!(
        reconstruction::reconstruct(&backend.evm_config, &block, &input.rlp, parent, &output)
            .is_err()
    );
    output.post_state_root = valid_post;
    output.execution_output.result.gas_used += 1;
    assert!(
        reconstruction::reconstruct(&backend.evm_config, &block, &input.rlp, parent, &output)
            .is_err()
    );
    output.execution_output.result.gas_used -= 1;
    output
        .execution_output
        .result
        .requests
        .push_request_with_type(0, [1u8]);
    assert!(
        reconstruction::reconstruct(&backend.evm_config, &block, &input.rlp, parent, &output)
            .is_err()
    );
}

#[test]
fn reconstruction_retains_the_complete_existing_execution_evidence() {
    let (checkpoint_input, checkpoint_config, _) = checkpoint_fixture();
    let (observation_input, observation_config) = observation_fixture();
    for (input, config) in [
        (fixture_input(), fixture_genesis()),
        (checkpoint_input, checkpoint_config),
        (observation_input, observation_config),
    ] {
        let mut backend = Backend::new(config, TEST_SYSTEM_ADDRESS);
        backend.derivation_enabled = false;
        let ordinary = backend.validate(std::slice::from_ref(&input)).unwrap();
        assert!(ordinary.blocks[0].reconstruction.is_none());
        backend.derivation_enabled = true;
        let mut reconstructed = backend.validate(&[input]).unwrap();
        assert!(reconstructed.blocks[0].reconstruction.take().is_some());
        assert_eq!(ordinary, reconstructed);
    }
}

fn checkpoint_fixture() -> (AdmittedBlock, Genesis, Vec<usize>) {
    let (genesis, input, _) = crate::testkit::native_capture("native-osaka-changing-static");
    let block: Block = alloy_rlp::decode_exact(&input.rlp).unwrap();
    assert_eq!(block.body.transactions.len(), 2);
    assert!(matches!(
        block.body.transactions[0],
        TransactionSigned::System(_)
    ));
    (input, genesis, vec![1])
}

#[test]
fn nonempty_ordinary_terminal_skips_checkpoints_without_changing_execution() {
    use eez_control_rpc::v2::client_frame;
    let name = "native-offline-bidirectional";
    let index = crate::testkit::native_frames(name)
        .into_iter()
        .filter_map(|frame| match frame.kind {
            Some(client_frame::Kind::Block(block)) => block.data,
            _ => None,
        })
        .position(|block| {
            let decoded: Block = alloy_rlp::decode_exact(&block.rlp).unwrap();
            !decoded.body.transactions.is_empty()
                && decoded
                    .body
                    .transactions
                    .iter()
                    .all(|tx| !matches!(tx, TransactionSigned::System(_)))
        })
        .expect("fixture must contain an ordinary deployment block");
    let (genesis, input) = crate::testkit::native_block(name, Some(index));
    let mut backend = Backend::new(genesis, TEST_SYSTEM_ADDRESS);
    backend.evm_config = backend
        .evm_config
        .clone()
        .with_l1_history(Arc::new(eez_protocol::l1_history::WitnessHistory(
            crate::testkit::checked_history(std::slice::from_ref(&input)).unwrap(),
        )))
        .unwrap();
    let decoded = decode_match_and_recover_signers(&input, &backend.chain_spec).unwrap();
    assert!(
        !decoded.body().transactions.is_empty(),
        "empty blocks would not expose the regression"
    );
    let independent = stateless_validation_recovered(
        decoded,
        input.witness.clone(),
        Arc::clone(&backend.chain_spec),
        backend.evm_config.clone(),
    )
    .unwrap();
    let output = backend.validate(&[input]).unwrap();
    assert_eq!(
        backend
            .checkpoint_replays
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "ordinary execution must not invoke the expensive checkpoint backend"
    );
    assert_eq!(output.blocks.len(), 1);
    let block = &output.blocks[0];
    assert!(block.transaction_state_checkpoints.is_empty());
    assert!(
        block
            .settlement_evidence
            .system_sender_flags
            .iter()
            .all(|&system| !system)
    );
    assert_eq!(block.post_state_root, independent.post_state_root);
    assert_eq!(block.computed_hash, independent.block_hash);
    assert_eq!(
        block.settlement_evidence.ordinary_prefix_hash,
        independent.block_hash
    );
}

fn observation_fixture() -> (AdmittedBlock, Genesis) {
    let (genesis, input, _) = crate::testkit::native_capture("native-osaka-lookup");
    (input, genesis)
}

fn observation_claim(
    pinned_block_hash: B256,
    call_mode: CallMode,
    data: Bytes,
) -> FifoObservationClaim {
    let (_, _, transactions) = crate::testkit::native_capture("native-osaka-lookup");
    assert_eq!(transactions.len(), 1);
    let action = &transactions[0].actions[0];
    FifoObservationClaim {
        pinned_block_hash,
        call_mode,
        target_address: action.target_address,
        source_address: action.source_address,
        source_rollup_id: action.source_rollup_id,
        data,
        value: U256::ZERO,
        expected_success: false,
        expected_output: Bytes::new(),
        callbacks: Vec::new(),
    }
}

fn validated_observation_fixture() -> (
    Validator,
    ValidatedWindow,
    Arc<eez_protocol::l1_history::HeaderChain>,
) {
    let (input, chain_config) = observation_fixture();
    let history = crate::testkit::checked_history(std::slice::from_ref(&input)).unwrap();
    let validator = Validator::stateless_for_test(chain_config, TEST_SYSTEM_ADDRESS);
    let window = validator.validate(&[input]).unwrap();
    (validator, window, history)
}

fn high_s_user_transaction() -> TransactionSigned {
    let transaction: reth_ethereum_primitives::TransactionSigned =
        alloy_rlp::decode_exact(hex::decode(LEGACY_USER_TX).unwrap()).unwrap();
    let signature = transaction.signature();
    let curve_order = U256::from_str_radix(
        "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141",
        16,
    )
    .unwrap();
    let high_s = Signature::new(signature.r(), curve_order - signature.s(), !signature.v());
    reth_ethereum_primitives::TransactionSigned::new_unhashed(
        transaction.into_typed_transaction(),
        high_s,
    )
    .into()
}

fn checkpoint(transaction_index: usize) -> ReplayStateCheckpoint {
    ReplayStateCheckpoint {
        transaction_index,
        state_root: B256::ZERO,
    }
}

fn prefix_test_block(
    mutate_body: impl FnOnce(&mut alloy_consensus::BlockBody<TransactionSigned>),
) -> RecoveredBlock<Block> {
    let transaction = TxLegacy::default()
        .into_signed(alloy_primitives::Signature::test_signature())
        .into();
    let mut body = alloy_consensus::BlockBody {
        transactions: vec![transaction],
        ..Default::default()
    };
    mutate_body(&mut body);
    RecoveredBlock::new_unhashed(Block::new(Header::default(), body), vec![Address::ZERO])
}

fn assert_prefix_policy_rejection(
    block: &RecoveredBlock<Block>,
    has_execution_requests: bool,
    expected_reason: &str,
) {
    let error = derive_prefix_block_hashes(
        block,
        &[EthereumReceipt::default()],
        &[checkpoint(0)],
        has_execution_requests,
    )
    .unwrap_err();
    let ValidationError::Rejected(reason) = error else {
        panic!("expected a fail-closed prefix-policy rejection, got {error}");
    };
    assert!(
        reason.contains(expected_reason),
        "unexpected rejection: {reason}"
    );
}

#[test]
fn prefix_block_hash_rejects_nonempty_consensus_withdrawals() {
    let block = prefix_test_block(|body| {
        body.withdrawals = Some(Default::default());
        body.withdrawals.as_mut().unwrap().push(Default::default());
    });

    assert_prefix_policy_rejection(&block, false, "has withdrawals");
}

#[test]
fn prefix_block_hash_rejects_ommers() {
    let block = prefix_test_block(|body| body.ommers.push(Header::default()));

    assert_prefix_policy_rejection(&block, false, "has ommers");
}

#[test]
fn prefix_block_hash_rejects_nonempty_eip_7685_requests() {
    let block = prefix_test_block(|_| {});

    assert_prefix_policy_rejection(&block, true, "produces EIP-7685 requests");
}

#[test]
fn ordinary_prefix_base_rejects_unsupported_post_execution_semantics() {
    let block = prefix_test_block(|body| {
        body.withdrawals = Some(Default::default());
        body.withdrawals.as_mut().unwrap().push(Default::default());
        body.ommers.push(Header::default());
    });

    assert!(matches!(
        derive_prefix_block_hashes(&block, &[EthereumReceipt::default()], &[], true),
        Err(ValidationError::Rejected(_))
    ));
}

fn outbound_log(address: Address, call_hash: B256) -> Log {
    outbound_log_with_gas(address, call_hash, 0)
}

fn outbound_log_with_gas(address: Address, call_hash: B256, call_gas: u64) -> Log {
    Log {
        address,
        data: CrossChainCallExecuted {
            crossChainCallHash: call_hash,
            proxy: Address::ZERO,
            sourceAddress: Address::repeat_byte(0x11),
            callData: Bytes::from_static(&[0xaa, 0xbb]),
            value: U256::from(7),
            callGas: call_gas,
        }
        .encode_log_data(),
    }
}

fn receipt_with_logs(logs: Vec<Log>) -> EthereumReceipt {
    EthereumReceipt {
        success: true,
        logs,
        ..Default::default()
    }
}

#[test]
fn outbound_observations_require_the_eezl2_emitter_and_event_signature() {
    let call_hash = B256::repeat_byte(0x11);
    let other = Address::repeat_byte(0x22);
    let logs = vec![
        outbound_log(other, call_hash),
        Log::new_unchecked(
            EEZL2_ADDRESS,
            vec![B256::repeat_byte(0xff), call_hash],
            Bytes::new(),
        ),
        outbound_log(EEZL2_ADDRESS, call_hash),
    ];

    assert_eq!(
        observe_outbound_events(&[receipt_with_logs(logs)]),
        [OutboundEventObservation::decoded_for_test(
            0, 2, call_hash, 0,
        )]
    );
}

#[test]
fn malformed_named_outbound_events_are_retained_without_decoded_fields() {
    let call_hash = B256::repeat_byte(0x33);
    let mut topic0_only = outbound_log(EEZL2_ADDRESS, call_hash);
    topic0_only.data.topics_mut_unchecked().truncate(1);
    let mut missing_proxy = outbound_log(EEZL2_ADDRESS, call_hash);
    missing_proxy.data.topics_mut_unchecked().truncate(2);
    let mut missing_body = outbound_log(EEZL2_ADDRESS, call_hash);
    missing_body.data.data = Bytes::new();
    let mut trailing_body = outbound_log(EEZL2_ADDRESS, call_hash);
    let mut body = trailing_body.data.data.to_vec();
    body.push(0);
    trailing_body.data.data = Bytes::from(body);
    // Alloy can decode the declared tuple while ignoring the suffix. The
    // adapter's exact re-encode comparison is what rejects this candidate.
    assert!(CrossChainCallExecuted::decode_log_validate(&trailing_body).is_ok());

    assert_eq!(
        observe_outbound_events(&[receipt_with_logs(vec![
            topic0_only,
            missing_proxy,
            missing_body,
            trailing_body,
        ])]),
        [
            OutboundEventObservation::malformed_for_test(0, 0),
            OutboundEventObservation::malformed_for_test(0, 1),
            OutboundEventObservation::malformed_for_test(0, 2),
            OutboundEventObservation::malformed_for_test(0, 3),
        ]
    );
}

#[test]
fn outbound_observations_preserve_receipt_log_order_and_duplicates() {
    let a = B256::repeat_byte(0xaa);
    let b = B256::repeat_byte(0xbb);
    let noise = Log::new_unchecked(Address::ZERO, Vec::new(), Bytes::new());
    let observations = observe_outbound_events(&[
        receipt_with_logs(vec![
            outbound_log(EEZL2_ADDRESS, a),
            outbound_log(EEZL2_ADDRESS, a),
        ]),
        receipt_with_logs(vec![
            noise,
            outbound_log_with_gas(EEZL2_ADDRESS, b, u64::MAX),
            outbound_log(EEZL2_ADDRESS, a),
        ]),
    ]);

    assert_eq!(
        observations,
        [
            OutboundEventObservation::decoded_for_test(0, 0, a, 0),
            OutboundEventObservation::decoded_for_test(0, 1, a, 0),
            OutboundEventObservation::decoded_for_test(1, 1, b, u64::MAX),
            OutboundEventObservation::decoded_for_test(1, 2, a, 0),
        ]
    );
}

#[test]
fn checkpoint_response_must_match_the_plan_exactly() {
    let plan = CheckpointPlan {
        transaction_indices: vec![0, 2],
    };
    assert!(
        plan.verify_returned(&[checkpoint(0), checkpoint(2)])
            .is_ok()
    );

    for returned in [
        vec![checkpoint(0)],
        vec![checkpoint(0), checkpoint(1), checkpoint(2)],
        vec![checkpoint(2), checkpoint(0)],
    ] {
        assert!(matches!(
            plan.verify_returned(&returned),
            Err(ValidationError::InvalidBackendOutput(_))
        ));
    }
}

#[test]
fn checkpoint_plan_is_derived_from_recovered_transactions() {
    let transaction = || {
        eez_primitives::SystemTransaction {
            chain_id: 1,
            source_hash: alloy_primitives::B256::with_last_byte(0),
            gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
            to: EEZL2_ADDRESS,
            value: U256::ZERO,
            input: Bytes::new(),
        }
        .into()
    };
    let block = Block::new(
        Default::default(),
        alloy_consensus::BlockBody {
            transactions: vec![transaction(), transaction(), transaction()],
            ..Default::default()
        },
    );
    let recovered = RecoveredBlock::new_unhashed(
        block,
        vec![TEST_SYSTEM_ADDRESS, Address::ZERO, TEST_SYSTEM_ADDRESS],
    );

    let (plan, system_sender_flags) =
        CheckpointPlan::from_recovered_block(&recovered, TEST_SYSTEM_ADDRESS);

    assert_eq!(system_sender_flags, [true, false, true]);
    assert_eq!(plan.transaction_indices, [1, 2]);
}

#[test]
fn checkpoint_plan_includes_every_inbound_boundary() {
    let transaction = || {
        eez_primitives::SystemTransaction {
            chain_id: 1,
            source_hash: alloy_primitives::B256::with_last_byte(0),
            gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
            to: EEZL2_ADDRESS,
            value: U256::ZERO,
            input: Bytes::new(),
        }
        .into()
    };

    for transaction_count in [8, 9, 64, 65] {
        let block = Block::new(
            Default::default(),
            alloy_consensus::BlockBody {
                transactions: std::iter::repeat_with(transaction)
                    .take(transaction_count)
                    .collect(),
                ..Default::default()
            },
        );
        let recovered =
            RecoveredBlock::new_unhashed(block, vec![TEST_SYSTEM_ADDRESS; transaction_count]);

        let (plan, system_sender_flags) =
            CheckpointPlan::from_recovered_block(&recovered, TEST_SYSTEM_ADDRESS);

        assert_eq!(system_sender_flags, vec![true; transaction_count]);
        assert_eq!(
            plan.transaction_indices,
            (0..transaction_count).collect::<Vec<_>>()
        );
    }
}

#[test]
fn sender_classification_uses_the_configured_system_address() {
    let transaction = TxLegacy::default()
        .into_signed(alloy_primitives::Signature::test_signature())
        .into();
    let block = Block::new(
        Default::default(),
        alloy_consensus::BlockBody {
            transactions: vec![transaction],
            ..Default::default()
        },
    );
    let recovered = RecoveredBlock::new_unhashed(block, vec![TEST_SYSTEM_ADDRESS]);

    assert_eq!(system_sender_flags(&recovered, TEST_SYSTEM_ADDRESS), [true]);
    assert_eq!(
        system_sender_flags(&recovered, Address::repeat_byte(0xbb)),
        [false]
    );
}

#[test]
fn recovered_sender_facts_follow_the_homestead_signature_rule() {
    let transaction = high_s_user_transaction();
    assert!(transaction.recover_signer().is_err());
    assert_eq!(
        transaction.recover_signer_unchecked().unwrap(),
        alloy_primitives::address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266")
    );
    let header = Header {
        number: 7,
        ..Default::default()
    };
    let block = Block::new(
        header,
        alloy_consensus::BlockBody {
            transactions: vec![transaction],
            ..Default::default()
        },
    );

    let pre_homestead = ChainSpec::from_genesis(Genesis {
        config: ChainConfig {
            homestead_block: Some(8),
            ..Default::default()
        },
        ..Default::default()
    });
    let recovered = recover_block(block.clone(), &pre_homestead).unwrap();
    assert_eq!(
        system_sender_flags(&recovered, TEST_SYSTEM_ADDRESS),
        [false]
    );

    let post_homestead = ChainSpec::from_genesis(Genesis {
        config: ChainConfig {
            homestead_block: Some(7),
            ..Default::default()
        },
        ..Default::default()
    });
    assert!(recover_block(block, &post_homestead).is_err());
}

#[test]
fn validates_the_golden_block_through_stateless() {
    let input = fixture_input();
    let parent: Header = alloy_rlp::decode_exact(input.witness.headers.last().unwrap()).unwrap();
    let captured: Block = alloy_rlp::decode_exact(&input.rlp).unwrap();
    let expected_hash = input.claimed_hash;
    let output = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS)
        .validate(&[input])
        .unwrap();
    let pre_root = parent.state_root;
    let post_root = captured.header.state_root;
    assert_ne!(
        pre_root, post_root,
        "the recorded history import must change state"
    );
    assert_eq!(captured.body.transactions.len(), 1);
    assert!(captured.body.transactions[0].is_history());
    assert!(captured.header.gas_used > 21_000);
    assert_eq!(output.pre_state_root, pre_root);
    assert_eq!(output.blocks.len(), 1);
    let block = &output.blocks[0];
    assert_eq!(block.computed_hash, expected_hash);
    assert_eq!(block.post_state_root, post_root);
    assert_eq!(block.receipt_successes, [true]);
    assert!(block.transaction_state_checkpoints.is_empty());
    assert_eq!(block.settlement_evidence.system_sender_flags, [false]);
    assert!(
        block
            .settlement_evidence
            .observed_outbound_events
            .is_empty()
    );
}

#[test]
fn selected_checkpoints_flow_through_the_stateless_adapter() {
    let (mut input, chain_config, expected) = checkpoint_fixture();
    let expected_hash = input.claimed_hash;
    let history = crate::testkit::checked_history(std::slice::from_ref(&input)).unwrap();

    let output = Backend::new(chain_config, TEST_SYSTEM_ADDRESS)
        .validate_blocks(
            std::slice::from_mut(&mut input),
            &CancellationToken::default(),
            history,
        )
        .unwrap();

    let block = &output.blocks[0];
    assert_eq!(block.settlement_evidence.system_sender_flags, [true, false]);
    assert!(
        !block
            .settlement_evidence
            .observed_outbound_events
            .is_empty()
    );
    assert_eq!(block.computed_hash, expected_hash);
    assert_eq!(
        block
            .transaction_state_checkpoints
            .iter()
            .map(|checkpoint| checkpoint.transaction_index)
            .collect::<Vec<_>>(),
        expected,
    );
    assert_eq!(
        block
            .transaction_state_checkpoints
            .last()
            .unwrap()
            .block_hash,
        expected_hash,
    );
}

#[test]
fn authenticated_observation_replay_accepts_exact_static_result() {
    let (validator, window, history) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let calldata: Bytes = alloy_primitives::keccak256("lastValue()")[..4]
        .to_vec()
        .into();
    let mut claim = observation_claim(
        window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash(),
        CallMode::Static,
        calldata,
    );
    let outcome = backend
        .replay_observations(&window, std::slice::from_ref(&claim), Arc::clone(&history))
        .unwrap()
        .pop()
        .unwrap();
    assert!(outcome.success);
    assert_eq!(outcome.output.as_ref(), U256::ZERO.to_be_bytes::<32>());
    claim.expected_success = outcome.success;
    claim.expected_output = outcome.output;

    backend
        .verify_observations(&window, &[claim], Arc::clone(&history))
        .unwrap();
}

#[test]
fn authenticated_observation_replay_accepts_each_fifo_prefix() {
    let (validator, window, history) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let calldata: Bytes = alloy_primitives::keccak256("lastValue()")[..4]
        .to_vec()
        .into();
    let mut pins = vec![
        window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash(),
    ];
    pins.extend(
        window
            .settling_block()
            .transaction_state_checkpoints()
            .iter()
            .map(|checkpoint| checkpoint.block_hash),
    );
    if pins.last().copied() != Some(window.window_post_block_hash()) {
        pins.push(window.window_post_block_hash());
    }
    let mut accepted = Vec::new();

    for pin in pins {
        let mut claim = observation_claim(pin, CallMode::Static, calldata.clone());
        match backend.replay_observations(
            &window,
            std::slice::from_ref(&claim),
            Arc::clone(&history),
        ) {
            Ok(mut outcomes) => {
                let outcome = outcomes.pop().unwrap();
                claim.expected_success = outcome.success;
                claim.expected_output = outcome.output;
                accepted.push(claim);
            }
            Err(error) => panic!("recognized FIFO pin failed unexpectedly: {error}"),
        }
    }
    assert!(!accepted.is_empty());

    backend
        .verify_observations(&window, &accepted, Arc::clone(&history))
        .unwrap();
}

#[test]
fn authenticated_observation_replay_rejects_forged_success_and_output() {
    let (validator, window, history) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let calldata: Bytes = alloy_primitives::keccak256("lastValue()")[..4]
        .to_vec()
        .into();
    let mut claim = observation_claim(
        window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash(),
        CallMode::Static,
        calldata,
    );
    let outcome = backend
        .replay_observations(&window, std::slice::from_ref(&claim), Arc::clone(&history))
        .unwrap()
        .pop()
        .unwrap();
    claim.expected_success = !outcome.success;
    claim.expected_output = outcome.output.clone();
    assert_eq!(
        backend.verify_observations(&window, std::slice::from_ref(&claim), Arc::clone(&history)),
        Err(ObservationValidationError::OutcomeMismatch { index: 0 })
    );

    claim.expected_success = outcome.success;
    let mut forged_output = outcome.output.to_vec();
    forged_output.push(0xff);
    claim.expected_output = forged_output.into();
    assert_eq!(
        backend.verify_observations(&window, &[claim], Arc::clone(&history)),
        Err(ObservationValidationError::OutcomeMismatch { index: 0 })
    );
}

#[test]
fn authenticated_observation_replay_binds_mutable_lookup_output() {
    let (validator, window, history) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let mut claim = observation_claim(
        window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash(),
        CallMode::Mutable,
        alloy_primitives::keccak256("lastValue()")[..4]
            .to_vec()
            .into(),
    );
    let outcome = backend
        .replay_observations(&window, std::slice::from_ref(&claim), Arc::clone(&history))
        .unwrap()
        .pop()
        .unwrap();
    assert!(outcome.success);
    claim.expected_success = true;
    claim.expected_output = outcome.output;
    backend
        .verify_observations(&window, std::slice::from_ref(&claim), Arc::clone(&history))
        .unwrap();

    let mut forged_output = claim.expected_output.to_vec();
    forged_output.push(0x01);
    claim.expected_output = forged_output.into();
    assert_eq!(
        backend.verify_observations(&window, &[claim], Arc::clone(&history)),
        Err(ObservationValidationError::OutcomeMismatch { index: 0 })
    );
}

fn history_observation_claim(pin: B256, mode: CallMode, output: Bytes) -> FifoObservationClaim {
    FifoObservationClaim {
        pinned_block_hash: pin,
        call_mode: mode,
        target_address: crate::testkit::history_observations::PROBE,
        source_address: Address::with_last_byte(0x50),
        source_rollup_id: eez_protocol::RollupId::MAINNET,
        data: Bytes::new(),
        value: U256::ZERO,
        expected_success: true,
        expected_output: output,
        callbacks: Vec::new(),
    }
}

#[test]
fn scheduled_history_observations_include_history_and_unfinalized_donations() {
    for suffix in [false, true] {
        let fixture = crate::testkit::history_observations::Fixture::new(suffix);
        let admitted = fixture.admitted();
        let history = crate::testkit::checked_history(&admitted).unwrap();
        let validator = Validator::stateless_for_test(fixture.genesis.clone(), TEST_SYSTEM_ADDRESS);
        let window = validator.validate(&admitted).unwrap();
        let Validator::Stateless(backend) = &validator else {
            unreachable!()
        };
        let pin = window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash();
        assert_eq!(pin == window.window_post_block_hash(), !suffix);
        for mode in [CallMode::Static, CallMode::Mutable] {
            let claim = history_observation_claim(pin, mode, fixture.outcome(7, 1));
            backend
                .verify_observations(&window, &[claim], Arc::clone(&history))
                .unwrap();
        }
    }
}

#[test]
fn scheduled_history_observations_reject_both_earlier_states() {
    for suffix in [false, true] {
        let fixture = crate::testkit::history_observations::Fixture::new(suffix);
        let admitted = fixture.admitted();
        let history = crate::testkit::checked_history(&admitted).unwrap();
        let validator = Validator::stateless_for_test(fixture.genesis.clone(), TEST_SYSTEM_ADDRESS);
        let window = validator.validate(&admitted).unwrap();
        let Validator::Stateless(backend) = &validator else {
            unreachable!()
        };
        let pin = window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash();
        for mode in [CallMode::Static, CallMode::Mutable] {
            for (balance, origin) in [(0, 0), (0, 1)] {
                let claim = history_observation_claim(pin, mode, fixture.outcome(balance, origin));
                assert_eq!(
                    backend.verify_observations(&window, &[claim], Arc::clone(&history)),
                    Err(ObservationValidationError::OutcomeMismatch { index: 0 }),
                    "suffix={suffix}, mode={mode:?}, earlier origin={origin}",
                );
            }
        }
    }
}

#[test]
fn authenticated_observation_replay_rejects_unknown_pins_and_missing_code() {
    let (validator, mut window, history) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let calldata: Bytes = alloy_primitives::keccak256("lastValue()")[..4]
        .to_vec()
        .into();
    let unknown = observation_claim(B256::repeat_byte(0xfe), CallMode::Static, calldata.clone());
    assert_eq!(
        backend.verify_observations(&window, &[unknown], Arc::clone(&history)),
        Err(ObservationValidationError::UnknownPinnedRoot {
            index: 0,
            block_hash: B256::repeat_byte(0xfe),
        })
    );

    let claim = observation_claim(
        window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash(),
        CallMode::Static,
        calldata,
    );

    // The caller is the source's CREATE2 proxy under EEZL2. This source is
    // chosen so that the proxy's account path reaches a blinded node in the
    // captured witness; a path whose absence the witness proves is a legitimate
    // read, so the constant depends on the fixture and the EEZL2 address.
    let mut missing_account = claim.clone();
    missing_account.source_address = Address::repeat_byte(0x78);
    let error = backend
        .verify_observations(&window, &[missing_account], Arc::clone(&history))
        .unwrap_err();
    assert!(
        matches!(&error, ObservationValidationError::Replay(reason)
            if reason.contains("incomplete account witness")),
        "{error:?}"
    );

    window.settling_block.replay_input.witness.codes.clear();
    let error = backend
        .verify_observations(&window, &[claim], Arc::clone(&history))
        .unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));
}

#[test]
fn authenticated_observation_replay_rejects_non_fifo_pins() {
    let (input, genesis, _) = checkpoint_fixture();
    let history = crate::testkit::checked_history(std::slice::from_ref(&input)).unwrap();
    let validator = Validator::stateless_for_test(genesis, TEST_SYSTEM_ADDRESS);
    let window = validator.validate(&[input]).unwrap();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let checkpoint = window
        .settling_block()
        .transaction_state_checkpoints()
        .first()
        .expect("fixture must expose a transaction checkpoint")
        .block_hash;
    let claims = [
        observation_claim(checkpoint, CallMode::Static, Bytes::new()),
        observation_claim(
            window
                .settling_block()
                .block()
                .settlement_evidence()
                .ordinary_prefix_hash(),
            CallMode::Static,
            Bytes::new(),
        ),
    ];

    let error = backend
        .verify_observations(&window, &claims, Arc::clone(&history))
        .unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));
}

#[test]
fn authenticated_observation_replay_rejects_a_mutated_parent_header() {
    let (validator, mut window, history) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let claim = observation_claim(
        window
            .settling_block()
            .block()
            .settlement_evidence()
            .ordinary_prefix_hash(),
        CallMode::Static,
        Bytes::new(),
    );
    let encoded_parent = window
        .settling_block
        .replay_input
        .witness
        .headers
        .last_mut()
        .expect("fixture must include the terminal block's parent");
    let mut parent = alloy_rlp::decode_exact::<Header>(encoded_parent.as_ref()).unwrap();
    parent.gas_limit ^= 1;
    *encoded_parent = alloy_rlp::encode(parent).into();

    let error = backend
        .verify_observations(&window, &[claim], Arc::clone(&history))
        .unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));
}

#[test]
fn pre_cancelled_validation_does_not_consume_the_witness() {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let mut input = fixture_input();
    let state_items = input.witness.state.len();
    let history = crate::testkit::checked_history(std::slice::from_ref(&input)).unwrap();

    let result = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS).validate_blocks(
        std::slice::from_mut(&mut input),
        &cancellation,
        history,
    );

    assert!(matches!(result, Err(ValidationError::Cancelled)));
    assert_eq!(input.witness.state.len(), state_items);
}

#[test]
fn consensus_rlp_must_decode_exactly() {
    let mut input = fixture_input();
    input.rlp.push(0x80);
    assert!(matches!(
        Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS).validate(&[input]),
        Err(ValidationError::Rejected(_))
    ));
}

#[test]
fn decoded_header_must_match_the_streamed_number_and_parent() {
    let mutations: [fn(&mut AdmittedBlock); 2] = [
        |input| input.declared_number += 1,
        |input| input.claimed_parent_hash = B256::repeat_byte(0xee),
    ];
    for mutate in mutations {
        let mut input = fixture_input();
        mutate(&mut input);
        assert!(matches!(
            Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS).validate(&[input]),
            Err(ValidationError::Rejected(_))
        ));
    }
}

#[test]
fn decoded_header_hash_must_match_the_streamed_hash_before_reexecution() {
    let mut input = fixture_input();
    input.claimed_hash = B256::repeat_byte(0xee);
    let mut node = input.witness.state[0].to_vec();
    node[0] ^= 0x01;
    input.witness.state[0] = node.into();
    let error = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS)
        .validate(&[input])
        .unwrap_err();
    assert!(matches!(error, ValidationError::Rejected(_)));
    assert!(
        error
            .to_string()
            .contains("does not match streamed block hash")
    );
}

#[test]
fn execution_must_produce_the_state_root_committed_by_the_header() {
    let mut input = fixture_input();
    let mut block = alloy_rlp::decode_exact::<Block>(&input.rlp).unwrap();
    block.header.state_root = B256::repeat_byte(0xee);
    input.claimed_hash = block.header.hash_slow();
    input.rlp = alloy_rlp::encode(block);

    let error = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS)
        .validate(&[input])
        .unwrap_err();

    assert!(matches!(
        error,
        ValidationError::Rejected(reason) if reason.contains("mismatched post-state root")
    ));
}

#[test]
fn corrupt_witness_data_is_rejected_by_stateless() {
    let mut input = fixture_input();
    let mut node = input.witness.state[0].to_vec();
    node[0] ^= 0x01;
    input.witness.state[0] = node.into();
    assert!(matches!(
        Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS).validate(&[input]),
        Err(ValidationError::Rejected(_))
    ));
}

#[test]
fn ordinary_prefix_then_two_native_actions_preserve_every_execution_checkpoint() {
    use alloy_consensus::TxReceipt as _;
    use eez_primitives::{EezTxType, SYSTEM_ADDRESS, SystemTransaction};

    // Expected nonce/balance roots include the mandatory history predeploy,
    // assembled independently from values rather than an EVM execution.
    fn state(
        user: Address,
        user_nonce: u64,
        balance: U256,
        collector_balance: U256,
        system_balance: U256,
    ) -> (B256, Vec<Bytes>) {
        let mut accounts = vec![
            (user, user_nonce, balance),
            (SYSTEM_ADDRESS, 7, system_balance),
        ];
        if collector_balance != U256::ZERO {
            accounts.push((
                alloy_primitives::address!("f39fd6e51aad88f6f4ce6ab8827279cfffb92266"),
                0,
                collector_balance,
            ));
        }
        crate::testkit::history::state(&accounts, 100)
    }
    let genesis = crate::testkit::history::genesis();
    let key = alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let user = key.address();
    let balance = U256::from(1_000_000_000_000_000_000u64);
    let (pre_root, witness_nodes) = state(user, 0, balance, U256::ZERO, U256::ZERO);
    let parent = Header {
        number: 10,
        timestamp: 100,
        gas_limit: 30_000_000,
        beneficiary: user,
        state_root: pre_root,
        base_fee_per_gas: Some(1_000_000_000),
        withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
        parent_beacon_block_root: Some(B256::ZERO),
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        requests_hash: Some(b256!(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        )),
        ..Default::default()
    };
    let base_fee = parent
        .next_block_base_fee(eez_protocol::l2::BASE_FEE_PARAMS)
        .unwrap();
    let ordinary = TxLegacy {
        chain_id: Some(1),
        nonce: 0,
        gas_limit: 21_000,
        gas_price: u128::from(base_fee),
        to: alloy_primitives::TxKind::Call(SYSTEM_ADDRESS),
        value: U256::from(11),
        ..Default::default()
    };
    let signature = Signature::from(
        key.credential()
            .sign_prehash_recoverable(ordinary.signature_hash().as_slice())
            .unwrap(),
    );
    let mut transactions: Vec<TransactionSigned> = vec![ordinary.into_signed(signature).into()];
    let selector = eez_protocol::entries::EXECUTE_INCOMING_SELECTOR;
    let native_gas = 21_000
        + selector
            .iter()
            .map(|byte| if *byte == 0 { 10 } else { 40 })
            .sum::<u64>();
    for occurrence in [7, 8] {
        transactions.push(
            SystemTransaction {
                chain_id: 1,
                source_hash: B256::with_last_byte(occurrence),
                gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
                to: EEZL2_ADDRESS,
                value: U256::from(occurrence),
                input: Bytes::copy_from_slice(&selector),
            }
            .into(),
        );
    }
    let receipts: Vec<_> = [
        EezTxType::Ethereum(alloy_consensus::TxType::Legacy),
        EezTxType::System,
        EezTxType::System,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, tx_type)| EthereumReceipt {
        tx_type,
        success: true,
        cumulative_gas_used: 21_000 + native_gas * index as u64,
        logs: Vec::new(),
    })
    .collect();
    let fee = U256::from(base_fee) * U256::from(21_000);
    let remaining = balance - fee - U256::from(11);
    let mut expected = Vec::new();
    for count in 1..=3 {
        let prefix_receipts: Vec<_> = receipts[..count]
            .iter()
            .map(|receipt| receipt.with_bloom_ref())
            .collect();
        expected.push(Header {
            number: 11,
            timestamp: 102,
            parent_hash: parent.hash_slow(),
            base_fee_per_gas: Some(base_fee),
            gas_used: 21_000 + native_gas * (count as u64 - 1),
            // Closing B[0] burns its ordinary donation. Continuing the raw
            // prefix still carries that donation into the first native group.
            state_root: state(user, 1, remaining, fee, U256::ZERO).0,
            transactions_root: calculate_transaction_root(&transactions[..count]),
            receipts_root: calculate_receipt_root(&prefix_receipts),
            ..parent.clone()
        });
    }
    let full = Block::new(
        expected[2].clone(),
        alloy_consensus::BlockBody {
            transactions,
            withdrawals: Some(Default::default()),
            ..Default::default()
        },
    );
    let input = AdmittedBlock {
        l1_headers_rlp: crate::testkit::history::l1_headers()
            .into_iter()
            .map(Into::into)
            .collect(),
        declared_number: 11,
        claimed_hash: expected[2].hash_slow(),
        claimed_parent_hash: parent.hash_slow(),
        rlp: alloy_rlp::encode(full),
        witness: alloy_rpc_types_debug::ExecutionWitness {
            state: witness_nodes,
            codes: vec![Bytes::copy_from_slice(
                eez_protocol::l1_history::HISTORY_RUNTIME,
            )],
            headers: vec![alloy_rlp::encode(&parent).into()],
            ..Default::default()
        },
    };
    for (index, header) in expected.iter().enumerate() {
        let mut prefix = input.clone();
        let mut block = alloy_rlp::decode_exact::<Block>(&prefix.rlp).unwrap();
        block.header = header.clone();
        block.body.transactions.truncate(index + 1);
        prefix.claimed_hash = header.hash_slow();
        prefix.rlp = alloy_rlp::encode(block);
        let validated = Backend::new(genesis.clone(), SYSTEM_ADDRESS)
            .validate(&[prefix])
            .unwrap();
        assert_eq!(validated.blocks[0].computed_hash, header.hash_slow());
        assert_eq!(validated.blocks[0].post_state_root, header.state_root);
    }
    let output = Backend::new(genesis.clone(), SYSTEM_ADDRESS)
        .validate(std::slice::from_ref(&input))
        .unwrap();
    let block = &output.blocks[0];
    assert_eq!(block.receipt_successes, [true, true, true]);
    assert_eq!(
        block.settlement_evidence.system_sender_flags,
        [false, true, true]
    );
    assert_eq!(
        block.settlement_evidence.ordinary_prefix_hash(),
        expected[0].hash_slow()
    );
    assert_ne!(expected[0].hash_slow(), parent.hash_slow());
    assert_eq!(
        block
            .transaction_state_checkpoints
            .iter()
            .map(|checkpoint| (checkpoint.transaction_index, checkpoint.block_hash))
            .collect::<Vec<_>>(),
        vec![(1, expected[1].hash_slow()), (2, expected[2].hash_slow())]
    );
    assert_eq!(block.post_state_root, expected[2].state_root);
    assert_eq!(block.computed_hash, expected[2].hash_slow());
    let mut unclean = input;
    let mut full = alloy_rlp::decode_exact::<Block>(&unclean.rlp).unwrap();
    full.header.state_root = state(user, 1, remaining, fee, U256::from(11)).0;
    unclean.claimed_hash = full.header.hash_slow();
    unclean.rlp = alloy_rlp::encode(full);
    let error = Backend::new(genesis, SYSTEM_ADDRESS)
        .validate(&[unclean])
        .unwrap_err();
    assert!(
        matches!(error, ValidationError::Rejected(reason) if reason.contains("mismatched post-state root"))
    );
}
