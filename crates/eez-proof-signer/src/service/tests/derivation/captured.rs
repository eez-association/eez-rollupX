//! Replay and mutate complete proving requests captured from native devnet tests.

use super::*;
use prost::Message as _;

fn chunks(bytes: &[u8]) -> Vec<ProveChunk> {
    let mut remaining = bytes;
    let mut chunks = Vec::new();
    while !remaining.is_empty() {
        chunks.push(ProveChunk::decode_length_delimited(&mut remaining).unwrap());
    }
    chunks
}

#[tokio::test]
async fn preparation_rejects_a_real_system_and_outbound_terminal_without_signing() {
    use eez_control_rpc::v1::{PrepareChunk, PrepareHeader, prepare_chunk};

    let fixture = Fixture::load("native-osaka-changing-static");
    let state = Arc::clone(&fixture.state);
    let server = TestServer::new(Arc::clone(&state)).await;
    let Some(prove_chunk::Kind::Block(terminal)) = fixture.input.last().unwrap().kind.clone()
    else {
        panic!("captured terminal witness")
    };
    let chunks = [
        PrepareChunk {
            kind: Some(prepare_chunk::Kind::Header(PrepareHeader {
                rollup_id: 1,
                expected_validation_identity: state.validation_identity.to_vec(),
                from_block: terminal.number,
                to_block: terminal.number,
                lease_id: Vec::new(),
            })),
        },
        PrepareChunk {
            kind: Some(prepare_chunk::Kind::Block(terminal)),
        },
    ];
    let status = server
        .client()
        .await
        .prepare_ordinary(tokio_stream::iter(chunks))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(state.attester.signing_calls(), 0);
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
        let Some(prove_chunk::Kind::Header(header)) = &input[0].kind else {
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

struct Fixture {
    input: Vec<ProveChunk>,
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
                max_window_blocks: nz(128),
                max_window_bytes: nz(16 * 1024 * 1024),
                max_window_witness_items: nz(100_000),
                stream_idle_timeout: Duration::from_secs(5),
                request_timeout: Duration::from_secs(30),
            })
            .unwrap(),
        )
        .await
    }

    fn load(name: &str) -> Self {
        macro_rules! fixture {
            ($name:literal) => {{
                |file| -> &[u8] {
                    match file {
                        "identity.json" => include_bytes!(concat!(
                            "../../../../tests/fixtures/",
                            $name,
                            "/identity.json"
                        )),
                        "response.json" => include_bytes!(concat!(
                            "../../../../tests/fixtures/",
                            $name,
                            "/response.json"
                        )),
                        "request.protobuf" => include_bytes!(concat!(
                            "../../../../tests/fixtures/",
                            $name,
                            "/request.protobuf"
                        )),
                        "l2-genesis.json" => include_bytes!(concat!(
                            "../../../../tests/fixtures/",
                            $name,
                            "/l2-genesis.json"
                        )),
                        "cold-request.protobuf" => include_bytes!(concat!(
                            "../../../../tests/fixtures/",
                            $name,
                            "/cold-request.protobuf"
                        )),
                        "l1-genesis.json" => include_bytes!(concat!(
                            "../../../../tests/fixtures/",
                            $name,
                            "/l1-genesis.json"
                        )),
                        _ => panic!("unknown fixture file"),
                    }
                }
            }};
        }
        let read: fn(&str) -> &[u8] = match name {
            "native-osaka-lookup" => fixture!("native-osaka-lookup"),
            "native-osaka-changing-static" => fixture!("native-osaka-changing-static"),
            _ => panic!("unknown fixture"),
        };
        let identity: serde_json::Value = serde_json::from_slice(read("identity.json")).unwrap();
        let response: serde_json::Value = serde_json::from_slice(read("response.json")).unwrap();
        let encoded = read("request.protobuf");
        assert_eq!(
            keccak256(encoded),
            response["request_hash"]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap()
        );
        assert_eq!(
            encoded.len() as u64,
            response["request_bytes"].as_u64().unwrap()
        );
        let system_address = identity["l2_system_address"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap();
        assert_eq!(
            system_address,
            eez_primitives::SYSTEM_ADDRESS,
            "fixture must be captured with the native protocol"
        );
        assert_eq!(identity["gnosis_l1"], false);
        // The same intentionally public Anvil account #5 used by Testkit.
        let attester = crate::attest::Attester::new(
            b256!("8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"),
            identity["vkey"].as_str().unwrap().parse().unwrap(),
            identity["proof_system"].as_str().unwrap().parse().unwrap(),
            system_address,
        )
        .unwrap();
        let genesis = crate::testkit::manifest_dir()
            .join("tests/fixtures")
            .join(name)
            .join("l2-genesis.json");
        assert_eq!(std::fs::read(&genesis).unwrap(), read("l2-genesis.json"));
        let state = Arc::new(
            ServiceState::new(
                crate::service::l1::L1Authority::Synthetic,
                Validator::stateless(&genesis, system_address).unwrap(),
                expected_rollup_id(identity["rollup_id"].as_u64().unwrap()),
                attester,
                Arc::new(reth_chainspec::ChainSpec::from(
                    serde_json::from_slice::<alloy_genesis::Genesis>(read("l1-genesis.json"))
                        .unwrap(),
                )),
            )
            .unwrap(),
        );
        let input = {
            // The original request used cached ranges. These are the exact
            // preparation witnesses expanded for independent full execution;
            // no cache facts from the capturing process are imported.
            let cold = read("cold-request.protobuf");
            assert_eq!(
                cold.len() as u64,
                response["cold_request_bytes"].as_u64().unwrap()
            );
            assert_eq!(
                keccak256(cold),
                response["cold_request_hash"]
                    .as_str()
                    .unwrap()
                    .parse::<B256>()
                    .unwrap()
            );
            let cold = chunks(cold);
            assert_eq!(cold.first(), chunks(encoded).first());
            assert_eq!(cold.last(), chunks(encoded).last());
            assert!(
                cold[1..]
                    .iter()
                    .all(|chunk| matches!(chunk.kind, Some(prove_chunk::Kind::Block(_))))
            );
            cold
        };
        let mut fixture = Self {
            input,
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
            eez_protocol::RollupId(1),
            fixture.state.validator.chain_id(),
        )
        .unwrap();
        let batch = settlement::decode_canonical_post_batch(
            header_mut(&mut fixture.input[0])
                .post_batch
                .as_ref()
                .unwrap()
                .abi_calldata
                .clone(),
        )
        .unwrap();
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

    fn digest(&mut self) -> B256 {
        let header = header_mut(&mut self.input[0]);
        let post = header.post_batch.as_ref().unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
        let carrier =
            settlement::verify_da_carrier_with_limit(&batch, &post.blobs, header.rollup_id, 16)
                .unwrap();
        settlement::CheckedPublicInputProfile::new(
            &batch,
            self.state.expected_rollup_id,
            batch.proofSystems[0],
        )
        .unwrap()
        .recompute_public_inputs_hash(
            self.state.attester.proof_system_vkey(),
            carrier.transaction_blob_hashes(),
            crate::testkit::target_from_wire(post.posting_context.as_ref().unwrap()),
        )
        .unwrap()
        .into_inner()
    }
}

fn native_payload(input: &mut [ProveChunk]) -> eez_blob_codec::NativePayload {
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

fn replace_native_payload(input: &mut [ProveChunk], native: eez_blob_codec::NativePayload) {
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

fn pipeline_error(state: &ServiceState, input: Vec<ProveChunk>) -> PipelineError {
    let mut input = input.into_iter();
    let mut assembler = crate::window::WindowAssembler::start(
        crate::window::WindowLimits {
            blocks: 128,
            payload_bytes: 16 * 1024 * 1024,
            witness_items: 100_000,
        },
        input.next().unwrap(),
    )
    .unwrap()
    .verify_rollup_identity(state.expected_rollup_id)
    .unwrap();
    for chunk in input {
        assembler.push(chunk).unwrap();
    }
    let (header, blocks) = assembler.finish().unwrap().into_parts();
    let maximum_blobs = state.posting_blob_limit(&header.posting_context).unwrap();
    validate_and_settle(
        state,
        blocks,
        header.submitted_post_batch_calldata,
        header.submitted_blobs,
        maximum_blobs,
        header.posting_context,
        crate::service::l1::CandidateContext::Synthetic,
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        &CancellationToken::default(),
    )
    .err()
    .expect("mutated request must fail the full pipeline")
}

#[test]
fn previous_signed_system_deployment_is_rejected_before_replay() {
    let identity: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/derivable-native-mixed/identity.json"
    ))
    .unwrap();
    let system_address = identity["l2_system_address"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(system_address, eez_primitives::SYSTEM_ADDRESS);
    let config = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/derivable-native-mixed/chain-config.json"
    ))
    .unwrap();
    let error = ServiceState::new(
        crate::service::l1::L1Authority::Synthetic,
        Validator::stateless_for_test(
            alloy_genesis::Genesis {
                config,
                ..Default::default()
            },
            system_address,
        ),
        expected_rollup_id(identity["rollup_id"].as_u64().unwrap()),
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
async fn native_changing_reads_and_caught_revert_replay_original_execution_with_v0() {
    for name in ["native-osaka-changing-static", "native-osaka-lookup"] {
        let mut fixture = Fixture::load(name);
        let native = native_payload(&mut fixture.input);
        let payload = Payload::decode(&native.operations).unwrap();
        assert!((1..=128).contains(&payload.block_count()));
        let post = header_mut(&mut fixture.input[0])
            .post_batch
            .as_ref()
            .unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
        let semantics =
            eez_protocol::blob_semantics::decode_semantic_transactions(&native.semantic_messages)
                .unwrap();
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
        let response = server.attest(fixture.input.clone()).await;
        assert_attestation(
            &response,
            fixture.expected_hash,
            fixture.state.attester.address(),
        );
        assert_eq!(response.signature, fixture.signature);
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
