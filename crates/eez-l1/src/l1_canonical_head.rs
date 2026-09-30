//! Shared L1-confirmed L2 cursor and canonical transaction evidence.
//!
//! The cursor is a scheduling hint. Only a snapshot whose scan has completed
//! can authorize user ownership changes. Publication and reorg invalidation use
//! one mutex; the deriver also holds the shared engine reconciliation lock.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use alloy_primitives::B256;

use crate::{L1BlockRef, L2Endpoint, ReconciledBatchSettlement};

/// One winning batch in the L1/L2 height index.
#[derive(Debug, Clone, Copy)]
pub struct BatchRecord {
    pub l1_block: u64,
    pub l1_block_hash: B256,
    pub tx_hash: B256,
    pub last_l2_block: u64,
}

/// Consistent evidence for recovery. An incomplete scan has no `through`.
#[derive(Debug, Clone, Default)]
pub struct SettlementSnapshot {
    pub generation: u64,
    pub through: Option<L1BlockRef>,
    pub endpoint: Option<L2Endpoint>,
    pub settlements: HashMap<B256, ReconciledBatchSettlement>,
}

/// Exact canonical occurrence that last changed the L2 commitment. The
/// containing block supplies its timestamp; a receipt alone cannot create it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitmentRef {
    pub l1: L1BlockRef,
    pub tx_hash: B256,
    pub tx_index: u64,
    pub endpoint: L2Endpoint,
}

/// Small consistent view for settlement scheduling, without cloning all user
/// evidence on every idle tick. `through == None` blocks policy evaluation.
#[derive(Debug, Clone, Copy)]
pub struct PolicyAnchorSnapshot {
    pub generation: u64,
    pub through: Option<L1BlockRef>,
    pub endpoint: Option<L2Endpoint>,
    pub commitment: Option<CommitmentRef>,
}

#[derive(Debug, Default)]
struct CanonicalState {
    batches: Vec<BatchRecord>,
    evidence: SettlementSnapshot,
    pending_l1_blocks: HashSet<L1BlockRef>,
    last_scanned: Option<L1BlockRef>,
    commitments: BTreeMap<(u64, u64), CommitmentRef>,
    latest_commitment: Option<CommitmentRef>,
    derivation: Option<crate::derivation::SeedHistory>,
}

impl CanonicalState {
    fn refresh_commitment(&mut self) {
        let mut entries = self.commitments.values().rev();
        let Some(mut latest) = entries.next().copied() else {
            self.latest_commitment = None;
            return;
        };
        // Repeated source completions that leave the commitment unchanged do
        // not restart the timer. Same-height *different* hashes do.
        for previous in entries {
            if previous.endpoint != latest.endpoint {
                break;
            }
            latest = *previous;
        }
        self.latest_commitment = Some(latest);
    }
}

/// Deriver-owned canonical state shared with the composer.
#[derive(Debug, Default)]
pub struct L1CanonicalHead {
    cursor: AtomicU64,
    finalized_l2: AtomicU64,
    state: Mutex<CanonicalState>,
}

impl L1CanonicalHead {
    pub fn cursor(&self) -> u64 {
        self.cursor.load(Ordering::Acquire)
    }

    pub fn finalized_l2(&self) -> u64 {
        self.finalized_l2.load(Ordering::Acquire)
    }

    pub fn last_indexed_l2(&self) -> u64 {
        self.state
            .lock()
            .unwrap()
            .batches
            .last()
            .map_or(0, |b| b.last_l2_block)
    }

    pub fn last_indexed(&self) -> Option<BatchRecord> {
        self.state.lock().unwrap().batches.last().copied()
    }

    pub fn count_at_l1_block(&self, l1_block: u64) -> usize {
        self.state
            .lock()
            .unwrap()
            .batches
            .iter()
            .filter(|b| b.l1_block == l1_block)
            .count()
    }

    pub fn contains_l1_tx(&self, tx_hash: &B256) -> bool {
        self.state
            .lock()
            .unwrap()
            .batches
            .iter()
            .any(|b| &b.tx_hash == tx_hash)
    }

    pub fn known_tx_hashes(&self) -> HashSet<B256> {
        self.state
            .lock()
            .unwrap()
            .batches
            .iter()
            .map(|b| b.tx_hash)
            .collect()
    }

    /// Seed the scheduling index from validated persistent correlations.
    /// This deliberately does not manufacture user settlement evidence.
    pub fn append(&self, record: BatchRecord) {
        self.append_many([record]);
    }

    pub fn append_many(&self, records: impl IntoIterator<Item = BatchRecord>) {
        let mut state = self.state.lock().unwrap();
        let mut known = state
            .batches
            .iter()
            .map(|b| b.tx_hash)
            .collect::<HashSet<_>>();
        let mut max_l2 = self.cursor();
        for record in records {
            if known.insert(record.tx_hash) {
                max_l2 = max_l2.max(record.last_l2_block);
                state.batches.push(record);
            }
        }
        self.cursor.store(max_l2, Ordering::Release);
    }

    /// Publish checked batch evidence before exposing its L2 cursor. The
    /// caller marks the scan complete only after every same-block batch and
    /// the safe-head update have succeeded.
    pub fn publish(&self, evidence: ReconciledBatchSettlement) {
        let mut state = self.state.lock().unwrap();
        self.publish_locked(&mut state, evidence);
    }

    /// Install/rebuild seed provenance before boot exposes readiness. The
    /// deriver holds the engine reconciliation lock while replacing this state.
    pub fn restore_derivation(&self, history: crate::derivation::SeedHistory) {
        self.state.lock().unwrap().derivation = Some(history);
    }

    /// Capture the seed for the exact block being built. The builder must retain
    /// it with the rest of its immutable candidate environment. Startup without
    /// authenticated provenance is an error, never a zero-seed fallback.
    pub fn derivation_seed(&self, number: u64) -> Result<B256, crate::derivation::DerivationError> {
        let state = self.state.lock().unwrap();
        let history =
            state
                .derivation
                .as_ref()
                .ok_or(crate::derivation::DerivationError::Unavailable(
                    "seed provenance not initialized",
                ))?;
        Ok(history.seed_for(number))
    }

    /// The immutable schedule initialized by authenticated derivation startup.
    pub fn derivation_schedule(
        &self,
    ) -> Result<eez_protocol::l2::Schedule, crate::derivation::DerivationError> {
        self.state
            .lock()
            .unwrap()
            .derivation
            .as_ref()
            .map(crate::derivation::SeedHistory::schedule)
            .ok_or(crate::derivation::DerivationError::Unavailable(
                "seed provenance not initialized",
            ))
    }

    /// Derive the next block's time and seed in one consistent read. Parent
    /// authentication and the candidate's reconciliation lock belong to caller.
    pub fn next_block_context(
        &self,
        parent: &alloy_consensus::Header,
    ) -> Result<(u64, B256), crate::derivation::DerivationError> {
        self.state
            .lock()
            .unwrap()
            .derivation
            .as_ref()
            .ok_or(crate::derivation::DerivationError::Unavailable(
                "seed provenance not initialized",
            ))?
            .next_block_context(parent)
    }

    /// Check seed continuity before persisting a correlation. The sole writer
    /// retains the reconciliation lock through persistence and publication.
    pub fn validate_accepted(
        &self,
        evidence: &ReconciledBatchSettlement,
        l1: &crate::derivation::AuthenticatedL1Header,
        endpoint: &alloy_consensus::Header,
    ) -> Result<(), crate::derivation::DerivationError> {
        self.state
            .lock()
            .unwrap()
            .derivation
            .as_ref()
            .ok_or(crate::derivation::DerivationError::Unavailable(
                "seed provenance not initialized",
            ))?
            .validate_record(evidence, l1, endpoint)
            .map(|_| ())
    }

    /// Atomically expose an accepted anchor and its seed under the same lock as
    /// the cursor. No-effect evidence uses `publish` and cannot refresh the seed.
    pub fn publish_accepted(
        &self,
        evidence: ReconciledBatchSettlement,
        l1: &crate::derivation::AuthenticatedL1Header,
        endpoint: &alloy_consensus::Header,
    ) -> Result<(), crate::derivation::DerivationError> {
        let mut state = self.state.lock().unwrap();
        if state.evidence.settlements.get(&evidence.tx_hash) == Some(&evidence) {
            return Ok(());
        }
        state
            .derivation
            .as_mut()
            .ok_or(crate::derivation::DerivationError::Unavailable(
                "seed provenance not initialized",
            ))?
            .record(&evidence, l1, endpoint)?;
        self.publish_locked(&mut state, evidence);
        Ok(())
    }

    fn publish_locked(&self, state: &mut CanonicalState, evidence: ReconciledBatchSettlement) {
        if state.evidence.settlements.get(&evidence.tx_hash) == Some(&evidence) {
            return;
        }
        state.evidence.generation = state
            .evidence
            .generation
            .checked_add(1)
            .expect("canonical evidence generation exhausted");
        state.evidence.through = None;
        state.pending_l1_blocks.insert(evidence.l1_block);
        if let Some(previous) = state.evidence.settlements.get(&evidence.tx_hash) {
            let previous_position = (previous.l1_block.number, previous.tx_index);
            state.commitments.remove(&previous_position);
        }
        if let Some(endpoint) = evidence.endpoint {
            state.commitments.insert(
                (evidence.l1_block.number, evidence.tx_index),
                CommitmentRef {
                    l1: evidence.l1_block,
                    tx_hash: evidence.tx_hash,
                    tx_index: evidence.tx_index,
                    endpoint,
                },
            );
            state.refresh_commitment();
            if !state.batches.iter().any(|b| b.tx_hash == evidence.tx_hash) {
                let position = state
                    .batches
                    .partition_point(|record| record.l1_block <= evidence.l1_block.number);
                state.batches.insert(
                    position,
                    BatchRecord {
                        l1_block: evidence.l1_block.number,
                        l1_block_hash: evidence.l1_block.hash,
                        tx_hash: evidence.tx_hash,
                        last_l2_block: endpoint.number,
                    },
                );
            }
            let cursor = self.cursor().max(endpoint.number);
            // Insertion precedes the release store while holding the same
            // lock as snapshots. No reader sees a confirmed height with a
            // newly missing evidence record.
            state
                .evidence
                .settlements
                .insert(evidence.tx_hash, evidence);
            self.cursor.store(cursor, Ordering::Release);
        } else {
            state.refresh_commitment();
            state
                .evidence
                .settlements
                .insert(evidence.tx_hash, evidence);
        }
    }

    /// Finish a complete L1 scan after all replay and safe-head operations.
    pub fn mark_scanned(&self, block: L1BlockRef, endpoint: L2Endpoint) {
        let mut state = self.state.lock().unwrap();
        assert_eq!(
            endpoint.number,
            self.cursor(),
            "scan endpoint must match cursor"
        );
        state.evidence.generation = state
            .evidence
            .generation
            .checked_add(1)
            .expect("canonical evidence generation exhausted");
        state.evidence.through = Some(block);
        state.evidence.endpoint = Some(endpoint);
        state.last_scanned = Some(block);
        state.pending_l1_blocks.clear();
    }

    pub fn pending_l1_blocks(&self) -> Vec<L1BlockRef> {
        self.state
            .lock()
            .unwrap()
            .pending_l1_blocks
            .iter()
            .copied()
            .collect()
    }

    pub fn last_scanned_block(&self) -> Option<L1BlockRef> {
        self.state.lock().unwrap().last_scanned
    }

    pub fn invalidate_scan(&self) {
        let mut state = self.state.lock().unwrap();
        state.evidence.through = None;
        state.evidence.generation = state
            .evidence
            .generation
            .checked_add(1)
            .expect("canonical evidence generation exhausted");
    }

    pub fn settlement_snapshot(&self) -> SettlementSnapshot {
        self.state.lock().unwrap().evidence.clone()
    }

    pub fn policy_anchor_snapshot(&self) -> PolicyAnchorSnapshot {
        let state = self.state.lock().unwrap();
        PolicyAnchorSnapshot {
            generation: state.evidence.generation,
            through: state.evidence.through,
            endpoint: state.evidence.endpoint,
            commitment: state.latest_commitment,
        }
    }

    /// Recheck immediately before changing ownership under the engine
    /// reconciliation lock. RPC reads made against an older snapshot are
    /// inconclusive after publication or reorg.
    pub fn generation_is(&self, generation: u64) -> bool {
        self.state.lock().unwrap().evidence.generation == generation
    }

    /// Invalidate by canonical L1 identity, including replacements at the
    /// same L2 height. Incomplete scans cannot authorize recovery.
    pub fn retreat_on_l1_reorg(&self, ancestor: u64) -> (u64, u64, usize) {
        let mut state = self.state.lock().unwrap();
        if let Some(history) = &mut state.derivation {
            history.retain_through(ancestor);
        }
        let original_len = state.batches.len();
        state.batches.retain(|b| b.l1_block <= ancestor);
        state
            .commitments
            .retain(|&(number, _), _| number <= ancestor);
        state.refresh_commitment();
        state
            .evidence
            .settlements
            .retain(|_, s| s.l1_block.number <= ancestor);
        state.evidence.through = None;
        state.evidence.endpoint = None;
        state.pending_l1_blocks.clear();
        if state
            .last_scanned
            .is_some_and(|block| block.number > ancestor)
        {
            state.last_scanned = None;
        }
        state.evidence.generation = state
            .evidence
            .generation
            .checked_add(1)
            .expect("canonical evidence generation exhausted");
        let cursor = state.batches.last().map_or(0, |b| b.last_l2_block);
        self.cursor.store(cursor, Ordering::Release);
        let finalized = self.finalized_l2().min(cursor);
        self.finalized_l2.store(finalized, Ordering::Release);
        (cursor, finalized, original_len - state.batches.len())
    }

    pub fn highest_l2_at_or_below_l1(&self, bound: u64) -> Option<u64> {
        self.state
            .lock()
            .unwrap()
            .batches
            .iter()
            .rev()
            .find(|b| b.l1_block <= bound)
            .map(|b| b.last_l2_block)
    }

    pub fn set_finalized_l2(&self, l2_block: u64) {
        self.finalized_l2.store(l2_block, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(l1: u64, tx: u8, height: Option<u64>) -> ReconciledBatchSettlement {
        ReconciledBatchSettlement {
            l1_block: L1BlockRef {
                number: l1,
                hash: B256::with_last_byte(1),
            },
            tx_hash: B256::with_last_byte(tx),
            tx_index: u64::from(tx),
            calldata_hash: B256::with_last_byte(tx),
            applied_effects: 0..2,
            anchor_applied: true,
            endpoint: height.map(|number| L2Endpoint {
                number,
                hash: B256::with_last_byte(tx),
            }),
            selection: crate::ReplaySelection::default(),
        }
    }

    #[test]
    fn policy_anchor_orders_same_block_events_and_requires_complete_scan() {
        let head = L1CanonicalHead::default();
        let first = evidence(100, 1, Some(50));
        let second = evidence(100, 2, Some(50));
        // This also checks arrival ordering does not replace transaction order.
        head.publish(second.clone());
        head.publish(first);
        let snapshot = head.policy_anchor_snapshot();
        assert!(snapshot.through.is_none());
        assert_eq!(snapshot.commitment.unwrap().tx_hash, second.tx_hash);
        head.mark_scanned(second.l1_block, second.endpoint.unwrap());
        let snapshot = head.policy_anchor_snapshot();
        assert!(snapshot.through.is_some());
        assert_eq!(snapshot.endpoint, second.endpoint);
        head.publish(second);
        assert_eq!(
            head.policy_anchor_snapshot().generation,
            snapshot.generation
        );
    }

    #[test]
    fn unchanged_commitment_and_source_only_completion_do_not_reset_anchor() {
        let head = L1CanonicalHead::default();
        let first = evidence(100, 1, Some(50));
        let mut same = evidence(101, 2, Some(50));
        same.endpoint = first.endpoint;
        head.publish(first.clone());
        head.publish(same.clone());
        head.publish(evidence(102, 3, None));
        head.mark_scanned(same.l1_block, same.endpoint.unwrap());
        assert_eq!(
            head.policy_anchor_snapshot().commitment.unwrap().tx_hash,
            first.tx_hash
        );
        let changed = evidence(103, 4, Some(50));
        head.publish(changed.clone());
        head.mark_scanned(changed.l1_block, changed.endpoint.unwrap());
        assert_eq!(
            head.policy_anchor_snapshot().commitment.unwrap().tx_hash,
            changed.tx_hash
        );
        head.retreat_on_l1_reorg(101);
        assert!(head.policy_anchor_snapshot().through.is_none());
        assert_eq!(
            head.policy_anchor_snapshot().commitment.unwrap().tx_hash,
            first.tx_hash
        );
    }

    #[test]
    fn cursor_publication_is_incomplete_until_entire_block_is_scanned() {
        let head = L1CanonicalHead::default();
        let first = evidence(100, 1, Some(50));
        let second = evidence(100, 2, Some(50));
        head.publish(first.clone());
        assert_eq!(head.cursor(), 50);
        let in_progress = head.settlement_snapshot();
        assert_eq!(in_progress.through, None);
        assert_eq!(in_progress.settlements[&first.tx_hash], first);
        head.publish(second.clone());
        assert!(!head.generation_is(in_progress.generation));
        assert!(head.settlement_snapshot().through.is_none());
        head.mark_scanned(second.l1_block, second.endpoint.unwrap());
        let complete = head.settlement_snapshot();
        assert_eq!(complete.through, Some(second.l1_block));
        assert_eq!(complete.endpoint, second.endpoint);
        assert_eq!(complete.settlements.len(), 2);
        assert_eq!(
            complete.settlements[&first.tx_hash].endpoint,
            first.endpoint
        );
        assert!(head.pending_l1_blocks().is_empty());
        head.publish(second);
        assert!(
            head.generation_is(complete.generation),
            "duplicate publication is idempotent"
        );
    }

    #[test]
    fn reorg_invalidates_same_height_evidence_and_old_snapshots() {
        let head = L1CanonicalHead::default();
        let prior = evidence(99, 1, Some(49));
        let old = evidence(100, 2, Some(50));
        head.publish(prior.clone());
        head.publish(old.clone());
        head.mark_scanned(old.l1_block, old.endpoint.unwrap());
        let before = head.settlement_snapshot();
        assert_eq!(head.retreat_on_l1_reorg(99).0, 49);
        assert!(!head.generation_is(before.generation));
        assert!(head.settlement_snapshot().through.is_none());
        assert!(
            !head
                .settlement_snapshot()
                .settlements
                .contains_key(&old.tx_hash)
        );
        let mut replacement = old;
        replacement.l1_block.hash = B256::with_last_byte(3);
        replacement.endpoint.as_mut().unwrap().hash = B256::with_last_byte(4);
        head.publish(replacement.clone());
        head.mark_scanned(replacement.l1_block, replacement.endpoint.unwrap());
        assert_eq!(
            head.settlement_snapshot().settlements[&replacement.tx_hash],
            replacement
        );
        assert_eq!(head.cursor(), 50);
    }

    #[test]
    fn empty_application_is_conclusive_without_advancing_cursor() {
        let head = L1CanonicalHead::default();
        let mut empty = evidence(100, 2, None);
        empty.applied_effects = 0..0;
        empty.anchor_applied = false;
        head.publish(empty.clone());
        assert_eq!(head.cursor(), 0);
        let genesis = L2Endpoint {
            number: 0,
            hash: B256::with_last_byte(10),
        };
        head.mark_scanned(empty.l1_block, genesis);
        let snapshot = head.settlement_snapshot();
        assert_eq!(snapshot.settlements[&empty.tx_hash], empty);
        assert_eq!(snapshot.endpoint, Some(genesis));
        head.invalidate_scan();
        assert!(!head.generation_is(snapshot.generation));
        assert!(head.settlement_snapshot().through.is_none());
    }

    #[test]
    fn checkpoints_do_not_manufacture_recovery_evidence() {
        let head = L1CanonicalHead::default();
        head.append_many([record(100, 1, 50), record(100, 1, 999)]);
        assert_eq!(head.cursor(), 50);
        assert!(head.settlement_snapshot().settlements.is_empty());
        assert!(head.settlement_snapshot().through.is_none());
    }

    fn record(l1_block: u64, tx_byte: u8, last_l2_block: u64) -> BatchRecord {
        BatchRecord {
            l1_block,
            l1_block_hash: B256::with_last_byte(u8::try_from(l1_block).expect("test block fits")),
            tx_hash: B256::with_last_byte(tx_byte),
            last_l2_block,
        }
    }

    #[test]
    fn append_dedups_by_tx_hash_not_l1_block() {
        let head = L1CanonicalHead::default();
        head.append(record(100, 0xaa, 50));
        head.append(record(100, 0xbb, 60)); // same L1 block, different tx — must be kept
        head.append(record(100, 0xaa, 70)); // duplicate tx_hash — dropped
        assert_eq!(head.cursor(), 60);
        assert_eq!(head.last_indexed_l2(), 60);
        assert!(head.contains_l1_tx(&B256::with_last_byte(0xaa)));
        assert!(head.contains_l1_tx(&B256::with_last_byte(0xbb)));
    }

    #[test]
    fn append_many_dedups_and_advances_cursor() {
        let head = L1CanonicalHead::default();
        head.append(record(100, 0xaa, 50));
        head.append_many([
            record(100, 0xaa, 999), // dup tx_hash — ignored
            record(100, 0xbb, 60),  // same L1 block, new tx — kept
            record(101, 0xcc, 70),  // new L1 block — kept
        ]);
        assert_eq!(head.cursor(), 70);
        assert_eq!(head.known_tx_hashes().len(), 3);
    }

    /// What the boot checkpoint's guard rests on: more than one batch in the
    /// tail's L1 block means a resume would seed one hash and replay the other.
    #[test]
    fn count_at_l1_block_counts_that_block_only() {
        let head = L1CanonicalHead::default();
        head.append_many([
            record(100, 0xaa, 50),
            record(100, 0xbb, 60), // shares block 100 — the shape the guard rejects
            record(101, 0xcc, 70),
        ]);
        assert_eq!(
            head.count_at_l1_block(100),
            2,
            "both same-block batches count"
        );
        assert_eq!(
            head.count_at_l1_block(101),
            1,
            "a sole batch is the safe shape"
        );
        assert_eq!(
            head.count_at_l1_block(102),
            0,
            "an unindexed block holds none"
        );

        // An L1 reorg must not leave the guard reading dropped batches.
        head.retreat_on_l1_reorg(100);
        assert_eq!(
            head.count_at_l1_block(101),
            0,
            "retreat removes the block's batches"
        );
        assert_eq!(
            head.count_at_l1_block(100),
            2,
            "at-or-below the ancestor survives"
        );
    }

    #[test]
    fn retreat_drops_entries_above_common_ancestor() {
        let head = L1CanonicalHead::default();
        head.append_many([
            record(100, 0xaa, 50),
            record(100, 0xbb, 60), // same L1 block as 0xaa
            record(101, 0xcc, 70),
            record(102, 0xdd, 80),
        ]);
        head.set_finalized_l2(60);
        let (new_cursor, new_finalized, dropped) = head.retreat_on_l1_reorg(100);
        assert_eq!(new_cursor, 60); // both batches at L1=100 survive
        assert_eq!(new_finalized, 60);
        assert_eq!(dropped, 2);
    }

    #[test]
    fn highest_l2_at_or_below_l1_returns_max_within_bound() {
        let head = L1CanonicalHead::default();
        head.append_many([
            record(100, 0xaa, 50),
            record(100, 0xbb, 60),
            record(101, 0xcc, 70),
            record(102, 0xdd, 80),
        ]);
        assert_eq!(head.highest_l2_at_or_below_l1(100), Some(60));
        assert_eq!(head.highest_l2_at_or_below_l1(101), Some(70));
        assert_eq!(head.highest_l2_at_or_below_l1(99), None);
    }
}
