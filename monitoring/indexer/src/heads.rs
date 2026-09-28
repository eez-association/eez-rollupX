//! Current RPC heads, independent of historical enrichment and scanner progress.
use crate::{
    config::Config,
    model::{Block, number},
    rpc::Rpc,
};
use alloy_primitives::B256;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_postgres::NoTls;

/// Read finalized first and latest last so normal head advancement is allowed.
/// Height reads pin all three observations to the node's canonical branch.
pub async fn collect(rpc: &Rpc, chain_id: u64, genesis: B256) -> Result<Value> {
    ensure!(
        number(&rpc.call("eth_chainId", json!([])).await?)? == chain_id
            && rpc.genesis_hash().await? == genesis,
        "head source identity mismatch"
    );
    let mut blocks = Vec::new();
    for tag in ["finalized", "safe", "latest"] {
        blocks.push(Block::parse(
            &rpc.call("eth_getBlockByNumber", json!([tag, false]))
                .await?,
        )?);
    }
    ensure!(
        blocks[0].number <= blocks[1].number && blocks[1].number <= blocks[2].number,
        "inconsistent head ordering"
    );
    for block in &blocks {
        ensure!(
            rpc.block(block.number).await?.hash == block.hash,
            "head branch changed"
        );
    }
    Ok(
        json!({"latest":blocks[2].number,"safe":blocks[1].number,"finalized":blocks[0].number,
        "latestHash":blocks[2].hash,"safeHash":blocks[1].hash,"finalizedHash":blocks[0].hash,
        "unsettled":blocks[2].number-blocks[1].number}),
    )
}

async fn sample(
    client: &tokio_postgres::Client,
    chain: &str,
    rpc: &Rpc,
    id: u64,
    genesis: B256,
) -> Result<()> {
    let started = Instant::now();
    // Clear failed samples immediately; do not carry an old success forward.
    let data = match tokio::time::timeout(Duration::from_secs(10), collect(rpc, id, genesis)).await
    {
        Ok(Ok(data)) => data,
        _ => json!({}),
    }
    .to_string();
    let elapsed = started.elapsed().as_secs_f64();
    client.execute("INSERT INTO eez_indexer.network_heads(chain,data,observed_at) VALUES($1,$2::text::jsonb,now()-$3::double precision*interval '1 second') ON CONFLICT(chain) DO UPDATE SET data=EXCLUDED.data,observed_at=EXCLUDED.observed_at", &[&chain,&data,&elapsed]).await?;
    Ok(())
}

async fn worker(config: &Config, url: &str) -> Result<()> {
    let mut settings: tokio_postgres::Config = url.parse()?;
    settings.connect_timeout(Duration::from_secs(10));
    let (client, connection) = settings.connect(NoTls).await?;
    let connection = tokio::spawn(async move {
        let _ = connection.await;
    });
    let result = async {
        client
            .batch_execute("SET statement_timeout='10s'; SET lock_timeout='5s'")
            .await?;
        let l1 = Rpc::new(&config.http_url)?;
        let l2 = config
            .details
            .as_ref()
            .map(|d| Rpc::new(&d.l2_http_url))
            .transpose()?;
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let a = sample(&client, "l1", &l1, config.chain_id, config.genesis_hash);
            let b = async {
                if let (Some(rpc), Some(d)) = (&l2, &config.details) {
                    sample(&client, "l2", rpc, d.l2_chain_id, d.l2_genesis_hash).await?;
                }
                Ok::<_, anyhow::Error>(())
            };
            let (a, b) = tokio::join!(a, b);
            a?;
            b?;
        }
    }
    .await;
    connection.abort();
    result
}

pub async fn run(config: Config, url: String) {
    loop {
        if worker(&config, &url).await.is_err() {
            tracing::warn!("network head sampler disconnected; retrying");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
