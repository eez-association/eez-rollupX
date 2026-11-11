//! Policy decisions and submission observations remain distinct from canonical
//! completion. These gauges only describe the existing Composer state.

use eez_composer::{
    composer::SettlementStatus, optimistic::SubmissionObservation, settlement_policy::DueReasons,
};
use metrics::{Gauge, Unit};

pub(crate) struct SettlementMetrics {
    due: Gauge,
    reasons: [(DueReasons, Gauge); 7],
    anchor: Gauge,
    anchor_l1_block: Gauge,
    settled: Gauge,
    unsettled: Gauge,
    threshold: Gauge,
    deadline: Gauge,
    target: Gauge,
    evaluated: Gauge,
    submission: [(&'static str, Gauge); 7],
}

impl SettlementMetrics {
    pub(crate) fn new(rollup_id: u64) -> Self {
        let rollup = rollup_id.to_string();
        let gauge = |name: &'static str, unit: Unit, help: &'static str| {
            metrics::describe_gauge!(name, unit, help);
            metrics::gauge!(name, "rollup_id" => rollup.clone())
        };
        metrics::describe_gauge!(
            "eez_composer_settlement_trigger",
            Unit::Count,
            "Active reasons in the last canonical-generation-valid policy evaluation; NaN when unknown"
        );
        metrics::describe_gauge!(
            "eez_composer_settlement_submission",
            Unit::Count,
            "Lowest unresolved attempt observation, one active state; included is not proof of completion"
        );
        Self {
            due: gauge("eez_composer_settlement_due", Unit::Count, "Last evaluated policy is due (1) or waiting for a trigger (0); NaN when unknown"),
            reasons: DueReasons::ALL.map(|(reason, name)| (reason,
                metrics::gauge!("eez_composer_settlement_trigger", "rollup_id" => rollup.clone(), "reason" => name))),
            anchor: gauge("eez_composer_settlement_anchor_timestamp_seconds", Unit::Seconds, "Canonical L1 timestamp anchoring the last policy evaluation; may be initial registration"),
            anchor_l1_block: gauge("eez_composer_settlement_anchor_l1_block", Unit::Count, "L1 block containing the commitment used by the last policy evaluation"),
            settled: gauge("eez_composer_settlement_anchor_l2_block", Unit::Count, "Validated L2 commitment used by the last policy evaluation"),
            unsettled: gauge("eez_composer_settlement_unsettled_blocks", Unit::Count, "Candidate-to-anchor block distance at the last evaluation, including the candidate Sync block"),
            threshold: gauge("eez_composer_settlement_block_threshold", Unit::Count, "Configured general settlement trigger in unsettled L2 blocks"),
            deadline: gauge("eez_composer_settlement_pure_interval_deadline_timestamp_seconds", Unit::Seconds, "Conditional pure-L2 interval deadline; NaN in always mode or without pending pure activity"),
            target: gauge("eez_composer_settlement_target_timestamp_seconds", Unit::Seconds, "Selected L1 target used for policy evaluation, not guaranteed submission or inclusion"),
            evaluated: gauge("eez_composer_settlement_evaluation_timestamp_seconds", Unit::Seconds, "Local time of the last successful canonical-generation-valid policy evaluation"),
            submission: ["none", "prepared", "dispatched", "included", "excluded", "preflight_rejected", "uncertain"].map(|state|
                (state, metrics::gauge!("eez_composer_settlement_submission", "rollup_id" => rollup.clone(), "state" => state))),
        }
    }

    #[allow(clippy::cast_precision_loss)] // Prometheus represents observational counts/times as f64.
    pub(crate) fn record(&self, status: Option<&SettlementStatus>) {
        let policy = status.and_then(|status| status.policy);
        let value = |f: fn(eez_composer::composer::SettlementPolicyObservation) -> f64| {
            policy.map_or(f64::NAN, f)
        };
        self.due.set(value(|p| f64::from(p.decision.is_due())));
        for (reason, gauge) in &self.reasons {
            gauge.set(policy.map_or(f64::NAN, |p| {
                f64::from(p.decision.reasons.contains(*reason))
            }));
        }
        self.anchor.set(value(|p| p.anchor.l1_timestamp as f64));
        self.anchor_l1_block
            .set(value(|p| p.anchor.l1.number as f64));
        self.settled.set(value(|p| p.anchor.endpoint.number as f64));
        self.unsettled
            .set(value(|p| p.decision.unsettled_blocks as f64));
        self.threshold.set(value(|p| p.block_threshold as f64));
        self.deadline.set(value(|p| {
            p.pure_interval_deadline_ms
                .map_or(f64::NAN, |ms| ms as f64 / 1000.0)
        }));
        self.target.set(value(|p| p.target_l1_timestamp as f64));
        self.evaluated
            .set(value(|p| p.evaluated_at_ms as f64 / 1000.0));
        let phase = status.map(|s| match &s.pending_submission {
            None => "none",
            Some(SubmissionObservation::Prepared) => "prepared",
            Some(SubmissionObservation::Dispatched { .. }) => "dispatched",
            Some(SubmissionObservation::Included { .. }) => "included",
            Some(SubmissionObservation::Excluded { .. }) => "excluded",
            Some(SubmissionObservation::PreflightRejected { .. }) => "preflight_rejected",
            Some(SubmissionObservation::Uncertain) => "uncertain",
        });
        for (name, gauge) in &self.submission {
            gauge.set(phase.map_or(f64::NAN, |phase| f64::from(phase == *name)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use eez_composer::{
        composer::SettlementPolicyObservation,
        settlement_policy::{PolicyDecision, SettlementAnchor},
    };
    use eez_l1::{L1BlockRef, L2Endpoint};
    use metrics_exporter_prometheus::PrometheusBuilder;
    use metrics_util::layers::{PrefixLayer, Stack};

    #[test]
    #[allow(clippy::float_cmp)] // Exact exported integer-valued gauges and fixed timestamp conversions.
    fn policy_reasons_reset_and_unknown_is_not_waiting_or_completed() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let recorder = Stack::new(recorder).push(PrefixLayer::new("reth"));
        let (metrics, other) = metrics::with_local_recorder(&recorder, || {
            (SettlementMetrics::new(1), SettlementMetrics::new(2))
        });
        other.record(None);
        let read = |family: &str, label: Option<(&str, &str)>| {
            let rendered = handle.render();
            let line = rendered
                .lines()
                .find(|line| {
                    line.starts_with(&format!("reth_eez_composer_settlement_{family}{{"))
                        && line.contains("rollup_id=\"1\"")
                        && label
                            .is_none_or(|(key, value)| line.contains(&format!("{key}=\"{value}\"")))
                })
                .unwrap();
            line.rsplit_once(' ').unwrap().1.parse::<f64>().unwrap()
        };
        let endpoint = L2Endpoint {
            number: 10,
            hash: B256::repeat_byte(10),
        };
        let mut observation = SettlementPolicyObservation {
            decision: PolicyDecision {
                reasons: DueReasons::default(),
                required_endpoint: None,
                unsettled_blocks: 12,
                settlement_age_ms: 5_000,
            },
            anchor: SettlementAnchor {
                l1: L1BlockRef {
                    number: 100,
                    hash: B256::repeat_byte(100),
                },
                endpoint,
                l1_timestamp: 1_000,
                tx_hash: B256::ZERO,
                tx_index: 0,
            },
            target_l1_timestamp: 1_005,
            block_threshold: 150,
            pure_interval_deadline_ms: Some(1_015_000),
            evaluated_at_ms: 1_004_500,
        };
        let mut status = SettlementStatus {
            policy: Some(observation),
            pending_submission: None,
        };
        metrics.record(Some(&status));
        assert_eq!(read("due", None), 0.0);
        assert_eq!(read("submission", Some(("state", "none"))), 1.0);
        assert_eq!(
            read("pure_interval_deadline_timestamp_seconds", None),
            1015.0
        );
        assert_eq!(read("evaluation_timestamp_seconds", None), 1004.5);
        assert_eq!(read("anchor_timestamp_seconds", None), 1000.0);
        assert_eq!(read("target_timestamp_seconds", None), 1005.0);
        assert_eq!(read("anchor_l2_block", None), 10.0);
        assert_eq!(read("anchor_l1_block", None), 100.0);
        assert_eq!(read("unsettled_blocks", None), 12.0);
        assert_eq!(read("block_threshold", None), 150.0);
        for (reason, name) in DueReasons::ALL {
            observation.decision.reasons = reason;
            status.policy = Some(observation);
            metrics.record(Some(&status));
            assert_eq!(read("due", None), 1.0);
            for (_, candidate) in DueReasons::ALL {
                assert_eq!(
                    read("trigger", Some(("reason", candidate))),
                    f64::from(candidate == name)
                );
            }
        }
        status.policy = None;
        metrics.record(Some(&status));
        assert!(read("due", None).is_nan());
        assert!(read("pure_interval_deadline_timestamp_seconds", None).is_nan());
        for (_, name) in DueReasons::ALL {
            assert!(read("trigger", Some(("reason", name))).is_nan());
        }
        for (name, observation) in [
            ("prepared", SubmissionObservation::Prepared),
            (
                "dispatched",
                SubmissionObservation::Dispatched {
                    block: 100,
                    timestamp: 1000,
                },
            ),
            (
                "included",
                SubmissionObservation::Included {
                    block: L1BlockRef {
                        number: 100,
                        hash: B256::ZERO,
                    },
                },
            ),
            (
                "excluded",
                SubmissionObservation::Excluded {
                    block: 100,
                    charge_attempt: false,
                },
            ),
            (
                "preflight_rejected",
                SubmissionObservation::PreflightRejected { source: None },
            ),
            ("uncertain", SubmissionObservation::Uncertain),
        ] {
            status.pending_submission = Some(observation);
            metrics.record(Some(&status));
            for (state, _) in &metrics.submission {
                assert_eq!(
                    read("submission", Some(("state", state))),
                    f64::from(*state == name)
                );
            }
        }
        metrics.record(None);
        assert!(read("submission", Some(("state", "included"))).is_nan());
        let rendered = handle.render();
        let other_lines = rendered
            .lines()
            .filter(|line| {
                line.starts_with("reth_eez_composer_settlement_")
                    && line.contains("rollup_id=\"2\"")
            })
            .collect::<Vec<_>>();
        assert_eq!(other_lines.len(), 23);
        assert!(other_lines.iter().all(|line| line.ends_with(" NaN")));
    }
}
