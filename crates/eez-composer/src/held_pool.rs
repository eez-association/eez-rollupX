//! Per-rollup pool of cross-chain transactions awaiting Sync-slot
//! composition.
//!
//! Cross-chain txs (detected at `TxIngress` time per §5.4.5) sit here
//! between submission and the next Sync slot, where they're drained,
//! handed to the umbrella's [`crate::Composer`] for simulation, and the
//! resulting `system_txs` get bundled into the Sync block.
//!
//! Held-pool drain semantics:
//!
//! - [`HeldPool::pop_n`] / [`HeldPool::pop_all`] — drain queued transactions
//!   and reserve their nonces as in-flight. Called by the umbrella on Sync-slot
//!   trigger after the composer's batch is built and the bundle is
//!   queued for submission. Cross-chain reorg recovery (re-injecting
//!   pre-composed txs after L1 reorg or bundle drop) replays into the
//!   pool via [`HeldPool::push_front_batch`].
//!
//! Concurrency: one `Mutex<PoolState>` keeps queue membership, hash
//! dedupe, and in-flight nonce reservations atomic. Held-pool operations
//! are infrequent (per sync slot, per L1 event) and never hot-path.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use alloy_primitives::{Address, Bytes, TxHash};
use tracing::{Level, event};

use crate::ingress::Direction;
use crate::optimistic::{OptimisticallyIncluded, TerminalReason, UserLocation};

mod status;
pub use status::{QueueCounts, QueueSnapshot, SourceState};

/// A cross-chain transaction held between submission and Sync-slot
/// composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTx {
    /// RLP-encoded source transaction (signed envelope). Handed to
    /// the composer's chained simulation verbatim.
    pub raw_tx: Bytes,
    /// Cached hash of the signed envelope, used for queued/in-flight dedupe.
    pub hash: TxHash,
    /// Failed settlement attempts so far. A proof rejection or bundle drop
    /// may be caused by a poison transaction even when compose-time simulation
    /// accepted it. Recovery increments this once per failed episode and
    /// evicts the tx at the applicable bound: three for prover/immediate relay
    /// rejection and numeric admission, six for automatic canonical settlement
    /// failures (allowing a reduced retry after the fifth exclusion). Switching
    /// failure paths preserves this counter. Eviction requires resubmission.
    pub attempts: u32,
    /// Recovered L1 sender. Together with `nonce`, lets the pool and
    /// the eviction path keep each sender's nonce chain CONTIGUOUS:
    /// ingress rejects out-of-sequence submissions, and evicting a tx
    /// cascades to the sender's higher nonces (they can never land
    /// once the gap exists — bundling them only poisons bundles).
    pub sender: Address,
    /// The tx's nonce, in the sender's chain for this `direction`
    /// (inbound: the originating chain's nonce; outbound: this L2's).
    pub nonce: u64,
    /// Cross-chain axis. Inbound and outbound txs of the same EOA keep
    /// independent nonce chains, keyed on `(sender, direction)`.
    pub direction: Direction,
    /// Declared `max_fee_per_gas` (legacy: `gas_price`). The pool is FIFO,
    /// so this buys no priority — it only enforces the replacement bump.
    pub max_fee_per_gas: u128,
    /// Effective priority fee (legacy/2930: `gas_price`, which is both cap
    /// and tip). Replacements must bump it alongside the cap (geth rule).
    pub priority_fee_per_gas: u128,
}

/// Minimum bump (percent) a queued-nonce replacement must offer on BOTH
/// `max_fee_per_gas` and the priority fee, each over its replaced value
/// (geth's price-bump rule). The cap bump bounds churn via the ingress
/// balance check; the tip bump keeps a replacement a real better offer.
pub const REPLACEMENT_TX_COST_PERCENT: u128 = 10;

/// A pool admission failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    /// A canonical or unresolved owner conflicts with the ingress nonce view.
    /// The caller can retry after canonical reconciliation; nothing was held.
    ReconciliationPending,
    /// Nonce is burned, reserved in-flight, or not the next unreserved slot.
    Nonce { expected: u64, on_chain: u64 },
    /// Queued-nonce replacement did not bump both fees by the required
    /// percent. `required_*` values are what the replacement had to offer.
    UnderpricedReplacement {
        offered_max_fee: u128,
        required_max_fee: u128,
        offered_priority_fee: u128,
        required_priority_fee: u128,
    },
}

/// Smallest fee a replacement must offer over `replaced`: ≥10% more,
/// always at least +1 (bars equal-fee churn).
fn min_replacement_fee(replaced: u128) -> u128 {
    let required_increase = (replaced.saturating_mul(REPLACEMENT_TX_COST_PERCENT) / 100).max(1);
    replaced.saturating_add(required_increase)
}

/// All pool state under one lock, so queue membership, hash dedupe and
/// nonce reservations can never disagree with each other.
#[derive(Debug)]
struct PoolState {
    ledger: Arc<OptimisticallyIncluded>,
    /// Queued txs awaiting the next Sync-slot drain, in FIFO order.
    txs: VecDeque<HeldTx>,
    /// Hashes of the queued txs — O(1) duplicate-submission check.
    by_hash: HashSet<TxHash>,
    /// Txs drained into a bundle whose outcome is not yet known
    /// (`hash → (sender, direction, nonce)`). The nonce stays reserved —
    /// still counted by admission, not replaceable — until the bundle
    /// settles (release) or fails (recovery re-queues the tx).
    /// Invariant: every drained tx must eventually be released, re-queued,
    /// or evicted by its caller; a forgotten reservation bricks its nonce
    /// slot (admission forever expects past it).
    in_flight: HashMap<TxHash, (Address, Direction, u64)>,
    /// Retain an omitted eligible source as the end of the next FIFO snapshot
    /// when the current deadline cannot accommodate canonical recomposition.
    /// Identity, rather than hash, lets an ingress replacement keep its position.
    fifo_boundary: Option<(Address, Direction, u64)>,
    telemetry: status::QueueTelemetry,
}

impl PoolState {
    /// Only explicit, validated admission may retire local terminal ownership.
    /// Attempt records retain their generations so old callbacks remain harmless.
    fn retire_local_owner_for_admission(&self, hash: TxHash) -> Result<(), AdmissionError> {
        if !self.ledger.read(|data| data.users.contains_key(&hash)) {
            return Ok(());
        }
        self.ledger
            .update(|data| {
                match data.users.get(&hash).map(|user| &user.location) {
                    None => {}
                    Some(UserLocation::Terminal {
                        canonical: None, ..
                    }) => {
                        let previous = data
                            .users
                            .remove(&hash)
                            .expect("matched local terminal owner");
                        event!(name: "eez.held_pool.readmitted", Level::INFO,
                            event_name = "eez.held_pool.readmitted",
                            tx_hash = %hash, previous_generation = previous.owner,
                            previous_disposition = ?previous.location,
                            "explicit admission retired local terminal ownership");
                    }
                    Some(_) => {
                        return Err("source ownership awaits canonical reconciliation".into());
                    }
                }
                Ok(())
            })
            .map_err(|_| AdmissionError::ReconciliationPending)
    }

    fn apply_resolution(&mut self, gaps: &[crate::optimistic::NonceGap]) {
        let mut users = self
            .ledger
            .read(|data| data.users.values().cloned().collect::<Vec<_>>());
        users.sort_by_key(|user| (user.owner, user.fifo_position));
        for user in &users {
            if user.location != UserLocation::Queued {
                self.txs.retain(|tx| tx.hash != user.tx.hash);
                self.by_hash.remove(&user.tx.hash);
                self.in_flight.remove(&user.tx.hash);
                if matches!(
                    user.location,
                    UserLocation::AwaitingEvidence | UserLocation::Composing
                ) {
                    self.mark_in_flight(&user.tx);
                }
            }
        }
        // These gaps come from this resolution's reusable source nonces;
        // source-included failures never generate a gap.
        for gap in gaps {
            self.txs.retain(|tx| {
                !(tx.sender == gap.sender && tx.direction == gap.direction && tx.nonce >= gap.nonce)
            });
            self.in_flight.retain(|_, (sender, direction, nonce)| {
                !(*sender == gap.sender && *direction == gap.direction && *nonce >= gap.nonce)
            });
        }
        if !gaps.is_empty() {
            self.by_hash = self.txs.iter().map(|tx| tx.hash).collect();
        }
        self.recover_batch(
            users
                .into_iter()
                .filter(|user| user.location == UserLocation::Queued)
                .map(|user| user.tx)
                .collect(),
        );
        self.update_recovery_ownership();
    }

    /// Track submitted users through recomposition, replacement and eviction
    /// so an old attempt cannot recover a discarded or newly owned user.
    fn update_recovery_ownership(&mut self) {
        self.ledger
            .update(|data| {
                for user in data.users.values_mut() {
                    if !matches!(
                        user.location,
                        UserLocation::Queued | UserLocation::Composing
                    ) {
                        continue;
                    }
                    if let Some(queued) = self.txs.iter().find(|tx| tx.hash == user.tx.hash) {
                        user.tx = queued.clone();
                        user.location = UserLocation::Queued;
                    } else if self.in_flight.contains_key(&user.tx.hash) {
                        user.location = UserLocation::Composing;
                    } else {
                        user.location = UserLocation::Terminal {
                            reason: TerminalReason::PoolDiscarded,
                            canonical: None,
                        };
                    }
                }
                Ok(())
            })
            .expect("cannot preserve recovered transaction ownership; held pool stopped");
        self.telemetry
            .retain(&self.by_hash, &self.in_flight, &self.ledger);
    }
    fn recover_batch(&mut self, recovered: Vec<HeldTx>) {
        let mut replacement_roots: HashMap<(Address, Direction), (u64, TxHash)> = HashMap::new();
        for tx in &recovered {
            let replacement = self.txs.iter().find(|queued| {
                queued.sender == tx.sender
                    && queued.direction == tx.direction
                    && queued.nonce == tx.nonce
                    && queued.hash != tx.hash
            });
            if let Some(replacement) = replacement {
                replacement_roots
                    .entry((tx.sender, tx.direction))
                    .and_modify(|(nonce, hash)| {
                        if tx.nonce < *nonce {
                            *nonce = tx.nonce;
                            *hash = replacement.hash;
                        }
                    })
                    .or_insert((tx.nonce, replacement.hash));
            }
        }

        for tx in recovered.into_iter().rev() {
            if let Some(&(conflict_nonce, replacement_hash)) =
                replacement_roots.get(&(tx.sender, tx.direction))
                && tx.nonce >= conflict_nonce
            {
                self.release_in_flight(&tx);
                event!(
                    name: "eez.held_pool.recovery_conflict",
                    Level::WARN,
                    recovered_hash = %tx.hash,
                    replacement_hash = %replacement_hash,
                    sender = %tx.sender,
                    direction = ?tx.direction,
                    nonce = tx.nonce,
                    conflict_nonce,
                    "newer queued replacement wins; discarding recovered nonce-chain suffix",
                );
                continue;
            }
            self.push_front_recovered(tx);
        }
    }

    /// Next nonce that keeps the `(sender, direction)` chain contiguous:
    /// first hole starting at the observed source nonce. A pending observation
    /// may roll back; a higher reservation must never hide a missing predecessor.
    /// Reservations below the observation are ignored, not removed.
    fn next_expected_nonce(
        &self,
        sender: Address,
        direction: Direction,
        on_chain: u64,
    ) -> Option<u64> {
        let reserved: HashSet<_> = self
            .txs
            .iter()
            .filter(|tx| tx.sender == sender && tx.direction == direction)
            .map(|tx| tx.nonce)
            .chain(
                self.in_flight
                    .values()
                    .filter(|(s, d, _)| *s == sender && *d == direction)
                    .map(|(_, _, nonce)| *nonce),
            )
            .filter(|nonce| *nonce >= on_chain)
            .collect();
        let mut expected = on_chain;
        while reserved.contains(&expected) {
            expected = expected.checked_add(1)?;
        }
        Some(expected)
    }

    /// Move a recovered tx from in-flight back to the queue front,
    /// skipping the re-queue if the hash is already queued (idempotent).
    fn push_front_recovered(&mut self, tx: HeldTx) {
        self.release_in_flight(&tx);
        self.insert_queued(tx, true);
    }

    /// One insertion rule for ingress and recovery. Global queue priority is
    /// constrained by the source chain's predecessor/successor positions.
    fn insert_queued(&mut self, tx: HeldTx, front_priority: bool) {
        if !self.by_hash.insert(tx.hash) {
            return;
        }
        let same_chain =
            |queued: &HeldTx| queued.sender == tx.sender && queued.direction == tx.direction;
        let position = if front_priority {
            self.txs
                .iter()
                .rposition(|queued| same_chain(queued) && queued.nonce < tx.nonce)
                .map_or(0, |index| index + 1)
        } else {
            self.txs
                .iter()
                .position(|queued| same_chain(queued) && queued.nonce > tx.nonce)
                .unwrap_or(self.txs.len())
        };
        if front_priority && position > 0 {
            let owner = self
                .ledger
                .read(|data| data.users.get(&tx.hash).map(|u| u.owner));
            event!(name: "eez.held_pool.recovery_ordered", Level::DEBUG,
                event_name = "eez.held_pool.recovery_ordered", tx_hash = %tx.hash,
                sender = %tx.sender, direction = ?tx.direction, nonce = tx.nonce,
                ?owner, position, "recovery retained queued nonce predecessors");
        }
        self.txs.insert(position, tx);
    }

    /// Reserve a drained tx's nonce until its bundle resolves.
    fn mark_in_flight(&mut self, tx: &HeldTx) {
        self.in_flight
            .entry(tx.hash)
            .or_insert((tx.sender, tx.direction, tx.nonce));
    }

    /// Drop a tx's nonce reservation (bundle settled, tx evicted, or
    /// re-queued by recovery).
    fn release_in_flight(&mut self, tx: &HeldTx) {
        self.in_flight.remove(&tx.hash);
    }
}

/// Per-rollup pool of held cross-chain transactions.
///
/// Every [`RollupState`](crate::RollupState) owned by a composer has one;
/// follower and development binaries do not construct `RollupState`.
#[derive(Debug)]
pub struct HeldPool {
    state: Mutex<PoolState>,
}

impl HeldPool {
    /// Bind the held pool to its in-memory attempt ledger.
    #[must_use]
    pub fn new(ledger: Arc<OptimisticallyIncluded>) -> Self {
        let state = PoolState {
            ledger,
            txs: VecDeque::new(),
            by_hash: HashSet::new(),
            in_flight: HashMap::new(),
            fifo_boundary: None,
            telemetry: Default::default(),
        };
        Self {
            state: Mutex::new(state),
        }
    }

    pub(crate) fn uses_ledger(&self, ledger: &Arc<OptimisticallyIncluded>) -> bool {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        Arc::ptr_eq(&state.ledger, ledger)
    }

    /// Apply an ownership partition and update reservations while ingress is excluded.
    pub fn resolve_attempt(
        &self,
        id: crate::optimistic::AttemptId,
        as_of: eez_l1::L1BlockRef,
        outcomes: &[crate::optimistic::SourceOutcome],
        charge_attempt: bool,
        attempt_limit: u32,
        is_canonical: impl FnOnce() -> bool,
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        if !is_canonical() {
            return Err("canonical settlement changed before ownership transfer".into());
        }
        let gaps = state
            .ledger
            .resolve(id, as_of, outcomes, charge_attempt, attempt_limit)?;
        state.apply_resolution(&gaps);
        let ledger = Arc::clone(&state.ledger);
        state.telemetry.observe_completion(&ledger);
        Ok(())
    }

    /// Reopen canonical outcomes and restore their reservations atomically
    /// with respect to ingress and a concurrent reconciliation decision.
    pub fn invalidate_after_l1(&self, ancestor: u64) -> Result<(), String> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        state.ledger.invalidate_after_l1(ancestor)?;
        state.telemetry.invalidate_after_l1(ancestor);
        state.apply_resolution(&[]);
        Ok(())
    }

    /// Move the current attempt's users back to the queue without declaring
    /// failure. Retain the old generation so its delayed observer cannot modify
    /// a newer owner. Called only at the validated replacement deadline.
    pub(crate) fn release_for_replacement(
        &self,
        id: crate::optimistic::AttemptId,
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        state.ledger.release_for_replacement(id)?;
        state.apply_resolution(&[]);
        Ok(())
    }

    /// A recomposition-time nonce check is not finalized settlement evidence.
    /// Previously submitted users return to canonical reconciliation instead
    /// of losing their reorg history through an ordinary stale-pool eviction.
    pub fn recheck_stale_sources(&self, txs: &[HeldTx]) -> Result<bool, String> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let reopened = state.ledger.update(|data| {
            let mut reopened = false;
            for tx in txs {
                if let Some(user) = data.users.get_mut(&tx.hash).filter(|user| {
                    matches!(
                        user.location,
                        UserLocation::Composing | UserLocation::Queued
                    )
                }) {
                    user.location = UserLocation::AwaitingEvidence;
                    let attempt = data
                        .attempts
                        .get_mut(&user.owner)
                        .ok_or("missing recovery attempt")?;
                    attempt.reconciled_at = None;
                    attempt.observation = crate::optimistic::SubmissionObservation::Uncertain;
                    attempt.replacement_released = false;
                    reopened = true;
                }
            }
            Ok(reopened)
        })?;
        for tx in txs {
            state.release_in_flight(tx);
        }
        state.apply_resolution(&[]);
        Ok(reopened)
    }

    /// Admit `tx` into the contiguous queued + in-flight nonce chain for
    /// `(sender, direction)`. A new hash at an existing queued nonce replaces
    /// that transaction in place, if it bumps `max_fee_per_gas` by
    /// [`REPLACEMENT_TX_COST_PERCENT`]. In-flight nonces cannot be replaced.
    /// Reservations below `on_chain` have already landed and do not advance
    /// the expected nonce a second time. Duplicate hashes are idempotent.
    pub fn push_contiguous(&self, tx: HeldTx, on_chain: u64) -> Result<(), AdmissionError> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        if state.by_hash.contains(&tx.hash) || state.in_flight.contains_key(&tx.hash) {
            return Ok(());
        }

        let target_sender = tx.sender;
        let target_direction = tx.direction;
        let target_nonce = tx.nonce;
        let same_chain = |sender: Address, direction: Direction| {
            sender == target_sender && direction == target_direction
        };
        let queued_match = state.txs.iter().position(|queued| {
            same_chain(queued.sender, queued.direction) && queued.nonce == target_nonce
        });

        if target_nonce < on_chain {
            let expected = state
                .next_expected_nonce(target_sender, target_direction, on_chain)
                .unwrap_or(u64::MAX);
            return Err(AdmissionError::Nonce { expected, on_chain });
        }

        if let Some(queued_idx) = queued_match {
            // Geth price-bump rule: BOTH fees must bump over the replaced tx.
            let required_max_fee = min_replacement_fee(state.txs[queued_idx].max_fee_per_gas);
            let required_priority_fee =
                min_replacement_fee(state.txs[queued_idx].priority_fee_per_gas);
            if tx.max_fee_per_gas < required_max_fee
                || tx.priority_fee_per_gas < required_priority_fee
            {
                return Err(AdmissionError::UnderpricedReplacement {
                    offered_max_fee: tx.max_fee_per_gas,
                    required_max_fee,
                    offered_priority_fee: tx.priority_fee_per_gas,
                    required_priority_fee,
                });
            }
            let replacement_hash = tx.hash;
            let previous_hash = state.txs[queued_idx].hash;
            state.retire_local_owner_for_admission(replacement_hash)?;
            state.telemetry.removed(
                previous_hash,
                (target_sender, target_direction, target_nonce),
                TerminalReason::PoolDiscarded,
                Some(replacement_hash),
            );
            state.telemetry.admitted(replacement_hash);
            state.txs[queued_idx] = tx;
            state.by_hash.remove(&previous_hash);
            state.by_hash.insert(replacement_hash);
            event!(name: "eez.held_pool.replaced", Level::INFO,
                event_name = "eez.held_pool.replaced",
                previous_hash = %previous_hash, replacement_hash = %replacement_hash,
                sender = %target_sender, nonce = target_nonce, direction = ?target_direction,
                "accepted queued transaction replacement");
            state.update_recovery_ownership();
            return Ok(());
        }

        if state.in_flight.values().any(|(sender, direction, nonce)| {
            same_chain(*sender, *direction) && *nonce == target_nonce
        }) {
            return Err(AdmissionError::Nonce {
                expected: state
                    .next_expected_nonce(target_sender, target_direction, on_chain)
                    .unwrap_or(u64::MAX),
                on_chain,
            });
        }

        let Some(expected) = state.next_expected_nonce(target_sender, target_direction, on_chain)
        else {
            return Err(AdmissionError::Nonce {
                expected: u64::MAX,
                on_chain,
            });
        };
        if target_nonce != expected {
            return Err(AdmissionError::Nonce { expected, on_chain });
        }
        state.retire_local_owner_for_admission(tx.hash)?;
        state.telemetry.admitted(tx.hash);
        state.insert_queued(tx, false);
        state.update_recovery_ownership();
        Ok(())
    }

    /// Re-queue recovered txs at the front, unless a newer queued replacement
    /// owns that nonce; then discard the recovered root and suffix.
    pub fn push_front_batch(&self, recovered: Vec<HeldTx>) {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        state.recover_batch(recovered);
        state.update_recovery_ownership();
    }

    /// Restore a failed composition's original candidates only while this
    /// composition still owns them. A later error must not undo an eviction,
    /// release, replacement, or transfer back to canonical reconciliation.
    pub(crate) fn requeue_composing(&self, candidates: Vec<HeldTx>) {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let retained = state.ledger.read(|data| {
            candidates
                .into_iter()
                .filter(|tx| {
                    state.in_flight.contains_key(&tx.hash)
                        && data
                            .users
                            .get(&tx.hash)
                            .is_none_or(|user| user.location == UserLocation::Composing)
                })
                .collect()
        });
        state.recover_batch(retained);
        state.update_recovery_ownership();
    }

    /// Release txs that were drained into an optimistic bundle and are now
    /// known not to need nonce reservation anymore.
    pub fn release_in_flight_batch(&self, txs: &[HeldTx]) {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        for tx in txs {
            state.release_in_flight(tx);
        }
        state.update_recovery_ownership();
    }

    /// Atomically evict every queued and in-flight reservation at or above
    /// `nonce` in one sender/direction chain. Returns queued transactions that
    /// were removed; callers already own any matching in-flight transactions.
    #[must_use]
    pub fn evict_chain_at_or_above(
        &self,
        sender: Address,
        direction: Direction,
        nonce: u64,
    ) -> Vec<HeldTx> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let mut evicted = Vec::new();
        let queued = std::mem::take(&mut state.txs);
        let mut kept = VecDeque::with_capacity(queued.len());
        for tx in queued {
            if tx.sender == sender && tx.direction == direction && tx.nonce >= nonce {
                state.by_hash.remove(&tx.hash);
                state.telemetry.removed(
                    tx.hash,
                    (tx.sender, tx.direction, tx.nonce),
                    if tx.nonce == nonce {
                        TerminalReason::PoolDiscarded
                    } else {
                        TerminalReason::NonceGap
                    },
                    None,
                );
                evicted.push(tx);
            } else {
                kept.push_back(tx);
            }
        }
        state.txs = kept;
        let PoolState {
            in_flight,
            telemetry,
            ..
        } = &mut *state;
        in_flight.retain(|hash, identity| {
            let remove = identity.0 == sender && identity.1 == direction && identity.2 >= nonce;
            if remove {
                telemetry.removed(
                    *hash,
                    *identity,
                    if identity.2 == nonce {
                        TerminalReason::PoolDiscarded
                    } else {
                        TerminalReason::NonceGap
                    },
                    None,
                );
            }
            !remove
        });
        state.update_recovery_ownership();
        evicted
    }

    /// Drain every queued transaction and reserve its nonce as in-flight.
    /// Recovery paths can return the batch with [`Self::push_front_batch`].
    pub fn pop_all(&self) -> Vec<HeldTx> {
        self.pop_n(usize::MAX)
    }

    /// Finite FIFO view. Transactions stay replaceable until claimed. A prior
    /// discarded candidate may restrict this snapshot through its oldest omitted
    /// source, preventing younger outbounds from consuming its capacity again.
    pub(crate) fn candidates(&self) -> Vec<HeldTx> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let mut candidates: Vec<_> = state.txs.iter().cloned().collect();
        if let Some(boundary) = state.fifo_boundary {
            if let Some(index) = candidates
                .iter()
                .position(|tx| (tx.sender, tx.direction, tx.nonce) == boundary)
            {
                candidates.truncate(index + 1);
            } else {
                state.fifo_boundary = None;
            }
        }
        candidates
    }

    /// Return only still-owned snapshot users, restoring their original FIFO
    /// positions across both execution phases. Queued replacements keep their
    /// positions; newly recovered predecessors retain nonce precedence through
    /// the pool's existing insertion rule. No terminal owner is resurrected.
    pub(crate) fn retry_fifo_prefix(&self, snapshot: &[HeldTx], prefix_len: usize) {
        assert!(prefix_len > 0 && prefix_len < snapshot.len());
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let queued: HashMap<_, _> = state
            .txs
            .iter()
            .map(|tx| ((tx.sender, tx.direction, tx.nonce), tx.clone()))
            .collect();
        let retained: Vec<_> = state.ledger.read(|data| {
            snapshot
                .iter()
                .filter_map(|tx| {
                    queued
                        .get(&(tx.sender, tx.direction, tx.nonce))
                        .cloned()
                        .or_else(|| {
                            (state.in_flight.contains_key(&tx.hash)
                                && data
                                    .users
                                    .get(&tx.hash)
                                    .is_none_or(|user| user.location == UserLocation::Composing))
                            .then(|| tx.clone())
                        })
                })
                .collect()
        });
        let hashes: HashSet<_> = retained.iter().map(|tx| tx.hash).collect();
        let remaining = std::mem::replace(&mut state.txs, retained.into());
        // The original snapshot preserves each chain's nonce order, including
        // replacements at the same nonce. Start with that order, then insert
        // concurrent arrivals using the normal predecessor rule. Repeated
        // front insertion would move younger independent users ahead of a
        // newly recovered predecessor and its older snapshot successor.
        state.in_flight.retain(|hash, _| !hashes.contains(hash));
        state.by_hash = hashes;
        for tx in remaining {
            state.insert_queued(tx, false);
        }
        state.update_recovery_ownership();
        let boundary = &snapshot[prefix_len - 1];
        state.fifo_boundary = Some((boundary.sender, boundary.direction, boundary.nonce));
    }

    /// Atomically claim the snapshot's exact identity. A concurrent replacement
    /// or newly recovered predecessor requires discovery at a later opportunity.
    /// Already composing candidates can be reused by bounded recomposition.
    pub(crate) fn claim(&self, tx: &HeldTx) -> bool {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let chain = |sender, direction| sender == tx.sender && direction == tx.direction;
        if state
            .txs
            .iter()
            .any(|queued| chain(queued.sender, queued.direction) && queued.nonce < tx.nonce)
        {
            return false;
        }
        if state.in_flight.contains_key(&tx.hash) {
            return state.ledger.read(|data| {
                data.users
                    .get(&tx.hash)
                    .is_none_or(|user| user.location == UserLocation::Composing)
            });
        }
        let Some(index) = state.txs.iter().position(|queued| {
            queued.hash == tx.hash
                && chain(queued.sender, queued.direction)
                && queued.nonce == tx.nonce
        }) else {
            return false;
        };
        // Claiming preserves identity and membership ownership; only this
        // user's ledger location changes. Avoid rescanning every queued user
        // for each candidate in a large snapshot.
        if state.ledger.read(|data| data.users.contains_key(&tx.hash)) {
            let claimed = &state.txs[index];
            let accepted = state
                .ledger
                .update(|data| {
                    let user = data.users.get_mut(&tx.hash).ok_or("source owner changed")?;
                    if user.location != UserLocation::Queued {
                        return Ok(false);
                    }
                    user.tx = claimed.clone();
                    user.location = UserLocation::Composing;
                    Ok(true)
                })
                .expect("cannot claim transaction ownership; held pool stopped");
            if !accepted {
                return false;
            }
        }
        let claimed = state.txs.remove(index).expect("matched queued transaction");
        state.by_hash.remove(&claimed.hash);
        state.mark_in_flight(&claimed);
        true
    }

    /// Drain up to `max` held txs (FIFO). Capping bounds the users affected by
    /// one failed settlement episode, at the cost of more slots to drain a
    /// large pool. Per-user reconciliation decides their eventual disposition.
    pub fn pop_n(&self, max: usize) -> Vec<HeldTx> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let count = max.min(state.txs.len());
        let drained: Vec<_> = state.txs.drain(..count).collect();
        for tx in &drained {
            state.by_hash.remove(&tx.hash);
            state.mark_in_flight(tx);
        }
        state.update_recovery_ownership();
        drained
    }

    /// Number of currently-held txs.
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .expect("held_pool mutex poisoned")
            .txs
            .len()
    }

    /// True iff the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Includes newly admitted users, which enter the submission ledger only
    /// when an attempt begins. Replacement, eviction and release remove this
    /// ownership, while draining/requeueing preserves it.
    pub(crate) fn owns(&self, hash: TxHash) -> bool {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        state.by_hash.contains(&hash) || state.in_flight.contains_key(&hash)
    }

    /// Cross-chain sources must not enter ordinary L2 sequencing when Reth
    /// reinjects an orphaned Sync block. Include the unfinalized ledger even
    /// after resolution/eviction: pool reinjection is asynchronous and must
    /// not race an ownership transfer. This reads the existing source records;
    /// it does not maintain a second transaction registry.
    #[must_use]
    pub fn is_cross_chain_source(&self, hash: TxHash) -> bool {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        state.by_hash.contains(&hash)
            || state.in_flight.contains_key(&hash)
            || state.ledger.read(|data| data.users.contains_key(&hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    fn insert(pool: &HeldPool, tx: HeldTx, on_chain: u64) {
        pool.push_contiguous(tx, on_chain).unwrap();
    }

    fn tx(byte: u8) -> HeldTx {
        tx_dir(byte, Direction::Inbound)
    }

    fn tx_dir(byte: u8, direction: Direction) -> HeldTx {
        HeldTx {
            raw_tx: Bytes::from(vec![byte; 32]),
            hash: TxHash::from(B256::repeat_byte(byte)),
            attempts: 0,
            max_fee_per_gas: u128::from(byte),
            priority_fee_per_gas: u128::from(byte),
            sender: Address::repeat_byte(byte),
            nonce: u64::from(byte),
            direction,
        }
    }

    #[test]
    fn pre_pr_unsubmitted_eviction_remains_observable_until_readmission() {
        let pool = HeldPool::new(Default::default());
        let mut root = tx_dir(1, Direction::Outbound);
        root.nonce = 0;
        let mut suffix = root.clone();
        suffix.hash = B256::repeat_byte(2);
        suffix.nonce = 1;
        insert(&pool, root.clone(), 0);
        insert(&pool, suffix.clone(), 0);
        let original_claim = pool.pop_n(1);
        let evicted = pool.evict_chain_at_or_above(root.sender, root.direction, root.nonce);
        assert_eq!(evicted, vec![suffix.clone()]);
        for hash in [root.hash, suffix.hash] {
            let status = pool
                .transaction_status(hash)
                .expect("retain a terminal observation");
            assert_eq!(status["lifecycle"], "terminal");
            assert!(status["ownerGeneration"].is_null());
        }
        assert_eq!(
            pool.transaction_status(root.hash).unwrap()["ownership"]["reason"],
            "PoolDiscarded"
        );
        assert_eq!(
            pool.transaction_status(suffix.hash).unwrap()["ownership"]["reason"],
            "NonceGap"
        );
        assert!(pool.push_contiguous(root.clone(), 1).is_err());
        assert_eq!(
            pool.transaction_status(root.hash).unwrap()["lifecycle"],
            "terminal"
        );
        pool.requeue_composing(original_claim.clone());
        assert!(pool.is_empty(), "observation must not authorize recovery");
        insert(&pool, root.clone(), 0);
        assert_eq!(
            pool.transaction_status(root.hash).unwrap()["lifecycle"],
            "queued"
        );
        pool.requeue_composing(original_claim);
        assert_eq!(pool.candidates(), vec![root.clone()]);
        assert!(pool.transaction_status(root.hash).unwrap()["ownership"].is_null());
    }

    #[test]
    fn local_removal_history_is_bounded_and_replacements_keep_current_identity() {
        let pool = HeldPool::new(Default::default());
        let mut original = tx(1);
        original.nonce = 0;
        insert(&pool, original.clone(), 0);
        let mut replacement = original.clone();
        replacement.hash = B256::repeat_byte(2);
        replacement.max_fee_per_gas = 2;
        replacement.priority_fee_per_gas = 2;
        insert(&pool, replacement.clone(), 0);
        let removed = pool.transaction_status(original.hash).unwrap();
        assert_eq!(removed["lifecycle"], "terminal");
        assert_eq!(
            removed["ownership"]["replacementHash"],
            serde_json::json!(replacement.hash)
        );
        assert_eq!(
            pool.transaction_status(replacement.hash).unwrap()["lifecycle"],
            "queued"
        );
        assert_eq!(pool.candidates(), vec![replacement.clone()]);
        let _ = pool.evict_chain_at_or_above(replacement.sender, replacement.direction, 0);
        for index in 0..=status::MAX_RECENT_REMOVALS {
            let mut held = original.clone();
            held.hash = B256::from(alloy_primitives::U256::from(index + 1));
            insert(&pool, held.clone(), 0);
            let _ = pool.evict_chain_at_or_above(held.sender, held.direction, 0);
        }
        assert!(pool.transaction_status(original.hash).is_none());
        assert!(
            pool.transaction_status(B256::from(alloy_primitives::U256::from(1)))
                .is_none()
        );
        assert!(
            pool.transaction_status(B256::from(alloy_primitives::U256::from(
                status::MAX_RECENT_REMOVALS + 1
            )))
            .is_some()
        );
        assert_eq!(pool.queue_status()["queued"], 0);
        assert_eq!(pool.queue_status()["inFlight"], 0);
    }

    #[test]
    #[ignore = "manual component profile; records timings without timing assertions"]
    fn profile_blocked_backlog_discovery() {
        use std::{hint::black_box, time::Instant};
        for count in [1_000u64, 4_000, 10_000] {
            let pool = HeldPool::new(Default::default());
            let started = Instant::now();
            for nonce in 0..count {
                let mut held = tx_dir(1, Direction::Outbound);
                held.nonce = nonce;
                held.hash = B256::from(alloy_primitives::U256::from(nonce + 1));
                held.raw_tx = Bytes::from(vec![1; 1024]);
                insert(&pool, held, 0);
            }
            let mut ready = Vec::new();
            for byte in 2..=65 {
                let held = tx(byte);
                insert(&pool, held.clone(), held.nonce);
                ready.push(held);
            }
            let admission = started.elapsed();
            assert_eq!(pool.len(), count as usize + ready.len());
            let started = Instant::now();
            let snapshot = black_box(pool.candidates());
            let snapshot_time = started.elapsed();
            assert_eq!(snapshot.len(), pool.len());
            let started = Instant::now();
            for held in &ready {
                assert!(pool.claim(held));
            }
            let claim_time = started.elapsed();
            assert_eq!(pool.len(), count as usize);
            eprintln!(
                "queue_profile count={count} envelope_bytes={} admission_us={} snapshot_us={} claim_64_us={}",
                count * 1024,
                admission.as_micros(),
                snapshot_time.as_micros(),
                claim_time.as_micros()
            );
            pool.release_in_flight_batch(&ready);
        }
    }

    #[test]
    fn queue_counts_track_both_directions_replacement_recovery_and_release() {
        let pool = HeldPool::new(Default::default());
        let check = |iq, ifl, oq, ofl| {
            assert_eq!(
                pool.queue_snapshot().counts,
                [
                    (
                        Direction::Inbound,
                        QueueCounts {
                            queued: iq,
                            in_flight: ifl
                        }
                    ),
                    (
                        Direction::Outbound,
                        QueueCounts {
                            queued: oq,
                            in_flight: ofl
                        }
                    ),
                ]
            );
            let rpc = pool.queue_status();
            assert_eq!(rpc["queued"], iq + oq);
            assert_eq!(rpc["inFlight"], ifl + ofl);
        };
        check(0, 0, 0, 0);
        let inbound = tx_dir(1, Direction::Inbound);
        let outbound = tx_dir(2, Direction::Outbound);
        insert(&pool, inbound.clone(), inbound.nonce);
        insert(&pool, inbound.clone(), inbound.nonce); // Exact duplicates add no membership.
        insert(&pool, outbound.clone(), outbound.nonce);
        check(1, 0, 1, 0);
        let replacement = HeldTx {
            hash: B256::repeat_byte(99),
            max_fee_per_gas: 100,
            priority_fee_per_gas: 100,
            ..inbound.clone()
        };
        insert(&pool, replacement.clone(), replacement.nonce);
        check(1, 0, 1, 0);
        let drained = pool.pop_all();
        check(0, 1, 0, 1);
        pool.push_front_batch(drained.clone());
        pool.push_front_batch(drained);
        check(1, 0, 1, 0);
        assert!(pool.claim(&replacement));
        check(0, 1, 1, 0);
        pool.release_in_flight_batch(&[replacement]);
        check(0, 0, 1, 0);
        assert_eq!(
            pool.evict_chain_at_or_above(outbound.sender, outbound.direction, outbound.nonce),
            vec![outbound],
        );
        check(0, 0, 0, 0);
        assert_eq!(
            pool.queue_snapshot().counts,
            HeldPool::new(Default::default()).queue_snapshot().counts
        );
    }

    #[test]
    fn queue_counts_do_not_lose_membership_during_concurrent_drain_and_recovery() {
        let pool = HeldPool::new(Default::default());
        for tx in [
            tx_dir(1, Direction::Inbound),
            tx_dir(2, Direction::Outbound),
        ] {
            let nonce = tx.nonce;
            insert(&pool, tx, nonce);
        }
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let barrier = &barrier;
            let pool = &pool;
            let writer = scope.spawn(move || {
                for _ in 0..100 {
                    barrier.wait();
                    let drained = pool.pop_all();
                    pool.push_front_batch(drained);
                    barrier.wait();
                }
            });
            for _ in 0..100 {
                barrier.wait();
                for (_, counts) in pool.queue_snapshot().counts {
                    assert_eq!(counts.queued + counts.in_flight, 1);
                }
                barrier.wait();
            }
            writer.join().unwrap();
        });
    }

    #[test]
    fn push_then_pop_returns_fifo() {
        let pool = HeldPool::new(Default::default());
        insert(&pool, tx(1), 1);
        insert(&pool, tx(2), 2);
        insert(&pool, tx(3), 3);
        let drained = pool.pop_all();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].hash, TxHash::from(B256::repeat_byte(1)));
        assert_eq!(drained[2].hash, TxHash::from(B256::repeat_byte(3)));
        assert!(pool.is_empty());
    }

    #[test]
    fn compose_error_restores_only_remaining_owners_without_undoing_classification() {
        let pool = HeldPool::new(Default::default());
        let original: Vec<_> = (1..=5).map(tx).collect();
        for tx in &original {
            insert(&pool, tx.clone(), tx.nonce);
        }
        // A setup failure before classification restores the complete FIFO.
        pool.requeue_composing(pool.pop_all());
        let drained = pool.pop_all();
        assert_eq!(drained, original);

        let evicted = &original[0];
        assert!(
            pool.evict_chain_at_or_above(evicted.sender, evicted.direction, evicted.nonce)
                .is_empty()
        );
        pool.release_in_flight_batch(&original[1..2]);
        // A source already requeued by an inner recovery can be replaced by
        // ingress before an outer fallback fails. Preserve that new owner.
        pool.push_front_batch(vec![original[2].clone()]);
        let mut replacement = original[2].clone();
        replacement.hash = B256::repeat_byte(99);
        replacement.max_fee_per_gas *= 2;
        replacement.priority_fee_per_gas *= 2;
        replacement.attempts = 1;
        insert(&pool, replacement.clone(), replacement.nonce);

        pool.requeue_composing(drained.clone());
        pool.requeue_composing(drained);
        assert_eq!(
            pool.pop_all(),
            vec![original[3].clone(), original[4].clone(), replacement]
        );
        assert!(!pool.owns(original[0].hash));
        assert!(!pool.owns(original[1].hash));
        assert!(!pool.owns(original[2].hash));
    }

    #[test]
    fn ownership_includes_unsubmitted_groups_and_tracks_their_entire_pool_lifecycle() {
        let ledger = Arc::new(OptimisticallyIncluded::default());
        let pool = HeldPool::new(Arc::clone(&ledger));
        let original = tx(1);
        insert(&pool, original.clone(), 1);
        assert!(ledger.read(|data| data.users.is_empty()));
        assert!(pool.owns(original.hash));
        assert!(pool.is_cross_chain_source(original.hash));
        let drained = pool.pop_all();
        assert!(pool.is_empty());
        assert!(pool.owns(original.hash));
        assert!(pool.is_cross_chain_source(original.hash));
        pool.push_front_batch(drained);
        assert!(pool.owns(original.hash));

        let mut replacement = original.clone();
        replacement.hash = B256::repeat_byte(9);
        replacement.max_fee_per_gas = 2;
        replacement.priority_fee_per_gas = 2;
        insert(&pool, replacement.clone(), 1);
        assert!(!pool.owns(original.hash));
        assert!(pool.owns(replacement.hash));
        let evicted = pool.evict_chain_at_or_above(replacement.sender, replacement.direction, 1);
        assert_eq!(evicted.len(), 1);
        assert!(!pool.owns(replacement.hash));

        insert(&pool, original.clone(), 1);
        let drained = pool.pop_all();
        pool.release_in_flight_batch(&drained);
        assert!(!pool.owns(original.hash));
    }

    #[test]
    fn push_dedupes_by_hash() {
        let pool = HeldPool::new(Default::default());
        let original = tx(1);
        insert(&pool, original.clone(), 1);
        pool.push_contiguous(original.clone(), 1).unwrap();
        assert_eq!(pool.len(), 1);

        let drained = pool.pop_all();
        pool.push_contiguous(original.clone(), 1).unwrap();
        let mut replacement = original;
        replacement.hash = TxHash::from(B256::repeat_byte(9));
        assert_eq!(
            pool.push_contiguous(replacement, 1).unwrap_err(),
            AdmissionError::Nonce {
                expected: 2,
                on_chain: 1
            }
        );
        pool.release_in_flight_batch(&drained);
    }

    #[test]
    fn underpriced_replacement_is_rejected() {
        let pool = HeldPool::new(Default::default());
        let mut original = tx(1);
        original.max_fee_per_gas = 100;
        original.priority_fee_per_gas = 10;
        insert(&pool, original, 1);

        // Cap bumped but tip not (geth rule: BOTH must bump).
        let mut replacement = tx(1);
        replacement.hash = TxHash::from(B256::repeat_byte(9));
        replacement.max_fee_per_gas = 110;
        replacement.priority_fee_per_gas = 10;
        assert_eq!(
            pool.push_contiguous(replacement.clone(), 1).unwrap_err(),
            AdmissionError::UnderpricedReplacement {
                offered_max_fee: 110,
                required_max_fee: 110,
                offered_priority_fee: 10,
                required_priority_fee: 11
            }
        );

        // Tip bumped but cap not.
        replacement.max_fee_per_gas = 109;
        replacement.priority_fee_per_gas = 11;
        assert!(pool.push_contiguous(replacement.clone(), 1).is_err());

        // Both bumped → replaces.
        replacement.max_fee_per_gas = 110;
        pool.push_contiguous(replacement.clone(), 1).unwrap();
        assert_eq!(pool.pop_all()[0].hash, replacement.hash);
    }

    #[test]
    fn contiguity_is_isolated_per_direction() {
        // Same EOA, two independent nonce chains. Admission / draining one
        // direction must never touch the other.
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(7);
        let mk = |nonce: u64, dir: Direction, h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce,
            direction: dir,
        };
        insert(&pool, mk(0, Direction::Inbound, 1), 0);
        insert(&pool, mk(1, Direction::Inbound, 2), 0);
        insert(&pool, mk(0, Direction::Outbound, 3), 0);
        insert(&pool, mk(1, Direction::Outbound, 4), 0);

        let evicted = pool.evict_chain_at_or_above(sender, Direction::Outbound, 1);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].nonce, 1);
        assert_eq!(evicted[0].direction, Direction::Outbound);

        let remaining = pool.pop_n(usize::MAX);
        assert_eq!(
            remaining
                .iter()
                .filter(|tx| tx.direction == Direction::Inbound)
                .count(),
            2,
            "inbound chain must remain untouched"
        );
        assert!(
            remaining
                .iter()
                .any(|tx| { tx.direction == Direction::Outbound && tx.nonce == 0 })
        );
    }

    #[test]
    fn queued_nonce_replacement_preserves_position_and_suffix() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(9);
        let other_sender = Address::repeat_byte(8);
        let mk = |sender: Address, nonce: u64, direction: Direction, h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce,
            direction,
        };

        insert(&pool, mk(sender, 0, Direction::Inbound, 1), 0);
        insert(&pool, mk(other_sender, 0, Direction::Inbound, 10), 0);
        insert(&pool, mk(sender, 1, Direction::Inbound, 2), 0);
        insert(&pool, mk(sender, 0, Direction::Outbound, 20), 0);
        insert(&pool, mk(sender, 2, Direction::Inbound, 3), 0);
        insert(&pool, mk(other_sender, 1, Direction::Inbound, 11), 0);
        insert(&pool, mk(sender, 1, Direction::Outbound, 21), 0);

        pool.push_contiguous(mk(sender, 1, Direction::Inbound, 9), 0)
            .unwrap();

        let drained = pool.pop_all();
        assert!(drained.iter().any(|tx| {
            tx.sender == sender
                && tx.direction == Direction::Outbound
                && tx.nonce == 1
                && tx.hash == TxHash::from(B256::repeat_byte(21))
        }));
        let hashes: Vec<_> = drained.iter().map(|tx| tx.hash).collect();
        assert_eq!(
            hashes,
            vec![
                TxHash::from(B256::repeat_byte(1)),
                TxHash::from(B256::repeat_byte(10)),
                TxHash::from(B256::repeat_byte(9)),
                TxHash::from(B256::repeat_byte(20)),
                TxHash::from(B256::repeat_byte(3)),
                TxHash::from(B256::repeat_byte(11)),
                TxHash::from(B256::repeat_byte(21)),
            ]
        );
    }

    #[test]
    fn queued_replacement_allows_lower_in_flight_but_not_in_flight_nonce() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(9);
        let mk = |nonce: u64, h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce,
            direction: Direction::Inbound,
        };
        for nonce in 0..=2 {
            insert(&pool, mk(nonce, nonce as u8 + 1), 0);
        }
        let drained = pool.pop_n(1);
        assert_eq!(drained[0].nonce, 0);

        pool.push_contiguous(mk(1, 9), 0).unwrap();
        assert_eq!(
            pool.push_contiguous(mk(0, 8), 0).unwrap_err(),
            AdmissionError::Nonce {
                expected: 3,
                on_chain: 0
            }
        );
        pool.push_contiguous(mk(2, 7), 0).unwrap();

        let queued = pool.pop_all();
        assert_eq!(queued.len(), 2);
        assert_eq!(queued[0].hash, TxHash::from(B256::repeat_byte(9)));
        assert_eq!(queued[1].hash, TxHash::from(B256::repeat_byte(7)));
    }

    #[test]
    fn contiguous_push_handles_landed_but_not_derived_reservation() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(0xa);
        let mk = |nonce: u64, h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce,
            direction: Direction::Inbound,
        };

        pool.push_contiguous(mk(0, 1), 0).unwrap();
        let drained = pool.pop_n(1);
        assert_eq!(drained.len(), 1);
        assert!(pool.is_empty());

        // Before landing, nonce 0 is supplied by the in-flight reservation.
        pool.push_contiguous(mk(1, 2), 0).unwrap();

        // Once nonce 0 lands, the source-chain nonce advances to 1 while the
        // reservation remains until Deriver confirmation. It must not be
        // counted a second time; queued nonce 1 makes nonce 2 the next value.
        pool.push_contiguous(mk(2, 3), 1).unwrap();
        let err = pool.push_contiguous(mk(4, 4), 1).unwrap_err();
        assert_eq!(
            err,
            AdmissionError::Nonce {
                expected: 3,
                on_chain: 1
            }
        );

        pool.release_in_flight_batch(&drained);
    }

    #[test]
    fn max_nonce_can_be_replaced_while_queued_but_not_in_flight() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(0xd);
        let mk = |h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce: u64::MAX,
            direction: Direction::Inbound,
        };

        insert(&pool, mk(1), u64::MAX);
        pool.push_contiguous(mk(2), u64::MAX).unwrap();
        let drained = pool.pop_all();
        assert_eq!(drained[0].hash, TxHash::from(B256::repeat_byte(2)));
        assert_eq!(
            pool.push_contiguous(mk(3), u64::MAX).unwrap_err(),
            AdmissionError::Nonce {
                expected: u64::MAX,
                on_chain: u64::MAX
            }
        );
    }

    #[test]
    fn nonce_chain_eviction_removes_queued_and_in_flight_suffix_atomically() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(0xe);
        let mk = |nonce: u64, h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce,
            direction: Direction::Inbound,
        };
        for nonce in 0..=3 {
            insert(&pool, mk(nonce, nonce as u8 + 1), 0);
        }
        let drained = pool.pop_n(3);
        assert_eq!(
            drained.iter().map(|tx| tx.nonce).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        let queued = pool.evict_chain_at_or_above(sender, Direction::Inbound, 1);

        assert_eq!(
            queued.iter().map(|tx| tx.nonce).collect::<Vec<_>>(),
            vec![3]
        );
        assert!(
            !pool
                .state
                .lock()
                .unwrap()
                .in_flight
                .values()
                .any(|(s, d, nonce)| { *s == sender && *d == Direction::Inbound && *nonce >= 1 }),
            "inclusive eviction must release the root's own reservation"
        );
        pool.push_contiguous(mk(1, 5), 0).unwrap();
    }

    #[test]
    fn recovery_with_multiple_nonce_gaps_preserves_other_chains_and_fifo() {
        use crate::optimistic::NonceGap;
        use Direction::{Inbound, Outbound};

        let a = Address::repeat_byte(0xa);
        let b = Address::repeat_byte(0xb);
        let c = Address::repeat_byte(0xc);
        let d = Address::repeat_byte(0xd);
        let users = [
            (a, Inbound, 0),
            (a, Inbound, 1),
            (a, Outbound, 0),
            (b, Inbound, 0),
            (c, Inbound, 0),
            (a, Inbound, 2),
            (a, Outbound, 1),
            (b, Inbound, 1),
            (c, Inbound, 1),
            (c, Outbound, u64::MAX),
            (d, Inbound, u64::MAX),
            (d, Outbound, u64::MAX),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (sender, direction, nonce))| HeldTx {
            sender,
            nonce,
            direction,
            ..tx(u8::try_from(index + 1).unwrap())
        })
        .collect::<Vec<_>>();

        for reverse in [false, true] {
            let pool = HeldPool::new(Default::default());
            for user in &users {
                let on_chain = if user.nonce == u64::MAX { u64::MAX } else { 0 };
                pool.push_contiguous(user.clone(), on_chain).unwrap();
            }
            pool.pop_n(5);
            let check_membership = |queued: &[usize], in_flight: &[usize]| {
                let state = pool.state.lock().unwrap();
                let expected_queue = queued.iter().map(|&i| users[i].hash).collect::<Vec<_>>();
                assert_eq!(
                    state.txs.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
                    expected_queue,
                );
                assert_eq!(state.by_hash, expected_queue.into_iter().collect());
                assert_eq!(
                    state.in_flight,
                    in_flight
                        .iter()
                        .map(|&i| {
                            let tx = &users[i];
                            (tx.hash, (tx.sender, tx.direction, tx.nonce))
                        })
                        .collect(),
                );
            };
            let untouched_queue = [5, 6, 7, 8, 9, 10, 11];
            let untouched_flight = [0, 1, 2, 3, 4];
            pool.state.lock().unwrap().apply_resolution(&[]);
            check_membership(&untouched_queue, &untouched_flight);
            pool.state.lock().unwrap().apply_resolution(&[NonceGap {
                sender: Address::repeat_byte(0xe),
                direction: Inbound,
                nonce: 0,
            }]);
            check_membership(&untouched_queue, &untouched_flight);

            // Overlapping gaps, independent senders and an inclusive MAX root.
            let mut gaps = [
                NonceGap {
                    sender: a,
                    direction: Inbound,
                    nonce: 2,
                },
                NonceGap {
                    sender: b,
                    direction: Inbound,
                    nonce: 0,
                },
                NonceGap {
                    sender: a,
                    direction: Inbound,
                    nonce: 1,
                },
                NonceGap {
                    sender: d,
                    direction: Inbound,
                    nonce: u64::MAX,
                },
            ];
            if reverse {
                gaps.reverse();
            }
            pool.state.lock().unwrap().apply_resolution(&gaps);
            check_membership(&[6, 8, 9, 11], &[0, 2, 4]);
            pool.state.lock().unwrap().apply_resolution(&gaps);
            check_membership(&[6, 8, 9, 11], &[0, 2, 4]);
        }
    }

    #[test]
    fn recovery_preserves_newer_replacement_and_discards_old_suffix() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(0xf);
        let mk = |nonce: u64, h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce,
            direction: Direction::Inbound,
        };
        let old_root = mk(0, 1);
        let old_suffix = mk(1, 2);
        insert(&pool, old_root.clone(), 0);
        insert(&pool, old_suffix.clone(), 0);
        let old_batch = pool.pop_n(2);

        pool.release_in_flight_batch(&old_batch);
        let replacement = mk(0, 9);
        insert(&pool, replacement.clone(), 0);
        pool.push_front_batch(old_batch);

        let state = pool.state.lock().unwrap();
        assert_eq!(state.txs.len(), 1);
        assert_eq!(state.txs[0].hash, replacement.hash);
        assert!(!state.in_flight.contains_key(&old_root.hash));
        assert!(!state.in_flight.contains_key(&old_suffix.hash));
    }

    #[test]
    fn same_hash_recovery_is_idempotent() {
        let pool = HeldPool::new(Default::default());
        let original = tx(1);
        insert(&pool, original.clone(), 1);
        let drained = pool.pop_n(1);

        pool.push_front_batch(drained.clone());
        pool.push_front_batch(drained);

        let state = pool.state.lock().unwrap();
        assert_eq!(state.txs.len(), 1);
        assert_eq!(state.txs[0].hash, original.hash);
        assert!(state.in_flight.is_empty());
    }

    #[test]
    fn pop_n_drains_fifo_prefix() {
        let pool = HeldPool::new(Default::default());
        let sender_a = Address::repeat_byte(0xa);
        let sender_b = Address::repeat_byte(0xb);
        let mk = |sender: Address, nonce: u64, h: u8| HeldTx {
            raw_tx: Bytes::from(vec![h; 4]),
            hash: TxHash::from(B256::repeat_byte(h)),
            attempts: 0,
            max_fee_per_gas: u128::from(h),
            priority_fee_per_gas: u128::from(h),
            sender,
            nonce,
            direction: Direction::Inbound,
        };

        insert(&pool, mk(sender_a, 0, 1), 0);
        insert(&pool, mk(sender_a, 1, 2), 0);
        insert(&pool, mk(sender_a, 2, 3), 0);
        insert(&pool, mk(sender_b, 0, 4), 0);

        let drained = pool.pop_n(3);
        let hashes: Vec<_> = drained.iter().map(|tx| tx.hash).collect();
        assert_eq!(
            hashes,
            vec![
                TxHash::from(B256::repeat_byte(1)),
                TxHash::from(B256::repeat_byte(2)),
                TxHash::from(B256::repeat_byte(3)),
            ]
        );
        assert_eq!(pool.pop_all()[0].hash, TxHash::from(B256::repeat_byte(4)));
    }
    #[test]
    fn snapshot_claim_rejects_replacements_and_restores_only_claimed_owners() {
        let pool = HeldPool::new(Default::default());
        let original = tx(1);
        insert(&pool, original.clone(), original.nonce);
        let snapshot = pool.candidates();
        let replacement = HeldTx {
            hash: B256::repeat_byte(99),
            max_fee_per_gas: 100,
            priority_fee_per_gas: 100,
            ..original.clone()
        };
        insert(&pool, replacement.clone(), replacement.nonce);
        assert!(!pool.claim(&snapshot[0]));
        pool.requeue_composing(snapshot);
        assert_eq!(pool.candidates(), vec![replacement.clone()]);
        assert!(pool.claim(&replacement));
        assert!(
            pool.claim(&replacement),
            "bounded recomposition keeps its ownership"
        );
        assert_eq!(pool.queue_status()["inFlight"], 1);
        pool.requeue_composing(vec![replacement.clone()]);
        assert_eq!(pool.candidates(), vec![replacement]);
    }

    #[test]
    fn recovered_predecessor_invalidates_a_successors_old_snapshot_claim() {
        let pool = HeldPool::new(Default::default());
        let successor = HeldTx { nonce: 2, ..tx(1) };
        insert(&pool, successor.clone(), 2);
        let snapshot = pool.candidates();
        let predecessor = HeldTx {
            nonce: 1,
            hash: B256::repeat_byte(33),
            ..successor.clone()
        };
        insert(&pool, predecessor.clone(), 1);
        assert!(!pool.claim(&snapshot[0]));
        assert!(pool.claim(&predecessor));
        assert!(pool.claim(&successor));
    }

    #[test]
    fn fifo_retry_restores_unclaimed_older_sources_before_claimed_younger_work() {
        let pool = HeldPool::new(Default::default());
        let users: Vec<_> = (1..=4).map(tx).collect();
        for user in &users {
            insert(&pool, user.clone(), user.nonce);
        }
        assert!(pool.claim(&users[1]));
        assert!(pool.claim(&users[3]));
        pool.retry_fifo_prefix(&users, 1);
        for _ in 0..6 {
            assert_eq!(
                pool.candidates(),
                users[..1],
                "a missed deadline cannot rotate younger work forward"
            );
        }
        assert_eq!(pool.pop_all(), users);
    }

    #[test]
    fn fifo_boundary_tracks_replacement_and_clears_after_consumption() {
        let pool = HeldPool::new(Default::default());
        let users: Vec<_> = (1..=3).map(tx).collect();
        for user in &users {
            insert(&pool, user.clone(), user.nonce);
        }
        let replacement = HeldTx {
            hash: B256::repeat_byte(98),
            max_fee_per_gas: 100,
            priority_fee_per_gas: 100,
            ..users[0].clone()
        };
        insert(&pool, replacement.clone(), replacement.nonce);
        pool.retry_fifo_prefix(&users, 1);
        assert_eq!(pool.candidates(), vec![replacement.clone()]);
        assert!(pool.claim(&replacement));
        pool.release_in_flight_batch(&[replacement]);
        assert_eq!(pool.candidates(), users[1..]);
    }

    #[test]
    fn fifo_retry_keeps_recovered_nonce_predecessors_and_does_not_revive_removed_users() {
        let pool = HeldPool::new(Default::default());
        let users: Vec<_> = (1..=3).map(tx).collect();
        for user in &users {
            insert(&pool, user.clone(), user.nonce);
        }
        assert!(pool.claim(&users[1]));
        pool.release_in_flight_batch(&users[1..2]);
        let predecessor = HeldTx {
            nonce: users[0].nonce - 1,
            hash: B256::repeat_byte(98),
            ..users[0].clone()
        };
        insert(&pool, predecessor.clone(), predecessor.nonce);
        pool.retry_fifo_prefix(&users, 1);
        assert_eq!(
            pool.candidates(),
            vec![predecessor.clone(), users[0].clone()]
        );
        assert_eq!(
            pool.pop_all(),
            vec![predecessor, users[0].clone(), users[2].clone()]
        );
        assert!(!pool.owns(users[1].hash));
    }

    #[test]
    fn blocked_fifo_boundary_reopens_independent_work_without_charging_attempts() {
        let pool = HeldPool::new(Default::default());
        let users: Vec<_> = (1..=3).map(tx).collect();
        for user in &users {
            insert(&pool, user.clone(), user.nonce);
        }
        pool.retry_fifo_prefix(&users, 1);
        assert_eq!(pool.candidates(), users[..1]);
        let source = SourceState {
            l1: eez_l1::L1BlockRef {
                number: 1,
                hash: B256::repeat_byte(1),
            },
            l2: eez_l1::L1BlockRef {
                number: 2,
                hash: B256::repeat_byte(2),
            },
        };
        pool.record_blocked(&users[0], eez_protocol::SourceCondition::Fee, source);
        assert_eq!(pool.candidates(), users);
        assert!(pool.candidates().iter().all(|user| user.attempts == 0));
    }

    #[test]
    fn sparse_maximum_nonce_does_not_prevent_repair_and_full_chain_overflows() {
        let pool = HeldPool::new(Default::default());
        let maximum = HeldTx {
            nonce: u64::MAX,
            ..tx(1)
        };
        insert(&pool, maximum.clone(), u64::MAX);
        let missing = HeldTx {
            nonce: 0,
            hash: B256::repeat_byte(97),
            ..maximum.clone()
        };
        insert(&pool, missing.clone(), 0);
        assert_eq!(pool.candidates(), vec![missing, maximum.clone()]);
        let state = pool.state.lock().unwrap();
        assert_eq!(
            state.next_expected_nonce(maximum.sender, maximum.direction, u64::MAX),
            None
        );
    }

    #[test]
    fn queue_observations_follow_identity_and_expire_on_same_height_replacement() {
        use eez_l1::L1BlockRef;
        let pool = HeldPool::new(Default::default());
        let first = tx(1);
        let second = HeldTx {
            nonce: 2,
            hash: B256::repeat_byte(96),
            ..first.clone()
        };
        insert(&pool, first.clone(), 1);
        insert(&pool, second.clone(), 1);
        let source = SourceState {
            l1: L1BlockRef {
                number: 10,
                hash: B256::repeat_byte(10),
            },
            l2: L1BlockRef {
                number: 20,
                hash: B256::repeat_byte(20),
            },
        };
        pool.begin_evaluation(source);
        pool.record_blocked(
            &first,
            eez_protocol::SourceCondition::Nonce {
                transaction_nonce: 1,
                required_nonce: 0,
            },
            source,
        );
        assert_eq!(pool.queue_status()["blocked"], 2);
        assert_eq!(
            pool.transaction_status(second.hash).unwrap()["blocking"]["condition"]["required_nonce"],
            0
        );
        assert!(!pool.queue_status().to_string().contains("raw_tx"));
        let changed = SourceState {
            l2: L1BlockRef {
                hash: B256::repeat_byte(21),
                ..source.l2
            },
            ..source
        };
        pool.begin_evaluation(changed);
        assert_eq!(pool.queue_status()["blocked"], 0);
        assert_eq!(pool.queue_status()["unknown"], 2);
        assert_eq!(
            pool.transaction_status(first.hash).unwrap()["blocking"]["current"],
            false
        );
        pool.record_ready(&first, changed);
        assert_eq!(pool.queue_status()["ready"], 1);
        let replacement = HeldTx {
            hash: B256::repeat_byte(95),
            max_fee_per_gas: 100,
            priority_fee_per_gas: 100,
            ..first
        };
        insert(&pool, replacement, 1);
        assert_eq!(pool.queue_status()["ready"], 0);
        assert_eq!(pool.queue_status()["unknown"], 2);
        pool.invalidate_after_l1(9).unwrap();
        assert!(pool.queue_status()["sourceState"].is_null());
    }
}
