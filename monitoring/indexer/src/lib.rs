pub mod config;
pub mod cross_chain;
pub mod database;
pub mod details;
pub mod heads;
pub mod l1_fees;
pub mod l2_fees;
pub mod model;
pub mod postings;
pub mod rpc;

use alloy_primitives::{B256, U256};
use alloy_sol_types::SolEvent;
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::collections::BTreeMap;

use crate::{
    config::Config,
    database::Database,
    model::{Event, L2ExecutionPerformed, Receipt, number},
    rpc::Rpc,
};

/// Advance at most 128 blocks. The caller loops immediately while catching up;
/// cancellation/reconnects do not require a second indexing implementation.
pub async fn sync(config: &Config, rpc: &Rpc, db: &mut Database) -> Result<bool> {
    rpc.identity(config).await?;
    let head = number(&rpc.call("eth_blockNumber", json!([])).await?)?;
    if let Some((tip, tip_hash)) = db.cursor().await?
        && (tip > head || rpc.block(tip).await?.hash != tip_hash)
    {
        let mut ancestor = tip.min(head);
        while ancestor >= config.start_block {
            if db.hash(ancestor).await? == Some(rpc.block(ancestor).await?.hash) {
                break;
            }
            ancestor -= 1;
        }
        // Confirm the chosen branch before changing persistent history.
        if ancestor >= config.start_block {
            ensure!(
                db.hash(ancestor).await? == Some(rpc.block(ancestor).await?.hash),
                "chain changed during reorg search"
            );
        }
        db.rewind(ancestor).await?;
        tracing::info!(ancestor, "rewound canonical settlement history");
    }
    let from = db
        .cursor()
        .await?
        .map_or(config.start_block, |(n, _)| n + 1);
    if from > head {
        db.checked(head).await?;
        return Ok(false);
    }
    let to = head.min(from.saturating_add(127));
    let range_end = rpc.block(to).await?;
    let logs = rpc.call("eth_getLogs", json!([{
        "address":config.registry, "topics":[L2ExecutionPerformed::SIGNATURE_HASH,B256::from(U256::from(config.rollup_id))],
        "fromBlock":format!("0x{from:x}"),"toBlock":format!("0x{to:x}")
    }])).await?;
    let mut by_block: BTreeMap<u64, Vec<Event>> = BTreeMap::new();
    for log in logs.as_array().context("getLogs did not return an array")? {
        let event = Event::parse(log, config)?;
        ensure!(
            (from..=to).contains(&event.block_number),
            "log outside requested range"
        );
        by_block.entry(event.block_number).or_default().push(event);
    }
    // Pin the range endpoint. Even blocks with no returned logs must not be
    // checkpointed across a reorg that changed the queried range's history.
    for height in from..=to {
        let block = rpc.block(height).await?;
        let mut events = by_block.remove(&height).unwrap_or_default();
        events.sort_by_key(|e| e.log_index);
        ensure!(
            events.windows(2).all(|w| w[0].log_index < w[1].log_index),
            "duplicate settlement log"
        );
        let mut by_tx: BTreeMap<B256, Vec<Event>> = BTreeMap::new();
        for event in &events {
            event.validate_block(&block)?;
            by_tx
                .entry(event.transaction)
                .or_default()
                .push(event.clone());
        }
        let mut receipts = Vec::new();
        for (hash, tx_events) in by_tx {
            let value = rpc.call("eth_getTransactionReceipt", json!([hash])).await?;
            receipts.push(Receipt::parse(&value, &block, &tx_events, config)?);
        }
        ensure!(
            rpc.block(height).await?.hash == block.hash
                && rpc.block(to).await?.hash == range_end.hash,
            "chain changed during collection"
        );
        db.store(&block, &events, &receipts, config.start_block)
            .await?;
    }
    db.checked(head).await?;
    tracing::info!(from, to, head, "indexed canonical L1 range");
    Ok(to < head)
}
