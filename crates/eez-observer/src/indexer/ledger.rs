use super::{Block, Config, Endpoint, Error, Receipt, Selection, Transaction};
use crate::database::Database;
use alloy_primitives::B256;
use std::time::Duration;
use tokio_postgres::{GenericClient, Row};

// Server statement timeouts do not bound a stalled network response. Each SQL
// operation also has a client deadline; dropping the failed scan's dedicated
// connection rolls back any transaction whose commit was not acknowledged.
pub(super) trait Deadline<T, E>: Future<Output = Result<T, E>> + Sized {
    async fn bounded(self) -> Result<T, Error>
    where
        E: Into<Error>,
    {
        tokio::time::timeout(Duration::from_secs(15), self)
            .await
            .map_err(|_| Error::Database)?
            .map_err(Into::into)
    }
}
impl<T, E, F: Future<Output = Result<T, E>>> Deadline<T, E> for F {}

const BLOCK_FIELDS: &str = "number::text, hash::bytea, parent_hash::bytea, timestamp_seconds::text";

fn block(row: &Row) -> Result<Block, Error> {
    let parse = |index| -> Result<u64, Error> {
        row.try_get::<_, String>(index)?
            .parse()
            .map_err(|_| Error::Inconsistent)
    };
    let hash = |index| -> Result<B256, Error> {
        Ok(B256::from(
            <[u8; 32]>::try_from(row.try_get::<_, Vec<u8>>(index)?)
                .map_err(|_| Error::Inconsistent)?,
        ))
    };
    Ok(Block {
        number: parse(0)?,
        hash: hash(1)?,
        parent: hash(2)?,
        timestamp: parse(3)?,
    })
}

pub(super) async fn register(
    db: &mut Database,
    config: &Config,
    endpoint: &Endpoint,
    selection: &Selection,
    role: &str,
) -> Result<i64, Error> {
    let transaction = db.client.transaction().bounded().await?;
    let deployment:i64=transaction.query_one("INSERT INTO eez.deployments(environment,name) VALUES($1,$2) ON CONFLICT(environment,name) DO UPDATE SET name=EXCLUDED.name RETURNING id",&[&config.environment,&config.deployment]).bounded().await?.try_get(0)?;
    let genesis: B256 = endpoint
        .genesis_hash
        .parse()
        .map_err(|_| Error::SelectionChanged)?;
    let rollup = if role == "l1" { 0 } else { config.rollup_id }.to_string();
    let policy = selection.policy();
    let row=transaction.query_one("INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,selection) VALUES($1,$2,$3::text::numeric,$4::bytea,$5::text::numeric,$6::text::numeric,$7) ON CONFLICT(deployment_id,role,chain_id,genesis_hash,rollup_id) DO UPDATE SET role=EXCLUDED.role RETURNING id,bootstrap_number::text,selection,frozen_reason",&[&deployment,&role,&endpoint.chain_id.to_string(),&genesis.as_slice(),&rollup,&selection.bootstrap.to_string(),&policy]).bounded().await?;
    let id: i64 = row.try_get(0)?;
    if row.try_get::<_, String>(1)? != selection.bootstrap.to_string()
        || row.try_get::<_, serde_json::Value>(2)? != policy
    {
        return Err(Error::SelectionChanged);
    }
    if row.try_get::<_, Option<String>>(3)?.is_some() {
        return Err(Error::Frozen);
    }
    transaction
        .execute(
            "INSERT INTO eez.scan_cursors(chain_instance_id) VALUES($1) ON CONFLICT DO NOTHING",
            &[&id],
        )
        .bounded()
        .await?;
    transaction.commit().bounded().await?;
    Ok(id)
}

pub(super) async fn find_chain(
    db: &Database,
    config: &Config,
    endpoint: &Endpoint,
    role: &str,
) -> Result<Option<i64>, Error> {
    let genesis: B256 = endpoint
        .genesis_hash
        .parse()
        .map_err(|_| Error::SelectionChanged)?;
    let rollup = if role == "l1" { 0 } else { config.rollup_id }.to_string();
    db.client.query_opt("SELECT c.id FROM eez.chain_instances c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.environment=$1 AND d.name=$2 AND c.role=$3 AND c.chain_id=$4::text::numeric AND c.genesis_hash=$5::bytea AND c.rollup_id=$6::text::numeric",&[&config.environment,&config.deployment,&role,&endpoint.chain_id.to_string(),&genesis.as_slice(),&rollup]).bounded().await?.map(|row|row.try_get(0).map_err(Error::from)).transpose()
}

pub(super) async fn cursor(db: &Database, id: i64) -> Result<Option<Block>, Error> {
    let rows=db.client.query(&format!("SELECT {BLOCK_FIELDS} FROM eez.blocks WHERE chain_instance_id=$1 AND hash=(SELECT block_hash FROM eez.scan_cursors WHERE chain_instance_id=$1)"),&[&id]).bounded().await?;
    rows.first().map(block).transpose()
}
pub(super) async fn finalized(db: &Database, id: i64) -> Result<Option<Block>, Error> {
    let rows=db.client.query(&format!("SELECT {BLOCK_FIELDS} FROM eez.blocks WHERE chain_instance_id=$1 AND finalized ORDER BY number DESC LIMIT 1"),&[&id]).bounded().await?;
    rows.first().map(block).transpose()
}
pub(super) async fn parent(db: &Database, id: i64, child: &Block) -> Result<Option<Block>, Error> {
    let rows=db.client.query(&format!("SELECT {BLOCK_FIELDS} FROM eez.blocks WHERE chain_instance_id=$1 AND hash=$2::bytea AND canonical"),&[&id,&child.parent.as_slice()]).bounded().await?;
    rows.first().map(block).transpose()
}

pub(super) async fn lock_cursor(
    transaction: &tokio_postgres::Transaction<'_>,
    id: i64,
    expected: Option<&Block>,
) -> Result<(), Error> {
    // Explicit lock order, shared by registration, block commits and finality.
    let chain = transaction
        .query_one(
            "SELECT frozen_reason FROM eez.chain_instances WHERE id=$1 FOR UPDATE",
            &[&id],
        )
        .bounded()
        .await?;
    if chain.try_get::<_, Option<String>>(0)?.is_some() {
        return Err(Error::Frozen);
    }
    let row = transaction
        .query_one(
            "SELECT block_hash::bytea FROM eez.scan_cursors WHERE chain_instance_id=$1 FOR UPDATE",
            &[&id],
        )
        .bounded()
        .await?;
    let actual: Option<Vec<u8>> = row.try_get(0)?;
    if actual.as_deref() != expected.map(|block| block.hash.as_slice()) {
        return Err(Error::Inconsistent);
    }
    Ok(())
}

pub(super) async fn retract(
    db: &mut Database,
    id: i64,
    expected: Option<&Block>,
    ancestor: Option<&Block>,
) -> Result<(), Error> {
    let transaction = db.client.transaction().bounded().await?;
    lock_cursor(&transaction, id, expected).bounded().await?;
    let height = ancestor.map(|block| block.number.to_string());
    let finalized:bool=transaction.query_one("SELECT EXISTS(SELECT FROM eez.blocks WHERE chain_instance_id=$1 AND canonical AND finalized AND ($2::text IS NULL OR number>$2::text::numeric))",&[&id,&height]).bounded().await?.try_get(0)?;
    if finalized {
        return Err(Error::Frozen);
    }
    transaction.execute("UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=$1 AND canonical AND ($2::text IS NULL OR number>$2::text::numeric)",&[&id,&height]).bounded().await?;
    transaction.execute("UPDATE eez.collection_gaps g SET state='orphaned',last_seen_at=now() FROM eez.blocks b WHERE g.chain_instance_id=$1 AND b.chain_instance_id=$1 AND NOT b.canonical AND g.source='receipt' AND g.reference_key LIKE '%' || encode(b.hash,'hex')",&[&id]).bounded().await?;
    let hash = ancestor.map(|block| block.hash.as_slice());
    transaction.execute("UPDATE eez.scan_cursors SET block_hash=$2::bytea,updated_at=now() WHERE chain_instance_id=$1",&[&id,&hash]).bounded().await?;
    transaction.commit().bounded().await?;
    Ok(())
}

async fn save_receipt(
    client: &impl GenericClient,
    id: i64,
    block: &Block,
    tx: &Transaction,
    receipt: Option<&Receipt>,
) -> Result<(), Error> {
    let recipient = tx.recipient.as_ref().map(|address| address.as_slice());
    client.execute("INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type,method_selector) VALUES($1,$2::bytea,$3::bytea,$4::bytea,$5::text::numeric,$6,$7) ON CONFLICT(chain_instance_id,hash) DO UPDATE SET sender=EXCLUDED.sender,recipient=EXCLUDED.recipient,nonce=EXCLUDED.nonce,transaction_type=EXCLUDED.transaction_type,method_selector=EXCLUDED.method_selector",&[&id,&tx.hash.as_slice(),&tx.sender.as_slice(),&recipient,&tx.nonce.to_string(),&tx.kind,&tx.selector]).bounded().await?;
    save_posting(client, id, tx.hash, &tx.posting).await?;
    let key = format!("{:#x}:{:#x}", tx.hash, block.hash);
    if let Some(receipt) = receipt {
        let cost = receipt.cost.as_ref().ok();
        let values = [
            cost.map(|c| c.gas_used.to_string()),
            cost.map(|c| c.effective_gas_price.to_string()),
            cost.map(|c| c.blob_gas_used.to_string()),
            cost.map(|c| c.blob_gas_price.to_string()),
            cost.map(|c| c.execution_paid.to_string()),
            cost.map(|c| c.blob_paid.to_string()),
            cost.map(|c| c.purchased_blobs.to_string()),
        ];
        let gap = receipt.cost.as_ref().err().map(|error| error.label());
        // Complete immutable observations survive later incomplete RPC answers.
        // A conflicting COMPLETE answer still reaches the SQL fact guard.
        client.execute(
            "INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs,accounting_gap)
            VALUES ($1,$2::bytea,$3::bytea,$4::text::numeric,$5,false,$6::text::numeric,$7::text::numeric,$8::text::numeric,$9::text::numeric,$10::text::numeric,$11::text::numeric,$12::text::numeric,$13)
            ON CONFLICT (chain_instance_id,transaction_hash,block_hash)
            DO UPDATE SET
              transaction_index=EXCLUDED.transaction_index,
              success=EXCLUDED.success,
              gas_used=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.gas_used ELSE eez.receipt_observations.gas_used END,
              effective_gas_price=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.effective_gas_price ELSE eez.receipt_observations.effective_gas_price END,
              blob_gas_used=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.blob_gas_used ELSE eez.receipt_observations.blob_gas_used END,
              blob_gas_price=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.blob_gas_price ELSE eez.receipt_observations.blob_gas_price END,
              execution_paid=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.execution_paid ELSE eez.receipt_observations.execution_paid END,
              blob_paid=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.blob_paid ELSE eez.receipt_observations.blob_paid END,
              purchased_blobs=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.purchased_blobs ELSE eez.receipt_observations.purchased_blobs END,
              accounting_gap=CASE WHEN eez.receipt_observations.accounting_gap IS NOT NULL OR EXCLUDED.accounting_gap IS NULL THEN EXCLUDED.accounting_gap ELSE eez.receipt_observations.accounting_gap END",
            &[&id,&tx.hash.as_slice(),&block.hash.as_slice(),&tx.index.to_string(),&receipt.success,&values[0],&values[1],&values[2],&values[3],&values[4],&values[5],&values[6],&gap]).bounded().await?;
    }
    let complete:bool=client.query_one("SELECT EXISTS(SELECT FROM eez.receipt_observations WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea AND block_hash=$3::bytea AND accounting_gap IS NULL)",&[&id,&tx.hash.as_slice(),&block.hash.as_slice()]).bounded().await?.try_get(0)?;
    if complete {
        client.execute("UPDATE eez.collection_gaps SET state='resolved',last_seen_at=now() WHERE chain_instance_id=$1 AND source='receipt' AND reference_key=$2",&[&id,&key]).bounded().await?;
    } else {
        client.execute("INSERT INTO eez.collection_gaps(chain_instance_id,source,reference_key,from_number,to_number,reason) VALUES($1,'receipt',$2,$3::text::numeric,$3::text::numeric,'receipt_unavailable_or_incomplete') ON CONFLICT(chain_instance_id,source,reference_key,reason) DO UPDATE SET state='open',last_seen_at=now()",&[&id,&key,&block.number.to_string()]).bounded().await?;
    }
    Ok(())
}

pub(super) async fn commit_block(
    db: &mut Database,
    id: i64,
    expected: Option<&Block>,
    block: &Block,
    receipts: &[(Transaction, Option<Receipt>)],
) -> Result<(), Error> {
    let transaction = db.client.transaction().bounded().await?;
    lock_cursor(&transaction, id, expected).bounded().await?;
    transaction.execute("INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage) VALUES($1,$2::bytea,$3::text::numeric,$4::bytea,$5::text::numeric,true,'pending') ON CONFLICT(chain_instance_id,hash) DO UPDATE SET number=EXCLUDED.number,parent_hash=EXCLUDED.parent_hash,timestamp_seconds=EXCLUDED.timestamp_seconds,canonical=true",&[&id,&block.hash.as_slice(),&block.number.to_string(),&block.parent.as_slice(),&block.timestamp.to_string()]).bounded().await?;
    for (tx, receipt) in receipts {
        save_receipt(&transaction, id, block, tx, receipt.as_ref())
            .bounded()
            .await?;
    }
    refresh_coverage(&transaction, id, block).bounded().await?;
    transaction.execute("UPDATE eez.collection_gaps SET state='resolved',last_seen_at=now() WHERE chain_instance_id=$1 AND source='chain' AND from_number=$2::text::numeric", &[&id,&block.number.to_string()]).bounded().await?;
    transaction.execute("UPDATE eez.scan_cursors SET block_hash=$2::bytea,updated_at=now() WHERE chain_instance_id=$1",&[&id,&block.hash.as_slice()]).bounded().await?;
    transaction.commit().bounded().await?;
    Ok(())
}

async fn refresh_coverage(
    client: &impl GenericClient,
    id: i64,
    block: &Block,
) -> Result<(), Error> {
    client.execute("UPDATE eez.blocks SET receipt_coverage=CASE WHEN EXISTS(SELECT FROM eez.collection_gaps WHERE chain_instance_id=$1 AND source='receipt' AND from_number=$3::text::numeric AND state='open' AND reference_key LIKE '%' || encode($2::bytea,'hex')) THEN 'gap' ELSE 'complete' END WHERE chain_instance_id=$1 AND hash=$2::bytea",&[&id,&block.hash.as_slice(),&block.number.to_string()]).bounded().await?;
    Ok(())
}

pub(super) async fn finalize(db: &mut Database, id: i64, anchor: &Block) -> Result<(), Error> {
    let transaction = db.client.transaction().bounded().await?;
    transaction
        .query_one(
            "SELECT id FROM eez.chain_instances WHERE id=$1 AND frozen_reason IS NULL FOR UPDATE",
            &[&id],
        )
        .bounded()
        .await?;
    let found:bool=transaction.query_one("SELECT EXISTS(SELECT FROM eez.blocks WHERE chain_instance_id=$1 AND hash=$2::bytea AND number=$3::text::numeric AND canonical)",&[&id,&anchor.hash.as_slice(),&anchor.number.to_string()]).bounded().await?.try_get(0)?;
    if !found {
        return Err(Error::Inconsistent);
    }
    // Scanner commits only contiguous parent-linked blocks, including after forks.
    // Exact hash membership above anchors the prefix; height alone is insufficient.
    transaction.execute("UPDATE eez.blocks SET finalized=true WHERE chain_instance_id=$1 AND canonical AND number<=$2::text::numeric",&[&id,&anchor.number.to_string()]).bounded().await?;
    transaction.commit().bounded().await?;
    Ok(())
}
pub(super) async fn mark_failure(db: &Database, id: i64, error: Error) -> Result<(), Error> {
    let freeze = match error {
        Error::Frozen => Some("finalized_contradiction"),
        Error::ConflictingFact => Some("conflicting_fact"),
        _ => None,
    };
    db.client.execute("UPDATE eez.chain_instances SET collection_error=$2,frozen_reason=COALESCE(frozen_reason,$3) WHERE id=$1",&[&id,&error.label(),&freeze]).bounded().await?;
    Ok(())
}
pub(super) async fn mark_success(db: &Database, id: i64) -> Result<(), Error> {
    let changed=db.client.execute("UPDATE eez.chain_instances SET last_success_at=now(),collection_error=NULL WHERE id=$1 AND frozen_reason IS NULL",&[&id]).bounded().await?;
    if changed != 1 {
        return Err(Error::Frozen);
    }
    Ok(())
}
pub(super) async fn gaps(db: &Database, id: i64) -> Result<i64, Error> {
    Ok(db.client.query_one("SELECT count(*) FROM eez.collection_gaps WHERE chain_instance_id=$1 AND state IN ('open','frozen')",&[&id]).bounded().await?.try_get(0)?)
}

pub(super) async fn pending_blocks(db: &Database, id: i64) -> Result<Vec<Block>, Error> {
    db.client.query(&format!("SELECT {BLOCK_FIELDS} FROM eez.blocks WHERE chain_instance_id=$1 AND canonical AND receipt_coverage='gap' ORDER BY last_receipt_attempt_at,number LIMIT 4"),&[&id]).bounded().await?.iter().map(block).collect()
}

pub(super) async fn touch_receipt_attempt(
    db: &Database,
    id: i64,
    block: &Block,
) -> Result<(), Error> {
    db.client.execute("UPDATE eez.blocks SET last_receipt_attempt_at=now() WHERE chain_instance_id=$1 AND hash=$2::bytea",&[&id,&block.hash.as_slice()]).bounded().await?;
    Ok(())
}

pub(super) async fn repair_block(
    db: &mut Database,
    id: i64,
    block: &Block,
    observations: &[(Transaction, Option<Receipt>)],
) -> Result<(), Error> {
    let transaction = db.client.transaction().bounded().await?;
    let chain = transaction
        .query_one(
            "SELECT frozen_reason FROM eez.chain_instances WHERE id=$1 FOR UPDATE",
            &[&id],
        )
        .bounded()
        .await?;
    if chain.try_get::<_, Option<String>>(0)?.is_some() {
        return Err(Error::Frozen);
    }
    let canonical:bool=transaction.query_one("SELECT canonical FROM eez.blocks WHERE chain_instance_id=$1 AND hash=$2::bytea FOR SHARE",&[&id,&block.hash.as_slice()]).bounded().await?.try_get(0)?;
    if !canonical {
        return Err(Error::Inconsistent);
    }
    for (tx, receipt) in observations {
        save_receipt(&transaction, id, block, tx, receipt.as_ref())
            .bounded()
            .await?;
    }
    refresh_coverage(&transaction, id, block).bounded().await?;
    transaction.commit().bounded().await?;
    Ok(())
}

pub(super) async fn record_block_gap(
    db: &Database,
    id: i64,
    number: u64,
    error: Error,
) -> Result<(), Error> {
    db.client.execute("INSERT INTO eez.collection_gaps(chain_instance_id,source,reference_key,from_number,to_number,reason) VALUES($1,'chain',$2::text,$2::text::numeric,$2::text::numeric,$3) ON CONFLICT(chain_instance_id,source,reference_key,reason) DO UPDATE SET state='open',last_seen_at=now()", &[&id,&number.to_string(),&error.label()]).bounded().await?;
    Ok(())
}

pub(super) async fn save_posting(
    client: &impl GenericClient,
    id: i64,
    hash: B256,
    analysis: &crate::posting::Analysis,
) -> Result<(), Error> {
    let digest = analysis.calldata_hash.as_ref().map(B256::as_slice);
    client.execute("INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at) VALUES($1,$2::bytea,$3,$4::bytea,now()) ON CONFLICT(chain_instance_id,transaction_hash) DO UPDATE SET state=EXCLUDED.state,calldata_hash=EXCLUDED.calldata_hash,observed_at=EXCLUDED.observed_at,collection_error=NULL", &[&id,&hash.as_slice(),&analysis.state,&digest]).bounded().await?;
    Ok(())
}

pub(super) async fn pending_posting(
    db: &Database,
    id: i64,
) -> Result<Vec<(B256, Option<Vec<u8>>)>, Error> {
    db.client.query("SELECT a.transaction_hash::bytea,t.method_selector FROM eez.posting_analysis a JOIN eez.transactions t ON t.chain_instance_id=a.chain_instance_id AND t.hash=a.transaction_hash WHERE a.chain_instance_id=$1 AND a.state='pending' ORDER BY a.last_attempt_at,a.transaction_hash LIMIT 4", &[&id]).bounded().await?.iter().map(|row| {
        let hash = B256::from(<[u8;32]>::try_from(row.try_get::<_,Vec<u8>>(0)?).map_err(|_| Error::Inconsistent)?);
        Ok((hash,row.try_get(1)?))
    }).collect()
}
pub(super) async fn touch_posting_attempt(db: &Database, id: i64, hash: B256) -> Result<(), Error> {
    db.client.execute("UPDATE eez.posting_analysis SET last_attempt_at=now() WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea", &[&id,&hash.as_slice()]).bounded().await?;
    Ok(())
}
pub(super) async fn repair_posting(db: &Database, id: i64, tx: &Transaction) -> Result<(), Error> {
    let recipient = tx.recipient.as_ref().map(|address| address.as_slice());
    let digest = tx.posting.calldata_hash.as_ref().map(B256::as_slice);
    let changed = db.client.execute("UPDATE eez.posting_analysis a SET state=$8,calldata_hash=$9::bytea,observed_at=now(),collection_error=NULL FROM eez.transactions t WHERE a.chain_instance_id=$1 AND a.transaction_hash=$2::bytea AND t.chain_instance_id=$1 AND t.hash=$2::bytea AND t.sender=$3::bytea AND t.recipient IS NOT DISTINCT FROM $4::bytea AND t.nonce=$5::text::numeric AND t.transaction_type=$6 AND t.method_selector IS NOT DISTINCT FROM $7", &[&id,&tx.hash.as_slice(),&tx.sender.as_slice(),&recipient,&tx.nonce.to_string(),&tx.kind,&tx.selector,&tx.posting.state,&digest]).bounded().await?;
    if changed != 1 {
        return Err(Error::ConflictingFact);
    }
    Ok(())
}

pub(super) async fn posting_failure(
    db: &Database,
    id: i64,
    hash: B256,
    error: Error,
) -> Result<(), Error> {
    db.client.execute("UPDATE eez.posting_analysis SET collection_error=$3 WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea AND state='pending'", &[&id,&hash.as_slice(),&error.label()]).bounded().await?;
    Ok(())
}
