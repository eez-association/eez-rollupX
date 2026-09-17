//! Sample active reservations under their existing owner, aggregate bounded data,
//! then release that owner before touching the process recorder.
use super::durations::{Kind, ReadinessClock};
use super::{Direction, HeldPool, QueueTelemetry, SourceState};
use alloy_primitives::{Address, B256};
use eez_telemetry::operational::{EVALUATION_MAX_AGE, MAX_QUEUE_SAMPLE, QueueSnapshot};
use std::{collections::HashMap, time::Instant};

#[derive(Debug, Default)]
pub(super) struct NativeQueue {
    pub admitted_at: HashMap<B256, Instant>,
    pub ready: HashMap<B256, (SourceState, Instant)>,
    pub current: Option<SourceState>,
    pub evaluated_at: Option<Instant>,
    pub(super) clocks: HashMap<B256, ReadinessClock>,
}
impl NativeQueue {
    pub(super) fn admit_clock(
        &mut self,
        hash: B256,
        chain: (Address, Direction, u64),
        now: Instant,
    ) {
        self.clocks.remove(&hash);
        if self.clocks.len() < MAX_QUEUE_SAMPLE {
            self.clocks.insert(hash, ReadinessClock::new(now, chain));
        }
    }
    pub(super) fn invalidate_clocks(&mut self, now: Instant) {
        for clock in self.clocks.values_mut() {
            clock.transition(Kind::Unknown, now);
        }
    }
    pub(super) fn evaluate(&mut self, source: SourceState, now: Instant) {
        if self.current != Some(source) {
            self.invalidate_clocks(now);
        }
        self.current = Some(source);
        self.evaluated_at = Some(now);
    }
    pub(super) fn block_chain(
        &mut self,
        chain: (Address, Direction, u64),
        source: SourceState,
        now: Instant,
        in_flight: &HashMap<B256, (Address, Direction, u64)>,
    ) {
        self.evaluate(source, now);
        for (hash, clock) in &mut self.clocks {
            if clock.chain.0 == chain.0
                && clock.chain.1 == chain.1
                && clock.chain.2 >= chain.2
                && !in_flight.contains_key(hash)
                && self
                    .ready
                    .get(hash)
                    .is_none_or(|(ready_source, _)| *ready_source != source)
            {
                clock.transition(Kind::Blocked, now);
            }
        }
    }
    pub(super) fn unblock_chain(
        &mut self,
        chain: (Address, Direction),
        source: SourceState,
        now: Instant,
    ) {
        self.evaluate(source, now);
        for clock in self.clocks.values_mut() {
            if (clock.chain.0, clock.chain.1) == chain && clock.blocked() {
                clock.transition(Kind::Unknown, now);
            }
        }
    }
}
impl QueueTelemetry {
    #[cfg(test)]
    pub(in crate::held_pool) fn enable_native_for_test(&mut self) {
        self.native = Some(NativeQueue::default());
    }
    pub(in crate::held_pool) fn native_admission_started(&self, hash: B256) -> Option<Instant> {
        self.native.as_ref()?.admitted_at.get(&hash).copied()
    }
    pub(in crate::held_pool) fn native_partition_clock(
        &self,
        hash: B256,
    ) -> Option<ReadinessClock> {
        self.native.as_ref()?.clocks.get(&hash).copied()
    }
    pub(in crate::held_pool) fn recover_native_clock(
        &mut self,
        hash: B256,
        chain: (Address, Direction, u64),
    ) {
        if let Some(native) = &mut self.native
            && !native.clocks.contains_key(&hash)
            && native.clocks.len() < MAX_QUEUE_SAMPLE
            && let Some(origin) = native.admitted_at.get(&hash)
        {
            // A terminal admission reopened by reorg has no retained partition.
            // Keep its original total clock; classify the missing prefix as unknown.
            native
                .clocks
                .insert(hash, ReadinessClock::new(*origin, chain));
        }
    }
    pub(in crate::held_pool) fn native_terminal(
        &self,
        hash: B256,
        direction: Direction,
        outcome: eez_telemetry::schema::Outcome,
    ) -> Option<eez_telemetry::operational::TerminalObservation> {
        self.native.as_ref()?;
        self.native_terminal_at(hash, direction, outcome, Instant::now())
    }
    pub(in crate::held_pool) fn native_terminal_at(
        &self,
        hash: B256,
        direction: Direction,
        outcome: eez_telemetry::schema::Outcome,
        now: Instant,
    ) -> Option<eez_telemetry::operational::TerminalObservation> {
        let native = self.native.as_ref()?;
        let origin = native.admitted_at.get(&hash).copied();
        Some(eez_telemetry::operational::TerminalObservation {
            direction: match direction {
                Direction::Inbound => eez_telemetry::schema::Direction::Inbound,
                Direction::Outbound => eez_telemetry::schema::Direction::Outbound,
            },
            outcome,
            elapsed: origin.and_then(|at| now.checked_duration_since(at)),
            readiness: origin.and_then(|at| native.clocks.get(&hash)?.finish(at, now)),
        })
    }
    pub(in crate::held_pool) fn native_enabled(&self) -> bool {
        self.native.is_some()
    }
    pub(in crate::held_pool) fn native_readiness(
        &mut self,
        hash: B256,
        source: SourceState,
        ready: bool,
    ) {
        let Some(native) = &mut self.native else {
            return;
        };
        let now = Instant::now();
        native.evaluate(source, now);
        if let Some(clock) = native.clocks.get_mut(&hash) {
            clock.transition(
                if ready {
                    Kind::Serviceable
                } else {
                    Kind::Blocked
                },
                now,
            );
        }
        if ready {
            native
                .ready
                .entry(hash)
                .or_insert_with(|| (source, Instant::now()))
                .0 = source;
        } else {
            native.ready.remove(&hash);
        }
    }
}
impl HeldPool {
    pub(crate) fn native_metrics_enabled(&self) -> bool {
        self.state
            .lock()
            .expect("held_pool mutex poisoned")
            .telemetry
            .native
            .is_some()
    }
    /// One fixed-size current-population snapshot. Above the capture bound, every
    /// value becomes unavailable; a truncated population must not look complete.
    pub fn publish_native_queue_metrics(&self) {
        if let Some(snapshot) = self.native_queue_snapshot(Instant::now()) {
            snapshot.publish();
        }
    }
    fn native_queue_snapshot(&self, now: Instant) -> Option<QueueSnapshot> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        let telemetry = &state.telemetry;
        let native = telemetry.native.as_ref()?;
        let mut snapshot = QueueSnapshot::default();
        if state.txs.len().saturating_add(state.in_flight.len()) > MAX_QUEUE_SAMPLE {
            return Some(snapshot);
        }
        snapshot.complete = true;
        snapshot.evaluation_age = native
            .evaluated_at
            .map(|at| now.saturating_duration_since(at));
        let valid = snapshot
            .evaluation_age
            .is_some_and(|age| age <= EVALUATION_MAX_AGE)
            && native.current.is_some();
        for (hash, sender, direction, nonce, in_flight) in state
            .txs
            .iter()
            .map(|tx| (tx.hash, tx.sender, tx.direction, tx.nonce, false))
            .chain(
                state
                    .in_flight
                    .iter()
                    .map(|(hash, (sender, direction, nonce))| {
                        (*hash, *sender, *direction, *nonce, true)
                    }),
            )
        {
            let output = &mut snapshot.directions[match direction {
                Direction::Inbound => 0,
                Direction::Outbound => 1,
            }];
            let ready = native
                .ready
                .get(&hash)
                .filter(|(source, _)| valid && (in_flight || Some(*source) == native.current));
            let blocked = valid
                && telemetry
                    .blocked
                    .get(&(sender, direction))
                    .is_some_and(|blocked| {
                        Some(blocked.source) == native.current && nonce >= blocked.nonce
                    });
            let index = if in_flight {
                3
            } else if ready.is_some() {
                0
            } else if blocked {
                1
            } else {
                2
            };
            output.states[index] += 1;
            let elapsed = native
                .admitted_at
                .get(&hash)
                .and_then(|at| now.checked_duration_since(*at));
            let partition = native
                .admitted_at
                .get(&hash)
                .and_then(|at| native.clocks.get(&hash)?.finish(*at, now));
            if partition.is_none() {
                output.readiness_clock_unknown += 1;
            }
            for (index, values) in output.readiness.iter_mut().enumerate() {
                values.add(partition.map(|parts| parts.parts[index]).or_else(|| {
                    // Missing partition history is not a zero observed wait.
                    // Only its unclassified total can be retained.
                    (index == 2).then_some(elapsed).flatten()
                }));
            }
            output.pending.add(
                native
                    .admitted_at
                    .get(&hash)
                    .map(|at| now.saturating_duration_since(*at)),
            );
            if let Some((_, at)) = ready {
                output
                    .serviceable
                    .add(Some(now.saturating_duration_since(*at)));
            } else if in_flight && valid {
                output.serviceable.add(None);
            }
        }
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeldTx;
    use alloy_primitives::Bytes;
    use eez_l1::L1BlockRef;
    use eez_protocol::SourceCondition;
    use std::time::Duration;

    fn fixture() -> (HeldPool, HeldTx, SourceState) {
        let pool = HeldPool::new(Default::default());
        pool.state.lock().unwrap().telemetry.native = Some(NativeQueue::default());
        let tx = HeldTx {
            raw_tx: Bytes::new(),
            hash: B256::repeat_byte(1),
            sender: Address::repeat_byte(1),
            direction: Direction::Inbound,
            nonce: 0,
            attempts: 0,
            max_fee_per_gas: 10,
            priority_fee_per_gas: 10,
        };
        let source = SourceState {
            l1: L1BlockRef {
                number: 1,
                hash: B256::repeat_byte(2),
            },
            l2: L1BlockRef {
                number: 2,
                hash: B256::repeat_byte(3),
            },
        };
        (pool, tx, source)
    }
    #[test]
    fn active_population_ages_survive_claim_and_recovery_but_not_readmission() {
        let (pool, tx, source) = fixture();
        let start = Instant::now();
        pool.push_contiguous(tx.clone(), 0).unwrap();
        pool.state
            .lock()
            .unwrap()
            .telemetry
            .native
            .as_mut()
            .unwrap()
            .admitted_at
            .insert(tx.hash, start);
        // A corrupt/future wall timestamp affects the compatible RPC only.
        pool.state
            .lock()
            .unwrap()
            .telemetry
            .admitted_at
            .insert(tx.hash, u64::MAX);
        pool.begin_evaluation(source);
        pool.record_blocked(&tx, SourceCondition::Fee, source);
        let sample = pool
            .native_queue_snapshot(start + Duration::from_secs(30))
            .unwrap();
        assert_eq!(sample.directions[0].states, [0, 1, 0, 0]);
        assert!((sample.directions[0].pending.sum - 30.).abs() < f64::EPSILON);
        assert_eq!(sample.directions[0].serviceable.count, 0);
        pool.record_ready(&tx, source);
        let ready = start + Duration::from_secs(20);
        pool.state
            .lock()
            .unwrap()
            .telemetry
            .native
            .as_mut()
            .unwrap()
            .ready
            .insert(tx.hash, (source, ready));
        assert!(pool.claim(&tx));
        let sample = pool
            .native_queue_snapshot(start + Duration::from_secs(30))
            .unwrap();
        assert_eq!(sample.directions[0].states, [0, 0, 0, 1]);
        assert!((sample.directions[0].serviceable.sum - 10.).abs() < f64::EPSILON);
        pool.requeue_composing(vec![tx.clone()]);
        let sample = pool
            .native_queue_snapshot(start + Duration::from_secs(30))
            .unwrap();
        assert_eq!(sample.directions[0].states, [1, 0, 0, 0]);
        assert!((sample.directions[0].pending.sum - 30.).abs() < f64::EPSILON);
        assert_eq!(
            pool.evict_chain_at_or_above(tx.sender, tx.direction, tx.nonce)
                .len(),
            1
        );
        assert_eq!(
            pool.native_queue_snapshot(start + Duration::from_secs(30))
                .unwrap()
                .directions[0]
                .pending
                .count,
            0
        );
        pool.push_contiguous(tx.clone(), 0).unwrap();
        assert!(
            pool.state
                .lock()
                .unwrap()
                .telemetry
                .native
                .as_ref()
                .unwrap()
                .admitted_at[&tx.hash]
                > start
        );
    }
    #[test]
    fn disabled_missing_stale_reorg_and_direction_populations_are_explicit() {
        let disabled = HeldPool::new(Default::default());
        assert!(disabled.native_queue_snapshot(Instant::now()).is_none());
        let (pool, inbound, source) = fixture();
        let mut outbound = inbound.clone();
        outbound.direction = Direction::Outbound;
        outbound.hash = B256::repeat_byte(4);
        pool.push_contiguous(inbound.clone(), 0).unwrap();
        pool.push_contiguous(outbound.clone(), 0).unwrap();
        let snapshot = pool.native_queue_snapshot(Instant::now()).unwrap();
        assert!(!snapshot.evaluation_valid());
        assert_eq!(snapshot.directions[0].states, [0, 0, 1, 0]);
        assert_eq!(snapshot.directions[1].states, [0, 0, 1, 0]);
        let rpc_before = pool.queue_status();
        pool.record_lifecycle_ready(inbound.hash, source);
        assert_eq!(pool.queue_status()["ready"], rpc_before["ready"]);
        let snapshot = pool.native_queue_snapshot(Instant::now()).unwrap();
        assert_eq!(snapshot.directions[0].states, [1, 0, 0, 0]);
        pool.state
            .lock()
            .unwrap()
            .telemetry
            .native
            .as_mut()
            .unwrap()
            .admitted_at
            .remove(&outbound.hash);
        assert_eq!(
            pool.native_queue_snapshot(Instant::now())
                .unwrap()
                .directions[1]
                .pending
                .unknown,
            1
        );
        let snapshot = pool
            .native_queue_snapshot(Instant::now() + Duration::from_secs(61))
            .unwrap();
        assert!(!snapshot.evaluation_valid());
        assert_eq!(snapshot.directions[0].states, [0, 0, 1, 0]);
        pool.invalidate_after_l1(0).unwrap();
        assert!(
            !pool
                .native_queue_snapshot(Instant::now())
                .unwrap()
                .evaluation_valid()
        );
    }
    #[test]
    fn replacement_resets_only_new_admission_and_duplicate_keeps_age() {
        let (pool, tx, _) = fixture();
        pool.push_contiguous(tx.clone(), 0).unwrap();
        let first = pool
            .state
            .lock()
            .unwrap()
            .telemetry
            .native
            .as_ref()
            .unwrap()
            .admitted_at[&tx.hash];
        pool.push_contiguous(tx.clone(), 0).unwrap();
        assert_eq!(
            pool.state
                .lock()
                .unwrap()
                .telemetry
                .native
                .as_ref()
                .unwrap()
                .admitted_at[&tx.hash],
            first
        );
        let mut replacement = tx;
        replacement.hash = B256::repeat_byte(9);
        replacement.max_fee_per_gas = 11;
        replacement.priority_fee_per_gas = 11;
        pool.push_contiguous(replacement.clone(), 0).unwrap();
        let state = pool.state.lock().unwrap();
        let native = state.telemetry.native.as_ref().unwrap();
        assert_eq!(native.admitted_at.len(), 1);
        assert!(native.admitted_at[&replacement.hash] >= first);
    }
    #[tokio::test]
    async fn metrics_only_preparation_classifies_the_original_result_without_lifecycle() {
        use eez_telemetry::schema::PreparationOutcome;
        let (pool, _, _) = fixture();
        assert!(pool.state.lock().unwrap().lifecycle.is_none());
        let sync = eez_l1::L2Endpoint {
            number: 1,
            hash: B256::repeat_byte(5),
        };
        let error = 7u8;
        let classified = std::cell::Cell::new(false);
        let result = pool
            .observe_preparation(
                sync,
                &[],
                std::future::ready(Err::<(Bytes, crate::held_pool::PreparationFacts), _>(error)),
                |actual| {
                    assert_eq!(*actual, error);
                    classified.set(true);
                    PreparationOutcome::ProverFailed
                },
            )
            .await;
        assert_eq!(result.unwrap_err(), error);
        assert!(classified.get());
        let facts = crate::held_pool::PreparationFacts {
            preparation: None,
            encoded_bytes: 5,
            blob_count: 1,
            blob_allowance: 2,
            nonce: 3,
        };
        let (bytes, returned) = pool
            .observe_preparation(
                sync,
                &[],
                std::future::ready(Ok::<_, u8>((Bytes::from_static(b"private"), facts))),
                |_| panic!("success is not a failure"),
            )
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), b"private");
        assert_eq!(
            (
                returned.preparation,
                returned.encoded_bytes,
                returned.blob_count,
                returned.blob_allowance,
                returned.nonce
            ),
            (None, 5, 1, 2, 3)
        );
    }

    #[test]
    fn oversized_snapshot_is_unknown_not_a_truncated_zero() {
        let (pool, tx, _) = fixture();
        let mut state = pool.state.lock().unwrap();
        for nonce in 0..=MAX_QUEUE_SAMPLE {
            let mut tx = tx.clone();
            tx.nonce = nonce as u64;
            state.txs.push_back(tx);
        }
        drop(state);
        let snapshot = pool.native_queue_snapshot(Instant::now()).unwrap();
        assert!(!snapshot.complete);
        assert!(!snapshot.evaluation_valid());
    }

    #[test]
    fn readiness_prefixes_source_changes_and_reopening_preserve_unknown_time() {
        let (_, first, source) = fixture();
        let start = Instant::now();
        let mut native = NativeQueue::default();
        let hashes = [first.hash, B256::repeat_byte(5), B256::repeat_byte(6)];
        let chains = [
            (first.sender, first.direction, 0),
            (first.sender, first.direction, 1),
            (Address::repeat_byte(9), first.direction, 0),
        ];
        for (hash, chain) in hashes.into_iter().zip(chains) {
            native.admitted_at.insert(hash, start);
            native.admit_clock(hash, chain, start);
        }
        native.block_chain(
            chains[0],
            source,
            start + Duration::from_secs(5),
            &HashMap::new(),
        );
        native.unblock_chain(
            (first.sender, first.direction),
            source,
            start + Duration::from_secs(10),
        );
        native
            .clocks
            .get_mut(&hashes[0])
            .unwrap()
            .transition(Kind::Serviceable, start + Duration::from_secs(10));
        let mut next = source;
        next.l1.hash = B256::repeat_byte(7);
        native.evaluate(next, start + Duration::from_secs(15));
        let end = start + Duration::from_secs(20);
        for (hash, seconds) in hashes.into_iter().zip([[5, 5, 10], [5, 0, 15], [0, 0, 20]]) {
            let partition = native.clocks[&hash].finish(start, end).unwrap();
            assert_eq!(partition.parts, seconds.map(Duration::from_secs));
            assert!(partition.conserves(Duration::from_secs(20)));
        }
        let mut telemetry = QueueTelemetry {
            native: Some(native),
            ..Default::default()
        };
        telemetry.native.as_mut().unwrap().clocks.remove(&hashes[0]);
        telemetry.recover_native_clock(hashes[0], chains[0]);
        let reopened = telemetry
            .native_terminal_at(
                hashes[0],
                first.direction,
                eez_telemetry::schema::Outcome::Committed,
                end,
            )
            .unwrap();
        assert_eq!(
            reopened.readiness.unwrap().parts,
            [Duration::ZERO, Duration::ZERO, Duration::from_secs(20)]
        );
        // Internal recovery cannot reset an existing observed partition.
        let before = telemetry
            .native_partition_clock(hashes[1])
            .unwrap()
            .finish(start, end);
        telemetry.recover_native_clock(hashes[1], chains[1]);
        assert_eq!(
            telemetry
                .native_partition_clock(hashes[1])
                .unwrap()
                .finish(start, end),
            before
        );
    }

    #[test]
    fn partition_capture_bound_does_not_remove_total_admission_clock() {
        let (_, first, _) = fixture();
        let start = Instant::now();
        let mut native = NativeQueue::default();
        let mut last = B256::ZERO;
        for n in 0..=MAX_QUEUE_SAMPLE {
            let mut bytes = [0u8; 32];
            bytes[24..].copy_from_slice(&u64::try_from(n).unwrap().to_be_bytes());
            last = B256::from(bytes);
            native.admitted_at.insert(last, start);
            native.admit_clock(
                last,
                (first.sender, first.direction, u64::try_from(n).unwrap()),
                start,
            );
        }
        assert_eq!(native.clocks.len(), MAX_QUEUE_SAMPLE);
        let telemetry = QueueTelemetry {
            native: Some(native),
            ..Default::default()
        };
        let observation = telemetry
            .native_terminal_at(
                last,
                first.direction,
                eez_telemetry::schema::Outcome::Committed,
                start + Duration::from_secs(7),
            )
            .unwrap();
        assert_eq!(observation.elapsed, Some(Duration::from_secs(7)));
        assert!(observation.readiness.is_none());
    }

    #[test]
    fn blocked_prefix_never_reclassifies_ready_or_in_flight_descendants() {
        let (_, first, source) = fixture();
        let start = Instant::now();
        let mut native = NativeQueue::default();
        let hashes = [first.hash, B256::repeat_byte(5), B256::repeat_byte(6)];
        for (nonce, hash) in hashes.into_iter().enumerate() {
            native.admit_clock(hash, (first.sender, first.direction, nonce as u64), start);
        }
        native.evaluate(source, start);
        for hash in &hashes[1..] {
            native.ready.insert(*hash, (source, start));
            native
                .clocks
                .get_mut(hash)
                .unwrap()
                .transition(Kind::Serviceable, start);
        }
        let in_flight = HashMap::from([(hashes[2], (first.sender, first.direction, 2))]);
        native.block_chain(
            (first.sender, first.direction, 0),
            source,
            start + Duration::from_secs(70),
            &in_flight,
        );
        for hash in &hashes[1..] {
            // Expired clock observation alone cannot overturn a validated ready
            // source, or turn an in-flight reservation into queued blocked work.
            assert_eq!(
                native.clocks[hash]
                    .finish(start, start + Duration::from_secs(80))
                    .unwrap()
                    .parts,
                [
                    Duration::ZERO,
                    Duration::from_secs(60),
                    Duration::from_secs(20)
                ]
            );
        }
        let mut next = source;
        next.l1.hash = B256::repeat_byte(7);
        native.block_chain(
            (first.sender, first.direction, 0),
            next,
            start + Duration::from_secs(90),
            &in_flight,
        );
        assert_eq!(
            native.clocks[&hashes[1]]
                .finish(start, start + Duration::from_secs(100))
                .unwrap()
                .parts,
            [
                Duration::from_secs(10),
                Duration::from_secs(60),
                Duration::from_secs(30)
            ]
        );
        assert_eq!(
            native.clocks[&hashes[2]]
                .finish(start, start + Duration::from_secs(100))
                .unwrap()
                .parts,
            [
                Duration::ZERO,
                Duration::from_secs(60),
                Duration::from_secs(40)
            ]
        );
    }
}
