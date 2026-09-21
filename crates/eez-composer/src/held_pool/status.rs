//! Bounded observations of existing queue/accounting state. These fields never
//! authorize execution and are discarded on restart along with the held pool.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

use alloy_primitives::{Address, B256};
use eez_l1::L1BlockRef;
use eez_protocol::SourceCondition;
use serde_json::{Value, json};

use super::{
    Direction, HeldPool, HeldTx, OptimisticallyIncluded, PoolState, TerminalReason, UserLocation,
};

/// Bound diagnostic history independently of the live pool and settlement ledger.
pub(super) const MAX_RECENT_REMOVALS: usize = 1024;

#[derive(Debug)]
struct LocalRemoval {
    hash: B256,
    sender: Address,
    direction: Direction,
    nonce: u64,
    reason: TerminalReason,
    replacement: Option<B256>,
    observed_at_ms: u64,
}

/// Exact states used for readiness evaluation, independent of wall-clock age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceState {
    pub l1: L1BlockRef,
    pub l2: L1BlockRef,
}

impl SourceState {
    fn json(self) -> Value {
        json!({"l1": {"number": self.l1.number, "hash": self.l1.hash},
            "l2": {"number": self.l2.number, "hash": self.l2.hash}})
    }
}

#[derive(Clone, Debug)]
struct Blocked {
    hash: B256,
    nonce: u64,
    condition: SourceCondition,
    source: SourceState,
    observed_at_ms: u64,
}

#[derive(Default, Debug)]
pub(super) struct QueueTelemetry {
    admitted_at: HashMap<B256, u64>,
    blocked: HashMap<(Address, Direction), Blocked>,
    ready: HashMap<B256, (SourceState, u64)>,
    current: Option<SourceState>,
    evaluated_at_ms: Option<u64>,
    completed: HashMap<(B256, u64), L1BlockRef>,
    last_completion: Option<(L1BlockRef, u64)>,
    completion_observations: [u64; 2],
    // Observations only: never consulted for admission, ownership or recovery.
    recent_removals: VecDeque<LocalRemoval>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Inbound => "inbound",
        Direction::Outbound => "outbound",
    }
}

impl QueueTelemetry {
    pub fn admitted(&mut self, hash: B256) {
        self.recent_removals.retain(|removed| removed.hash != hash);
        self.admitted_at.insert(hash, now_ms());
        self.ready.remove(&hash);
    }

    pub fn removed(
        &mut self,
        hash: B256,
        identity: (Address, Direction, u64),
        reason: TerminalReason,
        replacement: Option<B256>,
    ) {
        self.recent_removals.retain(|removed| removed.hash != hash);
        if self.recent_removals.len() == MAX_RECENT_REMOVALS {
            self.recent_removals.pop_front();
        }
        self.recent_removals.push_back(LocalRemoval {
            hash,
            sender: identity.0,
            direction: identity.1,
            nonce: identity.2,
            reason,
            replacement,
            observed_at_ms: now_ms(),
        });
    }

    fn removal_status(&self, hash: B256) -> Option<Value> {
        let removed = self
            .recent_removals
            .iter()
            .rev()
            .find(|removed| removed.hash == hash)?;
        Some(json!({
            "hash": hash, "sender": removed.sender, "direction": direction_name(removed.direction),
            "nonce": removed.nonce, "lifecycle": "terminal", "ownerGeneration": null,
            "ownership": {"state": "terminal", "reason": format!("{:?}", removed.reason),
                "canonical": null, "observedAtMs": removed.observed_at_ms,
                "replacementHash": removed.replacement},
            "blocking": null,
        }))
    }

    pub fn retain(
        &mut self,
        queued: &HashSet<B256>,
        in_flight: &HashMap<B256, (Address, Direction, u64)>,
        ledger: &OptimisticallyIncluded,
    ) {
        ledger.read(|data| {
            // Once an identity has live or authoritative ownership, its older
            // local observation must never resurface after finality pruning.
            self.recent_removals.retain(|removed| {
                !queued.contains(&removed.hash)
                    && !in_flight.contains_key(&removed.hash)
                    && !data.users.contains_key(&removed.hash)
            });
            self.admitted_at.retain(|hash, _| {
                queued.contains(hash)
                    || in_flight.contains_key(hash)
                    || data.users.contains_key(hash)
            });
            self.ready
                .retain(|hash, _| queued.contains(hash) || in_flight.contains_key(hash));
            self.blocked.retain(|_, blocked| {
                queued.contains(&blocked.hash) || in_flight.contains_key(&blocked.hash)
            });
            self.completed.retain(|(hash, generation), _| {
                data.users.get(hash).is_some_and(|u| u.owner == *generation)
            });
        });
    }

    pub fn observe_completion(&mut self, ledger: &OptimisticallyIncluded) {
        ledger.read(|data| {
            for (hash, user) in &data.users {
                if let UserLocation::Terminal {
                    reason: TerminalReason::Committed,
                    canonical: Some(block),
                } = user.location
                    && self.completed.insert((*hash, user.owner), block).is_none()
                {
                    let index = usize::from(user.tx.direction == Direction::Outbound);
                    self.completion_observations[index] =
                        self.completion_observations[index].saturating_add(1);
                    if self
                        .last_completion
                        .is_none_or(|(previous, _)| block.number >= previous.number)
                    {
                        self.last_completion = Some((block, now_ms()));
                    }
                }
            }
        });
    }

    pub fn invalidate_after_l1(&mut self, ancestor: u64) {
        if self
            .last_completion
            .is_some_and(|(block, _)| block.number > ancestor)
        {
            self.last_completion = None;
        }
        // Consecutive reorgs may move below an earlier surviving completion
        // while last_completion is already unknown.
        self.completed.retain(|_, block| block.number <= ancestor);
        self.current = None;
        self.ready.clear();
    }
}

impl HeldPool {
    /// Consistent queue observations, shared by Prometheus and the queue RPC.
    ///
    /// Readiness refers to the last evaluated source state, not the time of
    /// this read. The scan only reads existing telemetry under the pool lock.
    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.state
            .lock()
            .expect("held_pool mutex poisoned")
            .queue_snapshot()
    }

    pub(crate) fn begin_evaluation(&self, source: SourceState) {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        state.telemetry.current = Some(source);
        state.telemetry.evaluated_at_ms = Some(now_ms());
    }

    pub(crate) fn record_blocked(
        &self,
        tx: &HeldTx,
        condition: SourceCondition,
        source: SourceState,
    ) {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let key = (tx.sender, tx.direction);
        let changed = state
            .telemetry
            .blocked
            .get(&key)
            .is_none_or(|old| old.hash != tx.hash || old.condition != condition);
        if changed {
            let owner = state
                .ledger
                .read(|data| data.users.get(&tx.hash).map(|u| u.owner));
            tracing::info!(name: "eez.held_pool.source_blocked",
                event_name = "eez.held_pool.source_blocked", tx_hash = %tx.hash,
                sender = %tx.sender, nonce = tx.nonce, direction = ?tx.direction,
                ?owner, reason = %condition, l1_hash = %source.l1.hash, l2_hash = %source.l2.hash,
                "source deferred; independent senders remain eligible");
        }
        state.telemetry.ready.remove(&tx.hash);
        state.telemetry.blocked.insert(
            key,
            Blocked {
                hash: tx.hash,
                nonce: tx.nonce,
                condition,
                source,
                observed_at_ms: now_ms(),
            },
        );
    }

    pub(crate) fn record_ready(&self, tx: &HeldTx, source: SourceState) {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        if state
            .telemetry
            .blocked
            .remove(&(tx.sender, tx.direction))
            .is_some()
        {
            tracing::info!(name: "eez.held_pool.source_ready", event_name = "eez.held_pool.source_ready",
                tx_hash = %tx.hash, sender = %tx.sender, nonce = tx.nonce, direction = ?tx.direction,
                l1_hash = %source.l1.hash, l2_hash = %source.l2.hash,
                "source condition repaired; complete group admitted");
        }
        state
            .telemetry
            .ready
            .entry(tx.hash)
            .or_insert_with(|| (source, now_ms()))
            .0 = source;
    }

    /// Aggregate view; no envelopes or queue-sized RPC response.
    pub fn queue_status(&self) -> Value {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        let telemetry = &state.telemetry;
        let snapshot = state.queue_snapshot();
        json!({"queued": state.txs.len(), "inFlight": state.in_flight.len(),
            "ready": snapshot.ready, "blocked": snapshot.blocked, "blockedReasons": snapshot.blocked_reasons,
            "unknown": snapshot.unknown,
            "oldestPendingAgeMs": snapshot.oldest_pending_at_ms.map(|time| now_ms().saturating_sub(time)),
            "oldestServiceAgeMs": snapshot.oldest_service_at_ms.map(|time| now_ms().saturating_sub(time)),
            "evaluatedAtMs": telemetry.evaluated_at_ms,
            "sourceState": telemetry.current.map(SourceState::json),
            "lastCanonicalCompletion": telemetry.last_completion.map(|(block, time)|
                json!({"l1Block": {"number": block.number, "hash": block.hash}, "observedAtMs": time}))})
    }

    /// Inspect current ownership, or a bounded local removal observation when
    /// the identity is absent from both the live pool and settlement ledger.
    pub fn transaction_status(&self, hash: B256) -> Option<Value> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        state.ledger.read(|data| {
            let owner = data.users.get(&hash);
            let tx = state.txs.iter().find(|tx| tx.hash == hash).or_else(|| owner.map(|u| &u.tx));
            let Some((sender, direction, nonce)) = tx.map(|tx| (tx.sender, tx.direction, tx.nonce))
                .or_else(|| state.in_flight.get(&hash).copied()) else {
                    return state.telemetry.removal_status(hash);
                };
            let ownership = owner.map(|user| match &user.location {
                UserLocation::Queued => json!({"state": "queued"}),
                UserLocation::Composing => json!({"state": "composing"}),
                UserLocation::AwaitingEvidence => json!({"state": "awaiting_evidence"}),
                UserLocation::Terminal { reason, canonical } => json!({
                    "state": "terminal", "reason": format!("{reason:?}"),
                    "canonical": canonical.map(|block| json!({"number": block.number, "hash": block.hash})),
                }),
            });
            let lifecycle = if state.by_hash.contains(&hash) { "queued" }
                else if state.in_flight.contains_key(&hash) { "in_flight" }
                else { ownership.as_ref().and_then(|value| value["state"].as_str()).unwrap_or("unknown") };
            let blocked = state.telemetry.blocked.get(&(sender, direction)).filter(|b| nonce >= b.nonce);
            Some(json!({"hash": hash, "sender": sender, "direction": direction_name(direction),
                "nonce": nonce, "lifecycle": lifecycle, "ownerGeneration": owner.map(|u| u.owner),
                "ownership": ownership,
                "blocking": blocked.map(|b| json!({"condition": b.condition,
                    "predecessorHash": b.hash, "observedAtMs": b.observed_at_ms,
                    "sourceState": b.source.json(), "current": Some(b.source) == state.telemetry.current}))}))
        })
    }
}

impl PoolState {
    fn queue_snapshot(&self) -> QueueSnapshot {
        let telemetry = &self.telemetry;
        let mut ready = 0usize;
        let mut blocked = 0usize;
        let mut oldest = None::<u64>;
        let mut oldest_service = None::<u64>;
        let mut reasons = BTreeMap::<&str, usize>::new();
        for tx in &self.txs {
            if telemetry
                .ready
                .get(&tx.hash)
                .is_some_and(|(source, _)| Some(*source) == telemetry.current)
            {
                ready += 1;
            } else if let Some(condition) = telemetry
                .blocked
                .get(&(tx.sender, tx.direction))
                .filter(|b| Some(b.source) == telemetry.current && tx.nonce >= b.nonce)
            {
                blocked += 1;
                let reason = match condition.condition {
                    SourceCondition::Nonce { .. } => "nonce",
                    SourceCondition::Fee => "fee",
                    SourceCondition::Balance { .. } => "balance",
                    SourceCondition::Escrow { .. } => "escrow",
                };
                *reasons.entry(reason).or_default() += 1;
            }
        }
        for hash in self.by_hash.iter().chain(self.in_flight.keys()) {
            if let Some(time) = telemetry.admitted_at.get(hash) {
                oldest = Some(oldest.map_or(*time, |old| old.min(*time)));
            }
            if let Some((source, time)) = telemetry.ready.get(hash)
                && (Some(*source) == telemetry.current || self.in_flight.contains_key(hash))
            {
                oldest_service = Some(oldest_service.map_or(*time, |old| old.min(*time)));
            }
        }
        let queued_inbound = self
            .txs
            .iter()
            .filter(|tx| tx.direction == Direction::Inbound)
            .count();
        let flight_inbound = self
            .in_flight
            .values()
            .filter(|(_, direction, _)| *direction == Direction::Inbound)
            .count();
        QueueSnapshot {
            counts: [
                (
                    Direction::Inbound,
                    QueueCounts {
                        queued: queued_inbound,
                        in_flight: flight_inbound,
                    },
                ),
                (
                    Direction::Outbound,
                    QueueCounts {
                        queued: self.txs.len() - queued_inbound,
                        in_flight: self.in_flight.len() - flight_inbound,
                    },
                ),
            ],
            ready,
            blocked,
            unknown: self.txs.len() - ready - blocked,
            blocked_reasons: reasons,
            oldest_pending_at_ms: oldest,
            last_completion_at_ms: telemetry.last_completion.map(|(_, time)| time),
            last_completion_l1_block: telemetry.last_completion.map(|(block, _)| block.number),
            completion_observations: telemetry.completion_observations,
            oldest_service_at_ms: oldest_service,
            // Reorg invalidation clears current source state but retains the
            // old RPC evaluation timestamp. Do not export that as valid.
            evaluated_at_ms: telemetry.current.and(telemetry.evaluated_at_ms),
        }
    }
}

/// Read-only observations of the held pool. Readiness partitions queued requests;
/// in-flight reservations are separate and may await settlement evidence.
#[derive(Clone, Debug)]
pub struct QueueSnapshot {
    pub counts: [(Direction, QueueCounts); 2],
    pub ready: usize,
    pub blocked: usize,
    pub unknown: usize,
    pub blocked_reasons: BTreeMap<&'static str, usize>,
    /// Earliest retained admission timestamp among queued/in-flight requests.
    pub oldest_pending_at_ms: Option<u64>,
    /// Local observation time of the latest retained canonical cross-chain completion.
    pub last_completion_at_ms: Option<u64>,
    /// L1 state against which completion was confirmed, not necessarily its inclusion block.
    pub last_completion_l1_block: Option<u64>,
    /// Inbound/outbound canonical-completion observations since process start.
    /// Reobservation after reorg counts again; these are not unique-user totals.
    pub completion_observations: [u64; 2],
    /// Last evaluation start time; absent before evaluation or after invalidation.
    pub evaluated_at_ms: Option<u64>,
    oldest_service_at_ms: Option<u64>,
}

/// Point-in-time counts of held requests for one source direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueCounts {
    pub queued: usize,
    pub in_flight: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_reorgs_only_forget_invalidated_completion_identities() {
        let earlier = (B256::repeat_byte(1), 1);
        let later = (B256::repeat_byte(2), 2);
        let mut telemetry = QueueTelemetry::default();
        let block = |number| L1BlockRef {
            number,
            hash: B256::ZERO,
        };
        telemetry.completed.insert(earlier, block(10));
        telemetry.completed.insert(later, block(20));
        telemetry.last_completion = Some((block(20), 1000));
        telemetry.completion_observations = [1, 1];
        telemetry.invalidate_after_l1(15);
        assert!(telemetry.completed.contains_key(&earlier));
        assert!(!telemetry.completed.contains_key(&later));
        assert!(telemetry.last_completion.is_none());
        telemetry.invalidate_after_l1(5);
        assert!(telemetry.completed.is_empty());
        assert_eq!(telemetry.completion_observations, [1, 1]);
    }
    use alloy_primitives::Bytes;

    fn request(byte: u8, direction: Direction) -> HeldTx {
        HeldTx {
            raw_tx: Bytes::new(),
            hash: B256::repeat_byte(byte),
            sender: Address::repeat_byte(byte),
            direction,
            nonce: 0,
            attempts: 0,
            max_fee_per_gas: 1,
            priority_fee_per_gas: 1,
        }
    }

    fn source() -> SourceState {
        SourceState {
            l1: L1BlockRef {
                number: 10,
                hash: B256::repeat_byte(10),
            },
            l2: L1BlockRef {
                number: 20,
                hash: B256::repeat_byte(20),
            },
        }
    }

    #[test]
    fn snapshot_and_rpc_share_readiness_reasons_and_source_invalidation() {
        let pool = HeldPool::new(Default::default());
        let users = (1..=6)
            .map(|id| {
                request(
                    id,
                    if id % 2 == 0 {
                        Direction::Outbound
                    } else {
                        Direction::Inbound
                    },
                )
            })
            .collect::<Vec<_>>();
        for user in &users {
            pool.push_contiguous(user.clone(), 0).unwrap();
        }
        let successor = HeldTx {
            nonce: 1,
            hash: B256::repeat_byte(99),
            ..users[0].clone()
        };
        pool.push_contiguous(successor, 0).unwrap();
        let initial = pool.queue_snapshot();
        assert_eq!((initial.ready, initial.blocked, initial.unknown), (0, 0, 7));
        assert_eq!(initial.evaluated_at_ms, None);
        pool.begin_evaluation(source());
        let conditions = [
            SourceCondition::Fee,
            SourceCondition::Nonce {
                transaction_nonce: 0,
                required_nonce: 1,
            },
            SourceCondition::Balance {
                required: alloy_primitives::U256::from(10),
                available: alloy_primitives::U256::ZERO,
            },
            SourceCondition::Escrow {
                required: alloy_primitives::U256::from(10),
                available: alloy_primitives::U256::ZERO,
            },
        ];
        for (user, condition) in users.iter().zip(conditions) {
            pool.record_blocked(user, condition, source());
        }
        pool.record_ready(&users[4], source());
        let snapshot = pool.queue_snapshot();
        assert_eq!(
            (snapshot.ready, snapshot.blocked, snapshot.unknown),
            (1, 5, 1)
        );
        assert_eq!(
            snapshot.blocked_reasons,
            BTreeMap::from([("fee", 2), ("nonce", 1), ("balance", 1), ("escrow", 1)])
        );
        let rpc = pool.queue_status();
        assert_eq!(rpc["ready"], snapshot.ready);
        assert_eq!(rpc["blocked"], snapshot.blocked);
        assert_eq!(rpc["unknown"], snapshot.unknown);
        assert_eq!(rpc["blockedReasons"], json!(snapshot.blocked_reasons));
        assert_eq!(rpc["evaluatedAtMs"], snapshot.evaluated_at_ms.unwrap());
        assert_eq!(
            snapshot.ready + snapshot.blocked + snapshot.unknown,
            snapshot.counts.iter().map(|(_, c)| c.queued).sum::<usize>()
        );
        // Sampling never refreshes source evaluation time.
        assert_eq!(
            pool.queue_snapshot().evaluated_at_ms,
            snapshot.evaluated_at_ms
        );
        pool.record_ready(&users[0], source());
        let repaired = pool.queue_snapshot();
        assert_eq!(
            (repaired.ready, repaired.blocked, repaired.unknown),
            (2, 3, 2)
        );
        assert!(!repaired.blocked_reasons.contains_key("fee"));
        let changed = SourceState {
            l2: L1BlockRef {
                hash: B256::repeat_byte(21),
                ..source().l2
            },
            ..source()
        };
        pool.begin_evaluation(changed);
        let stale = pool.queue_snapshot();
        assert_eq!((stale.ready, stale.blocked, stale.unknown), (0, 0, 7));
        assert!(stale.blocked_reasons.is_empty());
        pool.invalidate_after_l1(9).unwrap();
        assert_eq!(pool.queue_snapshot().evaluated_at_ms, None);
        assert!(
            pool.queue_status()["evaluatedAtMs"].is_number(),
            "preserve the existing RPC contract"
        );
    }

    #[test]
    fn oldest_pending_tracks_membership_recovery_replacement_and_missing_history() {
        let pool = HeldPool::new(Default::default());
        let first = request(1, Direction::Inbound);
        let second = request(2, Direction::Outbound);
        for user in [&first, &second] {
            pool.push_contiguous(user.clone(), 0).unwrap();
        }
        {
            let mut state = pool.state.lock().unwrap();
            state.telemetry.admitted_at.insert(first.hash, 1_000);
            state.telemetry.admitted_at.insert(second.hash, 2_000);
        }
        assert_eq!(pool.queue_snapshot().oldest_pending_at_ms, Some(1_000));
        assert!(pool.claim(&first));
        assert_eq!(pool.queue_snapshot().oldest_pending_at_ms, Some(1_000));
        pool.push_front_batch(vec![first.clone()]);
        assert_eq!(pool.queue_snapshot().oldest_pending_at_ms, Some(1_000));
        let replacement = HeldTx {
            hash: B256::repeat_byte(3),
            max_fee_per_gas: 100,
            priority_fee_per_gas: 100,
            ..first
        };
        pool.push_contiguous(replacement, 0).unwrap();
        assert_eq!(pool.queue_snapshot().oldest_pending_at_ms, Some(2_000));
        // Missing admission history is unknown, never an epoch-zero age.
        pool.state.lock().unwrap().telemetry.admitted_at.clear();
        assert_eq!(pool.queue_snapshot().oldest_pending_at_ms, None);
        let drained = pool.pop_all();
        pool.release_in_flight_batch(&drained);
        assert_eq!(pool.queue_snapshot().oldest_pending_at_ms, None);
        assert_eq!(
            HeldPool::new(Default::default())
                .queue_snapshot()
                .oldest_pending_at_ms,
            None
        );
    }

    #[test]
    fn authoritative_status_retires_local_removal_even_after_ledger_pruning() {
        let ledger = std::sync::Arc::new(OptimisticallyIncluded::default());
        let pool = HeldPool::new(ledger.clone());
        let tx = HeldTx {
            raw_tx: Bytes::new(),
            hash: B256::repeat_byte(1),
            sender: Address::repeat_byte(2),
            direction: Direction::Inbound,
            nonce: 0,
            attempts: 0,
            max_fee_per_gas: 1,
            priority_fee_per_gas: 1,
        };
        pool.push_contiguous(tx.clone(), 0).unwrap();
        let _ = pool.evict_chain_at_or_above(tx.sender, tx.direction, 0);
        assert_eq!(
            pool.transaction_status(tx.hash).unwrap()["ownership"]["reason"],
            "PoolDiscarded"
        );
        // Supply an authoritative observation directly: this tests status
        // precedence and retention, not canonical reconciliation itself.
        ledger
            .update(|state| {
                state.users.insert(
                    tx.hash,
                    crate::optimistic::OwnedUser {
                        tx: tx.clone(),
                        owner: 7,
                        fifo_position: 0,
                        location: UserLocation::Terminal {
                            reason: TerminalReason::Committed,
                            canonical: Some(L1BlockRef {
                                number: 1,
                                hash: B256::repeat_byte(3),
                            }),
                        },
                    },
                );
                Ok(())
            })
            .unwrap();
        pool.release_in_flight_batch(&[]);
        let status = pool.transaction_status(tx.hash).unwrap();
        assert_eq!(status["ownerGeneration"], 7);
        assert_eq!(status["ownership"]["reason"], "Committed");
        ledger
            .update(|state| {
                state.users.remove(&tx.hash);
                Ok(())
            })
            .unwrap();
        assert!(pool.transaction_status(tx.hash).is_none());
    }

    #[test]
    fn service_age_excludes_source_waiting_and_survives_claiming() {
        let pool = HeldPool::new(Default::default());
        let tx = HeldTx {
            raw_tx: Bytes::new(),
            hash: B256::repeat_byte(1),
            sender: Address::repeat_byte(1),
            direction: Direction::Inbound,
            nonce: 0,
            attempts: 0,
            max_fee_per_gas: 1,
            priority_fee_per_gas: 1,
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
        pool.push_contiguous(tx.clone(), 0).unwrap();
        pool.state
            .lock()
            .unwrap()
            .telemetry
            .admitted_at
            .insert(tx.hash, now_ms() - 60_000);
        pool.begin_evaluation(source);
        pool.record_blocked(&tx, SourceCondition::Fee, source);
        assert!(pool.queue_status()["oldestServiceAgeMs"].is_null());
        pool.record_ready(&tx, source);
        assert!(pool.claim(&tx));
        let status = pool.queue_status();
        assert_eq!(status["inFlight"], 1);
        assert!(
            status["oldestServiceAgeMs"].as_u64().unwrap()
                < status["oldestPendingAgeMs"].as_u64().unwrap()
        );
        pool.record_blocked(&tx, SourceCondition::Fee, source);
        pool.requeue_composing(vec![tx]);
        assert!(pool.queue_status()["oldestServiceAgeMs"].is_null());
    }
}
