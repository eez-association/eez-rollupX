//! Attempt-scoped transaction ownership. A height or an observer verdict is
//! never proof of completion; only canonical derivation plus source outcomes
//! can resolve an attempt.

use std::path::Path;
use std::sync::Arc;

use alloy_primitives::{Address, B256, Bytes};
use eez_l1::{BundleTarget, L1BlockRef};
use eez_protocol::settlement_manifest::SettlementManifest;
use reth_primitives_traits::SealedHeader;

use crate::held_pool::HeldTx;
use crate::ingress::Direction;
pub use crate::settlement_journal::AttemptId;
use crate::settlement_journal::{
    AttemptRecord, JournalState, OwnedUser, SettlementJournal, SubmissionObservation,
    TerminalReason, UserLocation,
};

/// Conclusive outcome supplied by canonical source reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOutcome {
    Terminal(TerminalReason),
    /// Canonical source nonce at the checked snapshot, not an optimistic
    /// receipt or pending nonce.
    Retryable {
        source_nonce: u64,
    },
}

/// Nonce suffix that cannot progress after a retry budget is exhausted.
#[derive(Debug, Clone, Copy)]
pub struct NonceGap {
    pub sender: Address,
    pub direction: Direction,
    pub nonce: u64,
}

/// Read-only copy taken before asynchronous evidence lookups.
#[derive(Debug, Clone)]
pub struct PendingAttempt {
    pub id: AttemptId,
    pub sync_height: u64,
    pub parent: SealedHeader<alloy_consensus::Header>,
    pub txs: Vec<HeldTx>,
    pub manifest: SettlementManifest,
    pub post_batch: Bytes,
    pub requested_target: BundleTarget,
    pub dispatched_target: Option<(u64, u64)>,
    pub observation: SubmissionObservation,
}

/// All state changes are journaled before the held pool is updated.
#[derive(Debug, Default)]
pub struct OptimisticallyIncluded {
    journal: Arc<SettlementJournal>,
}

impl OptimisticallyIncluded {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            journal: Arc::new(SettlementJournal::open(path)?),
        })
    }

    #[must_use]
    pub fn journal(&self) -> Arc<SettlementJournal> {
        Arc::clone(&self.journal)
    }

    pub(crate) fn has_users(&self, id: AttemptId) -> bool {
        self.journal.read(|state| {
            state
                .attempts
                .get(&id.generation)
                .is_some_and(|attempt| attempt.id == id && !attempt.txs.is_empty())
        })
    }

    /// Persist a freshly prepared attempt before handing it to the submitter.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &self,
        sync_height: u64,
        post_batch_hash: B256,
        parent: SealedHeader<alloy_consensus::Header>,
        txs: Vec<HeldTx>,
        manifest: SettlementManifest,
        post_batch: Bytes,
        target: BundleTarget,
    ) -> Result<AttemptId, String> {
        if manifest.sync_number != sync_height || manifest.users.len() != txs.len() {
            return Err("attempt manifest does not cover its users and Sync height".into());
        }
        for (tx, user) in txs.iter().zip(&manifest.users) {
            if tx.hash != user.tx_hash
                || tx.sender != user.sender
                || tx.nonce != user.nonce
                || tx.attempts != user.attempts
                || (tx.direction == Direction::Inbound) != (user.source_rollup_id.0 == 0)
            {
                return Err("attempt signed-user identity differs from manifest".into());
            }
        }
        self.journal.update(|state| {
            if state
                .attempts
                .values()
                .any(|attempt| needs_resolution(state, attempt))
            {
                return Err("another submitted attempt still owns unresolved users".into());
            }
            let generation = state.next_generation;
            state.next_generation = generation
                .checked_add(1)
                .ok_or("attempt generation exhausted")?;
            let id = AttemptId {
                generation,
                post_batch_hash,
            };
            for (fifo_position, tx) in txs.iter().cloned().enumerate() {
                state.users.insert(
                    tx.hash,
                    OwnedUser {
                        tx,
                        owner: generation,
                        fifo_position,
                        location: UserLocation::AwaitingEvidence,
                    },
                );
            }
            state.attempts.insert(
                generation,
                AttemptRecord {
                    id,
                    sync_height,
                    parent: parent.unseal(),
                    txs,
                    manifest,
                    post_batch,
                    requested_target: target,
                    dispatched_target: None,
                    observation: SubmissionObservation::Prepared,
                    reconciled_at: None,
                },
            );
            Ok(id)
        })
    }

    /// Lowest unresolved attempt, independently of the scheduling cursor.
    #[must_use]
    pub fn blocking_height(&self) -> Option<u64> {
        self.journal.read(|state| {
            state
                .attempts
                .values()
                .filter(|attempt| needs_resolution(state, attempt))
                .map(|attempt| attempt.sync_height)
                .min()
        })
    }

    #[must_use]
    pub fn pending_attempt(&self) -> Option<PendingAttempt> {
        self.journal.read(|state| {
            state
                .attempts
                .values()
                .find(|attempt| needs_resolution(state, attempt))
                .map(|attempt| PendingAttempt {
                    id: attempt.id,
                    sync_height: attempt.sync_height,
                    parent: SealedHeader::seal_slow(attempt.parent.clone()),
                    txs: attempt.txs.clone(),
                    manifest: attempt.manifest.clone(),
                    post_batch: attempt.post_batch.clone(),
                    requested_target: attempt.requested_target,
                    dispatched_target: attempt.dispatched_target,
                    observation: attempt.observation.clone(),
                })
        })
    }

    /// A late callback can update only the exact attempt that created it.
    pub fn observe(
        &self,
        id: AttemptId,
        observation: SubmissionObservation,
    ) -> Result<bool, String> {
        self.journal.update(|state| {
            let Some(attempt) = state
                .attempts
                .get_mut(&id.generation)
                .filter(|attempt| attempt.id == id && attempt.reconciled_at.is_none())
            else {
                return Ok(false);
            };
            if let SubmissionObservation::Dispatched { block, timestamp } = observation {
                if attempt.dispatched_target.is_some() {
                    return Err(
                        "attempt already dispatched; expired envelopes must never be resent".into(),
                    );
                }
                attempt.dispatched_target = Some((block, timestamp));
            }
            attempt.observation = observation;
            Ok(true)
        })
    }

    /// Persist the user partition once, retaining the original record for
    /// reorg/finality but transferring only eligible users back to the queue.
    pub fn resolve(
        &self,
        id: AttemptId,
        as_of: L1BlockRef,
        outcomes: &[SourceOutcome],
        charge_attempt: bool,
        attempt_limit: u32,
    ) -> Result<Vec<NonceGap>, String> {
        self.journal.update(|state| {
            let Some(attempt) = state
                .attempts
                .get(&id.generation)
                .filter(|attempt| attempt.id == id && attempt.reconciled_at.is_none())
            else {
                return Ok(Vec::new());
            };
            if outcomes.len() != attempt.txs.len() {
                return Err("source outcomes do not cover the complete attempt".into());
            }
            let txs = attempt.txs.clone();
            let mut gaps = Vec::new();
            for (tx, outcome) in txs.iter().zip(outcomes) {
                let Some(user) = state.users.get_mut(&tx.hash).filter(|user| {
                    user.owner == id.generation && user.location == UserLocation::AwaitingEvidence
                }) else {
                    continue;
                };
                user.location = match outcome {
                    SourceOutcome::Terminal(reason) => UserLocation::Terminal {
                        reason: *reason,
                        canonical: Some(as_of),
                    },
                    SourceOutcome::Retryable { source_nonce } => {
                        if *source_nonce > tx.nonce {
                            return Err("retry requested for a spent source nonce".into());
                        }
                        if charge_attempt {
                            user.tx.attempts = user.tx.attempts.saturating_add(1);
                        }
                        if user.tx.attempts >= attempt_limit {
                            gaps.push(NonceGap {
                                sender: tx.sender,
                                direction: tx.direction,
                                nonce: tx.nonce,
                            });
                            UserLocation::Terminal {
                                reason: TerminalReason::AttemptsExhausted,
                                canonical: None,
                            }
                        } else {
                            UserLocation::Queued
                        }
                    }
                };
            }
            // A source-included revert/completion is terminal but creates no
            // nonce gap. Only a still-reusable nonce exhausted above cascades.
            for user in state.users.values_mut() {
                if matches!(
                    user.location,
                    UserLocation::Queued | UserLocation::Composing
                ) && gaps.iter().any(|gap| {
                    user.tx.sender == gap.sender
                        && user.tx.direction == gap.direction
                        && user.tx.nonce > gap.nonce
                }) {
                    user.location = UserLocation::Terminal {
                        reason: TerminalReason::NonceGap,
                        canonical: None,
                    };
                }
            }
            state
                .attempts
                .get_mut(&id.generation)
                .expect("attempt exists")
                .reconciled_at = Some(as_of);
            Ok(gaps)
        })
    }

    /// Reorgs revoke canonical conclusions, including same-L2-height rewrites.
    /// History whose users are now owned by a retry cannot reclaim them.
    pub fn invalidate_after_l1(&self, ancestor: u64) -> Result<(), String> {
        self.journal.update(|state| {
            for user in state.users.values_mut() {
                if matches!(user.location, UserLocation::Terminal {
                    canonical: Some(block), ..
                } if block.number > ancestor)
                {
                    user.location = UserLocation::AwaitingEvidence;
                    if let Some(attempt) = state.attempts.get_mut(&user.owner) {
                        attempt.reconciled_at = None;
                        attempt.observation = SubmissionObservation::Uncertain;
                    }
                }
            }
            for attempt in state.attempts.values_mut() {
                if attempt.txs.is_empty()
                    && attempt
                        .reconciled_at
                        .is_some_and(|block| block.number > ancestor)
                {
                    attempt.reconciled_at = None;
                    attempt.observation = SubmissionObservation::Uncertain;
                }
            }
            Ok(())
        })
    }

    #[must_use]
    pub fn finalization_candidates(&self, finalized_l1: u64) -> Vec<(AttemptId, L1BlockRef)> {
        self.journal.read(|state| {
            state
                .attempts
                .values()
                .filter_map(|attempt| {
                    let block = attempt
                        .reconciled_at
                        .filter(|block| block.number <= finalized_l1)?;
                    (!state.users.values().any(|user| {
                        user.owner == attempt.id.generation
                            && !matches!(user.location, UserLocation::Terminal { .. })
                    }))
                    .then_some((attempt.id, block))
                })
                .collect()
        })
    }

    /// Called only after a conclusive canonical finality audit. Errors or a
    /// reorg retain the record. No held-pool reservation is released here.
    pub fn forget_finalized(&self, id: AttemptId, audited: L1BlockRef) -> Result<(), String> {
        self.journal.update(|state| {
            let eligible =
                state.attempts.get(&id.generation).is_some_and(|attempt| {
                    attempt.id == id && attempt.reconciled_at == Some(audited)
                }) && !state.users.values().any(|user| {
                    user.owner == id.generation
                        && !matches!(user.location, UserLocation::Terminal { .. })
                });
            if eligible {
                state.attempts.remove(&id.generation);
                state.users.retain(|_, user| user.owner != id.generation);
            }
            Ok(())
        })
    }

    /// Inclusive recovery scan floor. Prepared NextBlock attempts never
    /// reached the network; dispatched ones always have an exact target.
    #[must_use]
    pub fn recovery_scan_from(&self) -> Option<u64> {
        self.journal.read(|state| {
            state
                .attempts
                .values()
                .filter_map(|attempt| {
                    attempt.dispatched_target.map(|target| target.0).or(
                        match attempt.requested_target {
                            BundleTarget::Exact { block, .. } => Some(block),
                            BundleTarget::NextBlock => None,
                        },
                    )
                })
                .min()
        })
    }
}

fn needs_resolution(state: &JournalState, attempt: &AttemptRecord) -> bool {
    attempt.reconciled_at.is_none()
        && (attempt.txs.is_empty()
            || state.users.values().any(|user| {
                user.owner == attempt.id.generation
                    && user.location == UserLocation::AwaitingEvidence
            }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeldPool;
    use alloy_consensus::transaction::SignerRecoverable;
    use alloy_consensus::{SignableTransaction, Transaction, TxEip1559, TxEnvelope};
    use alloy_eips::Encodable2718;
    use alloy_network::TxSignerSync;
    use alloy_primitives::{TxKind, U256};
    use alloy_signer_local::PrivateKeySigner;
    use eez_protocol::blob_semantics::SemanticTransaction;
    use eez_protocol::settlement_manifest::{ManifestInput, ProposedUser};
    use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId};
    use std::sync::Barrier;

    fn tx(tag: u8, nonce: u64) -> HeldTx {
        let signer: PrivateKeySigner = B256::with_last_byte(tag).to_string().parse().unwrap();
        let mut unsigned = TxEip1559 {
            chain_id: 31337,
            nonce,
            gas_limit: 1_000_000,
            max_fee_per_gas: 100,
            max_priority_fee_per_gas: 10,
            to: TxKind::Call(Address::repeat_byte(4)),
            ..Default::default()
        };
        let signature = signer.sign_transaction_sync(&mut unsigned).unwrap();
        let signed = TxEnvelope::from(unsigned.into_signed(signature));
        HeldTx {
            raw_tx: Bytes::from(signed.encoded_2718()),
            hash: *signed.tx_hash(),
            attempts: 0,
            sender: signed.recover_signer().unwrap(),
            nonce: signed.nonce(),
            direction: Direction::Inbound,
            max_fee_per_gas: 100,
            priority_fee_per_gas: 10,
        }
    }

    fn manifest(txs: &[HeldTx]) -> SettlementManifest {
        let semantics = txs
            .iter()
            .map(|tx| SemanticTransaction {
                origin_chain: RollupId(0),
                tx_data: tx.raw_tx.clone(),
                actions: vec![ExecutedAction {
                    parent_index: None,
                    call_mode: CallMode::Mutable,
                    target_address: Address::repeat_byte(5),
                    target_rollup_id: RollupId(1),
                    source_rollup_id: RollupId(0),
                    source_address: tx.sender,
                    data: Bytes::new(),
                    value: U256::ZERO,
                    outcome: ExecutionOutcome::Resolved {
                        return_data: vec![],
                        gas_used: 1,
                        success: true,
                    },
                    revert_span: None,
                }],
            })
            .collect::<Vec<_>>();
        let positions = (0..txs.len()).collect::<Vec<_>>();
        let hashes = positions
            .iter()
            .map(|i| B256::with_last_byte(u8::try_from(i + 1).unwrap()))
            .collect::<Vec<_>>();
        SettlementManifest::build(ManifestInput {
            rollup_id: 1,
            sync_number: 10,
            sync_hash: hashes.last().copied().unwrap_or_default(),
            users: txs
                .iter()
                .map(|tx| ProposedUser {
                    raw: &tx.raw_tx,
                    source: RollupId(0),
                    attempts: tx.attempts,
                })
                .collect(),
            semantics: &semantics,
            outbound_groups: &[],
            outbound_hosts: &[],
            checkpoint_positions: &positions,
            checkpoint_hashes: &hashes,
        })
        .unwrap()
    }

    fn begin(ledger: &OptimisticallyIncluded, txs: Vec<HeldTx>, tag: u8) -> AttemptId {
        let manifest = manifest(&txs);
        let parent = SealedHeader::seal_slow(alloy_consensus::Header {
            number: 9,
            ..Default::default()
        });
        ledger
            .begin(
                10,
                B256::repeat_byte(tag),
                parent,
                txs,
                manifest,
                Bytes::new(),
                BundleTarget::Exact {
                    block: 100,
                    timestamp: 1200,
                },
            )
            .unwrap()
    }

    fn setup() -> (
        Arc<OptimisticallyIncluded>,
        Arc<HeldPool>,
        Vec<HeldTx>,
        AttemptId,
    ) {
        let ledger = Arc::new(OptimisticallyIncluded::new());
        let pool = Arc::new(HeldPool::new());
        pool.attach_journal(ledger.journal());
        let txs = (1..=3).map(|tag| tx(tag, 0)).collect::<Vec<_>>();
        for tx in &txs {
            pool.push_contiguous(tx.clone(), 0).unwrap();
        }
        let id = begin(&ledger, pool.pop_all(), 10);
        (ledger, pool, txs, id)
    }

    fn block(number: u64) -> L1BlockRef {
        L1BlockRef {
            number,
            hash: B256::with_last_byte(u8::try_from(number).unwrap()),
        }
    }

    fn partial() -> [SourceOutcome; 3] {
        [
            SourceOutcome::Terminal(TerminalReason::Committed),
            SourceOutcome::Terminal(TerminalReason::Committed),
            SourceOutcome::Retryable { source_nonce: 0 },
        ]
    }

    #[test]
    fn partial_consumption_preserves_tail_in_both_observer_orders() {
        for observer_first in [true, false] {
            let (ledger, pool, txs, id) = setup();
            let barrier = Arc::new(Barrier::new(2));
            let observer = {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    if !observer_first {
                        barrier.wait();
                    }
                    ledger
                        .observe(
                            id,
                            SubmissionObservation::Excluded {
                                block: 100,
                                charge_attempt: true,
                            },
                        )
                        .unwrap();
                    if observer_first {
                        barrier.wait();
                    }
                })
            };
            if observer_first {
                barrier.wait();
            }
            // Canonical classification of a partial prefix; a competing
            // confirmed height alone has never opened this gate.
            assert_eq!(ledger.blocking_height(), Some(10));
            pool.resolve_attempt(&ledger, id, block(100), &partial(), true, 3, || true)
                .unwrap();
            if !observer_first {
                barrier.wait();
            }
            observer.join().unwrap();
            assert_eq!(ledger.blocking_height(), None);
            let recovered = pool.pop_all();
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].hash, txs[2].hash);
            assert_eq!(recovered[0].attempts, 1);
        }
    }

    #[test]
    fn recovery_entered_before_derivation_retains_all_ownership() {
        let (ledger, pool, txs, id) = setup();
        ledger
            .observe(id, SubmissionObservation::Uncertain)
            .unwrap();
        let pending = ledger.pending_attempt().unwrap();
        assert_eq!(pending.id, id);
        assert_eq!(pending.txs.len(), 3);
        assert!(pool.is_empty());
        assert_eq!(ledger.blocking_height(), Some(10));
        pool.resolve_attempt(&ledger, id, block(100), &partial(), true, 3, || true)
            .unwrap();
        assert_eq!(pool.pop_all()[0].hash, txs[2].hash);
    }

    #[test]
    fn duplicate_resolution_does_not_release_a_new_attempt_or_charge_again() {
        let (ledger, pool, _, old) = setup();
        pool.resolve_attempt(&ledger, old, block(100), &partial(), true, 3, || true)
            .unwrap();
        let retry = pool.pop_all();
        let new = begin(&ledger, retry, 11);
        pool.resolve_attempt(&ledger, old, block(100), &partial(), true, 3, || true)
            .unwrap();
        assert!(
            !ledger
                .observe(old, SubmissionObservation::Included { block: block(100) })
                .unwrap()
        );
        assert_eq!(ledger.pending_attempt().unwrap().id, new);
        assert!(pool.is_empty());
        assert_eq!(ledger.pending_attempt().unwrap().txs[0].attempts, 1);
        assert_eq!(ledger.blocking_height(), Some(10));
    }

    #[test]
    fn included_revert_and_caught_failure_spend_nonce_without_retry() {
        let (ledger, pool, _, id) = setup();
        pool.resolve_attempt(
            &ledger,
            id,
            block(100),
            &[
                SourceOutcome::Terminal(TerminalReason::SourceReverted),
                SourceOutcome::Terminal(TerminalReason::SourceCompletedWithMissingEffects),
                SourceOutcome::Terminal(TerminalReason::SourceCompleted),
            ],
            true,
            3,
            || true,
        )
        .unwrap();
        assert!(pool.is_empty());
        assert!(ledger.blocking_height().is_none());
        assert!(
            ledger
                .journal
                .read(|state| state.users.values().all(|u| u.tx.attempts == 0))
        );
    }

    #[test]
    fn malformed_partition_and_spent_nonce_retry_are_rejected_without_mutation() {
        let (ledger, pool, _, id) = setup();
        assert!(
            pool.resolve_attempt(&ledger, id, block(100), &partial()[..2], true, 3, || true)
                .is_err()
        );
        assert!(
            pool.resolve_attempt(
                &ledger,
                id,
                block(100),
                &[SourceOutcome::Retryable { source_nonce: 1 }; 3],
                true,
                3,
                || true
            )
            .is_err()
        );
        assert_eq!(ledger.blocking_height(), Some(10));
        assert!(pool.is_empty());
        assert!(
            ledger
                .journal
                .read(|state| state.users.values().all(|u| u.tx.attempts == 0))
        );
    }

    #[test]
    fn reorg_restores_reservations_and_rejects_a_stale_ownership_decision() {
        let (ledger, pool, txs, id) = setup();
        assert!(
            pool.resolve_attempt(&ledger, id, block(100), &partial(), true, 3, || false)
                .is_err()
        );
        assert_eq!(ledger.pending_attempt().unwrap().id, id);
        assert!(pool.is_empty());
        assert!(
            ledger
                .journal
                .read(|state| state.users.values().all(|u| u.tx.attempts == 0))
        );
        pool.resolve_attempt(
            &ledger,
            id,
            block(100),
            &[SourceOutcome::Terminal(TerminalReason::Committed); 3],
            false,
            3,
            || true,
        )
        .unwrap();
        assert!(ledger.pending_attempt().is_none());
        pool.invalidate_after_l1(&ledger, 99).unwrap();
        // Ingress cannot reuse a reopened user's source nonce while the
        // replacement canonical scan is still incomplete.
        let mut replacement = txs[0].clone();
        replacement.hash = B256::repeat_byte(0xff);
        assert!(pool.push_contiguous(replacement, 0).is_err());
        pool.push_contiguous(txs[0].clone(), 0).unwrap(); // exact duplicate is idempotent
        assert!(pool.is_empty());
        assert_eq!(ledger.pending_attempt().unwrap().id, id);
        pool.resolve_attempt(&ledger, id, block(101), &partial(), false, 3, || true)
            .unwrap();
        assert_eq!(
            pool.pop_all().iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![txs[2].hash]
        );
    }

    #[test]
    fn reorg_at_same_l2_height_reopens_only_canonical_owned_users() {
        let (ledger, pool, _, old) = setup();
        pool.resolve_attempt(&ledger, old, block(100), &partial(), true, 3, || true)
            .unwrap();
        let new = begin(&ledger, pool.pop_all(), 11);
        pool.invalidate_after_l1(&ledger, 99).unwrap();
        assert_eq!(ledger.pending_attempt().unwrap().id, old);
        assert!(ledger.journal.read(|state| {
            state
                .users
                .values()
                .filter(|u| u.owner == new.generation)
                .count()
                == 1
        }));
        // The old attempt cannot claim the tail now owned by its retry.
        pool.resolve_attempt(
            &ledger,
            old,
            block(101),
            &[SourceOutcome::Retryable { source_nonce: 0 }; 3],
            false,
            3,
            || true,
        )
        .unwrap();
        assert_eq!(pool.len(), 2);
        assert_eq!(ledger.pending_attempt().unwrap().id, new);
    }

    #[test]
    fn finality_prunes_reorged_history_only_after_ownership_has_transferred() {
        let (ledger, pool, _, old) = setup();
        pool.resolve_attempt(
            &ledger,
            old,
            block(100),
            &[SourceOutcome::Retryable { source_nonce: 0 }; 3],
            true,
            3,
            || true,
        )
        .unwrap();
        let new = begin(&ledger, pool.pop_all(), 11);
        pool.invalidate_after_l1(&ledger, 99).unwrap();
        ledger.forget_finalized(old, block(100)).unwrap();
        assert_eq!(ledger.pending_attempt().unwrap().id, new);
        assert!(ledger.journal.read(|state| {
            state.attempts.len() == 1
                && state.users.len() == 3
                && state.users.values().all(|user| {
                    user.owner == new.generation
                        && user.location == UserLocation::AwaitingEvidence
                        && user.tx.attempts == 1
                })
        }));

        // A reorged canonical completion still owned by this attempt must
        // instead await a fresh source audit, even at the same L2 height.
        pool.resolve_attempt(&ledger, new, block(101), &partial(), false, 3, || true)
            .unwrap();
        pool.invalidate_after_l1(&ledger, 100).unwrap();
        ledger.forget_finalized(new, block(101)).unwrap();
        assert_eq!(ledger.pending_attempt().unwrap().id, new);
    }

    #[test]
    fn finality_only_forgets_audited_history_without_touching_retry() {
        let (ledger, pool, _, old) = setup();
        pool.resolve_attempt(&ledger, old, block(100), &partial(), true, 3, || true)
            .unwrap();
        assert!(ledger.finalization_candidates(100).is_empty()); // queued retry remains durable
        let new = begin(&ledger, pool.pop_all(), 11);
        assert_eq!(ledger.finalization_candidates(100), [(old, block(100))]);
        ledger.forget_finalized(old, block(99)).unwrap(); // stale/incorrect audit
        assert_eq!(ledger.finalization_candidates(100).len(), 1);
        ledger.forget_finalized(old, block(100)).unwrap();
        assert_eq!(ledger.pending_attempt().unwrap().id, new);
        assert!(pool.is_empty());
    }

    #[test]
    fn retries_preserve_fifo_and_infrastructure_failures_do_not_charge() {
        let (ledger, pool, txs, id) = setup();
        pool.resolve_attempt(
            &ledger,
            id,
            block(100),
            &[SourceOutcome::Retryable { source_nonce: 0 }; 3],
            false,
            3,
            || true,
        )
        .unwrap();
        let recovered = pool.pop_all();
        assert_eq!(
            recovered.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            txs.iter().map(|tx| tx.hash).collect::<Vec<_>>()
        );
        assert!(recovered.iter().all(|tx| tx.attempts == 0));
    }

    struct JournalDirectory(std::path::PathBuf);

    impl JournalDirectory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/settlement-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> std::path::PathBuf {
            self.0.join("journal.json")
        }
    }

    impl Drop for JournalDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn durable_recovery_survives_dispatch_transfer_composition_and_new_attempt() {
        let directory = JournalDirectory::new();
        let original = vec![tx(1, 0), tx(2, 0), tx(3, 0)];
        let first;
        {
            let ledger = OptimisticallyIncluded::open(&directory.file()).unwrap();
            let pool = HeldPool::new();
            pool.attach_journal(ledger.journal());
            for tx in &original {
                pool.push_contiguous(tx.clone(), 0).unwrap();
            }
            first = begin(&ledger, pool.pop_all(), 10);
            ledger
                .observe(
                    first,
                    SubmissionObservation::Dispatched {
                        block: 100,
                        timestamp: 1200,
                    },
                )
                .unwrap();
            // Crash after journal-before-dispatch; whether the HTTP request
            // was accepted must be established from canonical evidence.
        }
        {
            let ledger = OptimisticallyIncluded::open(&directory.file()).unwrap();
            let pool = HeldPool::new();
            pool.attach_journal(ledger.journal());
            assert_eq!(ledger.pending_attempt().unwrap().id, first);
            assert_eq!(
                ledger.pending_attempt().unwrap().dispatched_target,
                Some((100, 1200))
            );
            assert_eq!(
                ledger.pending_attempt().unwrap().observation,
                SubmissionObservation::Uncertain
            );
            assert!(
                ledger
                    .observe(
                        first,
                        SubmissionObservation::Dispatched {
                            block: 101,
                            timestamp: 1212
                        }
                    )
                    .is_err()
            );
            assert!(pool.is_empty());
            pool.resolve_attempt(&ledger, first, block(100), &partial(), true, 3, || true)
                .unwrap();
            // Crash after the durable ownership transfer.
        }
        {
            let ledger = OptimisticallyIncluded::open(&directory.file()).unwrap();
            let pool = HeldPool::new();
            pool.attach_journal(ledger.journal());
            let retry = pool.pop_all();
            assert_eq!(retry.len(), 1);
            assert_eq!(retry[0].raw_tx, original[2].raw_tx);
            assert_eq!(retry[0].attempts, 1);
            // Crash during recomposition, before a new attempt exists.
        }
        let second;
        {
            let ledger = OptimisticallyIncluded::open(&directory.file()).unwrap();
            let pool = HeldPool::new();
            pool.attach_journal(ledger.journal());
            let retry = pool.pop_all();
            assert_eq!(retry.len(), 1);
            assert_eq!(retry[0].attempts, 1);
            second = begin(&ledger, retry, 11);
        }
        {
            let ledger = OptimisticallyIncluded::open(&directory.file()).unwrap();
            let pool = HeldPool::new();
            pool.attach_journal(ledger.journal());
            assert_eq!(ledger.pending_attempt().unwrap().id, second);
            assert_eq!(
                ledger.pending_attempt().unwrap().observation,
                SubmissionObservation::Uncertain
            );
            assert_eq!(ledger.pending_attempt().unwrap().txs[0].attempts, 1);
            ledger.forget_finalized(first, block(100)).unwrap();
            assert!(pool.is_empty());
            assert_eq!(ledger.pending_attempt().unwrap().id, second);
        }
    }

    #[test]
    fn journal_excludes_concurrent_writers_and_rejects_corruption() {
        let directory = JournalDirectory::new();
        let ledger = OptimisticallyIncluded::open(&directory.file()).unwrap();
        assert!(OptimisticallyIncluded::open(&directory.file()).is_err());
        begin(&ledger, vec![tx(1, 0)], 10);
        drop(ledger);
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.file()).unwrap()).unwrap();
        file["state"]["next_generation"] = serde_json::json!(99);
        std::fs::write(directory.file(), serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(
            OptimisticallyIncluded::open(&directory.file())
                .unwrap_err()
                .contains("checksum")
        );
    }

    #[test]
    fn journal_write_error_prevents_dispatch_and_all_future_mutations() {
        let directory = JournalDirectory::new();
        let ledger = OptimisticallyIncluded::open(&directory.file()).unwrap();
        let id = begin(&ledger, vec![tx(1, 0)], 10);
        let staging = directory.file().with_extension("new");
        std::fs::create_dir(&staging).unwrap();
        assert!(
            ledger
                .observe(
                    id,
                    SubmissionObservation::Dispatched {
                        block: 100,
                        timestamp: 1200
                    }
                )
                .is_err()
        );
        assert_eq!(ledger.pending_attempt().unwrap().dispatched_target, None);
        std::fs::remove_dir(&staging).unwrap();
        assert!(
            ledger
                .observe(id, SubmissionObservation::Uncertain)
                .is_err()
        );
        drop(ledger);
        let reopened = OptimisticallyIncluded::open(&directory.file()).unwrap();
        assert_eq!(reopened.pending_attempt().unwrap().dispatched_target, None);
    }

    #[test]
    fn stale_recomposition_preserves_canonical_reorg_recovery() {
        let (ledger, pool, _, id) = setup();
        pool.resolve_attempt(&ledger, id, block(100), &partial(), true, 3, || true)
            .unwrap();
        let drained = pool.pop_all();
        assert!(pool.recheck_stale_sources(&drained).unwrap());
        assert_eq!(ledger.blocking_height(), Some(10));
        assert_eq!(ledger.pending_attempt().unwrap().id, id);
        assert!(pool.is_empty());
        let mut outcomes = partial();
        outcomes[2] = SourceOutcome::Terminal(TerminalReason::SourceNonceSpent);
        pool.resolve_attempt(&ledger, id, block(101), &outcomes, false, 3, || true)
            .unwrap();
        ledger.invalidate_after_l1(100).unwrap();
        assert_eq!(ledger.pending_attempt().unwrap().id, id);
    }

    #[test]
    fn exhausted_unspent_nonce_evicts_only_its_direction_and_sender_suffix_once() {
        let ledger = OptimisticallyIncluded::new();
        let pool = HeldPool::new();
        pool.attach_journal(ledger.journal());
        let mut exhausted = tx(1, 0);
        exhausted.attempts = u32::MAX;
        let suffix = tx(1, 1);
        let independent = tx(2, 0);
        let users = vec![exhausted.clone(), suffix.clone(), independent.clone()];
        for user in &users {
            pool.push_contiguous(user.clone(), 0).unwrap();
        }
        let id = begin(&ledger, pool.pop_all(), 10);
        let outcomes = vec![SourceOutcome::Retryable { source_nonce: 0 }; 3];
        pool.resolve_attempt(&ledger, id, block(100), &outcomes, true, 3, || true)
            .unwrap();
        // A duplicate callback cannot increment the independent retry again.
        pool.resolve_attempt(&ledger, id, block(100), &outcomes, true, 3, || true)
            .unwrap();
        let queued = pool.pop_all();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].hash, independent.hash);
        assert_eq!(queued[0].attempts, 1);
        ledger.journal.read(|state| {
            assert_eq!(state.users[&exhausted.hash].tx.attempts, u32::MAX);
            assert!(matches!(
                state.users[&exhausted.hash].location,
                UserLocation::Terminal {
                    reason: TerminalReason::AttemptsExhausted,
                    canonical: None
                }
            ));
            assert!(matches!(
                state.users[&suffix.hash].location,
                UserLocation::Terminal {
                    reason: TerminalReason::NonceGap,
                    canonical: None
                }
            ));
        });
    }
}
