//! Bounded observations of existing queue/accounting state. These fields never
//! authorize execution and are discarded on restart along with the held pool.

use std::{
    collections::{HashMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

use alloy_primitives::{Address, B256};
use eez_l1::L1BlockRef;
use eez_protocol::SourceCondition;
use serde_json::{Value, json};

use super::{Direction, HeldPool, HeldTx, OptimisticallyIncluded, TerminalReason, UserLocation};

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
    completed: HashSet<(B256, u64)>,
    last_completion: Option<(L1BlockRef, u64)>,
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
        self.admitted_at.insert(hash, now_ms());
        self.ready.remove(&hash);
    }

    pub fn retain(
        &mut self,
        queued: &HashSet<B256>,
        in_flight: &HashMap<B256, (Address, Direction, u64)>,
        ledger: &OptimisticallyIncluded,
    ) {
        ledger.read(|data| {
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
            self.completed.retain(|(hash, generation)| {
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
                    && self.completed.insert((*hash, user.owner))
                    && self
                        .last_completion
                        .is_none_or(|(previous, _)| block.number >= previous.number)
                {
                    self.last_completion = Some((block, now_ms()));
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
            self.completed.clear();
        }
        self.current = None;
        self.ready.clear();
    }
}

impl HeldPool {
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
        let mut ready = 0usize;
        let mut blocked = 0usize;
        let mut oldest = None::<u64>;
        let mut oldest_service = None::<u64>;
        let mut reasons = std::collections::BTreeMap::<&str, usize>::new();
        for tx in &state.txs {
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
        for hash in state.by_hash.iter().chain(state.in_flight.keys()) {
            if let Some(time) = telemetry.admitted_at.get(hash) {
                oldest = Some(oldest.map_or(*time, |old| old.min(*time)));
            }
            if let Some((source, time)) = telemetry.ready.get(hash)
                && (Some(*source) == telemetry.current || state.in_flight.contains_key(hash))
            {
                oldest_service = Some(oldest_service.map_or(*time, |old| old.min(*time)));
            }
        }
        json!({"queued": state.txs.len(), "inFlight": state.in_flight.len(),
            "ready": ready, "blocked": blocked, "blockedReasons": reasons, "unknown": state.txs.len() - ready - blocked,
            "oldestPendingAgeMs": oldest.map(|time| now_ms().saturating_sub(time)),
            "oldestServiceAgeMs": oldest_service.map(|time| now_ms().saturating_sub(time)),
            "evaluatedAtMs": telemetry.evaluated_at_ms,
            "sourceState": telemetry.current.map(SourceState::json),
            "lastCanonicalCompletion": telemetry.last_completion.map(|(block, time)|
                json!({"l1Block": {"number": block.number, "hash": block.hash}, "observedAtMs": time}))})
    }

    /// Inspect one identity while its pool or existing ledger record is retained.
    pub fn transaction_status(&self, hash: B256) -> Option<Value> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        state.ledger.read(|data| {
            let owner = data.users.get(&hash);
            let tx = state.txs.iter().find(|tx| tx.hash == hash).or_else(|| owner.map(|u| &u.tx));
            let (sender, direction, nonce) = tx.map(|tx| (tx.sender, tx.direction, tx.nonce))
                .or_else(|| state.in_flight.get(&hash).copied())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;

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
