//! Correlation is an independent observer source. Missing claims never erase
//! receipt charges; finalized L2 contradictions freeze only derived finality.
use alloy_primitives::B256;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;

use super::ledger::Deadline;
use super::{Block, Config, Error, Rpc, SharedStatus, Status, ledger, remote_block};
use crate::{
    correlation::{self, BlockRef, Settlement},
    database::{Database, DatabaseConfig},
};

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
        tokio::time::sleep(Duration::from_secs(crate::retry_delay(failures))).await;
    }
}
pub(super) async fn poll(config: &Config, rpc: &Rpc, status: &SharedStatus) -> bool {
    status
        .write()
        .await
        .settlements
        .get_or_insert_with(Status::default);
    let result = cycle(config, rpc).await;
    let mut states = status.write().await;
    match result {
        Ok(state) => {
            states.settlements = Some(state);
            true
        }
        Err(error) => {
            states.settlements.get_or_insert_with(Status::default).error = Some(error);
            tracing::warn!(
                reason = error.label(),
                "settlement correlation collection failed"
            );
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
    let l1 = ledger::find_chain(&db, config, &config.exporter.l1, "l1")
        .await?
        .ok_or(Error::Bootstrap)?;
    let l2 = ledger::find_chain(&db, config, &config.exporter.l2, "l2")
        .await?
        .ok_or(Error::Bootstrap)?;
    db.client.execute("INSERT INTO eez.correlation_sources(l1_chain_id,l2_chain_id) VALUES($1,$2) ON CONFLICT DO NOTHING",&[&l1,&l2]).bounded().await?;
    let result = collect(&mut db, config, rpc, l1, l2).await;
    if let Err(error) = result {
        let frozen = (error == Error::SettlementFinality).then_some("l2_finalized_contradiction");
        db.client.execute("UPDATE eez.correlation_sources SET collection_error=$3,frozen_reason=COALESCE(frozen_reason,$4) WHERE l1_chain_id=$1 AND l2_chain_id=$2",&[&l1,&l2,&error.label(),&frozen]).bounded().await?;
    }
    result
}

async fn collect(
    db: &mut Database,
    config: &Config,
    rpc: &Rpc,
    l1: i64,
    l2: i64,
) -> Result<Status, Error> {
    let frozen:Option<String>=db.client.query_one("SELECT frozen_reason FROM eez.correlation_sources WHERE l1_chain_id=$1 AND l2_chain_id=$2",&[&l1,&l2]).bounded().await?.try_get(0)?;
    if frozen.is_some() {
        return Err(Error::SettlementFinality);
    }
    rpc.validate_identity(&config.exporter.l1).await?;
    rpc.validate_identity(&config.exporter.l2).await?;
    if let Some(row)=db.client.query_opt("SELECT number::text,hash::bytea FROM eez.settlement_finality WHERE l2_chain_id=$1 ORDER BY number DESC LIMIT 1",&[&l2]).bounded().await? {
        let number=row.try_get::<_,String>(0)?.parse().map_err(|_|Error::Inconsistent)?;
        let hash=decode_hash(row.try_get(1)?)?;
        if remote_block(rpc,&config.exporter.l2,number).await?.hash!=hash {
            rpc.validate_identity(&config.exporter.l2).await?;
            return Err(Error::SettlementFinality);
        }
    }
    let rows=db.client.query("WITH candidates AS (SELECT p.block_number,p.block_hash,bool_or(run.range_observed IS NOT TRUE) AS pending,COALESCE(min(run.last_attempt_at),'epoch'::timestamptz) AS attempted FROM eez.posting_receipts p LEFT JOIN eez.correlation_runs run ON run.l1_chain_id=p.chain_instance_id AND run.l2_chain_id=$2 AND run.transaction_hash=p.transaction_hash AND run.block_hash=p.block_hash WHERE p.chain_instance_id=$1 AND p.success GROUP BY p.block_number,p.block_hash) (SELECT block_number::text,block_hash::bytea FROM candidates WHERE pending ORDER BY attempted,candidates.block_number LIMIT 3) UNION ALL (SELECT block_number::text,block_hash::bytea FROM candidates WHERE NOT pending ORDER BY attempted,candidates.block_number LIMIT 1)",&[&l1,&l2]).bounded().await?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(BlockRef {
            number: row
                .try_get::<_, String>(0)?
                .parse()
                .map_err(|_| Error::Inconsistent)?,
            hash: decode_hash(row.try_get(1)?)?,
        });
    }
    if pending.is_empty() {
        // Probe the method even in a quiet interval. An empty answer proves API
        // collection only, never that cross-chain execution has been exercised.
        let cursor = ledger::cursor(db, l1).await?.ok_or(Error::Bootstrap)?;
        pending.push(BlockRef {
            number: cursor.number,
            hash: cursor.hash,
        });
    }
    for anchor in pending {
        let cursor1 = ledger::cursor(db, l1).await?.ok_or(Error::Bootstrap)?;
        let cursor2 = ledger::cursor(db, l2).await?.ok_or(Error::Bootstrap)?;
        db.client.execute("INSERT INTO eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash) SELECT $1,$2,transaction_hash,block_hash FROM eez.posting_receipts WHERE chain_instance_id=$1 AND block_hash=$3::bytea AND success ON CONFLICT(l1_chain_id,l2_chain_id,transaction_hash,block_hash) DO UPDATE SET last_attempt_at=now()",&[&l1,&l2,&anchor.hash.as_slice()]).bounded().await?;
        let result = fetch(rpc, config, anchor).await;
        let claims = match result {
            Ok(claims) => claims,
            Err(error) => {
                db.client.execute("UPDATE eez.correlation_runs SET collection_error=$4 WHERE l1_chain_id=$1 AND l2_chain_id=$2 AND block_hash=$3::bytea",&[&l1,&l2,&anchor.hash.as_slice(),&error.label()]).bounded().await?;
                return Err(error);
            }
        };
        verify_cursors(rpc, config, &cursor1, &cursor2).await?;
        if remote_block(rpc, &config.exporter.l1, anchor.number)
            .await?
            .hash
            != anchor.hash
        {
            return Err(Error::Inconsistent);
        }
        save(db, l1, l2, &cursor1, &cursor2, anchor, &claims).await?;
    }
    let cursor1 = ledger::cursor(db, l1).await?.ok_or(Error::Bootstrap)?;
    let cursor2 = ledger::cursor(db, l2).await?.ok_or(Error::Bootstrap)?;
    verify_cursors(rpc, config, &cursor1, &cursor2).await?;
    promote(db, l1, l2, &cursor1, &cursor2).await?;
    let coverage = db.client.query_one("SELECT posts_with_gaps,finality_pending_ranges FROM eez.settlement_coverage WHERE l1_chain_id=$1 AND l2_chain_id=$2",&[&l1,&l2]).bounded().await?;
    let gaps: i64 = coverage.try_get(0)?;
    let finality_pending: i64 = coverage.try_get(1)?;
    let finalized: Option<String> = db
        .client
        .query_one(
            "SELECT max(number)::text FROM eez.verified_l2_finality WHERE l2_chain_id=$1",
            &[&l2],
        )
        .bounded()
        .await?
        .try_get(0)?;
    Ok(Status {
        last_success: Some(crate::now_ms()),
        verified_cursors: Some([cursor1.hash, cursor2.hash]),
        gaps: Some(gaps),
        finality_pending: Some(finality_pending),
        finalized: finalized
            .map(|v| v.parse().map_err(|_| Error::Inconsistent))
            .transpose()?,
        ..Status::default()
    })
}

fn decode_hash(bytes: Vec<u8>) -> Result<B256, Error> {
    Ok(B256::from(
        <[u8; 32]>::try_from(bytes).map_err(|_| Error::Inconsistent)?,
    ))
}
async fn fetch(rpc: &Rpc, config: &Config, anchor: BlockRef) -> Result<Vec<Settlement>, Error> {
    let value = rpc
        .call(
            &config.exporter.l2.url,
            "eez_getSettledL2RangesByL1Block",
            json!([{"blockHash":format!("{:#x}",anchor.hash)}]),
        )
        .await?;
    Ok(correlation::parse(&value, anchor)?)
}
async fn verify_cursors(rpc: &Rpc, config: &Config, l1: &Block, l2: &Block) -> Result<(), Error> {
    rpc.validate_identity(&config.exporter.l1).await?;
    rpc.validate_identity(&config.exporter.l2).await?;
    if remote_block(rpc, &config.exporter.l1, l1.number)
        .await?
        .hash
        != l1.hash
        || remote_block(rpc, &config.exporter.l2, l2.number)
            .await?
            .hash
            != l2.hash
    {
        return Err(Error::Inconsistent);
    }
    Ok(())
}
async fn lock_pair(
    transaction: &tokio_postgres::Transaction<'_>,
    l1: i64,
    l2: i64,
    cursor1: &Block,
    cursor2: &Block,
) -> Result<(), Error> {
    let mut cursors = [(l1, cursor1), (l2, cursor2)];
    cursors.sort_by_key(|(id, _)| *id);
    for (id, cursor) in cursors {
        ledger::lock_cursor(transaction, id, Some(cursor)).await?;
    }
    Ok(())
}

async fn save(
    db: &mut Database,
    l1: i64,
    l2: i64,
    cursor1: &Block,
    cursor2: &Block,
    anchor: BlockRef,
    claims: &[Settlement],
) -> Result<(), Error> {
    let transaction = db.client.transaction().bounded().await?;
    lock_pair(&transaction, l1, l2, cursor1, cursor2).await?;
    let records: Vec<_> = claims.iter().map(|claim| {
        let mut digest=Sha256::new();
        digest.update(claim.transaction_index.to_be_bytes());
        for block in &claim.l2_blocks {digest.update(block.number.to_be_bytes());digest.update(block.hash.as_slice());}
        let digest:[u8;32]=digest.finalize().into();
        json!({"tx":alloy_primitives::hex::encode(claim.transaction_hash),"index":claim.transaction_index.to_string(),
            "count":claim.l2_blocks.len(),"digest":alloy_primitives::hex::encode(digest),"canonical":claim.reported_canonical_l2,
            "blocks":claim.l2_blocks.iter().enumerate().map(|(ordinal,block)|json!({"ordinal":ordinal,"number":block.number.to_string(),"hash":alloy_primitives::hex::encode(block.hash)})).collect::<Vec<_>>()})
    }).collect();
    let records = json!(records);
    // Bulk SQL keeps the chain-lock duration independent of per-row network
    // latency. Raw RPC size, claim count and total block count are all bounded.
    let valid:i64=transaction.query_one("SELECT count(*) FROM jsonb_to_recordset($3) claim(tx text,\"index\" text) WHERE EXISTS(SELECT FROM eez.posting_receipts p WHERE p.chain_instance_id=$1 AND p.block_hash=$2::bytea AND p.transaction_hash=decode(claim.tx,'hex') AND p.transaction_index=claim.\"index\"::numeric AND p.success)",&[&l1,&anchor.hash.as_slice(),&records]).bounded().await?.try_get(0)?;
    if usize::try_from(valid).ok() != Some(claims.len()) {
        return Err(Error::Inconsistent);
    }
    transaction.execute("INSERT INTO eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash) SELECT $1,$2,decode(claim.tx,'hex'),$3::bytea FROM jsonb_to_recordset($4) claim(tx text) ON CONFLICT DO NOTHING",&[&l1,&l2,&anchor.hash.as_slice(),&records]).bounded().await?;
    transaction.execute("INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2) SELECT $1,$2,decode(claim.tx,'hex'),$3::bytea,claim.\"index\"::numeric,claim.\"count\",decode(claim.digest,'hex'),claim.\"canonical\" FROM jsonb_to_recordset($4) claim(tx text,\"index\" text,\"count\" integer,digest text,\"canonical\" boolean) ON CONFLICT(l1_chain_id,l2_chain_id,transaction_hash,block_hash,evidence_digest) DO NOTHING",&[&l1,&l2,&anchor.hash.as_slice(),&records]).bounded().await?;
    transaction.execute("INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash) SELECT s.id,b.ordinal,b.number::numeric,decode(b.hash,'hex') FROM jsonb_to_recordset($4) claim(tx text,digest text,blocks jsonb) CROSS JOIN LATERAL jsonb_to_recordset(claim.blocks) b(ordinal integer,number text,hash text) JOIN eez.settlements s ON s.l1_chain_id=$1 AND s.l2_chain_id=$2 AND s.transaction_hash=decode(claim.tx,'hex') AND s.block_hash=$3::bytea AND s.evidence_digest=decode(claim.digest,'hex') ON CONFLICT(settlement_id,ordinal) DO UPDATE SET number=EXCLUDED.number,hash=EXCLUDED.hash WHERE ROW(eez.settlement_blocks.number,eez.settlement_blocks.hash) IS DISTINCT FROM ROW(EXCLUDED.number,EXCLUDED.hash)",&[&l1,&l2,&anchor.hash.as_slice(),&records]).bounded().await?;
    transaction.execute("UPDATE eez.correlation_runs run SET last_success_at=now(),range_observed=EXISTS(SELECT FROM jsonb_to_recordset($4) claim(tx text) WHERE decode(claim.tx,'hex')=run.transaction_hash),collection_error=CASE WHEN EXISTS(SELECT FROM jsonb_to_recordset($4) claim(tx text) WHERE decode(claim.tx,'hex')=run.transaction_hash) THEN NULL ELSE 'no_applied_range_observed' END WHERE run.l1_chain_id=$1 AND run.l2_chain_id=$2 AND run.block_hash=$3::bytea",&[&l1,&l2,&anchor.hash.as_slice(),&records]).bounded().await?;
    transaction.commit().bounded().await?;
    Ok(())
}

async fn promote(
    db: &mut Database,
    l1: i64,
    l2: i64,
    cursor1: &Block,
    cursor2: &Block,
) -> Result<(), Error> {
    let transaction = db.client.transaction().bounded().await?;
    lock_pair(&transaction, l1, l2, cursor1, cursor2).await?;
    let changed=transaction.execute("UPDATE eez.correlation_sources SET last_success_at=now(),collection_error=NULL WHERE l1_chain_id=$1 AND l2_chain_id=$2 AND frozen_reason IS NULL",&[&l1,&l2]).bounded().await?;
    if changed != 1 {
        return Err(Error::SettlementFinality);
    }
    transaction.execute("UPDATE eez.settlements s SET last_validation_at=now(),range_verified_at=CASE WHEN s.block_count=(SELECT count(*) FROM eez.settlement_blocks applied JOIN eez.blocks b ON b.chain_instance_id=$2 AND b.number=applied.number AND b.hash=applied.hash AND b.canonical WHERE applied.settlement_id=s.id) THEN now() ELSE NULL END WHERE s.id IN(SELECT id FROM eez.settlements WHERE l1_chain_id=$1 AND l2_chain_id=$2 AND range_verified_at IS NULL ORDER BY last_validation_at,id LIMIT 16)",&[&l1,&l2]).bounded().await?;
    let ids:Vec<i64>=transaction.query("SELECT id FROM eez.settlements WHERE l1_chain_id=$1 AND l2_chain_id=$2 AND range_verified_at IS NOT NULL AND NOT finality_done ORDER BY last_finality_attempt_at,id LIMIT 16",&[&l1,&l2]).bounded().await?.iter().map(|row|row.try_get(0)).collect::<Result<_,_>>()?;
    transaction
        .execute(
            "UPDATE eez.settlements SET last_finality_attempt_at=now() WHERE id=ANY($1)",
            &[&ids],
        )
        .bounded()
        .await?;
    transaction.execute("INSERT INTO eez.settlement_finality(settlement_id,l2_chain_id,number,hash) SELECT settlement_id,l2_chain_id,number,hash FROM eez.canonical_settled_l2_blocks b WHERE l1_chain_id=$1 AND l2_chain_id=$2 AND settlement_id=ANY($3) AND source_valid AND l1_finalized AND NOT EXISTS(SELECT FROM eez.settlement_finality f WHERE f.l2_chain_id=b.l2_chain_id AND f.number=b.number) ORDER BY settlement_id,number LIMIT 4096 ON CONFLICT(l2_chain_id,number) DO NOTHING",&[&l1,&l2,&ids]).bounded().await?;
    transaction.execute("UPDATE eez.settlements s SET finality_done=NOT EXISTS(SELECT FROM eez.settlement_blocks b LEFT JOIN eez.settlement_finality f ON f.l2_chain_id=s.l2_chain_id AND f.number=b.number AND f.hash=b.hash WHERE b.settlement_id=s.id AND f.l2_chain_id IS NULL) WHERE s.id=ANY($1)",&[&ids]).bounded().await?;
    transaction.commit().bounded().await?;
    Ok(())
}
