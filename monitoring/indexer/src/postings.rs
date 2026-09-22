//! Optional replay of the dedicated Composer JSON log directory. File offsets
//! and events commit together; the read-only source is never truncated/deleted.
use crate::config::Config;
use alloy_primitives::{Address, B256};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
    time::Duration,
};
use tokio_postgres::{Client, NoTls};

const MAX_LINE: usize = 64 * 1024;
const MAX_BATCH: u64 = 256 * 1024;
const STAGES: &[&str] = &[
    "prepared",
    "repriced",
    "dispatched",
    "included",
    "excluded",
    "uncertain",
    "preflight_failed",
    "preparation_failed",
];
const REASONS: &[&str] = &[
    "general",
    "pure_always",
    "pure_interval",
    "cross_chain",
    "blob_fullness",
    "drain",
    "window_limit",
];

/// Ignore unrelated JSON logs, but never silently skip an invalid lifecycle event.
pub fn parse(line: &[u8], config: &Config) -> Result<Option<Value>> {
    let log: Value = serde_json::from_slice(line).context("invalid JSON log line")?;
    let fields = log.get("fields").unwrap_or(&log);
    if !fields
        .get("event_name")
        .and_then(Value::as_str)
        .is_some_and(|s| s.starts_with("eez.composer.posting."))
    {
        return Ok(None);
    }
    ensure!(
        fields["event_name"] == "eez.composer.posting.v1",
        "unsupported posting event schema"
    );
    let data: Value = serde_json::from_str(
        fields["posting_event"]
            .as_str()
            .context("missing posting_event")?,
    )?;
    ensure!(
        data["version"].as_u64() == Some(1),
        "unsupported posting event version"
    );
    // Directory configuration is trusted, but a wrong mount must fail explicitly.
    ensure!(
        data["chain_id"].as_u64() == Some(config.chain_id)
            && data["rollup_id"].as_u64() == Some(config.rollup_id)
            && data["registry"]
                .as_str()
                .context("missing registry")?
                .parse::<Address>()?
                == config.registry,
        "posting event network/rollup mismatch"
    );
    let id = data["attempt_id"].as_str().context("missing attempt ID")?;
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b':' || b == b'-'),
        "invalid attempt ID"
    );
    for field in ["posting_id", "transaction_hash"] {
        if field == "transaction_hash"
            && data["stage"] == "preparation_failed"
            && data[field].is_null()
        {
            continue;
        }
        ensure!(
            data[field]
                .as_str()
                .context("missing hash")?
                .parse::<B256>()?
                != B256::ZERO,
            "zero posting hash"
        );
    }
    let stage = data["stage"].as_str().context("missing stage")?;
    ensure!(STAGES.contains(&stage), "invalid posting stage");
    ensure!(
        data["sequence"].as_u64().is_some_and(|n| n <= 4),
        "invalid event sequence"
    );
    ensure!(
        data["timestamp_ms"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= 253_402_300_799_999)
            && data["sync_height"]
                .as_u64()
                .is_some_and(|n| n <= i64::MAX as u64),
        "invalid event time/height"
    );
    ensure!(
        data["elapsed_ms"]
            .as_f64()
            .is_some_and(|n| n.is_finite() && n >= 0.0),
        "invalid elapsed time"
    );
    ensure!(
        data.get("dispatch_elapsed_ms")
            .is_some_and(|v| v.is_null() || v.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)),
        "invalid dispatch duration"
    );
    ensure!(
        data.get("trigger_reasons").is_some_and(|v| v.is_null()
            || v.as_array().is_some_and(|a| a.len() <= 7
                && a.iter()
                    .all(|r| r.as_str().is_some_and(|s| REASONS.contains(&s))))),
        "invalid trigger reasons"
    );
    for field in ["target_block", "target_timestamp"] {
        if data.get(field).is_some() {
            ensure!(
                data[field].as_u64().is_some_and(|n| n > 0
                    && n <= if field == "target_timestamp" {
                        253_402_300_799
                    } else {
                        i64::MAX as u64
                    }),
                "invalid posting target"
            );
        }
    }
    ensure!(
        data.get("target_block").is_some() == data.get("target_timestamp").is_some(),
        "incomplete posting target"
    );
    let detail = &data["detail"];
    ensure!(detail.is_object(), "missing event details");
    if stage == "preparation_failed" {
        ensure!(
            data["transaction_hash"].is_null()
                && data["sequence"] == 0
                && data.get("target_block").is_some(),
            "invalid failed preparation identity"
        );
        ensure!(
            matches!(
                detail["reason"].as_str(),
                Some(
                    "preparation_deadline"
                        | "prover_deadline"
                        | "prover_unavailable"
                        | "prover_aborted"
                        | "prover_rejected_candidate"
                        | "prover_cache_miss"
                        | "prover_backend"
                )
            ),
            "invalid preparation reason"
        );
        ensure!(
            detail["preparation_ms"]
                .as_f64()
                .is_some_and(|n| n.is_finite() && n >= 0.0),
            "invalid preparation duration"
        );
    }
    if stage == "dispatched" && data.get("target_block").is_some() {
        ensure!(
            data["target_block"] == detail["target_block"]
                && data["target_timestamp"] == detail["target_timestamp"],
            "dispatch target differs from prepared target"
        );
    }
    if stage == "included" {
        ensure!(
            detail["l1_block"]
                .as_u64()
                .is_some_and(|n| n <= i64::MAX as u64),
            "invalid inclusion block"
        );
        detail["l1_block_hash"]
            .as_str()
            .context("missing inclusion hash")?
            .parse::<B256>()?;
    }
    if matches!(stage, "dispatched" | "excluded") {
        ensure!(
            detail["target_block"]
                .as_u64()
                .is_some_and(|n| n <= i64::MAX as u64),
            "invalid target block"
        );
    }
    Ok(Some(data))
}

#[derive(Debug)]
pub struct Batch {
    pub file_id: String,
    pub offset: u64,
    pub anchor: String,
    pub lines: Vec<Vec<u8>>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn anchor(file: &mut File, offset: u64) -> Result<String> {
    let len = offset.min(128);
    file.seek(SeekFrom::Start(offset - len))?;
    let mut bytes = vec![0; len as usize];
    file.read_exact(&mut bytes)?;
    Ok(digest(&bytes))
}

/// A bounded read pinned to an opened file (safe across rename rotation).
/// First-line fingerprint + inode distinguish truncation/reuse from rotation.
#[cfg(unix)]
pub fn read_batch(
    path: &Path,
    checkpoints: &HashMap<String, (u64, String)>,
) -> Result<Option<Batch>> {
    use std::os::unix::fs::MetadataExt;
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "posting input is not a regular file");
    let mut first = Vec::new();
    BufReader::new((&mut file).take(MAX_LINE as u64 + 1)).read_until(b'\n', &mut first)?;
    ensure!(first.len() <= MAX_LINE, "posting log line exceeds 64 KiB");
    if first.last() != Some(&b'\n') {
        return Ok(None);
    }
    let file_id = format!("{}:{}:{}", meta.dev(), meta.ino(), digest(&first));
    let offset = match checkpoints.get(&file_id) {
        Some((offset, expected)) => {
            ensure!(
                *offset <= meta.len() && anchor(&mut file, *offset)? == *expected,
                "posting log was truncated or modified; preserve it and reset its checkpoint to replay"
            );
            *offset
        }
        None => 0,
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new((&mut file).take(MAX_BATCH));
    let mut lines = Vec::new();
    let mut end = offset;
    loop {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        ensure!(line.len() <= MAX_LINE, "posting log line exceeds 64 KiB");
        if line.last() != Some(&b'\n') {
            break;
        }
        end += line.len() as u64;
        lines.push(line);
    }
    Ok(Some(Batch {
        file_id,
        offset: end,
        anchor: anchor(&mut file, end)?,
        lines,
    }))
}

pub async fn import(client: &mut Client, config: &Config, batch: &Batch) -> Result<()> {
    let tx = client.transaction().await?;
    for line in &batch.lines {
        if let Some(event) = parse(line, config)? {
            let id = event["attempt_id"].as_str().context("attempt ID")?;
            let sequence = i32::try_from(event["sequence"].as_u64().context("sequence")?)?;
            if let Some(prior) = tx
                .query_opt(
                    "SELECT data::text FROM eez_indexer.posting_events WHERE attempt_id=$1 LIMIT 1",
                    &[&id],
                )
                .await?
            {
                let prior: Value = serde_json::from_str(&prior.get::<_, String>(0))?;
                for field in [
                    "posting_id",
                    "chain_id",
                    "registry",
                    "rollup_id",
                    "sync_height",
                    "trigger_reasons",
                    "target_block",
                    "target_timestamp",
                ] {
                    ensure!(
                        prior[field] == event[field],
                        "posting attempt context changed"
                    );
                }
            }
            let data = event.to_string();
            // An identical replay is harmless; a conflicting ID is corruption.
            let changed = tx.execute("INSERT INTO eez_indexer.posting_events(attempt_id,sequence,data) VALUES($1,$2,$3::text::jsonb) ON CONFLICT(attempt_id,sequence) DO UPDATE SET data=EXCLUDED.data WHERE posting_events.data=EXCLUDED.data", &[&id,&sequence,&data]).await?;
            ensure!(changed == 1, "conflicting duplicate posting event");
        }
    }
    let offset = i64::try_from(batch.offset)?;
    tx.execute("INSERT INTO eez_indexer.posting_files(file_id,byte_offset,anchor) VALUES($1,$2,$3) ON CONFLICT(file_id) DO UPDATE SET byte_offset=EXCLUDED.byte_offset,anchor=EXCLUDED.anchor", &[&batch.file_id,&offset,&batch.anchor]).await?;
    tx.commit().await?;
    Ok(())
}

async fn scan(client: &mut Client, config: &Config) -> Result<()> {
    let mut checkpoints = HashMap::new();
    for row in client
        .query(
            "SELECT file_id,byte_offset,anchor FROM eez_indexer.posting_files",
            &[],
        )
        .await?
    {
        checkpoints.insert(
            row.get::<_, String>(0),
            (
                u64::try_from(row.get::<_, i64>(1))?,
                row.get::<_, String>(2),
            ),
        );
    }
    let directory = config
        .posting_logs_dir
        .clone()
        .context("posting directory not configured")?;
    let batches = tokio::task::spawn_blocking(move || -> Result<Vec<Batch>> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            // Reth size rotation uses the configured prefix plus suffixes.
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("posting.log")
                && entry.file_type()?.is_file()
            {
                files.push(entry.path());
            }
        }
        ensure!(
            files.len() <= 128,
            "more than 128 posting log files; archive imported logs outside the source directory"
        );
        ensure!(
            !files.is_empty(),
            "no posting.log files; check Composer file logging and the mount"
        );
        files.sort();
        files
            .into_iter()
            .filter_map(|path| read_batch(&path, &checkpoints).transpose())
            .collect()
    })
    .await??;
    for batch in batches {
        if !batch.lines.is_empty() {
            import(client, config, &batch).await?;
        }
    }
    Ok(())
}

async fn worker(config: &Config, url: &str) -> Result<()> {
    let mut settings: tokio_postgres::Config = url.parse()?;
    settings.connect_timeout(Duration::from_secs(10));
    let (mut client, connection) = settings.connect(NoTls).await?;
    let connection = tokio::spawn(async move {
        let _ = connection.await;
    });
    let result = async {
        client.batch_execute("SET statement_timeout='10s'; SET lock_timeout='5s'").await?;
        let locked: bool = client.query_one("SELECT pg_try_advisory_lock(519763212)", &[]).await?.get(0);
        ensure!(locked, "another posting collector holds this database");
        loop {
            let error = scan(&mut client, config).await.err().map(|e| e.to_string());
            if let Some(error) = &error { tracing::warn!(%error, "posting collection failed; checkpoint retained"); }
            client.execute("INSERT INTO eez_indexer.posting_collection(checked_at,error) VALUES(now(),$1) ON CONFLICT(singleton) DO UPDATE SET checked_at=EXCLUDED.checked_at,error=EXCLUDED.error", &[&error]).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }.await;
    connection.abort();
    result
}

pub async fn run(config: Config, url: String) {
    loop {
        if let Err(error) = worker(&config, &url).await {
            tracing::warn!(%error, "posting collector reconnecting");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
