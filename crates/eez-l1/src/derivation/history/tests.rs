use super::*;
use alloy_consensus::Header;
use alloy_primitives::{U64, U256};
use alloy_provider::ProviderBuilder;
use eez_protocol::{
    l1_history::L1HistorySource,
    l2::{DerivationConfig, GenesisL1Reference},
};
use std::{collections::BTreeMap, sync::Mutex};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct Fixture {
    schedule: Schedule,
    genesis: Header,
    blocks: Arc<Mutex<BTreeMap<u64, alloy_rpc_types_eth::Block>>>,
    provider: alloy_provider::DynProvider,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(times: &[u64]) -> Self {
        let mut blocks = BTreeMap::new();
        let mut parent = B256::ZERO;
        for (number, timestamp) in times.iter().copied().enumerate() {
            let mut block = super::super::tests::block(number as u64, timestamp, number as u8 + 1);
            block.header.inner.parent_hash = parent;
            block.header.hash = block.header.inner.hash_slow();
            parent = block.header.hash;
            blocks.insert(number as u64, block);
        }
        let reference = &blocks[&0].header;
        let genesis = Header {
            timestamp: times[0] + 4,
            mix_hash: reference.mix_hash,
            ..Default::default()
        };
        let schedule = Schedule::new(
            DerivationConfig {
                block_interval_seconds: 1,
                sync_interval_blocks: 4,
                history_update_offset: 2,
                l1_reference: GenesisL1Reference {
                    chain_id: 31337,
                    number: 0,
                    hash: reference.hash,
                    timestamp: reference.timestamp,
                    prev_randao: reference.mix_hash,
                },
            },
            &genesis,
        )
        .unwrap();
        let blocks = Arc::new(Mutex::new(blocks));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let served = Arc::clone(&blocks);
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut input = Vec::new();
                let mut buffer = [0; 1024];
                let request = loop {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0 && input.len() < 8192);
                    input.extend_from_slice(&buffer[..n]);
                    if let Some(end) = input.windows(4).position(|v| v == b"\r\n\r\n")
                        && let Ok(json) =
                            serde_json::from_slice::<serde_json::Value>(&input[end + 4..])
                    {
                        break json;
                    }
                };
                let result = match request["method"].as_str().unwrap() {
                    "eth_chainId" => serde_json::json!(U64::from(31337)),
                    "eth_getBlockByNumber" => {
                        let number: U64 =
                            serde_json::from_value(request["params"][0].clone()).unwrap();
                        serde_json::to_value(served.lock().unwrap().get(&number.to::<u64>()))
                            .unwrap()
                    }
                    method => panic!("unexpected source method {method}"),
                };
                let body =
                    serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
                        .to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let provider = ProviderBuilder::new()
            .connect_http(format!("http://{address}").parse().unwrap())
            .erased();
        Self {
            schedule,
            genesis,
            blocks,
            provider,
            server,
        }
    }

    async fn source(&self) -> DerivationSource {
        DerivationSource::authenticate(self.provider.clone(), self.schedule)
            .await
            .unwrap()
    }

    fn reference(&self, number: u64) -> L1BlockRef {
        L1BlockRef {
            number,
            hash: self.blocks.lock().unwrap()[&number].header.hash,
        }
    }

    fn rules(&self) -> HeaderRules {
        HeaderRules {
            london_block: None,
            shanghai_time: None,
            cancun_time: None,
            prague_time: None,
            unsupported_time: None,
        }
    }

    fn reorg_from(&self, start: u64) {
        let mut blocks = self.blocks.lock().unwrap();
        let mut parent = blocks[&(start - 1)].header.hash;
        for (_, block) in blocks.range_mut(start..) {
            // Keep the randomness identical: endpoint hashes must still change.
            block.header.inner.extra_data = Bytes::from_static(b"replacement");
            block.header.inner.parent_hash = parent;
            block.header.hash = block.header.inner.hash_slow();
            parent = block.header.hash;
        }
    }
}

#[tokio::test]
async fn canonical_history_drives_delayed_selection_and_keeps_bridge_only_headers() {
    let fixture = Fixture::new(&[1000, 1001, 1003, 1004, 1008]).await;
    let source = fixture.source().await;
    let prepared = source
        .prepare_history(1004, fixture.reference(4), fixture.rules(), None)
        .await
        .unwrap();
    assert_eq!(prepared.chain().headers().len(), 5);
    for number in 0..=4 {
        assert_eq!(
            prepared.chain().at(number).unwrap().hash(),
            fixture.reference(number).hash
        );
    }
    let head = crate::L1CanonicalHead::default();
    head.initialize_history(fixture.schedule);
    assert!(head.next_block_context(&fixture.genesis).is_err());
    head.publish_history(prepared).unwrap();
    assert_eq!(
        head.next_block_context(&fixture.genesis).unwrap(),
        (1005, B256::with_last_byte(1))
    );
    assert_eq!(head.derivation_randao(2).unwrap(), B256::with_last_byte(1));
    assert_eq!(head.derivation_randao(3).unwrap(), B256::with_last_byte(4));
    assert_eq!(
        head.derivation_randao(4).unwrap(),
        B256::with_last_byte(4),
        "Sync retains the source"
    );
    assert_eq!(head.derivation_randao(7).unwrap(), B256::with_last_byte(5));
    assert!(
        head.headers(1012).is_err(),
        "late evidence cannot select an older source"
    );
}

#[tokio::test]
async fn late_delivery_waits_and_missed_slots_do_not_create_headers() {
    let fixture = Fixture::new(&[1000, 1003, 1006]).await;
    let source = fixture.source().await;
    let error = source
        .prepare_history(1007, fixture.reference(1), fixture.rules(), None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        DerivationError::Unavailable("canonical L1 has not resolved the source cutoff")
    ));
    let prepared = source
        .prepare_history(1007, fixture.reference(2), fixture.rules(), None)
        .await
        .unwrap();
    assert_eq!(prepared.chain().select(1004).unwrap().header().number, 1);
    assert_eq!(
        prepared.chain().headers().len(),
        2,
        "the selected predecessor plus its actual successor"
    );
    let mut history = CanonicalHistory::new(fixture.schedule);
    history.publish(prepared).unwrap();
    assert_eq!(history.prev_randao(3).unwrap(), B256::with_last_byte(2));
    assert!(
        history.prev_randao(7).is_err(),
        "future unresolved cutoff still waits"
    );
}

#[tokio::test]
async fn owned_replacement_has_a_bounded_parent_exception_and_new_evidence_revokes_it() {
    let fixture = Fixture::new(&[1000, 1004, 1008]).await;
    let source = fixture.source().await;
    let prepared = source
        .prepare_history(1004, fixture.reference(0), fixture.rules(), None)
        .await
        .unwrap();
    let head = crate::L1CanonicalHead::default();
    head.initialize_history(fixture.schedule);
    head.publish_history(prepared).unwrap();
    let endpoint = crate::L2Endpoint {
        number: 0,
        hash: fixture.genesis.hash_slow(),
    };
    let parent = fixture.reference(0);
    head.mark_scanned(parent, endpoint);
    let generation = head.settlement_snapshot().generation;
    assert!(head.derivation_randao(3).is_err());
    for (generation, parent, target) in [
        (generation + 1, parent, 1008),
        (generation, fixture.reference(1), 1008),
        (generation, parent, 1004),
        (generation, parent, 1009),
        (generation, parent, 1024),
    ] {
        assert!(
            head.authorize_replacement_history(generation, parent, target)
                .is_err()
        );
        assert!(head.derivation_randao(3).is_err());
    }
    head.authorize_replacement_history(generation, parent, 1008)
        .unwrap();
    assert_eq!(head.derivation_randao(3).unwrap(), B256::with_last_byte(1));
    assert!(
        head.headers(1008).is_err(),
        "the exception ends before its target"
    );
    head.authorize_replacement_history(generation, parent, 1020)
        .unwrap();
    assert_eq!(head.headers(1016).unwrap().headers().len(), 1);
    assert!(head.headers(1020).is_err());
    assert!(
        head.authorize_replacement_history(generation, parent, 1008)
            .is_err()
    );
    head.invalidate_scan();
    assert!(
        head.headers(1004).is_err(),
        "incomplete reconciliation revokes the exception"
    );
    head.mark_scanned(parent, endpoint);
    head.authorize_replacement_history(head.settlement_snapshot().generation, parent, 1020)
        .unwrap();

    // A delayed original can arrive without changing the previous parent's
    // hash. Its source replaces the provisional repeat, even with no settled
    // L2 blocks removed and regardless of the PREVRANDAO field.
    head.publish_history(
        source
            .prepare_history(1004, fixture.reference(1), fixture.rules(), None)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(head.derivation_randao(3).unwrap(), B256::with_last_byte(2));
    assert!(head.headers(1008).is_err());
    head.retreat_on_l1_reorg(0);
    assert!(head.headers(1004).is_err());
}

#[tokio::test]
async fn reorg_invalidates_evidence_without_any_settlement_or_randomness_change() {
    let fixture = Fixture::new(&[1000, 1001, 1003, 1004, 1008]).await;
    let source = fixture.source().await;
    let old = source
        .prepare_history(1004, fixture.reference(4), fixture.rules(), None)
        .await
        .unwrap();
    let head = crate::L1CanonicalHead::default();
    head.initialize_history(fixture.schedule);
    head.publish_history(old.clone()).unwrap();
    let seed = head.derivation_randao(3).unwrap();
    fixture.reorg_from(1);
    assert_eq!(head.retreat_on_l1_reorg(0), (0, 0, 0));
    assert!(head.derivation_randao(3).is_err());
    let fresh = source
        .prepare_history(
            1004,
            fixture.reference(4),
            fixture.rules(),
            Some(old.chain()),
        )
        .await
        .unwrap();
    assert_ne!(
        fresh.chain().at(1).unwrap().hash(),
        old.chain().at(1).unwrap().hash()
    );
    head.publish_history(fresh).unwrap();
    assert_eq!(head.derivation_randao(3).unwrap(), seed);
}

#[tokio::test]
async fn broken_bridge_missing_header_and_wrong_fork_layout_fail_before_publication() {
    for case in 0..3 {
        let fixture = Fixture::new(&[1000, 1001, 1003, 1004, 1008]).await;
        let source = fixture.source().await;
        {
            let mut blocks = fixture.blocks.lock().unwrap();
            match case {
                0 => {
                    blocks.remove(&2);
                }
                1 => {
                    let block = blocks.get_mut(&2).unwrap();
                    block.header.inner.parent_hash = B256::ZERO;
                    block.header.hash = block.header.inner.hash_slow();
                }
                _ => {
                    let block = blocks.get_mut(&2).unwrap();
                    block.header.inner.base_fee_per_gas = Some(1);
                    block.header.hash = block.header.inner.hash_slow();
                }
            }
        }
        assert!(
            source
                .prepare_history(1004, fixture.reference(4), fixture.rules(), None)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn canonical_cache_reuses_prefix_but_never_substitutes_a_claimed_anchor() {
    let fixture = Fixture::new(&[1000, 1001, 1003, 1004, 1008, 1012]).await;
    let source = fixture.source().await;
    let old = source
        .prepare_history(1004, fixture.reference(4), fixture.rules(), None)
        .await
        .unwrap();
    let fresh = source
        .prepare_history(
            1008,
            fixture.reference(5),
            fixture.rules(),
            Some(old.chain()),
        )
        .await
        .unwrap();
    assert_eq!(
        fresh.chain().at(0).unwrap(),
        old.chain().at(0).unwrap(),
        "retain bounded evidence for terminal-prefix replay"
    );
    assert_eq!(
        fresh.chain().endpoint().unwrap().hash(),
        fixture.reference(5).hash
    );
    let wrong = L1BlockRef {
        number: 5,
        hash: B256::with_last_byte(99),
    };
    assert!(matches!(
        source
            .prepare_history(1008, wrong, fixture.rules(), Some(fresh.chain()))
            .await,
        Err(DerivationError::Mismatch("L1 block is no longer canonical"))
    ));
}

#[tokio::test]
async fn accepted_commitments_do_not_refresh_randomness_and_bad_bindings_do_not_advance() {
    let fixture = Fixture::new(&[1000, 1001, 1003, 1004, 1008, 1012]).await;
    let source = fixture.source().await;
    let head = crate::L1CanonicalHead::default();
    head.initialize_history(fixture.schedule);
    head.publish_history(
        source
            .prepare_history(1004, fixture.reference(5), fixture.rules(), None)
            .await
            .unwrap(),
    )
    .unwrap();
    let l1 = source.canonical_header(fixture.reference(4)).await.unwrap();
    let endpoint = Header {
        number: 4,
        timestamp: 1008,
        mix_hash: head.derivation_randao(4).unwrap(),
        difficulty: U256::ZERO,
        ..Default::default()
    };
    let evidence = crate::ReconciledBatchSettlement {
        l1_block: fixture.reference(4),
        tx_hash: B256::with_last_byte(21),
        tx_index: 0,
        calldata_hash: B256::with_last_byte(22),
        applied_effects: 0..1,
        anchor_applied: true,
        endpoint: Some(crate::L2Endpoint {
            number: 4,
            hash: endpoint.hash_slow(),
        }),
        selection: Default::default(),
    };
    for case in 0..4 {
        let mut invalid = evidence.clone();
        let mut header = endpoint.clone();
        match case {
            0 => invalid.endpoint.as_mut().unwrap().hash = B256::ZERO,
            1 => invalid.l1_block.hash = B256::ZERO,
            2 => {
                invalid.anchor_applied = false;
                invalid.applied_effects = 0..0;
            }
            _ => {
                header.mix_hash = B256::ZERO;
                invalid.endpoint.as_mut().unwrap().hash = header.hash_slow();
            }
        }
        assert!(head.publish_accepted(invalid, &l1, &header).is_err());
        assert_eq!(head.cursor(), 0);
    }
    let before = head.derivation_randao(5).unwrap();
    head.publish_accepted(evidence.clone(), &l1, &endpoint)
        .unwrap();
    head.publish_accepted(evidence, &l1, &endpoint).unwrap();
    assert_eq!(head.cursor(), 4);
    assert_eq!(head.derivation_randao(5).unwrap(), before);
    assert_ne!(
        before,
        l1.header().mix_hash,
        "settlement randomness does not replace the scheduled source"
    );
}
