//! Pure settlement regressions with controlled execution facts.
//! These tests never authorize or sign; native v2 transcripts exercise the complete boundary.

use super::*;

/// Exercise backend-output and settlement gates without manufacturing signing
/// authority. The stub cannot establish a deployment, schedule or L1 anchor;
/// the v2 native transcript tests cover those gates and the actual attester.
struct SettlementChecks {
    state: Arc<ServiceState>,
}

impl SettlementChecks {
    fn new(state: Arc<ServiceState>) -> Self {
        Self { state }
    }

    fn run(&self, chunks: Vec<WindowChunk>) -> Result<B256, PipelineError> {
        let mut chunks = chunks.into_iter();
        let Some(WindowPart::Metadata(header)) = chunks.next().unwrap().kind else {
            panic!("settlement fixture must start with its metadata");
        };
        let blocks = chunks
            .map(|chunk| {
                let Some(WindowPart::Block(block)) = chunk.kind else {
                    panic!("settlement fixture must contain blocks");
                };
                AdmittedBlock::from_wire(block, &mut 0, limits().window_limits()).unwrap()
            })
            .collect::<Vec<_>>();
        let validated = self
            .state
            .validator
            .validate(&blocks)
            .map_err(PipelineError::Validation)?;
        let post = header.post_batch.unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata)
            .map_err(SettlementPipelineError::from)?;
        let posting = crate::testkit::target_from_wire(post.posting_context.as_ref().unwrap());
        let context = crate::testkit::proof_context(&batch, posting);
        let result = run_settlement(SettlementInput {
            canonical_batch: batch,
            posting_context: context,
            submitted_blobs: &post.blobs,
            validated_window: &validated,
            expected_rollup_id: self.state.expected_rollup_id,
            expected_l2_system_address: TEST_SYSTEM_ADDRESS,
            proof_system_vkey: self.state.attester.proof_system_vkey(),
            expected_proof_system: self.state.attester.expected_proof_system(),
            system_transaction_reconstructor: &self.state.system_transaction_reconstructor,
            cancellation: &CancellationToken::default(),
        })
        .map(settlement::RecomputedPublicInputsHash::into_inner)
        .map_err(PipelineError::Settlement);
        assert_eq!(self.state.attester.signing_calls(), 0);
        result
    }

    #[track_caller]
    fn check(&self, chunks: Vec<WindowChunk>) -> B256 {
        self.run(chunks)
            .expect("valid settlement fixture was rejected")
    }

    #[track_caller]
    fn reject(&self, chunks: Vec<WindowChunk>) -> Status {
        self.run(chunks)
            .expect_err("invalid settlement fixture was accepted")
            .status()
    }
}

#[test]
fn a_canonical_outbound_window_passes_settlement_checks() {
    let (batch, block_rlp, _user, call_hash) = canonical_outbound_case();
    // Controlled receipt facts isolate settlement binding; v2 transcript tests
    // separately prove that these checks are mandatory before signing.
    let validator = Validator::stub_with_settlement_evidence(
        outbound_backend_output(),
        vec![outbound_evidence(call_hash)],
    );
    let server = SettlementChecks::new(inner(validator));
    let window = single_block_settlement_window(batch, block_rlp);
    let expected_hash =
        recompute_window_public_inputs_hash(&window, server.state.attester.proof_system_vkey());

    let response = server.check(window);

    assert_eq!(response, expected_hash);
}

#[test]
fn a_mixed_outbound_then_inbound_window_passes_settlement_checks() {
    let (batch, block_rlp, outbound_call_hash) = mixed_outbound_inbound_case();
    let validator = Validator::stub_with_settlement_evidence(
        mixed_backend_output(),
        vec![mixed_evidence(outbound_call_hash)],
    );
    let server = SettlementChecks::new(inner(validator));
    let window = single_block_settlement_window(batch, block_rlp);
    let expected_hash =
        recompute_window_public_inputs_hash(&window, server.state.attester.proof_system_vkey());

    let response = server.check(window);

    assert_eq!(response, expected_hash);
}

#[test]
fn a_value_bearing_outbound_window_passes_settlement_checks() {
    let value = U256::from(1);
    let (batch, block_rlp, _user, call_hash) = outbound_case(value);
    let validator = Validator::stub_with_settlement_evidence(
        outbound_backend_output(),
        vec![outbound_evidence(call_hash)],
    );
    let server = SettlementChecks::new(inner(validator));
    let window = single_block_settlement_window(batch, block_rlp);
    let expected_hash =
        recompute_window_public_inputs_hash(&window, server.state.attester.proof_system_vkey());

    let response = server.check(window);

    assert_eq!(response, expected_hash);
}

#[test]
fn a_valid_anchor_only_window_passes_settlement_checks() {
    let server = SettlementChecks::new(one_accepting_validator());
    let window = happy_window();
    let expected_hash =
        recompute_window_public_inputs_hash(&window, server.state.attester.proof_system_vkey());

    let response = server.check(window);

    assert_eq!(response, expected_hash);
}

#[test]
fn matching_intermediate_transaction_da_payload_passes_settlement_checks() {
    let server = SettlementChecks::new(two_block_validator_with_execution_evidence(
        vec![true],
        vec![false],
        Vec::new(),
        Vec::new(),
    ));
    let mut window = vec![
        header_chunk(5, 6),
        transaction_block_chunk(5, 0x04, 0x05, non_system_transaction()),
        block_chunk(6, 0x05, 0x06),
    ];
    replace_batch_bound_to_window(&mut window, anchor_batch());

    let _response = server.check(window);
}

#[test]
fn mismatched_intermediate_transaction_da_payload_is_rejected() {
    let inner = two_block_validator_with_execution_evidence(
        vec![true],
        vec![false],
        Vec::new(),
        Vec::new(),
    );
    let server = SettlementChecks::new(Arc::clone(&inner));
    let mut window = vec![
        header_chunk(5, 6),
        transaction_block_chunk(5, 0x04, 0x05, non_system_transaction()),
        block_chunk(6, 0x05, 0x06),
    ];
    let payload_source = vec![
        header_chunk(5, 6),
        block_chunk(5, 0x04, 0x05),
        block_chunk(6, 0x05, 0x06),
    ];
    let mut batch = anchor_batch();
    bind_anchor_commitments(&mut batch, 2);
    batch.callData = da_payload_for_window(&payload_source).into();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn mismatched_immediate_entry_count_is_rejected() {
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output_for(
        &happy_block_inputs(),
    ))])));
    let mut batch = anchor_batch();
    batch.immediateEntryCount = U256::ZERO;
    let mut window = happy_window();
    replace_post_batch(
        &mut window,
        public_input_post_batch_for_empty_blocks(batch, 3),
    );

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn an_empty_batch_is_rejected_by_the_root_update_chain_gate() {
    let server = SettlementChecks::new(one_accepting_validator());
    let mut window = happy_window();
    let mut batch = anchor_batch();
    batch.entries.clear();
    batch.immediateEntryCount = U256::ZERO;
    replace_post_batch(&mut window, public_input_post_batch_for(batch));
    let error = server.run(window).unwrap_err();
    assert_eq!(error.gate(), "root_update_chain", "{error:?}");
    assert_eq!(error.status().code(), Code::FailedPrecondition);
}

#[test]
fn a_noncanonical_anchor_is_rejected_by_the_effect_prefix_gate() {
    let server = SettlementChecks::new(one_accepting_validator());
    let mut batch = anchor_batch();
    batch.entries[0].rollingHash = B256::repeat_byte(0xee);
    let mut window = happy_window();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn a_nonzero_anchor_ether_delta_is_rejected() {
    let server = SettlementChecks::new(one_accepting_validator());
    let mut batch = anchor_batch();
    batch.entries[0].rootUpdates[0].etherDelta = EtherDeltaSol::ONE;
    let mut window = happy_window();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn a_second_anchor_is_rejected_by_the_effect_prefix_gate() {
    let server = SettlementChecks::new(one_accepting_validator());
    let mut batch = anchor_batch();
    let second_anchor = batch.entries[0].clone();
    batch.entries.push(second_anchor);
    batch.immediateEntryCount = U256::from(2);
    let mut window = happy_window();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn empty_transaction_checkpoint_output_accepts_the_anchor_only_prefix() {
    let inputs = happy_block_inputs();
    let mut backend_output = backend_output_for(&inputs);
    backend_output.blocks[2].transaction_state_checkpoints = Vec::new();
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output)])));

    let _response = server.check(happy_window());
}

#[test]
fn an_anchor_only_batch_rejects_a_settling_transaction() {
    let server = SettlementChecks::new(single_block_validator_with_execution_evidence(
        vec![true],
        vec![false],
    ));

    let status = server.reject(single_non_system_transaction_window());

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn an_outbound_effect_without_an_observed_call_is_rejected() {
    let (batch, block_rlp, _user, _call_hash) = canonical_outbound_case();
    // The RLP still supplies the canonical [load, user] positions, but this
    // test backend intentionally supplies no receipt observation.
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(outbound_backend_output())])));

    let status = server.reject(single_block_settlement_window(batch, block_rlp));

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn a_multi_block_effect_uses_the_penultimate_block_root() {
    let pre_settling_root = B256::repeat_byte(0x55);
    let final_root = B256::repeat_byte(0x66);
    let inputs = [stub_block(5, 0x04, 0x05), stub_block(6, 0x05, 0x06)];
    let mut backend_output = backend_output_for(&inputs);
    backend_output.blocks[0].post_state_root = pre_settling_root;
    backend_output.blocks[1].post_state_root = final_root;
    backend_output.blocks[0].set_transaction_results_for_test(vec![true]);
    backend_output.blocks[1].set_transaction_results_for_test(vec![true]);
    backend_output.blocks[0]
        .settlement_evidence
        .set_system_sender_flags_for_test(vec![false]);
    backend_output.blocks[1]
        .settlement_evidence
        .set_system_sender_flags_for_test(vec![false]);
    backend_output.blocks[1].transaction_state_checkpoints = vec![checkpoint(0, final_root)];
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output)])));

    let mut window =
        two_block_transaction_window(non_system_transaction(), non_system_transaction());
    replace_post_batch(
        &mut window,
        public_input_post_batch_for(outbound_batch(B256::ZERO, pre_settling_root, final_root)),
    );

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn a_root_update_final_block_hash_mismatch_is_rejected() {
    let server = SettlementChecks::new(one_accepting_validator());
    let mut batch = anchor_batch();
    bind_anchor_commitments(&mut batch, 3);
    batch.entries[0].rootUpdates[0].newRoot = B256::repeat_byte(0xee);
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    batch.callData = settlement::encode_da_payload(&vec![Vec::new(); 3]).into();
    let mut window = happy_window();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn a_root_update_rollup_mismatch_is_rejected() {
    let inner = one_accepting_validator();
    let server = SettlementChecks::new(Arc::clone(&inner));
    let mut batch = anchor_batch();
    batch.entries[0].rootUpdates[0].rollupId = 2;
    let mut window = happy_window();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn malformed_settling_block_rlp_after_backend_success_is_internal() {
    let server = SettlementChecks::new(one_accepting_validator());
    let mut window = happy_window();
    block_mut(window.last_mut().unwrap()).rlp = vec![0xff];

    let status = server.reject(window);

    assert_eq!(status.code(), Code::Internal, "{status:?}");
    assert_eq!(
        status.message(),
        "validation backend returned invalid output"
    );
}

#[test]
fn distinct_reexecuted_state_roots_do_not_replace_block_hash_commitments() {
    let parent_state_root = B256::repeat_byte(0x11);
    let final_state_root = B256::repeat_byte(0x22);
    let mut backend_output = backend_output_for(&happy_block_inputs());
    backend_output.pre_state_root = parent_state_root;
    backend_output.blocks.last_mut().unwrap().post_state_root = final_state_root;
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output)])));
    let mut window = happy_window();
    replace_post_batch(
        &mut window,
        public_input_post_batch_for_empty_blocks(anchor_batch(), 3),
    );

    let _response = server.check(window);
}

#[test]
fn a_matching_nondefault_rollup_identity_passes_settlement_checks() {
    const ROLLUP_ID: u64 = 7;

    let server = SettlementChecks::new(inner_with_rollup(
        Validator::stub(vec![Ok(backend_output_for(&happy_block_inputs()))]),
        expected_rollup_id(ROLLUP_ID),
    ));
    let mut window = happy_window();
    header_mut(&mut window[0]).rollup_id = ROLLUP_ID;
    replace_post_batch(
        &mut window,
        public_input_post_batch_for_empty_blocks(anchor_batch_for(ROLLUP_ID), 3),
    );

    let _response = server.check(window);
}

#[test]
fn settlement_digest_binds_the_exact_l1_parent_and_target_timestamp() {
    for change_parent in [false, true] {
        let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output_for(
            &happy_block_inputs(),
        ))])));
        let mut post_batch = public_input_post_batch();
        let original = recompute_test_public_inputs_hash(
            &post_batch,
            server.state.attester.proof_system_vkey(),
        );
        let target = post_batch.posting_context.as_mut().unwrap();
        if change_parent {
            target.parent_hash[0] ^= 1;
        } else {
            target.target_timestamp += 1;
        }
        let expected = recompute_test_public_inputs_hash(
            &post_batch,
            server.state.attester.proof_system_vkey(),
        );
        assert_ne!(original, expected);
        let mut window = happy_window();
        replace_post_batch(&mut window, post_batch);
        let response = server.check(window);
        assert_eq!(response, expected);
        assert_ne!(response, original);
    }
}

#[test]
fn short_settling_statuses_are_an_invalid_backend_output() {
    let inner = single_block_validator_with_execution_evidence(Vec::new(), vec![false]);
    let server = SettlementChecks::new(Arc::clone(&inner));

    let status = server.reject(single_non_system_transaction_window());

    assert_eq!(status.code(), Code::Internal, "{status:?}");
    assert_eq!(
        status.message(),
        "validation backend returned invalid output"
    );
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn a_reverted_system_transaction_is_failed_precondition_after_validation() {
    let inner = single_block_validator_with_execution_evidence(vec![false], vec![true]);
    let server = SettlementChecks::new(Arc::clone(&inner));

    let status = server.reject(single_system_transaction_window());

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn a_user_revert_passes_the_system_gate_before_effect_prefix_rejection() {
    let server = SettlementChecks::new(single_block_validator_with_execution_evidence(
        vec![false],
        vec![false],
    ));

    let status = server.reject(single_non_system_transaction_window());

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
}

#[test]
fn an_intermediate_system_transaction_is_rejected() {
    let inner =
        two_block_validator_with_execution_evidence(vec![true], vec![true], Vec::new(), Vec::new());
    let server = SettlementChecks::new(Arc::clone(&inner));
    let window = vec![
        header_chunk(5, 6),
        transaction_block_chunk(5, 0x04, 0x05, system_transaction()),
        block_chunk(6, 0x05, 0x06),
    ];

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn a_successful_system_transaction_reaches_the_effect_prefix_gate() {
    let server = SettlementChecks::new(two_block_validator_with_execution_evidence(
        vec![false],
        vec![false],
        vec![true],
        vec![true],
    ));
    let window = two_block_transaction_window(non_system_transaction(), system_transaction());

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
}

#[test]
fn an_inbound_candidate_hidden_in_an_outbound_pair_is_rejected() {
    let inputs = [stub_block(5, 0x04, 0x05)];
    let mut backend_output = backend_output_for(&inputs);
    backend_output.blocks[0].set_transaction_results_for_test(vec![true, true]);
    backend_output.blocks[0]
        .settlement_evidence
        .set_system_sender_flags_for_test(vec![true, false]);
    backend_output.blocks[0].transaction_state_checkpoints = vec![checkpoint(1, B256::ZERO)];
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output)])));
    let mut window = vec![
        header_chunk(5, 5),
        transactions_block_chunk(
            5,
            0x04,
            0x05,
            vec![
                strict_inbound_transaction(U256::ZERO).0,
                non_system_transaction(),
            ],
        ),
    ];
    replace_post_batch(
        &mut window,
        public_input_post_batch_for(outbound_batch(B256::ZERO, B256::ZERO, B256::ZERO)),
    );

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn a_static_entry_carrier_is_rejected() {
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output_for(
        &happy_block_inputs(),
    ))])));
    let mut batch = anchor_batch();
    batch.staticEntries.push(Default::default());
    let mut window = happy_window();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
}

#[test]
fn invalid_public_input_structure_is_failed_precondition_after_validation() {
    let inner = inner(Validator::stub(vec![Ok(backend_output_for(
        &happy_block_inputs(),
    ))]));
    let server = SettlementChecks::new(Arc::clone(&inner));
    let mut batch = anchor_batch();
    batch.proofSystems.clear();
    let mut window = happy_window();
    replace_post_batch(&mut window, public_input_post_batch_for(batch));

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn a_batch_for_a_different_proof_system_is_rejected() {
    let inner = one_accepting_validator();
    let server = SettlementChecks::new(Arc::clone(&inner));
    let mut batch = anchor_batch();
    batch.proofSystems[0] = address!("00000000000000000000000000000000000000bb");
    let mut window = happy_window();
    replace_post_batch(
        &mut window,
        public_input_post_batch_for_empty_blocks(batch, 3),
    );

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "settlement validation rejected");
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn malformed_settlement_calldata_is_rejected_after_validation() {
    let inner = one_accepting_validator();
    let server = SettlementChecks::new(Arc::clone(&inner));
    let mut window = happy_window();
    header_mut(&mut window[0])
        .post_batch
        .as_mut()
        .unwrap()
        .abi_calldata = vec![0xde, 0xad, 0xbe, 0xef];

    let status = server.reject(window);

    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    assert_eq!(status.message(), "invalid PostBatch calldata");
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn malformed_or_trailing_da_payload_is_an_invalid_argument() {
    let backend_outputs = vec![
        Ok(backend_output_for(&happy_block_inputs())),
        Ok(backend_output_for(&happy_block_inputs())),
    ];
    let inner = inner(Validator::stub(backend_outputs));
    let server = SettlementChecks::new(Arc::clone(&inner));
    let mut trailing = settlement::encode_da_payload(&vec![Vec::new(); 3]);
    trailing.push(0xff);

    for payload in [vec![0x00], trailing] {
        let mut batch = anchor_batch();
        bind_anchor_commitments(&mut batch, 3);
        batch.callData = payload.into();
        let mut window = happy_window();
        replace_post_batch(&mut window, public_input_post_batch_for(batch));

        let status = server.reject(window);

        assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
        assert_eq!(status.message(), "invalid blob operations payload");
    }
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[test]
fn validator_rejection_precedes_settlement_decoding() {
    let server = SettlementChecks::new(inner(Validator::stub(vec![Err(
        "re-execution mismatch".to_owned()
    )])));
    let mut window = happy_window();
    header_mut(&mut window[0])
        .post_batch
        .as_mut()
        .unwrap()
        .abi_calldata
        .clear();

    let status = server.reject(window);

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "window validation rejected");
}

#[test]
fn stateless_validation_is_mandatory_and_remains_fail_closed() {
    let server = SettlementChecks::new(native_validator_state(expected_rollup_id(1)));
    let status = server.reject(stateless_window());
    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "window validation rejected");
}

#[test]
fn stateless_rejections_are_redacted() {
    let server = SettlementChecks::new(native_validator_state(expected_rollup_id(1)));
    let mut window = stateless_window();
    block_mut(&mut window[1]).parent_hash = vec![0xee; 32];

    let status = server.reject(window);
    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "window validation rejected");
}

#[test]
fn invalid_backend_outputs_are_internal_errors() {
    let server = SettlementChecks::new(inner(Validator::stub(vec![Ok(backend_output_for(&[]))])));

    let status = server.reject(happy_window());
    assert_eq!(status.code(), Code::Internal, "{status:?}");
    assert_eq!(
        status.message(),
        "validation backend returned invalid output"
    );
}

#[test]
fn the_validator_is_consulted_and_its_rejection_refuses() {
    let inner = inner(Validator::stub(vec![
        Ok(backend_output_for(&happy_block_inputs())),
        Err("re-execution mismatch".to_owned()),
    ]));
    let server = SettlementChecks::new(Arc::clone(&inner));

    // First window: controlled execution and settlement checks both accept.
    let _response = server.check(happy_window());

    // Second window: the stub rejects with a stable, redacted status.
    let status = server.reject(happy_window());
    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "window validation rejected");

    let validator = &inner.validator;
    assert_eq!(
        validator.stub_remaining(),
        0,
        "both windows reached the backend"
    );
}
