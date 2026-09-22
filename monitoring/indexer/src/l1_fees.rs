//! Historical execution-cost split and a preceding-block inclusion-price reference.
use alloy_primitives::{B256, U256};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    model::{Block, number, quantity},
    rpc::Rpc,
};

pub fn split(header: &Value, block: B256, height: u64, gas: U256, price: U256) -> Result<Value> {
    let parsed = Block::parse(header)?;
    ensure!(
        parsed.hash == block && parsed.number == height,
        "L1 fee block mismatch"
    );
    let base = quantity(&header["baseFeePerGas"])?;
    let tip = price
        .checked_sub(base)
        .context("effective price below L1 base fee")?;
    Ok(
        json!({"baseFeeWei": base.to_string(), "tipPerGasWei": tip.to_string(),
        "baseCostWei": base.checked_mul(gas).context("base fee overflow")?.to_string(),
        "tipCostWei": tip.checked_mul(gas).context("tip fee overflow")?.to_string()}),
    )
}

pub fn reference(history: &Value, end: u64, count: u64) -> Result<Value> {
    ensure!(count > 0 && count <= 20, "invalid fee reference window");
    let start = end
        .checked_sub(count - 1)
        .context("fee reference before genesis")?;
    ensure!(
        number(&history["oldestBlock"])? == start,
        "fee history window mismatch"
    );
    let rewards = history["reward"]
        .as_array()
        .context("missing fee rewards")?;
    let ratios = history["gasUsedRatio"]
        .as_array()
        .context("missing gas ratios")?;
    ensure!(
        rewards.len() == count as usize && ratios.len() == rewards.len(),
        "incomplete fee history"
    );
    let mut tips = Vec::new();
    for (reward, ratio) in rewards.iter().zip(ratios) {
        let row = reward.as_array().context("invalid fee reward")?;
        ensure!(row.len() == 1, "unexpected fee percentiles");
        let tip = quantity(&row[0])?;
        let ratio = ratio.as_f64().context("invalid gas ratio")?;
        ensure!(
            ratio.is_finite() && (0.0..=1.0).contains(&ratio),
            "invalid gas ratio"
        );
        // Empty blocks have no included-transaction tip; zero-tip nonempty blocks do.
        if ratio > 0.0 {
            tips.push(tip);
        }
    }
    tips.sort_unstable();
    let median = match tips.len() {
        0 => None,
        n if n % 2 == 1 => Some(tips[n / 2]),
        n => Some(
            tips[n / 2 - 1]
                .checked_add(tips[n / 2])
                .context("median overflow")?
                / U256::from(2),
        ),
    };
    Ok(
        json!({"firstBlock":start,"lastBlock":end,"sampleBlocks":tips.len(),
        "percentile":50,"medianTipWei":median.map(|value|value.to_string())}),
    )
}

pub async fn collect_reference(rpc: &Rpc, height: u64, block: B256) -> Result<Value> {
    ensure!(height > 0, "no preceding L1 block");
    ensure!(
        rpc.block(height).await?.hash == block,
        "fee reference branch changed"
    );
    let count = height.min(20);
    let history = rpc
        .call(
            "eth_feeHistory",
            json!([format!("0x{count:x}"), format!("0x{:x}", height - 1), [50]]),
        )
        .await?;
    let result = reference(&history, height - 1, count)?;
    ensure!(
        rpc.block(height).await?.hash == block,
        "fee reference branch changed"
    );
    Ok(result)
}
