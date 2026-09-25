//! Correlated operational events. Tracing owns transport; this module never
//! performs I/O or changes submission/ownership decisions.
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Wall-clock points for the candidate that produced this posting. A fallback or
/// rebuilt candidate starts its own preparation interval. Proof request/receipt
/// bracket the complete client operation, including retries and RPC waits; they
/// are not signer CPU timings. Missing receipt means no successful proof returned.
pub(crate) struct PreparationTiming {
    pub(crate) preparation_started: u64,
    pub(crate) proof_requested: Option<u64>,
    pub(crate) proof_received: Option<u64>,
}

impl PreparationTiming {
    pub(crate) fn detail(&self) -> Value {
        json!({
            "preparation_started_ms": self.preparation_started,
            "proof_requested_ms": self.proof_requested,
            "proof_received_ms": self.proof_received,
        })
    }
}

pub(crate) struct PostingEvents {
    context: Value,
    sequence: AtomicU64,
    started: OnceLock<Instant>,
    dispatched: OnceLock<Instant>,
}

impl PostingEvents {
    pub(crate) fn new(
        chain_id: u64,
        registry: Address,
        rollup_id: u64,
        generation: u64,
        posting: B256,
        sync_height: u64,
        reasons: Option<Vec<&'static str>>,
    ) -> Self {
        static SESSION: OnceLock<String> = OnceLock::new();
        let session = SESSION.get_or_init(|| {
            // Only uniqueness is needed, not a secret or consensus identity.
            format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )
        });
        Self {
            context: json!({"version":1,"attempt_id":format!("{session}:{rollup_id}:{generation}"),
                "posting_id":posting,"chain_id":chain_id,"registry":registry,
                "rollup_id":rollup_id,"sync_height":sync_height,"trigger_reasons":reasons}),
            sequence: AtomicU64::new(0),
            started: OnceLock::new(),
            dispatched: OnceLock::new(),
        }
    }

    pub(crate) fn with_target(mut self, target: eez_l1::BundleTarget) -> Self {
        if let eez_l1::BundleTarget::Exact { block, timestamp } = target {
            self.context["target_block"] = json!(block);
            self.context["target_timestamp"] = json!(timestamp);
        }
        self
    }

    /// A failed preparation has no signed L1 transaction or ledger generation.
    /// Give it a separate identity namespace, never a fabricated transaction hash.
    pub(crate) fn preparation_failed(self, detail: Value) {
        let event = self.preparation_failure_record(detail);
        tracing::info!(target: "eez_posting", event_name = "eez.composer.posting.v1", posting_event = %event, "posting lifecycle");
    }

    fn preparation_failure_record(mut self, detail: Value) -> Value {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // The constructor always supplies a string attempt ID.
        self.context["attempt_id"] = json!(format!(
            "{}:0:{}",
            self.context["attempt_id"].as_str().unwrap(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut event = self.record("preparation_failed", B256::ZERO, detail);
        event["transaction_hash"] = Value::Null;
        event
    }

    fn record(&self, stage: &str, hash: B256, detail: Value) -> Value {
        let started = self.started.get_or_init(Instant::now);
        let mut event = self.context.clone();
        event["sequence"] = json!(self.sequence.fetch_add(1, Ordering::Relaxed));
        event["stage"] = json!(stage);
        event["transaction_hash"] = json!(hash);
        event["timestamp_ms"] = json!(crate::prover_retry::unix_time_millis());
        event["elapsed_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        event["dispatch_elapsed_ms"] = json!(
            self.dispatched
                .get()
                .map(|t| t.elapsed().as_secs_f64() * 1000.0)
        );
        event["detail"] = detail;
        event
    }

    pub(crate) fn emit(&self, stage: &str, hash: B256, detail: Value) {
        let event = self.record(stage, hash, detail);
        tracing::info!(target: "eez_posting", event_name = "eez.composer.posting.v1", posting_event = %event, "posting lifecycle");
    }

    pub(crate) fn dispatch(&self, hash: B256, block: u64, timestamp: u64) {
        if self.dispatched.set(Instant::now()).is_ok() {
            self.emit(
                "dispatched",
                hash,
                json!({"target_block":block,"target_timestamp":timestamp}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_preparations_keep_target_and_have_unique_unsigned_identity() {
        let make = || {
            PostingEvents::new(
                10200,
                Address::repeat_byte(1),
                1,
                0,
                B256::repeat_byte(2),
                9,
                None,
            )
            .with_target(eez_l1::BundleTarget::Exact {
                block: 42,
                timestamp: 100,
            })
        };
        let detail = json!({"reason":"preparation_deadline","preparation_ms":501.0});
        let first = make().preparation_failure_record(detail.clone());
        let second = make().preparation_failure_record(detail);
        assert_ne!(first["attempt_id"], second["attempt_id"]);
        assert_ne!(first["attempt_id"], make().context["attempt_id"]);
        assert_eq!(first["sequence"], 0);
        assert_eq!(first["stage"], "preparation_failed");
        assert!(first["transaction_hash"].is_null());
        assert_eq!(first["target_block"], 42);
        assert_eq!(first["target_timestamp"], 100);
        assert_eq!(
            make().record("prepared", B256::repeat_byte(3), json!({}))["target_timestamp"],
            100
        );
    }

    #[test]
    fn identity_survives_repricing_and_attempts_stay_distinct() {
        let events = PostingEvents::new(
            10200,
            Address::repeat_byte(1),
            1,
            4,
            B256::repeat_byte(2),
            9,
            Some(vec!["cross_chain"]),
        );
        let prepared = events.record("prepared", B256::repeat_byte(3), json!({}));
        let priced = events.record("repriced", B256::repeat_byte(4), json!({}));
        assert_eq!(prepared["attempt_id"], priced["attempt_id"]);
        assert_eq!(prepared["posting_id"], priced["posting_id"]);
        assert_ne!(prepared["transaction_hash"], priced["transaction_hash"]);
        assert_eq!(prepared["sequence"], 0);
        assert_eq!(priced["sequence"], 1);
        assert_eq!(priced["trigger_reasons"], json!(["cross_chain"]));
        let other = PostingEvents::new(
            10200,
            Address::repeat_byte(1),
            1,
            5,
            B256::repeat_byte(2),
            9,
            None,
        );
        assert_ne!(prepared["attempt_id"], other.context["attempt_id"]);
        assert!(other.context["trigger_reasons"].is_null());
        assert!(prepared["dispatch_elapsed_ms"].is_null());
        events.dispatch(B256::repeat_byte(4), 20, 30);
        events.dispatch(B256::repeat_byte(4), 20, 30);
        let included = events.record("included", B256::repeat_byte(4), json!({"l1_block":20}));
        assert_eq!(included["sequence"], 3);
        assert!(included["dispatch_elapsed_ms"].as_f64().unwrap() >= 0.0);
    }
}
