//! Real canonical-source and signing boundaries with a synchronized execution
//! stub. Source-based process tests separately cover Stateless EVM execution.

use super::*;
use alloy_consensus::Header;
use eez_protocol::l2::{DerivationConfig, GenesisL1Reference, Schedule};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const REGISTRY: Address = Address::repeat_byte(0x11);

fn fixture() -> (Schedule, Header, Header, Vec<BlockWitness>) {
    let reference = Header {
        number: 10,
        timestamp: 1_710_338_123,
        mix_hash: B256::with_last_byte(7),
        ..Default::default()
    };
    let parent = Header {
        timestamp: reference.timestamp,
        mix_hash: reference.mix_hash,
        ..Default::default()
    };
    let schedule = Schedule::new(
        DerivationConfig {
            block_interval_seconds: 2,
            sync_interval_blocks: 6,
            l1_reference: GenesisL1Reference {
                chain_id: 1,
                number: reference.number,
                hash: reference.hash_slow(),
                timestamp: reference.timestamp,
                prev_randao: reference.mix_hash,
            },
        },
        &parent,
    )
    .unwrap();
    let mut previous = parent.hash_slow();
    let blocks = (1..=6)
        .map(|number| {
            let header = Header {
                number,
                timestamp: parent.timestamp + number * 2,
                parent_hash: previous,
                mix_hash: parent.mix_hash,
                ..Default::default()
            };
            let block = Block::new(header, Default::default());
            let hash = block.header.hash_slow();
            let witness = BlockWitness {
                number,
                hash: hash.to_vec(),
                parent_hash: previous.to_vec(),
                rlp: alloy_rlp::encode(block),
                witness: Some(ExecutionWitness::default()),
            };
            previous = hash;
            witness
        })
        .collect();
    (schedule, reference, parent, blocks)
}

/// Serve only the six reads needed for a genesis context. No latest-head or
/// lifetime log query is accepted, and the listener closes before replay ends.
async fn source(
    reference: &Header,
    registry_root: B256,
) -> (url::Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let mut block = serde_json::to_value(reference).unwrap();
    block["hash"] = json!(reference.hash_slow());
    block["transactions"] = json!([]);
    block["uncles"] = json!([]);
    // Even finality has advanced beyond the requested parent. Its historical
    // context remains valid input, although inclusion may no longer be possible.
    let mut finalized_header = reference.clone();
    finalized_header.parent_hash = reference.hash_slow();
    finalized_header.number += 1;
    finalized_header.timestamp += 12;
    let mut finalized = serde_json::to_value(&finalized_header).unwrap();
    finalized["hash"] = json!(finalized_header.hash_slow());
    finalized["transactions"] = json!([]);
    finalized["uncles"] = json!([]);
    let number = format!("0x{:x}", reference.number);
    let hash = reference.hash_slow();
    let registry = Bytes::from((REGISTRY, registry_root, U256::ZERO).abi_encode());
    let task = tokio::spawn(async move {
        for (method, parameter, result) in [
            ("eth_chainId", None, json!("0x1")),
            ("eth_getBlockByNumber", Some(json!("finalized")), finalized),
            ("eth_getBlockByNumber", Some(json!(number)), block.clone()),
            ("eth_getBlockByNumber", Some(json!(number)), block.clone()),
            ("eth_call", None, json!(registry)),
            ("eth_getBlockByNumber", Some(json!(number)), block),
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut length = None;
            loop {
                let mut line = String::new();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            stream.read_exact(&mut body).await.unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["method"], method);
            if let Some(parameter) = parameter {
                assert_eq!(request["params"][0], parameter);
            }
            if method == "eth_call" {
                assert_eq!(request["params"][0]["to"], json!(REGISTRY));
                assert_eq!(
                    request["params"][1],
                    json!({"blockHash": hash, "requireCanonical": true})
                );
            }
            let response =
                json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string();
            stream.get_mut().write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len(),
            ).as_bytes()).await.unwrap();
            stream.get_mut().shutdown().await.unwrap();
        }
    });
    (url, task)
}

#[tokio::test]
async fn authenticated_snapshot_signs_without_l1_after_replay_but_rejects_wrong_registry_root() {
    for wrong_root in [false, true] {
        let (schedule, reference, parent, blocks) = fixture();
        let (url, source) = source(
            &reference,
            if wrong_root {
                B256::repeat_byte(0x55)
            } else {
                parent.hash_slow()
            },
        )
        .await;
        let inputs = blocks
            .iter()
            .cloned()
            .map(|block| AdmittedBlock::from_wire(block, &mut 0, limits().window_limits()).unwrap())
            .collect::<Vec<_>>();
        let (validator, started, release) =
            Validator::blocking_stub(Ok(backend_output_for(&inputs)));
        let state = Arc::new(
            ServiceState::new(
                crate::service::l1::L1Authority::new(url, REGISTRY, 1, schedule).unwrap(),
                validator,
                expected_rollup_id(1),
                test_attester(),
                crate::testkit::l1_chain_spec(),
            )
            .unwrap(),
        );
        let svc = ProveSvc::new(state.clone(), limits());
        let observer = svc.clone();
        let server = TestServer::with_service(svc).await;
        let mut chunks = request(blocks.clone(), payload(&blocks));
        let header = header_mut(&mut chunks[0]);
        header.parent_provenance.as_mut().unwrap().parent_header_rlp = alloy_rlp::encode(parent);
        header.post_batch.as_mut().unwrap().posting_context =
            Some(eez_control_rpc::v1::L1PostingContext {
                chain_id: 1,
                parent_number: reference.number,
                parent_hash: reference.hash_slow().to_vec(),
                target_timestamp: reference.timestamp + 12,
            });
        server.bind_identity(&mut chunks);
        let expected = recompute_window_public_inputs_hash(&chunks);
        let mut client = server.client().await;
        let request = tokio::spawn(async move { client.prove(tokio_stream::iter(chunks)).await });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        // Authentication is complete and execution is blocked. Closing the RPC
        // here deterministically catches any post-replay network dependency.
        source.await.unwrap();
        assert_eq!(state.attester.signing_calls(), 0);
        assert_eq!(observer.active_request_slot.available_permits(), 0);
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        if wrong_root {
            let status = result.unwrap_err();
            assert_eq!(status.code(), Code::FailedPrecondition);
            // The registry reply is the only difference from the successful
            // case. The service deliberately redacts settlement gate details.
            assert_eq!(status.message(), "settlement validation rejected");
            assert!(status.details().is_empty());
            assert_eq!(state.attester.signing_calls(), 0);
        } else {
            assert_attestation(
                &result.unwrap().into_inner(),
                expected,
                state.attester.address(),
            );
            assert_eq!(state.attester.signing_calls(), 1);
        }
        assert_eq!(observer.active_request_slot.available_permits(), 1);
    }
}
