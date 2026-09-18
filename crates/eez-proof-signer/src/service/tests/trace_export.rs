//! Real RemoteProver/tonic transport, prepared ordinary facts and fresh terminal
//! execution. The captured Stateless fixture exercises every new RPC trace edge.
use super::*;
use alloy_sol_types::SolCall;
use eez_prover::Prover as _;
use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest, common::v1::any_value::Value,
};
use prost::Message;
use tracing::{Instrument, instrument::WithSubscriber};
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone)]
struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn remote_prover_and_tonic_signer_export_connected_spans_without_changing_attestation() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let receiver = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let collector = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut captured: Vec<(String, opentelemetry_proto::tonic::trace::v1::Span)> = Vec::new();
        let expected = [
            "composer_proving",
            "prover_client",
            "prove",
            "prover_capabilities",
            "capabilities",
            "prover_prepare_ordinary",
            "prepare_ordinary",
            "prover_retain_ordinary",
            "retain_ordinary_ranges",
        ];
        while !expected
            .iter()
            .all(|name| captured.iter().any(|(_, span)| span.name == *name))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "missing connected trace spans"
            );
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("collector accept: {error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                assert!(header.len() < 8192);
            }
            let header = String::from_utf8(header).unwrap();
            assert!(header.starts_with("POST /v1/traces HTTP/1.1"));
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|value| value.parse::<usize>().unwrap())
                })
                .unwrap();
            assert!(length < 65536);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let request = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
            for resource in request.resource_spans {
                let service = resource
                    .resource
                    .unwrap()
                    .attributes
                    .into_iter()
                    .find(|attribute| attribute.key == "service.name")
                    .unwrap()
                    .value
                    .unwrap()
                    .value
                    .unwrap();
                let Value::StringValue(service) = service else {
                    panic!("service string");
                };
                for scope in resource.scope_spans {
                    for span in scope.spans {
                        captured.push((service.clone(), span));
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        }
        captured
    });
    let config = eez_telemetry::traces::Config {
        receiver,
        allow_non_loopback: false,
        sample_ratio: 1.,
        queue_capacity: 16,
        export_timeout_ms: 1000,
    };
    let identity = |service: &str| eez_telemetry::schema::Identity {
        environment: "test".into(),
        deployment: "rpc".into(),
        service: service.into(),
        instance: "one".into(),
    };
    let (server_guard, server_layer) =
        eez_telemetry::traces::Guard::start(&identity("eez-proof-signer"), Some(&config)).unwrap();
    let (client_guard, client_layer) =
        eez_telemetry::traces::Guard::start(&identity("eez-composer"), Some(&config)).unwrap();
    // The test runtime is deliberately one thread; tonic's spawned request tasks
    // inherit this thread's server subscriber. Only the client future overrides it.
    let logs = LogCapture(Arc::new(std::sync::Mutex::new(Vec::new())));
    let writer = logs.clone();
    let _server_dispatch = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(server_layer).with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(move || writer.clone()),
        ),
    );
    // RemoteProver's vkey is the attester address. The generic signer fixture
    // uses an unrelated test vkey, so configure this actual client/server pair
    // with the same registered proof identity instead of weakening verification.
    let attester = test_attester_for(
        crate::attest::NonZeroProofSystemVkey::new(test_attester().address().into_word()).unwrap(),
        test_proof_system(),
    );
    let state = ServiceState::new(
        Validator::stateless_for_test(super::derivation::chain_config(), TEST_SYSTEM_ADDRESS),
        expected_rollup_id(1),
        attester,
        test_system_transaction_key(),
        crate::testkit::l1_chain_spec(),
    )
    .unwrap();
    let server = TestServer::new(Arc::new(state)).await;
    let prover = eez_prover_client::RemoteProver::new(
        server.endpoint.clone(),
        test_attester().address(),
        server.state.validation_identity,
    );
    let blocks = super::derivation::blocks(3);
    let payload =
        super::derivation::payload(&blocks, super::derivation::environment(&blocks), vec![]);
    let mut window = super::derivation::request(blocks, payload.encode().unwrap()).into_iter();
    let Some(prove_chunk::Kind::Header(header)) = window.next().unwrap().kind else {
        panic!("header");
    };
    let post = header.post_batch.unwrap();
    let context = eez_prover::ProvingContext {
        rollup_id: header.rollup_id,
        from_block: header.from_block,
        to_block: header.to_block,
        batch: eez_protocol::abi::postAndVerifyBatchCall::abi_decode(&post.abi_calldata)
            .unwrap()
            .batch,
        blobs: post
            .blobs
            .iter()
            .enumerate()
            .map(|(index, bytes)| eez_blob_codec::Blob::try_from_slice(index, bytes).unwrap())
            .collect(),
        blocks: window
            .map(|chunk| {
                let Some(prove_chunk::Kind::Block(block)) = chunk.kind else {
                    panic!("block");
                };
                let witness = block.witness.unwrap();
                eez_prover::BlockWitness {
                    number: block.number,
                    hash: B256::from_slice(&block.hash),
                    parent_hash: B256::from_slice(&block.parent_hash),
                    rlp: block.rlp.into(),
                    witness: alloy_rpc_types_debug::ExecutionWitness {
                        state: witness.state.into_iter().map(Bytes::from).collect(),
                        codes: witness.codes.into_iter().map(Bytes::from).collect(),
                        keys: witness.keys.into_iter().map(Bytes::from).collect(),
                        headers: witness.headers.into_iter().map(Bytes::from).collect(),
                    },
                }
            })
            .collect(),
        l1_block_hash: None,
        l1_posting: {
            let posting = post.posting_context.unwrap();
            eez_prover::L1PostingContext {
                chain_id: posting.chain_id,
                target_timestamp: posting.target_timestamp,
            }
        },
    };
    let signature = async {
        let span = tracing::info_span!(target: "eez_trace", "composer_proving");
        prover.prove(context).instrument(span).await.unwrap()
    }
    .with_subscriber(tracing_subscriber::registry().with(client_layer))
    .await;
    assert_eq!(signature.len(), 65);
    assert!(client_guard.shutdown(Duration::from_secs(8)));
    assert!(server_guard.shutdown(Duration::from_secs(8)));
    let captured = collector.join().unwrap();
    let find = |name: &str| captured.iter().find(|(_, span)| span.name == name).unwrap();
    let (composer_service, composer) = find("composer_proving");
    let (client_service, client) = find("prover_client");
    let (signer_service, signer) = find("prove");
    assert_eq!(composer_service, "eez-composer");
    assert_eq!(client_service, composer_service);
    assert_eq!(signer_service, "eez-proof-signer");
    assert_eq!(client.parent_span_id, composer.span_id);
    assert_eq!(signer.parent_span_id, client.span_id);
    assert_eq!(signer.trace_id, composer.trace_id);
    assert_ne!(signer.span_id, client.span_id);
    for (client_name, server_name) in [
        ("prover_capabilities", "capabilities"),
        ("prover_prepare_ordinary", "prepare_ordinary"),
        ("prover_retain_ordinary", "retain_ordinary_ranges"),
    ] {
        let (_, caller) = find(client_name);
        let (service, server) = find(server_name);
        assert_eq!(service, "eez-proof-signer");
        assert_eq!(server.parent_span_id, caller.span_id);
        assert_eq!(server.trace_id, composer.trace_id);
    }
    assert!(captured.iter().all(|(_, span)| span.events.is_empty()));
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains(&format!(
            "\"trace_id\":\"{}\"",
            hex::encode(&signer.trace_id)
        )),
        "{logs}"
    );
    assert!(logs.contains("window validated and signed"));
}
