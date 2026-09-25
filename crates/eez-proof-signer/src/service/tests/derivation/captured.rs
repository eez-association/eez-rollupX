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

    let fixture = Fixture::load("derivable-native-mixed");
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
                "sidecars": payload.l2_entries().len(), "groups": payload.outbound_group_sizes(),
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
                max_window_blocks: nz(64),
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
                        "chain-config.json" => include_bytes!(concat!(
                            "../../../../tests/fixtures/",
                            $name,
                            "/chain-config.json"
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
            "derivable-native-mixed" => fixture!("derivable-native-mixed"),
            "derivable-native-lookup" => fixture!("derivable-native-lookup"),
            "derivable-native-changing-static" => fixture!("derivable-native-changing-static"),
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
        let system_key = settlement::SystemTransactionKey::new(
            b256!("6f7d72ecb79c8bf1bd8e7c49a1c4a22741ab708f06bb19e5b5d44a6f0934a7c1"),
            system_address,
        )
        .unwrap();
        assert_eq!(identity["gnosis_l1"], false);
        // The same intentionally public Anvil account #5 used by Testkit.
        let attester = crate::attest::Attester::new(
            b256!("8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"),
            identity["vkey"].as_str().unwrap().parse().unwrap(),
            identity["proof_system"].as_str().unwrap().parse().unwrap(),
            system_address,
        )
        .unwrap();
        let state = Arc::new(
            ServiceState::new(
                Validator::stateless_for_test(
                    serde_json::from_slice(read("chain-config.json")).unwrap(),
                    system_address,
                ),
                expected_rollup_id(identity["rollup_id"].as_u64().unwrap()),
                attester,
                system_key,
                Arc::new(reth_chainspec::ChainSpec::from(
                    serde_json::from_slice::<alloy_genesis::Genesis>(read("l1-genesis.json"))
                        .unwrap(),
                )),
            )
            .unwrap(),
        );
        let input = if name == "derivable-native-changing-static" {
            // The original request used cached ranges. These are the exact
            // preparation witnesses expanded for independent full execution;
            // no cache facts from the capturing process are imported.
            let cold = include_bytes!(
                "../../../../tests/fixtures/derivable-native-changing-static/cold-request.protobuf"
            );
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
        } else {
            chunks(encoded)
        };
        Self {
            input,
            state,
            expected_hash: B256::from_slice(
                &hex::decode(response["public_inputs_hash"].as_str().unwrap()).unwrap(),
            ),
            signature: hex::decode(response["signature"].as_str().unwrap()).unwrap(),
        }
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
            blocks: 64,
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
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        &CancellationToken::default(),
    )
    .err()
    .expect("mutated request must fail the full pipeline")
}

#[tokio::test]
async fn previous_protocol_system_selectors_are_rejected_without_signing() {
    // Preserve the original bytes: this is a compatibility boundary, not a
    // regenerated fixture that hides a changed execution contract.
    let fixture = Fixture::load("derivable-native-mixed");
    let error = pipeline_error(&fixture.state, fixture.input.clone());
    assert!(matches!(
        error,
        PipelineError::Settlement(SettlementPipelineError::InboundEffects(
            settlement::InboundEffectError::MissingCandidate {
                entry_index: 2,
                transaction_index: 2
            }
        ))
    ));
    let server = fixture.server().await;
    assert_eq!(
        server.prove(fixture.input).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(fixture.state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn native_changing_reads_and_caught_revert_reproduce_original_attestations() {
    for name in [
        "derivable-native-changing-static",
        "derivable-native-lookup",
    ] {
        let mut fixture = Fixture::load(name);
        let native = native_payload(&mut fixture.input);
        let payload = Payload::decode(&native.operations).unwrap();
        assert!((1..=64).contains(&payload.block_count()));
        let post = header_mut(&mut fixture.input[0])
            .post_batch
            .as_ref()
            .unwrap();
        let batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone()).unwrap();
        if name.ends_with("changing-static") {
            assert_eq!(payload.block_count(), 42);
            assert_eq!(batch.entries.len(), 2);
            assert_eq!(payload.l2_entries().len(), 5);
            assert_eq!(payload.outbound_group_sizes(), [5]);
            assert_eq!(batch.entries[1].l2ToL1Calls.len(), 5);
            let read_hash =
                eez_protocol::abi::L2StaticExecutionEntrySol::abi_decode(&payload.l2_entries()[1])
                    .unwrap()
                    .proxyEntryHash;
            for (row, (position, value)) in
                payload.l2_entries()[1..]
                    .iter()
                    .zip([(0, 5), (0, 5), (1, 79), (1, 79)])
            {
                let entry = eez_protocol::abi::L2StaticExecutionEntrySol::abi_decode(row)
                    .expect("native static row");
                assert_eq!(entry.abi_encode(), *row);
                assert_eq!(entry.expectedEntryIndex, U256::from(position));
                assert_eq!(entry.proxyEntryHash, read_hash);
                assert!(entry.success);
                assert_eq!(
                    entry.returnData.as_ref(),
                    U256::from(value).to_be_bytes::<32>()
                );
            }
        } else {
            assert_eq!(payload.block_count(), 5);
            assert_eq!(batch.entries.len(), 2);
            assert!(!batch.entries[1].success);
            assert!(payload.l2_entries().is_empty());
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
async fn native_mixed_sidecars_and_group_boundaries_cannot_be_changed_before_signing() {
    let mut fixture = Fixture::load("derivable-native-changing-static");
    let original = native_payload(&mut fixture.input);
    let p = Payload::decode(&original.operations).unwrap();
    let server = fixture.server().await;
    for mutation in 0..8 {
        let mut entries = p.l2_entries().to_vec();
        let mut groups = p.outbound_group_sizes().to_vec();
        match mutation {
            0 => {
                entries.pop();
            }
            1 => {
                entries.push(entries[0].clone());
            }
            2 => entries.swap(0, 1),
            3 => {
                let last = entries[0].len() - 1;
                entries[0][last] ^= 1;
            }
            4 => groups.clear(),
            5 => groups[0] += 1,
            6 | 7 => {
                let mut after_write =
                    eez_protocol::abi::L2StaticExecutionEntrySol::abi_decode(&entries[3]).unwrap();
                assert_eq!(after_write.expectedEntryIndex, U256::from(1));
                after_write.expectedEntryIndex = if mutation == 6 { U256::ZERO } else { U256::MAX };
                entries[3] = after_write.abi_encode();
            }
            _ => unreachable!(),
        }
        let changed = Payload::new(
            p.ordinary_block_count() as u64,
            p.initial_environment().clone(),
            p.records().to_vec(),
            p.terminal_block().to_vec(),
            entries,
            groups,
        )
        .unwrap();
        let mut input = fixture.input.clone();
        let mut native = original.clone();
        native.operations = changed.encode().unwrap().into();
        replace_native_payload(&mut input, native);
        // Repacking produces a valid carrier; the specific authorization gate
        // must reject the changed sidecars/grouping after actual execution.
        let error = pipeline_error(&fixture.state, input.clone());
        assert_eq!(error.gate(), "da_payload", "mutation {mutation}: {error:?}");
        assert_eq!(server.prove(input).await.code(), Code::FailedPrecondition);
        assert_eq!(fixture.state.attester.signing_calls(), 0);
    }
}

#[tokio::test]
async fn forged_native_lookup_outcome_reaches_observation_replay_and_never_signs() {
    let mut fixture = Fixture::load("derivable-native-lookup");
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
