use super::*;
use alloy_provider::ProviderBuilder;
use alloy_transport::mock::Asserter;
use eez_protocol::l2::{DerivationConfig, GenesisL1Reference};

pub(super) fn block(number: u64, timestamp: u64, seed: u8) -> alloy_rpc_types_eth::Block {
    let mut block: alloy_rpc_types_eth::Block = Default::default();
    block.header.inner = Header {
        number,
        timestamp,
        mix_hash: B256::with_last_byte(seed),
        ..Default::default()
    };
    block.header.hash = block.header.inner.hash_slow();
    block
}

pub(super) fn fixture() -> (Schedule, alloy_rpc_types_eth::Block) {
    let reference = block(10, 1000, 7);
    let schedule = Schedule::new(
        DerivationConfig {
            history_update_offset: 0,
            block_interval_seconds: 1,
            sync_interval_blocks: 4,
            l1_reference: GenesisL1Reference {
                chain_id: 31337,
                number: 10,
                hash: reference.header.hash,
                timestamp: 1000,
                prev_randao: reference.header.mix_hash,
            },
        },
        &Header {
            timestamp: 1004,
            mix_hash: reference.header.mix_hash,
            ..Default::default()
        },
    )
    .unwrap();
    (schedule, reference)
}

pub(super) fn startup_replies(replies: &Asserter, reference: &alloy_rpc_types_eth::Block) {
    replies.push_success(&alloy_primitives::U64::from(31337));
    replies.push_success(&block(12, 1008, 9));
    replies.push_success(reference);
}

fn pruned_header_reply(replies: &Asserter, header: &Header) {
    replies.push_failure(
        serde_json::from_value(serde_json::json!({
            "code": 4444, "message": "pruned history unavailable"
        }))
        .unwrap(),
    );
    replies.push_success(&alloy_primitives::Bytes::from(alloy_rlp::encode(header)));
}

#[tokio::test]
async fn pruned_bodies_do_not_prevent_authenticating_retained_canonical_headers() {
    let (schedule, reference) = fixture();
    let replies = Asserter::new();
    replies.push_success(&alloy_primitives::U64::from(31337));
    replies.push_success(&block(12, 1008, 9));
    pruned_header_reply(&replies, &reference.header.inner);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .expect("retained canonical genesis header is sufficient without its body");
    let accepted = block(11, 1004, 8);
    let expected = L1BlockRef {
        number: 11,
        hash: accepted.header.hash,
    };
    pruned_header_reply(&replies, &accepted.header.inner);
    let canonical = source.canonical_header(expected).await.unwrap();
    assert_eq!(canonical.reference(), expected);
    assert_eq!(canonical.header(), &accepted.header.inner);
    assert!(replies.read_q().is_empty());

    pruned_header_reply(&replies, &block(11, 1004, 99).header.inner);
    assert!(matches!(
        source.canonical_header(expected).await,
        Err(DerivationError::Mismatch("L1 block is no longer canonical"))
    ));
    pruned_header_reply(&replies, &block(12, 1008, 8).header.inner);
    assert!(matches!(
        source.canonical_header(expected).await,
        Err(DerivationError::Mismatch("canonical RPC header identity"))
    ));
}

#[tokio::test]
async fn pruned_genesis_still_requires_matching_identity_and_finality() {
    let (schedule, reference) = fixture();
    for case in 0..4 {
        let replies = Asserter::new();
        replies.push_success(&alloy_primitives::U64::from(31337));
        if case == 0 {
            pruned_header_reply(&replies, &block(9, 996, 6).header.inner);
        } else {
            replies.push_success(&block(12, 1008, 9));
            let mut header = reference.header.inner.clone();
            match case {
                1 => header.mix_hash = B256::with_last_byte(88),
                2 => header.timestamp += 1,
                3 => header.number += 1,
                _ => unreachable!(),
            }
            pruned_header_reply(&replies, &header);
        }
        let result = DerivationSource::authenticate(
            ProviderBuilder::new().connect_mocked_client(replies),
            schedule,
        )
        .await;
        assert!(
            matches!(result, Err(DerivationError::Mismatch(_))),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn raw_header_fallback_rejects_malformed_encoding_and_unavailable_sources() {
    let (_, reference) = fixture();
    let header = &reference.header.inner;
    let mut trailing = alloy_rlp::encode(header);
    trailing.push(0);
    for raw in [vec![], vec![0xc0], trailing] {
        let replies = Asserter::new();
        replies.push_failure(
            serde_json::from_value(serde_json::json!({
                "code": 4444, "message": "private endpoint must not leak"
            }))
            .unwrap(),
        );
        replies.push_success(&alloy_primitives::Bytes::from(raw));
        let result = fetch(
            &ProviderBuilder::new().connect_mocked_client(replies),
            31337,
            header.number.into(),
        )
        .await;
        assert!(
            matches!(
                result,
                Err(DerivationError::Mismatch("canonical raw header encoding"))
            ),
            "{result:?}"
        );
    }
    for code in [4444, -32000, -32601] {
        let replies = Asserter::new();
        replies.push_failure(
            serde_json::from_value(serde_json::json!({
                "code": code, "message": "pruned history unavailable: private endpoint"
            }))
            .unwrap(),
        );
        // A pruning error permits exactly one fallback. Unsupported raw-header
        // RPC remains unavailable; identical text with another code cannot
        // trigger fallback or turn an infrastructure failure into acceptance.
        replies.push_failure_msg("raw headers unavailable: private endpoint");
        let result = fetch(
            &ProviderBuilder::new().connect_mocked_client(replies.clone()),
            31337,
            header.number.into(),
        )
        .await;
        assert!(
            matches!(
                result,
                Err(DerivationError::Unavailable("canonical header RPC"))
            ),
            "{result:?}"
        );
        assert_eq!(replies.read_q().len(), usize::from(code != 4444));
    }
}

#[tokio::test]
async fn pruned_header_fallback_uses_canonical_height_on_the_wire() {
    use serde_json::json;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let (schedule, reference) = fixture();
    let accepted = block(11, 1004, 8);
    let expected = L1BlockRef {
        number: 11,
        hash: accepted.header.hash,
    };
    let pruned = json!({"error": {"code": 4444, "message": "expired bodies"}});
    let responses = [
        (
            "eth_chainId",
            serde_json::Value::Null,
            json!({"result": "0x7a69"}),
        ),
        (
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            json!({"result": block(12, 1008, 9)}),
        ),
        (
            "eth_getBlockByNumber",
            json!(["0xa", false]),
            pruned.clone(),
        ),
        (
            "debug_getRawHeader",
            json!(["0xa"]),
            json!({"result": alloy_primitives::Bytes::from(alloy_rlp::encode(&reference.header.inner))}),
        ),
        ("eth_getBlockByNumber", json!(["0xb", false]), pruned),
        (
            "debug_getRawHeader",
            json!(["0xb"]),
            json!({"result": alloy_primitives::Bytes::from(alloy_rlp::encode(&accepted.header.inner))}),
        ),
    ];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for (method, params, mut response) in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            let request = loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0 && request.len() < 8192);
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    && let Ok(json) =
                        serde_json::from_slice::<serde_json::Value>(&request[end + 4..])
                {
                    break json;
                }
            };
            assert_eq!(request["method"], method);
            assert_eq!(
                request["params"], params,
                "fallback must never request an orphan by hash"
            );
            response["jsonrpc"] = json!("2.0");
            response["id"] = request["id"].clone();
            let body = response.to_string();
            stream.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            ).as_bytes()).await.unwrap();
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let source = DerivationSource::authenticate(
            ProviderBuilder::new().connect_http(format!("http://{address}").parse().unwrap()),
            schedule,
        )
        .await
        .unwrap();
        assert_eq!(
            source.canonical_header(expected).await.unwrap().reference(),
            expected
        );
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn finalized_genesis_and_subsequent_headers_are_authenticated_by_canonical_identity() {
    let (schedule, reference) = fixture();
    let replies = Asserter::new();
    startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let accepted = block(11, 1004, 8);
    replies.push_success(&accepted);
    let expected = L1BlockRef {
        number: 11,
        hash: accepted.header.hash,
    };
    let canonical = source.canonical_header(expected).await.unwrap();
    assert_eq!(canonical.reference(), expected);
    assert_eq!(canonical.header(), &accepted.header.inner);
    assert_eq!(
        source.schedule().config().l1_reference.prev_randao,
        reference.header.mix_hash
    );

    replies.push_success(&block(11, 1004, 99));
    assert!(matches!(
        source.canonical_header(expected).await,
        Err(DerivationError::Mismatch("L1 block is no longer canonical"))
    ));
    replies.push_success(&Option::<alloy_rpc_types_eth::Block>::None);
    assert!(matches!(
        source.canonical_header(expected).await,
        Err(DerivationError::Unavailable("canonical header absent"))
    ));
    let mut forged = accepted.clone();
    forged.header.inner.mix_hash = B256::ZERO;
    replies.push_success(&forged);
    assert!(matches!(
        source.canonical_header(expected).await,
        Err(DerivationError::Mismatch("canonical RPC header identity"))
    ));
    replies.push_success(&block(12, 1008, 8));
    assert!(matches!(
        source.canonical_header(expected).await,
        Err(DerivationError::Mismatch("canonical RPC header identity"))
    ));
}

#[tokio::test]
async fn wrong_network_unfinalized_or_fabricated_genesis_reference_fails_closed() {
    let (schedule, reference) = fixture();
    for case in 0..6 {
        let replies = Asserter::new();
        replies.push_success(&alloy_primitives::U64::from(if case == 0 {
            1
        } else {
            31337
        }));
        match case {
            0 => {}
            1 => replies.push_success(&Option::<alloy_rpc_types_eth::Block>::None),
            2 => replies.push_success(&block(9, 996, 6)),
            _ => {
                replies.push_success(&block(12, 1008, 9));
                let mut altered = reference.clone();
                match case {
                    3 => altered.header.inner.mix_hash = B256::with_last_byte(88),
                    4 => altered.header.inner.timestamp += 1,
                    5 => altered.header.inner.number += 1,
                    _ => unreachable!(),
                }
                altered.header.hash = altered.header.inner.hash_slow();
                replies.push_success(&altered);
            }
        }
        let result = DerivationSource::authenticate(
            ProviderBuilder::new().connect_mocked_client(replies),
            schedule,
        )
        .await;
        match case {
            0 => assert!(matches!(
                result,
                Err(DerivationError::Mismatch(
                    "genesis reference chain identity"
                ))
            )),
            1 => assert!(matches!(
                result,
                Err(DerivationError::Unavailable("canonical header absent"))
            )),
            2 => assert!(matches!(
                result,
                Err(DerivationError::Mismatch(
                    "genesis reference is not finalized"
                ))
            )),
            3 | 4 => assert!(matches!(
                result,
                Err(DerivationError::Mismatch(
                    "finalized genesis reference header"
                ))
            )),
            5 => assert!(matches!(
                result,
                Err(DerivationError::Mismatch("canonical RPC header identity"))
            )),
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn l1_genesis_reference_needs_no_finalized_checkpoint_but_must_be_canonical_block_zero() {
    let reference = block(0, 1000, 7);
    let schedule = Schedule::new(
        DerivationConfig {
            history_update_offset: 0,
            block_interval_seconds: 1,
            sync_interval_blocks: 4,
            l1_reference: GenesisL1Reference {
                chain_id: 31337,
                number: 0,
                hash: reference.header.hash,
                timestamp: 1000,
                prev_randao: reference.header.mix_hash,
            },
        },
        &Header {
            timestamp: 1004,
            mix_hash: reference.header.mix_hash,
            ..Default::default()
        },
    )
    .unwrap();
    // Only chain identity and block zero are served. A `finalized` request
    // would consume block zero out of order and then find no reply left.
    for case in 0..4 {
        let replies = Asserter::new();
        replies.push_success(&alloy_primitives::U64::from(31337));
        let mut served = reference.clone();
        match case {
            0 => {}
            1 => served.header.inner.mix_hash = B256::with_last_byte(88),
            2 => served.header.inner.timestamp += 1,
            3 => served.header.inner.number = 1,
            _ => unreachable!(),
        }
        served.header.hash = served.header.inner.hash_slow();
        replies.push_success(&served);
        let result = DerivationSource::authenticate(
            ProviderBuilder::new().connect_mocked_client(replies.clone()),
            schedule,
        )
        .await;
        match case {
            0 => {
                assert!(result.is_ok());
                assert!(replies.read_q().is_empty(), "no finalized request");
            }
            1 | 2 => assert!(matches!(
                result,
                Err(DerivationError::Mismatch("L1 genesis reference header"))
            )),
            3 => assert!(matches!(
                result,
                Err(DerivationError::Mismatch("canonical RPC header identity"))
            )),
            _ => unreachable!(),
        }
    }
}
