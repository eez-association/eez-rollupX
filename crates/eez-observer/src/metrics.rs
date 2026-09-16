//! Prometheus text exposition of bounded gauges; no global recorder is needed.
use std::fmt::Write;

use crate::model::{Data, Sample};

fn family(out: &mut String, name: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
}

pub fn render(samples: &[Sample], now_ms: u64) -> String {
    let mut out = String::new();
    family(
        &mut out,
        "eez_observer_collection_success",
        "Whether the latest collection succeeded and is no older than 90 seconds.",
    );
    for sample in samples {
        let valid = sample.is_current(now_ms);
        let _ = writeln!(
            out,
            "eez_observer_collection_success{{source=\"{}\"}} {}",
            sample.source.label(),
            u8::from(valid)
        );
    }
    family(
        &mut out,
        "eez_observer_collection_last_success_timestamp_seconds",
        "Last successful identity-verified collection; absent before first success.",
    );
    for sample in samples {
        if let Some(time) = sample.last_success {
            let _ = writeln!(
                out,
                "eez_observer_collection_last_success_timestamp_seconds{{source=\"{}\"}} {}.{:03}",
                sample.source.label(),
                time / 1000,
                time % 1000
            );
        }
    }
    family(
        &mut out,
        "eez_observer_collection_error",
        "Latest bounded collection error; absent before first attempt and after recovery.",
    );
    for sample in samples {
        if let Some(error) = sample.failure {
            let _ = writeln!(
                out,
                "eez_observer_collection_error{{source=\"{}\",reason=\"{}\"}} 1",
                sample.source.label(),
                error.label()
            );
        }
    }
    family(
        &mut out,
        "eez_observer_chain_head_number",
        "Latest observed head number, not a finality claim.",
    );
    family(
        &mut out,
        "eez_observer_chain_head_timestamp_seconds",
        "Timestamp of the latest observed chain head.",
    );
    family(
        &mut out,
        "eez_observer_queue_transactions",
        "Disjoint queue state counts; readiness requires queue_evaluation_valid.",
    );
    family(
        &mut out,
        "eez_observer_queue_evaluation_valid",
        "Source evaluation is present and at most 60 seconds old with at most 5 seconds future skew.",
    );
    family(
        &mut out,
        "eez_observer_queue_evaluated_timestamp_seconds",
        "Runtime readiness evaluation time; absent if unknown.",
    );
    family(
        &mut out,
        "eez_observer_queue_oldest_age_seconds",
        "Source-reported wall-clock age at collection, not a completion latency; absent if unknown.",
    );
    for sample in samples {
        if !sample.is_current(now_ms) {
            continue;
        }
        match &sample.data {
            Some(Data::Head(head)) => {
                let _ = writeln!(
                    out,
                    "eez_observer_chain_head_number{{source=\"{}\"}} {}",
                    sample.source.label(),
                    head.number
                );
                let _ = writeln!(
                    out,
                    "eez_observer_chain_head_timestamp_seconds{{source=\"{}\"}} {}",
                    sample.source.label(),
                    head.timestamp
                );
            }
            Some(Data::Queue(queue)) => {
                for (state, count) in [
                    ("queued_ready", queue.ready),
                    ("queued_blocked", queue.blocked),
                    ("queued_unknown", queue.unknown),
                    ("in_flight", queue.in_flight),
                ] {
                    let _ = writeln!(
                        out,
                        "eez_observer_queue_transactions{{state=\"{state}\"}} {count}"
                    );
                }
                let _ = writeln!(
                    out,
                    "eez_observer_queue_evaluation_valid {}",
                    u8::from(queue.evaluation_valid(now_ms))
                );
                if let Some(time) = queue.evaluated_at_ms {
                    let _ = writeln!(
                        out,
                        "eez_observer_queue_evaluated_timestamp_seconds {}.{:03}",
                        time / 1000,
                        time % 1000
                    );
                }
                for (kind, age) in [
                    ("pending", queue.oldest_pending_age_ms),
                    ("serviceable", queue.oldest_service_age_ms),
                ] {
                    if let Some(age) = age {
                        let _ = writeln!(
                            out,
                            "eez_observer_queue_oldest_age_seconds{{kind=\"{kind}\"}} {}.{:03}",
                            age / 1000,
                            age % 1000
                        );
                    }
                }
            }
            None => {}
        }
    }
    out
}
