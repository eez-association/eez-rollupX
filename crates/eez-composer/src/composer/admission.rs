//! Automatic FIFO selection is a preparation estimate, never a protocol limit.
//! Gas and encoded DA are still checked before accepting each complete group,
//! and the final encoded transaction/bundle remains the authoritative check.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use alloy_eips::Decodable2718;
use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use eez_driver::RollupTiming;
use eez_l1::BundleTarget;

use crate::{
    prover_retry::{proof_start_cutoff_ms, unix_time_millis},
    settlement_policy::index::DaOverhead,
};

#[derive(Clone, Copy, Debug)]
struct Sample {
    groups: usize,
    preparation: Duration,
    tail: Duration,
    blob_preparation: Duration,
    blobs: usize,
    group_work: Option<Duration>,
}

impl Sample {
    /// Admission already measures source/target execution, canonical appends,
    /// prefix replay and per-group encoding. A clean sample relates the accepted
    /// groups' measured work to the remaining build/encoding/witness passes.
    /// Otherwise reserve a full admission pass. Keep the prior finishing
    /// observation as a floor, scaling it down for a smaller candidate but never
    /// multiplying a small post's fixed overhead by the new group count. KZG is
    /// separate. Add jitter to a calibrated estimate. The uncalibrated full-pass
    /// fallback is already conservative: it floors the observed tail plus
    /// jitter, rather than adding both allowances to a small post's fixed cost.
    /// This is an estimate; proof/dispatch deadlines still bind.
    fn tail_for(self, groups: usize, blobs: usize, work: Duration, jitter: Duration) -> u128 {
        let variable = self.tail.saturating_sub(self.blob_preparation).as_nanos();
        let observed = variable
            .checked_mul(groups.min(self.groups.max(1)) as u128)
            .map_or(u128::MAX, |n| n.div_ceil(self.groups.max(1) as u128));
        // Never learn a zero reserve or amplify fixed overhead. Missing or
        // interrupted observations retain the full-pass estimate. The current
        // work includes setup, classification and canonical appends, while the
        // denominator measures only accepted group simulation/prefix work.
        let finishing = match self.group_work.map(|d| d.as_nanos()) {
            Some(measured) if variable > 0 && measured > variable => {
                let predicted = work
                    .as_nanos()
                    .checked_mul(variable)
                    .map_or(u128::MAX, |n| n.div_ceil(measured));
                observed.max(predicted).saturating_add(jitter.as_nanos())
            }
            _ => observed
                .saturating_add(jitter.as_nanos())
                .max(work.as_nanos()),
        };
        let kzg = self
            .blob_preparation
            .as_nanos()
            .checked_mul(blobs as u128)
            .map_or(u128::MAX, |n| n.div_ceil(self.blobs.max(1) as u128));
        finishing.saturating_add(kzg)
    }
}

#[derive(Default, Debug)]
pub(super) struct AutomaticAdmission {
    sample: Option<Sample>,
    retry_limit: Option<usize>,
}

impl AutomaticAdmission {
    /// Relay rejection, exclusion or partial consumption can expose a limit
    /// absent from the local gas/blob model. Canonical reconciliation decides
    /// which users retry; only shrink their next preparation selection here.
    pub fn submission_failed(&mut self, submitted: usize) -> usize {
        let limit = (submitted / 2).max(1);
        self.retry_limit = Some(self.retry_limit.map_or(limit, |old| old.min(limit)));
        self.retry_limit.unwrap()
    }

    fn limit(&self, queued: usize, available: Duration) -> usize {
        if available.is_zero() {
            return 0;
        }
        // Learn the final preparation tail with one cold-start group. Once it
        // is known, inspect the finite FIFO snapshot and stop incrementally at
        // the actual time/gas/blob budget. A low-traffic sample must not reset a
        // learned network to a one-user or doubling-only admission ramp.
        let estimated = self.sample.map_or(1, |_| queued);
        estimated
            .min(self.retry_limit.unwrap_or(usize::MAX))
            .min(queued)
    }

    pub fn begin(&self, queued: usize, timing: RollupTiming, target: BundleTarget) -> Attempt {
        let available = Duration::from_millis(
            proof_start_cutoff_ms(timing, target)
                // A missing target has no safe preparation budget. The normal
                // minimal/history path resolves a target without draining users.
                .unwrap_or(0)
                .saturating_sub(unix_time_millis()),
        );
        Attempt {
            started: Instant::now(),
            available,
            selected: self.limit(queued, available),
            previous: self.sample,
            progress: Mutex::default(),
        }
    }

    pub fn finish(&mut self, attempt: &Attempt) {
        let progress = attempt.progress.lock().unwrap();
        // Waiting for source state says nothing about this network's capacity.
        // Keep the last useful sample and backoff when no group was prepared.
        if progress.source_deferred
            && progress.completed
            && !progress.limited
            && progress.prepared.is_none()
        {
            return;
        }
        if let Some(sample) = progress.prepared {
            self.sample = Some(sample);
        } else if progress.estimate_blocked && progress.accepted == 0 {
            // A transiently slow sample must not prevent every later attempt
            // from learning again. With no admitted group, discard only the
            // performance estimate and let the retry limit below probe one
            // FIFO group next time. Exact resource/deadline guards still apply.
            self.sample = None;
        }
        self.retry_limit = if !progress.completed
            || progress
                .prepared
                .is_none_or(|s| s.preparation > attempt.available)
        {
            // Selected is FIFO lookahead, which may be the whole backlog. A
            // cutoff miss must reduce the workload that actually reached
            // preparation, including a smaller rebuilt candidate, rather than
            // leave enough lookahead to repeat the same oversized prefix.
            let admitted = progress.prepared.map_or(progress.accepted, |s| s.groups);
            let workload = if admitted == 0 {
                attempt.selected
            } else {
                admitted.min(attempt.selected)
            };
            Some((workload / 2).max(1))
        } else {
            None
        };
        // Shrink lookahead when outbound-first execution bypassed an older
        // eligible group. A successful FIFO prefix already made fair progress;
        // let the next opportunity use its measured budgets, not a count cap.
        if progress.limited && (progress.fifo_bypassed || progress.prepared.is_none()) {
            let fair_limit = progress.accepted.max(1).min((attempt.selected / 2).max(1));
            self.retry_limit = Some(
                self.retry_limit
                    .map_or(fair_limit, |limit| limit.min(fair_limit)),
            );
        }
    }
}

#[derive(Default, Debug)]
// Independent observations during preparation; several can hold at once.
#[allow(clippy::struct_excessive_bools)]
struct Progress {
    source_deferred: bool,
    estimate_blocked: bool,
    phase: Duration,
    accepted: usize,
    limited: bool,
    fifo_bypassed: bool,
    completed: bool,
    prepared: Option<Sample>,
    blob_preparation: Duration,
    blobs: usize,
    started_groups: usize,
    measured_groups: usize,
    group_started: Option<(usize, Duration)>,
    group_work: Duration,
    unmeasured_work: bool,
}

impl Progress {
    fn sample(&self, groups: usize, elapsed: Duration) -> Sample {
        Sample {
            groups,
            preparation: elapsed,
            tail: elapsed.saturating_sub(self.phase),
            blob_preparation: self.blob_preparation,
            blobs: self.blobs,
            // A rejected group, source-state wait, failed append or later
            // rebuild must not make finishing appear artificially cheap.
            group_work: (!self.source_deferred
                && !self.unmeasured_work
                && self.prepared.is_none()
                && self.started_groups == groups
                && self.measured_groups == groups
                && self.group_started.is_none()
                && !self.group_work.is_zero())
            .then_some(self.group_work),
        }
    }
}

#[derive(Debug)]
pub(super) struct Attempt {
    started: Instant,
    available: Duration,
    pub selected: usize,
    previous: Option<Sample>,
    progress: Mutex<Progress>,
}

impl Attempt {
    /// Add 10% of the actual window to measured finishing predictions. The
    /// conservative full-pass fallback can cover this allowance already.
    /// The proof budget and absolute cutoff remain authoritative.
    fn jitter_reserve(&self) -> Duration {
        self.available / 10
    }

    pub fn source_deferred(&self, deferred: bool) {
        self.progress.lock().unwrap().source_deferred |= deferred;
    }

    pub fn can_start_group(&self, accepted: usize) -> bool {
        self.can_start_at(accepted, self.started.elapsed())
    }

    /// Reserve finishing time when checking deferred sources, without recording
    /// an execution attempt or contaminating accepted-group timing samples.
    pub fn can_check_source(&self, accepted: usize) -> bool {
        self.next_group_fits(accepted, self.started.elapsed())
    }

    fn next_group_fits(&self, accepted: usize, elapsed: Duration) -> bool {
        let remaining = self.available.saturating_sub(elapsed);
        if remaining.is_zero() {
            return false;
        }
        let next = accepted.saturating_add(1);
        let blobs = self.progress.lock().unwrap().blobs.max(1);
        let jitter = self.jitter_reserve();
        let tail = self.previous.map_or(elapsed.max(jitter).as_nanos(), |s| {
            s.tail_for(next, blobs, elapsed, jitter)
        });
        let group = if accepted == 0 {
            self.previous.map_or(0, |s| {
                s.preparation.saturating_sub(s.tail).as_nanos() / s.groups.max(1) as u128
            })
        } else {
            elapsed.as_nanos() / accepted as u128
        };
        remaining.as_nanos() > group.saturating_add(tail)
    }

    fn can_start_at(&self, accepted: usize, elapsed: Duration) -> bool {
        let fits = self.next_group_fits(accepted, elapsed);
        let next = accepted.saturating_add(1);
        let mut progress = self.progress.lock().unwrap();
        if fits {
            if let Some(started) = progress.started_groups.checked_add(1) {
                progress.started_groups = started;
            } else {
                progress.unmeasured_work = true;
            }
            progress.unmeasured_work |= progress.group_started.is_some();
            progress.group_started = Some((next, elapsed));
        } else if accepted == 0 && self.previous.is_some() {
            progress.estimate_blocked = true;
        }
        fits
    }

    /// A just-simulated group can cross a blob boundary. Recheck its full KZG
    /// allowance before committing it to the draft; a count estimate must not
    /// conceal the extra physical blob's preparation cost.
    pub fn can_finish_group(&self, groups: usize, blobs: usize) -> bool {
        self.can_finish_at(groups, blobs, self.started.elapsed())
    }

    fn can_finish_at(&self, groups: usize, blobs: usize, elapsed: Duration) -> bool {
        let remaining = self.available.saturating_sub(elapsed);
        let jitter = self.jitter_reserve();
        let tail = self.previous.map_or(jitter.as_nanos(), |s| {
            s.tail_for(groups, blobs, elapsed, jitter)
        });
        let mut progress = self.progress.lock().unwrap();
        let measured = progress
            .group_started
            .take()
            .and_then(|(started_groups, started)| {
                (started_groups == groups)
                    .then(|| elapsed.checked_sub(started))
                    .flatten()
            });
        progress.unmeasured_work |= measured.is_none();
        if remaining.as_nanos() <= tail {
            // This matching group is refused before its duration is added or
            // the caller appends it. Its work must not train the estimate, but
            // it does not invalidate the earlier accepted prefix. Cancel only
            // this known final start; every unknown/failed group still prevents
            // the final started/measured/prepared counts from agreeing.
            if measured.is_some()
                && !progress.unmeasured_work
                && progress.started_groups == groups
                && progress.measured_groups.checked_add(1) == Some(groups)
            {
                progress.started_groups = progress.measured_groups;
            }
            if groups == 1 && !remaining.is_zero() && self.previous.is_some() {
                progress.estimate_blocked = true;
            }
            return false;
        }
        progress.blobs = blobs;
        if let Some(work) = measured
            && let Some(total) = progress.group_work.checked_add(work)
            && let Some(groups) = progress.measured_groups.checked_add(1)
        {
            progress.group_work = total;
            progress.measured_groups = groups;
        } else {
            progress.unmeasured_work = true;
        }
        true
    }

    pub fn blobs_prepared(&self, blobs: usize, elapsed: Duration) {
        let mut progress = self.progress.lock().unwrap();
        progress.blobs = blobs;
        progress.blob_preparation = elapsed;
    }

    pub fn phase_finished(&self, accepted: usize, limited: bool, fifo_bypassed: bool) {
        let mut progress = self.progress.lock().unwrap();
        progress.phase = self.started.elapsed();
        progress.accepted = accepted;
        progress.limited |= limited;
        progress.fifo_bypassed |= fifo_bypassed;
    }

    pub fn completed(&self) {
        self.progress.lock().unwrap().completed = true;
    }

    pub fn prepared(&self, rollup_id: u64, groups: usize) {
        if groups == 0 {
            return;
        }
        let elapsed = self.started.elapsed();
        let mut progress = self.progress.lock().unwrap();
        let sample = progress.sample(groups, elapsed);
        progress.prepared = Some(sample);
        tracing::event!(name: "eez.composer.admission.prepared", tracing::Level::INFO,
            event_name = "eez.composer.admission.prepared", rollup_id,
            selected = self.selected, accepted = groups,
            preparation_ms = elapsed.as_millis() as u64,
            admission_ms = progress.phase.as_millis() as u64,
            finishing_ms = sample.tail.as_millis() as u64,
            non_blob_finishing_ms = sample.tail.saturating_sub(sample.blob_preparation).as_millis() as u64,
            measured_group_work_ms = progress.group_work.as_millis() as u64,
            group_work_calibrated = sample.group_work.is_some(),
            blob_preparation_ms = progress.blob_preparation.as_millis() as u64,
            blob_count = progress.blobs,
            available_ms = self.available.as_millis() as u64,
            jitter_reserve_ms = self.jitter_reserve().as_millis() as u64,
            limited = progress.limited,
            "measured automatic admission through the start of proof generation");
    }
}

/// Incremental upper bound for the indivisible terminal Sync block. Earlier
/// history is handled by the existing exact encoding / historical-drain path.
/// Unknown execution header fields and RLP prefixes are rounded up. Transaction,
/// metadata and semantic bytes use their actual wire encodings, without padding.
#[derive(Clone, Copy, Debug)]
pub(super) struct BlobAdmission {
    header_and_body: usize,
    header_and_body_items: usize,
    transaction_items: usize,
    transaction_byte_items: usize,
    overhead: DaOverhead,
    pub companion_blobs: u64,
}

fn list_len(items: usize) -> Result<usize, String> {
    items
        .checked_add(
            alloy_rlp::Header {
                list: true,
                payload_length: items,
            }
            .length(),
        )
        .ok_or_else(|| "admission RLP size overflow".into())
}

impl BlobAdmission {
    pub fn new(mut empty: reth_ethereum_primitives::Block) -> Self {
        // These fields are only known after final execution. Maximal encodings
        // bound their contribution to Vec<u8>'s RLP INTEGER list, where every
        // byte >= 128 takes two bytes. The remaining header fields are fixed by
        // the same parent and next-block attributes as the canonical builder.
        empty.header.gas_used = u64::MAX;
        empty.header.state_root = alloy_primitives::B256::repeat_byte(0xff);
        empty.header.receipts_root = alloy_primitives::B256::repeat_byte(0xff);
        empty.header.transactions_root = alloy_primitives::B256::repeat_byte(0xff);
        empty.header.logs_bloom = alloy_primitives::Bloom::repeat_byte(0xff);
        if let Some(hash) = &mut empty.header.requests_hash {
            *hash = alloy_primitives::B256::repeat_byte(0xff);
        }
        if let Some(blob_gas) = &mut empty.header.blob_gas_used {
            *blob_gas = u64::MAX;
        }
        let header = alloy_rlp::encode(&empty.header);
        let body_lists = 1 + usize::from(empty.body.withdrawals.is_some());
        Self {
            // Empty ommers, plus an empty withdrawals list when present. The
            // synthetic Sync builder never adds ommers or withdrawals.
            header_and_body: header.len() + body_lists,
            // The header's own RLP prefix can change when gas-used changes.
            // Charge its maximum byte expansion even if this template's prefix
            // happened to contain low bytes.
            header_and_body_items: header.iter().map(Encodable::length).sum::<usize>()
                + 2 * body_lists
                + 2 * (1 + std::mem::size_of::<usize>()),
            transaction_items: 0,
            transaction_byte_items: 0,
            overhead: DaOverhead::default(),
            companion_blobs: 0,
        }
    }

    pub fn extend(
        &self,
        txs: &[Bytes],
        overhead: DaOverhead,
        companion: Option<&Bytes>,
    ) -> Result<Self, String> {
        let mut next = *self;
        for raw in txs {
            let tx = alloy_consensus::TxEnvelope::decode_2718(&mut raw.as_ref())
                .map_err(|e| format!("admission transaction decode: {e}"))?;
            next.transaction_items = next
                .transaction_items
                .checked_add(tx.length())
                .ok_or("admission transaction size overflow")?;
            let encoded = alloy_rlp::encode(tx);
            next.transaction_byte_items = next
                .transaction_byte_items
                .checked_add(encoded.iter().map(Encodable::length).sum::<usize>())
                .ok_or("admission encoded transaction size overflow")?;
        }
        if let Some(raw) = companion {
            use alloy_consensus::Transaction;
            let tx = alloy_consensus::TxEnvelope::decode_2718(&mut raw.as_ref())
                .map_err(|e| format!("admission companion decode: {e}"))?;
            next.companion_blobs = next
                .companion_blobs
                .checked_add(
                    tx.blob_versioned_hashes()
                        .map_or(0, |hashes| hashes.len() as u64),
                )
                .ok_or("admission companion blob count overflow")?;
        }
        next.overhead.entry_items = next
            .overhead
            .entry_items
            .checked_add(overhead.entry_items)
            .ok_or("admission entry size overflow")?;
        next.overhead.group_items = next
            .overhead
            .group_items
            .checked_add(overhead.group_items)
            .ok_or("admission group size overflow")?;
        next.overhead.semantic_bytes = next
            .overhead
            .semantic_bytes
            .checked_add(overhead.semantic_bytes)
            .ok_or("admission semantic size overflow")?;
        Ok(next)
    }

    pub fn stream_bytes(&self) -> Result<usize, String> {
        let block_payload = self
            .header_and_body
            .checked_add(list_len(self.transaction_items)?)
            .ok_or("admission block size overflow")?;
        let tx_prefix = list_len(self.transaction_items)? - self.transaction_items;
        let block_prefix = list_len(block_payload)? - block_payload;
        let block_byte_items = self
            .header_and_body_items
            .checked_add(self.transaction_byte_items)
            .and_then(|n| n.checked_add(2 * (tx_prefix + block_prefix)))
            .ok_or("admission block byte list size overflow")?;
        // Preserve the codec's Vec<u8> representation: an RLP integer list,
        // not Bytes. Prefix bytes may change as the final header gets shorter.
        let block_item = block_byte_items
            .checked_add(2 * (list_len(block_byte_items)? - block_byte_items))
            .ok_or("admission block item size overflow")?;
        let payload = eez_payload_codec::DerivablePayloadSizer::default()
            .encoded_len_from_items(
                &eez_payload_codec::derivable::Environment::unused(),
                block_item,
                self.overhead.entry_items,
                self.overhead.group_items,
            )
            .map_err(|e| e.to_string())?;
        eez_blob_codec::native_stream_len(payload, self.overhead.semantic_bytes)
            .map_err(|e| e.to_string())
    }

    pub fn fits(&self, blobs: usize) -> Result<bool, String> {
        Ok(self.required_blobs()? <= blobs)
    }

    pub fn required_blobs(&self) -> Result<usize, String> {
        Ok(self
            .stream_bytes()?
            .div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip1559, TxEip4844, TxEnvelope, TxLegacy};
    use alloy_eips::Encodable2718;
    use alloy_primitives::{B256, Signature, U256};

    fn sample(groups: usize, preparation_ms: u64) -> Sample {
        Sample {
            groups,
            preparation: Duration::from_millis(preparation_ms),
            tail: Duration::from_millis(preparation_ms / 2),
            blob_preparation: Duration::ZERO,
            blobs: 1,
            group_work: None,
        }
    }

    fn attempt(
        selected: usize,
        accepted: usize,
        limited: bool,
        prepared: Option<Sample>,
    ) -> Attempt {
        Attempt {
            started: Instant::now(),
            available: Duration::from_secs(1),
            selected,
            previous: prepared,
            progress: Mutex::new(Progress {
                accepted,
                limited,
                completed: prepared.is_some(),
                prepared,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn source_checks_reserve_time_without_recording_execution_work() {
        let attempt = attempt(3, 0, false, None);
        assert!(attempt.can_check_source(0));
        assert_eq!(attempt.progress.lock().unwrap().started_groups, 0);
        assert!(attempt.progress.lock().unwrap().group_started.is_none());
        assert!(!attempt.next_group_fits(0, attempt.available));
        assert!(attempt.can_start_group(0));
        assert_eq!(attempt.progress.lock().unwrap().started_groups, 1);
    }

    #[test]
    fn automatic_selection_learns_once_then_uses_finite_snapshot_and_actual_time() {
        let mut state = AutomaticAdmission::default();
        assert_eq!(state.limit(1000, Duration::ZERO), 0);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1);
        assert_eq!(state.limit(0, Duration::from_secs(1)), 0);
        let unknown_target = state.begin(
            1000,
            RollupTiming::new(5000, 1000, 1000, 100),
            BundleTarget::NextBlock,
        );
        assert_eq!(
            unknown_target.selected, 0,
            "no safe admission deadline without an exact target"
        );
        for groups in [1, 2, 4, 8, 16, 32, 64] {
            state.finish(&attempt(groups, groups, false, Some(sample(groups, 200))));
            assert_eq!(state.limit(1000, Duration::from_secs(1)), 1000);
        }
        let mut a = attempt(1000, 0, false, state.sample);
        a.available = Duration::from_millis(50);
        assert!(a.can_start_at(0, Duration::ZERO));
        assert!(!a.can_start_at(64, Duration::from_millis(30)));
        assert_eq!(state.limit(3, Duration::from_secs(1)), 3);
        // Restart discards only performance estimates.
        assert_eq!(
            AutomaticAdmission::default().limit(1000, Duration::from_secs(1)),
            1
        );
    }

    #[test]
    fn capacity_cuts_reduce_fifo_lookahead_until_an_older_opposite_direction_is_alone() {
        let mut state = AutomaticAdmission {
            sample: Some(sample(64, 100)),
            retry_limit: None,
        };
        let mut selected = 64;
        while selected > 1 {
            // Outbound-first execution used the budget before the FIFO's oldest inbound.
            let a = attempt(
                selected,
                selected - 1,
                true,
                Some(sample(selected - 1, 100)),
            );
            a.progress.lock().unwrap().fifo_bypassed = true;
            state.finish(&a);
            let next = state.limit(1000, Duration::from_secs(1));
            assert!(next < selected);
            selected = next;
        }
        assert_eq!(selected, 1);
    }

    #[test]
    fn successful_fifo_prefix_cut_does_not_cap_the_next_opportunity() {
        let mut state = AutomaticAdmission::default();
        state.finish(&attempt(30, 10, true, Some(sample(10, 250))));
        assert_eq!(state.limit(20, Duration::from_millis(500)), 20);
        // A failed submission still backs off, even after fair local progress.
        state.submission_failed(10);
        assert_eq!(state.limit(20, Duration::from_millis(500)), 5);
    }

    #[test]
    fn timeouts_and_incomplete_preparation_back_off_without_an_unbounded_retry() {
        let mut state = AutomaticAdmission::default();
        state.finish(&attempt(64, 64, false, Some(sample(64, 2000))));
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 32);
        state.finish(&attempt(32, 0, false, None));
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 16);
        state.finish(&attempt(16, 0, true, None));
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1);
        let failed_proof = attempt(16, 16, false, Some(sample(16, 100)));
        failed_proof.progress.lock().unwrap().completed = false;
        state.finish(&failed_proof);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 8);
        state.submission_failed(8);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 4);
        state.submission_failed(4);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 2);
        state.submission_failed(2);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1);
    }

    #[test]
    fn failed_preparation_reduces_admitted_work_even_with_a_large_fifo_snapshot() {
        // Chiado: 1,164 selected, 60 admitted, 498 ms preparation against
        // 491 ms available. Halving lookahead to 582 still admits ~60 again.
        for selected in [60, 1164, usize::MAX] {
            for prepared in [None, Some(sample(60, 498))] {
                for fifo_bypassed in [false, true] {
                    let mut state = AutomaticAdmission {
                        sample: Some(sample(62, 477)),
                        retry_limit: None,
                    };
                    let mut failed = attempt(selected, 60, true, prepared);
                    failed.available = Duration::from_millis(491);
                    {
                        let mut progress = failed.progress.lock().unwrap();
                        progress.completed = false;
                        progress.fifo_bypassed = fifo_bypassed;
                    }
                    state.finish(&failed);
                    assert_eq!(state.limit(1164, failed.available), 30);
                    // Successful fair progress releases the temporary count cap.
                    state.finish(&attempt(30, 30, true, Some(sample(30, 300))));
                    assert_eq!(state.limit(1164, failed.available), 1164);
                }
            }
        }
    }

    #[test]
    fn failed_rebuilt_candidate_backs_off_from_its_final_prepared_work() {
        let mut state = AutomaticAdmission::default();
        let failed = attempt(1164, 60, true, Some(sample(12, 1100)));
        state.finish(&failed);
        assert_eq!(state.limit(1164, failed.available), 6);
    }

    #[test]
    fn recovery_admission_leaves_room_for_finishing_jitter() {
        // Last successful Chiado preparation before the 11:46:38 cutoff miss.
        let previous = Sample {
            groups: 62,
            preparation: Duration::from_millis(477),
            tail: Duration::from_millis(197),
            blob_preparation: Duration::from_millis(176),
            blobs: 1,
            group_work: Some(Duration::from_millis(266)),
        };
        let mut a = attempt(1164, 0, false, Some(previous));
        a.available = Duration::from_millis(491);
        // A smaller prefix still fits, including a 30 ms finishing overrun.
        let elapsed = Duration::from_millis(240);
        assert!(a.can_start_at(49, elapsed));
        assert!(a.can_finish_at(50, 1, elapsed));
        assert!(
            elapsed.as_nanos()
                + previous.tail_for(50, 1, elapsed, Duration::ZERO)
                + Duration::from_millis(30).as_nanos()
                < a.available.as_nanos()
        );
        // The old estimate admitted more work with almost no finishing margin.
        assert!(!a.can_start_at(59, Duration::from_millis(279)));
        assert!(!a.can_finish_at(60, 1, Duration::from_millis(279)));
        assert!(!a.can_finish_at(1, 1, a.available));
    }

    #[test]
    fn group_start_reserves_final_preparation_and_stops_at_the_shared_proof_cutoff() {
        let a = attempt(64, 0, false, Some(sample(10, 500)));
        assert!(a.can_start_at(0, Duration::from_millis(1)));
        assert!(!a.can_start_at(0, Duration::from_secs(1)));
        assert!(a.can_start_at(5, Duration::from_millis(300)));
        assert!(!a.can_start_at(10, Duration::from_millis(700)));
    }

    #[test]
    fn full_pass_fallback_covers_jitter_without_splitting_a_fitting_wave() {
        // Native 45-source regression: the one-source warmup has more fixed
        // finishing overhead than measured source work. Its conservative full
        // pass already leaves much more headroom than the 10% jitter allowance.
        let observed = Sample {
            groups: 1,
            preparation: Duration::from_millis(187),
            tail: Duration::from_millis(181),
            blob_preparation: Duration::from_millis(172),
            blobs: 1,
            group_work: Some(Duration::from_millis(2)),
        };
        let mut a = attempt(45, 0, false, Some(observed));
        a.available = Duration::from_millis(495);
        assert!(a.can_start_at(44, Duration::from_millis(145)));
        assert!(a.can_finish_at(45, 1, Duration::from_millis(150)));
        assert!(!a.can_finish_at(45, 2, Duration::from_millis(150)));
        assert!(!a.can_finish_at(45, 1, a.available));
        // Fast source work must still leave jitter above the observed tail.
        a.available = Duration::from_millis(200);
        assert!(!a.can_start_at(0, Duration::ZERO));
    }

    #[test]
    fn one_blob_wave_is_not_limited_by_quadratic_kzg_or_a_small_previous_post() {
        let observed = Sample {
            groups: 1,
            preparation: Duration::from_millis(200),
            tail: Duration::from_millis(182),
            blob_preparation: Duration::from_millis(180),
            blobs: 1,
            group_work: None,
        };
        let mut state = AutomaticAdmission::default();
        state.finish(&attempt(1, 1, false, Some(observed)));
        let mut a = attempt(
            state.limit(30, Duration::from_millis(500)),
            0,
            false,
            Some(observed),
        );
        a.available = Duration::from_millis(500);
        assert_eq!(
            a.selected, 30,
            "a small previous post must not impose a count ramp"
        );
        for accepted in 0..30 {
            assert!(
                a.can_start_at(accepted, Duration::from_millis(20 + accepted as u64)),
                "the measured one-blob tail leaves time for this complete group"
            );
        }
        assert!(a.can_finish_at(30, 1, Duration::from_millis(75)));
        assert!(
            !a.can_finish_at(30, 2, Duration::from_millis(100)),
            "an additional physical blob must reserve additional KZG work"
        );
        assert!(!a.can_start_at(9, Duration::from_millis(499)));
    }

    #[test]
    fn a_small_posts_fixed_overhead_does_not_limit_a_fast_thirty_group_wave() {
        let observed = Sample {
            groups: 1,
            preparation: Duration::from_millis(185),
            tail: Duration::from_millis(182),
            blob_preparation: Duration::from_millis(171),
            blobs: 1,
            group_work: None,
        };
        let mut a = attempt(30, 0, false, Some(observed));
        a.available = Duration::from_millis(495);
        assert!(a.can_start_at(26, Duration::from_millis(71)));
        assert!(a.can_finish_at(30, 1, Duration::from_millis(90)));
        assert!(
            !a.can_finish_at(30, 1, Duration::from_millis(170)),
            "slower measured work must leave time for finishing, not just KZG"
        );
    }

    #[test]
    fn a_short_window_does_not_start_a_group_whose_preparation_tail_cannot_fit() {
        let observed = Sample {
            groups: 1,
            preparation: Duration::from_millis(200),
            tail: Duration::from_millis(180),
            blob_preparation: Duration::from_millis(170),
            blobs: 1,
            group_work: None,
        };
        let mut a = attempt(10, 0, false, Some(observed));
        a.available = Duration::from_millis(101);
        assert!(!a.can_start_at(0, Duration::ZERO));
        a.available = Duration::from_millis(200);
        assert!(
            !a.can_start_at(0, Duration::ZERO),
            "equality leaves no headroom"
        );
        a.available = Duration::from_millis(500);
        assert!(a.can_start_at(0, Duration::ZERO));
    }

    #[test]
    fn a_stale_tail_estimate_defers_once_then_allows_one_probe_and_relearning() {
        let mut state = AutomaticAdmission {
            sample: Some(sample(1, 2000)),
            retry_limit: None,
        };
        let mut blocked = attempt(state.limit(100, Duration::from_millis(500)), 0, true, None);
        blocked.previous = state.sample;
        blocked.available = Duration::from_millis(500);
        assert!(!blocked.can_start_at(0, Duration::ZERO));
        state.finish(&blocked);
        assert!(state.sample.is_none());
        assert_eq!(state.limit(100, Duration::from_millis(500)), 1);
        let probe = attempt(1, 0, false, state.sample);
        assert!(probe.can_start_at(0, Duration::ZERO));
        assert!(!probe.can_start_at(0, probe.available));
        state.finish(&attempt(1, 1, false, Some(sample(1, 200))));
        assert!(state.sample.is_some());
        assert_eq!(state.limit(100, Duration::from_millis(500)), 100);
    }

    #[test]
    fn multi_blob_samples_do_not_permanently_block_smaller_candidates_and_overflow_fails_closed() {
        let observed = Sample {
            groups: 10,
            preparation: Duration::from_millis(450),
            tail: Duration::from_millis(410),
            blob_preparation: Duration::from_millis(400),
            blobs: 2,
            group_work: None,
        };
        let mut a = attempt(10, 0, false, Some(observed));
        a.available = Duration::from_millis(300);
        assert!(a.can_start_at(0, Duration::ZERO));
        assert!(a.can_finish_at(1, 1, Duration::from_millis(20)));
        assert!(!a.can_finish_at(1, 2, Duration::from_millis(20)));
        let huge = Sample {
            tail: Duration::MAX,
            blob_preparation: Duration::MAX,
            groups: 1,
            blobs: 1,
            ..observed
        };
        assert_eq!(
            huge.tail_for(1, usize::MAX, Duration::ZERO, Duration::ZERO),
            u128::MAX
        );
    }

    fn measured_attempt() -> Attempt {
        let a = attempt(2, 0, false, None);
        // Setup/classification and post-check canonical appends are deliberately
        // excluded from the training denominator. They remain part of the
        // current elapsed work used to predict finishing time.
        assert!(a.can_start_at(0, Duration::from_millis(50)));
        assert!(a.can_finish_at(1, 1, Duration::from_millis(100)));
        assert!(a.can_start_at(1, Duration::from_millis(110)));
        assert!(a.can_finish_at(2, 1, Duration::from_millis(160)));
        {
            let mut progress = a.progress.lock().unwrap();
            progress.phase = Duration::from_millis(180);
            progress.blob_preparation = Duration::from_millis(180);
        }
        a
    }

    fn prefix_with_next_group_started(previous: Sample) -> Attempt {
        let mut a = attempt(100, 0, false, None);
        a.previous = Some(previous);
        a.available = Duration::from_millis(500);
        assert!(a.can_start_at(0, Duration::from_millis(10)));
        assert!(a.can_finish_at(1, 1, Duration::from_millis(40)));
        assert!(a.can_start_at(1, Duration::from_millis(45)));
        assert!(a.can_finish_at(2, 1, Duration::from_millis(80)));
        assert!(a.can_start_at(2, Duration::from_millis(90)));
        a
    }

    fn time_refused_prefix(rejected_ms: u64) -> Attempt {
        let previous = Sample {
            groups: 1,
            preparation: Duration::from_millis(200),
            tail: Duration::from_millis(180),
            blob_preparation: Duration::from_millis(170),
            blobs: 1,
            group_work: None,
        };
        let a = prefix_with_next_group_started(previous);
        assert!(!a.can_finish_at(3, 1, Duration::from_millis(rejected_ms)));
        {
            let mut progress = a.progress.lock().unwrap();
            progress.phase = Duration::from_millis(rejected_ms + 1);
            progress.blob_preparation = Duration::from_millis(170);
        }
        a
    }

    #[test]
    fn a_known_time_refusal_preserves_prefix_learning_without_charging_rejected_work() {
        let mut reserve = None;
        for rejected_ms in [166, 499, 5_000, 500_000_000] {
            let a = time_refused_prefix(rejected_ms);
            let progress = a.progress.lock().unwrap();
            let observed = progress.sample(2, progress.phase + Duration::from_millis(200));
            assert_eq!(observed.group_work, Some(Duration::from_millis(65)));
            let next = observed.tail_for(2, 1, Duration::from_millis(200), Duration::ZERO);
            assert_eq!(
                *reserve.get_or_insert(next),
                next,
                "arbitrarily expensive rejected work cannot lower the learned reserve"
            );
            assert!(
                progress
                    .sample(1, observed.preparation)
                    .group_work
                    .is_none()
            );
            assert!(
                progress
                    .sample(3, observed.preparation)
                    .group_work
                    .is_none()
            );
        }
    }

    #[test]
    fn a_time_limited_prefix_improves_the_next_attempt_but_keeps_work_blob_and_deadline_guards() {
        let a = time_refused_prefix(166);
        let observed = {
            let progress = a.progress.lock().unwrap();
            progress.sample(2, progress.phase + Duration::from_millis(200))
        };
        let old = Sample {
            group_work: None,
            ..observed
        };
        assert!(!prefix_with_next_group_started(old).can_finish_at(
            3,
            1,
            Duration::from_millis(200)
        ));
        for (blobs, elapsed_ms, fits) in [
            // The 500 ms window leaves 450 ms for predicted work.
            (1, 180, true),
            (1, 200, false),
            (2, 70, true),
            (2, 80, false),
            (1, 500, false),
        ] {
            let next = prefix_with_next_group_started(observed);
            assert_eq!(
                next.can_finish_at(3, blobs, Duration::from_millis(elapsed_ms)),
                fits
            );
        }
        // A refused second physical blob belongs only to the discarded group.
        // Preserve both the accepted prefix's work and its one-blob sample.
        let next = prefix_with_next_group_started(observed);
        assert!(!next.can_finish_at(3, 2, Duration::from_millis(120)));
        let mut progress = next.progress.lock().unwrap();
        assert_eq!(progress.group_work, Duration::from_millis(65));
        assert_eq!(progress.blobs, 1);
        progress.phase = Duration::from_millis(130);
        progress.blob_preparation = Duration::from_millis(170);
        let prefix = progress.sample(2, Duration::from_millis(330));
        assert_eq!(prefix.group_work, Some(Duration::from_millis(65)));
        assert_eq!(prefix.blobs, 1);
        assert_eq!(
            prefix.tail_for(2, 2, Duration::from_millis(100), Duration::ZERO)
                - prefix.tail_for(2, 1, Duration::from_millis(100), Duration::ZERO),
            Duration::from_millis(170).as_nanos()
        );
    }

    #[test]
    fn time_refusal_does_not_clear_source_rebuild_append_or_bracket_uncertainty() {
        for invalid in 0..5 {
            let a = time_refused_prefix(166);
            if invalid == 4 {
                // A second finish has no start. The known first time refusal
                // must not authorize treating this unknown call as cancelled.
                assert!(!a.can_finish_at(3, 1, Duration::from_millis(170)));
            }
            let mut progress = a.progress.lock().unwrap();
            match invalid {
                0 => progress.source_deferred = true,
                1 => progress.prepared = Some(sample(2, 400)),
                2 => progress.measured_groups = 3, // finish succeeded, append failed
                3 => {
                    progress.started_groups = 3;
                    progress.group_started = Some((3, Duration::from_millis(170)));
                }
                _ => {}
            }
            let observed = progress.sample(2, progress.phase + Duration::from_millis(200));
            assert!(
                observed.group_work.is_none(),
                "invalid observation {invalid}"
            );
        }
        // Gas/blob admission can reject before can_finish. An unfinished start
        // has no explicit time-refusal cancellation and remains disqualifying.
        let a = prefix_with_next_group_started(sample(1, 200));
        assert!(
            a.progress
                .lock()
                .unwrap()
                .sample(2, Duration::from_millis(400))
                .group_work
                .is_none()
        );
        // A prior unfinished start cannot be hidden by a later matching refusal.
        assert!(a.can_start_at(2, Duration::from_millis(100)));
        assert!(!a.can_finish_at(3, 1, Duration::from_millis(500)));
        assert!(
            a.progress
                .lock()
                .unwrap()
                .sample(2, Duration::from_millis(600))
                .group_work
                .is_none()
        );
    }

    #[test]
    fn clean_measured_group_work_scales_finishing_without_repeating_all_admission() {
        let a = measured_attempt();
        let observed = a
            .progress
            .lock()
            .unwrap()
            .sample(2, Duration::from_millis(400));
        assert_eq!(observed.group_work, Some(Duration::from_millis(100)));
        assert_eq!(observed.tail, Duration::from_millis(220));
        for (groups, work_ms, expected_ms) in
            [(2, 100, 220), (2, 200, 260), (1, 25, 200), (20, 200, 260)]
        {
            assert_eq!(
                observed.tail_for(groups, 1, Duration::from_millis(work_ms), Duration::ZERO),
                Duration::from_millis(expected_ms).as_nanos()
            );
        }
        // A small prior post cannot multiply its fixed overhead as a queue grows.
        let fixed = Sample {
            group_work: Some(Duration::from_millis(10)),
            ..observed
        };
        assert_eq!(
            fixed.tail_for(20, 1, Duration::from_millis(200), Duration::ZERO),
            Duration::from_millis(380).as_nanos()
        );
    }

    #[test]
    fn learned_finishing_tracks_slower_work_and_each_physical_blob_before_commit() {
        let observed = measured_attempt()
            .progress
            .lock()
            .unwrap()
            .sample(2, Duration::from_millis(400));
        let mut a = attempt(100, 0, false, Some(observed));
        a.available = Duration::from_millis(500);
        assert!(a.can_start_at(0, Duration::ZERO));
        assert!(a.can_finish_at(20, 1, Duration::from_millis(190)));
        assert!(a.can_start_at(20, Duration::from_millis(180)));
        assert!(
            !a.can_finish_at(21, 1, Duration::from_millis(200)),
            "slower current work must increase the finishing allowance"
        );
        assert!(a.can_finish_at(2, 2, Duration::from_millis(40)));
        assert!(!a.can_finish_at(2, 2, Duration::from_millis(50)));
        assert!(!a.can_finish_at(2, 1, a.available));
        // An expensive historical finishing observation remains a floor even
        // when the next candidate's source execution is fast.
        let historical = Sample {
            tail: Duration::from_millis(320),
            ..observed
        };
        assert_eq!(
            historical.tail_for(2, 1, Duration::from_millis(20), Duration::ZERO),
            Duration::from_millis(320).as_nanos()
        );
    }

    #[test]
    fn rejected_deferred_unfinished_and_rebuilt_work_cannot_train_a_cheap_tail() {
        for failure in 0..6 {
            let a = measured_attempt();
            if failure == 0 {
                // Rejected expensive source: no matching accepted finish.
                assert!(a.can_start_at(2, Duration::from_millis(170)));
            }
            let mut progress = a.progress.lock().unwrap();
            match failure {
                1 => progress.source_deferred = true,
                2 => progress.prepared = Some(sample(2, 400)),
                3 => progress.measured_groups = 1,
                4 => progress.group_work = Duration::ZERO,
                5 => progress.unmeasured_work = true,
                _ => {}
            }
            let observed = progress.sample(2, Duration::from_millis(400));
            assert!(observed.group_work.is_none(), "failure {failure}");
            assert_eq!(
                observed.tail_for(2, 1, Duration::from_millis(200), Duration::ZERO),
                Duration::from_millis(380).as_nanos()
            );
        }
        let a = measured_attempt();
        let reduced = a
            .progress
            .lock()
            .unwrap()
            .sample(1, Duration::from_millis(400));
        assert!(
            reduced.group_work.is_none(),
            "a trimmed/rebuilt candidate cannot reuse original work"
        );
    }

    #[test]
    fn group_timing_requires_unique_matching_start_and_finish_and_valid_arithmetic() {
        for failure in 0..6 {
            let a = attempt(1, 0, false, None);
            assert!(a.can_start_at(0, Duration::from_millis(10)));
            match failure {
                0 => assert!(a.can_finish_at(2, 1, Duration::from_millis(20))),
                1 => {
                    assert!(a.can_finish_at(1, 1, Duration::from_millis(20)));
                    assert!(a.can_finish_at(1, 1, Duration::from_millis(30)));
                }
                2 => {
                    assert!(a.can_start_at(0, Duration::from_millis(20)));
                    assert!(a.can_finish_at(1, 1, Duration::from_millis(30)));
                }
                3 => assert!(a.can_finish_at(1, 1, Duration::from_millis(9))),
                4 => assert!(a.can_finish_at(1, 1, Duration::from_millis(10))),
                5 => {
                    a.progress.lock().unwrap().group_work = Duration::MAX;
                    assert!(a.can_finish_at(1, 1, Duration::from_millis(20)));
                }
                _ => unreachable!(),
            }
            let observed = a
                .progress
                .lock()
                .unwrap()
                .sample(1, Duration::from_millis(100));
            assert!(observed.group_work.is_none(), "failure {failure}");
        }
        let huge = Sample {
            groups: 1,
            preparation: Duration::MAX,
            tail: Duration::MAX / 2,
            blob_preparation: Duration::ZERO,
            blobs: 1,
            group_work: Some(Duration::MAX),
        };
        assert_eq!(
            huge.tail_for(1, 1, Duration::MAX, Duration::ZERO),
            u128::MAX
        );
    }

    fn signed(size: usize, typed: bool) -> TxEnvelope {
        let signature = Signature::new(U256::from(1), U256::from(2), false);
        if typed {
            TxEip1559 {
                gas_limit: 1_000_000,
                input: vec![0xffu8; size].into(),
                ..Default::default()
            }
            .into_signed(signature)
            .into()
        } else {
            TxLegacy {
                gas_limit: 1_000_000,
                input: vec![0u8; size].into(),
                ..Default::default()
            }
            .into_signed(signature)
            .into()
        }
    }

    #[test]
    fn incremental_da_bounds_real_encoding_across_rlp_boundaries_and_multiple_blobs() {
        for withdrawals in [false, true] {
            let mut block = reth_ethereum_primitives::Block::default();
            block.header.number = 1;
            block.header.gas_limit = 30_000_000;
            block.header.gas_used = block.header.gas_limit;
            if withdrawals {
                block.body.withdrawals = Some(Default::default());
            }
            let mut budget = BlobAdmission::new(block.clone());
            let mut entries = Vec::<Vec<u8>>::new();
            let mut groups = Vec::<u16>::new();
            let mut messages = Vec::new();
            for (i, size) in [0, 55, 56, 128, 255, 256, 16_384, 126_976]
                .into_iter()
                .enumerate()
            {
                let tx = signed(size, i % 2 == 0);
                let raw = Bytes::from(tx.encoded_2718());
                block.body.transactions.push(tx.into());
                let entry = vec![0u8; size];
                let group = 1u16;
                let group_messages = vec![
                    eez_blob_codec::Message::InitiateCrossChainTransaction {
                        chain_id: 1,
                        tx_data: raw.clone(),
                    },
                    eez_blob_codec::Message::FinishCrossChainTransaction,
                ];
                let semantic =
                    eez_blob_codec::encode_native_stream(1, Bytes::new(), group_messages.clone())
                        .unwrap()
                        .blob_portion
                        .len()
                        - eez_blob_codec::native_stream_len(0, 0).unwrap();
                budget = budget
                    .extend(
                        &[raw],
                        DaOverhead {
                            entry_items: entry.length(),
                            group_items: group.length(),
                            semantic_bytes: semantic,
                        },
                        None,
                    )
                    .unwrap();
                entries.push(entry);
                groups.push(group);
                messages.extend(group_messages);
                let payload = eez_payload_codec::derivable::Payload::new(
                    0,
                    eez_payload_codec::derivable::Environment::unused(),
                    vec![],
                    alloy_rlp::encode(&block),
                    entries.clone(),
                    groups.clone(),
                )
                .unwrap()
                .encode()
                .unwrap();
                let encoded =
                    eez_blob_codec::encode_native_stream(1, payload.into(), messages.clone())
                        .unwrap();
                let estimated = budget.stream_bytes().unwrap();
                let actual = encoded.blob_portion.len();
                assert!(estimated >= actual, "estimate {estimated}, actual {actual}");
                assert!(
                    estimated - actual < 600,
                    "only the unknown header and prefixes may be rounded up"
                );
                let required = estimated.div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB);
                assert!(budget.fits(required).unwrap());
                assert!(!budget.fits(required - 1).unwrap());
            }
            assert!(budget.stream_bytes().unwrap() > 2 * eez_blob_codec::DATA_BYTES_PER_BLOB);
        }
    }

    #[test]
    fn refused_blob_candidate_does_not_spend_budget_and_companion_blobs_are_counted() {
        let budget = BlobAdmission::new(Default::default());
        let initial = budget.stream_bytes().unwrap();
        let raw = Bytes::from(signed(2 * eez_blob_codec::DATA_BYTES_PER_BLOB, true).encoded_2718());
        let next = budget.extend(&[raw], DaOverhead::default(), None).unwrap();
        assert!(!next.fits(2).unwrap());
        assert_eq!(budget.stream_bytes().unwrap(), initial);
        let companion = TxEip4844 {
            blob_versioned_hashes: vec![B256::ZERO; 2],
            ..Default::default()
        }
        .into_signed(Signature::new(U256::from(1), U256::from(2), false));
        let raw = Bytes::from(TxEnvelope::from(companion).encoded_2718());
        let next = budget
            .extend(&[], DaOverhead::default(), Some(&raw))
            .unwrap();
        assert_eq!(next.companion_blobs, 2);
        assert_eq!(next.stream_bytes().unwrap(), initial);
        assert!(
            budget
                .extend(&[Bytes::from_static(&[0xff])], DaOverhead::default(), None)
                .is_err()
        );
    }
    #[test]
    fn source_only_deferral_preserves_capacity_but_resource_and_transport_failures_back_off() {
        let mut state = AutomaticAdmission {
            sample: Some(sample(64, 100)),
            retry_limit: None,
        };
        let a = attempt(64, 0, false, None);
        a.source_deferred(true);
        a.completed();
        state.finish(&a);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1000);
        let timed_out = attempt(64, 0, true, None);
        timed_out.source_deferred(true);
        timed_out.completed();
        state.finish(&timed_out);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1);
        state.retry_limit = None;
        let transport = attempt(64, 0, false, None);
        transport.source_deferred(true);
        state.finish(&transport);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 32);
    }
}
