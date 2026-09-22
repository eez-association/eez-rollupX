//! Receipt-based L2 execution fees. Checkpoint bounded canonical-range scans in
//! the existing details row; never expose a partially collected sum as a total.
use alloy_primitives::{B256, U256};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    model::{Block, hash, number, quantity},
    rpc::Rpc,
};

pub fn block_fees(header: &Value, receipts: &Value) -> Result<(U256, U256)> {
    let block = Block::parse(header)?;
    let base = quantity(&header["baseFeePerGas"])?;
    let receipts = receipts.as_array().context("missing L2 receipts")?;
    ensure!(
        receipts.len() == block.transactions.len(),
        "incomplete L2 receipts"
    );
    let mut gas = 0u64;
    let mut burn = U256::ZERO;
    let mut tip = U256::ZERO;
    for (index, receipt) in receipts.iter().enumerate() {
        ensure!(
            hash(&receipt["blockHash"])? == block.hash
                && number(&receipt["blockNumber"])? == block.number
                && number(&receipt["transactionIndex"])? == index as u64
                && hash(&receipt["transactionHash"])? == block.transactions[index],
            "L2 receipt inclusion mismatch"
        );
        ensure!(
            number(&receipt["status"])? <= 1,
            "invalid L2 receipt status"
        );
        let used = number(&receipt["gasUsed"])?;
        gas = gas.checked_add(used).context("L2 gas overflow")?;
        ensure!(
            number(&receipt["cumulativeGasUsed"])? == gas,
            "L2 cumulative gas mismatch"
        );
        let price = quantity(&receipt["effectiveGasPrice"])?;
        let priority = price
            .checked_sub(base)
            .context("L2 effective price below base fee")?;
        burn = burn
            .checked_add(
                base.checked_mul(U256::from(used))
                    .context("L2 burn overflow")?,
            )
            .context("L2 burn overflow")?;
        tip = tip
            .checked_add(
                priority
                    .checked_mul(U256::from(used))
                    .context("L2 tip overflow")?,
            )
            .context("L2 tip overflow")?;
    }
    ensure!(gas == number(&header["gasUsed"])?, "L2 block gas mismatch");
    Ok((burn, tip))
}

pub async fn collect(rpc: &Rpc, ranges: &Value, old: &Value, budget: usize) -> Value {
    let Some(ranges) = ranges["items"].as_array() else {
        let mut previous = old.clone();
        if !previous.is_object() {
            previous = json!({});
        }
        previous["status"] = json!("pending");
        return previous;
    };
    let mut remaining = budget;
    let mut items = Vec::new();
    for range in ranges {
        let cached = old["items"].as_array().and_then(|items| {
            items.iter().find(|item| {
                crate::details::same_range(&item["range"], range) && item["processed"].is_u64()
            })
        });
        let mut item = cached.cloned().unwrap_or_else(
            || json!({"range":range,"status":"pending","processed":0,"burnWei":"0","tipWei":"0"}),
        );
        // A canonical result is provisional: validate its hash again even if
        // fully collected. Finality promotion reuses receipts, not stale status.
        let revalidate = range["state"] != "finalized"
            || cached.is_none_or(|c| c["range"]["state"] != "finalized");
        item["range"] = range.clone();
        if !matches!(range["state"].as_str(), Some("canonical" | "finalized")) {
            item = json!({"range":range,"status":"pending","error":"L2 range not canonical"});
        } else if (item["status"] != "available" || revalidate)
            && let Err(error) = advance(rpc, range, &mut item, &mut remaining).await
        {
            item["status"] = json!("pending");
            item["error"] = json!(error.to_string());
        }
        items.push(item);
    }
    let complete = !items.is_empty() && items.iter().all(|item| item["status"] == "available");
    json!({"status":if complete {"available"} else {"pending"},"items":items})
}

async fn advance(rpc: &Rpc, range: &Value, item: &mut Value, budget: &mut usize) -> Result<()> {
    let first = range["first"].as_u64().context("missing range start")?;
    let last = range["last"].as_u64().context("missing range end")?;
    let count = range["count"].as_u64().context("missing range length")?;
    ensure!(
        last.checked_sub(first).and_then(|n| n.checked_add(1)) == Some(count) && count > 0,
        "invalid fee range"
    );
    let mut processed = item["processed"].as_u64().unwrap_or(0);
    ensure!(processed <= count, "invalid fee checkpoint");
    let endpoint = rpc.block(last).await?;
    ensure!(
        endpoint.hash == hash(&range["lastHash"])?,
        "L2 fee endpoint changed"
    );
    let mut parent: Option<B256> = if processed > 0 {
        let checkpoint = rpc.block(first + processed - 1).await?;
        if checkpoint.hash != hash(&item["lastHash"])? {
            *item =
                json!({"range":range,"status":"pending","processed":0,"burnWei":"0","tipWei":"0"});
            anyhow::bail!("L2 fee checkpoint changed; restarting range");
        }
        Some(checkpoint.hash)
    } else {
        None
    };
    let mut burn = U256::from_str_radix(
        item["burnWei"]
            .as_str()
            .context("missing burn checkpoint")?,
        10,
    )?;
    let mut tip = U256::from_str_radix(
        item["tipWei"].as_str().context("missing tip checkpoint")?,
        10,
    )?;
    while processed < count && *budget > 0 {
        *budget -= 1;
        let height = first + processed;
        let header = rpc
            .call(
                "eth_getBlockByNumber",
                json!([format!("0x{height:x}"), false]),
            )
            .await?;
        let block = Block::parse(&header)?;
        ensure!(block.number == height, "wrong L2 fee block height");
        if let Some(parent) = parent {
            ensure!(block.parent == parent, "L2 fee parent mismatch");
        }
        if processed == 0 {
            ensure!(
                block.hash == hash(&range["firstHash"])?,
                "L2 fee start changed"
            );
        }
        if height == last {
            ensure!(block.hash == endpoint.hash, "L2 fee end changed");
        }
        let receipts = if block.transactions.is_empty() {
            json!([])
        } else {
            rpc.call("eth_getBlockReceipts", json!([block.hash]))
                .await?
        };
        let (block_burn, block_tip) = block_fees(&header, &receipts)?;
        let next_burn = burn
            .checked_add(block_burn)
            .context("L2 range burn overflow")?;
        let next_tip = tip
            .checked_add(block_tip)
            .context("L2 range tip overflow")?;
        // A failed request never advances this checkpoint or doubles a block on retry.
        burn = next_burn;
        tip = next_tip;
        processed += 1;
        parent = Some(block.hash);
        item["processed"] = json!(processed);
        item["lastHash"] = json!(block.hash);
        item["burnWei"] = json!(burn.to_string());
        item["tipWei"] = json!(tip.to_string());
    }
    if processed == count {
        ensure!(
            rpc.block(last).await?.hash == endpoint.hash,
            "L2 fee range changed during collection"
        );
        item["status"] = json!("available");
    }
    item.as_object_mut().unwrap().remove("error");
    Ok(())
}
