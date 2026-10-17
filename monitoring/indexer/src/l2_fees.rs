//! Receipt-based L2 execution fees. Checkpoint bounded canonical-range scans in
//! the existing details row; never expose a partially collected sum as a total.
use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, U256};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    model::{Block, hash, number, quantity},
    rpc::Rpc,
};

// Matches the pinned native transaction profile in eez-proof-signer/SPEC.md.
// The monitoring workspace deliberately has no node/Reth dependencies.
const NATIVE_TRANSACTION_TYPE: u64 = 0x76;
const ACCOUNTING_VERSION: u64 = 1;

/// Execution fees in the L2 network's native base units, plus receipt gas split
/// by fee-paying ordinary transactions and zero-priced native transactions.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockFees {
    pub beneficiary: Address,
    pub ordinary_gas_used: u64,
    pub native_gas_used: u64,
    pub burn: U256,
    /// Gross priority payments to the header beneficiary, including operator
    /// transactions. This is not qualified external revenue or L1 liquidity.
    pub tip: U256,
}

/// Account for a complete block's execution receipts. Native type 0x76 consumes
/// gas at zero price; ordinary transactions pay fees even when they revert.
/// Reject missing/unsupported types, nonzero native prices, ordinary prices below
/// the base fee, arithmetic overflow, and inconsistent inclusion or gas evidence.
pub fn block_fees(header: &Value, receipts: &Value) -> Result<BlockFees> {
    let block = Block::parse(header)?;
    let base = quantity(&header["baseFeePerGas"])?;
    let beneficiary = header["miner"]
        .as_str()
        .context("missing L2 fee beneficiary")?
        .parse()?;
    let receipts = receipts.as_array().context("missing L2 receipts")?;
    ensure!(
        receipts.len() == block.transactions.len(),
        "incomplete L2 receipts"
    );
    let mut gas = 0u64;
    let mut fees = BlockFees {
        beneficiary,
        ordinary_gas_used: 0,
        native_gas_used: 0,
        burn: U256::ZERO,
        tip: U256::ZERO,
    };
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
        let transaction_type = number(&receipt["type"])?;
        if transaction_type == NATIVE_TRANSACTION_TYPE {
            ensure!(price.is_zero(), "nonzero native L2 transaction price");
            fees.native_gas_used = fees
                .native_gas_used
                .checked_add(used)
                .context("L2 native gas overflow")?;
            continue;
        }
        ensure!(transaction_type <= 4, "unsupported L2 receipt type");
        fees.ordinary_gas_used = fees
            .ordinary_gas_used
            .checked_add(used)
            .context("L2 ordinary gas overflow")?;
        let priority = price
            .checked_sub(base)
            .context("L2 effective price below base fee")?;
        fees.burn = fees
            .burn
            .checked_add(
                base.checked_mul(U256::from(used))
                    .context("L2 burn overflow")?,
            )
            .context("L2 burn overflow")?;
        fees.tip = fees
            .tip
            .checked_add(
                priority
                    .checked_mul(U256::from(used))
                    .context("L2 tip overflow")?,
            )
            .context("L2 tip overflow")?;
    }
    ensure!(gas == number(&header["gasUsed"])?, "L2 block gas mismatch");
    Ok(fees)
}

#[derive(Default)]
struct RangeFees {
    burn: U256,
    tip: U256,
    ordinary_gas_used: U256,
    native_gas_used: U256,
    tips_by_beneficiary: BTreeMap<Address, U256>,
}

impl RangeFees {
    fn read(item: &Value) -> Result<Self> {
        let mut tips_by_beneficiary = BTreeMap::new();
        let mut attributed = U256::ZERO;
        for (address, amount) in item["tipsByBeneficiary"]
            .as_object()
            .context("missing L2 fee beneficiary checkpoint")?
        {
            let amount = decimal(amount)?;
            ensure!(
                tips_by_beneficiary
                    .insert(address.parse()?, amount)
                    .is_none(),
                "duplicate L2 fee beneficiary"
            );
            attributed = attributed
                .checked_add(amount)
                .context("L2 beneficiary tip overflow")?;
        }
        let tip = decimal(&item["tipWei"])?;
        ensure!(attributed == tip, "L2 beneficiary tips do not match total");
        Ok(Self {
            burn: decimal(&item["burnWei"])?,
            tip,
            ordinary_gas_used: decimal(&item["ordinaryGasUsed"])?,
            native_gas_used: decimal(&item["nativeGasUsed"])?,
            tips_by_beneficiary,
        })
    }

    fn add(&mut self, fees: BlockFees) -> Result<()> {
        self.burn = self
            .burn
            .checked_add(fees.burn)
            .context("L2 range burn overflow")?;
        self.tip = self
            .tip
            .checked_add(fees.tip)
            .context("L2 range tip overflow")?;
        self.ordinary_gas_used = self
            .ordinary_gas_used
            .checked_add(U256::from(fees.ordinary_gas_used))
            .context("L2 range ordinary gas overflow")?;
        self.native_gas_used = self
            .native_gas_used
            .checked_add(U256::from(fees.native_gas_used))
            .context("L2 range native gas overflow")?;
        if !fees.tip.is_zero() {
            let recipient = self
                .tips_by_beneficiary
                .entry(fees.beneficiary)
                .or_default();
            *recipient = recipient
                .checked_add(fees.tip)
                .context("L2 beneficiary tip overflow")?;
        }
        Ok(())
    }

    fn write(&self, item: &mut Value) {
        item["burnWei"] = json!(self.burn.to_string());
        item["tipWei"] = json!(self.tip.to_string());
        item["ordinaryGasUsed"] = json!(self.ordinary_gas_used.to_string());
        item["nativeGasUsed"] = json!(self.native_gas_used.to_string());
        item["tipsByBeneficiary"] = json!(
            self.tips_by_beneficiary
                .iter()
                .map(|(address, amount)| (format!("{address:#x}"), amount.to_string()))
                .collect::<BTreeMap<_, _>>()
        );
    }
}

fn decimal(value: &Value) -> Result<U256> {
    let text = value.as_str().context("missing decimal fee checkpoint")?;
    ensure!(
        !text.is_empty()
            && text.len() <= 78
            && (text == "0" || !text.starts_with('0'))
            && text.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid decimal fee checkpoint"
    );
    Ok(U256::from_str_radix(text, 10)?)
}

fn new_checkpoint(range: &Value) -> Value {
    let mut item =
        json!({"version": ACCOUNTING_VERSION, "range": range, "status": "pending", "processed": 0});
    RangeFees::default().write(&mut item);
    item
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
                crate::details::same_range(&item["range"], range)
                    && item["version"] == ACCOUNTING_VERSION
                    && item["processed"].is_u64()
            })
        });
        // Older totals cannot establish beneficiary attribution or native gas.
        // Rescan the whole range instead of attaching guessed fields to them.
        let mut item = cached.cloned().unwrap_or_else(|| new_checkpoint(range));
        // A canonical result is provisional: validate its hash again even if
        // fully collected. Finality promotion reuses receipts, not stale status.
        let revalidate = range["state"] != "finalized"
            || cached.is_none_or(|c| c["range"]["state"] != "finalized");
        let complete = item["status"] == "available" && item["processed"] == range["count"];
        item["range"] = range.clone();
        if !matches!(range["state"].as_str(), Some("canonical" | "finalized")) {
            item = json!({"range":range,"status":"pending","error":"L2 range not canonical"});
        } else if (!complete || revalidate)
            && let Err(error) = advance(rpc, range, &mut item, &mut remaining).await
        {
            item["status"] = json!("pending");
            item["error"] = json!(error.to_string());
        } else if let Err(error) = RangeFees::read(&item) {
            item["status"] = json!("pending");
            item["error"] = json!(error.to_string());
        }
        items.push(item);
    }
    let complete = !items.is_empty() && items.iter().all(|item| item["status"] == "available");
    json!({"status":if complete {"available"} else {"pending"},"items":items})
}

async fn advance(rpc: &Rpc, range: &Value, item: &mut Value, budget: &mut usize) -> Result<()> {
    // Availability is re-established only after the complete range is checked.
    item["status"] = json!("pending");
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
            *item = new_checkpoint(range);
            anyhow::bail!("L2 fee checkpoint changed; restarting range");
        }
        Some(checkpoint.hash)
    } else {
        None
    };
    let mut fees = RangeFees::read(item)?;
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
        fees.add(block_fees(&header, &receipts)?)?;
        // A failed request never advances this checkpoint or doubles a block on retry.
        processed += 1;
        parent = Some(block.hash);
        item["processed"] = json!(processed);
        item["lastHash"] = json!(block.hash);
        fees.write(item);
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
