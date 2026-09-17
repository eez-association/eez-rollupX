//! Confirm a native current derivation dependency, never historical audit misses.
use super::{Config, Rpc, SharedStatus, remote_block};
use crate::{
    accounting::hash,
    model::{Head, quantity},
};
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};
use std::{fmt::Write as _, time::Duration};

#[derive(Debug, Clone, Default)]
pub struct Status {
    at: Option<u64>,
    blocked: Option<bool>,
    attempts: Option<u64>,
    reason: Option<&'static str>,
}
pub fn render(status: Option<&Status>, now: u64) -> String {
    let Some(status) = status else {
        return "eez_observer_required_blob_evaluation_valid 0\n".into();
    };
    let fresh = status
        .at
        .is_some_and(|at| at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000);
    let known = status.blocked.filter(|_| fresh);
    let mut result = format!(
        "eez_observer_required_blob_evaluation_valid {}\n",
        u8::from(known.is_some())
    );
    if let Some(blocked) = known {
        writeln!(
            result,
            "eez_observer_required_blob_blocked {}",
            u8::from(blocked)
        )
        .unwrap();
    }
    if fresh && let Some(attempts) = status.attempts {
        writeln!(result, "eez_observer_required_blob_retries {attempts}").unwrap();
    }
    if let Some(at) = status.at {
        writeln!(
            result,
            "eez_observer_required_blob_last_attempt_timestamp_seconds {}",
            at / 1000
        )
        .unwrap();
    }
    if known.is_none() {
        writeln!(
            result,
            "eez_observer_required_blob_unknown{{reason=\"{}\"}} 1",
            if fresh {
                status.reason.unwrap_or("unproven")
            } else {
                "stale"
            }
        )
        .unwrap();
    }
    result
}
pub(super) async fn run(config: &Config, rpc: &Rpc, status: &SharedStatus) {
    if config.l1.contracts.is_empty() {
        return;
    }
    let mut failures = 0u32;
    loop {
        failures = if poll(config, rpc, status).await {
            0
        } else {
            failures.saturating_add(1)
        };
        tokio::time::sleep(Duration::from_secs(crate::retry_delay(failures))).await;
    }
}
pub(super) async fn poll(config: &Config, rpc: &Rpc, status: &SharedStatus) -> bool {
    if config.l1.contracts.is_empty() {
        return true;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), collect(config, rpc))
        .await
        .unwrap_or(Err("timeout"));
    let okay = result.is_ok();
    let mut state = result.unwrap_or_else(|reason| Status {
        reason: Some(reason),
        ..Default::default()
    });
    state.at = Some(crate::now_ms());
    status.write().await.required_blobs = Some(state);
    okay
}
fn parse_hash(value: &Value) -> Result<B256, &'static str> {
    hash(value).map_err(|_| "invalid_evidence")
}
fn number(value: &Value) -> Result<u64, &'static str> {
    value.as_u64().ok_or("invalid_evidence")
}
fn response(value: &Value) -> Result<&Value, &'static str> {
    if value["version"] != 1
        || !matches!(
            value["identity"]["role"].as_str(),
            Some("composer" | "follower")
        )
        || value["epoch"]
            .as_str()
            .is_none_or(|s| s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        || value["generation"].as_u64().is_none()
    {
        return Err("unsupported");
    }
    value.get("requiredBlob").ok_or("unsupported")
}
fn reference(value: &Value, rollup: u64) -> Result<(u64, B256, B256, u64, u64), &'static str> {
    if value["rollupId"] != rollup
        || !matches!(
            value["reason"].as_str(),
            Some("beacon_unavailable" | "expected_hash_missing")
        )
        || !matches!(value["consumer"].as_str(), Some("deriver" | "watcher"))
        || number(&value["ageMs"])? > 90000
    {
        return Err("stale_or_invalid_evidence");
    }
    let attempts = number(&value["attempts"])?;
    if attempts == 0 || attempts > u64::from(u32::MAX) {
        return Err("invalid_evidence");
    }
    Ok((
        number(&value["l1BlockNumber"])?,
        parse_hash(&value["l1BlockHash"])?,
        parse_hash(&value["transactionHash"])?,
        number(&value["transactionIndex"])?,
        attempts,
    ))
}
fn stable(first: &Value, last: &Value) -> bool {
    if ["epoch", "identity", "generation", "through", "ready"]
        .iter()
        .any(|key| first[*key] != last[*key])
    {
        return false;
    }
    let a = &first["requiredBlob"];
    let b = &last["requiredBlob"];
    if a.is_null() || b.is_null() {
        return a == b;
    }
    [
        "l1BlockNumber",
        "l1BlockHash",
        "transactionHash",
        "transactionIndex",
        "rollupId",
        "consumer",
        "reason",
    ]
    .iter()
    .all(|key| a[*key] == b[*key])
        && b["attempts"].as_u64() >= a["attempts"].as_u64()
}
async fn diagnostic(config: &Config, rpc: &Rpc) -> Result<Value, &'static str> {
    rpc.call(&config.exporter.l2.url,"eez_getDerivationEvidence",json!([{"l1BlockNumber":0,"l1BlockHash":B256::ZERO,"transactionHash":B256::ZERO,"transactionIndex":0,"l2BlockNumber":0}])).await.map_err(|e|if e==crate::rpc::Failure::Unsupported {"unsupported"}else{"rpc"})
}
fn inclusion(
    value: &Value,
    block: u64,
    anchor: B256,
    tx: B256,
    index: u64,
    contracts: &[String],
) -> Result<Address, &'static str> {
    if parse_hash(&value["blockHash"])? != anchor
        || parse_hash(&value["transactionHash"])? != tx
        || quantity(&value["blockNumber"]).map_err(|_| "invalid_receipt")? != block
        || quantity(&value["transactionIndex"]).map_err(|_| "invalid_receipt")? != index
        || value["status"] != "0x1"
    {
        return Err("invalid_receipt");
    }
    let recipient = value["to"]
        .as_str()
        .and_then(|s| s.parse::<Address>().ok())
        .ok_or("invalid_receipt")?;
    if !contracts
        .iter()
        .any(|s| s.parse::<Address>().ok() == Some(recipient))
    {
        return Err("scope");
    }
    Ok(recipient)
}
async fn collect(config: &Config, rpc: &Rpc) -> Result<Status, &'static str> {
    rpc.validate_identity(&config.exporter.l1)
        .await
        .map_err(|_| "identity")?;
    rpc.validate_identity(&config.exporter.l2)
        .await
        .map_err(|_| "identity")?;
    let first = diagnostic(config, rpc).await?;
    let required = response(&first)?;
    let identity = &first["identity"];
    let eez = identity["eez"]
        .as_str()
        .and_then(|s| s.parse::<Address>().ok())
        .ok_or("scope")?;
    if identity["l1ChainId"] != config.exporter.l1.chain_id
        || identity["rollupId"] != config.rollup_id
        || !config
            .l1
            .contracts
            .iter()
            .any(|s| s.parse::<Address>().ok() == Some(eez))
    {
        return Err("scope");
    }

    let (blocked, attempts, block, anchor) = if required.is_null() {
        // No outstanding failure is healthy only with a completed current scan.
        if first["ready"] != true || first["through"].is_null() {
            return Err("unproven");
        }
        let through = number(&first["through"]["number"])?;
        let hash = parse_hash(&first["through"]["hash"])?;
        let latest = Head::parse(
            &rpc.block(&config.exporter.l1, json!("latest"), false)
                .await
                .map_err(|_| "rpc")?,
        )
        .map_err(|_| "rpc")?;
        if latest.number != through || latest.hash.parse::<B256>().ok() != Some(hash) {
            return Err("lag");
        }
        (false, 0, through, hash)
    } else {
        let (block, anchor, tx, index, attempts) = reference(required, config.rollup_id)?;
        if first["through"]["number"]
            .as_u64()
            .is_some_and(|through| through >= block)
        {
            return Err("superseded_dependency");
        }
        let receipt = rpc
            .receipt(&config.exporter.l1, tx)
            .await
            .map_err(|_| "rpc")?;
        let recipient = inclusion(&receipt, block, anchor, tx, index, &config.l1.contracts)?;
        if recipient != eez {
            return Err("scope");
        }
        let transaction = rpc
            .transaction(&config.exporter.l1, tx)
            .await
            .map_err(|_| "rpc")?;
        if parse_hash(&transaction["hash"])? != tx
            || parse_hash(&transaction["blockHash"])? != anchor
            || quantity(&transaction["blockNumber"]).map_err(|_| "invalid_transaction")? != block
            || quantity(&transaction["transactionIndex"]).map_err(|_| "invalid_transaction")?
                != index
            || transaction["type"] != "0x3"
            || transaction["to"]
                .as_str()
                .and_then(|s| s.parse::<Address>().ok())
                != Some(recipient)
            || transaction["blobVersionedHashes"]
                .as_array()
                .is_none_or(|hashes| {
                    hashes.is_empty()
                        || hashes.len() > 64
                        || hashes.iter().any(|value| parse_hash(value).is_err())
                })
        {
            return Err("invalid_transaction");
        }
        (true, attempts, block, anchor)
    };
    if remote_block(rpc, &config.exporter.l1, block)
        .await
        .map_err(|_| "rpc")?
        .hash
        != anchor
    {
        return Err("reorg");
    }
    let last = diagnostic(config, rpc).await?;
    response(&last)?;
    if !stable(&first, &last) {
        return Err("changed_evidence");
    }
    if !last["requiredBlob"].is_null() {
        reference(&last["requiredBlob"], config.rollup_id)?;
    }
    if remote_block(rpc, &config.exporter.l1, block)
        .await
        .map_err(|_| "rpc")?
        .hash
        != anchor
    {
        return Err("reorg");
    }
    Ok(Status {
        blocked: Some(blocked),
        attempts: Some(attempts),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn failure() -> Value {
        json!({"l1BlockNumber":7,"l1BlockHash":B256::repeat_byte(7),"transactionHash":B256::repeat_byte(8),"transactionIndex":1,"rollupId":2,"reason":"beacon_unavailable","consumer":"watcher","attempts":3,"ageMs":1})
    }
    fn capability() -> Value {
        json!({"version":1,"identity":{"role":"composer","l1ChainId":1,"rollupId":2,"eez":Address::repeat_byte(1)},"epoch":"0123456789abcdef0123456789abcdef","generation":1,"ready":true,"through":{"number":6,"hash":B256::repeat_byte(6)},"requiredBlob":failure()})
    }
    #[test]
    fn only_typed_fresh_scoped_current_dependency_is_valid() {
        let mut value = capability();
        assert_eq!(reference(response(&value).unwrap(), 2).unwrap().4, 3);
        value["requiredBlob"]["rollupId"] = json!(3);
        assert!(reference(&value["requiredBlob"], 2).is_err());
        value = capability();
        value["requiredBlob"]["ageMs"] = json!(90001);
        assert!(reference(&value["requiredBlob"], 2).is_err());
        value = capability();
        value["requiredBlob"]["reason"] = json!("historical_probe_pruned");
        assert!(reference(&value["requiredBlob"], 2).is_err());
        value = capability();
        value.as_object_mut().unwrap().remove("requiredBlob");
        assert_eq!(response(&value), Err("unsupported"));
    }
    #[test]
    fn retries_may_increase_but_reference_restart_reorg_and_clear_invalidate() {
        let first = capability();
        let mut last = first.clone();
        last["requiredBlob"]["attempts"] = json!(4);
        assert!(stable(&first, &last));
        for key in ["epoch", "generation", "through"] {
            let mut changed = first.clone();
            changed[key] = Value::Null;
            assert!(!stable(&first, &changed));
        }
        last = first.clone();
        last["requiredBlob"]["l1BlockHash"] = json!(B256::ZERO);
        assert!(!stable(&first, &last));
        last = first.clone();
        last["requiredBlob"] = Value::Null;
        assert!(!stable(&first, &last));
    }
    #[test]
    fn paid_receipt_is_not_enough_without_exact_identity_and_contract() {
        let contract = Address::repeat_byte(1);
        let receipt = json!({"blockNumber":"0x7","blockHash":B256::repeat_byte(7),"transactionHash":B256::repeat_byte(8),"transactionIndex":"0x1","status":"0x1","to":contract});
        assert_eq!(
            inclusion(
                &receipt,
                7,
                B256::repeat_byte(7),
                B256::repeat_byte(8),
                1,
                &[format!("{contract:#x}")]
            ),
            Ok(contract)
        );
        assert!(
            inclusion(
                &receipt,
                7,
                B256::repeat_byte(9),
                B256::repeat_byte(8),
                1,
                &[format!("{contract:#x}")]
            )
            .is_err()
        );
        assert_eq!(
            inclusion(
                &receipt,
                7,
                B256::repeat_byte(7),
                B256::repeat_byte(8),
                1,
                &[]
            ),
            Err("scope")
        );
    }
    #[test]
    fn unknown_and_expired_samples_cannot_page_as_blocked_or_healthy() {
        assert!(!render(None, 1).contains("required_blob_blocked"));
        let state = Status {
            at: Some(1),
            blocked: Some(true),
            attempts: Some(3),
            reason: None,
        };
        assert!(render(Some(&state), 2).contains("required_blob_retries 3"));
        assert!(!render(Some(&state), 90002).contains("required_blob_blocked"));
    }
    #[tokio::test]
    async fn private_rpc_confirms_dependency_and_recovery_without_using_archive_samples() {
        use axum::{
            Router,
            body::Bytes,
            extract::{Path, State},
            routing::post,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Fixture {
            mode: AtomicUsize,
            reads: AtomicUsize,
        }
        async fn serve(
            State(state): State<Arc<Fixture>>,
            Path(role): Path<String>,
            body: Bytes,
        ) -> String {
            let request: Value = serde_json::from_slice(&body).unwrap();
            let mode = state.mode.load(Ordering::SeqCst);
            let result = match request["method"].as_str().unwrap() {
                "eth_chainId" => json!(if role == "l1" { "0x1" } else { "0x2" }),
                "eth_getBlockByNumber" => {
                    let number = if request["params"][0] == "0x0" { 0 } else { 7 };
                    json!({"number":format!("0x{number:x}"),"hash":B256::repeat_byte(if number==0 {0}else if mode==3 {9}else{7}),"parentHash":B256::repeat_byte(6),"timestamp":"0x64"})
                }
                "eez_getDerivationEvidence" => {
                    let mut value = capability();
                    match mode {
                        1 => {
                            value["requiredBlob"] = Value::Null;
                            value["through"] = json!({"number":7,"hash":B256::repeat_byte(7)});
                        }
                        2 => value["requiredBlob"]["ageMs"] = json!(90001),
                        4 => {
                            value["generation"] = json!(state.reads.fetch_add(1, Ordering::SeqCst))
                        }
                        5 => value["through"] = json!({"number":7,"hash":B256::repeat_byte(7)}),
                        _ => {}
                    }
                    value
                }
                "eth_getTransactionReceipt" => {
                    json!({"blockNumber":"0x7","blockHash":B256::repeat_byte(7),"transactionHash":B256::repeat_byte(8),"transactionIndex":"0x1","status":"0x1","to":Address::repeat_byte(1)})
                }
                "eth_getTransactionByHash" => {
                    json!({"blockNumber":"0x7","blockHash":B256::repeat_byte(7),"hash":B256::repeat_byte(8),"transactionIndex":"0x1","type":"0x3","to":Address::repeat_byte(1),"blobVersionedHashes":[B256::repeat_byte(1)]})
                }
                _ => panic!("unexpected RPC"),
            };
            json!({"jsonrpc":"2.0","id":1,"result":result}).to_string()
        }
        let state = Arc::new(Fixture {
            mode: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/{role}", post(serve))
            .with_state(Arc::clone(&state));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config:Config=serde_json::from_value(json!({"exporter":{"l1":{"url":format!("{base}/l1"),"chain_id":1,"genesis_hash":B256::ZERO},"l2":{"url":format!("{base}/l2"),"chain_id":2,"genesis_hash":B256::ZERO}},"environment":"fixture","deployment":"required-blob","rollup_id":2,"database_config":"unused","l1":{"bootstrap":0,"contracts":[Address::repeat_byte(1)]},"l2":{"bootstrap":0,"include_all_transactions":true}})).unwrap();
        let rpc = Rpc::new().unwrap();
        let blocked = collect(&config, &rpc).await.unwrap();
        assert_eq!(blocked.blocked, Some(true));
        assert_eq!(blocked.attempts, Some(3));
        for (mode, reason) in [
            (2, "stale_or_invalid_evidence"),
            (3, "reorg"),
            (4, "changed_evidence"),
            (5, "superseded_dependency"),
        ] {
            state.mode.store(mode, Ordering::SeqCst);
            assert_eq!(collect(&config, &rpc).await.unwrap_err(), reason);
        }
        state.mode.store(1, Ordering::SeqCst);
        let recovered = collect(&config, &rpc).await.unwrap();
        assert_eq!(recovered.blocked, Some(false));
        assert_eq!(recovered.attempts, Some(0));
        server.abort();
    }
}
