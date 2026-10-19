use alloy_consensus::{Header, SignableTransaction as _, TxLegacy};
use alloy_primitives::{Log, Signature, b256};
use eez_primitives::{EezTxEnvelope as TransactionSigned, Receipt as EthereumReceipt};
use eez_protocol::CallMode;
use reth_primitives_traits::SignerRecoverable as _;

use super::*;
use crate::testkit::{LEGACY_USER_TX, TEST_SYSTEM_ADDRESS};
use crate::validate::Validator;

fn fixture_genesis() -> Genesis {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native-osaka-empty/l2-genesis.json"
    )))
    .unwrap()
}

fn fixture_input() -> AdmittedBlock {
    let rlp = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native-osaka-empty/block.rlp"
    ))
    .to_vec();
    let block = alloy_rlp::decode_exact::<Block>(&rlp).unwrap();
    let witness = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native-osaka-empty/witness.json"
    )))
    .unwrap();
    AdmittedBlock {
        declared_number: block.header.number,
        claimed_hash: block.header.hash_slow(),
        claimed_parent_hash: block.header.parent_hash,
        rlp,
        witness,
    }
}

#[test]
fn reconstruction_fact_is_built_only_from_exact_replay_outputs() {
    let input = fixture_input();
    let backend = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS);
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
    use eez_control_rpc::v1::{ProveChunk, prove_chunk};
    use prost::Message as _;
    let directory = crate::testkit::manifest_dir().join("tests/fixtures/native-osaka-lookup");
    let bytes = std::fs::read(directory.join("cold-request.protobuf")).unwrap();
    let mut remaining = bytes.as_slice();
    let _header = ProveChunk::decode_length_delimited(&mut remaining).unwrap();
    let Some(prove_chunk::Kind::Block(block)) = ProveChunk::decode_length_delimited(&mut remaining)
        .unwrap()
        .kind
    else {
        panic!("capture must begin with a deployment block");
    };
    let input = AdmittedBlock::from_wire(
        block,
        &mut 0,
        crate::window::WindowLimits {
            blocks: 1,
            payload_bytes: 1024 * 1024,
            witness_items: 100_000,
        },
    )
    .unwrap();
    let genesis =
        serde_json::from_slice(&std::fs::read(directory.join("l2-genesis.json")).unwrap()).unwrap();
    let backend = Backend::new(genesis, TEST_SYSTEM_ADDRESS);
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

fn validated_observation_fixture() -> (Validator, ValidatedWindow) {
    let (input, chain_config) = observation_fixture();
    let validator = Validator::stateless_for_test(chain_config, TEST_SYSTEM_ADDRESS);
    let window = validator.validate(&[input]).unwrap();
    (validator, window)
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
            nonce: 0,
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
            nonce: 0,
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
    let output = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS)
        .validate(&[fixture_input()])
        .unwrap();
    let pre_root = b256!("345cfb3dfd143f8484075eccccd3e14c3adaa3da42adfcda081328be75a7b178");
    let post_root = b256!("c03f7e9458ad5d7523f21bd518ad68730effb14c1323ac0264a1bcd817819ce2");
    assert_ne!(
        pre_root, post_root,
        "an empty block still writes its parent's hash via EIP-2935"
    );
    assert_eq!(output.pre_state_root, pre_root);
    assert_eq!(output.blocks.len(), 1);
    let block = &output.blocks[0];
    assert_eq!(
        block.computed_hash,
        b256!("d5fdf6983f57dddaff35f2004d7965813b7bff1a4da48e47734fda8042acb858")
    );
    assert_eq!(block.post_state_root, post_root);
    assert!(block.receipt_successes.is_empty());
    assert!(block.transaction_state_checkpoints.is_empty());
    assert!(block.settlement_evidence.system_sender_flags.is_empty());
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

    let output = Backend::new(chain_config, TEST_SYSTEM_ADDRESS)
        .validate_blocks(
            std::slice::from_mut(&mut input),
            &CancellationToken::default(),
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
    let (validator, window) = validated_observation_fixture();
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
        .replay_observations(&window, std::slice::from_ref(&claim))
        .unwrap()
        .pop()
        .unwrap();
    assert!(outcome.success);
    assert_eq!(outcome.output.as_ref(), U256::ZERO.to_be_bytes::<32>());
    claim.expected_success = outcome.success;
    claim.expected_output = outcome.output;

    backend.verify_observations(&window, &[claim]).unwrap();
}

#[test]
fn authenticated_observation_replay_accepts_each_fifo_prefix() {
    let (validator, window) = validated_observation_fixture();
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
        match backend.replay_observations(&window, std::slice::from_ref(&claim)) {
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

    backend.verify_observations(&window, &accepted).unwrap();
}

#[test]
fn authenticated_observation_replay_rejects_forged_success_and_output() {
    let (validator, window) = validated_observation_fixture();
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
        .replay_observations(&window, std::slice::from_ref(&claim))
        .unwrap()
        .pop()
        .unwrap();
    claim.expected_success = !outcome.success;
    claim.expected_output = outcome.output.clone();
    assert_eq!(
        backend.verify_observations(&window, std::slice::from_ref(&claim)),
        Err(ObservationValidationError::OutcomeMismatch { index: 0 })
    );

    claim.expected_success = outcome.success;
    let mut forged_output = outcome.output.to_vec();
    forged_output.push(0xff);
    claim.expected_output = forged_output.into();
    assert_eq!(
        backend.verify_observations(&window, &[claim]),
        Err(ObservationValidationError::OutcomeMismatch { index: 0 })
    );
}

#[test]
fn authenticated_observation_replay_binds_mutable_lookup_output() {
    let (validator, window) = validated_observation_fixture();
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
        .replay_observations(&window, std::slice::from_ref(&claim))
        .unwrap()
        .pop()
        .unwrap();
    assert!(outcome.success);
    claim.expected_success = true;
    claim.expected_output = outcome.output;
    backend
        .verify_observations(&window, std::slice::from_ref(&claim))
        .unwrap();

    let mut forged_output = claim.expected_output.to_vec();
    forged_output.push(0x01);
    claim.expected_output = forged_output.into();
    assert_eq!(
        backend.verify_observations(&window, &[claim]),
        Err(ObservationValidationError::OutcomeMismatch { index: 0 })
    );
}

#[test]
fn authenticated_observation_replay_rejects_unknown_pins_and_missing_code() {
    let (validator, mut window) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let calldata: Bytes = alloy_primitives::keccak256("lastValue()")[..4]
        .to_vec()
        .into();
    let unknown = observation_claim(B256::repeat_byte(0xfe), CallMode::Static, calldata.clone());
    assert_eq!(
        backend.verify_observations(&window, &[unknown]),
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
        .verify_observations(&window, &[missing_account])
        .unwrap_err();
    assert!(
        matches!(&error, ObservationValidationError::Replay(reason)
            if reason.contains("incomplete account witness")),
        "{error:?}"
    );

    window.settling_block.replay_input.witness.codes.clear();
    let error = backend.verify_observations(&window, &[claim]).unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));
}

#[test]
fn authenticated_observation_replay_rejects_non_fifo_pins() {
    let (input, genesis, _) = checkpoint_fixture();
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

    let error = backend.verify_observations(&window, &claims).unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));
}

#[test]
fn authenticated_observation_replay_rejects_a_mutated_parent_header() {
    let (validator, mut window) = validated_observation_fixture();
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

    let error = backend.verify_observations(&window, &[claim]).unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));
}

#[test]
fn historical_signed_selectors_do_not_become_native_transactions() {
    let rlp = hex::decode(
        include_str!(
            "../../../tests/fixtures/stateless-checkpoint-2175/checkpoint-block-2175.rlp.hex"
        )
        .trim()
        .trim_start_matches("0x"),
    )
    .unwrap();
    let block: Block = alloy_rlp::decode_exact(&rlp).unwrap();
    assert!(
        block
            .body
            .transactions
            .iter()
            .all(|transaction| !matches!(transaction, TransactionSigned::System(_)))
    );
    assert!(
        block
            .body
            .transactions
            .iter()
            .all(|transaction| transaction.recover_signer().unwrap() != TEST_SYSTEM_ADDRESS)
    );
}

#[test]
fn pre_cancelled_validation_does_not_consume_the_witness() {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let mut input = fixture_input();
    let state_items = input.witness.state.len();

    let result = Backend::new(fixture_genesis(), TEST_SYSTEM_ADDRESS)
        .validate_blocks(std::slice::from_mut(&mut input), &cancellation);

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

    // Hand-built two-account MPT: distinct first nibbles yield one branch and
    // two 63-nibble leaves. Expected nonce/balance roots do not use an EVM or
    // the checkpoint/reconstruction implementation under test.
    fn state(
        user: Address,
        user_nonce: u64,
        balance: U256,
        native_nonce: u64,
    ) -> (B256, Vec<Bytes>) {
        #[derive(alloy_rlp::RlpEncodable)]
        struct Account {
            nonce: u64,
            balance: U256,
            storage_root: B256,
            code_hash: B256,
        }
        let mut branch = vec![Bytes::new(); 17];
        let mut nodes = Vec::new();
        for (address, nonce, balance) in [
            (user, user_nonce, balance),
            (SYSTEM_ADDRESS, native_nonce, U256::ZERO),
        ] {
            let key = keccak256(address);
            let child = usize::from(key[0] >> 4);
            assert!(
                branch[child].is_empty(),
                "fixture keys need distinct first nibbles"
            );
            let mut path = vec![0x30 | (key[0] & 0x0f)];
            path.extend_from_slice(&key[1..]);
            let leaf = alloy_rlp::encode(vec![
                Bytes::from(path),
                Bytes::from(alloy_rlp::encode(Account {
                    nonce,
                    balance,
                    storage_root: alloy_consensus::EMPTY_ROOT_HASH,
                    code_hash: alloy_primitives::KECCAK256_EMPTY,
                })),
            ]);
            assert!(leaf.len() >= 32);
            branch[child] = Bytes::copy_from_slice(keccak256(&leaf).as_slice());
            nodes.push(leaf.into());
        }
        let root = alloy_rlp::encode(branch);
        let hash = keccak256(&root);
        nodes.push(root.into());
        (hash, nodes)
    }

    let mut genesis = fixture_genesis();
    genesis.config.chain_id = 1;
    genesis.timestamp = 80;
    let schedule = genesis.config.extra_fields.get_mut("rollupX").unwrap();
    schedule["blockIntervalSeconds"] = 2u64.into();
    schedule["syncIntervalBlocks"] = 6u64.into();
    schedule["l1Reference"]["timestamp"] = 80u64.into();
    let key = alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let user = key.address();
    let balance = U256::from(1_000_000_000_000_000_000u64);
    let (pre_root, witness_nodes) = state(user, 0, balance, 7);
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
        to: alloy_primitives::TxKind::Call(user),
        ..Default::default()
    };
    let signature = Signature::from(
        key.credential()
            .sign_prehash_recoverable(ordinary.signature_hash().as_slice())
            .unwrap(),
    );
    let mut transactions: Vec<TransactionSigned> = vec![ordinary.into_signed(signature).into()];
    for nonce in [7, 8] {
        transactions.push(
            SystemTransaction {
                chain_id: 1,
                nonce,
                to: EEZL2_ADDRESS,
                value: U256::ZERO,
                input: Bytes::new(),
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
        cumulative_gas_used: 21_000 * (index as u64 + 1),
        logs: Vec::new(),
    })
    .collect();
    let remaining = balance - U256::from(base_fee) * U256::from(21_000);
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
            gas_used: 21_000 * count as u64,
            state_root: state(user, 1, remaining, 6 + count as u64).0,
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
        declared_number: 11,
        claimed_hash: expected[2].hash_slow(),
        claimed_parent_hash: parent.hash_slow(),
        rlp: alloy_rlp::encode(full),
        witness: alloy_rpc_types_debug::ExecutionWitness {
            state: witness_nodes,
            headers: vec![alloy_rlp::encode(&parent).into()],
            ..Default::default()
        },
    };
    let output = Backend::new(genesis, SYSTEM_ADDRESS)
        .validate(&[input])
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
}
