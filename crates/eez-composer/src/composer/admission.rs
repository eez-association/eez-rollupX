//! Automatic FIFO selection is a preparation estimate, never a protocol limit.
//! Gas and encoded DA are still checked before accepting each complete group,
//! and the final encoded transaction/bundle remains the authoritative check.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use alloy_eips::Decodable2718;
use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use eez_driver::RollupTiming;
use eez_l1::BundleTarget;

use crate::{
    prover_retry::{PipelineDeadline, PipelineTimings, Workload, unix_time_millis},
    settlement_policy::index::DaOverhead,
};

use super::SUBMISSION_BACKOFF_THRESHOLD;
// The fifth charged exclusion must leave room for its reduced retry. Prover
// rejection keeps its existing, smaller episode bound in composer.rs.
pub(super) const MAX_AUTOMATIC_SETTLEMENT_ATTEMPTS: u32 = SUBMISSION_BACKOFF_THRESHOLD + 1;

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
    retry_limit: Option<SelectionLimit>,
    pub submission_failures: u32,
    timings: Arc<PipelineTimings>,
}

#[derive(Clone, Copy, Debug)]
struct SelectionLimit {
    count: usize,
    reason: &'static str,
}

impl SelectionLimit {
    fn tighter(self, other: Self) -> Self {
        if self.count <= other.count {
            self
        } else {
            other
        }
    }
}

impl AutomaticAdmission {
    /// Only charged, canonically confirmed incomplete settlement trains this
    /// counter. An isolated omission does not establish a capacity problem.
    /// Keep the streak across preparation/dispatch, resetting on canonical
    /// completion. Source retry ownership and its cumulative budget are separate.
    pub fn submission_failed(&mut self, submitted: usize) -> Option<usize> {
        if submitted == 0 {
            return None;
        }
        self.submission_failures = self.submission_failures.saturating_add(1);
        if self.submission_failures < SUBMISSION_BACKOFF_THRESHOLD {
            return None;
        }
        let limit = SelectionLimit {
            count: (submitted / 2).max(1),
            reason: "submission_backoff",
        };
        let limit = self.retry_limit.map_or(limit, |old| old.tighter(limit));
        self.retry_limit = Some(limit);
        Some(limit.count)
    }

    pub fn submission_completed(&mut self) {
        self.submission_failures = 0;
        if self
            .retry_limit
            .is_some_and(|limit| limit.reason == "submission_backoff")
        {
            self.retry_limit = None;
        }
    }

    fn limit(&self, queued: usize, available: Duration) -> usize {
        if available.is_zero() {
            return 0;
        }
        // The first candidate uses the same finite queue and resource checks.
        // Bootstrap finishing reserves protect the first proof handoff without
        // spending an L1 posting solely to measure a single group.
        let estimated = queued;
        estimated
            .min(self.retry_limit.map_or(usize::MAX, |limit| limit.count))
            .min(queued)
    }

    pub fn begin(&self, queued: usize, timing: RollupTiming, target: BundleTarget) -> Attempt {
        let deadline = PipelineDeadline::new(timing, target, unix_time_millis());
        let available = deadline.map_or(Duration::ZERO, PipelineDeadline::remaining);
        Attempt {
            started: Instant::now(),
            available,
            selected: self.limit(queued, available),
            selection_limit_reason: self.retry_limit.map(|limit| limit.reason),
            submission_failures: self.submission_failures,
            previous: self.sample,
            deadline,
            timings: Arc::clone(&self.timings),
            proof_estimate: timing.proof_time(),
            post_reserve: self.timings.post_reserve(Workload {
                groups: queued,
                blobs: usize::MAX,
                witnesses: usize::MAX,
            }),
            witnesses: std::sync::atomic::AtomicUsize::new(usize::MAX),
            previous_candidate: Mutex::default(),
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
        self.retry_limit = if progress.window_exhausted {
            let admitted = progress.prepared.map_or(progress.accepted, |s| s.groups);
            let workload = if admitted == 0 {
                attempt.selected
            } else {
                admitted.min(attempt.selected)
            };
            Some(SelectionLimit {
                count: (workload / 2).max(1),
                reason: "pipeline_deadline_backoff",
            })
        } else if progress.completed {
            None
        } else {
            // Source/provider/prover uncertainty does not measure capacity.
            self.retry_limit
        };
        // A completed attempt without a preparation observation, or a stale
        // pipeline estimate that prevented work, still needs a bounded probe.
        // A later infrastructure abort after a capacity miss supplies no usable
        // sample and must preserve the prior limit.
        // Bypassing an older independent group does not measure capacity: FIFO
        // is a preference, and the next snapshot uses its resource/time budgets.
        if (progress.completed || progress.estimate_blocked)
            && progress.limited
            && progress.prepared.is_none()
        {
            let probe_limit = SelectionLimit {
                count: progress.accepted.max(1).min((attempt.selected / 2).max(1)),
                reason: "preparation_probe_limit",
            };
            self.retry_limit = Some(
                self.retry_limit
                    .map_or(probe_limit, |limit| limit.tighter(probe_limit)),
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
    window_exhausted: bool,
    cold_timing_probe: bool,
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
    pub selection_limit_reason: Option<&'static str>,
    pub submission_failures: u32,
    previous: Option<Sample>,
    pub deadline: Option<PipelineDeadline>,
    pub timings: Arc<PipelineTimings>,
    proof_estimate: Duration,
    pub post_reserve: Duration,
    witnesses: std::sync::atomic::AtomicUsize,
    pub previous_candidate: Mutex<Option<String>>,
    progress: Mutex<Progress>,
}

impl Attempt {
    /// A resource cut can precede a count cap. Exhausting the snapshot says
    /// nothing about sources that arrived after it was taken.
    pub fn stop_reason(
        &self,
        resource: Option<&'static str>,
        accepted: usize,
        snapshot: usize,
    ) -> &'static str {
        if let Some(reason) = resource {
            reason
        } else if accepted >= self.selected && self.selected < snapshot {
            self.selection_limit_reason.unwrap_or("selection_limit")
        } else if accepted == snapshot {
            "snapshot_exhausted"
        } else {
            "eligible_sources_exhausted"
        }
    }

    pub fn window_exhausted(&self) {
        self.progress.lock().unwrap().window_exhausted = true;
    }

    fn bootstrap_tail(&self, blobs: usize, elapsed: Duration) -> u128 {
        // Conservative cold bootstrap allowance, verified by the native
        // cold-process suite before rollout. Non-KZG work also tracks this admission
        // pass, protecting larger/complex candidates before calibration.
        elapsed
            .max(Duration::from_millis(100))
            .as_nanos()
            .saturating_add(
                Duration::from_millis(250)
                    .as_nanos()
                    .saturating_mul(blobs as u128),
            )
    }

    pub fn witness_count(&self, witnesses: usize) {
        self.witnesses
            .store(witnesses, std::sync::atomic::Ordering::Relaxed);
    }
    fn pipeline_reserve(&self, groups: usize, blobs: usize) -> u128 {
        let workload = Workload {
            groups,
            blobs,
            witnesses: self.witnesses.load(std::sync::atomic::Ordering::Relaxed),
        };
        self.timings
            .proof_estimate(self.proof_estimate, workload)
            .saturating_add(if self.deadline.is_some() {
                self.timings.post_reserve(workload)
            } else {
                self.post_reserve
            })
            .as_nanos()
    }

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

    /// Check the shared deadline before a cheap capacity test. A refusal must
    /// still invalidate a stale estimate that prevented all work, while a cheap
    /// capacity miss must not start an unmeasured execution timing sample.
    pub fn can_check_group(&self, accepted: usize) -> bool {
        let fits = self.next_group_fits(accepted, self.started.elapsed());
        if !fits && accepted == 0 && self.previous.is_some() {
            self.progress.lock().unwrap().estimate_blocked = true;
        }
        fits
    }

    /// Reserve finishing time when checking deferred sources, without recording
    /// an execution attempt or contaminating accepted-group timing samples.
    pub fn can_check_source(&self, accepted: usize) -> bool {
        self.next_group_fits(accepted, self.started.elapsed())
    }

    fn next_group_fits(&self, accepted: usize, elapsed: Duration) -> bool {
        self.group_start_decision(accepted, elapsed).0
    }

    /// An unmeasured proof allocation must not prevent the first observation
    /// forever. Probe at most one complete group when the ordinary estimate
    /// refuses it, retaining finishing/finalization reserves and the original
    /// proof deadline. Wider windows still use normal multi-group admission.
    fn cold_probe_fits(&self, groups: usize, blobs: usize, elapsed: Duration) -> bool {
        if self.deadline.is_none() || self.previous.is_some() || groups != 1 {
            return false;
        }
        let post = self.timings.post_reserve(Workload {
            groups,
            blobs,
            witnesses: self.witnesses.load(std::sync::atomic::Ordering::Relaxed),
        });
        self.available.saturating_sub(elapsed).as_nanos()
            > self
                .bootstrap_tail(blobs, elapsed)
                .saturating_add(post.saturating_add(Duration::from_millis(25)).as_nanos())
    }

    pub fn used_cold_timing_probe(&self) -> bool {
        self.progress.lock().unwrap().cold_timing_probe
    }

    fn group_start_decision(&self, accepted: usize, elapsed: Duration) -> (bool, bool) {
        let remaining = self.available.saturating_sub(elapsed);
        if remaining.is_zero() {
            return (false, false);
        }
        let next = accepted.saturating_add(1);
        let blobs = self.progress.lock().unwrap().blobs.max(1);
        let jitter = self.jitter_reserve();
        let tail = self
            .previous
            .map_or(self.bootstrap_tail(blobs, elapsed), |s| {
                s.tail_for(next, blobs, elapsed, jitter)
            });
        let group = if accepted == 0 {
            self.previous.map_or(0, |s| {
                s.preparation.saturating_sub(s.tail).as_nanos() / s.groups.max(1) as u128
            })
        } else {
            elapsed.as_nanos() / accepted as u128
        };
        let estimated = remaining.as_nanos()
            > group
                .saturating_add(tail)
                .saturating_add(self.pipeline_reserve(next, blobs));
        let probe = !estimated && self.cold_probe_fits(next, blobs, elapsed);
        (estimated || probe, probe)
    }

    fn can_start_at(&self, accepted: usize, elapsed: Duration) -> bool {
        let (fits, probe) = self.group_start_decision(accepted, elapsed);
        let next = accepted.saturating_add(1);
        let mut progress = self.progress.lock().unwrap();
        progress.cold_timing_probe |= probe;
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
        let tail = self
            .previous
            .map_or(self.bootstrap_tail(blobs, elapsed), |s| {
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
        let estimated =
            remaining.as_nanos() > tail.saturating_add(self.pipeline_reserve(groups, blobs));
        let probe = !estimated && self.cold_probe_fits(groups, blobs, elapsed);
        progress.cold_timing_probe |= probe;
        if !estimated && !probe {
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
    transaction_items: usize,
    transaction_count: usize,
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
        // bound the full block byte string. The remaining header fields are fixed by
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
            transaction_items: 0,
            transaction_count: 0,
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
        next.transaction_count = next
            .transaction_count
            .checked_add(txs.len())
            .ok_or("admission transaction count overflow")?;
        for raw in txs {
            let tx = alloy_consensus::TxEnvelope::decode_2718(&mut raw.as_ref())
                .map_err(|e| format!("admission transaction decode: {e}"))?;
            next.transaction_items = next
                .transaction_items
                .checked_add(tx.length())
                .ok_or("admission transaction size overflow")?;
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
        // One list prefix for the block RLP, one byte-string prefix for its
        // operations field. Both prefixes have the same length at this size.
        let block_item = list_len(list_len(block_payload)?)?;
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
        // Count the complete group, including generated system transactions.
        // A refusal uses the same whole-group deferral as a byte-cap refusal.
        Ok(
            self.transaction_count <= eez_payload_codec::derivable::MAX_TRANSACTIONS
                && self.required_blobs()? <= blobs,
        )
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

    #[test]
    fn terminal_count_limit_counts_extensions_without_mutating_the_previous_budget() {
        let raw = Bytes::from(signed(0, true).encoded_2718());
        let mut budget = BlobAdmission::new(Default::default());
        budget.transaction_count = eez_payload_codec::derivable::MAX_TRANSACTIONS - 1;
        let fitting = budget
            .extend(std::slice::from_ref(&raw), DaOverhead::default(), None)
            .unwrap();
        assert_eq!(
            fitting.transaction_count,
            eez_payload_codec::derivable::MAX_TRANSACTIONS
        );
        assert!(fitting.fits(8).unwrap());
        let oversized = budget
            .extend(&[raw.clone(), raw.clone()], DaOverhead::default(), None)
            .unwrap();
        assert!(!oversized.fits(8).unwrap());
        assert_eq!(
            budget.transaction_count,
            eez_payload_codec::derivable::MAX_TRANSACTIONS - 1
        );
        // Valid bytes ensure an unrelated decode error cannot satisfy this check.
        budget.transaction_count = usize::MAX;
        assert_eq!(
            budget
                .extend(&[raw], DaOverhead::default(), None)
                .unwrap_err(),
            "admission transaction count overflow"
        );
    }

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
            selection_limit_reason: None,
            submission_failures: 0,
            previous: prepared,
            deadline: None,
            timings: Arc::default(),
            proof_estimate: Duration::ZERO,
            post_reserve: Duration::ZERO,
            witnesses: std::sync::atomic::AtomicUsize::new(usize::MAX),
            previous_candidate: Mutex::default(),
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
        for _ in 0..3 {
            assert!(attempt.can_check_group(0));
        }
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
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1000);
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
            1000
        );
    }

    #[test]
    fn confirmed_failures_wait_for_fifth_before_backoff() {
        let mut state = AutomaticAdmission::default();
        for failure in 1..=5 {
            let _ = state.submission_failed(80);
            assert_eq!(
                state.limit(100, Duration::from_secs(1)),
                if failure < 5 { 100 } else { 40 },
                "confirmed failure {failure}"
            );
            if failure < 5 {
                // A complete preparation does not establish canonical success.
                state.finish(&attempt(100, 80, false, Some(sample(80, 100))));
            }
        }
    }

    #[test]
    fn canonical_success_resets_streak_but_not_deadline_limits() {
        let mut state = AutomaticAdmission::default();
        assert_eq!(state.submission_failed(0), None);
        assert_eq!(state.submission_failures, 0);
        for _ in 0..4 {
            assert_eq!(state.submission_failed(80), None);
        }
        state.submission_completed();
        assert_eq!(state.submission_failures, 0);
        for _ in 0..4 {
            assert_eq!(state.submission_failed(80), None);
        }
        assert_eq!(state.submission_failed(80), Some(40));
        assert_eq!(state.submission_failed(40), Some(20));
        state.submission_completed();
        assert_eq!(state.limit(100, Duration::from_secs(1)), 100);
        let late = attempt(100, 30, false, Some(sample(30, 100)));
        late.window_exhausted();
        state.finish(&late);
        state.submission_completed();
        assert_eq!(state.limit(100, Duration::from_secs(1)), 15);
        assert_eq!(
            state.retry_limit.unwrap().reason,
            "pipeline_deadline_backoff"
        );
        state.submission_failures = u32::MAX;
        assert_eq!(state.submission_failed(10), Some(5));
        assert_eq!(state.submission_failures, u32::MAX);
    }

    #[test]
    fn capacity_miss_followed_by_infrastructure_abort_does_not_train_a_probe_limit() {
        let mut state = AutomaticAdmission::default();
        let a = attempt(100, 3, true, None);
        a.progress.lock().unwrap().completed = false;
        state.finish(&a);
        assert_eq!(state.limit(100, Duration::from_secs(1)), 100);
        for _ in 0..SUBMISSION_BACKOFF_THRESHOLD {
            let _ = state.submission_failed(80);
        }
        state.finish(&a);
        assert_eq!(state.limit(100, Duration::from_secs(1)), 40);
        assert_eq!(state.retry_limit.unwrap().reason, "submission_backoff");
    }

    #[test]
    fn fifo_bypass_does_not_cap_the_next_fitting_snapshot() {
        let mut state = AutomaticAdmission::default();
        let previous = attempt(100, 30, true, Some(sample(30, 100)));
        previous.progress.lock().unwrap().fifo_bypassed = true;
        state.finish(&previous);
        assert_eq!(state.limit(100, Duration::from_secs(1)), 100);
        assert!(state.retry_limit.is_none());
    }

    #[test]
    fn selection_causes_survive_tighter_limits_and_resource_cuts_take_precedence() {
        let mut state = AutomaticAdmission::default();
        let fair = attempt(100, 30, true, Some(sample(30, 100)));
        fair.progress.lock().unwrap().fifo_bypassed = true;
        state.finish(&fair);
        assert!(state.retry_limit.is_none());
        for _ in 0..SUBMISSION_BACKOFF_THRESHOLD {
            let _ = state.submission_failed(80);
        }
        assert_eq!(state.retry_limit.unwrap().count, 40);
        assert_eq!(state.retry_limit.unwrap().reason, "submission_backoff");
        for _ in 0..SUBMISSION_BACKOFF_THRESHOLD {
            let _ = state.submission_failed(40);
        }
        assert_eq!(state.retry_limit.unwrap().reason, "submission_backoff");
        let mut a = attempt(20, 20, false, Some(sample(20, 100)));
        a.selection_limit_reason = Some(state.retry_limit.unwrap().reason);
        assert_eq!(a.stop_reason(None, 20, 100), "submission_backoff");
        assert_eq!(
            a.stop_reason(Some("l1_tx_gas_budget"), 10, 100),
            "l1_tx_gas_budget"
        );
        assert_eq!(a.stop_reason(None, 20, 20), "snapshot_exhausted");
        assert_eq!(a.stop_reason(None, 10, 100), "eligible_sources_exhausted");
        a.window_exhausted();
        state.finish(&a);
        assert_eq!(
            state.retry_limit.unwrap().reason,
            "pipeline_deadline_backoff"
        );
        state.finish(&attempt(10, 10, false, Some(sample(10, 100))));
        assert!(state.retry_limit.is_none());
    }

    #[test]
    fn cold_proof_seed_cannot_permanently_block_the_first_observation() {
        let mut a = attempt(30, 0, false, None);
        a.available = Duration::from_millis(1490);
        a.proof_estimate = Duration::from_millis(1000);
        a.deadline = PipelineDeadline::new(
            RollupTiming::new(5000, 1000, 1000, 1500),
            BundleTarget::Exact {
                block: 1,
                timestamp: unix_time_millis() / 1000 + 5,
            },
            unix_time_millis(),
        );
        assert!(a.can_start_at(0, Duration::ZERO));
        assert!(a.can_finish_at(1, 1, Duration::from_millis(10)));
        assert!(a.used_cold_timing_probe());
        assert!(!a.can_start_at(1, Duration::from_millis(10)));
        assert!(!a.can_finish_at(1, 5, Duration::from_millis(10)));
        assert!(!a.can_finish_at(1, 1, a.available));
        a.available = Duration::from_millis(525);
        assert!(!a.can_start_at(0, Duration::ZERO));
        // A roomier cold window admits more than one without a probe.
        a.available = Duration::from_secs(3);
        a.progress.lock().unwrap().cold_timing_probe = false;
        assert!(a.can_start_at(1, Duration::from_millis(10)));
        assert!(!a.used_cold_timing_probe());
    }

    #[test]
    fn repeated_capacity_cuts_do_not_install_a_fifo_count_limit() {
        let mut state = AutomaticAdmission::default();
        for accepted in [63, 30, 10, 1] {
            let a = attempt(64, accepted, true, Some(sample(accepted, 100)));
            a.progress.lock().unwrap().fifo_bypassed = true;
            state.finish(&a);
            assert_eq!(state.limit(1000, Duration::from_secs(1)), 1000);
        }
    }

    #[test]
    fn successful_fifo_prefix_cut_does_not_cap_the_next_opportunity() {
        let mut state = AutomaticAdmission::default();
        state.finish(&attempt(30, 10, true, Some(sample(10, 250))));
        assert_eq!(state.limit(20, Duration::from_millis(500)), 20);
        // A failed submission still backs off, even after fair local progress.
        for _ in 0..SUBMISSION_BACKOFF_THRESHOLD {
            let _ = state.submission_failed(10);
        }
        assert_eq!(state.limit(20, Duration::from_millis(500)), 5);
    }

    #[test]
    fn nominal_preparation_overrun_and_infrastructure_uncertainty_do_not_reduce_capacity() {
        let mut state = AutomaticAdmission::default();
        // A prepared candidate may exceed the nominal allocation and still prove
        // before the shared deadline. The overrun alone is not a capacity failure.
        state.finish(&attempt(64, 64, false, Some(sample(64, 1200))));
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1000);
        let failed_rpc = attempt(64, 64, false, Some(sample(64, 100)));
        failed_rpc.progress.lock().unwrap().completed = false;
        state.finish(&failed_rpc);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1000);
        failed_rpc.window_exhausted();
        state.finish(&failed_rpc);
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 32);
        for _ in 0..SUBMISSION_BACKOFF_THRESHOLD {
            let _ = state.submission_failed(32);
        }
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 16);
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
                        submission_failures: 0,
                        timings: Arc::default(),
                    };
                    let mut failed = attempt(selected, 60, true, prepared);
                    failed.available = Duration::from_millis(491);
                    failed.window_exhausted();
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
        failed.window_exhausted();
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
            submission_failures: 0,
            timings: Arc::default(),
        };
        let mut blocked = attempt(state.limit(100, Duration::from_millis(500)), 0, true, None);
        blocked.previous = state.sample;
        blocked.available = Duration::from_millis(500);
        assert!(!blocked.can_check_group(0));
        assert_eq!(blocked.progress.lock().unwrap().started_groups, 0);
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
                            entry_items: entry.as_slice().length(),
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
    fn source_and_transport_uncertainty_preserve_capacity_but_zero_progress_resource_cuts_back_off()
    {
        let mut state = AutomaticAdmission {
            sample: Some(sample(64, 100)),
            retry_limit: None,
            submission_failures: 0,
            timings: Arc::default(),
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
        assert_eq!(state.limit(1000, Duration::from_secs(1)), 1000);
    }
}
