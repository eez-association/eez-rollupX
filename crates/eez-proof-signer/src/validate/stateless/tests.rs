use std::num::NonZeroU64;

use alloy_consensus::{Header, SignableTransaction as _, TxLegacy};
use alloy_primitives::{Log, Signature, TxKind, b256};
use alloy_sol_types::SolCall as _;
use eez_protocol::CallMode;
use eez_protocol::abi::computeCrossChainProxyAddressCall;
use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::SignerRecoverable as _;

use super::*;
use crate::settlement::inspect_validated_settling_block;
use crate::testkit::{SYSTEM_TX, TEST_SYSTEM_ADDRESS};
use crate::validate::ValidatedBlock;
use crate::validate::Validator;

fn fixture_chain_config() -> ChainConfig {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-block-13/chain-config.json"
    )))
    .unwrap()
}

fn fixture_input() -> AdmittedBlock {
    let rlp = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-block-13/block-13.rlp"
    ))
    .to_vec();
    let block = alloy_rlp::decode_exact::<Block>(&rlp).unwrap();
    let witness = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-block-13/witness-13.json"
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
    let backend = Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS);
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
        (fixture_input(), fixture_chain_config()),
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

fn checkpoint_fixture() -> (AdmittedBlock, ChainConfig, Vec<usize>) {
    let rlp = hex::decode(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/stateless-checkpoint-2175/checkpoint-block-2175.rlp.hex"
        ))
        .trim()
        .trim_start_matches("0x"),
    )
    .unwrap();
    let block = alloy_rlp::decode_exact::<Block>(&rlp).unwrap();
    let witness = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-checkpoint-2175/checkpoint-witness-2175.json"
    )))
    .unwrap();
    let chain_config = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-checkpoint-2175/checkpoint-chain-config.json"
    )))
    .unwrap();
    let oracle: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-checkpoint-2175/checkpoint-oracle-2175.json"
    )))
    .unwrap();
    let indices = oracle["expected_checkpoint_indices"].as_array().unwrap();
    let roots = oracle["transaction_state_roots"].as_array().unwrap();
    assert_eq!(indices.len(), roots.len());
    let checkpoints = indices
        .iter()
        .map(|transaction_index| usize::try_from(transaction_index.as_u64().unwrap()).unwrap())
        .collect();

    (
        AdmittedBlock {
            declared_number: block.header.number,
            claimed_hash: block.header.hash_slow(),
            claimed_parent_hash: block.header.parent_hash,
            rlp,
            witness,
        },
        chain_config,
        checkpoints,
    )
}

fn observation_fixture() -> (AdmittedBlock, ChainConfig) {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-observation-26382/block-witness-26382.json"
    )))
    .unwrap();
    let chain_config = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stateless-observation-26382/chain-config.json"
    )))
    .unwrap();
    let rlp = hex::decode(fixture["rlp"].as_str().unwrap()).unwrap();
    let witness = serde_json::from_value(fixture["witness"].clone()).unwrap();
    (
        AdmittedBlock {
            declared_number: fixture["declared_number"].as_u64().unwrap(),
            claimed_hash: serde_json::from_value(fixture["claimed_hash"].clone()).unwrap(),
            claimed_parent_hash: serde_json::from_value(fixture["claimed_parent_hash"].clone())
                .unwrap(),
            rlp,
            witness,
        },
        chain_config,
    )
}

fn observation_claim(
    pinned_block_hash: B256,
    call_mode: CallMode,
    data: Bytes,
) -> FifoObservationClaim {
    FifoObservationClaim {
        pinned_block_hash,
        call_mode,
        // The captured block witness contains this account and the
        // authenticated `authorizedProxies[target]` lookup made by the
        // composition inspector. Observation-only reads are covered by the
        // separate Composer witness-generation regression in eez-driver.
        target_address: alloy_primitives::address!("91bcb16fadf29d28e6ab93ddef681b2afa54434b"),
        // This proxy was exercised by the captured block, so the original
        // execution witness contains its authenticated account proof.
        source_address: alloy_primitives::address!("44554d844465037c0e0c26a4ef97a5d3a692642c"),
        source_rollup_id: eez_protocol::RollupId::MAINNET,
        data,
        value: U256::ZERO,
        expected_success: false,
        expected_output: Bytes::new(),
    }
}

fn validated_observation_fixture() -> (Validator, ValidatedWindow) {
    let (input, chain_config) = observation_fixture();
    let validator = Validator::stateless_for_test(chain_config, TEST_SYSTEM_ADDRESS);
    let window = validator.validate(&[input]).unwrap();
    (validator, window)
}

fn high_s_system_transaction() -> TransactionSigned {
    let transaction: TransactionSigned =
        alloy_rlp::decode_exact(hex::decode(SYSTEM_TX).unwrap()).unwrap();
    let signature = transaction.signature();
    let curve_order = U256::from_str_radix(
        "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141",
        16,
    )
    .unwrap();
    let high_s = Signature::new(signature.r(), curve_order - signature.s(), !signature.v());
    TransactionSigned::new_unhashed(transaction.into_typed_transaction(), high_s)
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
fn prefix_policy_is_not_applied_when_no_prefix_is_requested() {
    let block = prefix_test_block(|body| {
        body.withdrawals = Some(Default::default());
        body.withdrawals.as_mut().unwrap().push(Default::default());
        body.ommers.push(Header::default());
    });

    assert!(
        derive_prefix_block_hashes(&block, &[], &[], true)
            .unwrap()
            .is_empty()
    );
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
        TxLegacy {
            to: TxKind::Call(EEZL2_ADDRESS),
            ..Default::default()
        }
        .into_signed(alloy_primitives::Signature::test_signature())
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
        TxLegacy {
            to: TxKind::Call(EEZL2_ADDRESS),
            ..Default::default()
        }
        .into_signed(alloy_primitives::Signature::test_signature())
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
    let transaction = high_s_system_transaction();
    assert!(transaction.recover_signer().is_err());
    assert_eq!(
        transaction.recover_signer_unchecked().unwrap(),
        TEST_SYSTEM_ADDRESS
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
    assert_eq!(system_sender_flags(&recovered, TEST_SYSTEM_ADDRESS), [true]);

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
    let output = Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS)
        .validate(&[fixture_input()])
        .unwrap();
    let root = b256!("f09d8f7da5bc5036f8dd9536c953e2212390a46fb3e553ece2b7d419131537b1");

    assert_eq!(output.pre_state_root, root);
    assert_eq!(output.blocks.len(), 1);
    let block = &output.blocks[0];
    assert_eq!(
        block.computed_hash,
        b256!("16b64a78e9b3e0d533cafe81f9121735f6a2c8122c69b0bb5994ee75fe7bface")
    );
    assert_eq!(block.post_state_root, root);
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
    assert_eq!(
        block.settlement_evidence.system_sender_flags,
        [true, true, true]
    );
    assert!(
        block
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
    let calldata: Bytes = computeCrossChainProxyAddressCall {
        originalAddress: Address::repeat_byte(0x55),
        originalRollupId: 9,
    }
    .abi_encode()
    .into();
    let mut claim = observation_claim(window.settling_pre_block_hash(), CallMode::Static, calldata);
    let outcome = backend
        .replay_observations(&window, std::slice::from_ref(&claim))
        .unwrap()
        .pop()
        .unwrap();
    assert!(outcome.success);
    assert!(outcome.output.is_empty());
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
    let calldata: Bytes = computeCrossChainProxyAddressCall {
        originalAddress: Address::repeat_byte(0x66),
        originalRollupId: 11,
    }
    .abi_encode()
    .into();
    let mut pins = vec![window.settling_pre_block_hash()];
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
    let mut rejected_cross_chain = 0usize;
    for pin in pins {
        let mut claim = observation_claim(pin, CallMode::Static, calldata.clone());
        match backend.replay_observations(&window, std::slice::from_ref(&claim)) {
            Ok(mut outcomes) => {
                let outcome = outcomes.pop().unwrap();
                claim.expected_success = outcome.success;
                claim.expected_output = outcome.output;
                accepted.push(claim);
            }
            Err(ObservationValidationError::Replay(reason))
                if reason.contains("unsupported nested cross-chain call") =>
            {
                // This address is registered as a proxy by a canonical tx in
                // the fixture. Later prefixes must reject it independently,
                // which also proves the live prefix state is being used.
                rejected_cross_chain += 1;
            }
            Err(error) => panic!("recognized FIFO pin failed unexpectedly: {error}"),
        }
    }
    assert!(!accepted.is_empty());
    assert!(rejected_cross_chain > 0);

    backend.verify_observations(&window, &accepted).unwrap();
}

#[test]
fn authenticated_observation_replay_rejects_forged_success_and_output() {
    let (validator, window) = validated_observation_fixture();
    let Validator::Stateless(backend) = &validator else {
        unreachable!()
    };
    let calldata: Bytes = computeCrossChainProxyAddressCall {
        originalAddress: Address::repeat_byte(0x55),
        originalRollupId: 9,
    }
    .abi_encode()
    .into();
    let mut claim = observation_claim(window.settling_pre_block_hash(), CallMode::Static, calldata);
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
        window.settling_pre_block_hash(),
        CallMode::Mutable,
        Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
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
    let calldata: Bytes = computeCrossChainProxyAddressCall {
        originalAddress: Address::repeat_byte(0x55),
        originalRollupId: 9,
    }
    .abi_encode()
    .into();
    let unknown = observation_claim(B256::repeat_byte(0xfe), CallMode::Static, calldata.clone());
    assert_eq!(
        backend.verify_observations(&window, &[unknown]),
        Err(ObservationValidationError::UnknownPinnedRoot {
            index: 0,
            block_hash: B256::repeat_byte(0xfe),
        })
    );

    let claim = observation_claim(window.settling_pre_block_hash(), CallMode::Static, calldata);

    let mut missing_account = claim.clone();
    missing_account.source_address = Address::repeat_byte(0x77);
    let error = backend
        .verify_observations(&window, &[missing_account])
        .unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));

    window.settling_block.replay_input.witness.codes.clear();
    let error = backend.verify_observations(&window, &[claim]).unwrap_err();
    assert!(matches!(error, ObservationValidationError::Replay(_)));
}

#[test]
fn authenticated_observation_replay_rejects_non_fifo_pins() {
    let (validator, window) = validated_observation_fixture();
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
            window.settling_pre_block_hash(),
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
        window.settling_pre_block_hash(),
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
fn real_checkpoints_do_not_classify_the_legacy_inbound_selector() {
    let (mut input, chain_config, _) = checkpoint_fixture();
    let block_number = input.declared_number;
    let block_rlp = input.rlp.clone();
    let mut output = Backend::new(chain_config, TEST_SYSTEM_ADDRESS)
        .validate_blocks(
            std::slice::from_mut(&mut input),
            &CancellationToken::default(),
        )
        .unwrap();
    let block_output = output.blocks.remove(0);
    assert_eq!(block_output.receipt_successes, [true, true, true]);

    let validated_block =
        ValidatedBlock::for_test(block_number, block_rlp, block_output.settlement_evidence);
    let settling = inspect_validated_settling_block(
        &validated_block,
        block_output.receipt_successes.as_slice(),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    // The transactions are still genuine system calls and checkpoint inputs,
    // but their obsolete selector cannot enter the target inbound decoder.
    assert_eq!(settling.system_sender_flags(), [true, true, true]);
    assert!(settling.inbound_candidates().is_empty());
}

#[test]
fn pre_cancelled_validation_does_not_consume_the_witness() {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let mut input = fixture_input();
    let state_items = input.witness.state.len();

    let result = Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS)
        .validate_blocks(std::slice::from_mut(&mut input), &cancellation);

    assert!(matches!(result, Err(ValidationError::Cancelled)));
    assert_eq!(input.witness.state.len(), state_items);
}

#[test]
fn consensus_rlp_must_decode_exactly() {
    let mut input = fixture_input();
    input.rlp.push(0x80);
    assert!(matches!(
        Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS).validate(&[input]),
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
            Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS).validate(&[input]),
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
    let error = Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS)
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

    let error = Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS)
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
        Backend::new(fixture_chain_config(), TEST_SYSTEM_ADDRESS).validate(&[input]),
        Err(ValidationError::Rejected(_))
    ));
}
