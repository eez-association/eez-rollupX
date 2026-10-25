//! The history/ordinary prefix must bind the observed result before signing.

use super::*;
use crate::testkit::history_observations::{Fixture, PROBE};

fn state(fixture: &Fixture) -> Arc<ServiceState> {
    Arc::new(
        ServiceState::new(
            Address::repeat_byte(0x33),
            Address::repeat_byte(0x44),
            Validator::stateless_for_test(fixture.genesis.clone(), TEST_SYSTEM_ADDRESS),
            expected_rollup_id(1),
            test_attester(),
            crate::testkit::history::l1_spec(),
        )
        .unwrap(),
    )
}

fn observation_request(
    state: &ServiceState,
    fixture: &Fixture,
    mode: CallMode,
    output: Bytes,
) -> Vec<WindowChunk> {
    let operations = payload(&fixture.blocks);
    let mut chunks = request(state, fixture.blocks.clone(), operations.clone());
    let mut action = fixture_action(
        RollupId::MAINNET,
        RollupId(1),
        Address::with_last_byte(0x50),
        PROBE,
        U256::ZERO,
        Bytes::new(),
        output,
        true,
    );
    action.call_mode = mode;
    // A source-frame revert preserves the lookup result without applying it
    // to L2, so both modes observe the same complete ordinary prefix.
    action.revert_span = (mode == CallMode::Mutable).then_some(1);
    let semantic = SemanticTransaction {
        origin_chain: RollupId::MAINNET,
        tx_data: Bytes::new(),
        actions: vec![action],
    };
    let mut source = semantic.materialize().unwrap().source;
    let root = B256::from_slice(&fixture.blocks.last().unwrap().hash);
    for entry in &mut source.batch.entries {
        entry.rootUpdates = vec![RootUpdateSol {
            rollupId: 1,
            currentRoot: root,
            newRoot: root,
            etherDelta: EtherDeltaSol::ZERO,
        }];
    }
    for entry in &mut source.batch.staticEntries {
        entry.expectedRoots =
            vec![eez_protocol::abi::ExpectedRootPerRollupSol { rollupId: 1, root }];
    }
    eez_protocol::entries::finalize_l1_entry_traces(&mut source.batch, &source.entry_traces)
        .unwrap();
    let post = header_mut(&mut chunks[0]).post_batch.as_mut().unwrap();
    let mut batch = settlement::decode_canonical_post_batch(post.abi_calldata.clone())
        .unwrap()
        .as_batch()
        .clone();
    batch.entries.extend(source.batch.entries);
    batch.staticEntries.extend(source.batch.staticEntries);
    batch.callData = operations.into();
    let context = post.posting_context.clone();
    *post = public_input_post_batch_for_semantics(batch, &[semantic]);
    post.posting_context = context;
    chunks
}

#[tokio::test]
async fn scheduled_history_correct_observations_reach_the_signing_gate() {
    let fixture = Fixture::new(false);
    for mode in [CallMode::Static, CallMode::Mutable] {
        let state = state(&fixture);
        let chunks = observation_request(&state, &fixture, mode, fixture.outcome(7, 1));
        let expected = recompute_window_public_inputs_for_state(&state, &chunks);
        let server = TestServer::new(state.clone()).await;
        let response = server.attest(chunks).await;
        assert_attestation(&response, expected, state.attester.address());
        assert_eq!(state.attester.signing_calls(), 1);
    }
}

#[tokio::test]
async fn scheduled_history_earlier_state_observations_never_sign() {
    let fixture = Fixture::new(false);
    for mode in [CallMode::Static, CallMode::Mutable] {
        let state = state(&fixture);
        let server = TestServer::new(state.clone()).await;
        for origin in [0, 1] {
            let chunks = observation_request(&state, &fixture, mode, fixture.outcome(0, origin));
            let response = server.exchange(chunks.clone()).await;
            if let Ok(proof) = &response {
                assert_attestation(
                    proof,
                    recompute_window_public_inputs_for_state(&state, &chunks),
                    state.attester.address(),
                );
            }
            assert_eq!(
                state.attester.signing_calls(),
                0,
                "mode={mode:?}, origin={origin}"
            );
            assert_eq!(response.unwrap_err().code(), Code::FailedPrecondition);
            let error = captured::pipeline_error(&state, chunks.clone());
            assert!(
                matches!(
                    error,
                    PipelineError::Settlement(SettlementPipelineError::ObservationReplay(
                        crate::validate::ObservationValidationError::OutcomeMismatch { index: 0 }
                    ))
                ),
                "mode={mode:?}, origin={origin}: {error:?}"
            );
            assert_eq!(error.gate(), "observation_replay");
            assert_eq!(state.attester.signing_calls(), 0);
        }
    }
}

fn process_fixtures(write: bool) {
    use prost::Message as _;
    let fixture = Fixture::new(false);
    let state = state(&fixture);
    let directory =
        crate::testkit::manifest_dir().join("tests/fixtures/native-history-observations");
    if write {
        std::fs::create_dir_all(&directory).unwrap();
    }
    let retain = |name: &str, bytes: Vec<u8>| {
        let path = directory.join(name);
        if write {
            std::fs::write(path, bytes).unwrap();
        } else {
            assert_eq!(std::fs::read(path).unwrap(), bytes, "fixture drift: {name}");
        }
    };
    retain(
        "l2-genesis.json",
        serde_json::to_vec_pretty(&fixture.genesis).unwrap(),
    );
    retain(
        "l1-genesis.json",
        serde_json::to_vec_pretty(&crate::testkit::history::l1_spec().genesis).unwrap(),
    );
    let mut cases = Vec::new();
    for (label, mode) in [("static", CallMode::Static), ("lookup", CallMode::Mutable)] {
        for (suffix, balance, origin) in [
            ("before-history", 0, 0),
            ("before-ordinary", 0, 1),
            ("correct", 7, 1),
        ] {
            let chunks =
                observation_request(&state, &fixture, mode, fixture.outcome(balance, origin));
            let mut encoded = Vec::new();
            for (index, kind) in request_frames(&chunks).into_iter().enumerate() {
                eez_control_rpc::v2::ClientFrame {
                    request_id: index as u64 + 1,
                    kind: Some(kind),
                    ..Default::default()
                }
                .encode_length_delimited(&mut encoded)
                .unwrap();
            }
            let name = format!("{label}-{suffix}.v2.protobuf");
            cases.push(serde_json::json!({
                "request": name,
                "request_hash": keccak256(&encoded),
                "public_inputs_hash": recompute_window_public_inputs_for_state(&state, &chunks),
                "accept": balance == 7,
            }));
            retain(&name, encoded);
        }
    }
    retain(
        "identity.json",
        serde_json::to_vec_pretty(&serde_json::json!({
            "validation_identity": state.validation_identity,
            "attester": state.attester.address(),
            "registry": Address::repeat_byte(0x33),
            "manager": Address::repeat_byte(0x44),
            "proof_system": test_proof_system(),
            "vkey": B256::repeat_byte(0x42),
            "rollup_id": 1,
            "cases": cases,
        }))
        .unwrap(),
    );
}

#[test]
fn history_observation_process_fixtures_match_the_regressions() {
    process_fixtures(false);
}

#[test]
#[ignore = "explicit regeneration utility for the standalone signer process regression"]
fn regenerate_history_observation_process_fixtures() {
    process_fixtures(true);
}
