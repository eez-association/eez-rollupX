//! Canonical companion receipt observations, including sources whose reverted
//! protocol call emitted no registry logs. A receipt is not proof of consumption.
use alloy_primitives::B256;
use anyhow::{Context, Result, ensure};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use tokio_postgres::Client;

use crate::{
    config::Config,
    model::{Block, hash, number},
    rpc::Rpc,
};

#[derive(Debug, PartialEq, Eq)]
pub struct Observation {
    pub coverage: &'static str,
    pub gas: Option<u64>,
    pub success: Option<bool>,
}

/// Validate inclusion against the exact canonical posting block and position.
/// Missing receipts for included sources are retryable errors. Only a complete
/// block transaction list can establish absence, never a null receipt response.
pub fn observe(
    block: &Block,
    posting: B256,
    source: B256,
    receipt: Option<&Value>,
) -> Result<Observation> {
    let post_index = block
        .transactions
        .iter()
        .position(|tx| *tx == posting)
        .context("posting missing from canonical block")?;
    let Some(index) = block.transactions.iter().position(|tx| *tx == source) else {
        return Ok(Observation {
            coverage: "absent",
            gas: None,
            success: None,
        });
    };
    if index <= post_index {
        return Ok(Observation {
            coverage: "before_postbatch",
            gas: None,
            success: None,
        });
    }
    let receipt = receipt.context("included source receipt unavailable")?;
    ensure!(
        hash(&receipt["blockHash"])? == block.hash
            && number(&receipt["blockNumber"])? == block.number,
        "source receipt block mismatch"
    );
    ensure!(
        hash(&receipt["transactionHash"])? == source
            && number(&receipt["transactionIndex"])? == index as u64,
        "source receipt position mismatch"
    );
    let status = number(&receipt["status"])?;
    ensure!(status <= 1, "invalid source receipt status");
    Ok(Observation {
        coverage: "available",
        gas: Some(number(&receipt["gasUsed"])?),
        success: Some(status == 1),
    })
}

/// At most 32 source observations, with four concurrent RPC sequences. The
/// existing details worker owns cancellation and retries. FK deletion prevents
/// a late response from recreating an observation after its posting reorgs out.
pub async fn refresh(client: &Client, rpc: &Rpc, config: &Config) -> Result<()> {
    // Start with canonical posting links, then inspect only each candidate's
    // source events. Joining the dashboard views here aggregates the complete
    // admission history repeatedly and can starve enrichment at its SQL timeout.
    // Keep the views' latest-estimate / earliest-selection direction precedence.
    let rows = client.query(r#"
        WITH canonical AS MATERIALIZED (
            SELECT link.data->'detail'->>'candidate_id' AS candidate_id,
                   p.canonical_l1_hash,p.transaction_hash,p.canonical_l1_block
            FROM eez_indexer.posting_events link
            JOIN eez_indexer.posting_attempts p USING(attempt_id)
            WHERE link.data->>'stage'='prepared'
              AND link.data->'detail'->>'candidate_id' IS NOT NULL
              AND p.canonical_l1_hash IS NOT NULL
        )
        SELECT DISTINCT c.canonical_l1_hash,c.transaction_hash,c.canonical_l1_block,s.source_hash
        FROM canonical c
        JOIN eez_indexer.transactions t ON t.block_hash=c.canonical_l1_hash AND t.transaction_hash=c.transaction_hash
        JOIN LATERAL (
            SELECT data->'detail'->>'source_hash' AS source_hash
            FROM eez_indexer.admission_events WHERE candidate_id=c.candidate_id
              AND data->>'kind' IN ('source_selected','source_estimate','source_accepted')
            GROUP BY data->'detail'->>'source_hash'
            HAVING bool_or(data->>'kind'='source_accepted') AND coalesce(
                (array_agg(data->'detail'->>'direction' ORDER BY sequence DESC) FILTER(WHERE data->>'kind'='source_estimate'))[1],
                (array_agg(data->'detail'->>'direction' ORDER BY sequence) FILTER(WHERE data->>'kind'='source_selected'))[1]
            )='inbound'
        ) s ON true
        LEFT JOIN eez_indexer.admission_source_receipts r ON r.block_hash=t.block_hash AND r.postbatch_hash=t.transaction_hash AND r.source_hash=s.source_hash
        WHERE r.source_hash IS NULL
        ORDER BY c.canonical_l1_block DESC,s.source_hash LIMIT 32
    "#, &[]).await?;
    if rows.is_empty() {
        return Ok(());
    }
    rpc.identity(config).await?;
    let pending = stream::iter(rows).map(|row| async move {
        let block_hash: B256 = row.get::<_, String>(0).parse()?;
        let post: B256 = row.get::<_, String>(1).parse()?;
        let height = u64::try_from(row.get::<_, i64>(2))?;
        let source: B256 = row.get::<_, String>(3).parse()?;
        let block = rpc.block(height).await?;
        ensure!(block.hash == block_hash, "source posting block changed");
        let receipt = if block.transactions.contains(&source) {
            Some(rpc.call("eth_getTransactionReceipt", json!([source])).await?)
        } else { None };
        let observation = observe(&block, post, source, receipt.as_ref())?;
        ensure!(rpc.block(height).await?.hash == block_hash, "source posting block changed");
        let gas = observation.gas.map(|gas| gas.to_string());
        let saved = client.execute(
            "INSERT INTO eez_indexer.admission_source_receipts(block_hash,postbatch_hash,source_hash,coverage,gas_used,success) \
             VALUES($1,$2,$3,$4,$5::text::numeric,$6) ON CONFLICT DO NOTHING",
            &[&block_hash.to_string(),&post.to_string(),&source.to_string(),&observation.coverage,&gas,&observation.success]).await;
        match saved {
            Err(error) if error.code() == Some(&tokio_postgres::error::SqlState::FOREIGN_KEY_VIOLATION) => Ok(()),
            result => result.map(|_| ()).map_err(Into::into),
        }
    });
    // Finish independent observations even if one RPC is temporarily missing.
    let results: Vec<Result<()>> = pending.buffer_unordered(4).collect().await;
    for result in results {
        result?;
    }
    Ok(())
}
