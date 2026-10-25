//! Offline process regression for observations after a scheduled history import.

use alloy_primitives::{Address, B256, Signature, keccak256};
use eez_control_rpc::v2::{self, client_frame, server_frame};
use eez_testkit::{ANVIL_KEY_1, ProofSignerConfig, ProofSignerHandle};
use prost::Message as _;
use std::{path::PathBuf, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_history_observations_are_bound_before_offline_signing() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../eez-proof-signer/tests/fixtures/native-history-observations");
    let identity: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("identity.json")).unwrap()).unwrap();
    let address = |field: &str| {
        identity[field]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap()
    };
    let expected_identity: B256 = identity["validation_identity"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let signer = ProofSignerHandle::spawn_network_denied(&ProofSignerConfig {
        rollup_manager_address: address("manager"),
        registry_address: address("registry"),
        chain_config: &directory.join("l2-genesis.json"),
        l1_chain_config: directory.join("l1-genesis.json").to_str().unwrap().into(),
        rollup_id: 1,
        signer_key: ANVIL_KEY_1,
        vkey: identity["vkey"].as_str().unwrap().parse().unwrap(),
        proof_system: address("proof_system"),
        max_request_blocks: Some(16),
    })
    .await
    .unwrap();
    let mut client = v2::prover_client::ProverClient::connect(signer.endpoint().to_owned())
        .await
        .unwrap()
        .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES)
        .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
    let capabilities = client
        .capabilities(v2::CapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(capabilities.profile_id, eez_control_rpc::OFFLINE_PROFILE_ID);
    assert_eq!(
        capabilities.validation_identity,
        expected_identity.as_slice()
    );
    let mut proofs = 0;
    let mut rejections = 0;
    for case in identity["cases"].as_array().unwrap() {
        let name = case["request"].as_str().unwrap();
        let encoded = std::fs::read(directory.join(name)).unwrap();
        assert_eq!(
            keccak256(&encoded),
            case["request_hash"]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap()
        );
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut request =
            tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(receiver));
        request.metadata_mut().insert_bin(
            "eez-validation-identity-bin",
            tonic::metadata::MetadataValue::from_bytes(expected_identity.as_slice()),
        );
        let mut responses = client.prove_stream(request).await.unwrap().into_inner();
        let mut remaining = encoded.as_slice();
        let mut session_id = Vec::new();
        let mut epoch = 0;
        let mut validated = 0;
        let mut finalized = false;
        while !remaining.is_empty() {
            let mut frame = v2::ClientFrame::decode_length_delimited(&mut remaining).unwrap();
            assert!(!finalized, "frames after Finalize: {name}");
            frame.session_id = session_id.clone();
            frame.epoch = epoch;
            let request_id = frame.request_id;
            let kind = frame.kind.clone().unwrap();
            sender.send(frame).await.unwrap();
            let response = tokio::time::timeout(Duration::from_secs(30), responses.message())
                .await
                .unwrap()
                .unwrap()
                .expect("one response per input frame");
            assert_eq!(response.request_id, request_id);
            match (kind, response.kind.unwrap()) {
                (client_frame::Kind::Begin(begin), server_frame::Kind::Ready(ready)) => {
                    assert_eq!(ready.validated_through, begin.anchor_number);
                    assert_eq!(ready.validated_hash, begin.anchor_hash);
                    session_id = response.session_id;
                    epoch = response.epoch;
                }
                (client_frame::Kind::Block(block), server_frame::Kind::Validated(ack)) => {
                    let block = block.data.unwrap();
                    assert_eq!(ack.number, block.number);
                    assert_eq!(ack.hash, block.hash);
                    validated += 1;
                }
                (client_frame::Kind::Finalize(_), server_frame::Kind::Proof(proof)) => {
                    let digest: B256 = case["public_inputs_hash"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap();
                    assert_eq!(proof.public_inputs_hash, digest.as_slice());
                    assert_eq!(
                        Signature::try_from(proof.proof.as_slice())
                            .unwrap()
                            .recover_address_from_prehash(&digest)
                            .unwrap(),
                        address("attester")
                    );
                    assert!(
                        case["accept"].as_bool().unwrap(),
                        "earlier state was signed: {name}"
                    );
                    proofs += 1;
                    finalized = true;
                }
                (client_frame::Kind::Finalize(_), server_frame::Kind::Rejected(rejection)) => {
                    assert!(
                        !case["accept"].as_bool().unwrap(),
                        "correct state rejected: {name}"
                    );
                    assert_eq!(rejection.code, i32::from(tonic::Code::FailedPrecondition));
                    rejections += 1;
                    finalized = true;
                }
                other => panic!("{name}: unexpected request/response {other:?}"),
            }
        }
        assert_eq!(
            validated, 6,
            "must replay every block before Finalize: {name}"
        );
        assert!(finalized);
        assert_eq!(signer.streaming_attestations().unwrap(), proofs);
        drop(sender);
    }
    assert_eq!((proofs, rejections), (2, 4));
    assert_eq!(
        signer
            .log_tail(1_000)
            .lines()
            .filter(|line| line.contains("offline finalization rejected")
                && line.contains("observation_replay"))
            .count(),
        4,
        "all rejections must reach observation replay"
    );
    signer.assert_alive();
}
