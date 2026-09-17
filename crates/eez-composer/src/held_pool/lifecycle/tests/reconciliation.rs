use super::*;
use crate::optimistic::{SourceOutcome, TerminalReason, UserLocation};
use eez_l1::{
    L1BlockRef, L2Endpoint, ReconciledBatchSettlement, ReplaySelection,
    submitter::{CanonicalSourceReceipt, CanonicalSourceState},
};
use eez_telemetry::schema::{ResolutionDisposition, SourceBasis};

fn at(number: u64, tag: u8) -> L1BlockRef {
    L1BlockRef {
        number,
        hash: B256::repeat_byte(tag),
    }
}
fn reference(block: L1BlockRef) -> schema::BlockRef {
    schema::BlockRef {
        number: block.number,
        hash: Hash32(block.hash.0),
    }
}
fn receipt(block: L1BlockRef, index: u64, success: bool) -> CanonicalSourceReceipt {
    CanonicalSourceReceipt {
        block,
        transaction_index: index,
        success,
        consumed: Vec::new(),
        cost: Err(eez_l1::posting_fees::PostingFeeError::Unavailable(
            "fixture",
        )),
    }
}

#[test]
fn reconciliation_preserves_receipt_positions_applied_endpoint_and_distinct_snapshot() {
    let (dir, pool, publisher, writer) = setup();
    pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
    pool.push_contiguous(tx(2, Direction::Outbound), 0).unwrap();
    let txs = pool.pop_all();
    let id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    let snapshot = at(105, 105);
    let inclusion = at(100, 100);
    let endpoint = L2Endpoint {
        number: 10,
        hash: B256::repeat_byte(81),
    };
    let post = receipt(inclusion, 3, true);
    let mut source = receipt(inclusion, 4, true);
    source.consumed = vec![(txs[0].hash, 0)];
    let inbound = [
        Some(CanonicalSourceState {
            nonce: 1,
            receipt: Some(source),
        }),
        None,
    ];
    let observations = [
        SourceObservation {
            nonce: 1,
            block: Some(reference(inclusion)),
            basis: SourceBasis::Receipt,
        },
        SourceObservation {
            nonce: 1,
            block: Some(schema::BlockRef {
                number: 10,
                hash: Hash32([81; 32]),
            }),
            basis: SourceBasis::AppliedOutboundGroup,
        },
    ];
    let applied = ReconciledBatchSettlement {
        l1_block: inclusion,
        tx_hash: id.post_batch_hash,
        tx_index: 3,
        calldata_hash: B256::repeat_byte(93),
        applied_effects: 0..2,
        anchor_applied: true,
        endpoint: Some(endpoint),
        selection: ReplaySelection {
            outbound_groups: 0..1,
            inbound_effects: 0..1,
        },
    };
    let facts = Some(ReconciliationFacts {
        endpoint,
        post: Some(&post),
        applied: Some(&applied),
        inbound: &inbound,
        sources: Some(&observations),
    });
    let outcomes = [SourceOutcome::Terminal(TerminalReason::Committed); 2];
    pool.resolve_attempt_with_evidence(id, snapshot, &outcomes, true, 3, facts, || true)
        .unwrap();
    let assigned = publisher.stats().last_assigned_sequence;
    pool.resolve_attempt_with_evidence(id, snapshot, &outcomes, true, 3, facts, || true)
        .unwrap();
    assert_eq!(publisher.stats().last_assigned_sequence, assigned);
    assert!(!pool.owns(txs[0].hash));
    assert!(!pool.owns(txs[1].hash));
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let (sequence, event) = events
        .iter()
        .find(|(_, e)| matches!(e, Event::Reconciled { .. }))
        .unwrap();
    let Event::Reconciled {
        attempt,
        anchor,
        details: Some(details),
    } = event
    else {
        panic!("reconciliation details")
    };
    assert_eq!(*anchor, reference(snapshot));
    assert_eq!(details.post.unwrap().block, reference(inclusion));
    assert_eq!(details.post.unwrap().transaction_index, 3);
    assert_eq!(details.sources[0].receipt.unwrap().transaction_index, 4);
    assert_eq!(
        details.sources[0].consumed,
        vec![schema::SourceAction {
            call_hash: Hash32([1; 32]),
            source_index: 0
        }]
    );
    assert_eq!(
        details.applied.as_ref().unwrap().endpoint.unwrap().hash,
        Hash32([81; 32])
    );
    assert_ne!(
        details.applied.as_ref().unwrap().endpoint.unwrap().hash,
        Hash32([80; 32])
    );
    assert_eq!(details.l1.genesis_hash, Hash32([11; 32]));
    assert_eq!(details.l2.rollup_id, 7);
    assert_eq!(details.resolutions.len(), 2);
    let terminals = events
        .iter()
        .filter_map(|(_, event)| {
            if let Event::Terminal {
                member,
                attempt,
                outcome,
                assurance,
                anchor,
                reconciliation,
                elapsed_ns,
            } = event
            {
                Some((
                    member,
                    attempt,
                    outcome,
                    assurance,
                    anchor,
                    reconciliation,
                    elapsed_ns,
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 2);
    for (resolution, terminal) in details.resolutions.iter().zip(terminals) {
        assert_eq!(*terminal.0, resolution.member);
        assert_eq!(*terminal.1, Some(*attempt));
        assert_eq!(*terminal.2, Outcome::Committed);
        assert_eq!(*terminal.3, Assurance::NodeReconciled);
        assert!(terminal.4.is_none());
        assert_eq!(
            *terminal.5,
            Some(schema::ReconciliationId {
                epoch: publisher.epoch(),
                sequence: *sequence
            })
        );
        assert_eq!(*terminal.6, resolution.elapsed_ns);
    }
}

#[test]
fn invalid_partition_and_stale_snapshot_emit_nothing_and_preserve_owner() {
    let (dir, pool, publisher, writer) = setup();
    pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
    pool.push_contiguous(tx(2, Direction::Outbound), 0).unwrap();
    let txs = pool.pop_all();
    let id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    let before = publisher.stats().last_assigned_sequence;
    assert!(
        pool.resolve_attempt(id, at(100, 100), &[], false, 3, || true)
            .is_err()
    );
    assert!(
        pool.resolve_attempt(
            id,
            at(100, 100),
            &[
                SourceOutcome::Terminal(TerminalReason::Committed),
                SourceOutcome::Retryable { source_nonce: 1 }
            ],
            true,
            3,
            || true
        )
        .is_err()
    );
    assert!(
        pool.resolve_attempt(
            id,
            at(100, 100),
            &[SourceOutcome::Terminal(TerminalReason::Committed); 2],
            false,
            3,
            || false
        )
        .is_err()
    );
    assert_eq!(publisher.stats().last_assigned_sequence, before);
    for tx in txs {
        assert!(pool.owns(tx.hash));
    }
    pool.state.lock().unwrap().ledger.read(|ledger| {
        assert!(
            ledger.users.values().all(
                |user| user.location == UserLocation::AwaitingEvidence && user.tx.attempts == 0
            )
        );
    });
    assert!(writer.shutdown(Duration::from_secs(8)));
    assert_eq!(events(dir.path()).len(), 3);
}

#[test]
fn retries_and_local_exhaustion_preserve_admission_and_charge_once() {
    let (dir, pool, publisher, writer) = setup();
    pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
    let txs = pool.pop_all();
    let first = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    let retry = [SourceOutcome::Retryable { source_nonce: 0 }];
    pool.resolve_attempt(first, at(100, 100), &retry, true, 2, || true)
        .unwrap();
    pool.resolve_attempt(first, at(100, 100), &retry, true, 2, || true)
        .unwrap();
    let txs = pool.pop_all();
    assert_eq!(txs[0].attempts, 1);
    let second = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    pool.resolve_attempt(second, at(101, 101), &retry, true, 2, || true)
        .unwrap();
    assert!(!pool.owns(txs[0].hash));
    assert_eq!(publisher.stats().dropped_records, 0);
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let resolutions = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Reconciled {
                details: Some(d), ..
            } => Some(&d.resolutions[0]),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(resolutions.len(), 2);
    assert_eq!(resolutions[0].member, resolutions[1].member);
    assert_eq!(
        resolutions[0].disposition,
        ResolutionDisposition::Requeued {
            charged_attempts: 1
        }
    );
    assert_eq!(
        resolutions[1].disposition,
        ResolutionDisposition::Terminal {
            outcome: Outcome::AttemptsExhausted,
            assurance: Assurance::LocalPolicy
        }
    );
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(e, Event::Terminal { .. }))
            .count(),
        1
    );
}

#[test]
fn reconciliation_capture_failure_does_not_undo_canonical_ownership() {
    for missing in [false, true] {
        let (dir, pool, publisher, writer) = setup();
        pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
        let txs = pool.pop_all();
        let id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
        if missing {
            pool.state
                .lock()
                .unwrap()
                .lifecycle
                .as_mut()
                .unwrap()
                .admissions
                .clear();
        }
        let mut source = receipt(at(100, 100), 1, true);
        source.consumed = vec![(B256::ZERO, 0); 32769];
        let inbound = [Some(CanonicalSourceState {
            nonce: 1,
            receipt: Some(source),
        })];
        let observations = [SourceObservation {
            nonce: 1,
            block: Some(reference(at(100, 100))),
            basis: SourceBasis::Receipt,
        }];
        let facts = (!missing).then_some(ReconciliationFacts {
            endpoint: L2Endpoint {
                number: 10,
                hash: B256::repeat_byte(81),
            },
            post: None,
            applied: None,
            inbound: &inbound,
            sources: Some(&observations),
        });
        pool.resolve_attempt_with_evidence(
            id,
            at(105, 105),
            &[SourceOutcome::Terminal(TerminalReason::SourceCompleted)],
            false,
            3,
            facts,
            || true,
        )
        .unwrap();
        assert!(!pool.owns(txs[0].hash));
        assert_eq!(publisher.stats().last_assigned_sequence, 3);
        assert_eq!(publisher.stats().dropped_records, 1);
        assert_eq!(publisher.stats().dropped_sequence_min, Some(3));
        assert!(writer.shutdown(Duration::from_secs(8)));
        assert_eq!(events(dir.path()).len(), 2);
    }
}

#[test]
fn competing_reconciliation_callbacks_capture_only_committed_partition() {
    use std::sync::{Arc, Barrier};
    let (dir, pool, publisher, writer) = setup();
    pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
    let txs = pool.pop_all();
    let id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    let pool = Arc::new(pool);
    let barrier = Arc::new(Barrier::new(3));
    let handles = [
        TerminalReason::SourceReverted,
        TerminalReason::SourceCompleted,
    ]
    .into_iter()
    .map(|reason| {
        let pool = pool.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            pool.resolve_attempt(
                id,
                at(100, 100),
                &[SourceOutcome::Terminal(reason)],
                false,
                3,
                || true,
            )
        })
    })
    .collect::<Vec<_>>();
    barrier.wait();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    assert_eq!(publisher.stats().last_assigned_sequence, 4);
    let reason = pool.state.lock().unwrap().ledger.read(|ledger| {
        match ledger.users[&txs[0].hash].location {
            UserLocation::Terminal { reason, .. } => reason,
            _ => panic!("terminal owner"),
        }
    });
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let terminal = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::Terminal {
                outcome, assurance, ..
            } => Some((*outcome, *assurance)),
            _ => None,
        })
        .unwrap();
    assert_eq!(terminal.1, Assurance::Unknown);
    assert_eq!(
        terminal.0,
        if reason == TerminalReason::SourceReverted {
            Outcome::SourceReverted
        } else {
            Outcome::SourceCompleted
        }
    );
}

#[test]
fn same_height_reorg_reopens_owner_and_preserves_both_reconciliation_observations() {
    let (dir, pool, publisher, writer) = setup();
    pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
    let txs = pool.pop_all();
    let id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    let outcomes = [SourceOutcome::Terminal(TerminalReason::Committed)];
    pool.resolve_attempt(id, at(100, 100), &outcomes, false, 3, || true)
        .unwrap();
    assert!(!pool.owns(txs[0].hash));
    pool.observe_l1_reorg(at(99, 99), B256::repeat_byte(100), at(100, 101))
        .unwrap();
    assert!(pool.owns(txs[0].hash));
    pool.resolve_attempt(id, at(100, 101), &outcomes, false, 3, || true)
        .unwrap();
    assert!(!pool.owns(txs[0].hash));
    let assigned = publisher.stats().last_assigned_sequence;
    pool.invalidate_after_l1(99).unwrap();
    assert!(pool.owns(txs[0].hash));
    assert_eq!(
        publisher.stats().last_assigned_sequence,
        assigned,
        "height-only audit invalidation cannot invent an ancestor hash"
    );
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let reconciled = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Reconciled {
                attempt,
                anchor,
                details: Some(d),
            } => Some((*attempt, *anchor, d.resolutions[0].member)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(reconciled.len(), 2);
    assert_eq!(reconciled[0].0, reconciled[1].0);
    assert_eq!(reconciled[0].2, reconciled[1].2);
    assert_eq!(reconciled[0].1.number, reconciled[1].1.number);
    assert_ne!(reconciled[0].1.hash, reconciled[1].1.hash);
    let reorgs = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Reorg {
                ancestor,
                details: Some(details),
            } => Some((ancestor, details)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(reorgs.len(), 1);
    assert_eq!(*reorgs[0].0, reference(at(99, 99)));
    assert_eq!(reorgs[0].1.old_head_hash, Hash32([100; 32]));
    assert_eq!(reorgs[0].1.new_head, reference(at(100, 101)));
    assert_eq!(reorgs[0].1.l1.genesis_hash, Hash32([11; 32]));
    assert_eq!(reorgs[0].1.l2.rollup_id, 7);
}

#[test]
fn local_nonce_gap_cascade_keeps_original_owner_and_never_claims_canonical_progress() {
    let (dir, pool, _, writer) = setup();
    pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
    let mut suffix = tx(2, Direction::Inbound);
    suffix.nonce = 1;
    pool.push_contiguous(suffix, 0).unwrap();
    let txs = pool.pop_all();
    let first = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    pool.resolve_attempt(
        first,
        at(100, 100),
        &[SourceOutcome::Retryable { source_nonce: 0 }; 2],
        false,
        3,
        || true,
    )
    .unwrap();
    let root = pool.pop_n(1);
    let second = prepare(&pool, root.clone(), manifest(&root)).unwrap();
    pool.resolve_attempt(
        second,
        at(101, 101),
        &[SourceOutcome::Retryable { source_nonce: 0 }],
        true,
        1,
        || true,
    )
    .unwrap();
    assert!(!pool.owns(txs[0].hash));
    assert!(!pool.owns(txs[1].hash));
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let resolutions = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::Reconciled {
                attempt,
                details: Some(d),
                ..
            } if attempt.generation == second.generation => Some(&d.resolutions),
            _ => None,
        })
        .unwrap();
    assert_eq!(resolutions.len(), 2);
    assert_eq!(resolutions[0].owner.generation, second.generation);
    assert_eq!(
        resolutions[0].disposition,
        ResolutionDisposition::Terminal {
            outcome: Outcome::AttemptsExhausted,
            assurance: Assurance::LocalPolicy
        }
    );
    assert_eq!(resolutions[1].owner.generation, first.generation);
    assert_eq!(
        resolutions[1].disposition,
        ResolutionDisposition::Terminal {
            outcome: Outcome::NonceGap,
            assurance: Assurance::LocalPolicy
        }
    );
}
