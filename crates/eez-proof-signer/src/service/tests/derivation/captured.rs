//! Replay and mutate complete proving requests captured from native devnet tests.

use super::*;
use prost::Message as _;

fn chunks(bytes: &[u8]) -> Vec<WindowChunk> {
    use eez_control_rpc::v2::{ClientFrame, client_frame};
    let mut remaining = bytes;
    let mut begin = None;
    let mut blocks = Vec::new();
    let mut finalize = None;
    while !remaining.is_empty() {
        match ClientFrame::decode_length_delimited(&mut remaining)
            .unwrap()
            .kind
            .unwrap()
        {
            client_frame::Kind::Begin(value) => {
                assert!(begin.replace(value).is_none());
            }
            client_frame::Kind::Block(value) => blocks.push(value.data.unwrap()),
            client_frame::Kind::Finalize(value) => {
                assert!(finalize.replace(value).is_none());
            }
            _ => panic!("capture must be a complete cold Begin/Block/Finalize exchange"),
        }
    }
    let begin = begin.unwrap();
    let finalize = finalize.unwrap();
    let parent_rlp = blocks[0]
        .witness
        .as_ref()
        .unwrap()
        .headers
        .last()
        .unwrap()
        .clone();
    let parent: alloy_consensus::Header = alloy_rlp::decode_exact(&parent_rlp).unwrap();
    assert_eq!(parent.number, begin.anchor_number);
    assert_eq!(parent.hash_slow().as_slice(), begin.anchor_hash);
    assert_eq!(parent.state_root.as_slice(), begin.anchor_state_root);
    let post = finalize.settlement.unwrap();
    let header = WindowMetadata {
        rollup_id: begin.rollup_id,
        from_block: finalize.from_block,
        to_block: finalize.to_block,
        parent_header_rlp: parent_rlp,
        post_batch: Some(WireSettlement {
            abi_calldata: post.abi_calldata,
            blobs: post.blobs,
            posting_context: post
                .posting_context
                .map(|c| eez_control_rpc::v2::L1PostingContext {
                    chain_id: c.chain_id,
                    target_timestamp: c.target_timestamp,
                    parent_number: c.parent_number,
                    parent_hash: c.parent_hash,
                }),
        }),
    };
    std::iter::once(WindowChunk {
        kind: Some(WindowPart::Metadata(header)),
    })
    .chain(blocks.into_iter().map(|block| WindowChunk {
        kind: Some(WindowPart::Block(block)),
    }))
    .collect()
}

#[test]
#[ignore = "explicit local native-fixture inventory utility"]
fn inventory_native_proving_fixtures() {
    let directory = std::env::var("EEZ_TEST_PROVE_FIXTURE_DIR").unwrap();
    let mut files = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "protobuf")
        })
        .collect::<Vec<_>>();
    files.sort();
    for path in files {
        let input = chunks(&std::fs::read(&path).unwrap());
        let Some(WindowPart::Metadata(header)) = &input[0].kind else {
            panic!("header")
        };
        let post = header.post_batch.as_ref().unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
        let da =
            settlement::verify_da_carrier_with_limit(&batch, &post.blobs, header.rollup_id, 16)
                .unwrap();
        let payload = Payload::decode(da.payload()).unwrap();
        eprintln!(
            "{}",
            serde_json::json!({
                "file": path.file_name().unwrap().to_string_lossy(),
                "from": header.from_block, "to": header.to_block,
                "operations_bytes": da.payload().len(),
                "entries": batch.entries.iter().map(|entry| serde_json::json!({
                    "l1_to_l2": entry.expectedL1ToL2Calls.len(), "l2_to_l1": entry.l2ToL1Calls.len(),
                    "success": entry.success, "destination": entry.destinationRollupId,
                })).collect::<Vec<_>>(),
                "static_entries": batch.staticEntries.len(),
                "blocks": payload.block_count(), "ordinary_transactions": payload.transaction_count(),
            })
        );
    }
}

fn fixture_state(directory: &std::path::Path, identity: &serde_json::Value) -> Arc<ServiceState> {
    let system_address = identity["l2_system_address"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let attester = crate::attest::Attester::new(
        b256!("8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"),
        identity["vkey"].as_str().unwrap().parse().unwrap(),
        identity["proof_system"].as_str().unwrap().parse().unwrap(),
        system_address,
    )
    .unwrap();
    Arc::new(
        ServiceState::new(
            identity["registry_address"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            identity["rollup_manager_address"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            Validator::stateless(&directory.join("l2-genesis.json"), system_address).unwrap(),
            expected_rollup_id(identity["rollup_id"].as_u64().unwrap()),
            attester,
            Arc::new(reth_chainspec::ChainSpec::from(
                serde_json::from_slice::<alloy_genesis::Genesis>(
                    &std::fs::read(directory.join("l1-genesis.json")).unwrap(),
                )
                .unwrap(),
            )),
        )
        .unwrap(),
    )
}

struct Fixture {
    input: Vec<WindowChunk>,
    state: Arc<ServiceState>,
    expected_hash: B256,
    signature: Vec<u8>,
}

impl Fixture {
    async fn server(&self) -> TestServer {
        // This captured request spans more than the generic 16-block test cap.
        // Keep explicit fixture limits; production quotas are unchanged.
        TestServer::with_limits(
            Arc::clone(&self.state),
            ServiceLimits::new(ServiceLimitsParams {
                max_window_blocks: nz(4_096),
                max_window_bytes: nz(16 * 1024 * 1024),
                max_window_witness_items: nz(100_000),
                stream_idle_timeout: Duration::from_secs(5),
                request_timeout: Duration::from_mins(2),
            })
            .unwrap(),
        )
        .await
    }

    fn load(name: &str) -> Self {
        let directory = crate::testkit::manifest_dir()
            .join("tests/fixtures")
            .join(name);
        let read = |file: &str| std::fs::read(directory.join(file)).unwrap();
        let identity: serde_json::Value = serde_json::from_slice(&read("identity.json")).unwrap();
        let response: serde_json::Value = serde_json::from_slice(&read("response.json")).unwrap();
        let encoded = read("request.v2.protobuf");
        assert_eq!(
            encoded.len() as u64,
            response["request_bytes"].as_u64().unwrap()
        );
        assert_eq!(
            keccak256(&encoded),
            response["request_hash"]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap()
        );
        let state = fixture_state(&directory, &identity);
        let mut fixture = Self {
            input: chunks(&encoded),
            state,
            expected_hash: B256::from_slice(
                &hex::decode(response["public_inputs_hash"].as_str().unwrap()).unwrap(),
            ),
            signature: hex::decode(response["signature"].as_str().unwrap()).unwrap(),
        };
        let native = native_payload(&mut fixture.input);
        let semantics =
            eez_protocol::blob_semantics::decode_semantic_transactions(&native.semantic_messages)
                .unwrap();
        let actions = eez_protocol::blob_semantics::ReconstructionActions::from_semantics(
            &semantics,
            RollupId(1),
            fixture.state.validator.chain_id(),
        )
        .unwrap();
        let post = header_mut(&mut fixture.input[0])
            .post_batch
            .as_ref()
            .unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
        actions.bind_settlement(&batch).unwrap();
        assert_eq!(fixture.digest(), fixture.expected_hash);
        assert_eq!(
            Signature::try_from(fixture.signature.as_slice())
                .unwrap()
                .recover_address_from_prehash(&fixture.expected_hash)
                .unwrap(),
            fixture.state.attester.address()
        );
        fixture
    }

    fn digest(&self) -> B256 {
        recompute_window_public_inputs_for_state(&self.state, &self.input)
    }
}

fn native_payload(input: &mut [WindowChunk]) -> eez_blob_codec::NativePayload {
    let header = header_mut(&mut input[0]);
    let blobs = header
        .post_batch
        .as_ref()
        .unwrap()
        .blobs
        .iter()
        .enumerate()
        .map(|(index, bytes)| eez_blob_codec::Blob::try_from_slice(index, bytes).unwrap())
        .collect::<Vec<_>>();
    eez_blob_codec::decode_native_payload(&blobs, header.rollup_id).unwrap()
}

fn replace_native_payload(input: &mut [WindowChunk], native: eez_blob_codec::NativePayload) {
    let header = header_mut(&mut input[0]);
    let blobs = eez_blob_codec::encode_native_payload(
        header.rollup_id,
        native.operations,
        native.semantic_messages,
    )
    .unwrap();
    let post = header.post_batch.as_mut().unwrap();
    let mut batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
    batch.blobIndices = (0..blobs.len()).map(U256::from).collect();
    post.abi_calldata = eez_protocol::entries::encode_postbatch(&batch);
    post.blobs = blobs.iter().map(|blob| blob.as_bytes().to_vec()).collect();
}

pub(super) fn pipeline_error(state: &ServiceState, input: Vec<WindowChunk>) -> PipelineError {
    pipeline_error_at(
        state,
        input,
        tokio::time::Instant::now() + Duration::from_secs(30),
    )
}

fn pipeline_error_at(
    state: &ServiceState,
    input: Vec<WindowChunk>,
    deadline: tokio::time::Instant,
) -> PipelineError {
    use crate::validate::streaming::{BlockAnchor, ValidatedStreamBlock};
    let Some(WindowPart::Metadata(header)) = &input[0].kind else {
        panic!("fixture header")
    };
    let parent: alloy_consensus::Header =
        alloy_rlp::decode_exact(&header.parent_header_rlp).unwrap();
    let initial = BlockAnchor {
        number: parent.number,
        hash: parent.hash_slow(),
        state_root: parent.state_root,
    };
    let post = header.post_batch.as_ref().unwrap();
    let result = (|| {
        let mut previous = initial;
        let mut checked: Vec<Arc<ValidatedStreamBlock>> = Vec::new();
        let mut history = Arc::new(eez_protocol::l1_history::HeaderChain::default());
        let cancellation = CancellationToken::default();
        for chunk in &input[1..] {
            let Some(WindowPart::Block(block)) = &chunk.kind else {
                panic!("full fixture block")
            };
            let admitted = AdmittedBlock::from_wire(
                block.clone(),
                &mut 0,
                crate::window::WindowLimits {
                    blocks: 65_536,
                    payload_bytes: 64 * 1024 * 1024,
                    witness_items: 1_000_000,
                },
            )
            .unwrap();
            let length = history.headers().len();
            let (block, next_history) = state.validator.validate_stream_block(
                previous,
                admitted,
                history,
                length,
                checked.last().map(Arc::as_ref),
                &state.offline_rules.headers,
                131_072,
                &cancellation,
            )?;
            previous = block.as_anchor();
            checked.push(Arc::new(block));
            history = next_history;
        }
        let window = validate::ValidatedWindow::from_streamed_prefix(initial, &checked)?;
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone())
            .map_err(SettlementPipelineError::from)?;
        let context = state
            .deployment
            .candidate(
                state.expected_rollup_id.get(),
                &batch,
                post.posting_context
                    .as_ref()
                    .map(crate::testkit::target_from_wire),
                initial,
                &checked,
                &history,
            )
            .map_err(PipelineError::L1Authorization)?;
        let maximum_blobs = state
            .posting_blob_limit(context.posting_timestamp())
            .unwrap();
        super::super::super::settlement_job::settle_validated(
            state,
            &window,
            batch,
            post.blobs.clone(),
            maximum_blobs,
            context,
            deadline,
            &cancellation,
        )
    })();
    assert_eq!(state.attester.signing_calls(), 0);
    result
        .err()
        .expect("mutated request must fail the full pipeline")
}

#[test]
fn elapsed_deadline_stops_settlement_after_real_offline_replay() {
    let fixture = Fixture::load("native-offline-bidirectional");
    let error = pipeline_error_at(&fixture.state, fixture.input, tokio::time::Instant::now());
    assert!(
        matches!(error, PipelineError::DeadlineBeforeSettlement),
        "{error:?}"
    );
    assert_eq!(error.gate(), "deadline");
    assert_eq!(error.status().code(), Code::DeadlineExceeded);
    assert_eq!(fixture.state.attester.signing_calls(), 0);
}

#[test]
fn validator_rejects_a_non_protocol_system_identity() {
    let error = ServiceState::new(
        Address::repeat_byte(0x33),
        Address::repeat_byte(0x44),
        Validator::stateless_for_test(
            crate::testkit::history::genesis(),
            Address::repeat_byte(0x12),
        ),
        expected_rollup_id(1),
        test_attester(),
        crate::testkit::l1_chain_spec(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "validator does not use the native L2 system address"
    );
}

#[tokio::test]
async fn native_offline_cold_request_matches_the_posted_signature() {
    let fixture = Fixture::load("native-offline-bidirectional");
    let server = fixture.server().await;
    let response = server.attest(fixture.input.clone()).await;
    assert_attestation(
        &response,
        fixture.expected_hash,
        fixture.state.attester.address(),
    );
    assert_eq!(response.proof, fixture.signature);
    // A new server/state has no retained session or earlier witness cache.
    let cold = Fixture::load("native-offline-bidirectional");
    let restarted = cold.server().await;
    assert_eq!(restarted.attest(cold.input).await, response);
}

#[tokio::test]
async fn native_offline_request_without_l1_evidence_cannot_sign() {
    let mut fixture = Fixture::load("native-offline-bidirectional");
    for chunk in &mut fixture.input[1..] {
        block_mut(chunk)
            .witness
            .as_mut()
            .unwrap()
            .l1_headers_rlp
            .clear();
    }
    let error = pipeline_error(&fixture.state, fixture.input.clone());
    assert_eq!(error.gate(), "execution", "{error:?}");
    let server = fixture.server().await;
    let rejected = server.prove(fixture.input).await;
    assert_eq!(rejected.code(), Code::FailedPrecondition);
    assert_eq!(fixture.state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn unsigned_root_assertion_does_not_change_the_signed_proof() {
    let mut fixture = Fixture::load("native-offline-bidirectional");
    let post = header_mut(&mut fixture.input[0])
        .post_batch
        .as_mut()
        .unwrap();
    let mut batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
    batch.expectedRootPerRollup[0].root = B256::repeat_byte(0x91);
    post.abi_calldata = eez_protocol::entries::encode_postbatch(&batch);
    assert_eq!(fixture.digest(), fixture.expected_hash);
    let server = fixture.server().await;
    let proof = server.attest(fixture.input.clone()).await;
    assert_eq!(proof.proof, fixture.signature);
    assert_eq!(fixture.state.attester.signing_calls(), 1);

    // An unsigned admission hint cannot authorize a different signed entry root.
    batch.entries[0].rootUpdates[0].currentRoot = B256::repeat_byte(0x91);
    header_mut(&mut fixture.input[0])
        .post_batch
        .as_mut()
        .unwrap()
        .abi_calldata = eez_protocol::entries::encode_postbatch(&batch);
    let negative = Fixture::load("native-offline-bidirectional");
    let error = pipeline_error(&negative.state, fixture.input.clone());
    assert_eq!(error.gate(), "root_update_chain", "{error:?}");
    let negative_server = negative.server().await;
    assert_eq!(
        negative_server.prove(fixture.input).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(negative.state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn native_changing_reads_and_caught_revert_replay_original_execution_with_v0() {
    for name in ["native-osaka-changing-static", "native-osaka-lookup"] {
        let mut fixture = Fixture::load(name);
        let native = native_payload(&mut fixture.input);
        let payload = Payload::decode(&native.operations).unwrap();
        assert!((1..=4_096).contains(&payload.block_count()));
        let post = header_mut(&mut fixture.input[0])
            .post_batch
            .as_ref()
            .unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
        let semantics =
            eez_protocol::blob_semantics::decode_semantic_transactions(&native.semantic_messages)
                .unwrap();
        // The producing Composer publishes the signed origin only for an L2 origin.
        for transaction in &semantics {
            assert_eq!(
                transaction.tx_data.is_empty(),
                transaction.origin_chain.is_mainnet(),
                "{name}: tx_data publication for origin {}",
                transaction.origin_chain.0
            );
        }
        let actions = eez_protocol::blob_semantics::ReconstructionActions::from_semantics(
            &semantics,
            eez_protocol::RollupId(1),
            fixture.state.validator.chain_id(),
        )
        .unwrap();
        assert!(payload.block_count().is_multiple_of(5));
        assert_eq!(batch.entries.len(), 2);
        if name.ends_with("changing-static") {
            assert_eq!(actions.outbound().len(), 1);
            let effects = &actions.outbound()[0].effects;
            assert_eq!(effects.len(), 5);
            assert_eq!(batch.entries[1].l2ToL1Calls.len(), 5);
            let read_hash = effects[1].source_entry.as_static().unwrap().proxyEntryHash;
            for (effect, (position, value)) in
                effects[1..].iter().zip([(0, 5), (0, 5), (1, 79), (1, 79)])
            {
                let entry = effect.source_entry.as_static().unwrap();
                assert_eq!(entry.expectedEntryIndex, U256::from(position));
                assert_eq!(entry.proxyEntryHash, read_hash);
                assert!(entry.success);
                assert_eq!(
                    entry.returnData.as_ref(),
                    U256::from(value).to_be_bytes::<32>()
                );
            }
        } else {
            assert!(!batch.entries[1].success);
            assert!(actions.inbound().is_empty());
            assert!(actions.outbound().is_empty());
        }
        let server = fixture.server().await;
        let response = server
            .exchange(fixture.input.clone())
            .await
            .unwrap_or_else(|status| {
                panic!(
                    "{name}: {status:?}; {:?}",
                    pipeline_error(&fixture.state, fixture.input.clone())
                )
            });
        assert_attestation(
            &response,
            fixture.expected_hash,
            fixture.state.attester.address(),
        );
        assert_eq!(response.proof, fixture.signature);
        // Retrying the identical request must recompute the same result.
        let retry = server.attest(fixture.input).await;
        assert_eq!(retry, response);
        assert_eq!(fixture.state.attester.signing_calls(), 2);
    }
}

#[tokio::test]
async fn native_action_source_gas_and_outcomes_cannot_be_changed_before_signing() {
    let mut fixture = Fixture::load("native-osaka-changing-static");
    let original = native_payload(&mut fixture.input);
    let server = fixture.server().await;
    for mutation in 0..3 {
        let mut native = original.clone();
        let mut changed = false;
        for message in &mut native.semantic_messages {
            use eez_blob_codec::Message;
            match (mutation, message) {
                (0, Message::Call { from_address, .. }) => {
                    *from_address = Address::repeat_byte(0x99);
                    changed = true;
                }
                (1, Message::StaticCall { gas, .. }) => {
                    *gas = 1;
                    changed = true;
                }
                (2, Message::ReturnSuccess { return_data }) => {
                    *return_data = Bytes::from_static(b"forged");
                    changed = true;
                }
                _ => {}
            }
            if changed {
                break;
            }
        }
        assert!(changed, "fixture must contain mutation target {mutation}");
        let mut input = fixture.input.clone();
        replace_native_payload(&mut input, native);
        // The carrier and witness remain valid. Independent semantic binding
        // must refute changes to the source, fixed gas profile, or result.
        let error = pipeline_error(&fixture.state, input.clone());
        assert_eq!(
            error.gate(),
            if mutation == 1 {
                "blob_da"
            } else {
                "blob_semantics"
            },
            "mutation {mutation}: {error:?}"
        );
        assert_eq!(
            server.prove(input).await.code(),
            if mutation == 1 {
                Code::InvalidArgument
            } else {
                Code::FailedPrecondition
            }
        );
        assert_eq!(fixture.state.attester.signing_calls(), 0);
    }
}

#[tokio::test]
async fn forged_native_lookup_outcome_reaches_observation_replay_and_never_signs() {
    let mut fixture = Fixture::load("native-osaka-lookup");
    let mut native = native_payload(&mut fixture.input);
    let forged = Bytes::from_static(b"forged revert output");
    let mut changed = 0;
    for message in &mut native.semantic_messages {
        if let eez_blob_codec::Message::ReturnFail { return_data } = message {
            *return_data = forged.clone();
            changed += 1;
        }
    }
    assert_eq!(
        changed, 1,
        "the fixture has exactly one caught destination revert"
    );
    let post = header_mut(&mut fixture.input[0])
        .post_batch
        .as_mut()
        .unwrap();
    let mut batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
    assert_eq!(batch.entries.len(), 2);
    assert!(!batch.entries[1].success);
    batch.entries[1].returnData = forged;
    post.abi_calldata = eez_protocol::entries::encode_postbatch(&batch);
    replace_native_payload(&mut fixture.input, native);
    // Both claim-side copies agree and the physical blobs are valid. Only
    // authenticated replay at the FIFO root can refute the fabricated outcome.
    let error = pipeline_error(&fixture.state, fixture.input.clone());
    assert_eq!(error.gate(), "observation_replay", "{error:?}");
    let server = fixture.server().await;
    assert_eq!(
        server.prove(fixture.input).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(fixture.state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn missing_lookup_manager_code_reaches_observation_replay_and_never_signs() {
    let mut fixture = Fixture::load("native-osaka-lookup");
    let (genesis, _, _) = crate::testkit::native_capture("native-osaka-lookup");
    let code = genesis.alloc[&eez_primitives::EEZL2_GENESIS_IMPLEMENTATION_ADDRESS]
        .code
        .as_ref()
        .unwrap();
    let Some(WindowPart::Block(block)) = &mut fixture.input.last_mut().unwrap().kind else {
        panic!("terminal block");
    };
    let codes = &mut block.witness.as_mut().unwrap().codes;
    let count = codes.len();
    codes.retain(|candidate| candidate.as_slice() != code.as_ref());
    assert_eq!(codes.len() + 1, count);
    // Canonical block execution and native semantic binding must pass. This
    // implementation is needed by the subsequent scratch manager delivery.
    let error = pipeline_error(&fixture.state, fixture.input.clone());
    assert_eq!(error.gate(), "observation_replay", "{error:?}");
    assert!(format!("{error:?}").contains("bytecode"), "{error:?}");
    let server = fixture.server().await;
    assert_eq!(
        server.prove(fixture.input).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(fixture.state.attester.signing_calls(), 0);
}

#[test]
#[ignore = "explicit supplemental witness generation from exact captured block replay"]
fn supplement_native_lookup_capture_witness() {
    crate::testkit::history_observations::complete_native_capture_witness("native-osaka-lookup");
}

/// Refresh commitments after an intentional proof-policy change while retaining
/// the captured execution/witness bytes. This constructs expected values from
/// inputs; it never invokes a service or learns an expected digest from one.
#[test]
#[ignore = "explicit native commitment regeneration; see conformance/README.md"]
fn regenerate_native_fixture_commitments() {
    let key = b256!("8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba");
    let signer = eez_protocol::signer::EcdsaProofSigner::from_private_key(key).unwrap();
    let oracle_dir =
        crate::testkit::manifest_dir().join("../../artifacts/rollup-core-fixture-oracle");
    std::fs::create_dir_all(&oracle_dir).unwrap();
    for name in [
        "native-offline-bidirectional",
        "native-osaka-lookup",
        "native-osaka-changing-static",
    ] {
        let directory = crate::testkit::manifest_dir()
            .join("tests/fixtures")
            .join(name);
        let read = |file: &str| std::fs::read(directory.join(file)).unwrap();
        let mut identity: serde_json::Value =
            serde_json::from_slice(&read("identity.json")).unwrap();
        let validator = Validator::stateless(
            &directory.join("l2-genesis.json"),
            eez_primitives::SYSTEM_ADDRESS,
        )
        .unwrap();
        let l1 = reth_chainspec::ChainSpec::from(
            serde_json::from_slice::<alloy_genesis::Genesis>(&read("l1-genesis.json")).unwrap(),
        );
        let profile = eez_l1::chain_config::offline_l1_rules(&l1)
            .unwrap()
            .profile(validator.schedule().unwrap(), validator.chain_id())
            .unwrap();
        let registry = identity["registry_address"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let proof_system = identity["proof_system"].as_str().unwrap().parse().unwrap();
        let vkey = profile.verification_key(registry, proof_system, signer.address());
        identity["vkey"] = serde_json::json!(vkey);
        let state = fixture_state(&directory, &identity);
        let encoded = read("request.v2.protobuf");
        let input = chunks(&encoded);
        let digest = recompute_window_public_inputs_for_state(&state, &input);
        let signature = signer.sign_prehash(digest).unwrap();
        let response = serde_json::json!({
            "public_inputs_hash": hex::encode(digest),
            "signature": hex::encode(&signature),
            "request_bytes": encoded.len(),
            "request_hash": keccak256(&encoded),
        });
        for (file, value) in [("identity.json", &identity), ("response.json", &response)] {
            std::fs::write(
                directory.join(file),
                serde_json::to_vec_pretty(value).unwrap(),
            )
            .unwrap();
        }
        let Some(WindowPart::Metadata(header)) = &input[0].kind else {
            panic!("metadata")
        };
        let post = header.post_batch.as_ref().unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
        let carrier =
            settlement::verify_da_carrier_with_limit(&batch, &post.blobs, header.rollup_id, 16)
                .unwrap();
        let posting = post.posting_context.as_ref().unwrap();
        let oracle = serde_json::json!({
            "calldata": alloy_primitives::hex::encode_prefixed(&post.abi_calldata),
            "blob_hashes": carrier.transaction_blob_hashes(),
            "digest": digest, "signature": signature, "vkey": vkey,
            "profile_preimage": profile.canonical_bytes(), "profile_hash": profile.commitment(),
            "registry": registry, "proof_system": proof_system, "attester": signer.address(),
            "parent_number": posting.parent_number,
            "parent_hash": alloy_primitives::hex::encode_prefixed(&posting.parent_hash),
            "target_timestamp": posting.target_timestamp,
        });
        std::fs::write(
            oracle_dir.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&oracle).unwrap(),
        )
        .unwrap();
    }
}
