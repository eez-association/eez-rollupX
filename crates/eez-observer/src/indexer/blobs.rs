//! Optional bounded Beacon enrichment; canonical receipt scanning owns chain
//! state and remains independent of historical blob availability.
use super::{
    Block, Config, Error, Rpc, SharedStatus, Transaction,
    ledger::{self, Deadline},
    remote_block,
};
use crate::{
    blobs::{Metadata, RESPONSE_BYTES},
    database::{Database, DatabaseConfig},
    rpc::GetResponse,
};
use alloy_primitives::B256;
use serde::Deserialize;
use serde_json::Value;
use std::{
    fmt,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Semaphore;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobConfig {
    pub beacon_url: String,
}
impl fmt::Debug for BlobConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlobConfig").finish_non_exhaustive()
    }
}
impl BlobConfig {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        let url = reqwest::Url::parse(&self.beacon_url).map_err(|_| "invalid blob Beacon URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || url.fragment().is_some()
        {
            return Err("blob Beacon URL must be HTTP(S) without a fragment");
        }
        Ok(())
    }
    fn url(&self, path: &str) -> Result<reqwest::Url, &'static str> {
        let mut url = reqwest::Url::parse(&self.beacon_url).map_err(|_| "invalid_metadata")?;
        let base = url.path().trim_end_matches('/');
        url.set_path(&format!("{base}/{path}"));
        Ok(url)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    last_success: Option<u64>,
    error: Option<Error>,
    counts: Vec<(String, i64)>,
    utilization: Option<f64>,
    coverage: Option<f64>,
    last_retrieval: Option<(f64, i64)>,
}
pub fn render(state: Option<&Status>, now: u64) -> String {
    use std::fmt::Write;
    let Some(state) = state else {
        return String::new();
    };
    let fresh = state.error.is_none()
        && state
            .last_success
            .is_some_and(|at| at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000);
    let mut result = format!("eez_observer_blob_collection_success {}\n", u8::from(fresh));
    if let Some(at) = state.last_success {
        writeln!(
            result,
            "eez_observer_blob_last_success_timestamp_seconds {}",
            at / 1000
        )
        .unwrap();
    }
    if let Some(error) = state.error {
        writeln!(
            result,
            "eez_observer_blob_error{{reason=\"{}\"}} 1",
            error.label()
        )
        .unwrap();
    }
    if fresh {
        for (name, count) in &state.counts {
            writeln!(
                result,
                "eez_observer_blob_posts{{state=\"{name}\"}} {count}"
            )
            .unwrap();
        }
        if let Some(value) = state.utilization {
            writeln!(
                result,
                "eez_observer_blob_purchased_capacity_utilization {value}"
            )
            .unwrap();
        }
        if let Some(value) = state.coverage {
            writeln!(result, "eez_observer_blob_measurement_coverage {value}").unwrap();
        }
        if let Some((seconds, at)) = state.last_retrieval {
            writeln!(
                result,
                "eez_observer_blob_retrieval_last_duration_seconds {seconds}"
            )
            .unwrap();
            writeln!(
                result,
                "eez_observer_blob_retrieval_last_attempt_timestamp_seconds {at}"
            )
            .unwrap();
        }
    }
    result
}
pub(super) async fn run(config: &Config, rpc: &Rpc, status: &SharedStatus) {
    if config.blobs.is_none() {
        return;
    }
    let mut failures = 0u32;
    loop {
        failures = if poll(config, rpc, status).await {
            0
        } else {
            failures.saturating_add(1)
        };
        tokio::time::sleep(Duration::from_secs(if failures == 0 {
            5
        } else {
            crate::retry_delay(failures)
        }))
        .await;
    }
}
pub(super) async fn poll(config: &Config, rpc: &Rpc, status: &SharedStatus) -> bool {
    if config.blobs.is_none() {
        return true;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), cycle(config, rpc))
        .await
        .unwrap_or(Err(Error::Rpc(crate::rpc::Failure::Transport)));
    match result {
        Ok(state) => {
            status.write().await.blobs = Some(state);
            true
        }
        Err(error) => {
            status
                .write()
                .await
                .blobs
                .get_or_insert_with(Status::default)
                .error = Some(error);
            tracing::warn!(reason = error.label(), "blob measurement collection failed");
            false
        }
    }
}
fn decimal(value: &Value) -> Result<u64, &'static str> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .ok_or("invalid_response")
}
async fn json(
    rpc: &Rpc,
    config: &BlobConfig,
    path: &str,
    limit: usize,
) -> Result<GetResponse, &'static str> {
    rpc.get_bounded(config.url(path)?, limit)
        .await
        .map_err(crate::rpc::Failure::label)
}
async fn present(rpc: &Rpc, config: &BlobConfig, path: &str) -> Result<Value, &'static str> {
    match json(rpc, config, path, 1024 * 1024).await? {
        GetResponse::Value(value) => Ok(value),
        GetResponse::Absent => Err("unavailable"),
        GetResponse::Pruned => Err("pruned"),
    }
}
async fn measure(
    rpc: &Rpc,
    config: &BlobConfig,
    timestamp: u64,
    metadata: Metadata,
) -> Result<(u64, &'static str), &'static str> {
    if !metadata.supported() {
        return Err("unsupported_carrier");
    }
    // Fetch timing per pass: a changed/misconfigured endpoint must not keep a
    // stale cached genesis. Authentication ultimately binds bytes to EL hashes.
    let genesis = present(rpc, config, "eth/v1/beacon/genesis").await?;
    let spec = present(rpc, config, "eth/v1/config/spec").await?;
    let genesis = decimal(&genesis["data"]["genesis_time"])?;
    let seconds = decimal(&spec["data"]["SECONDS_PER_SLOT"])?;
    let elapsed = timestamp.checked_sub(genesis).ok_or("identity_mismatch")?;
    if seconds == 0 || !elapsed.is_multiple_of(seconds) {
        return Err("identity_mismatch");
    }
    let slot = elapsed / seconds;
    let (value, legacy) = match json(
        rpc,
        config,
        &format!("eth/v1/beacon/blobs/{slot}"),
        RESPONSE_BYTES,
    )
    .await?
    {
        GetResponse::Value(value) => (value, false),
        GetResponse::Pruned => return Err("pruned"),
        GetResponse::Absent => match json(
            rpc,
            config,
            &format!("eth/v1/beacon/blob_sidecars/{slot}"),
            RESPONSE_BYTES,
        )
        .await?
        {
            GetResponse::Value(value) => (value, true),
            GetResponse::Pruned => return Err("pruned"),
            GetResponse::Absent => return Err("unavailable"),
        },
    };
    // A cancelled async caller must not release the CPU permit while KZG work
    // continues on the blocking pool. At most one such task exists per process.
    static CPU: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = CPU
        .get_or_init(|| Arc::new(Semaphore::new(1)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| "interrupted")?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crate::blobs::measure(value, legacy, &metadata)
    })
    .await
    .map_err(|_| "interrupted")??;
    Ok((
        bytes,
        if legacy {
            "beacon_sidecars"
        } else {
            "beacon_blobs"
        },
    ))
}
fn hash(row: &tokio_postgres::Row, index: usize) -> Result<B256, Error> {
    Ok(B256::from(
        <[u8; 32]>::try_from(row.try_get::<_, Vec<u8>>(index)?).map_err(|_| Error::Inconsistent)?,
    ))
}
async fn cycle(config: &Config, rpc: &Rpc) -> Result<Status, Error> {
    let Some(settings) = config.blobs.as_ref() else {
        return Ok(Status::default());
    };
    let database = DatabaseConfig::load(&config.database_config).map_err(|_| Error::Schema)?;
    let mut db = Database::connect(&database)
        .await
        .map_err(|_| Error::Database)?;
    db.verify_writer().await.map_err(|_| Error::Schema)?;
    let endpoint = &config.exporter.l1;
    let chain = ledger::find_chain(&db, config, endpoint, "l1")
        .await?
        .ok_or(Error::Bootstrap)?;
    rpc.validate_identity(endpoint).await?;
    let result = collect(&mut db, config, rpc, settings, chain).await;
    if result == Err(Error::ConflictingFact) {
        ledger::mark_failure(&db, chain, Error::ConflictingFact).await?;
    }
    result?;
    let counts=db.client.query("SELECT measurement_state,count(*) FROM eez.posting_blob_measurements WHERE chain_instance_id=$1 AND source_valid GROUP BY measurement_state ORDER BY measurement_state",&[&chain]).bounded().await?.iter().map(|row| Ok((row.try_get(0)?,row.try_get(1)?))).collect::<Result<_,Error>>()?;
    let ratios=db.client.query_opt("SELECT purchased_capacity_utilization::float8,measurement_coverage::float8 FROM eez.posting_blob_totals WHERE chain_instance_id=$1 AND source_valid",&[&chain]).bounded().await?;
    let last_retrieval=db.client.query_opt("SELECT retrieval_duration_ns::float8/1000000000,extract(epoch FROM last_attempt_at)::bigint FROM eez.blob_attempts WHERE chain_instance_id=$1 AND retrieval_duration_ns IS NOT NULL ORDER BY last_attempt_at DESC,transaction_hash LIMIT 1",&[&chain]).bounded().await?.map(|row| -> Result<_,Error> {Ok((row.try_get(0)?,row.try_get(1)?))}).transpose()?;
    Ok(Status {
        last_success: Some(crate::now_ms()),
        last_retrieval,
        error: None,
        counts,
        utilization: ratios.as_ref().map(|r| r.try_get(0)).transpose()?.flatten(),
        coverage: ratios.as_ref().map(|r| r.try_get(1)).transpose()?.flatten(),
    })
}
async fn collect(
    db: &mut Database,
    config: &Config,
    rpc: &Rpc,
    settings: &BlobConfig,
    chain: i64,
) -> Result<(), Error> {
    let endpoint = &config.exporter.l1;
    let cursor = ledger::cursor(db, chain).await?.ok_or(Error::Bootstrap)?;
    {
        let transaction = db.client.transaction().bounded().await?;
        ledger::lock_cursor(&transaction, chain, Some(&cursor)).await?;
        transaction.commit().bounded().await?;
    }
    // Measured content is immutable and never fetched again. Missing/pruned
    // historical rows rotate persistently with new posts; no one gap blocks all.
    let rows=db.client.query("WITH candidates AS (SELECT p.transaction_hash,p.block_hash,p.block_number,p.transaction_index,p.timestamp_seconds,p.calldata_hash,p.purchased_blobs,a.last_attempt_at FROM eez.posting_receipts p LEFT JOIN eez.blob_attempts a USING(chain_instance_id,transaction_hash) WHERE p.chain_instance_id=$1 AND p.transaction_type=3 AND NOT EXISTS(SELECT FROM eez.blob_measurements measured WHERE measured.chain_instance_id=p.chain_instance_id AND measured.transaction_hash=p.transaction_hash)) SELECT transaction_hash::bytea,block_hash::bytea,block_number::text,transaction_index::text,timestamp_seconds::text,calldata_hash::bytea,purchased_blobs::text FROM ((SELECT *,true AS repair FROM candidates WHERE last_attempt_at IS NOT NULL ORDER BY last_attempt_at,block_number,transaction_index LIMIT 1) UNION ALL (SELECT *,false AS repair FROM candidates WHERE last_attempt_at IS NULL ORDER BY block_number,transaction_index LIMIT 3)) scheduled ORDER BY repair DESC,block_number,transaction_index",&[&chain]).bounded().await?;
    for row in rows {
        let tx_hash = hash(&row, 0)?;
        let block_hash = hash(&row, 1)?;
        let calldata_hash = hash(&row, 5)?;
        let number = row
            .try_get::<_, String>(2)?
            .parse()
            .map_err(|_| Error::Inconsistent)?;
        let position: usize = row
            .try_get::<_, String>(3)?
            .parse()
            .map_err(|_| Error::Inconsistent)?;
        let timestamp = row
            .try_get::<_, String>(4)?
            .parse()
            .map_err(|_| Error::Inconsistent)?;
        let purchased: Option<String> = row.try_get(6)?;
        db.client.execute("INSERT INTO eez.blob_attempts(chain_instance_id,transaction_hash,state) VALUES($1,$2::bytea,'interrupted') ON CONFLICT(chain_instance_id,transaction_hash) DO UPDATE SET last_attempt_at=now(),state='interrupted',retrieval_duration_ns=NULL",&[&chain,&tx_hash.as_slice()]).bounded().await?;
        let result = match rpc.transaction(endpoint, tx_hash).await {
            Ok(raw) if raw.is_null() => Err("metadata_unavailable"),
            Ok(raw) => {
                // This decoder binds input/type/position to the already indexed
                // receipt; it never moves the canonical scan cursor.
                let block = Block {
                    number,
                    hash: block_hash,
                    parent: B256::ZERO,
                    timestamp,
                };
                match Transaction::parse(&raw, &block, position) {
                    Ok(tx) if tx.hash == tx_hash && tx.kind == 3 => {
                        let recipient = tx.recipient.map(|address| address.as_slice().to_vec());
                        let matches: bool = db.client.query_one("SELECT sender=$3::bytea AND recipient IS NOT DISTINCT FROM $4::bytea AND nonce=$5::text::numeric AND transaction_type=$6 AND method_selector IS NOT DISTINCT FROM $7::bytea FROM eez.transactions WHERE chain_instance_id=$1 AND hash=$2::bytea", &[&chain,&tx_hash.as_slice(),&tx.sender.as_slice(),&recipient,&tx.nonce.to_string(),&tx.kind,&tx.selector]).bounded().await?.try_get(0)?;
                        if !matches {
                            return Err(Error::ConflictingFact);
                        }
                        Metadata::parse(&raw, calldata_hash)
                    }
                    _ => Err("invalid_metadata"),
                }
            }
            Err(error) => Err(error.label()),
        };
        let mut metadata_fact = None;
        let mut retrieval_ns = None;
        let result = match result {
            Ok(metadata) => {
                if purchased
                    .as_ref()
                    .is_some_and(|value| value != &metadata.hashes.len().to_string())
                {
                    return Err(Error::ConflictingFact);
                }
                let hashes: Vec<Vec<u8>> = metadata
                    .hashes
                    .iter()
                    .map(|hash| hash.as_slice().to_vec())
                    .collect();
                let indices: Vec<String> =
                    metadata.indices.iter().map(ToString::to_string).collect();
                metadata_fact = Some((hashes, indices, metadata.tail_hash));
                let count =
                    i32::try_from(metadata.hashes.len()).map_err(|_| Error::Inconsistent)?;
                let started = tokio::time::Instant::now();
                let result = measure(rpc, settings, timestamp, metadata).await;
                let duration =
                    u64::try_from(started.elapsed().as_nanos()).map_err(|_| Error::Inconsistent)?;
                retrieval_ns = Some(duration.to_string());
                result.map(|(bytes, via)| (bytes, via, count))
            }
            Err(error) => Err(error),
        };
        let cursor = ledger::cursor(db, chain).await?.ok_or(Error::Bootstrap)?;
        rpc.validate_identity(endpoint).await?;
        if remote_block(rpc, endpoint, cursor.number).await?.hash != cursor.hash
            || remote_block(rpc, endpoint, number).await?.hash != block_hash
        {
            return Err(Error::Inconsistent);
        }
        let transaction = db.client.transaction().bounded().await?;
        ledger::lock_cursor(&transaction, chain, Some(&cursor)).await?;
        let canonical:bool=transaction.query_one("SELECT canonical AND number=$3::text::numeric FROM eez.blocks WHERE chain_instance_id=$1 AND hash=$2::bytea",&[&chain,&block_hash.as_slice(),&number.to_string()]).bounded().await?.try_get(0)?;
        if !canonical {
            return Err(Error::Inconsistent);
        }
        if let Some((hashes, indices, tail_hash)) = metadata_fact {
            transaction.execute("INSERT INTO eez.blob_metadata(chain_instance_id,transaction_hash,calldata_hash,blob_hashes,blob_indices,calldata_tail_hash) VALUES($1,$2::bytea,$3::bytea,$4::bytea[]::eez.hash32[],$5::text[]::numeric[]::eez.u256[],$6::bytea) ON CONFLICT(chain_instance_id,transaction_hash) DO UPDATE SET calldata_hash=EXCLUDED.calldata_hash,blob_hashes=EXCLUDED.blob_hashes,blob_indices=EXCLUDED.blob_indices,calldata_tail_hash=EXCLUDED.calldata_tail_hash", &[&chain,&tx_hash.as_slice(),&calldata_hash.as_slice(),&hashes,&indices,&tail_hash.as_slice()]).bounded().await?;
        }
        if let Ok((bytes, via, count)) = result {
            transaction.execute("INSERT INTO eez.blob_measurements(chain_instance_id,transaction_hash,codec_version,encoded_bytes,purchased_blobs,usable_bytes_per_blob,retrieval) VALUES($1,$2::bytea,0,$3::text::numeric,$4,126976,$5) ON CONFLICT(chain_instance_id,transaction_hash) DO UPDATE SET encoded_bytes=EXCLUDED.encoded_bytes,purchased_blobs=EXCLUDED.purchased_blobs",&[&chain,&tx_hash.as_slice(),&bytes.to_string(),&count,&via]).bounded().await?;
        }
        let state = result.err().unwrap_or("measured");
        transaction.execute("UPDATE eez.blob_attempts SET state=$3,retrieval_duration_ns=$4::text::numeric WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea",&[&chain,&tx_hash.as_slice(),&state,&retrieval_ns]).bounded().await?;
        transaction.commit().bounded().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn optional_config_validates_before_runtime_and_keeps_urls_private() {
        let mut raw = json!({"exporter":{"l1":{"url":"http://localhost:1","chain_id":1,"genesis_hash":format!("0x{}","01".repeat(32))},"l2":{"url":"http://localhost:2","chain_id":2,"genesis_hash":format!("0x{}","02".repeat(32))}},"environment":"devnet","deployment":"fixture","rollup_id":7,"database_config":"/private/writer.json","l1":{"bootstrap":0,"include_all_transactions":true},"l2":{"bootstrap":0,"include_all_transactions":true}});
        let config: Config = serde_json::from_value(raw.clone()).unwrap();
        assert!(config.blobs.is_none());
        assert!(config.validate().is_ok());
        raw["blobs"] = json!({"beacon_url":"http://fixture.invalid/private?token=not-for-logs"});
        let config: Config = serde_json::from_value(raw.clone()).unwrap();
        assert!(config.validate().is_err());
        raw["l1"]["contracts"] = json!([format!("0x{}", "bb".repeat(20))]);
        let config: Config = serde_json::from_value(raw.clone()).unwrap();
        assert!(config.validate().is_ok());
        assert!(!format!("{config:?}").contains("not-for-logs"));
        let blob = config.blobs.unwrap();
        assert_eq!(
            blob.url("eth/v1/config/spec").unwrap().as_str(),
            "http://fixture.invalid/private/eth/v1/config/spec?token=not-for-logs"
        );
        for url in ["", "file:///tmp/blob", "http://host/#fragment"] {
            raw["blobs"]["beacon_url"] = json!(url);
            assert!(
                serde_json::from_value::<Config>(raw.clone())
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
    #[test]
    fn unknown_utilization_is_absent_and_stale_measurements_are_hidden() {
        let mut state = Status {
            last_success: Some(100_000),
            counts: vec![("unavailable".into(), 2)],
            ..Default::default()
        };
        let text = render(Some(&state), 100_000);
        assert!(text.contains("state=\"unavailable\"} 2"));
        assert!(!text.contains("utilization"));
        state.last_retrieval = Some((0.25, 100));
        assert!(render(Some(&state), 100_000).contains("retrieval_last_duration_seconds 0.25"));
        assert!(
            render(Some(&state), 100_000).contains("retrieval_last_attempt_timestamp_seconds 100")
        );
        state.utilization = Some(0.5);
        state.coverage = Some(0.25);
        assert!(render(Some(&state), 100_000).contains("purchased_capacity_utilization 0.5"));
        assert!(!render(Some(&state), 190_001).contains("utilization"));
        state.error = Some(Error::Frozen);
        assert!(!render(Some(&state), 100_000).contains("blob_posts"));
    }
    #[tokio::test]
    async fn bounded_beacon_get_distinguishes_unavailability_pruning_and_oversize() {
        use axum::{Router, http::StatusCode, routing::get};
        let app = Router::new()
            .route("/value", get(|| async { "{\"data\":[]}" }))
            .route("/absent", get(|| async { StatusCode::NOT_FOUND }))
            .route("/gone", get(|| async { StatusCode::GONE }))
            .route("/large", get(|| async { "a".repeat(100) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let rpc = Rpc::new().unwrap();
        let url = |path| reqwest::Url::parse(&format!("http://{address}/{path}")).unwrap();
        assert!(matches!(
            rpc.get_bounded(url("value"), 64).await,
            Ok(GetResponse::Value(_))
        ));
        assert!(matches!(
            rpc.get_bounded(url("absent"), 64).await,
            Ok(GetResponse::Absent)
        ));
        assert!(matches!(
            rpc.get_bounded(url("gone"), 64).await,
            Ok(GetResponse::Pruned)
        ));
        assert!(matches!(
            rpc.get_bounded(url("large"), 64).await,
            Err(crate::rpc::Failure::Oversized)
        ));
        task.abort();
    }
}
