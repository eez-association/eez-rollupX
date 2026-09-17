//! Source receipt facts are independent of companion attribution. Dependency
//! lookups never extend the configured scan interval or its selected cost totals.
use super::{
    Block, Config, Error, Rpc, SharedStatus, Transaction,
    ledger::{self, Deadline},
    remote_block,
};
use crate::{
    accounting::{Receipt, hash as parse_hash},
    database::{Database, DatabaseConfig},
};
use alloy_primitives::B256;
use eez_telemetry::schema::{Direction, Source};
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct Status {
    last_projection: Option<u64>,
    error: Option<Error>,
    chains: [ChainStatus; 2],
    identity_errors: i64,
    pending_admissions: i64,
    pending_references: i64,
}
#[derive(Debug, Clone, Default)]
struct ChainStatus {
    last_success: Option<u64>,
    error: Option<Error>,
    counts: [i64; 8],
    failures: u32,
    next_attempt: Option<tokio::time::Instant>,
}

#[derive(Debug, Clone, Copy)]
enum Lookup {
    Observed(Option<&'static str>),
    NotObserved,
    OutsideInterval,
    TransactionUnavailable,
    UnindexedBlock,
}
impl Lookup {
    const fn status(self) -> &'static str {
        match self {
            Self::Observed(None) => "observed",
            Self::Observed(Some(_)) => "accounting_gap",
            Self::NotObserved => "not_observed",
            Self::OutsideInterval => "outside_indexed_interval",
            Self::TransactionUnavailable => "transaction_unavailable",
            Self::UnindexedBlock => "source_block_not_indexed",
        }
    }
    const fn error(self) -> Option<&'static str> {
        match self {
            Self::Observed(gap) => gap,
            _ => None,
        }
    }
}

pub(super) fn source_role(config: &Config, source: &Source) -> Option<usize> {
    let (index, endpoint, rollup) = match source.direction {
        Direction::Inbound => (0, &config.exporter.l1, 0),
        Direction::Outbound => (1, &config.exporter.l2, config.rollup_id),
    };
    let genesis: B256 = endpoint.genesis_hash.parse().ok()?;
    (source.chain_id == endpoint.chain_id
        && source.genesis_hash.0 == genesis.0
        && source.rollup_id == rollup)
        .then_some(index)
}

pub fn render(state: Option<&Status>, now: u64) -> String {
    use std::fmt::Write;
    let Some(state) = state else {
        return String::new();
    };
    let fresh = |time: Option<u64>| {
        time.is_some_and(|time| {
            time <= now.saturating_add(5000) && now.saturating_sub(time) <= 90000
        })
    };
    let projected = state.error.is_none() && fresh(state.last_projection);
    let mut result = format!(
        "eez_observer_source_receipt_projection_success {}\n",
        u8::from(projected)
    );
    if let Some(time) = state.last_projection {
        writeln!(
            result,
            "eez_observer_source_receipt_projection_last_success_timestamp_seconds {}",
            time / 1000
        )
        .unwrap();
    }
    if let Some(error) = state.error {
        writeln!(
            result,
            "eez_observer_source_receipt_projection_error{{reason=\"{}\"}} 1",
            error.label()
        )
        .unwrap();
    }
    if projected {
        writeln!(
            result,
            "eez_observer_source_receipt_identity_errors {}",
            state.identity_errors
        )
        .unwrap();
        writeln!(
            result,
            "eez_observer_source_receipt_pending_admissions {}",
            state.pending_admissions
        )
        .unwrap();
        writeln!(
            result,
            "eez_observer_source_receipt_pending_references {}",
            state.pending_references
        )
        .unwrap();
    }
    for (role, chain) in ["l1", "l2"].into_iter().zip(&state.chains) {
        let valid = state.error.is_none() && chain.error.is_none() && fresh(chain.last_success);
        writeln!(
            result,
            "eez_observer_source_receipt_collection_success{{chain=\"{role}\"}} {}",
            u8::from(valid)
        )
        .unwrap();
        if let Some(time) = chain.last_success {
            writeln!(
                result,
                "eez_observer_source_receipt_last_success_timestamp_seconds{{chain=\"{role}\"}} {}",
                time / 1000
            )
            .unwrap();
        }
        if let Some(error) = chain.error {
            writeln!(
                result,
                "eez_observer_source_receipt_error{{chain=\"{role}\",reason=\"{}\"}} 1",
                error.label()
            )
            .unwrap();
        }
        if valid {
            for (name, value) in [
                "known_sources",
                "missing_receipts",
                "accounting_gaps",
                "lookup_errors",
                "not_observed",
                "outside_interval",
                "unavailable_transactions",
                "unindexed_blocks",
            ]
            .into_iter()
            .zip(chain.counts)
            {
                writeln!(
                    result,
                    "eez_observer_source_receipt_{name}{{chain=\"{role}\"}} {value}"
                )
                .unwrap();
            }
        }
    }
    result
}

pub(super) async fn run(config: &Config, rpc: &Rpc, status: &SharedStatus) {
    if config.lifecycle.is_empty() {
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
    if config.lifecycle.is_empty() {
        return true;
    }
    let previous = status
        .read()
        .await
        .source_receipts
        .clone()
        .unwrap_or_default();
    match cycle(config, rpc, previous).await {
        Ok(state) => {
            status.write().await.source_receipts = Some(state);
            true
        }
        Err(error) => {
            status
                .write()
                .await
                .source_receipts
                .get_or_insert_with(Status::default)
                .error = Some(error);
            tracing::warn!(reason = error.label(), "source receipt collection failed");
            false
        }
    }
}

async fn cycle(config: &Config, rpc: &Rpc, mut state: Status) -> Result<Status, Error> {
    let settings = DatabaseConfig::load(&config.database_config).map_err(|_| Error::Schema)?;
    let mut db = Database::connect(&settings)
        .await
        .map_err(|_| Error::Database)?;
    db.verify_writer().await.map_err(|_| Error::Schema)?;
    let chains = [
        ledger::find_chain(&db, config, &config.exporter.l1, "l1")
            .await?
            .ok_or(Error::Bootstrap)?,
        ledger::find_chain(&db, config, &config.exporter.l2, "l2")
            .await?
            .ok_or(Error::Bootstrap)?,
    ];
    let identities = json!(
        config
            .lifecycle
            .iter()
            .map(|source| json!({"service":source.service,"instance":source.instance}))
            .collect::<Vec<_>>()
    );
    let sources=db.client.query("SELECT s.id FROM eez.lifecycle_sources s JOIN eez.deployments d ON d.id=s.deployment_id WHERE d.environment=$1 AND d.name=$2 AND EXISTS(SELECT FROM jsonb_to_recordset($3::jsonb) AS configured(service text,instance text) WHERE configured.service=s.service AND configured.instance=s.instance) ORDER BY s.id",&[&config.environment,&config.deployment,&identities]).bounded().await?;
    let mut source_ids = Vec::new();
    for source in sources {
        let source: i64 = source.try_get(0)?;
        source_ids.push(source);
        project(&mut db, config, source, chains).await?;
    }
    for (index, chain) in chains.into_iter().enumerate() {
        let current = &mut state.chains[index];
        if current
            .next_attempt
            .is_some_and(|at| at > tokio::time::Instant::now())
        {
            continue;
        }
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            collect_chain(&mut db, config, rpc, index, chain),
        )
        .await
        .unwrap_or(Err(Error::SourceBudget));
        match result {
            Ok(()) => {
                let row=db.client.query_one("SELECT known_sources,missing_receipts,accounting_gaps,lookup_errors,not_observed,outside_interval,unavailable_transactions,unindexed_blocks FROM eez.source_receipt_coverage WHERE chain_instance_id=$1",&[&chain]).bounded().await?;
                for (column, count) in current.counts.iter_mut().enumerate() {
                    *count = row.try_get(column)?;
                }
                current.last_success = Some(crate::now_ms());
                current.error = None;
                current.failures = 0;
                current.next_attempt = None;
            }
            Err(error) => {
                current.error = Some(error);
                current.failures = current.failures.saturating_add(1);
                current.next_attempt = Some(
                    tokio::time::Instant::now()
                        + Duration::from_secs(crate::retry_delay(current.failures)),
                );
                tracing::warn!(
                    chain = if index == 0 { "l1" } else { "l2" },
                    reason = error.label(),
                    "source receipt chain collection failed"
                );
            }
        }
    }
    state.error = None;
    state.last_projection = Some(crate::now_ms());
    state.pending_admissions=db.client.query_one("SELECT count(*) FROM eez.lifecycle_events e WHERE e.source_id=ANY($1) AND e.state='complete' AND e.payload->'event'->>'event'='admitted' AND NOT EXISTS(SELECT FROM eez.lifecycle_source_links link WHERE link.source_id=e.source_id AND link.epoch=e.epoch AND link.sequence=e.sequence AND link.context_l1=$2 AND link.context_l2=$3 AND link.source=e.payload->'event'->'source')", &[&source_ids,&chains[0],&chains[1]]).bounded().await?.try_get(0)?;
    state.pending_references=db.client.query_one("SELECT count(*) FROM eez.lifecycle_declared_sources e WHERE e.source_id=ANY($1) AND NOT EXISTS(SELECT FROM eez.lifecycle_source_links link WHERE link.source_id=e.source_id AND link.epoch=e.epoch AND link.sequence=e.sequence AND link.context_l1=$2 AND link.context_l2=$3 AND link.source=e.source)", &[&source_ids,&chains[0],&chains[1]]).bounded().await?.try_get(0)?;
    state.identity_errors=db.client.query_one("SELECT count(*) FROM eez.lifecycle_source_link_status link WHERE link.source_id=ANY($1) AND link.context_l1=$2 AND link.context_l2=$3 AND link.state='identity_mismatch'",&[&source_ids,&chains[0],&chains[1]]).bounded().await?.try_get(0)?;
    Ok(state)
}

async fn collect_chain(
    db: &mut Database,
    config: &Config,
    rpc: &Rpc,
    index: usize,
    chain: i64,
) -> Result<(), Error> {
    let frozen: Option<String> = db
        .client
        .query_one(
            "SELECT frozen_reason FROM eez.chain_instances WHERE id=$1",
            &[&chain],
        )
        .bounded()
        .await?
        .try_get(0)?;
    if frozen.is_some() {
        return Err(Error::Frozen);
    }
    let endpoint = if index == 0 {
        &config.exporter.l1
    } else {
        &config.exporter.l2
    };
    rpc.validate_identity(endpoint).await?;
    // Prefer missing/gapped receipts after a reorg or provider recovery;
    // retain one fair recheck lane for already complete observations.
    let rows=db.client.query("WITH candidates AS (SELECT request.transaction_hash,request.last_attempt_at,NOT EXISTS(SELECT FROM eez.canonical_receipt_facts r WHERE r.chain_instance_id=request.chain_instance_id AND r.transaction_hash=request.transaction_hash AND r.accounting_gap IS NULL) AS pending FROM eez.source_receipt_requests request WHERE request.chain_instance_id=$1 AND EXISTS(SELECT FROM eez.lifecycle_source_references a WHERE a.chain_instance_id=request.chain_instance_id AND a.transaction_hash=request.transaction_hash)) (SELECT transaction_hash::bytea FROM candidates WHERE pending ORDER BY last_attempt_at,transaction_hash LIMIT 31) UNION ALL (SELECT transaction_hash::bytea FROM candidates WHERE NOT pending ORDER BY last_attempt_at,transaction_hash LIMIT 1)",&[&chain]).bounded().await?;
    for row in rows {
        let hash = decode_hash(row.try_get(0)?)?;
        db.client.execute("UPDATE eez.source_receipt_requests SET last_attempt_at=now() WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea",&[&chain,&hash.as_slice()]).bounded().await?;
        let lookup = lookup(db, config, rpc, index, chain, hash).await;
        let lookup = match lookup {
            Ok(result) => result,
            Err(error) => {
                if error == Error::ConflictingFact {
                    ledger::mark_failure(db, chain, error).await?;
                }
                db.client.execute("UPDATE eez.source_receipt_requests SET collection_error=$3,observation_status='failed' WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea",&[&chain,&hash.as_slice(),&error.label()]).bounded().await?;
                return Err(error);
            }
        };
        let found = matches!(lookup, Lookup::Observed(_));
        db.client.execute("UPDATE eez.source_receipt_requests SET collection_error=$3,observation_status=$5,last_success_at=CASE WHEN $4 THEN now() ELSE last_success_at END WHERE chain_instance_id=$1 AND transaction_hash=$2::bytea",&[&chain,&hash.as_slice(),&lookup.error(),&found,&lookup.status()]).bounded().await?;
    }
    Ok(())
}

async fn project(
    db: &mut Database,
    config: &Config,
    source_id: i64,
    chains: [i64; 2],
) -> Result<(), Error> {
    // Return at most 64 distinct event/source objects, not their potentially
    // large enclosing manifests. Repeated members share a single receipt link.
    let rows=db.client.query("SELECT e.epoch::bytea,e.sequence::text,e.source FROM eez.lifecycle_declared_sources e WHERE e.source_id=$1 AND NOT EXISTS(SELECT FROM eez.lifecycle_source_links link WHERE link.source_id=e.source_id AND link.epoch=e.epoch AND link.sequence=e.sequence AND link.context_l1=$2 AND link.context_l2=$3 AND link.source=e.source) ORDER BY e.epoch,e.sequence,e.source LIMIT 64",&[&source_id,&chains[0],&chains[1]]).bounded().await?;
    for row in rows {
        let epoch: Vec<u8> = row.try_get(0)?;
        let sequence: String = row.try_get(1)?;
        let source_value: serde_json::Value = row.try_get(2)?;
        let source: Source =
            serde_json::from_value(source_value.clone()).map_err(|_| Error::Inconsistent)?;
        let chain = source_role(config, &source).map(|index| chains[index]);
        let state = if chain.is_some() {
            "linked"
        } else {
            "identity_mismatch"
        };
        let transaction = db.client.transaction().bounded().await?;
        let current=transaction.query_one("SELECT state='complete' AND EXISTS(SELECT FROM eez.lifecycle_event_sources(payload) source WHERE source=$4::jsonb) FROM eez.lifecycle_events WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric FOR SHARE",&[&source_id,&epoch,&sequence,&source_value]).bounded().await?;
        if !current.try_get::<_, bool>(0)? {
            continue;
        }
        transaction.execute("INSERT INTO eez.lifecycle_source_links(source_id,epoch,sequence,state,chain_instance_id,transaction_hash,context_l1,context_l2,source) VALUES($1,$2::bytea,$3::text::numeric,$4,$5,$6::bytea,$7,$8,$9) ON CONFLICT DO NOTHING",&[&source_id,&epoch,&sequence,&state,&chain,&source.transaction_hash.0.as_slice(),&chains[0],&chains[1],&source_value]).bounded().await?;
        if let Some(chain) = chain {
            transaction.execute("INSERT INTO eez.source_receipt_requests(chain_instance_id,transaction_hash) VALUES($1,$2::bytea) ON CONFLICT DO NOTHING",&[&chain,&source.transaction_hash.0.as_slice()]).bounded().await?;
        }
        transaction.commit().bounded().await?;
    }
    Ok(())
}

fn decode_hash(bytes: Vec<u8>) -> Result<B256, Error> {
    Ok(B256::from(
        <[u8; 32]>::try_from(bytes).map_err(|_| Error::Inconsistent)?,
    ))
}

async fn lookup(
    db: &mut Database,
    config: &Config,
    rpc: &Rpc,
    index: usize,
    chain: i64,
    hash: B256,
) -> Result<Lookup, Error> {
    let (endpoint, selection) = if index == 0 {
        (&config.exporter.l1, &config.l1)
    } else {
        (&config.exporter.l2, &config.l2)
    };
    let cursor = ledger::cursor(db, chain).await?.ok_or(Error::Bootstrap)?;
    let value = rpc.receipt(endpoint, hash).await?;
    if value.is_null() {
        return Ok(Lookup::NotObserved);
    }
    let receipt = Receipt::parse(&value)?;
    if receipt.transaction_hash != hash {
        return Err(Error::Inconsistent);
    }
    if receipt.block_number < selection.bootstrap || receipt.block_number > cursor.number {
        return Ok(Lookup::OutsideInterval);
    }
    let value_block = rpc
        .block(
            endpoint,
            json!(format!("{:#x}", receipt.block_number)),
            false,
        )
        .await?;
    let block = Block::parse(&value_block)?;
    let position = usize::try_from(receipt.transaction_index).map_err(|_| Error::Inconsistent)?;
    let transactions = value_block["transactions"]
        .as_array()
        .ok_or(Error::Inconsistent)?;
    if block.number != receipt.block_number
        || block.hash != receipt.block_hash
        || transactions.len() > 10_000
        || transactions.get(position).map(parse_hash).transpose()? != Some(hash)
    {
        return Err(Error::Inconsistent);
    }
    let raw = rpc.transaction(endpoint, hash).await?;
    if raw.is_null() {
        return Ok(Lookup::TransactionUnavailable);
    }
    let tx = Transaction::parse(
        &raw,
        &block,
        usize::try_from(receipt.transaction_index).map_err(|_| Error::Inconsistent)?,
    )?;
    if tx.hash != hash {
        return Err(Error::Inconsistent);
    }
    let receipt = tx.parse_receipt(&block, &value)?;
    rpc.validate_identity(endpoint).await?;
    if remote_block(rpc, endpoint, cursor.number).await?.hash != cursor.hash {
        return Err(Error::Inconsistent);
    }
    let selected = selection.includes(&tx);
    let transaction = db.client.transaction().bounded().await?;
    ledger::lock_cursor(&transaction, chain, Some(&cursor)).await?;
    let canonical:bool=transaction.query_one("SELECT EXISTS(SELECT FROM eez.blocks WHERE chain_instance_id=$1 AND hash=$2::bytea AND number=$3::text::numeric AND canonical)",&[&chain,&block.hash.as_slice(),&block.number.to_string()]).bounded().await?.try_get(0)?;
    if !canonical {
        return Ok(Lookup::UnindexedBlock);
    }
    ledger::save_receipt(&transaction, chain, &block, &tx, Some(&receipt), selected).await?;
    if selected {
        ledger::refresh_coverage(&transaction, chain, &block).await?;
    }
    transaction.commit().bounded().await?;
    Ok(Lookup::Observed(
        receipt.cost.as_ref().err().map(|gap| gap.label()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_telemetry::schema::Hash32;
    fn config() -> Config {
        serde_json::from_value(json!({"exporter":{"l1":{"url":"http://localhost:1","chain_id":1,"genesis_hash":format!("0x{}","01".repeat(32))},"l2":{"url":"http://localhost:2","chain_id":2,"genesis_hash":format!("0x{}","02".repeat(32))}},"environment":"devnet","deployment":"fixture","rollup_id":7,"database_config":"/private/writer.json","l1":{"bootstrap":0,"include_all_transactions":true},"l2":{"bootstrap":0,"include_all_transactions":true}})).unwrap()
    }
    #[test]
    fn source_identity_requires_direction_chain_genesis_and_rollup_agreement() {
        let config = config();
        let inbound = Source {
            chain_id: 1,
            genesis_hash: Hash32([1; 32]),
            rollup_id: 0,
            transaction_hash: Hash32([3; 32]),
            direction: Direction::Inbound,
        };
        let outbound = Source {
            chain_id: 2,
            genesis_hash: Hash32([2; 32]),
            rollup_id: 7,
            direction: Direction::Outbound,
            ..inbound
        };
        assert_eq!(source_role(&config, &inbound), Some(0));
        assert_eq!(source_role(&config, &outbound), Some(1));
        for original in [inbound, outbound] {
            for invalid in [
                Source {
                    chain_id: 99,
                    ..original
                },
                Source {
                    genesis_hash: Hash32([9; 32]),
                    ..original
                },
                Source {
                    rollup_id: 99,
                    ..original
                },
                Source {
                    direction: if original.direction == Direction::Inbound {
                        Direction::Outbound
                    } else {
                        Direction::Inbound
                    },
                    ..original
                },
            ] {
                assert!(source_role(&config, &invalid).is_none());
            }
        }
    }
    #[test]
    fn source_receipt_metrics_hide_stale_counts_and_isolate_failed_chains() {
        assert!(render(None, 0).is_empty());
        let mut state = Status {
            last_projection: Some(100_000),
            identity_errors: 9,
            pending_admissions: 10,
            pending_references: 11,
            chains: [
                ChainStatus {
                    last_success: Some(100_000),
                    counts: [1, 2, 3, 4, 0, 0, 0, 0],
                    ..Default::default()
                },
                ChainStatus {
                    last_success: Some(100_000),
                    counts: [5, 6, 7, 8, 0, 0, 0, 0],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let text = render(Some(&state), 190_000);
        assert!(text.contains("eez_observer_source_receipt_known_sources{chain=\"l1\"} 1"));
        assert!(text.contains("eez_observer_source_receipt_lookup_errors{chain=\"l2\"} 8"));
        assert!(text.contains("eez_observer_source_receipt_identity_errors 9"));
        assert!(text.contains("eez_observer_source_receipt_pending_admissions 10"));
        assert!(text.contains("eez_observer_source_receipt_pending_references 11"));
        for now in [190_001, 94_999] {
            let text = render(Some(&state), now);
            assert!(!text.contains("known_sources"));
            assert!(text.contains("last_success_timestamp_seconds{chain=\"l1\"} 100"));
            assert!(text.contains("collection_success{chain=\"l1\"} 0"));
        }
        state.chains[0].error = Some(Error::Inconsistent);
        let text = render(Some(&state), 100_000);
        assert!(!text.contains("known_sources{chain=\"l1\"}"));
        assert!(text.contains("known_sources{chain=\"l2\"} 5"));
        assert!(text.contains("reason=\"inconsistent_chain_facts\""));
        state.error = Some(Error::Database);
        assert!(!render(Some(&state), 100_000).contains("known_sources"));
    }
}
