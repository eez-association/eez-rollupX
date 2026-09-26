//! Optional receipt enrichment. One separate worker keeps L2/Beacon outages
//! from delaying canonical L1 collection; the database FK enforces reorg cleanup.
use alloy_primitives::{B256, U256};
use anyhow::{Context, Result, anyhow, ensure};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio_postgres::NoTls;

use crate::{
    config::Config,
    model::{hash, number},
    rpc::Rpc,
};

pub async fn run(config: Config, database_url: String) {
    loop {
        if let Err(error) = worker(&config, &database_url).await {
            tracing::warn!(%error, "settlement details worker will reconnect");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn worker(config: &Config, url: &str) -> Result<()> {
    let settings = config.details.as_ref().context("details disabled")?;
    let mut pg: tokio_postgres::Config = url.parse()?;
    pg.connect_timeout(Duration::from_secs(10));
    let (client, connection) = pg.connect(NoTls).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute("SET statement_timeout='10s'; SET lock_timeout='5s'")
        .await?;
    let l1 = Rpc::new(settings.l1_http_url.as_deref().unwrap_or(&config.http_url))?;
    let l2 = Rpc::new(&settings.l2_http_url)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    loop {
        if let Err(error) = crate::admission_receipts::refresh(&client, &l1, config).await {
            tracing::warn!(%error, "admission source receipts will retry");
        }
        let rows = client.query("SELECT t.block_hash,t.transaction_hash,t.transaction_index,b.height,b.timestamp,t.blob_gas_used::text,d.data::text,t.gas_used::text,t.gas_price_wei::text FROM eez_indexer.transactions t JOIN eez_indexer.blocks b ON b.hash=t.block_hash LEFT JOIN eez_indexer.details d USING(block_hash,transaction_hash) WHERE d.retry_at IS NULL OR d.retry_at <= now() ORDER BY d.checked_at NULLS FIRST,b.height DESC,t.transaction_index DESC LIMIT 8", &[]).await?;
        // Each future owns a different details row. Dropping the worker cancels
        // all outstanding requests; FK checks discard results orphaned by L1 reorgs.
        let pending = stream::iter(rows).map(|row| async {
            let row = row;

            let block: B256 = row.get::<_, String>(0).parse()?;
            let tx: B256 = row.get::<_, String>(1).parse()?;
            let index = u64::try_from(row.get::<_, i64>(2))?;
            let height = u64::try_from(row.get::<_, i64>(3))?;
            let timestamp = u64::try_from(row.get::<_, i64>(4))?;
            let blob_gas: u64 = row.get::<_, String>(5).parse()?;
            let old: Value =
                serde_json::from_str(row.get::<_, Option<String>>(6).as_deref().unwrap_or("{}"))?;
            let gas_used: u64 = row.get::<_, String>(7).parse()?;
            let mut data = json!({"version":5});
            // Immutable transaction details and successful blob retrieval are
            // reused. Availability means retrieved at the recorded check time.
            data["transaction"] = if old["transaction"]["status"] == "available"
                && old["transaction"].get("gasLimit").is_some()
            {
                old["transaction"].clone()
            } else {
                outcome(
                    async {
                        l1.identity(config).await?;
                        let value = l1.call("eth_getTransactionByHash", json!([tx])).await?;
                        transaction(&value, block, tx, index, blob_gas, gas_used)
                    }
                    .await,
                )
            };
            data["l1Fees"] = if old["l1Fees"]["status"] == "available" {
                old["l1Fees"].clone()
            } else {
                outcome(
                    async {
                        let header = l1.call("eth_getBlockByHash", json!([block, false])).await?;
                        crate::l1_fees::split(
                            &header,
                            block,
                            height,
                            U256::from(gas_used),
                            row.get::<_, String>(8).parse()?,
                        )
                    }
                    .await,
                )
            };
            data["tipReference"] = if old["tipReference"]["status"] == "available" {
                old["tipReference"].clone()
            } else {
                outcome(crate::l1_fees::collect_reference(&l1, height, block).await)
            };
            data["ranges"] = if blob_gas == 0 {
                // Correlations belong to DA postings, not their non-blob execution updates.
                json!({"status":"not_applicable","items":[]})
            } else {
                outcome(
                    async {
                        ensure!(
                            number(&l2.call("eth_chainId", json!([])).await?)?
                                == settings.l2_chain_id
                                && l2.block(0).await?.hash == settings.l2_genesis_hash,
                            "L2 identity mismatch"
                        );
                        let value = l2
                            .call("eez_getSettledL2RangesByL1Block", json!([block]))
                            .await?;
                        ranges(&value, block, tx, index, height)
                    }
                    .await,
                )
            };
            data["l2Fees"] = if blob_gas == 0 {
                json!({"status":"not_applicable","items":[]})
            } else {
                crate::l2_fees::collect(&l2, &data["ranges"], &old["l2Fees"], 128).await
            };
            data["inboundTransactions"] = if old["inboundTransactions"]["status"] == "available" {
                old["inboundTransactions"].clone()
            } else {
                outcome(
                    async {
                        let header = l1.block(height).await?;
                        ensure!(header.hash == block, "inbound source block changed");
                        let receipt = l1.call("eth_getTransactionReceipt", json!([tx])).await?;
                        let inbound = crate::cross_chain::inbound(
                            &receipt,
                            &header,
                            tx,
                            config.registry,
                            config.rollup_id,
                        )?;
                        ensure!(
                            l1.block(height).await?.hash == block,
                            "inbound source block changed"
                        );
                        Ok(json!({"count": u64::from(inbound)}))
                    }
                    .await,
                )
            };
            data["outboundTransactions"] = if blob_gas == 0 {
                json!({"status":"not_applicable","items":[]})
            } else {
                crate::cross_chain::collect(&l2, &data["ranges"], &old["outboundTransactions"])
                    .await
            };
            data["beacon"] = if old["beacon"]["status"] == "available"
                && old["beacon"].get("usedBytes").is_some()
            {
                old["beacon"].clone()
            } else {
                let result = async {
                    let hashes = data["transaction"]["blobHashes"]
                        .as_array()
                        .context("transaction blob hashes pending")?
                        .iter()
                        .map(hash)
                        .collect::<Result<Vec<_>>>()?;
                    beacon(
                        &http,
                        &settings.beacon_url,
                        &l1,
                        block,
                        height,
                        timestamp,
                        &hashes,
                    )
                    .await
                }
                .await;
                match result {
                    Err(error) if old["beacon"]["status"] == "available" => {
                        let mut previous = old["beacon"].clone();
                        previous["usageError"] = json!(error.to_string());
                        previous
                    }
                    result => outcome(result),
                }
            };
            // Finalized ranges and successful retrieval are historical facts.
            // Retry missing details and unfinalized ranges without continually
            // querying the entire finalized history.
            let complete = data["inboundTransactions"]["status"] == "available"
                && (blob_gas == 0 || data["outboundTransactions"]["status"] == "available")
                && data["l1Fees"]["status"] == "available"
                && data["tipReference"]["status"] == "available"
                && (blob_gas == 0 || data["l2Fees"]["status"] == "available")
                && data["transaction"]["status"] == "available"
                && data["beacon"]["status"] == "available"
                && data["beacon"].get("usedBytes").is_some()
                && (blob_gas == 0
                    || data["ranges"]["items"].as_array().is_some_and(|items| {
                        !items.is_empty() && items.iter().all(|r| r["state"] == "finalized")
                    }));
            let retry_seconds = details_retry_seconds(&old, &data);
            let encoded = data.to_string();
            let saved = client.execute("INSERT INTO eez_indexer.details(block_hash,transaction_hash,data,retry_at) VALUES($1,$2,$3::text::jsonb,CASE WHEN $4 THEN 'infinity'::timestamptz ELSE now()+$5::bigint*interval '1 second' END) ON CONFLICT(block_hash,transaction_hash) DO UPDATE SET data=EXCLUDED.data,checked_at=now(),retry_at=EXCLUDED.retry_at", &[&block.to_string(),&tx.to_string(),&encoded,&complete,&retry_seconds]).await;
            if let Err(error) = saved {
                if error.code() == Some(&tokio_postgres::error::SqlState::FOREIGN_KEY_VIOLATION) {
                    return Ok(());
                }
                return Err(error.into());
            }
            Ok::<(), anyhow::Error>(())
        });
        pending.buffer_unordered(4).try_collect::<Vec<_>>().await?;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// State promotion must not discard an exact range's receipt checkpoint. Compare
// every other field so changed hashes/bounds always create a fresh scan.
pub(crate) fn same_range(a: &Value, b: &Value) -> bool {
    let (Some(mut a), Some(mut b)) = (a.as_object().cloned(), b.as_object().cloned()) else {
        return false;
    };
    a.remove("state");
    b.remove("state");
    a == b
}

// Resume bounded fee scans promptly when they made progress. Unavailable data,
// unchanged checkpoints and RPC failures retain the slower retry cadence.
fn details_retry_seconds(old: &Value, new: &Value) -> i64 {
    let progressing = ["l2Fees", "outboundTransactions"].iter().any(|field| {
        new[*field]["status"] == "pending"
            && new[*field]["items"].as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    item["status"] == "pending"
                        && matches!(
                            item["range"]["state"].as_str(),
                            Some("canonical" | "finalized")
                        )
                        && item.get("error").is_none()
                        && item["processed"].as_u64().is_some_and(|processed| {
                            let previous = old[*field]["items"]
                                .as_array()
                                .and_then(|items| {
                                    items.iter().find(|old| old["range"] == item["range"])
                                })
                                .and_then(|old| old["processed"].as_u64())
                                .unwrap_or(0);
                            processed > previous
                        })
                })
            })
    });
    if progressing { 5 } else { 60 }
}

fn outcome(result: Result<Value>) -> Value {
    match result {
        Ok(mut value) => {
            value["status"] = json!("available");
            value
        }
        Err(error) => json!({"status":"pending", "error":error.to_string()}),
    }
}

pub fn transaction(
    value: &Value,
    block: B256,
    tx: B256,
    index: u64,
    blob_gas: u64,
    gas_used: u64,
) -> Result<Value> {
    ensure!(
        hash(&value["hash"])? == tx
            && hash(&value["blockHash"])? == block
            && number(&value["transactionIndex"])? == index,
        "transaction inclusion mismatch"
    );
    let blobs = match value.get("blobVersionedHashes") {
        Some(v) => v
            .as_array()
            .context("invalid blob hashes")?
            .iter()
            .map(hash)
            .collect::<Result<Vec<_>>>()?,
        None => vec![],
    };
    ensure!(
        blobs.len() as u64 * 131_072 == blob_gas,
        "blob hashes disagree with receipt gas"
    );
    ensure!(blobs.iter().all(|h| h[0] == 1), "unsupported blob version");
    let gas_limit = number(&value["gas"])?;
    ensure!(
        gas_limit > 0 && gas_used <= gas_limit,
        "receipt gas exceeds transaction limit"
    );
    Ok(json!({"blobHashes":blobs,"blobCount":blobs.len(),"gasLimit":gas_limit.to_string()}))
}

pub fn ranges(value: &Value, block: B256, tx: B256, index: u64, height: u64) -> Result<Value> {
    let mut result = Vec::new();
    for entry in value
        .as_array()
        .context("invalid L2 correlation response")?
    {
        if hash(&entry["l1TransactionHash"])? != tx {
            continue;
        }
        ensure!(
            hash(&entry["l1BlockHash"])? == block
                && number(&entry["l1BlockNumber"])? == height
                && number(&entry["l1TransactionIndex"])? == index,
            "L2 correlation inclusion mismatch"
        );
        let r = &entry["l2Range"];
        let first = number(&r["firstBlockNumber"])?;
        let last = number(&r["lastBlockNumber"])?;
        let count = number(&r["blockCount"])?;
        ensure!(
            last >= first && last.checked_sub(first).and_then(|n| n.checked_add(1)) == Some(count),
            "invalid L2 range length"
        );
        let blocks = entry["l2Blocks"]
            .as_array()
            .context("missing L2 block list")?;
        ensure!(
            blocks.len() as u64 == count && count > 0,
            "incomplete L2 block list"
        );
        for (i, b) in blocks.iter().enumerate() {
            ensure!(
                number(&b["number"])? == first + i as u64,
                "noncontiguous L2 range"
            );
            hash(&b["hash"])?;
        }
        let first_hash = hash(&r["firstBlockHash"])?;
        let last_hash = hash(&r["lastBlockHash"])?;
        ensure!(
            hash(&blocks[0]["hash"])? == first_hash
                && hash(&blocks[blocks.len() - 1]["hash"])? == last_hash,
            "L2 range endpoint mismatch"
        );
        let canonical = entry["canonicalL2"]
            .as_bool()
            .context("missing L2 canonical status")?;
        let finalized = entry["l2Finalized"]
            .as_bool()
            .context("missing L2 finality")?;
        ensure!(!finalized || canonical, "noncanonical finalized range");
        result.push(json!({"first":first,"last":last,"count":count,"firstHash":first_hash,"lastHash":last_hash,"state":if finalized {"finalized"} else if canonical {"canonical"} else {"non-canonical"}}));
    }
    ensure!(!result.is_empty(), "L2 range not indexed yet");
    Ok(json!({"items":result}))
}

async fn get(
    http: &reqwest::Client,
    base: &str,
    path: &str,
) -> Result<(reqwest::StatusCode, Value)> {
    let mut response = http
        .get(format!("{}/{path}", base.trim_end_matches('/')))
        .send()
        .await
        .map_err(|_| anyhow!("Beacon transport failed"))?;
    let status = response.status();
    if !status.is_success() {
        return Ok((status, Value::Null));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Beacon body failed"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 32 * 1024 * 1024,
            "Beacon response exceeds 32 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, serde_json::from_slice(&bytes)?))
}

async fn beacon(
    http: &reqwest::Client,
    base: &str,
    l1: &Rpc,
    block: B256,
    height: u64,
    timestamp: u64,
    transaction_hashes: &[B256],
) -> Result<Value> {
    if transaction_hashes.is_empty() {
        return Ok(json!({"source":"no blobs", "blobCount":0,"usedBytes":0,"capacityBytes":0}));
    }
    let (_, genesis) = get(http, base, "eth/v1/beacon/genesis").await?;
    let (_, spec) = get(http, base, "eth/v1/config/spec").await?;
    let genesis: u64 = genesis["data"]["genesis_time"]
        .as_str()
        .context("missing Beacon genesis")?
        .parse()?;
    let seconds: u64 = spec["data"]["SECONDS_PER_SLOT"]
        .as_str()
        .context("missing Beacon slot duration")?
        .parse()?;
    ensure!(
        seconds > 0 && timestamp >= genesis && (timestamp - genesis).is_multiple_of(seconds),
        "invalid Beacon slot timestamp"
    );
    let slot = (timestamp - genesis) / seconds;
    // Bind this Beacon source to the exact L1 execution block, not merely a slot.
    let (_, beacon_block) = get(http, base, &format!("eth/v2/beacon/blocks/{slot}")).await?;
    ensure!(
        hash(&beacon_block["data"]["message"]["body"]["execution_payload"]["block_hash"])? == block,
        "Beacon execution block mismatch"
    );
    let (status, mut response) = get(http, base, &format!("eth/v1/beacon/blobs/{slot}")).await?;
    let mut source = "full-blobs";
    if matches!(status.as_u16(), 404 | 405 | 501) {
        let (status, value) =
            get(http, base, &format!("eth/v1/beacon/blob_sidecars/{slot}")).await?;
        ensure!(status.is_success(), "Beacon blobs unavailable");
        response = value;
        source = "blob-sidecars";
    } else {
        ensure!(status.is_success(), "Beacon blobs unavailable");
    }
    let canonical = l1
        .call(
            "eth_getBlockByNumber",
            json!([format!("0x{height:x}"), false]),
        )
        .await?;
    ensure!(
        hash(&canonical["hash"])? == block,
        "L1 changed during Beacon check"
    );
    let count = validate_blobs(&response, number(&canonical["blobGasUsed"])?)?;
    let usage = select_transaction_blobs(
        &response,
        &beacon_block["data"]["message"]["body"]["blob_kzg_commitments"],
        transaction_hashes,
    )
    .and_then(|selected| blob_usage(&selected));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let mut result = json!({"slot":slot,"source":source,"blobCount":count,"checkedAt":now});
    match usage {
        Ok((used, capacity)) => {
            result["usedBytes"] = json!(used);
            result["capacityBytes"] = json!(capacity);
        }
        Err(error) => {
            result["usageError"] = json!(error.to_string());
        }
    }
    Ok(result)
}

/// Match only this transaction's blobs, in transaction order. Beacon may
/// return other transactions' blobs or sidecars in a different index order.
pub fn select_transaction_blobs(
    response: &Value,
    commitments: &Value,
    wanted: &[B256],
) -> Result<Vec<Vec<u8>>> {
    let commitments = commitments
        .as_array()
        .context("missing Beacon commitments")?;
    let blobs = response["data"]
        .as_array()
        .context("missing Beacon blobs")?;
    ensure!(
        commitments.len() == blobs.len(),
        "Beacon commitments/blob count mismatch"
    );
    let hashes = commitments
        .iter()
        .map(|c| {
            let bytes = alloy_primitives::hex::decode(c.as_str().context("invalid commitment")?)?;
            ensure!(bytes.len() == 48, "invalid commitment size");
            let mut digest: [u8; 32] = Sha256::digest(&bytes).into();
            digest[0] = 1;
            Ok(B256::from(digest))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut ordered = vec![None; blobs.len()];
    for (position, blob) in blobs.iter().enumerate() {
        let index = if blob.is_string() {
            position
        } else {
            blob["index"]
                .as_str()
                .context("missing sidecar index")?
                .parse::<usize>()?
        };
        ensure!(
            index < ordered.len() && ordered[index].is_none(),
            "invalid or duplicate sidecar index"
        );
        ordered[index] = Some(blob);
    }
    wanted
        .iter()
        .map(|wanted| {
            let index = hashes
                .iter()
                .position(|h| h == wanted)
                .context("transaction blob missing from Beacon commitments")?;
            let blob = ordered[index].context("transaction blob missing from Beacon response")?;
            let text = blob
                .as_str()
                .or_else(|| blob["blob"].as_str())
                .context("missing blob bytes")?;
            Ok(alloy_primitives::hex::decode(text)?)
        })
        .collect()
}

/// Measure the occupied prefix of an accepted EEZ blob stream, not nonzero
/// bytes. BLOB_FORMAT_SPEC sections 1/4 require 31 reversed bytes per field,
/// version zero, a final nonzero CloseBlobStream marker (1), then zero padding.
/// This measurement does not decode inner ChainOperation payloads or verify KZG.
pub fn blob_usage(blobs: &[Vec<u8>]) -> Result<(usize, usize)> {
    if blobs.is_empty() {
        return Ok((0, 0));
    }
    let mut stream: Vec<u8> = Vec::with_capacity(blobs.len() * 126_976);
    for blob in blobs {
        ensure!(blob.len() == 131_072, "invalid physical blob length");
        for field in blob.chunks_exact(32) {
            ensure!(field[0] == 0, "nonzero reserved blob field byte");
            stream.extend(field[1..].iter().rev());
        }
    }
    ensure!(stream[0] == 0, "unsupported EEZ stream version");
    let end = stream
        .iter()
        .rposition(|b| *b != 0)
        .context("empty EEZ stream")?;
    ensure!(stream[end] == 1, "EEZ stream missing final close marker");
    Ok((end + 1, stream.len()))
}

pub fn validate_blobs(value: &Value, block_blob_gas: u64) -> Result<usize> {
    let blobs = value["data"].as_array().context("missing Beacon blobs")?;
    ensure!(
        !blobs.is_empty() && blobs.len() as u64 * 131_072 == block_blob_gas,
        "Beacon blob count mismatch"
    );
    for blob in blobs {
        let text = blob
            .as_str()
            .or_else(|| blob["blob"].as_str())
            .context("missing blob bytes")?;
        ensure!(
            text.starts_with("0x")
                && text.len() == 131_072 * 2 + 2
                && text[2..].bytes().all(|b| b.is_ascii_hexdigit()),
            "malformed Beacon blob"
        );
    }
    Ok(blobs.len())
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    fn progress(processed: u64) -> Value {
        json!({"l2Fees":{"status":"pending","items":[{
            "status":"pending","range":{"first":1,"last":7905,"state":"finalized"},
            "processed":processed
        }]}})
    }

    #[test]
    fn successful_bounded_scans_continue_promptly() {
        assert_eq!(details_retry_seconds(&json!({}), &progress(128)), 5);
        assert_eq!(details_retry_seconds(&progress(128), &progress(256)), 5);
        let outbound = json!({"outboundTransactions": progress(8192)["l2Fees"]});
        assert_eq!(details_retry_seconds(&json!({}), &outbound), 5);
        assert_eq!(details_retry_seconds(&outbound, &outbound), 60);
        let mut failed = outbound.clone();
        failed["outboundTransactions"]["items"][0]["error"] = json!("RPC unavailable");
        assert_eq!(details_retry_seconds(&json!({}), &failed), 60);
    }

    #[test]
    fn stalls_and_errors_keep_backoff() {
        let old = progress(128);
        assert_eq!(details_retry_seconds(&old, &old), 60);
        let mut failed = progress(256);
        failed["l2Fees"]["items"][0]["error"] = json!("RPC unavailable");
        assert_eq!(details_retry_seconds(&old, &failed), 60);
        assert_eq!(details_retry_seconds(&old, &json!({})), 60);
        let mut unfinalized = progress(256);
        unfinalized["l2Fees"]["items"][0]["range"]["state"] = json!("safe");
        assert_eq!(details_retry_seconds(&old, &unfinalized), 60);
    }
}
