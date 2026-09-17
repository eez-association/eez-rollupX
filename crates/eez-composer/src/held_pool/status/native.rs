//! Sample active reservations under their existing owner, aggregate bounded data,
//! then release that owner before touching the process recorder.
use super::{Direction, HeldPool, QueueTelemetry, SourceState};
use alloy_primitives::B256;
use eez_telemetry::operational::{EVALUATION_MAX_AGE, MAX_QUEUE_SAMPLE, QueueSnapshot};
use std::{collections::HashMap, time::Instant};

#[derive(Debug, Default)]
pub(super) struct NativeQueue {
    pub admitted_at: HashMap<B256, Instant>,
    pub ready: HashMap<B256, (SourceState, Instant)>,
    pub current: Option<SourceState>,
    pub evaluated_at: Option<Instant>,
}
impl QueueTelemetry {
    #[cfg(test)]
    pub(in crate::held_pool) fn enable_native_for_test(&mut self) {
        self.native = Some(NativeQueue::default());
    }
    pub(in crate::held_pool) fn native_admission_started(&self, hash: B256) -> Option<Instant> {
        self.native.as_ref()?.admitted_at.get(&hash).copied()
    }
    pub(in crate::held_pool) fn native_terminal(
        &self,
        hash: B256,
        direction: Direction,
        outcome: eez_telemetry::schema::Outcome,
    ) -> Option<eez_telemetry::operational::TerminalObservation> {
        let native = self.native.as_ref()?;
        Some(eez_telemetry::operational::TerminalObservation {
            direction: match direction {
                Direction::Inbound => eez_telemetry::schema::Direction::Inbound,
                Direction::Outbound => eez_telemetry::schema::Direction::Outbound,
            },
            outcome,
            elapsed: native.admitted_at.get(&hash).map(Instant::elapsed),
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
        native.current = Some(source);
        native.evaluated_at = Some(Instant::now());
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
    use alloy_primitives::{Address, Bytes};
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
}
