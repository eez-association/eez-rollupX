//! Independent best-effort lifecycle import. Canonical receipt indexing never
//! waits for this worker, and complete transport does not prove chain outcomes.
use super::{Config, Error, SharedStatus, ledger::Deadline};
use crate::database::{Database, DatabaseConfig};
use eez_telemetry::{
    Liveness,
    reader::{self, SegmentId},
    schema::{Epoch, Identity},
};
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Component, PathBuf},
    time::Duration,
};
mod store;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub service: String,
    pub instance: String,
    pub directory: PathBuf,
}
impl SourceConfig {
    fn identity(&self, config: &Config) -> Identity {
        Identity {
            environment: config.environment.clone(),
            deployment: config.deployment.clone(),
            service: self.service.clone(),
            instance: self.instance.clone(),
        }
    }
}
pub(super) fn validate_sources(config: &Config) -> Result<(), &'static str> {
    if config.lifecycle.len() > 8 {
        return Err("at most eight lifecycle sources are supported");
    }
    let mut identities = HashSet::new();
    let mut paths = HashSet::new();
    for source in &config.lifecycle {
        source.identity(config).validate()?;
        if !source.directory.is_absolute()
            || source.directory.to_str().is_none()
            || source
                .directory
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            || !identities.insert((&source.service, &source.instance))
            || !paths.insert(&source.directory)
        {
            return Err(
                "lifecycle sources require unique identities and absolute, distinct spool directories",
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub service: String,
    pub instance: String,
    pub last_success: Option<u64>,
    pub error: Option<&'static str>,
    pub counts: [i64; 10],
}
pub fn render(states: &[Status], now: u64) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for state in states {
        let labels = format!(
            "source_service=\"{}\",source_instance=\"{}\"",
            state.service, state.instance
        );
        let valid = state.error.is_none()
            && state.last_success.is_some_and(|time| {
                time <= now.saturating_add(5000) && now.saturating_sub(time) <= 90000
            });
        writeln!(
            result,
            "eez_observer_lifecycle_collection_success{{{labels}}} {}",
            u8::from(valid)
        )
        .unwrap();
        if let Some(time) = state.last_success {
            writeln!(
                result,
                "eez_observer_lifecycle_last_success_timestamp_seconds{{{labels}}} {}",
                time / 1000
            )
            .unwrap();
        }
        if let Some(reason) = state.error {
            writeln!(
                result,
                "eez_observer_lifecycle_error{{{labels},reason=\"{reason}\"}} 1"
            )
            .unwrap();
        }
        if valid {
            for (name, value) in [
                "complete_events",
                "gap_intervals",
                "quarantined_records",
                "lost_segments",
                "stale_partial_segments",
                "unknown_trailing_epochs",
                "unreadable_segments",
                "pending_events",
                "unknown_liveness_epochs",
                "pending_segments",
            ]
            .iter()
            .zip(state.counts)
            {
                writeln!(result, "eez_observer_lifecycle_{name}{{{labels}}} {value}").unwrap();
            }
        }
    }
    result
}
async fn poll(config: &Config, source: &SourceConfig, status: &SharedStatus) -> bool {
    let result = cycle(config, source).await;
    let mut states = status.write().await;
    let position = states
        .lifecycle
        .iter()
        .position(|s| s.service == source.service && s.instance == source.instance);
    let position = position.unwrap_or_else(|| {
        states.lifecycle.push(Status {
            service: source.service.clone(),
            instance: source.instance.clone(),
            ..Status::default()
        });
        states.lifecycle.len() - 1
    });
    match result {
        Ok(counts) => {
            states.lifecycle[position].last_success = Some(crate::now_ms());
            states.lifecycle[position].error = None;
            states.lifecycle[position].counts = counts;
            true
        }
        Err(reason) => {
            states.lifecycle[position].error = Some(reason);
            tracing::warn!(
                reason,
                service = source.service,
                instance = source.instance,
                "lifecycle import failed"
            );
            false
        }
    }
}
pub(super) async fn once(config: &Config, status: &SharedStatus) {
    for source in &config.lifecycle {
        poll(config, source, status).await;
    }
}
pub(super) async fn run(config: &Config, status: &SharedStatus) {
    let mut workers = tokio::task::JoinSet::new();
    for source in &config.lifecycle {
        let (config, source, status) = (config.clone(), source.clone(), status.clone());
        workers.spawn(async move {
            let mut failures = 0u32;
            loop {
                let ok = poll(&config, &source, &status).await;
                failures = if ok { 0 } else { failures.saturating_add(1) };
                tokio::time::sleep(Duration::from_secs(if ok {
                    5
                } else {
                    crate::retry_delay(failures)
                }))
                .await;
            }
        });
    }
    while let Some(result) = workers.join_next().await {
        if result.is_err() {
            tracing::error!(
                reason = "lifecycle_worker_failed",
                "lifecycle worker exited unexpectedly"
            );
        }
    }
}
async fn cycle(config: &Config, source: &SourceConfig) -> Result<[i64; 10], &'static str> {
    let settings = DatabaseConfig::load(&config.database_config)?;
    let mut db = Database::connect(&settings).await?;
    db.verify_writer().await?;
    let id = store::register(&mut db, config, source)
        .await
        .map_err(Error::label)?;
    let result = collect(&mut db, id, config, source).await;
    if let Err(reason) = result {
        db.client
            .execute(
                "UPDATE eez.lifecycle_sources SET collection_error=$2 WHERE id=$1",
                &[&id, &reason],
            )
            .bounded()
            .await
            .map_err(Error::label)?;
    }
    result
}
async fn collect(
    db: &mut Database,
    id: i64,
    config: &Config,
    source: &SourceConfig,
) -> Result<[i64; 10], &'static str> {
    let generation: i64 = db
        .client
        .query_one(
            "SELECT inventory_generation FROM eez.lifecycle_sources WHERE id=$1",
            &[&id],
        )
        .bounded()
        .await
        .map_err(Error::label)?
        .try_get(0)
        .map_err(|_| "database")?;
    let directory = source.directory.clone();
    let keys = tokio::task::spawn_blocking(move || reader::inventory(&directory))
        .await
        .map_err(|_| "reader_task")?
        .map_err(|_| "spool_inventory")?;
    store::inventory(db, id, generation, &keys)
        .await
        .map_err(Error::label)?;
    let rows=db.client.query("SELECT epoch::bytea FROM eez.lifecycle_epochs WHERE source_id=$1 AND liveness<>'ended' ORDER BY last_probe_at NULLS FIRST,epoch LIMIT 16",&[&id]).bounded().await.map_err(Error::label)?;
    for row in rows {
        let epoch = epoch(row.try_get(0).map_err(|_| "database")?).map_err(Error::label)?;
        let directory = source.directory.clone();
        let liveness =
            tokio::task::spawn_blocking(move || eez_telemetry::probe_epoch(&directory, epoch))
                .await
                .map_err(|_| "reader_task")?;
        let state = match liveness {
            Liveness::Live => "live",
            Liveness::Ended => "ended",
            Liveness::Unknown => "unknown",
        };
        db.client.execute("UPDATE eez.lifecycle_epochs SET liveness=$3,last_probe_at=now(),ended_at=CASE WHEN $3::text='ended' THEN COALESCE(ended_at,now()) ELSE ended_at END WHERE source_id=$1 AND epoch=$2::bytea AND liveness<>'ended'",&[&id,&epoch.0.as_slice(),&state]).bounded().await.map_err(Error::label)?;
    }
    // Requery oldest work after each pass so one active segment can use the
    // remaining budget, while incomplete earlier files cannot starve later IDs.
    for _ in 0..16 {
        let row=db.client.query_opt("SELECT epoch::bytea,segment_id::bytea,offset_bytes FROM eez.lifecycle_segments WHERE source_id=$1 AND present AND (NOT scan_complete OR last_scan_at<now()-interval '5 seconds') ORDER BY last_scan_at,epoch,segment_id LIMIT 1",&[&id]).bounded().await.map_err(Error::label)?;
        let Some(row) = row else { break };
        let key = SegmentId {
            epoch: epoch(row.try_get(0).map_err(|_| "database")?).map_err(Error::label)?,
            segment: epoch(row.try_get(1).map_err(|_| "database")?).map_err(Error::label)?,
        };
        let offset: i64 = row.try_get(2).map_err(|_| "database")?;
        let read_offset = u64::try_from(offset).map_err(|_| "invalid_checkpoint")?;
        let directory = source.directory.clone();
        let read =
            tokio::task::spawn_blocking(move || reader::read_segment(&directory, key, read_offset))
                .await
                .map_err(|_| "reader_task")?;
        match read {
            Ok(batch) => store::ingest(db, id, &source.identity(config), key, offset, batch)
                .await
                .map_err(Error::label)?,
            Err(error) => {
                let reason = if error.kind() == std::io::ErrorKind::NotFound {
                    "segment_unavailable"
                } else {
                    "segment_read_failed"
                };
                db.client.execute("UPDATE eez.lifecycle_segments SET last_scan_at=now(),read_error=$4 WHERE source_id=$1 AND epoch=$2::bytea AND segment_id=$3::bytea",&[&id,&key.epoch.0.as_slice(),&key.segment.0.as_slice(),&reason]).bounded().await.map_err(Error::label)?;
            }
        }
    }
    store::assemble_pending(db, id)
        .await
        .map_err(Error::label)?;
    db.client.execute("UPDATE eez.lifecycle_epochs e SET fully_scanned=NOT EXISTS(SELECT FROM eez.lifecycle_segments s WHERE s.source_id=e.source_id AND s.epoch=e.epoch AND s.present AND NOT s.scan_complete) WHERE e.source_id=$1 AND e.liveness='ended'",&[&id]).bounded().await.map_err(Error::label)?;
    db.client.execute("UPDATE eez.lifecycle_sources SET last_success_at=now(),collection_error=NULL WHERE id=$1",&[&id]).bounded().await.map_err(Error::label)?;
    let row=db.client.query_one("SELECT complete_events,gap_intervals,quarantined_records,lost_segments,stale_partial_segments,unknown_trailing_epochs,unreadable_segments,pending_events,unknown_liveness_epochs,pending_segments FROM eez.lifecycle_coverage WHERE source_id=$1",&[&id]).bounded().await.map_err(Error::label)?;
    let mut counts = [0; 10];
    for (index, value) in counts.iter_mut().enumerate() {
        *value = row.try_get(index).map_err(|_| "database")?;
    }
    Ok(counts)
}
fn epoch(bytes: Vec<u8>) -> Result<Epoch, Error> {
    Ok(Epoch(bytes.try_into().map_err(|_| Error::Inconsistent)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn sources_are_optional_bounded_and_identity_checked() {
        let value = json!({"exporter":{"l1":{"url":"http://localhost:1","chain_id":1,"genesis_hash":format!("0x{}","01".repeat(32))},"l2":{"url":"http://localhost:2","chain_id":2,"genesis_hash":format!("0x{}","02".repeat(32))}},"environment":"devnet","deployment":"fixture","rollup_id":1,"database_config":"/private/writer.json","l1":{"bootstrap":0,"include_all_transactions":true},"l2":{"bootstrap":0,"include_all_transactions":true}});
        let mut config: Config = serde_json::from_value(value).unwrap();
        assert!(config.lifecycle.is_empty());
        config.validate().unwrap();
        let source = SourceConfig {
            service: "composer".into(),
            instance: "node-1".into(),
            directory: "/private/spool".into(),
        };
        config.lifecycle.push(source.clone());
        config.validate().unwrap();
        config.lifecycle.push(source.clone());
        assert!(config.validate().is_err());
        config.lifecycle.pop();
        config.lifecycle[0].directory = "relative".into();
        assert!(config.validate().is_err());
        config.lifecycle[0].directory = "/private/../spool".into();
        assert!(config.validate().is_err());
        config.lifecycle[0] = source.clone();
        let mut same_path = source.clone();
        same_path.instance = "node-2".into();
        config.lifecycle.push(same_path);
        assert!(config.validate().is_err());
        config.lifecycle.pop();
        config.lifecycle = (0..8)
            .map(|i| SourceConfig {
                service: "composer".into(),
                instance: format!("node-{i}"),
                directory: format!("/private/spool-{i}").into(),
            })
            .collect();
        config.validate().unwrap();
        config.lifecycle.push(source.clone());
        assert!(config.validate().is_err());
        config.lifecycle = vec![source];
        config.lifecycle[0].instance = "bad\"label".into();
        assert!(config.validate().is_err());
        assert!(serde_json::from_value::<SourceConfig>(json!({"service":"composer","instance":"node-1","directory":"/private/spool","typo":true})).is_err());
    }
    #[test]
    fn stale_or_failed_ingestion_preserves_timestamp_but_omits_counts() {
        assert!(render(&[], 100000).is_empty());
        let mut states = [Status {
            service: "composer".into(),
            instance: "node-1".into(),
            last_success: Some(100000),
            counts: [3; 10],
            error: None,
        }];
        assert!(render(&states,100000).contains("eez_observer_lifecycle_complete_events{source_service=\"composer\",source_instance=\"node-1\"} 3"));
        assert!(!render(&states, 190001).contains("eez_observer_lifecycle_complete_events"));
        states[0].error = Some("spool_inventory");
        let text = render(&states, 100000);
        assert!(!text.contains("eez_observer_lifecycle_complete_events"));
        assert!(text.contains("timestamp_seconds"));
        assert!(text.contains("spool_inventory"));
    }
}
