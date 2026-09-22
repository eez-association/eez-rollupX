//! Unique successful source transactions, derived from canonical protocol receipts.
use crate::{
    model::{Block, hash, number},
    rpc::Rpc,
};
use alloy_primitives::{Address, B256, address, keccak256};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MANAGER: Address = address!("4200000000000000000000000000000000000007");
const L1_CONSUMED: &str = "ExecutionConsumed(bytes32,uint64,uint256)";
const L2_CONSUMED: &str = "ExecutionConsumed(bytes32,uint256)";
const INCOMING: &str =
    "IncomingCrossChainCallExecuted(bytes32,bool,address,uint64,address,uint256,uint64,bytes)";

fn topics(log: &Value) -> Result<Vec<B256>> {
    log["topics"]
        .as_array()
        .context("missing log topics")?
        .iter()
        .map(hash)
        .collect()
}

fn checked_logs<'a>(receipt: &'a Value, block: &Block, tx: B256) -> Result<Vec<&'a Value>> {
    ensure!(
        hash(&receipt["blockHash"])? == block.hash
            && number(&receipt["blockNumber"])? == block.number
            && hash(&receipt["transactionHash"])? == tx,
        "cross-chain receipt inclusion mismatch"
    );
    let index = number(&receipt["transactionIndex"])?;
    ensure!(
        block.transactions.get(usize::try_from(index)?) == Some(&tx),
        "cross-chain transaction index mismatch"
    );
    ensure!(
        number(&receipt["status"])? == 1,
        "cross-chain receipt failed"
    );
    let logs = receipt["logs"].as_array().context("missing receipt logs")?;
    let mut previous = None;
    for log in logs {
        let position = number(&log["logIndex"])?;
        ensure!(
            previous.is_none_or(|last| position > last),
            "duplicate or unordered receipt logs"
        );
        previous = Some(position);
        ensure!(
            log.get("removed").is_none_or(|v| v == false)
                && hash(&log["blockHash"])? == block.hash
                && number(&log["blockNumber"])? == block.number
                && hash(&log["transactionHash"])? == tx
                && number(&log["transactionIndex"])? == index,
            "cross-chain log inclusion mismatch"
        );
    }
    Ok(logs.iter().collect())
}

pub fn inbound(
    receipt: &Value,
    block: &Block,
    tx: B256,
    registry: Address,
    rollup: u64,
) -> Result<bool> {
    let mut found = false;
    for log in checked_logs(receipt, block, tx)? {
        let emitter: Address = log["address"]
            .as_str()
            .context("missing emitter")?
            .parse()?;
        if emitter != registry {
            continue;
        }
        let t = topics(log)?;
        if t.first() == Some(&keccak256(L1_CONSUMED)) {
            ensure!(
                t.len() == 4 && log["data"] == "0x",
                "malformed L1 consumption log"
            );
            // Zero-key entries execute outbound L2 transactions, not inbound sources.
            if t[1] != B256::ZERO && t[2] == B256::from(alloy_primitives::U256::from(rollup)) {
                found = true;
            }
        }
    }
    Ok(found)
}

pub fn outbound(receipt: &Value, block: &Block, tx: B256) -> Result<bool> {
    let mut consumed = false;
    let mut incoming = false;
    for log in checked_logs(receipt, block, tx)? {
        let emitter: Address = log["address"]
            .as_str()
            .context("missing emitter")?
            .parse()?;
        if emitter != MANAGER {
            continue;
        }
        let t = topics(log)?;
        if t.first() == Some(&keccak256(L2_CONSUMED)) {
            ensure!(
                t.len() == 3 && log["data"] == "0x",
                "malformed L2 consumption log"
            );
            consumed = true;
        }
        // Inbound system deliveries may make nested outbound calls; they are not
        // additional source transactions. Only this system-only entry emits it.
        if t.first() == Some(&keccak256(INCOMING)) {
            incoming = true;
        }
    }
    Ok(consumed && !incoming)
}

pub async fn collect(rpc: &Rpc, ranges: &Value, old: &Value) -> Value {
    let Some(ranges) = ranges["items"].as_array() else {
        return json!({"status":"pending","items":[]});
    };
    let mut items = Vec::new();
    let mut budget = 4usize;
    for range in ranges {
        let cached = old["items"].as_array().and_then(|items| {
            items.iter().find(|item| {
                crate::details::same_range(&item["range"], range) && item["processed"].is_u64()
            })
        });
        let mut item = cached.cloned().unwrap_or_else(
            || json!({"range":range,"status":"pending","processed":0,"transactions":[]}),
        );
        // A canonical result is provisional: validate its hash again even if
        // fully collected. Finality promotion reuses receipts, not stale status.
        let revalidate = range["state"] != "finalized"
            || cached.is_none_or(|c| c["range"]["state"] != "finalized");
        item["range"] = range.clone();
        if !matches!(range["state"].as_str(), Some("canonical" | "finalized")) {
            item = json!({"range":range,"status":"pending","error":"L2 range not canonical"});
        } else if (item["status"] != "available" || revalidate)
            && let Err(error) = advance(rpc, range, &mut item, &mut budget).await
        {
            item["status"] = json!("pending");
            item["error"] = json!(error.to_string());
        }
        items.push(item);
    }
    let complete = !items.is_empty() && items.iter().all(|i| i["status"] == "available");
    json!({"status":if complete {"available"} else {"pending"},"items":items})
}

async fn advance(rpc: &Rpc, range: &Value, item: &mut Value, budget: &mut usize) -> Result<()> {
    let first = range["first"].as_u64().context("missing range start")?;
    let last = range["last"].as_u64().context("missing range end")?;
    let count = range["count"].as_u64().context("missing range count")?;
    ensure!(
        last.checked_sub(first).and_then(|n| n.checked_add(1)) == Some(count) && count > 0,
        "invalid cross-chain range"
    );
    ensure!(
        rpc.block(first).await?.hash == hash(&range["firstHash"])?
            && rpc.block(last).await?.hash == hash(&range["lastHash"])?,
        "cross-chain range changed"
    );
    let mut processed = item["processed"].as_u64().unwrap_or(0);
    ensure!(processed <= count, "cross-chain checkpoint outside range");
    let mut transactions: BTreeSet<B256> = item["transactions"]
        .as_array()
        .context("missing cross-chain checkpoint transactions")?
        .iter()
        .map(hash)
        .collect::<Result<_>>()?;
    while processed < count && *budget > 0 {
        *budget -= 1;
        let start = first.checked_add(processed).context("range overflow")?;
        let end = start.saturating_add(2047).min(last);
        let endpoint = rpc.block(end).await?;
        let response = rpc.call("eth_getLogs",json!([{"address":MANAGER,"topics":[keccak256(L2_CONSUMED)],"fromBlock":format!("0x{start:x}"),"toBlock":format!("0x{end:x}")}])).await?;
        let logs = response.as_array().context("invalid cross-chain logs")?;
        let mut headers = BTreeMap::new();
        let mut receipts = BTreeMap::new();
        let mut observed = BTreeSet::new();
        let mut found = BTreeSet::new();
        for log in logs {
            ensure!(
                log["address"]
                    .as_str()
                    .context("missing emitter")?
                    .parse::<Address>()?
                    == MANAGER
                    && topics(log)?.first() == Some(&keccak256(L2_CONSUMED)),
                "unexpected cross-chain log"
            );
            ensure!(
                log.get("removed").is_none_or(|v| v == false),
                "removed cross-chain log"
            );
            let height = number(&log["blockNumber"])?;
            ensure!(
                (start..=end).contains(&height),
                "cross-chain log outside requested range"
            );
            ensure!(
                observed.insert((height, number(&log["logIndex"])?)),
                "duplicate cross-chain log"
            );
            let tx = hash(&log["transactionHash"])?;
            if let std::collections::btree_map::Entry::Vacant(entry) = headers.entry(height) {
                entry.insert(rpc.block(height).await?);
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = receipts.entry(tx) {
                entry.insert(rpc.call("eth_getTransactionReceipt", json!([tx])).await?);
            }
            let header = &headers[&height];
            let receipt = &receipts[&tx];
            let outbound = outbound(receipt, header, tx)?;
            let receipt_logs = receipt["logs"].as_array().context("missing receipt logs")?;
            ensure!(
                receipt_logs.iter().any(|r| [
                    "address",
                    "topics",
                    "data",
                    "blockHash",
                    "blockNumber",
                    "transactionHash",
                    "transactionIndex",
                    "logIndex"
                ]
                .iter()
                .all(|key| r[*key] == log[*key])),
                "queried cross-chain log differs from receipt"
            );
            if outbound {
                found.insert(tx);
            }
        }
        ensure!(
            rpc.block(end).await?.hash == endpoint.hash
                && rpc.block(last).await?.hash == hash(&range["lastHash"])?,
            "cross-chain range changed during collection"
        );
        // Commit a whole verified chunk together; failed retries cannot add partial counts.
        transactions.extend(found);
        processed = end - first + 1;
        item["processed"] = json!(processed);
        item["transactions"] = json!(transactions);
    }
    if processed == count {
        item["status"] = json!("available");
    }
    item.as_object_mut()
        .context("invalid cross-chain checkpoint")?
        .remove("error");
    Ok(())
}
