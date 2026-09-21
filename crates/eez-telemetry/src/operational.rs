//! Process-local observations, separate from canonical observer accounting.
//! Recorder calls happen after protocol owners have released their locks.
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::schema::PreparationOutcome;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Entrypoints enable instrumentation after validating the optional metrics config.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Fixed current-population buckets. These are gauges, not cumulative observations
/// over completed requests: query them directly, never through `rate()`.
pub const AGE_BUCKETS: &[(f64, &str)] = &[
    (0.1, "0.1"),
    (1., "1"),
    (5., "5"),
    (10., "10"),
    (30., "30"),
    (60., "60"),
    (120., "120"),
    (300., "300"),
    (600., "600"),
    (1800., "1800"),
    (3600., "3600"),
    (21600., "21600"),
    (86400., "86400"),
    (f64::INFINITY, "+Inf"),
];
pub const MAX_QUEUE_SAMPLE: usize = 4096;
pub const EVALUATION_MAX_AGE: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub struct Ages {
    pub count: usize,
    pub unknown: usize,
    pub oldest: f64,
    pub sum: f64,
    pub buckets: [usize; 14],
}
impl Ages {
    pub fn add(&mut self, age: Option<Duration>) {
        let Some(age) = age else {
            self.unknown += 1;
            return;
        };
        let age = age.as_secs_f64();
        self.count += 1;
        self.oldest = self.oldest.max(age);
        self.sum += age;
        for (index, (bound, _)) in AGE_BUCKETS.iter().enumerate() {
            if age <= *bound {
                self.buckets[index] += 1;
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct QueueDirection {
    /// queued_ready, queued_blocked, queued_unknown, in_flight, respectively.
    pub states: [usize; 4],
    pub pending: Ages,
    pub serviceable: Ages,
    pub readiness: [Ages; 3],
    pub readiness_clock_unknown: usize,
}
#[derive(Debug, Default)]
pub struct QueueSnapshot {
    pub complete: bool,
    pub evaluation_age: Option<Duration>,
    /// Inbound, outbound; identity values never enter this fixed cardinality.
    pub directions: [QueueDirection; 2],
}
impl QueueSnapshot {
    pub fn evaluation_valid(&self) -> bool {
        self.complete
            && self
                .evaluation_age
                .is_some_and(|age| age <= EVALUATION_MAX_AGE)
    }
    /// Publish a fully aggregated snapshot after releasing the pool lock. Invalid
    /// samples overwrite earlier values with NaN rather than leaving stale zeros.
    pub fn publish(&self) {
        // The producer bounds populations at MAX_QUEUE_SAMPLE. A malformed
        // oversized snapshot must remain unknown instead of rounding its count.
        let bounded_count = |count: usize| u32::try_from(count).map_or(f64::NAN, f64::from);
        metrics::gauge!("eez_native_queue_sample_valid").set(f64::from(u8::from(self.complete)));
        metrics::gauge!("eez_native_queue_evaluation_valid")
            .set(f64::from(u8::from(self.evaluation_valid())));
        metrics::gauge!("eez_native_queue_evaluation_age_seconds").set(
            self.evaluation_age
                .map_or(f64::NAN, |age| age.as_secs_f64()),
        );
        metrics::gauge!("eez_native_queue_sample_timestamp_seconds").set(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(f64::NAN, |age| age.as_secs_f64()),
        );
        for (name, direction) in ["inbound", "outbound"].into_iter().zip(&self.directions) {
            for (state, count) in [
                "queued_ready",
                "queued_blocked",
                "queued_unknown",
                "in_flight",
            ]
            .into_iter()
            .zip(direction.states)
            {
                metrics::gauge!("eez_native_queue_transactions", "direction" => name, "state" => state).set(if self.complete { bounded_count(count) } else { f64::NAN });
            }
            metrics::gauge!("eez_native_queue_readiness_clock_unknown_transactions", "direction" => name).set(if self.complete { bounded_count(direction.readiness_clock_unknown) } else { f64::NAN });
            for (kind, durations) in READINESS_KINDS.into_iter().zip(&direction.readiness) {
                let value = |n: f64| if self.complete { n } else { f64::NAN };
                metrics::gauge!("eez_native_queue_readiness_duration_count", "direction" => name, "kind" => kind).set(value(bounded_count(durations.count)));
                metrics::gauge!("eez_native_queue_readiness_duration_unknown_transactions", "direction" => name, "kind" => kind).set(value(bounded_count(durations.unknown)));
                metrics::gauge!("eez_native_queue_readiness_duration_sum_seconds", "direction" => name, "kind" => kind).set(value(durations.sum));
                for ((_, bound), count) in AGE_BUCKETS.iter().zip(durations.buckets) {
                    metrics::gauge!("eez_native_queue_readiness_duration_seconds_bucket", "direction" => name, "kind" => kind, "le" => *bound).set(value(bounded_count(count)));
                }
            }
            for (kind, ages, valid) in [
                ("pending", &direction.pending, self.complete),
                (
                    "serviceable",
                    &direction.serviceable,
                    self.evaluation_valid(),
                ),
            ] {
                let value = |n: f64| if valid { n } else { f64::NAN };
                metrics::gauge!("eez_native_queue_age_count", "direction" => name, "kind" => kind)
                    .set(value(bounded_count(ages.count)));
                metrics::gauge!("eez_native_queue_age_unknown_transactions", "direction" => name, "kind" => kind).set(value(bounded_count(ages.unknown)));
                metrics::gauge!("eez_native_queue_age_sum_seconds", "direction" => name, "kind" => kind).set(value(ages.sum));
                metrics::gauge!("eez_native_queue_oldest_age_seconds", "direction" => name, "kind" => kind).set(if valid && ages.count != 0 { ages.oldest } else { f64::NAN });
                for ((_, bound), count) in AGE_BUCKETS.iter().zip(ages.buckets) {
                    metrics::gauge!("eez_native_queue_age_seconds_bucket", "direction" => name, "kind" => kind, "le" => *bound).set(value(bounded_count(count)));
                }
            }
        }
    }
}

/// Closed producer boundaries; process identity identifies Composer versus signer.
#[derive(Debug, Clone, Copy)]
pub enum Stage {
    Simulation,
    Witness,
    Submission,
    Signer,
    ProverClient,
    SignerIngestion,
    SignerPreparation,
    SignerValidation,
    SignerSettlement,
    SignerSigning,
}
impl Stage {
    const fn label(self) -> &'static str {
        match self {
            Self::Simulation => "simulation",
            Self::Witness => "witness",
            Self::Submission => "submission",
            Self::Signer => "signer",
            Self::ProverClient => "prover_client",
            Self::SignerIngestion => "signer_ingestion",
            Self::SignerPreparation => "signer_preparation",
            Self::SignerValidation => "signer_validation",
            Self::SignerSettlement => "signer_settlement",
            Self::SignerSigning => "signer_signing",
        }
    }
}

/// Operational outcomes, never receipt finality or canonical commitment.
#[derive(Debug, Clone, Copy)]
pub enum StageOutcome {
    Success,
    Failed,
    Error,
    Invalid,
    Rejected,
    Unavailable,
    Deadline,
    Aborted,
    Cancelled,
    Panicked,
    Included,
    Excluded,
}
impl StageOutcome {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Error => "internal",
            Self::Invalid => "invalid",
            Self::Rejected => "rejected",
            Self::Unavailable => "unavailable",
            Self::Deadline => "deadline",
            Self::Aborted => "aborted",
            Self::Cancelled => "cancelled",
            Self::Panicked => "panicked",
            Self::Included => "included",
            Self::Excluded => "excluded",
        }
    }
}

/// Starts only when metrics are enabled. Keep blocking-stage guards inside the
/// worker: dropping the awaiting RPC does not finish detached execution.
#[derive(Debug)]
pub struct StageTimer {
    stage: Stage,
    started: Option<Instant>,
}
impl StageTimer {
    pub fn start(stage: Stage) -> Self {
        Self::with_enabled(stage, enabled())
    }
    fn with_enabled(stage: Stage, active: bool) -> Self {
        Self {
            stage,
            started: active.then(Instant::now),
        }
    }
    pub fn finish(&mut self, outcome: StageOutcome) {
        if let Some(started) = self.started.take() {
            metrics::histogram!("eez_stage_duration_seconds", "stage" => self.stage.label(), "outcome" => outcome.label()).record(started.elapsed().as_secs_f64());
            if matches!(self.stage, Stage::Signer) {
                metrics::counter!("eez_signer_requests_total", "outcome" => outcome.label())
                    .increment(1);
            }
        }
    }
    pub fn measure<T, E>(stage: Stage, work: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        let mut timer = Self::start(stage);
        let result = work();
        timer.finish(if result.is_ok() {
            StageOutcome::Success
        } else {
            StageOutcome::Failed
        });
        result
    }
}
impl Drop for StageTimer {
    fn drop(&mut self) {
        self.finish(if std::thread::panicking() {
            StageOutcome::Panicked
        } else {
            StageOutcome::Cancelled
        });
    }
}

/// Outcomes after the complete signer pipeline has authorized attestation.
/// Admission or execution-only success must not construct an observation.
#[derive(Debug, Clone, Copy)]
pub enum AuthorizedOutcome {
    Success,
    Error,
    Deadline,
    Panicked,
    Cancelled,
}
impl AuthorizedOutcome {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "internal",
            Self::Deadline => "deadline",
            Self::Panicked => "panicked",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Exactly one local response outcome for an already authorized request.
/// This guard neither owns the RPC permit nor changes its deadline.
#[derive(Debug)]
#[must_use]
pub struct AuthorizedRequest {
    active: bool,
}
impl AuthorizedRequest {
    pub fn start() -> Self {
        Self { active: enabled() }
    }
    pub fn finish(&mut self, outcome: AuthorizedOutcome) {
        if std::mem::replace(&mut self.active, false) {
            metrics::counter!("eez_signer_authorized_requests_total", "outcome" => outcome.label())
                .increment(1);
        }
    }
}
impl Drop for AuthorizedRequest {
    fn drop(&mut self) {
        self.finish(if std::thread::panicking() {
            AuthorizedOutcome::Panicked
        } else {
            AuthorizedOutcome::Cancelled
        });
    }
}

/// Register zero observations at metrics-enabled signer startup, so the first
/// rejection after a successful initial scrape is visible to counter rates.
pub fn initialize_signer_metrics() {
    for outcome in [
        AuthorizedOutcome::Success,
        AuthorizedOutcome::Error,
        AuthorizedOutcome::Deadline,
        AuthorizedOutcome::Panicked,
        AuthorizedOutcome::Cancelled,
    ] {
        metrics::counter!("eez_signer_authorized_requests_total", "outcome" => outcome.label())
            .increment(0);
    }
    for outcome in [
        StageOutcome::Success,
        StageOutcome::Invalid,
        StageOutcome::Rejected,
        StageOutcome::Unavailable,
        StageOutcome::Deadline,
        StageOutcome::Cancelled,
        StageOutcome::Error,
        StageOutcome::Panicked,
    ] {
        metrics::counter!("eez_signer_requests_total", "outcome" => outcome.label()).increment(0);
    }
}

/// One cut per actual finite planning decision, not one failed user request.
#[derive(Debug, Clone, Copy)]
pub enum AdmissionCut {
    Gas,
    Da,
    Count,
    PreparationDeadline,
    QueueCleanup,
}
pub fn admission_cut(reason: AdmissionCut) {
    if enabled() {
        let reason = match reason {
            AdmissionCut::Gas => "gas",
            AdmissionCut::Da => "da",
            AdmissionCut::Count => "count",
            AdmissionCut::PreparationDeadline => "preparation_deadline",
            AdmissionCut::QueueCleanup => "queue_cleanup",
        };
        metrics::counter!("eez_admission_cuts_total", "reason" => reason).increment(1);
    }
}

/// A scheduled slot or one submitted bundle missed its exact target. These are
/// separate stages of work, not unique admission failures.
#[derive(Debug, Clone, Copy)]
pub enum SlotMiss {
    ScheduledLate,
    Excluded,
    Skipped,
    SubmissionDeadline,
    TargetChanged,
}
pub fn slot_miss(reason: SlotMiss) {
    if enabled() {
        let reason = match reason {
            SlotMiss::ScheduledLate => "scheduled_late",
            SlotMiss::Excluded => "excluded",
            SlotMiss::Skipped => "skipped",
            SlotMiss::SubmissionDeadline => "submission_deadline",
            SlotMiss::TargetChanged => "target_changed",
        };
        metrics::counter!("eez_target_slot_misses_total", "reason" => reason).increment(1);
    }
}

/// Approximate nonfinancial operational magnitudes. Exact money stays in SQL;
/// full-width configuration counters can exceed binary64 integer precision.
#[allow(clippy::cast_precision_loss)]
fn magnitude(value: u64) -> f64 {
    value as f64
}

#[derive(Debug)]
pub struct PolicySettings {
    pub enabled: bool,
    pub pure_interval: Option<Duration>,
    pub max_unsettled_blocks: Option<u64>,
    pub fullness_bps: Option<u16>,
    pub blob_cap: Option<u64>,
    pub count_cap: Option<usize>,
    pub gas_cap: Option<u64>,
    pub proof_time: Duration,
    pub submission_slack: Duration,
    pub slot_time: Duration,
}
impl PolicySettings {
    pub fn publish(&self) {
        if !enabled() {
            return;
        }
        metrics::gauge!("eez_settlement_policy_enabled").set(f64::from(u8::from(self.enabled)));
        let mode = if !self.enabled {
            "disabled"
        } else if self.pure_interval.is_some() {
            "interval"
        } else {
            "always"
        };
        let admission = if self.count_cap.is_some() {
            "fixed"
        } else {
            "automatic"
        };
        metrics::gauge!("eez_settlement_policy_info", "pure_l2_mode" => mode, "admission" => admission).set(1.);
        for (setting, value) in [
            (
                "pure_l2_interval_seconds",
                self.pure_interval.map(|d| d.as_secs_f64()),
            ),
            (
                "max_unsettled_blocks",
                self.max_unsettled_blocks.map(magnitude),
            ),
            ("blob_fullness_bps", self.fullness_bps.map(f64::from)),
            ("operator_blob_cap", self.blob_cap.map(magnitude)),
            (
                "user_count_cap",
                self.count_cap
                    .and_then(|n| u64::try_from(n).ok())
                    .map(magnitude),
            ),
            ("operator_gas_cap", self.gas_cap.map(magnitude)),
            ("proof_seconds", Some(self.proof_time.as_secs_f64())),
            (
                "submission_slack_seconds",
                Some(self.submission_slack.as_secs_f64()),
            ),
            ("l1_slot_seconds", Some(self.slot_time.as_secs_f64())),
        ] {
            metrics::gauge!("eez_settlement_policy_setting", "setting" => setting)
                .set(value.unwrap_or(f64::NAN));
        }
    }
}

/// Rollup IDs come only from the finite configured runtime set, never requests.
/// Evaluation freshness is independent of scrape freshness. Age is measured
/// from the last canonical settlement anchor to the candidate target slot.
pub fn policy_evaluation(rollup_id: u64, observation: Option<(u64, u64, u64, bool, u64)>) {
    if !enabled() {
        return;
    }
    let rollup = rollup_id.to_string();
    metrics::gauge!("eez_settlement_policy_evaluation_valid", "rollup" => rollup.clone())
        .set(f64::from(u8::from(observation.is_some())));
    if let Some((unsettled, age_ms, pure_transactions, due, blob_allowance)) = observation {
        metrics::gauge!("eez_settlement_policy_evaluation_timestamp_seconds", "rollup" => rollup.clone()).set(SystemTime::now().duration_since(UNIX_EPOCH).map_or(f64::NAN, |d| d.as_secs_f64()));
        metrics::gauge!("eez_settlement_unsettled_blocks", "rollup" => rollup.clone())
            .set(magnitude(unsettled));
        metrics::gauge!("eez_settlement_anchor_age_seconds", "rollup" => rollup.clone())
            .set(Duration::from_millis(age_ms).as_secs_f64());
        metrics::gauge!("eez_settlement_pending_pure_l2_transactions", "rollup" => rollup.clone())
            .set(magnitude(pure_transactions));
        metrics::gauge!("eez_settlement_due", "rollup" => rollup.clone())
            .set(f64::from(u8::from(due)));
        metrics::gauge!("eez_settlement_blob_allowance", "rollup" => rollup)
            .set(magnitude(blob_allowance));
    }
}

/// Owned by the existing preparation future. It records once on finish or drop;
/// an unpolled future never constructs this guard. No lifecycle writer is needed.
#[derive(Debug)]
pub struct PreparationTimer {
    started: Instant,
    finished: bool,
}
impl PreparationTimer {
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
            finished: false,
        }
    }
    pub fn finish(&mut self, outcome: PreparationOutcome) {
        if self.finished {
            return;
        }
        self.finished = true;
        record_preparation(outcome, self.started.elapsed());
    }
}
impl Drop for PreparationTimer {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(if std::thread::panicking() {
                PreparationOutcome::Panicked
            } else {
                PreparationOutcome::Cancelled
            });
        }
    }
}
fn record_preparation(outcome: PreparationOutcome, elapsed: Duration) {
    let outcome = match outcome {
        PreparationOutcome::Prepared => "prepared",
        PreparationOutcome::ConstructionFailed => "construction_failed",
        PreparationOutcome::CapacityLimited => "capacity_limited",
        PreparationOutcome::ProverFailed => "prover_failed",
        PreparationOutcome::ValidationRejected => "validation_rejected",
        PreparationOutcome::Cancelled => "cancelled",
        PreparationOutcome::Panicked => "panicked",
    };
    metrics::histogram!("eez_stage_duration_seconds", "stage" => "preparation", "outcome" => outcome).record(elapsed.as_secs_f64());
}

pub const READINESS_KINDS: [&str; 3] = ["blocked", "serviceable", "unknown"];
/// Cumulative locally observed portions of the same admission interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessDurations {
    pub parts: [Duration; 3],
}
impl ReadinessDurations {
    pub fn conserves(self, elapsed: Duration) -> bool {
        self.parts
            .iter()
            .try_fold(Duration::ZERO, |sum, part| sum.checked_add(*part))
            == Some(elapsed)
    }
}

/// An actual native terminal transition. Reorg reopening may later produce
/// another observation for the same admission: this is not canonical accounting.
#[derive(Debug)]
pub struct TerminalObservation {
    pub direction: crate::schema::Direction,
    pub outcome: crate::schema::Outcome,
    pub elapsed: Option<Duration>,
    pub readiness: Option<ReadinessDurations>,
}
impl TerminalObservation {
    pub fn publish(self) {
        use crate::schema::{Direction, Outcome};
        let direction = match self.direction {
            Direction::Inbound => "inbound",
            Direction::Outbound => "outbound",
        };
        let outcome = match self.outcome {
            Outcome::Committed => "committed",
            Outcome::SourceReverted => "source_reverted",
            Outcome::SourceCompletedWithMissingEffects => "source_completed_with_missing_effects",
            Outcome::SourceCompleted => "source_completed",
            Outcome::SourceNonceSpent => "source_nonce_spent",
            Outcome::AttemptsExhausted => "attempts_exhausted",
            Outcome::NonceGap => "nonce_gap",
            Outcome::PoolDiscarded => "pool_discarded",
            Outcome::Replaced => "replaced",
        };
        metrics::counter!("eez_native_terminal_observations_total", "direction" => direction, "outcome" => outcome).increment(1);
        let readiness = self
            .elapsed
            .and_then(|elapsed| self.readiness.filter(|parts| parts.conserves(elapsed)));
        if readiness.is_none() {
            metrics::counter!("eez_native_terminal_readiness_clock_unknown_total", "direction" => direction, "outcome" => outcome).increment(1);
        }
        if let Some(elapsed) = self.elapsed {
            if let Some(readiness) = readiness {
                for (kind, duration) in READINESS_KINDS.into_iter().zip(readiness.parts) {
                    metrics::histogram!("eez_request_readiness_duration_seconds", "direction" => direction, "outcome" => outcome, "kind" => kind).record(duration.as_secs_f64());
                }
            } else {
                metrics::histogram!("eez_request_readiness_duration_seconds", "direction" => direction, "outcome" => outcome, "kind" => "unknown").record(elapsed.as_secs_f64());
            }

            metrics::histogram!("eez_request_duration_seconds", "direction" => direction, "outcome" => outcome).record(elapsed.as_secs_f64());
        } else {
            metrics::counter!("eez_native_terminal_duration_unknown_total", "direction" => direction, "outcome" => outcome).increment(1);
        }
    }
}
/// Closed reasons only; capture loss never alters protocol ownership.
pub fn terminal_capture_gap(failure: crate::CaptureFailure) {
    let reason = match failure {
        crate::CaptureFailure::Limit => "capture_limit",
        crate::CaptureFailure::MissingContext => "capture_context",
    };
    metrics::counter!("eez_native_terminal_capture_gaps_total", "reason" => reason).increment(1);
}

/// Successfully accepted admissions only; replacement is one new admission.
pub fn admitted(direction: crate::schema::Direction) {
    let direction = match direction {
        crate::schema::Direction::Inbound => "inbound",
        crate::schema::Direction::Outbound => "outbound",
    };
    metrics::counter!("eez_native_admissions_total", "direction" => direction).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "native")]
    #[test]
    fn authorized_response_guard_records_once_without_invalid_work_labels() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            initialize_signer_metrics();
            let mut disabled = AuthorizedRequest { active: false };
            disabled.finish(AuthorizedOutcome::Error);
            for outcome in [
                AuthorizedOutcome::Success,
                AuthorizedOutcome::Error,
                AuthorizedOutcome::Deadline,
            ] {
                let mut guard = AuthorizedRequest { active: true };
                guard.finish(outcome);
                guard.finish(AuthorizedOutcome::Error);
            }
            drop(AuthorizedRequest { active: true });
            let _ = std::panic::catch_unwind(|| {
                let _guard = AuthorizedRequest { active: true };
                panic!("controlled authorized-response unwind");
            });
        });
        let rendered = handle.render();
        for outcome in ["success", "internal", "deadline", "panicked", "cancelled"] {
            assert!(
                rendered.contains(&format!(
                    "eez_signer_authorized_requests_total{{outcome=\"{outcome}\"}} 1"
                )),
                "{rendered}"
            );
        }
        assert!(!rendered.contains("eez_signer_authorized_requests_total{outcome=\"invalid\"}"));
    }

    #[cfg(feature = "native")]
    #[test]
    fn stage_guards_record_once_and_preserve_cancellation_panic_and_disabled_paths() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let mut disabled = StageTimer::with_enabled(Stage::Submission, false);
            disabled.finish(StageOutcome::Success);
            let mut successful = StageTimer::with_enabled(Stage::Simulation, true);
            successful.finish(StageOutcome::Success);
            successful.finish(StageOutcome::Failed);
            drop(successful);
            drop(StageTimer::with_enabled(Stage::Witness, true));
            let _ = std::panic::catch_unwind(|| {
                let _guard = StageTimer::with_enabled(Stage::SignerValidation, true);
                panic!("controlled worker panic");
            });
            initialize_signer_metrics();
            let mut signer = StageTimer::with_enabled(Stage::Signer, true);
            signer.finish(StageOutcome::Rejected);
        });
        let rendered = handle.render();
        for (stage, outcome) in [
            ("simulation", "success"),
            ("witness", "cancelled"),
            ("signer_validation", "panicked"),
            ("signer", "rejected"),
        ] {
            assert!(
                rendered.contains(&format!(
                    "eez_stage_duration_seconds_count{{stage=\"{stage}\",outcome=\"{outcome}\"}} 1"
                )),
                "{rendered}"
            );
        }
        assert!(
            !rendered.contains("stage=\"submission\""),
            "disabled instrumentation must create no series"
        );
        assert!(
            !rendered.contains("outcome=\"failed\""),
            "finishing twice must not count twice"
        );
        assert!(rendered.contains("eez_signer_requests_total{outcome=\"rejected\"} 1"));
        assert!(rendered.contains("eez_signer_requests_total{outcome=\"invalid\"} 0"));
    }

    #[test]
    fn current_population_is_not_a_completion_histogram() {
        let mut ages = Ages::default();
        ages.add(Some(Duration::from_secs(60)));
        ages.add(Some(Duration::from_secs(120)));
        ages.add(None);
        assert_eq!(
            (ages.count, ages.unknown, ages.oldest, ages.sum),
            (2, 1, 120., 180.)
        );
        assert_eq!(ages.buckets[4], 0);
        assert_eq!(ages.buckets[5], 1);
        assert_eq!(ages.buckets[6], 2);
        assert_eq!(ages.buckets[13], 2);
    }
    #[cfg(feature = "native")]
    #[test]
    fn preparation_outcomes_are_finite_histograms_and_guard_finishes_once() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let mut timer = PreparationTimer::start();
            timer.finish(PreparationOutcome::Prepared);
            timer.finish(PreparationOutcome::ProverFailed);
            drop(timer);
            drop(PreparationTimer::start());
            let _ = std::panic::catch_unwind(|| {
                let _timer = PreparationTimer::start();
                panic!("controlled");
            });
            for outcome in [
                PreparationOutcome::ConstructionFailed,
                PreparationOutcome::CapacityLimited,
                PreparationOutcome::ProverFailed,
                PreparationOutcome::ValidationRejected,
            ] {
                record_preparation(outcome, Duration::from_secs(1));
            }
        });
        let rendered = handle.render();
        assert!(rendered.contains("# TYPE eez_stage_duration_seconds histogram"));
        for outcome in [
            "prepared",
            "construction_failed",
            "capacity_limited",
            "prover_failed",
            "validation_rejected",
            "cancelled",
            "panicked",
        ] {
            assert!(rendered.contains(&format!("eez_stage_duration_seconds_count{{stage=\"preparation\",outcome=\"{outcome}\"}} 1")), "{rendered}");
        }
    }
    #[cfg(feature = "native")]
    #[test]
    fn terminal_histograms_preserve_outcomes_and_unknown_duration_coverage() {
        use crate::schema::{Direction, Outcome};
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            for outcome in [
                Outcome::Committed,
                Outcome::SourceReverted,
                Outcome::SourceCompletedWithMissingEffects,
                Outcome::SourceCompleted,
                Outcome::SourceNonceSpent,
                Outcome::AttemptsExhausted,
                Outcome::NonceGap,
                Outcome::PoolDiscarded,
                Outcome::Replaced,
            ] {
                TerminalObservation {
                    direction: Direction::Inbound,
                    outcome,
                    elapsed: Some(Duration::from_secs(60)),
                    readiness: Some(ReadinessDurations {
                        parts: [
                            Duration::from_secs(10),
                            Duration::from_secs(40),
                            Duration::from_secs(10),
                        ],
                    }),
                }
                .publish();
            }
            TerminalObservation {
                direction: Direction::Outbound,
                outcome: Outcome::SourceReverted,
                elapsed: None,
                readiness: None,
            }
            .publish();
        });
        let rendered = handle.render();
        assert!(rendered.contains("# TYPE eez_request_duration_seconds histogram"));
        for outcome in [
            "committed",
            "source_reverted",
            "source_completed_with_missing_effects",
            "source_completed",
            "source_nonce_spent",
            "attempts_exhausted",
            "nonce_gap",
            "pool_discarded",
            "replaced",
        ] {
            assert!(rendered.contains(&format!("eez_request_duration_seconds_count{{direction=\"inbound\",outcome=\"{outcome}\"}} 1")), "{rendered}");
            for (kind, seconds) in [("blocked", 10), ("serviceable", 40), ("unknown", 10)] {
                assert!(rendered.contains(&format!("eez_request_readiness_duration_seconds_sum{{direction=\"inbound\",outcome=\"{outcome}\",kind=\"{kind}\"}} {seconds}")), "{rendered}");
            }
        }
        assert!(rendered.contains("eez_native_terminal_duration_unknown_total{direction=\"outbound\",outcome=\"source_reverted\"} 1"), "{rendered}");
        assert!(
            !rendered.contains("eez_request_duration_seconds_count{direction=\"outbound\""),
            "{rendered}"
        );
        assert!(!rendered.contains("canonical"));
    }

    #[cfg(feature = "native")]
    #[test]
    fn absent_or_inconsistent_partition_never_manufactures_zero_known_waits() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            for readiness in [
                None,
                Some(ReadinessDurations {
                    parts: [Duration::from_secs(1); 3],
                }),
            ] {
                TerminalObservation {
                    direction: crate::schema::Direction::Outbound,
                    outcome: crate::schema::Outcome::Committed,
                    elapsed: Some(Duration::from_secs(60)),
                    readiness,
                }
                .publish();
            }
        });
        let rendered = handle.render();
        assert!(rendered.contains("eez_native_terminal_readiness_clock_unknown_total{direction=\"outbound\",outcome=\"committed\"} 2"), "{rendered}");
        assert!(rendered.contains("eez_request_readiness_duration_seconds_sum{direction=\"outbound\",outcome=\"committed\",kind=\"unknown\"} 120"), "{rendered}");
        assert!(!rendered.contains("kind=\"blocked\""));
        assert!(!rendered.contains("kind=\"serviceable\""));
    }

    #[cfg(feature = "native")]
    #[test]
    fn missing_and_expired_queue_samples_overwrite_old_values() {
        let recorder = crate::native::prometheus_builder().build_recorder();
        let handle = recorder.handle();
        let mut snapshot = QueueSnapshot {
            complete: true,
            evaluation_age: Some(Duration::ZERO),
            ..Default::default()
        };
        snapshot.directions[0].states[0] = 1;
        snapshot.directions[0]
            .pending
            .add(Some(Duration::from_secs(60)));
        metrics::with_local_recorder(&recorder, || snapshot.publish());
        let rendered = handle.render();
        assert!(
            rendered.contains(
                "eez_native_queue_transactions{direction=\"inbound\",state=\"queued_ready\"} 1"
            ),
            "{rendered}"
        );
        snapshot.complete = false;
        metrics::with_local_recorder(&recorder, || snapshot.publish());
        let rendered = handle.render();
        assert!(
            rendered.contains(
                "eez_native_queue_transactions{direction=\"inbound\",state=\"queued_ready\"} NaN"
            ),
            "{rendered}"
        );
        snapshot.complete = true;
        snapshot.evaluation_age = Some(EVALUATION_MAX_AGE + Duration::from_nanos(1));
        assert!(!snapshot.evaluation_valid());
    }
}
