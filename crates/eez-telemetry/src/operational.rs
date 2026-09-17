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

/// An actual native terminal transition. Reorg reopening may later produce
/// another observation for the same admission: this is not canonical accounting.
#[derive(Debug)]
pub struct TerminalObservation {
    pub direction: crate::schema::Direction,
    pub outcome: crate::schema::Outcome,
    pub elapsed: Option<Duration>,
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
        if let Some(elapsed) = self.elapsed {
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
                }
                .publish();
            }
            TerminalObservation {
                direction: Direction::Outbound,
                outcome: Outcome::SourceReverted,
                elapsed: None,
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
