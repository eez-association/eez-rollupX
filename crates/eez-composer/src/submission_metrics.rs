//! Event measurements belong to the submission task, not the sampled unresolved
//! ledger. A receipt can arrive after reconciliation has already retired its
//! attempt. Labels are bounded; hashes/heights belong in structured logs.

use std::{
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use eez_l1::{BundleDropReason, SendOutcome};
use metrics::{Counter, Gauge, Unit};

const EVENTS: [&str; 6] = [
    "prepared",
    "dispatched",
    "included",
    "excluded",
    "uncertain",
    "preflight_failed",
];
const REASONS: [&str; 4] = [
    "excluded",
    "slot_skipped",
    "submission_window_missed",
    "target_changed",
];

pub(crate) struct SubmissionMetrics {
    rollup: String,
    started: Instant,
    dispatched: OnceLock<Instant>,
    events: [(Counter, Gauge); 6],
}

#[allow(clippy::cast_precision_loss)] // Prometheus stores counts/heights as f64.
impl SubmissionMetrics {
    /// One instance per prepared submission task. Preparation/proving before
    /// task creation is deliberately outside these timing measurements.
    pub(crate) fn new(rollup_id: u64) -> Self {
        let rollup = rollup_id.to_string();
        metrics::describe_counter!(
            "eez_composer_submission_events_total",
            Unit::Count,
            "Local submission events since process start; receipts are not canonical user completions"
        );
        metrics::describe_gauge!(
            "eez_composer_submission_event_timestamp_seconds",
            Unit::Seconds,
            "Latest local wall-clock observation of this event; zero before the first event"
        );
        metrics::describe_histogram!(
            "eez_composer_submission_duration_seconds",
            Unit::Seconds,
            "Monotonic elapsed time for submission stages; excludes proving before the prepared task"
        );
        metrics::describe_gauge!(
            "eez_composer_submission_last_duration_seconds",
            Unit::Seconds,
            "Latest measured submission-stage duration"
        );
        metrics::describe_counter!(
            "eez_composer_submission_exclusions_total",
            Unit::Count,
            "Conclusive submission exclusions by bounded submitter reason; not user eviction counts"
        );
        // Register zero baselines so the first event can be represented by
        // Prometheus increases even after a long idle period.
        let events = EVENTS.map(|event| {
            let count = metrics::counter!("eez_composer_submission_events_total", "rollup_id" => rollup.clone(), "event" => event);
            count.increment(0);
            let timestamp = metrics::gauge!("eez_composer_submission_event_timestamp_seconds", "rollup_id" => rollup.clone(), "event" => event);
            (count, timestamp)
        });
        for reason in REASONS {
            metrics::counter!("eez_composer_submission_exclusions_total", "rollup_id" => rollup.clone(), "reason" => reason).increment(0);
        }
        Self {
            rollup,
            started: Instant::now(),
            dispatched: OnceLock::new(),
            events,
        }
    }

    pub(crate) fn prepared(&self) {
        self.event(0);
    }

    fn event(&self, index: usize) {
        self.events[index].0.increment(1);
        if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
            self.events[index].1.set(now.as_secs_f64());
        }
    }

    fn duration(&self, stage: &'static str, elapsed: f64) {
        metrics::histogram!("eez_composer_submission_duration_seconds", "rollup_id" => self.rollup.clone(), "stage" => stage).record(elapsed);
        metrics::gauge!("eez_composer_submission_last_duration_seconds", "rollup_id" => self.rollup.clone(), "stage" => stage).set(elapsed);
    }

    pub(crate) fn dispatch(&self, block: u64, timestamp: u64) {
        let now = Instant::now();
        if self.dispatched.set(now).is_err() {
            return;
        }
        self.event(1);
        self.duration(
            "prepared_to_dispatch",
            now.duration_since(self.started).as_secs_f64(),
        );
        metrics::gauge!("eez_composer_submission_target_l1_block", "rollup_id" => self.rollup.clone()).set(block as f64);
        metrics::gauge!("eez_composer_submission_target_timestamp_seconds", "rollup_id" => self.rollup.clone()).set(timestamp as f64);
    }

    /// Called exactly once when the observer returns, even if canonical
    /// reconciliation won the race and the ledger rejects a late observation.
    pub(crate) fn outcome(&self, outcome: Option<&SendOutcome>) {
        let (index, stage) = match outcome {
            Some(SendOutcome::Included { l1_block, .. }) => {
                metrics::gauge!("eez_composer_submission_included_l1_block", "rollup_id" => self.rollup.clone()).set(*l1_block as f64);
                (2, "dispatch_to_included")
            }
            Some(SendOutcome::Dropped { reason, .. }) => {
                let reason = match reason {
                    BundleDropReason::Excluded => "excluded",
                    BundleDropReason::SlotSkipped => "slot_skipped",
                    BundleDropReason::SubmissionWindowMissed => "submission_window_missed",
                    BundleDropReason::TargetChanged => "target_changed",
                };
                metrics::counter!("eez_composer_submission_exclusions_total", "rollup_id" => self.rollup.clone(), "reason" => reason).increment(1);
                (3, "dispatch_to_excluded")
            }
            None => (4, "dispatch_to_uncertain"),
        };
        self.event(index);
        if let Some(dispatched) = self.dispatched.get() {
            self.duration(stage, dispatched.elapsed().as_secs_f64());
        }
    }

    pub(crate) fn preflight_failed(&self) {
        self.event(5);
        self.duration(
            "prepared_to_preflight_failure",
            self.started.elapsed().as_secs_f64(),
        );
    }
}

/// Counts signed cross-chain user requests selected for a submitted Sync
/// attempt, not generated effect/system transactions or confirmed successes.
/// Retries are separate attempts; anchor-only attempts contribute zeros.
#[allow(clippy::cast_precision_loss)] // Prometheus stores counts/heights as f64.
pub(crate) fn sync_requests(rollup_id: u64, height: u64, counts: [u64; 2]) {
    let rollup = rollup_id.to_string();
    metrics::gauge!("eez_composer_submission_sync_l2_block", "rollup_id" => rollup.clone())
        .set(height as f64);
    for (direction, count) in ["inbound", "outbound"].into_iter().zip(counts) {
        metrics::gauge!("eez_composer_submission_sync_requests", "rollup_id" => rollup.clone(), "direction" => direction).set(count as f64);
        metrics::histogram!("eez_composer_submission_sync_requests_per_attempt", "rollup_id" => rollup.clone(), "direction" => direction).record(count as f64);
    }
    tracing::info!(
        event_name = "eez.composer.submission.sync_requests",
        rollup_id,
        sync_height = height,
        inbound = counts[0],
        outbound = counts[1],
        "cross-chain user requests selected for Sync submission attempt"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use metrics_exporter_prometheus::PrometheusBuilder;

    #[test]
    #[allow(clippy::float_cmp)] // Exported small integer counts must match exactly.
    fn short_lived_outcomes_survive_without_sampling_and_keep_rollups_separate() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let untouched = SubmissionMetrics::new(2);
            let included = SubmissionMetrics::new(1);
            included.prepared();
            included.dispatch(100, 1000);
            included.dispatch(100, 1000); // A repeated callback is not another dispatch.
            included.outcome(Some(&SendOutcome::Included {
                tx_hash: B256::ZERO,
                l1_block: 100,
                l1_block_hash: B256::ZERO,
            }));
            for reason in [
                BundleDropReason::Excluded,
                BundleDropReason::SlotSkipped,
                BundleDropReason::SubmissionWindowMissed,
                BundleDropReason::TargetChanged,
            ] {
                let attempt = SubmissionMetrics::new(1);
                attempt.prepared();
                attempt.outcome(Some(&SendOutcome::Dropped {
                    tx_hash: B256::ZERO,
                    target_block: 101,
                    reason,
                }));
            }
            let uncertain = SubmissionMetrics::new(1);
            uncertain.prepared();
            uncertain.dispatch(102, 1010);
            uncertain.outcome(None);
            let preflight = SubmissionMetrics::new(1);
            preflight.prepared();
            preflight.preflight_failed();
            // Constructing the next observer must not zero previous timestamps/counters.
            let _ = SubmissionMetrics::new(1);
            drop(untouched);
            sync_requests(1, 7, [3, 2]);
            sync_requests(1, 8, [0, 0]);
        });
        let output = handle.render();
        let value = |name: &str, labels: &[&str]| -> f64 {
            output
                .lines()
                .find(|line| {
                    line.starts_with(&format!("{name}{{"))
                        && labels.iter().all(|l| line.contains(l))
                })
                .unwrap_or_else(|| panic!("missing {name} {labels:?}: {output}"))
                .rsplit_once(' ')
                .unwrap()
                .1
                .parse()
                .unwrap()
        };
        for (event, expected) in [
            ("prepared", 7.),
            ("dispatched", 2.),
            ("included", 1.),
            ("excluded", 4.),
            ("uncertain", 1.),
            ("preflight_failed", 1.),
        ] {
            assert_eq!(
                value(
                    "eez_composer_submission_events_total",
                    &["rollup_id=\"1\"", &format!("event=\"{event}\"")]
                ),
                expected
            );
            assert_eq!(
                value(
                    "eez_composer_submission_events_total",
                    &["rollup_id=\"2\"", &format!("event=\"{event}\"")]
                ),
                0.
            );
            assert!(
                value(
                    "eez_composer_submission_event_timestamp_seconds",
                    &["rollup_id=\"1\"", &format!("event=\"{event}\"")]
                ) > 0.
            );
        }
        for reason in REASONS {
            assert_eq!(
                value(
                    "eez_composer_submission_exclusions_total",
                    &["rollup_id=\"1\"", &format!("reason=\"{reason}\"")]
                ),
                1.
            );
        }
        assert_eq!(
            value(
                "eez_composer_submission_duration_seconds_count",
                &["stage=\"dispatch_to_included\""]
            ),
            1.
        );
        assert!(
            !output.contains("stage=\"dispatch_to_excluded\""),
            "undispatched exclusions must not fabricate dispatch durations"
        );
        assert_eq!(
            value(
                "eez_composer_submission_sync_requests_per_attempt_sum",
                &["direction=\"inbound\""]
            ),
            3.
        );
        assert_eq!(
            value(
                "eez_composer_submission_sync_requests_per_attempt_sum",
                &["direction=\"outbound\""]
            ),
            2.
        );
        assert_eq!(
            value(
                "eez_composer_submission_sync_requests_per_attempt_count",
                &["direction=\"inbound\""]
            ),
            2.
        );
        assert_eq!(
            value(
                "eez_composer_submission_sync_requests",
                &["direction=\"inbound\""]
            ),
            0.
        );
        assert_eq!(
            value(
                "eez_composer_submission_sync_l2_block",
                &["rollup_id=\"1\""]
            ),
            8.
        );
        assert!(
            !output.contains("sync_height=\""),
            "block heights must never be metric labels"
        );
    }
}
