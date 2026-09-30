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
            timestamp: 1000,
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
    assert_eq!(source.seed_history().seed_for(1), reference.header.mix_hash);

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

fn endpoint(number: u64, seed: u8, variant: u8) -> Header {
    Header {
        number,
        timestamp: 1000 + number,
        mix_hash: B256::with_last_byte(seed),
        state_root: B256::with_last_byte(variant),
        ..Default::default()
    }
}

fn acceptance(
    l1: &AuthenticatedL1Header,
    endpoint: &Header,
    tx_index: u64,
) -> ReconciledBatchSettlement {
    ReconciledBatchSettlement {
        l1_block: l1.reference(),
        tx_hash: B256::from(alloy_primitives::U256::from(
            l1.header.number * 10 + tx_index,
        )),
        tx_index,
        calldata_hash: B256::with_last_byte(1),
        applied_effects: 0..1,
        anchor_applied: true,
        endpoint: Some(L2Endpoint {
            number: endpoint.number,
            hash: endpoint.hash_slow(),
        }),
        selection: crate::ReplaySelection::default(),
    }
}

#[tokio::test]
async fn live_seed_starts_after_terminal_height_partial_resume_catchup_reorg_and_rebuild() {
    let (schedule, reference) = fixture();
    let replies = Asserter::new();
    startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let mut history = source.seed_history();
    let mut replay = Vec::new();
    for (l1_number, l1_ts, seed, l2) in [
        (11, 1004, 8, endpoint(4, 7, 0)),
        // Same-height B[1] extends B[0] after its live slot has passed.
        (12, 1008, 9, endpoint(4, 7, 1)),
        (13, 1012, 10, endpoint(12, 8, 0)),
    ] {
        let served = block(l1_number, l1_ts, seed);
        replies.push_success(&served);
        let l1 = source
            .canonical_header(L1BlockRef {
                number: l1_number,
                hash: served.header.hash,
            })
            .await
            .unwrap();
        let settled = acceptance(&l1, &l2, 0);
        history.record(&settled, &l1, &l2).unwrap();
        // Exact duplicate re-observation is idempotent.
        history.record(&settled, &l1, &l2).unwrap();
        replay.push((settled, l1, l2));
    }
    assert_eq!(history.anchors().len(), 3);
    for (number, seed) in [(0, 7), (4, 7), (5, 8), (8, 8), (12, 8), (13, 10)] {
        assert_eq!(history.seed_for(number), B256::with_last_byte(seed));
    }
    assert_eq!(history.anchors()[1].kind(), AnchorKind::CatchUp);

    // A fresh process starts from genesis and reauthenticates/replays accepted
    // occurrences. The last L2 header's mixHash (8) is not the child seed (10).
    let mut rebuilt = source.seed_history();
    for (settled, l1, l2) in &replay {
        rebuilt.record(settled, l1, l2).unwrap();
    }
    assert_eq!(rebuilt.anchors(), history.anchors());
    assert_eq!(rebuilt.seed_for(13), history.seed_for(13));

    history.reorg_to(&replay[1].1).unwrap();
    assert_eq!(
        history.seed_for(13),
        B256::with_last_byte(8),
        "remove live anchor"
    );
    history.reorg_to(&replay[0].1).unwrap();
    assert_eq!(
        history.seed_for(13),
        B256::with_last_byte(8),
        "remove catch-up anchor"
    );
    replies.push_success(&reference);
    let genesis = source
        .canonical_header(L1BlockRef {
            number: 10,
            hash: reference.header.hash,
        })
        .await
        .unwrap();
    history.reorg_to(&genesis).unwrap();
    assert!(history.anchors().is_empty());
    assert_eq!(history.seed_for(13), B256::with_last_byte(7));
}

#[tokio::test]
async fn unaccepted_wrong_seed_wrong_header_and_unordered_transitions_leave_history_unchanged() {
    let (schedule, reference) = fixture();
    let replies = Asserter::new();
    startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let served = block(11, 1004, 8);
    replies.push_success(&served);
    let l1 = source
        .canonical_header(L1BlockRef {
            number: 11,
            hash: served.header.hash,
        })
        .await
        .unwrap();
    let l2 = endpoint(4, 7, 0);
    let settled = acceptance(&l1, &l2, 0);
    let mut history = source.seed_history();
    let mutations: &[fn(&mut ReconciledBatchSettlement)] = &[
        |s| s.endpoint = None,
        |s| s.endpoint.as_mut().unwrap().hash = B256::ZERO,
        |s| s.l1_block.hash = B256::ZERO,
        |s| {
            s.anchor_applied = false;
            s.applied_effects = 0..0;
        },
    ];
    for mutate in mutations {
        let mut invalid = settled.clone();
        mutate(&mut invalid);
        assert!(matches!(
            history.record(&invalid, &l1, &l2),
            Err(DerivationError::Mismatch(_))
        ));
        assert!(history.anchors().is_empty());
    }
    let wrong_seed = endpoint(4, 8, 0);
    assert!(matches!(
        history.record(&acceptance(&l1, &wrong_seed, 0), &l1, &wrong_seed),
        Err(DerivationError::Mismatch(
            "accepted endpoint used the wrong seed"
        ))
    ));
    history.record(&settled, &l1, &l2).unwrap();
    let conflict = endpoint(4, 7, 1);
    assert!(matches!(
        history.record(&acceptance(&l1, &conflict, 0), &l1, &conflict),
        Err(DerivationError::Mismatch(
            "conflicting accepted anchor occurrence"
        ))
    ));
    assert_eq!(history.anchors().len(), 1);
}

#[tokio::test]
async fn canonical_cursor_and_seed_publish_together_and_reorg_at_the_same_height() {
    let (schedule, reference) = fixture();
    let replies = Asserter::new();
    startup_replies(&replies, &reference);
    let source = DerivationSource::authenticate(
        ProviderBuilder::new().connect_mocked_client(replies.clone()),
        schedule,
    )
    .await
    .unwrap();
    let head = crate::L1CanonicalHead::default();
    assert!(head.derivation_seed(1).is_err());
    head.restore_derivation(source.seed_history());
    let before = head.next_block_context(&endpoint(4, 7, 0)).unwrap();
    assert_eq!(before, (1005, B256::with_last_byte(7)));
    let mut accepted = Vec::new();
    for (number, time, seed, l2) in [
        (11, 1004, 8, endpoint(4, 7, 0)),
        (12, 1008, 9, endpoint(4, 7, 1)),
    ] {
        let served = block(number, time, seed);
        replies.push_success(&served);
        let l1 = source
            .canonical_header(L1BlockRef {
                number,
                hash: served.header.hash,
            })
            .await
            .unwrap();
        let evidence = acceptance(&l1, &l2, 0);
        head.validate_accepted(&evidence, &l1, &l2).unwrap();
        assert_eq!(head.cursor(), if number == 11 { 0 } else { 4 });
        head.publish_accepted(evidence.clone(), &l1, &l2).unwrap();
        assert_eq!(head.cursor(), 4);
        assert_eq!(head.derivation_seed(4).unwrap(), B256::with_last_byte(7));
        assert_eq!(head.derivation_seed(5).unwrap(), B256::with_last_byte(8));
        accepted.push((evidence, l1, l2));
    }
    // A draft keeps the captured seed even after canonical history advances.
    assert_eq!(before.1, B256::with_last_byte(7));
    let generation = head.settlement_snapshot().generation;
    let (mut invalid, l1, l2) = accepted[1].clone();
    invalid.endpoint.as_mut().unwrap().hash = B256::ZERO;
    assert!(head.publish_accepted(invalid, &l1, &l2).is_err());
    assert_eq!(head.settlement_snapshot().generation, generation);
    assert_eq!(head.retreat_on_l1_reorg(11).0, 4);
    assert_eq!(head.derivation_seed(5).unwrap(), B256::with_last_byte(8));
    assert_eq!(head.retreat_on_l1_reorg(10).0, 0);
    assert_eq!(head.derivation_seed(5).unwrap(), B256::with_last_byte(7));
}
