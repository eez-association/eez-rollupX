//! Optional, independent L1 log enrichment. Each receipt is parsed and committed
//! as a whole; an unavailable log set never stalls or changes fee accounting.
use super::{
    Config, Error, Rpc, SharedStatus,
    ledger::{self, Deadline},
    remote_block,
};
use crate::{
    accounting::Receipt,
    database::{Database, DatabaseConfig},
    protocol_logs::Evidence,
};
use alloy_primitives::B256;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct Status {
    last_success: Option<u64>,
    error: Option<Error>,
    counts: [i64; 4],
}

pub fn render(state: Option<&Status>, now: u64) -> String {
    use std::fmt::Write;
    let Some(state) = state else {
        return String::new();
    };
    let valid = state.error.is_none()
        && state.last_success.is_some_and(|time| {
            time <= now.saturating_add(5000) && now.saturating_sub(time) <= 90000
        });
    let mut result = format!(
        "eez_observer_protocol_log_collection_success {}\n",
        u8::from(valid)
    );
    if let Some(time) = state.last_success {
        writeln!(
            result,
            "eez_observer_protocol_log_last_success_timestamp_seconds {}",
            time / 1000
        )
        .unwrap();
    }
    if let Some(error) = state.error {
        writeln!(
            result,
            "eez_observer_protocol_log_error{{reason=\"{}\"}} 1",
            error.label()
        )
        .unwrap();
    }
    if valid {
        for (label, count) in ["pending", "gap", "complete", "conflict"]
            .into_iter()
            .zip(state.counts)
        {
            writeln!(
                result,
                "eez_observer_protocol_log_receipts{{state=\"{label}\"}} {count}"
            )
            .unwrap();
        }
    }
    result
}

pub(super) async fn run(config: &Config, rpc: &Rpc, status: &SharedStatus) {
    if config.l1.contracts.is_empty() {
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
    if config.l1.contracts.is_empty() {
        return true;
    }
    // Cancellation drops this worker's dedicated DB connection, including any
    // unacknowledged transaction. Other indexers keep their own connections.
    let result = tokio::time::timeout(Duration::from_secs(30), cycle(config, rpc))
        .await
        .unwrap_or(Err(Error::Rpc(crate::rpc::Failure::Transport)));
    match result {
        Ok(state) => {
            status.write().await.protocol_logs = Some(state);
            true
        }
        Err(error) => {
            status
                .write()
                .await
                .protocol_logs
                .get_or_insert_with(Status::default)
                .error = Some(error);
            tracing::warn!(reason = error.label(), "protocol log collection failed");
            false
        }
    }
}

async fn cycle(config: &Config, rpc: &Rpc) -> Result<Status, Error> {
    let settings = DatabaseConfig::load(&config.database_config).map_err(|_| Error::Schema)?;
    let mut db = Database::connect(&settings)
        .await
        .map_err(|_| Error::Database)?;
    db.verify_writer().await.map_err(|_| Error::Schema)?;
    let endpoint = &config.exporter.l1;
    rpc.validate_identity(endpoint).await?;
    let id = ledger::find_chain(&db, config, endpoint, "l1")
        .await?
        .ok_or(Error::Bootstrap)?;
    let checkpoint = ledger::historical_checkpoint(&db, id).await?;
    {
        let transaction = db.client.transaction().bounded().await?;
        ledger::lock_historical(&transaction, id, &checkpoint).await?;
        transaction.commit().bounded().await?;
    }
    // Oldest attempted inclusions rotate even when a response is missing or
    // times out. Historical schema-5 receipts enter naturally without a rewrite.
    let rows=db.client.query("SELECT r.transaction_hash::bytea,r.block_hash::bytea,r.block_number::text,r.transaction_index::text,r.success,r.transaction_type FROM eez.canonical_receipt_facts r LEFT JOIN eez.protocol_log_attempts a USING(chain_instance_id,transaction_hash,block_hash) WHERE r.chain_instance_id=$1 AND r.frozen_reason IS NULL ORDER BY a.last_attempt_at NULLS FIRST,r.block_number,r.transaction_index LIMIT 32", &[&id]).bounded().await?;
    for row in rows {
        let parse_hash = |index| -> Result<B256, Error> {
            Ok(B256::from(
                <[u8; 32]>::try_from(row.try_get::<_, Vec<u8>>(index)?)
                    .map_err(|_| Error::Inconsistent)?,
            ))
        };
        let tx_hash = parse_hash(0)?;
        let block_hash = parse_hash(1)?;
        let number = row
            .try_get::<_, String>(2)?
            .parse::<u64>()
            .map_err(|_| Error::Inconsistent)?;
        let index = row
            .try_get::<_, String>(3)?
            .parse::<u64>()
            .map_err(|_| Error::Inconsistent)?;
        let success: bool = row.try_get(4)?;
        let kind: i16 = row.try_get(5)?;
        let checkpoint = ledger::historical_checkpoint(&db, id).await?;
        let cursor = &checkpoint.cursor;
        {
            let transaction = db.client.transaction().bounded().await?;
            ledger::lock_historical(&transaction, id, &checkpoint).await?;
            transaction.execute("INSERT INTO eez.protocol_log_attempts(chain_instance_id,transaction_hash,block_hash,collection_error) VALUES($1,$2::bytea,$3::bytea,'interrupted') ON CONFLICT(chain_instance_id,transaction_hash,block_hash) DO UPDATE SET last_attempt_at=now(),collection_error='interrupted'", &[&id,&tx_hash.as_slice(),&block_hash.as_slice()]).bounded().await?;
            transaction.commit().bounded().await?;
        }
        let result = match rpc.receipt(endpoint, tx_hash).await {
            Ok(value) if value.is_null() => Err("receipt_unavailable"),
            Ok(value) => match Receipt::parse(&value) {
                Ok(receipt)
                    if receipt.transaction_hash == tx_hash
                        && receipt.block_hash == block_hash
                        && receipt.block_number == number
                        && receipt.transaction_index == index
                        && receipt.success == success
                        && crate::model::quantity(&value["type"])
                            .is_ok_and(|value| i16::try_from(value) == Ok(kind)) =>
                {
                    Evidence::parse(&value, &receipt).map_err(crate::protocol_logs::Gap::label)
                }
                _ => Err("receipt_identity_mismatch"),
            },
            Err(error) => Err(error.label()),
        };
        rpc.validate_identity(endpoint).await?;
        if remote_block(rpc, endpoint, cursor.number).await?.hash != cursor.hash
            || remote_block(rpc, endpoint, number).await?.hash != block_hash
        {
            return Err(Error::Inconsistent);
        }
        let transaction = db.client.transaction().bounded().await?;
        ledger::lock_historical(&transaction, id, &checkpoint).await?;
        let canonical:bool=transaction.query_one("SELECT canonical AND number=$3::text::numeric FROM eez.blocks WHERE chain_instance_id=$1 AND hash=$2::bytea", &[&id,&block_hash.as_slice(),&number.to_string()]).bounded().await?.try_get(0)?;
        if !canonical {
            return Err(Error::Inconsistent);
        }
        if let Ok(evidence) = &result {
            let digest = evidence.digest();
            // The shared chain lock serializes this cap across observer workers.
            // Two contradictory complete claims are enough to retain a conflict;
            // later repeats or a third answer never restore stronger evidence.
            transaction.execute("INSERT INTO eez.protocol_log_claims(chain_instance_id,transaction_hash,block_hash,digest,events) SELECT $1,$2::bytea,$3::bytea,$4::bytea,$5 WHERE (SELECT count(*) FROM eez.protocol_log_claims WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea AND block_hash=$3::bytea)<2 ON CONFLICT DO NOTHING", &[&id,&tx_hash.as_slice(),&block_hash.as_slice(),&&digest[..],&evidence.records()]).bounded().await?;
        }
        let error = result.err();
        transaction.execute("UPDATE eez.protocol_log_attempts SET collection_error=$4::text,last_success_at=CASE WHEN $4::text IS NULL THEN now() ELSE last_success_at END WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea AND block_hash=$3::bytea", &[&id,&tx_hash.as_slice(),&block_hash.as_slice(),&error]).bounded().await?;
        transaction.commit().bounded().await?;
    }
    let row=db.client.query_one("SELECT count(*) FILTER(WHERE state='pending'),count(*) FILTER(WHERE state='gap'),count(*) FILTER(WHERE state='complete'),count(*) FILTER(WHERE state='conflict') FROM eez.protocol_log_coverage WHERE chain_instance_id=$1", &[&id]).bounded().await?;
    Ok(Status {
        last_success: Some(crate::now_ms()),
        error: None,
        counts: [
            row.try_get(0)?,
            row.try_get(1)?,
            row.try_get(2)?,
            row.try_get(3)?,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metrics_distinguish_collection_health_from_evidence_coverage() {
        assert!(render(None, 100000).is_empty());
        let mut state = Status {
            last_success: Some(100000),
            error: None,
            counts: [1, 2, 3, 4],
        };
        let current = render(Some(&state), 100000);
        assert!(current.contains("collection_success 1"));
        assert!(current.contains("receipts{state=\"conflict\"} 4"));
        for now in [94999, 190001] {
            let text = render(Some(&state), now);
            assert!(text.contains("collection_success 0"));
            assert!(!text.contains("_receipts{"));
            assert!(text.contains("last_success_timestamp_seconds 100"));
        }
        state.error = Some(Error::Frozen);
        let text = render(Some(&state), 100000);
        assert!(text.contains("collection_success 0"));
        assert!(!text.contains("_receipts{"));
    }
}
