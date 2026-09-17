use super::*;
use crate::optimistic::{SourceOutcome, TerminalReason, UserLocation};

#[test]
fn local_chain_eviction_captures_queued_and_inflight_once_with_direction_isolation() {
    let (dir, pool, publisher, writer) = setup();
    let root = tx(1, Direction::Inbound);
    let mut suffix = tx(2, Direction::Inbound);
    suffix.nonce = 1;
    let independent = tx(3, Direction::Outbound);
    for held in [root.clone(), suffix.clone(), independent.clone()] {
        pool.push_contiguous(held, 0).unwrap();
    }
    let _ = pool.pop_n(1);
    let evicted = pool.evict_exhausted_chain_at_or_above(root.sender, root.direction, 0);
    assert_eq!(
        evicted.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
        vec![suffix.hash]
    );
    assert!(!pool.owns(root.hash));
    assert!(!pool.owns(suffix.hash));
    assert!(pool.owns(independent.hash));
    let assigned = publisher.stats().last_assigned_sequence;
    assert!(
        pool.evict_chain_at_or_above(root.sender, root.direction, 0)
            .is_empty()
    );
    pool.release_in_flight_batch(&[root.clone(), suffix]);
    assert_eq!(publisher.stats().last_assigned_sequence, assigned);
    pool.push_contiguous(root.clone(), 0).unwrap();
    assert!(pool.owns(root.hash));
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let terminal = events
        .iter()
        .filter_map(|(_, event)| match event {
            Event::Terminal {
                member,
                attempt,
                outcome,
                assurance,
                anchor,
                reconciliation,
                ..
            } => Some((member, attempt, outcome, assurance, anchor, reconciliation)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 2);
    assert_eq!(*terminal[0].2, Outcome::AttemptsExhausted);
    assert_eq!(*terminal[1].2, Outcome::NonceGap);
    for (member, attempt, _, assurance, anchor, reconciliation) in terminal {
        assert!(attempt.is_none());
        assert!(anchor.is_none());
        assert!(reconciliation.is_none());
        assert_eq!(*assurance, Assurance::LocalPolicy);
        assert_eq!(member.source.direction, schema::Direction::Inbound);
    }
    let readmissions=events.iter().filter(|(_,event)|matches!(event,Event::Admitted{source,..} if source.transaction_hash.0 == root.hash.0)).collect::<Vec<_>>();
    assert_eq!(readmissions.len(), 2);
    assert_ne!(readmissions[0].0, readmissions[1].0);
}

#[test]
fn release_only_disposes_actual_inflight_users_and_preserves_canonical_owners() {
    let (dir, pool, publisher, writer) = setup();
    let held = tx(1, Direction::Inbound);
    pool.push_contiguous(held.clone(), 0).unwrap();
    pool.release_in_flight_batch(std::slice::from_ref(&held));
    assert!(pool.owns(held.hash));
    assert_eq!(publisher.stats().last_assigned_sequence, 1);
    let drained = pool.pop_all();
    pool.release_in_flight_batch(&vec![held.clone(); MAX_MEMBERS + 1]);
    assert_eq!(publisher.stats().last_assigned_sequence, 2);
    pool.release_in_flight_batch(&drained);
    assert_eq!(publisher.stats().last_assigned_sequence, 2);
    pool.push_contiguous(held.clone(), 0).unwrap();
    let txs = pool.pop_all();
    let id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    let assigned = publisher.stats().last_assigned_sequence;
    pool.release_in_flight_batch(&txs);
    assert_eq!(publisher.stats().last_assigned_sequence, assigned);
    pool.state.lock().unwrap().ledger.read(|ledger| {
        assert_eq!(
            ledger.users[&held.hash].location,
            UserLocation::AwaitingEvidence
        )
    });
    pool.resolve_attempt(
        id,
        eez_l1::L1BlockRef {
            number: 100,
            hash: B256::repeat_byte(100),
        },
        &[SourceOutcome::Terminal(TerminalReason::SourceCompleted)],
        false,
        3,
        || true,
    )
    .unwrap();
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let terminals = events
        .iter()
        .filter_map(|(_, event)| match event {
            Event::Terminal {
                member,
                outcome,
                assurance,
                ..
            } => Some((member.admission, *outcome, *assurance)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 2);
    assert_eq!(
        (terminals[0].1, terminals[0].2),
        (Outcome::PoolDiscarded, Assurance::LocalPolicy)
    );
    assert_eq!(
        (terminals[1].1, terminals[1].2),
        (Outcome::SourceCompleted, Assurance::Unknown)
    );
    assert_ne!(terminals[0].0, terminals[1].0);
}

#[test]
fn recovery_conflict_reports_removed_suffix_without_terminating_replacement() {
    let (dir, pool, _, writer) = setup();
    let root = tx(1, Direction::Inbound);
    let mut suffix = tx(2, Direction::Inbound);
    suffix.nonce = 1;
    pool.push_contiguous(root.clone(), 0).unwrap();
    pool.push_contiguous(suffix.clone(), 0).unwrap();
    let recovered = pool.pop_all();
    pool.release_in_flight_batch(std::slice::from_ref(&root));
    let replacement = tx(3, Direction::Inbound);
    pool.push_contiguous(replacement.clone(), 0).unwrap();
    pool.push_front_batch(recovered);
    assert!(pool.owns(replacement.hash));
    assert!(!pool.owns(suffix.hash));
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let terminals = events
        .iter()
        .filter_map(|(_, event)| match event {
            Event::Terminal {
                member, outcome, ..
            } => Some((member.source.transaction_hash, *outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminals,
        vec![
            (Hash32(root.hash.0), Outcome::PoolDiscarded),
            (Hash32(suffix.hash.0), Outcome::PoolDiscarded)
        ]
    );
}

#[test]
fn reconciliation_reports_unsubmitted_nonce_gap_without_duplicating_ledger_terminals() {
    let (dir, pool, _, writer) = setup();
    let root = tx(1, Direction::Inbound);
    let mut suffix = tx(2, Direction::Inbound);
    suffix.nonce = 1;
    pool.push_contiguous(root, 0).unwrap();
    pool.push_contiguous(suffix.clone(), 0).unwrap();
    let txs = pool.pop_n(1);
    let id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    pool.resolve_attempt(
        id,
        eez_l1::L1BlockRef {
            number: 100,
            hash: B256::repeat_byte(100),
        },
        &[SourceOutcome::Retryable { source_nonce: 0 }],
        true,
        1,
        || true,
    )
    .unwrap();
    assert!(pool.is_empty());
    assert!(!pool.owns(suffix.hash));
    assert!(writer.shutdown(Duration::from_secs(8)));
    let events = events(dir.path());
    let terminals = events
        .iter()
        .filter_map(|(_, event)| match event {
            Event::Terminal {
                member,
                outcome,
                attempt,
                reconciliation,
                ..
            } => Some((
                member.source.transaction_hash,
                *outcome,
                *attempt,
                *reconciliation,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 2);
    assert_eq!(terminals[0].1, Outcome::AttemptsExhausted);
    assert!(terminals[0].2.is_some());
    assert!(terminals[0].3.is_some());
    assert_eq!(terminals[1].0, Hash32(suffix.hash.0));
    assert_eq!(terminals[1].1, Outcome::NonceGap);
    assert!(terminals[1].2.is_none());
    assert!(terminals[1].3.is_none());
}

#[test]
fn missing_capture_or_closed_writer_cannot_prevent_local_disposal() {
    for closed in [false, true] {
        let (dir, pool, publisher, writer) = setup();
        let held = tx(1, Direction::Inbound);
        pool.push_contiguous(held.clone(), 0).unwrap();
        let mut writer = Some(writer);
        if closed {
            assert!(writer.take().unwrap().shutdown(Duration::from_secs(8)));
        } else {
            pool.state
                .lock()
                .unwrap()
                .lifecycle
                .as_mut()
                .unwrap()
                .admissions
                .clear();
        }
        let evicted = pool.evict_chain_at_or_above(held.sender, held.direction, held.nonce);
        assert_eq!(evicted.len(), 1);
        assert!(!pool.owns(held.hash));
        if !closed {
            assert_eq!(publisher.stats().dropped_records, 1);
            assert_eq!(publisher.stats().dropped_sequence_min, Some(2));
            assert!(writer.take().unwrap().shutdown(Duration::from_secs(8)));
        }
        assert_eq!(events(dir.path()).len(), 1);
    }
}

#[test]
fn local_capture_limit_loses_whole_snapshot_without_limiting_protocol_eviction() {
    let (dir, pool, publisher, writer) = setup();
    for index in 0..=MAX_MEMBERS {
        let mut held = tx(1, Direction::Inbound);
        held.nonce = index as u64;
        let mut hash = [0u8; 32];
        hash[24..].copy_from_slice(&(index as u64).to_be_bytes());
        held.hash = B256::from(hash);
        pool.push_contiguous(held, 0).unwrap();
    }
    let before = publisher.stats();
    let evicted = pool.evict_chain_at_or_above(Address::repeat_byte(3), Direction::Inbound, 0);
    assert_eq!(evicted.len(), MAX_MEMBERS + 1);
    assert!(pool.is_empty());
    let after = publisher.stats();
    assert_eq!(
        after.last_assigned_sequence,
        before.last_assigned_sequence + 1
    );
    assert_eq!(after.dropped_records, before.dropped_records + 1);
    assert!(writer.shutdown(Duration::from_secs(8)));
    assert!(
        events(dir.path())
            .iter()
            .all(|(_, event)| !matches!(event, Event::Terminal { .. })),
        "oversized membership must not be partially published"
    );
}

#[test]
fn native_terminal_durations_work_without_lifecycle_and_skip_unchanged_reservations() {
    let (_dir, pool, _, writer) = setup();
    {
        let mut state = pool.state.lock().unwrap();
        state.lifecycle = None;
        state.telemetry.enable_native_for_test();
    }
    let held = tx(1, Direction::Inbound);
    pool.push_contiguous(held.clone(), 0).unwrap();
    let capture = {
        let state = pool.state.lock().unwrap();
        super::super::LocalCapture::before(
            &state,
            [(held.hash, Outcome::PoolDiscarded, held.direction)],
        )
        .unwrap()
    };
    // Queued work is not removed by the in-flight-only API.
    pool.release_in_flight_batch(std::slice::from_ref(&held));
    assert!(
        capture
            .after(&pool.state.lock().unwrap())
            .observations()
            .is_empty()
    );
    let drained = pool.pop_all();
    let capture = {
        let state = pool.state.lock().unwrap();
        super::super::LocalCapture::before(
            &state,
            [(held.hash, Outcome::PoolDiscarded, held.direction)],
        )
        .unwrap()
    };
    pool.release_in_flight_batch(&drained);
    let pending = capture.after(&pool.state.lock().unwrap());
    assert_eq!(pending.observations().len(), 1);
    let observation = &pending.observations()[0];
    assert_eq!(observation.direction, schema::Direction::Inbound);
    assert_eq!(observation.outcome, Outcome::PoolDiscarded);
    assert!(observation.elapsed.is_some());
    assert!(
        observation
            .readiness
            .unwrap()
            .conserves(observation.elapsed.unwrap())
    );
    assert_eq!(
        observation.readiness.unwrap().parts[2],
        observation.elapsed.unwrap()
    );
    let capture = super::super::LocalCapture::before(
        &pool.state.lock().unwrap(),
        [(held.hash, Outcome::PoolDiscarded, held.direction)],
    )
    .unwrap();
    pool.release_in_flight_batch(&drained);
    assert!(
        capture
            .after(&pool.state.lock().unwrap())
            .observations()
            .is_empty()
    );
    assert!(writer.shutdown(Duration::from_secs(8)));
}

#[test]
fn native_local_metrics_cannot_terminate_an_unresolved_canonical_owner() {
    let (_dir, pool, _, writer) = setup();
    pool.state
        .lock()
        .unwrap()
        .telemetry
        .enable_native_for_test();
    let held = tx(1, Direction::Outbound);
    pool.push_contiguous(held.clone(), 0).unwrap();
    let txs = pool.pop_all();
    let _id = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
    pool.state.lock().unwrap().lifecycle = None;
    let capture = super::super::LocalCapture::before(
        &pool.state.lock().unwrap(),
        [(held.hash, Outcome::PoolDiscarded, held.direction)],
    )
    .unwrap();
    pool.release_in_flight_batch(&txs);
    assert!(
        capture
            .after(&pool.state.lock().unwrap())
            .observations()
            .is_empty()
    );
    assert!(writer.shutdown(Duration::from_secs(8)));
}
