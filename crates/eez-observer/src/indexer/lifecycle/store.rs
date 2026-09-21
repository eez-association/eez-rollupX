use super::{Config, Database, Deadline, Error, SourceConfig};
use eez_telemetry::{
    MAX_RECORD_BYTES,
    reader::{ReadBatch, SegmentId, Tail},
    schema::{Data, Identity, Record, VERSION, WriterStatus},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) async fn register(
    db: &mut Database,
    config: &Config,
    source: &SourceConfig,
) -> Result<i64, Error> {
    let tx = db.client.transaction().bounded().await?;
    let deployment:i64=tx.query_one("INSERT INTO eez.deployments(environment,name) VALUES($1,$2) ON CONFLICT(environment,name) DO UPDATE SET name=EXCLUDED.name RETURNING id",&[&config.environment,&config.deployment]).bounded().await?.try_get(0)?;
    let directory = source.directory.to_str().ok_or(Error::SelectionChanged)?;
    let row=tx.query_one("INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory) VALUES($1,$2,$3,$4) ON CONFLICT(deployment_id,service,instance) DO UPDATE SET instance=EXCLUDED.instance RETURNING id,directory",&[&deployment,&source.service,&source.instance,&directory]).bounded().await?;
    if row.try_get::<_, String>(1)? != directory {
        return Err(Error::SelectionChanged);
    }
    let id = row.try_get(0)?;
    tx.commit().bounded().await?;
    Ok(id)
}
async fn lock(tx: &tokio_postgres::Transaction<'_>, id: i64) -> Result<i64, Error> {
    Ok(tx
        .query_one(
            "SELECT inventory_generation FROM eez.lifecycle_sources WHERE id=$1 FOR UPDATE",
            &[&id],
        )
        .bounded()
        .await?
        .try_get(0)?)
}
pub(super) async fn inventory(
    db: &mut Database,
    id: i64,
    generation: i64,
    keys: &[SegmentId],
) -> Result<(), Error> {
    let tx = db.client.transaction().bounded().await?;
    if lock(&tx, id).await? != generation {
        return Err(Error::Inconsistent);
    }
    let keys=json!(keys.iter().map(|key|json!({"epoch":alloy_primitives::hex::encode(key.epoch.0),"segment":alloy_primitives::hex::encode(key.segment.0)})).collect::<Vec<_>>());
    tx.execute("INSERT INTO eez.lifecycle_epochs(source_id,epoch) SELECT DISTINCT $1::bigint,decode(k.epoch,'hex') FROM jsonb_to_recordset($2) k(epoch text) ON CONFLICT DO NOTHING",&[&id,&keys]).bounded().await?;
    tx.execute("INSERT INTO eez.lifecycle_segments(source_id,epoch,segment_id) SELECT $1::bigint,decode(k.epoch,'hex'),decode(k.segment,'hex') FROM jsonb_to_recordset($2) k(epoch text,segment text) ON CONFLICT DO NOTHING",&[&id,&keys]).bounded().await?;
    // Require two successful absent inventories. Directory errors never enter
    // this transaction; a stale concurrent inventory cannot change presence.
    tx.execute("UPDATE eez.lifecycle_segments s SET present=CASE WHEN EXISTS(SELECT FROM jsonb_to_recordset($2) k(epoch text,segment text) WHERE decode(k.epoch,'hex')=s.epoch AND decode(k.segment,'hex')=s.segment_id) THEN true ELSE missing_since IS NULL END,missing_since=CASE WHEN EXISTS(SELECT FROM jsonb_to_recordset($2) k(epoch text,segment text) WHERE decode(k.epoch,'hex')=s.epoch AND decode(k.segment,'hex')=s.segment_id) THEN NULL ELSE COALESCE(missing_since,now()) END WHERE source_id=$1",&[&id,&keys]).bounded().await?;
    tx.execute("UPDATE eez.lifecycle_epochs e SET fully_scanned=false WHERE source_id=$1 AND EXISTS(SELECT FROM eez.lifecycle_segments s WHERE s.source_id=e.source_id AND s.epoch=e.epoch AND s.present AND NOT s.scan_complete)",&[&id]).bounded().await?;
    tx.execute(
        "UPDATE eez.lifecycle_sources SET inventory_generation=inventory_generation+1 WHERE id=$1",
        &[&id],
    )
    .bounded()
    .await?;
    tx.commit().bounded().await?;
    Ok(())
}
async fn raw(
    tx: &tokio_postgres::Transaction<'_>,
    id: i64,
    key: SegmentId,
    offset: u64,
    bytes: &[u8],
    reason: Option<&str>,
) -> Result<(), Error> {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let offset = i64::try_from(offset).map_err(|_| Error::Inconsistent)?;
    tx.execute("INSERT INTO eez.lifecycle_records(source_id,epoch,segment_id,offset_bytes,digest,raw,reason) VALUES($1,$2::bytea,$3::bytea,$4,$5::bytea,$6,$7) ON CONFLICT DO NOTHING",&[&id,&key.epoch.0.as_slice(),&key.segment.0.as_slice(),&offset,&digest.as_slice(),&bytes,&reason]).bounded().await?;
    Ok(())
}
pub(super) async fn ingest(
    db: &mut Database,
    id: i64,
    identity: &Identity,
    key: SegmentId,
    expected_offset: i64,
    batch: ReadBatch,
) -> Result<(), Error> {
    let tx = db.client.transaction().bounded().await?;
    lock(&tx, id).await?;
    let row=tx.query_one("SELECT offset_bytes,header_digest::bytea FROM eez.lifecycle_segments WHERE source_id=$1 AND epoch=$2::bytea AND segment_id=$3::bytea FOR UPDATE",&[&id,&key.epoch.0.as_slice(),&key.segment.0.as_slice()]).bounded().await?;
    if row.try_get::<_, i64>(0)? != expected_offset {
        return Err(Error::Inconsistent);
    }
    let persisted: Option<Vec<u8>> = row.try_get(1)?;
    let complete_header =
        batch.header.last() == Some(&b'\n') && batch.header.len() <= MAX_RECORD_BYTES;
    let header = serde_json::from_slice::<Record>(&batch.header).ok();
    let valid_header = match &header {
        Some(Record::Header {
            version,
            identity: actual,
            epoch,
            segment_id,
            ..
        }) => {
            *version == VERSION
                && actual == identity
                && *epoch == key.epoch
                && *segment_id == key.segment
                && complete_header
        }
        _ => false,
    };
    let mut header_reason = (!valid_header).then_some("invalid_or_unknown_header");
    if let Some(digest) = persisted
        && digest != batch.header_digest
    {
        raw(
            &tx,
            id,
            key,
            0,
            &batch.header,
            Some("changed_segment_header"),
        )
        .await?;
        tx.execute("UPDATE eez.lifecycle_segments SET last_scan_at=now(),read_error='changed_segment_header',scan_complete=false WHERE source_id=$1 AND epoch=$2::bytea AND segment_id=$3::bytea",&[&id,&key.epoch.0.as_slice(),&key.segment.0.as_slice()]).bounded().await?;
        tx.commit().bounded().await?;
        return Ok(());
    }
    if valid_header {
        let Some(Record::Header {
            started_at_unix_ms,
            process_id,
            ..
        }) = header
        else {
            return Err(Error::Inconsistent);
        };
        let row=tx.query_one("SELECT started_at_ms::text,process_id FROM eez.lifecycle_epochs WHERE source_id=$1 AND epoch=$2::bytea",&[&id,&key.epoch.0.as_slice()]).bounded().await?;
        let started: Option<String> = row.try_get(0)?;
        let process: Option<i64> = row.try_get(1)?;
        if started
            .as_ref()
            .is_some_and(|value| value != &started_at_unix_ms.to_string())
            || process.is_some_and(|value| value != i64::from(process_id))
        {
            header_reason = Some("conflicting_epoch_header");
        } else {
            tx.execute("UPDATE eez.lifecycle_epochs SET started_at_ms=$3::text::numeric,process_id=$4 WHERE source_id=$1 AND epoch=$2::bytea",&[&id,&key.epoch.0.as_slice(),&started_at_unix_ms.to_string(),&i64::from(process_id)]).bounded().await?;
        }
    }
    let ended:bool=tx.query_one("SELECT liveness='ended' FROM eez.lifecycle_epochs WHERE source_id=$1 AND epoch=$2::bytea",&[&id,&key.epoch.0.as_slice()]).bounded().await?.try_get(0)?;
    for line in &batch.lines {
        let reason = if let Some(reason) = header_reason {
            Some(reason)
        } else {
            match serde_json::from_slice::<Record>(&line.bytes) {
                Ok(Record::Header { .. }) if line.offset == 0 => None,
                Ok(Record::Data(data))
                    if data.identity == *identity
                        && data.epoch == key.epoch
                        && eez_telemetry::validate_chunk(&data).is_ok() =>
                {
                    data_record(&tx, id, key, line.offset, &data, &line.bytes).await?
                }
                Ok(Record::Control {
                    version,
                    identity: actual,
                    epoch,
                    control_sequence,
                    observed_at_unix_ms,
                    status,
                    stats,
                    ..
                }) if version == VERSION
                    && actual == *identity
                    && epoch == key.epoch
                    && control_sequence > 0 =>
                {
                    let state = match status {
                        WriterStatus::Started => "started",
                        WriterStatus::Active => "active",
                        WriterStatus::SegmentClosed => "segment_closed",
                        WriterStatus::Stopped => "stopped",
                    };
                    let digest: [u8; 32] = Sha256::digest(&line.bytes).into();
                    let stats_json =
                        serde_json::to_value(stats).map_err(|_| Error::Inconsistent)?;
                    let row=tx.query_one("INSERT INTO eez.lifecycle_controls(source_id,epoch,control_sequence,last_assigned_sequence,status,stats,raw_digest,observed_at_ms) VALUES($1,$2::bytea,$3::text::numeric,$4::text::numeric,$5,$6,$7::bytea,$8::text::numeric) ON CONFLICT(source_id,epoch,control_sequence) DO UPDATE SET conflicted=eez.lifecycle_controls.conflicted OR eez.lifecycle_controls.raw_digest<>EXCLUDED.raw_digest RETURNING conflicted",&[&id,&key.epoch.0.as_slice(),&control_sequence.to_string(),&stats.last_assigned_sequence.to_string(),&state,&stats_json,&digest.as_slice(),&observed_at_unix_ms.to_string()]).bounded().await?;
                    row.try_get::<_, bool>(0)?.then_some("conflicting_control")
                }
                _ => Some("invalid_or_unknown_record"),
            }
        };
        raw(&tx, id, key, line.offset, &line.bytes, reason).await?;
    }
    let mut scan_complete = batch.snapshot_complete;
    let mut read_error = header_reason;
    let partial = matches!(batch.tail, Some(Tail::Partial(_)));
    match &batch.tail {
        Some(Tail::Partial(line)) if ended => {
            raw(
                &tx,
                id,
                key,
                line.offset,
                &line.bytes,
                Some("truncated_ended_epoch"),
            )
            .await?;
            scan_complete = true;
        }
        Some(Tail::Oversized(line)) => {
            raw(
                &tx,
                id,
                key,
                line.offset,
                &line.bytes,
                Some("oversized_line_prefix"),
            )
            .await?;
            read_error = Some("oversized_line");
        }
        _ => {}
    }
    let offset = i64::try_from(batch.next_offset).map_err(|_| Error::Inconsistent)?;
    let length = i64::try_from(batch.snapshot_len).map_err(|_| Error::Inconsistent)?;
    let header_digest = complete_header.then_some(batch.header_digest.as_slice());
    tx.execute("UPDATE eez.lifecycle_segments SET offset_bytes=$4,snapshot_bytes=$5,last_scan_at=now(),scan_complete=$6,partial_since=CASE WHEN $7 THEN COALESCE(partial_since,now()) ELSE NULL END,read_error=$8,header_digest=COALESCE(header_digest,$9::bytea) WHERE source_id=$1 AND epoch=$2::bytea AND segment_id=$3::bytea",&[&id,&key.epoch.0.as_slice(),&key.segment.0.as_slice(),&offset,&length,&scan_complete,&partial,&read_error,&header_digest]).bounded().await?;
    tx.commit().bounded().await?;
    Ok(())
}
async fn data_record(
    tx: &tokio_postgres::Transaction<'_>,
    id: i64,
    key: SegmentId,
    offset: u64,
    data: &Data,
    bytes: &[u8],
) -> Result<Option<&'static str>, Error> {
    let offset = i64::try_from(offset).map_err(|_| Error::Inconsistent)?;
    let sequence = data.sequence.to_string();
    let count = i32::try_from(data.chunk_count).map_err(|_| Error::Inconsistent)?;
    let row=tx.query_one("INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count) VALUES($1,$2::bytea,$3::text::numeric,$4) ON CONFLICT(source_id,epoch,sequence) DO UPDATE SET state=CASE WHEN eez.lifecycle_events.chunk_count<>EXCLUDED.chunk_count THEN 'conflict' ELSE eez.lifecycle_events.state END,payload=CASE WHEN eez.lifecycle_events.chunk_count<>EXCLUDED.chunk_count THEN NULL ELSE eez.lifecycle_events.payload END RETURNING state",&[&id,&data.epoch.0.as_slice(),&sequence,&count]).bounded().await?;
    let conflicted = row.try_get::<_, String>(0)? == "conflict";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let record = serde_json::to_value(data).map_err(|_| Error::Inconsistent)?;
    let index = i32::try_from(data.chunk_index).map_err(|_| Error::Inconsistent)?;
    tx.execute("INSERT INTO eez.lifecycle_chunks(source_id,epoch,sequence,chunk_index,record,raw_digest,segment_id,offset_bytes) VALUES($1,$2::bytea,$3::text::numeric,$4,$5,$6::bytea,$7::bytea,$8) ON CONFLICT DO NOTHING",&[&id,&data.epoch.0.as_slice(),&sequence,&index,&record,&digest.as_slice(),&key.segment.0.as_slice(),&offset]).bounded().await?;
    let existing:Vec<u8>=tx.query_one("SELECT raw_digest::bytea FROM eez.lifecycle_chunks WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric AND chunk_index=$4",&[&id,&data.epoch.0.as_slice(),&sequence,&index]).bounded().await?.try_get(0)?;
    if conflicted || existing != digest {
        tx.execute("UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric",&[&id,&data.epoch.0.as_slice(),&sequence]).bounded().await?;
        Ok(Some("conflicting_chunk"))
    } else {
        Ok(None)
    }
}
pub(super) async fn assemble_pending(db: &mut Database, id: i64) -> Result<(), Error> {
    let rows=db.client.query("SELECT epoch::bytea,sequence::text,chunk_count FROM eez.lifecycle_events WHERE source_id=$1 AND state='incomplete' ORDER BY last_assembly_at,epoch,sequence LIMIT 512",&[&id]).bounded().await?;
    let mut processed_bytes = 0usize;
    for row in rows {
        let epoch: Vec<u8> = row.try_get(0)?;
        let sequence: String = row.try_get(1)?;
        let count: i32 = row.try_get(2)?;
        let rows=db.client.query("SELECT record FROM eez.lifecycle_chunks WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric ORDER BY chunk_index LIMIT 1366",&[&id,&epoch,&sequence]).bounded().await?;
        if Some(rows.len()) != usize::try_from(count).ok() {
            db.client.execute("UPDATE eez.lifecycle_events SET last_assembly_at=now() WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric",&[&id,&epoch,&sequence]).bounded().await?;
            continue;
        }
        let values = rows
            .iter()
            .map(|row| row.try_get::<_, Value>(0))
            .collect::<Result<Vec<_>, _>>()?;
        let bytes = values
            .iter()
            .map(|value| value["payload_hex"].as_str().map_or(0, str::len))
            .sum::<usize>();
        if processed_bytes > 0 && processed_bytes.saturating_add(bytes) > 16 * 1024 * 1024 {
            break;
        }
        processed_bytes = processed_bytes.saturating_add(bytes);
        db.client.execute("UPDATE eez.lifecycle_events SET last_assembly_at=now() WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric",&[&id,&epoch,&sequence]).bounded().await?;
        let payload = tokio::task::spawn_blocking(move || {
            let records = values
                .into_iter()
                .map(serde_json::from_value::<Data>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| ())?;
            let payload = eez_telemetry::assemble(&records).map_err(|_| ())?;
            serde_json::to_value(payload).map_err(|_| ())
        })
        .await
        .map_err(|_| Error::Inconsistent)?;
        let (state, payload) = match payload {
            Ok(value) => ("complete", Some(value)),
            Err(()) => ("invalid", None),
        };
        if state == "invalid" {
            db.client.execute("UPDATE eez.lifecycle_records r SET reason='invalid_payload' FROM eez.lifecycle_chunks c WHERE c.source_id=$1 AND c.epoch=$2::bytea AND c.sequence=$3::text::numeric AND r.source_id=c.source_id AND r.epoch=c.epoch AND r.segment_id=c.segment_id AND r.offset_bytes=c.offset_bytes AND r.digest=c.raw_digest AND r.reason IS NULL",&[&id,&epoch,&sequence]).bounded().await?;
        }
        // A conflicting late chunk may have invalidated the row during the
        // blocking assembly. Never publish that stale success over the conflict.
        db.client.execute("UPDATE eez.lifecycle_events SET state=$4,payload=$5 WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric AND state='incomplete'",&[&id,&epoch,&sequence,&state,&payload]).bounded().await?;
    }
    Ok(())
}
