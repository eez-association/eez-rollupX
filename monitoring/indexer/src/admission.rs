//! Admission history shares the posting collector's atomic file checkpoints.
//! Records are operational observations; canonical completion is joined by the
//! final signed transaction, never inferred from an admission decision.
use crate::config::Config;
use alloy_primitives::{Address, B256};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio_postgres::Transaction;

const KINDS: &[&str] = &[
    "started",
    "source_selected",
    "source_outcome",
    "source_preflight",
    "source_estimate",
    "source_capacity_check",
    "source_accepted",
    "summary",
    "full_projection",
    "proof_requested",
    "proof_received",
    "proof_failed",
    "preflight",
    "preflight_failed",
    "dispatch",
    "delivery",
    "finished",
];
const CONTEXT: &[&str] = &[
    "chain_id",
    "registry",
    "rollup_id",
    "l2_parent",
    "target_block",
    "target_timestamp",
    "rebuild_of",
    "policy_version",
    "estimator_version",
];
fn identity(value: &Value) -> bool {
    value.as_str().is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b':' || b == b'-')
    })
}

pub fn parse(line: &[u8], config: &Config) -> Result<Option<Value>> {
    let log: Value = serde_json::from_slice(line)?;
    let fields = log.get("fields").unwrap_or(&log);
    if !fields["event_name"]
        .as_str()
        .is_some_and(|s| s.starts_with("eez.composer.admission."))
    {
        return Ok(None);
    }
    ensure!(line.len() <= 64 * 1024, "oversized admission event");
    ensure!(
        fields["event_name"] == "eez.composer.admission.v1",
        "unsupported admission schema"
    );
    let data: Value = serde_json::from_str(
        fields["admission_event"]
            .as_str()
            .context("missing admission event")?,
    )?;
    ensure!(data["version"] == 1, "unsupported admission version");
    ensure!(
        data["chain_id"].as_u64() == Some(config.chain_id)
            && data["rollup_id"].as_u64() == Some(config.rollup_id)
            && data["registry"]
                .as_str()
                .context("missing registry")?
                .parse::<Address>()?
                == config.registry,
        "admission network/rollup mismatch"
    );
    ensure!(
        identity(&data["candidate_id"]),
        "invalid candidate identity"
    );
    ensure!(
        data["rebuild_of"].is_null() || identity(&data["rebuild_of"]),
        "invalid rebuild identity"
    );
    ensure!(
        data["sequence"]
            .as_u64()
            .is_some_and(|n| n <= i32::MAX as u64),
        "invalid admission sequence"
    );
    ensure!(
        data["timestamp_ms"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= 253_402_300_799_999),
        "invalid admission time"
    );
    ensure!(
        data["elapsed_ms"]
            .as_f64()
            .is_some_and(|n| n.is_finite() && n >= 0.0),
        "invalid admission elapsed time"
    );
    ensure!(
        data["l2_parent"]
            .as_str()
            .context("missing parent")?
            .parse::<B256>()?
            != B256::ZERO,
        "zero parent"
    );
    for field in ["policy_version", "estimator_version"] {
        ensure!(
            data[field]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 64),
            "invalid policy/estimator version"
        );
    }
    ensure!(
        data["target_block"].is_null() == data["target_timestamp"].is_null(),
        "incomplete admission target"
    );
    for field in ["target_block", "target_timestamp"] {
        ensure!(
            data[field].is_null()
                || data[field].as_u64().is_some_and(|n| n > 0
                    && n <= if field == "target_timestamp" {
                        253_402_300_799
                    } else {
                        i64::MAX as u64
                    }),
            "invalid admission target"
        );
    }
    let kind = data["kind"].as_str().context("missing admission kind")?;
    ensure!(KINDS.contains(&kind), "unknown admission kind");
    let detail = data["detail"]
        .as_object()
        .context("missing admission detail")?;
    if kind.starts_with("source_") {
        ensure!(
            detail
                .get("source_hash")
                .and_then(Value::as_str)
                .context("missing source hash")?
                .parse::<B256>()?
                != B256::ZERO,
            "zero source hash"
        );
    }
    if matches!(kind, "preflight" | "dispatch") {
        ensure!(
            data["detail"]["transaction_hash"]
                .as_str()
                .context("missing signed hash")?
                .parse::<B256>()?
                != B256::ZERO,
            "zero signed hash"
        );
    }
    // Every SQL numeric cast below is backed by a bounded numeric JSON value.
    for (key, value) in detail {
        if matches!(
            key.as_str(),
            "queued"
                | "selected"
                | "selection_limit"
                | "accepted"
                | "deferred"
                | "position"
                | "entries"
                | "blobs"
                | "calldata_bytes"
                | "entry_count"
        ) || key.ends_with("_count")
            || key.ends_with("_gas")
            || key.ends_with("_cap")
            || key.ends_with("_discount")
            || matches!(
                key.as_str(),
                "projected_before" | "projected_after" | "initial_queue_cleanup"
            )
        {
            ensure!(
                value.is_null() || value.as_u64().is_some(),
                "invalid admission numeric field {key}"
            );
        }
        if key.ends_with("_ms") {
            ensure!(
                value.is_null()
                    || value.as_f64().is_some_and(
                        |n| n.is_finite() && (0.0..=253_402_300_799_999.0).contains(&n)
                    ),
                "invalid admission duration {key}"
            );
        }
        if matches!(
            key.as_str(),
            "preflight_gas_used" | "preflight_gas_required" | "signed_gas_limits"
        ) {
            ensure!(
                value
                    .as_array()
                    .is_some_and(|a| a.len() <= 4096 && a.iter().all(|n| n.as_u64().is_some())),
                "invalid preflight gas vector"
            );
        }
    }
    if kind == "summary" {
        if let Some(flag) = detail.get("fifo_bypassed") {
            ensure!(flag.is_boolean(), "invalid FIFO bypass flag");
        }
        if let Some(reasons) = detail.get("capacity_deferrals") {
            let reasons = reasons.as_object().context("invalid capacity deferrals")?;
            ensure!(reasons.len() <= 32, "too many capacity deferral reasons");
            let mut total = 0_u64;
            for (reason, count) in reasons {
                ensure!(
                    !reason.is_empty()
                        && reason.len() <= 64
                        && reason
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'),
                    "invalid capacity deferral reason"
                );
                let count = count
                    .as_u64()
                    .filter(|n| *n > 0)
                    .context("invalid capacity deferral count")?;
                total = total
                    .checked_add(count)
                    .context("capacity deferral count overflow")?;
            }
            ensure!(
                detail
                    .get("deferred")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| total <= n),
                "capacity deferrals exceed deferred groups"
            );
        }
    }
    if kind == "source_estimate" {
        if let Some(reasons) = detail.get("estimate_reasons") {
            let reasons = reasons.as_object().context("invalid estimate reasons")?;
            ensure!(reasons.len() <= 32, "too many estimate reasons");
            let mut total = 0_u64;
            for (reason, count) in reasons {
                ensure!(
                    !reason.is_empty()
                        && reason.len() <= 64
                        && reason.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'),
                    "invalid estimate reason"
                );
                let count = count
                    .as_u64()
                    .filter(|v| *v != 0)
                    .context("invalid estimate reason count")?;
                total = total
                    .checked_add(count)
                    .context("estimate reason count overflow")?;
            }
            ensure!(
                detail.get("entry_count").and_then(Value::as_u64) == Some(total),
                "estimate reasons do not cover entries"
            );
        }
        if let Some(revoked) = detail.get("batch_discount_revoked") {
            ensure!(revoked.is_boolean(), "invalid batch discount flag");
        }
        ensure!(
            data["detail"]["gas_gate_accepted"].is_boolean(),
            "missing gas decision"
        );
        ensure!(
            matches!(
                data["detail"]["direction"].as_str(),
                Some("inbound" | "outbound")
            ),
            "invalid source direction"
        );
    }
    if kind == "source_capacity_check" {
        ensure!(
            detail.get("basis").and_then(Value::as_str) == Some("minimum_admission_cost"),
            "invalid capacity check basis"
        );
        ensure!(
            matches!(
                detail.get("direction").and_then(Value::as_str),
                Some("inbound" | "outbound")
            ),
            "invalid capacity check direction"
        );
        ensure!(
            matches!(
                detail.get("constraint").and_then(Value::as_str),
                Some("l1_tx_gas_budget" | "l1_bundle_gas_budget" | "l2_block_capacity")
            ),
            "invalid capacity check constraint"
        );
        for field in [
            "minimum_entry_gas",
            "projected_before",
            "postbatch_planning_cap",
            "companion_prefix_gas",
            "companion_required_gas",
            "companion_declared_gas",
            "fixed_drain_reserve_gas",
            "whole_block_cap",
            "transaction_cap",
            "l2_used_gas",
            "minimum_l2_gas",
            "l2_block_cap",
        ] {
            ensure!(
                detail.get(field).and_then(Value::as_u64).is_some(),
                "missing capacity bound: {field}"
            );
        }
    }
    Ok(Some(data))
}

pub async fn import(tx: &Transaction<'_>, event: &Value) -> Result<()> {
    let id = event["candidate_id"].as_str().context("candidate ID")?;
    let sequence = i32::try_from(event["sequence"].as_u64().context("sequence")?)?;
    if let Some(prior) = tx
        .query_opt(
            "SELECT data::text FROM eez_indexer.admission_events WHERE candidate_id=$1 LIMIT 1",
            &[&id],
        )
        .await?
    {
        let prior: Value = serde_json::from_str(&prior.get::<_, String>(0))?;
        for field in CONTEXT {
            ensure!(
                prior[field] == event[field],
                "admission context changed: {field}"
            );
        }
    }
    let changed = tx.execute("INSERT INTO eez_indexer.admission_events(candidate_id,sequence,data) VALUES($1,$2,$3::text::jsonb) ON CONFLICT(candidate_id,sequence) DO UPDATE SET data=EXCLUDED.data WHERE admission_events.data=EXCLUDED.data", &[&id,&sequence,&event.to_string()]).await?;
    ensure!(changed == 1, "conflicting duplicate admission event");
    Ok(())
}
