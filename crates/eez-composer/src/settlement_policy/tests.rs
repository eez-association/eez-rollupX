use super::*;

fn scheduling(input: PolicySnapshot) -> SchedulingSnapshot {
    SchedulingSnapshot {
        anchor: input.anchor,
        target_l2_number: input.candidate.number,
        target_l1_timestamp: input.target_l1_timestamp,
        latest_pure_block: input.latest_pure_block,
        estimated_stream_bytes: input.estimated_stream_bytes,
        window_limit_due: input.window_limit_due,
        effective_blobs: input.effective_blobs,
        usable_bytes_per_blob: input.usable_bytes_per_blob,
    }
}

#[test]
fn scheduling_shares_final_triggers_without_creating_or_extending_drain() {
    for (cfg, input, reason) in [
        (config(None, 6, None), snapshot(6), DueReasons::GENERAL),
        (
            config(None, 100, None),
            PolicySnapshot {
                latest_pure_block: Some(endpoint(3)),
                ..snapshot(6)
            },
            DueReasons::PURE_ALWAYS,
        ),
        (
            config(Some(12000), 100, None),
            PolicySnapshot {
                latest_pure_block: Some(endpoint(3)),
                ..snapshot(6)
            },
            DueReasons::PURE_INTERVAL,
        ),
        (
            config(None, 100, Some(5000)),
            PolicySnapshot {
                estimated_stream_bytes: Some(500),
                effective_blobs: 1,
                usable_bytes_per_blob: 1000,
                ..snapshot(6)
            },
            DueReasons::FULLNESS,
        ),
        (
            config(None, 100, None),
            PolicySnapshot {
                window_limit_due: true,
                estimated_stream_bytes: None,
                ..snapshot(6)
            },
            DueReasons::WINDOW_LIMIT,
        ),
    ] {
        let mut state = PolicyState::default();
        let before = state.clone();
        let planned = state.preparation_reasons(&cfg, scheduling(input)).unwrap();
        assert!(planned.contains(reason), "{planned:?}");
        assert_eq!(state, before);
        assert_eq!(planned, state.evaluate(&cfg, input).unwrap().reasons);
        let with_drain = state.clone();
        let later = PolicySnapshot {
            candidate: endpoint(12),
            latest_pure_block: None,
            ..input
        };
        assert!(
            state
                .preparation_reasons(&config(Some(999_000), 999, None), scheduling(later))
                .unwrap()
                .contains(DueReasons::DRAIN)
        );
        assert_eq!(state, with_drain);
    }
}

#[test]
fn first_late_activity_can_change_final_due_without_mutating_the_earlier_hint() {
    let mut state = PolicyState::default();
    let cfg = config(None, 100, None);
    assert!(
        state
            .preparation_reasons(&cfg, scheduling(snapshot(6)))
            .unwrap()
            .is_empty()
    );
    let late = PolicySnapshot {
        latest_pure_block: Some(endpoint(5)),
        ..snapshot(6)
    };
    assert!(
        state
            .evaluate(&cfg, late)
            .unwrap()
            .reasons
            .contains(DueReasons::PURE_ALWAYS)
    );
    let before = state.clone();
    let invalid = SchedulingSnapshot {
        effective_blobs: 0,
        ..scheduling(late)
    };
    assert!(state.preparation_reasons(&cfg, invalid).is_err());
    assert_eq!(state, before);
}

fn byte_summary(size: usize) -> eez_payload_codec::v0::PayloadSizer {
    eez_payload_codec::v0::PayloadSizer::run(&eez_payload_codec::v0::BlockRun {
        blocks: 1,
        beneficiary: Default::default(),
        extra_data: &[],
        transactions: &[vec![0; size].into()],
    })
    .unwrap()
}

#[test]
fn canonical_publication_and_reorg_restore_the_original_timer_and_rebuild_pending_activity() {
    use super::index::{ActivityIndex, BlockActivity};
    use eez_l1::{ReconciledBatchSettlement, ReplaySelection, l1_canonical_head::L1CanonicalHead};
    let head = L1CanonicalHead::default();
    let occurrence = |l1_number, l2_number, timestamp| {
        let mut anchor = anchor(l2_number, timestamp);
        anchor.l1.number = l1_number;
        anchor.l1.hash = B256::with_last_byte(l1_number as u8);
        anchor.tx_hash = anchor.l1.hash;
        anchor
    };
    let initial = occurrence(10, 2, 1000);
    let intermediate = occurrence(11, 4, 1020);
    let completed = occurrence(12, 6, 1025);
    let publish = |anchor: SettlementAnchor| {
        head.publish(ReconciledBatchSettlement {
            l1_block: anchor.l1,
            tx_hash: anchor.tx_hash,
            tx_index: anchor.tx_index,
            calldata_hash: B256::ZERO,
            applied_effects: 0..1,
            anchor_applied: true,
            endpoint: Some(anchor.endpoint),
            selection: ReplaySelection::default(),
        });
        head.mark_scanned(anchor.l1, anchor.endpoint);
    };
    let canonical_anchor = || {
        let snapshot = head.policy_anchor_snapshot();
        assert!(snapshot.through.is_some());
        let commitment = snapshot.commitment.unwrap();
        let anchor = [initial, intermediate, completed]
            .into_iter()
            .find(|anchor| anchor.l1 == commitment.l1)
            .unwrap();
        assert_eq!(commitment.endpoint, anchor.endpoint);
        anchor
    };
    let mut index = ActivityIndex::default();
    let mut policy = PolicyState::default();
    let cfg = config(Some(20_000), 150, None);
    let load = |hash: B256| {
        let number = u64::from(hash.as_slice()[31]);
        Ok(BlockActivity {
            endpoint: endpoint(number),
            parent_hash: endpoint(number - 1).hash,
            ordinary: byte_summary(700),
            pure_transactions: u64::from(number == 4),
        })
    };
    let evaluate = |policy: &mut PolicyState, index: &ActivityIndex, height, time, coverage| {
        let input = PolicySnapshot {
            anchor: canonical_anchor(),
            candidate: endpoint(height),
            target_l1_timestamp: time,
            latest_pure_block: index.latest_pure(),
            estimated_stream_bytes: Some(
                index
                    .estimated_stream_bytes_with_overhead(&byte_summary(700), Default::default())
                    .unwrap(),
            ),
            drain_coverage: coverage,
            ..snapshot(height)
        };
        policy.evaluate(&cfg, input).unwrap()
    };
    publish(initial);
    index.update(initial.endpoint, endpoint(5), load).unwrap();
    assert_eq!(index.pending_pure_transactions().unwrap(), 1);
    assert!(evaluate(&mut policy, &index, 6, 1020, DrainCoverage::Pending).is_due());
    publish(intermediate);
    index
        .update(intermediate.endpoint, endpoint(6), load)
        .unwrap();
    assert_eq!(index.pending_pure_transactions().unwrap(), 0);
    assert_eq!(
        evaluate(&mut policy, &index, 7, 1025, DrainCoverage::Pending).required_endpoint,
        Some(endpoint(6)),
        "an intermediate commitment retains only the original finite drain"
    );
    publish(completed);
    index.update(completed.endpoint, endpoint(7), load).unwrap();
    assert!(!evaluate(&mut policy, &index, 8, 1028, DrainCoverage::Covered).is_due());

    let obsolete = head.policy_anchor_snapshot();
    assert_eq!(head.retreat_on_l1_reorg(10).0, 2);
    assert!(!head.generation_is(obsolete.generation));
    assert!(
        head.policy_anchor_snapshot().through.is_none(),
        "an incomplete reorg scan cannot authorize a candidate"
    );
    head.mark_scanned(
        L1BlockRef {
            number: 11,
            hash: B256::repeat_byte(0xaa),
        },
        initial.endpoint,
    );
    assert_eq!(
        canonical_anchor().l1_timestamp,
        1000,
        "reorg restores the original time, not the replacement block's time"
    );
    index.update(initial.endpoint, endpoint(7), load).unwrap();
    assert_eq!(index.pending_pure_transactions().unwrap(), 1);
    assert!(evaluate(&mut policy, &index, 8, 1028, DrainCoverage::Replaced).is_due());

    // The replacement L2 branch contains none of the formerly pending users.
    // Hash-based cache and intent invalidation must remove that obligation.
    let replacement = |number| L2Endpoint {
        number,
        hash: B256::with_last_byte(100 + number as u8),
    };
    index
        .update(initial.endpoint, replacement(7), |hash| {
            let number = u64::from(hash.as_slice()[31] - 100);
            Ok(BlockActivity {
                endpoint: replacement(number),
                parent_hash: if number == 3 {
                    initial.endpoint.hash
                } else {
                    replacement(number - 1).hash
                },
                ordinary: byte_summary(650),
                pure_transactions: 0,
            })
        })
        .unwrap();
    let input = PolicySnapshot {
        anchor: canonical_anchor(),
        candidate: replacement(8),
        target_l1_timestamp: 1030,
        latest_pure_block: index.latest_pure(),
        drain_coverage: DrainCoverage::Replaced,
        ..snapshot(8)
    };
    assert!(!policy.evaluate(&cfg, input).unwrap().is_due());
    assert_eq!(index.pending_pure_transactions().unwrap(), 0);
    assert!(policy.drain().is_none());
}

fn endpoint(number: u64) -> L2Endpoint {
    L2Endpoint {
        number,
        hash: B256::with_last_byte(u8::try_from(number % 256).unwrap()),
    }
}

fn anchor(number: u64, timestamp: u64) -> SettlementAnchor {
    SettlementAnchor {
        l1: L1BlockRef {
            number: timestamp,
            hash: B256::with_last_byte(1),
        },
        l1_timestamp: timestamp,
        tx_hash: B256::with_last_byte(2),
        tx_index: 3,
        endpoint: endpoint(number),
    }
}

fn config(interval_ms: Option<u64>, blocks: u64, fullness: Option<u16>) -> PolicyConfig {
    PolicyConfig {
        pure_l2: interval_ms.map_or(PureL2Mode::Always, |milliseconds| PureL2Mode::Interval {
            milliseconds: std::num::NonZeroU64::new(milliseconds).unwrap(),
        }),
        max_unsettled_l2_blocks: std::num::NonZeroU64::new(blocks).unwrap(),
        blob_fullness_bps: fullness.map(|value| std::num::NonZeroU16::new(value).unwrap()),
        inactive_interval_ms: None,
        max_speculative_depth: 0,
    }
}

fn snapshot(height: u64) -> PolicySnapshot {
    PolicySnapshot {
        anchor: anchor(0, 1000),
        candidate: endpoint(height),
        target_l1_timestamp: 1012,
        latest_pure_block: None,
        cross_chain: CrossChainWork::None,
        estimated_stream_bytes: Some(0),
        window_limit_due: false,
        effective_blobs: 10,
        usable_bytes_per_blob: eez_blob_codec::DATA_BYTES_PER_BLOB.try_into().unwrap(),
        drain_coverage: DrainCoverage::Pending,
    }
}

#[test]
fn idle_window_limit_requires_posting_without_pure_activity_or_blob_fullness() {
    let cfg = config(None, 100_000, None);
    let mut policy = PolicyState::default();
    let mut input = snapshot(65_532);
    input.window_limit_due = true;
    let decision = policy.evaluate(&cfg, input).unwrap();
    assert!(decision.is_due());
    assert!(decision.reasons.contains(DueReasons::WINDOW_LIMIT));
    assert!(!decision.reasons.contains(DueReasons::GENERAL));
    assert!(!decision.reasons.contains(DueReasons::FULLNESS));
    assert_eq!(decision.required_endpoint, Some(input.candidate));
    // A stalled/restarted node with a larger backlog still acquires a finite
    // drain obligation; the composer must select a supported historical prefix.
    input.candidate = endpoint(70_002);
    input.estimated_stream_bytes = None;
    assert!(policy.evaluate(&cfg, input).unwrap().is_due());
    input.window_limit_due = false;
    let before = policy.clone();
    assert!(policy.evaluate(&cfg, input).is_err());
    assert_eq!(policy, before);
}

#[test]
fn empty_sync_and_mempool_absence_do_not_imply_settlement() {
    let cfg = config(None, 150, None);
    for h in [0, 1, 12, 149] {
        assert!(
            !PolicyState::default()
                .evaluate(&cfg, snapshot(h))
                .unwrap()
                .is_due()
        );
    }
    assert!(
        PolicyState::default()
            .evaluate(&cfg, snapshot(150))
            .unwrap()
            .reasons
            .contains(DueReasons::GENERAL)
    );
}

#[test]
fn intervening_pure_activity_is_due_always_even_with_empty_terminal() {
    let mut input = snapshot(12);
    input.latest_pure_block = Some(endpoint(3));
    let decision = PolicyState::default()
        .evaluate(&config(None, 150, None), input)
        .unwrap();
    assert_eq!(decision.reasons, DueReasons::PURE_ALWAYS);
    assert_eq!(decision.required_endpoint, Some(endpoint(12)));
}

#[test]
fn interval_uses_latest_l1_settlement_and_requires_pure_activity() {
    let cfg = config(Some(900_000), 10_000, None);
    let mut input = snapshot(12);
    for (target, pure, due) in [
        (1899, true, false),
        (1900, true, true),
        (1901, true, true),
        (1901, false, false),
    ] {
        input.target_l1_timestamp = target;
        input.latest_pure_block = pure.then(|| endpoint(3));
        assert_eq!(
            PolicyState::default()
                .evaluate(&cfg, input)
                .unwrap()
                .is_due(),
            due
        );
    }
    // An earlier empty or cross-chain consolidation resets the real timer.
    input.anchor = anchor(2, 1890);
    input.latest_pure_block = Some(endpoint(3));
    assert!(
        !PolicyState::default()
            .evaluate(&cfg, input)
            .unwrap()
            .is_due()
    );
}

#[test]
fn timer_rounds_neither_target_nor_interval_to_l1_slots() {
    let cfg = config(Some(12_001), 150, None);
    let mut input = snapshot(12);
    input.latest_pure_block = Some(endpoint(1));
    assert!(
        !PolicyState::default()
            .evaluate(&cfg, input)
            .unwrap()
            .is_due()
    );
    input.target_l1_timestamp += 1;
    assert!(
        PolicyState::default()
            .evaluate(&cfg, input)
            .unwrap()
            .is_due()
    );
}

#[test]
fn general_wins_on_both_grids_without_rounding_n() {
    let cfg = config(Some(900_000), 150, None);
    for (k, l2_seconds) in [(6u64, 2u64), (12, 1), (5, 1)] {
        let first = 150u64.div_ceil(k) * k;
        for h in (k..=first).step_by(usize::try_from(k).unwrap()) {
            let mut input = snapshot(h);
            input.target_l1_timestamp = 1000 + h * l2_seconds;
            let decision = PolicyState::default().evaluate(&cfg, input).unwrap();
            assert_eq!(decision.is_due(), h >= 150);
        }
        assert_eq!(cfg.max_unsettled_l2_blocks.get(), 150);
    }
}

#[test]
fn simultaneous_rules_make_one_decision_and_cross_chain_ignores_timers() {
    let cfg = config(None, 12, Some(1000));
    let mut input = snapshot(12);
    input.latest_pure_block = Some(endpoint(3));
    input.cross_chain = CrossChainWork::Executable;
    input.estimated_stream_bytes = Some(126_976);
    let decision = PolicyState::default().evaluate(&cfg, input).unwrap();
    for reason in [
        DueReasons::GENERAL,
        DueReasons::PURE_ALWAYS,
        DueReasons::CROSS_CHAIN,
        DueReasons::FULLNESS,
    ] {
        assert!(decision.reasons.contains(reason));
    }
    assert_eq!(decision.required_endpoint, Some(endpoint(12)));
    input.latest_pure_block = None;
    input.estimated_stream_bytes = Some(0);
    let decision = PolicyState::default()
        .evaluate(&config(Some(900_000), 150, None), input)
        .unwrap();
    assert_eq!(decision.reasons, DueReasons::CROSS_CHAIN);
}

#[test]
fn fullness_is_aggregate_integer_soft_trigger_including_empty_history() {
    let cfg = config(None, 10_000, Some(1000));
    for (blobs, bytes, due) in [
        (10, 126_975, false),
        (10, 126_976, true),
        (6, 76_185, false),
        (6, 76_186, true),
        (10, 152_372, true),
    ] {
        let mut input = snapshot(12);
        input.effective_blobs = blobs;
        input.estimated_stream_bytes = Some(bytes);
        let decision = PolicyState::default().evaluate(&cfg, input).unwrap();
        assert_eq!(decision.is_due(), due);
        assert_eq!(decision.required_endpoint, due.then(|| endpoint(12)));
    }
    let mut input = snapshot(12);
    input.estimated_stream_bytes = Some(u64::MAX);
    assert!(
        !PolicyState::default()
            .evaluate(&config(None, 150, None), input)
            .unwrap()
            .is_due()
    );
    input.candidate = endpoint(0);
    assert!(
        !PolicyState::default()
            .evaluate(&cfg, input)
            .unwrap()
            .is_due()
    );
}

#[test]
fn failed_or_delayed_submission_does_not_extend_periodic_drain() {
    let cfg = config(Some(900_000), 150, None);
    let mut state = PolicyState::default();
    state.evaluate(&cfg, snapshot(150)).unwrap();
    for h in [156, 168, 300] {
        let decision = state.evaluate(&cfg, snapshot(h)).unwrap();
        assert_eq!(decision.required_endpoint, Some(endpoint(150)));
        assert!(decision.reasons.contains(DueReasons::DRAIN));
    }
    // The late receipt/observer cannot clear it. Only canonical coverage can.
    let mut input = snapshot(168);
    input.anchor = anchor(150, 1012);
    input.target_l1_timestamp = 1024;
    input.drain_coverage = DrainCoverage::Covered;
    assert!(!state.evaluate(&cfg, input).unwrap().is_due());
    assert_eq!(state.drain(), None);
}

#[test]
fn capacity_prefixes_drain_without_restarting_timer_or_extending_target() {
    let cfg = config(Some(900_000), 10_000, None);
    let mut state = PolicyState::default();
    let mut input = snapshot(300);
    input.target_l1_timestamp = 1900;
    input.latest_pure_block = Some(endpoint(1));
    state.evaluate(&cfg, input).unwrap();
    input.anchor = anchor(100, 1900);
    input.target_l1_timestamp = 1912;
    input.candidate = endpoint(312);
    input.latest_pure_block = None;
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(300))
    );
    // Even a new general threshold does not replace the unfinished endpoint.
    assert_eq!(
        state
            .evaluate(&config(Some(900_000), 150, None), input)
            .unwrap()
            .required_endpoint,
        Some(endpoint(300))
    );
    // Restart intentionally loses this obligation and uses the new anchor.
    assert!(
        !PolicyState::default()
            .evaluate(&cfg, input)
            .unwrap()
            .is_due()
    );
}

#[test]
fn newer_timed_activity_uses_new_anchor_after_original_drain_completes() {
    let cfg = config(Some(900_000), 10_000, None);
    let mut state = PolicyState::default();
    let mut input = snapshot(150);
    input.target_l1_timestamp = 1900;
    input.latest_pure_block = Some(endpoint(3));
    state.evaluate(&cfg, input).unwrap();
    input.candidate = endpoint(162);
    input.latest_pure_block = Some(endpoint(155));
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(150))
    );
    input.anchor = anchor(150, 1912);
    input.target_l1_timestamp = 1924;
    input.drain_coverage = DrainCoverage::Covered;
    assert!(!state.evaluate(&cfg, input).unwrap().is_due());
    input.target_l1_timestamp = 2812;
    assert!(state.evaluate(&cfg, input).unwrap().is_due());
}

#[test]
fn always_activity_extends_only_for_new_obligations() {
    let cfg = config(None, 10_000, None);
    let mut state = PolicyState::default();
    let mut input = snapshot(12);
    input.latest_pure_block = Some(endpoint(3));
    state.evaluate(&cfg, input).unwrap();
    input.candidate = endpoint(24);
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(12))
    );
    input.latest_pure_block = Some(endpoint(17));
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(24))
    );
}

#[test]
fn cross_chain_predecessor_is_finite_and_does_not_include_unrelated_tail() {
    let cfg = config(Some(900_000), 10_000, None);
    let mut input = snapshot(36);
    input.cross_chain = CrossChainWork::Predecessor(endpoint(24));
    assert_eq!(
        PolicyState::default()
            .evaluate(&cfg, input)
            .unwrap()
            .required_endpoint,
        Some(endpoint(24))
    );
    for height in [0, 37] {
        input.cross_chain = CrossChainWork::Predecessor(endpoint(height));
        assert!(PolicyState::default().evaluate(&cfg, input).is_err());
    }
}

#[test]
fn invalidated_cross_chain_work_never_leaves_an_orphan_periodic_drain() {
    let cfg = config(Some(900_000), 150, None);
    let mut state = PolicyState::default();
    let mut input = snapshot(36);
    input.cross_chain = CrossChainWork::Predecessor(endpoint(24));
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(24))
    );
    input.cross_chain = CrossChainWork::None;
    assert!(!state.evaluate(&cfg, input).unwrap().is_due());

    input = snapshot(150);
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(150))
    );
    input = snapshot(180);
    input.anchor.endpoint = endpoint(60); // an intermediate prefix, not the drain end
    input.cross_chain = CrossChainWork::Predecessor(endpoint(168));
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(168))
    );
    input.cross_chain = CrossChainWork::None;
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().required_endpoint,
        Some(endpoint(150)),
        "invalidating a held group must neither cancel nor extend the periodic obligation"
    );
}

#[test]
fn restart_uses_current_settings_and_never_process_start_time() {
    let mut input = snapshot(12);
    input.latest_pure_block = Some(endpoint(3));
    input.target_l1_timestamp = 1900;
    assert!(
        PolicyState::default()
            .evaluate(&config(Some(900_000), 150, None), input)
            .unwrap()
            .is_due()
    );
    assert!(
        !PolicyState::default()
            .evaluate(&config(Some(1_800_000), 150, None), input)
            .unwrap()
            .is_due()
    );
    input.latest_pure_block = None;
    input.estimated_stream_bytes = Some(100_000);
    input.effective_blobs = 6;
    assert!(
        PolicyState::default()
            .evaluate(&config(None, 150, Some(1000)), input)
            .unwrap()
            .is_due()
    );
    input.effective_blobs = 10;
    assert!(
        !PolicyState::default()
            .evaluate(&config(None, 150, Some(1000)), input)
            .unwrap()
            .is_due()
    );
}

#[test]
fn changing_allowance_does_not_cancel_existing_drain() {
    let cfg = config(None, 150, Some(1000));
    let mut state = PolicyState::default();
    let mut input = snapshot(12);
    input.effective_blobs = 6;
    input.estimated_stream_bytes = Some(100_000);
    state.evaluate(&cfg, input).unwrap();
    input.effective_blobs = 10;
    assert_eq!(
        state.evaluate(&cfg, input).unwrap().reasons,
        DueReasons::DRAIN
    );
}

#[test]
fn reorged_endpoint_is_recomputed_and_same_height_hash_is_not_coverage() {
    let cfg = config(None, 150, None);
    let mut state = PolicyState::default();
    let mut input = snapshot(150);
    state.evaluate(&cfg, input).unwrap();
    input.candidate.hash = B256::with_last_byte(255);
    input.drain_coverage = DrainCoverage::Replaced;
    assert_eq!(
        state
            .evaluate(&cfg, input)
            .unwrap()
            .required_endpoint
            .unwrap()
            .hash,
        input.candidate.hash
    );
    // Cursor equality alone does not remove an unreconciled obligation.
    input.anchor = anchor(150, 1012);
    input.anchor.endpoint.hash = B256::with_last_byte(254);
    input.drain_coverage = DrainCoverage::Pending;
    assert!(state.evaluate(&cfg, input).unwrap().is_due());
}

#[test]
fn invalid_or_overflowing_snapshot_is_atomic_and_never_capacity_signal() {
    let cfg = config(Some(u64::MAX), 150, Some(10_000));
    let base = snapshot(150);
    let mut state = PolicyState::default();
    state.evaluate(&config(None, 150, None), base).unwrap();
    let before = state.clone();
    for input in [
        PolicySnapshot {
            effective_blobs: 0,
            ..base
        },
        PolicySnapshot {
            usable_bytes_per_blob: 0,
            ..base
        },
        PolicySnapshot {
            target_l1_timestamp: u64::MAX,
            ..base
        },
        PolicySnapshot {
            anchor: anchor(0, u64::MAX),
            ..base
        },
        PolicySnapshot {
            target_l1_timestamp: 999,
            ..base
        },
        PolicySnapshot {
            anchor: anchor(151, 1000),
            ..base
        },
        PolicySnapshot {
            latest_pure_block: Some(endpoint(0)),
            ..base
        },
        PolicySnapshot {
            latest_pure_block: Some(endpoint(151)),
            ..base
        },
        PolicySnapshot {
            latest_pure_block: Some(endpoint(1)),
            ..base
        },
        PolicySnapshot {
            effective_blobs: u64::MAX,
            usable_bytes_per_blob: u64::MAX,
            ..base
        },
    ] {
        assert!(state.evaluate(&cfg, input).is_err());
        assert_eq!(state, before);
    }
}

#[test]
fn truth_table_is_deterministic_across_independent_rule_combinations() {
    for interval in [None, Some(100_000)] {
        for general in [false, true] {
            for elapsed in [false, true] {
                for pure in [false, true] {
                    for cross in [false, true] {
                        for full in [false, true] {
                            let cfg = config(interval, 150, Some(1000));
                            let mut input = snapshot(if general { 150 } else { 12 });
                            input.target_l1_timestamp = if elapsed { 1100 } else { 1099 };
                            input.latest_pure_block = pure.then(|| endpoint(1));
                            input.cross_chain = if cross {
                                CrossChainWork::Executable
                            } else {
                                CrossChainWork::None
                            };
                            input.estimated_stream_bytes =
                                Some(if full { 126_976 } else { 126_975 });
                            let first = PolicyState::default().evaluate(&cfg, input).unwrap();
                            let second = PolicyState::default().evaluate(&cfg, input).unwrap();
                            assert_eq!(first, second);
                            assert_eq!(
                                first.is_due(),
                                general
                                    || cross
                                    || full
                                    || (pure && (interval.is_none() || elapsed))
                            );
                        }
                    }
                }
            }
        }
    }
}
