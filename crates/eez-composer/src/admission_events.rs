//! Per-candidate observations use the retained, nonblocking tracing sink. Source
//! hashes belong in these records, never in metric labels or queue ownership.
use alloy_primitives::{Address, B256};
use eez_l1::BundleTarget;
use serde_json::{Value, json};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

pub(crate) struct Candidate {
    pub id: String,
    context: Value,
    sequence: AtomicU64,
    started: Instant,
    rollup: String,
}

impl Candidate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_id: u64,
        registry: Address,
        rollup_id: u64,
        parent: B256,
        target: BundleTarget,
        selected: usize,
        queued: usize,
        selection_limit: (usize, Option<&'static str>),
        rebuild_of: Option<String>,
    ) -> Self {
        static SESSION: OnceLock<String> = OnceLock::new();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let session = SESSION.get_or_init(|| {
            format!(
                "{}-{}",
                std::process::id(),
                crate::prover_retry::unix_time_millis()
            )
        });
        let id = format!(
            "{session}:{rollup_id}:{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let (block, timestamp) = match target {
            BundleTarget::Exact { block, timestamp } => (Some(block), Some(timestamp)),
            BundleTarget::NextBlock => (None, None),
        };
        let candidate = Self {
            context: json!({"version":1,"candidate_id":id,"chain_id":chain_id,
                "registry":registry,"rollup_id":rollup_id,"l2_parent":parent,
                "target_block":block,"target_timestamp":timestamp,"rebuild_of":rebuild_of,
                "policy_version":"packing-fifo-preference-2","estimator_version":"companion-execution-5"}),
            id,
            sequence: AtomicU64::new(0),
            started: Instant::now(),
            rollup: rollup_id.to_string(),
        };
        candidate.emit(
            "started",
            json!({"queued":queued,"selected":selected,
            "selection_limit":selection_limit.0,"selection_limit_reason":selection_limit.1}),
        );
        candidate
    }

    pub fn emit(&self, kind: &'static str, detail: Value) {
        let event = self.record(kind, detail);
        tracing::info!(target: "eez_posting", event_name = "eez.composer.admission.v1",
            admission_event = %event, "admission observation");
        metrics::counter!("eez_composer_admission_events_total", "rollup_id" => self.rollup.clone(), "kind" => kind).increment(1);
    }

    fn record(&self, kind: &str, detail: Value) -> Value {
        let mut event = self.context.clone();
        event["sequence"] = json!(self.sequence.fetch_add(1, Ordering::Relaxed));
        event["kind"] = json!(kind);
        event["timestamp_ms"] = json!(crate::prover_retry::unix_time_millis());
        event["elapsed_ms"] = json!(self.started.elapsed().as_secs_f64() * 1000.0);
        event["detail"] = detail;
        event
    }

    pub fn duration(&self, stage: &'static str, elapsed: std::time::Duration) {
        metrics::histogram!("eez_composer_admission_duration_seconds", "rollup_id" => self.rollup.clone(), "stage" => stage).record(elapsed.as_secs_f64());
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        // The sequence on this final record defines the expected number of
        // preceding observations, including paths that failed before proving.
        self.emit("finished", json!({}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilt_candidates_have_distinct_identity_and_explicit_parent() {
        let make = |parent| {
            Candidate::new(
                10200,
                Address::repeat_byte(1),
                1,
                B256::repeat_byte(2),
                BundleTarget::Exact {
                    block: 42,
                    timestamp: 100,
                },
                40,
                100,
                (20, Some("submission_backoff")),
                parent,
            )
        };
        let first = make(None);
        let rebuilt = make(Some(first.id.clone()));
        assert_ne!(first.id, rebuilt.id);
        let event = rebuilt.record("summary", json!({"accepted":20}));
        assert_eq!(event["rebuild_of"], first.id);
        assert_eq!(event["sequence"], 1);
        assert_eq!(event["target_timestamp"], 100);
        assert_eq!(event["detail"]["accepted"], 20);
        assert!(event.to_string().len() < 64 * 1024);
    }
}
