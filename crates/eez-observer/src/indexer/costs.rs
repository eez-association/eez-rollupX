//! Bounded projection of paid attempts, including those with only requeued users.
use super::{
    Config, Error, SharedStatus,
    ledger::{self, Deadline},
    outcomes::{Loaded, load},
};
use crate::{
    costs::paid_attempt,
    database::{Database, DatabaseConfig},
    outcomes::MAX_LINEAGE,
};
use eez_telemetry::schema::{ChainScope, Epoch, Event, Hash32};
use serde_json::{Value, json};
use std::{fmt::Write as _, time::Duration};

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub last_success: Option<u64>,
    pub error: Option<Error>,
}

pub fn render(status: Option<&Status>, now: u64) -> String {
    let Some(status) = status else {
        return String::new();
    };
    let valid = status.error.is_none()
        && status
            .last_success
            .is_some_and(|at| at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90_000);
    let mut text = format!("eez_observer_cost_projection_success {}\n", u8::from(valid));
    if let Some(at) = status.last_success {
        let _ = writeln!(
            text,
            "eez_observer_cost_projection_last_success_timestamp_seconds {}",
            at / 1000
        );
    }
    text
}

pub(super) async fn run(config: &Config, status: &SharedStatus) {
    if config.lifecycle.is_empty() {
        return;
    }
    let mut failures = 0u32;
    loop {
        failures = if poll(config, status).await {
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

pub(super) async fn poll(config: &Config, status: &SharedStatus) -> bool {
    if config.lifecycle.is_empty() {
        return true;
    }
    match tokio::time::timeout(Duration::from_secs(30), cycle(config))
        .await
        .unwrap_or(Err(Error::CostBudget))
    {
        Ok(()) => {
            status.write().await.costs = Some(Status {
                last_success: Some(crate::now_ms()),
                error: None,
            });
            true
        }
        Err(error) => {
            status
                .write()
                .await
                .costs
                .get_or_insert_with(Status::default)
                .error = Some(error);
            tracing::warn!(
                reason = error.label(),
                "native cost membership projection failed"
            );
            false
        }
    }
}

struct Check {
    state: &'static str,
    generation: Option<String>,
    references: Value,
    decision: Option<Value>,
}
impl Check {
    fn gap(state: &'static str) -> Self {
        Self {
            state,
            generation: None,
            references: json!([]),
            decision: None,
        }
    }
}

async fn check(
    db: &Database,
    scopes: &[ChainScope; 2],
    source: i64,
    epoch: Epoch,
    sequence: u64,
) -> Result<Check, Error> {
    let mut remaining = 32 * 1024 * 1024;
    let parent = match load(db, source, epoch, sequence, &mut remaining).await? {
        Ok(record) => record,
        Err(state) => return Ok(Check::gap(state)),
    };
    let Event::Reconciled { attempt, .. } = &parent.payload.event else {
        return Ok(Check::gap("missing_parent"));
    };
    if attempt.epoch != epoch {
        return Ok(Check::gap("identity"));
    }
    let rows = db.client.query(
        "SELECT sequence::text FROM eez.lifecycle_native_lineage WHERE source_id=$1 AND epoch=$2::bytea AND sequence<$3::text::numeric AND generation=$4 ORDER BY sequence LIMIT 129",
        &[&source, &epoch.0.as_slice(), &sequence.to_string(), &attempt.generation.to_string()],
    ).bounded().await?;
    if rows.len() > MAX_LINEAGE {
        return Ok(Check::gap("limit"));
    }
    let mut lineage = Vec::with_capacity(rows.len());
    for row in rows {
        let sequence = row
            .try_get::<_, String>(0)?
            .parse()
            .map_err(|_| Error::Inconsistent)?;
        match load(db, source, epoch, sequence, &mut remaining).await? {
            Ok(record) => lineage.push(record),
            Err(state) => return Ok(Check::gap(state)),
        }
    }
    let records: Vec<_> = lineage.iter().map(|record| record.record(source)).collect();
    let paid = match paid_attempt(scopes, parent.record(source), &records) {
        Ok(paid) => paid,
        Err(gap) => return Ok(Check::gap(gap.label())),
    };
    // Identical native populations from different admissions/producers share one
    // cost population. Stable source order also defines the integer remainder rule.
    let mut population: Vec<_> = paid
        .membership
        .members
        .iter()
        .map(|member| member.source)
        .collect();
    population.sort_by_key(|source| {
        (
            source.chain_id,
            source.genesis_hash.0,
            source.rollup_id,
            u8::from(source.direction == eez_telemetry::schema::Direction::Outbound),
            source.transaction_hash.0,
        )
    });
    let mut references = vec![parent.reference()];
    references.extend(lineage.iter().map(Loaded::reference));
    Ok(Check {
        state: "coherent",
        generation: Some(paid.owner.generation.to_string()),
        references: json!(references),
        decision: Some(
            json!({"owner":paid.owner,"snapshot":paid.snapshot,"post":paid.post,
            "population":population,"sources":paid.reconciliation.sources.iter().map(|source|
                json!({"transaction_hash":source.transaction_hash,"direction":source.direction,"receipt":source.receipt})
            ).collect::<Vec<_>>()}),
        ),
    })
}

async fn cycle(config: &Config) -> Result<(), Error> {
    let settings = DatabaseConfig::load(&config.database_config).map_err(|_| Error::Schema)?;
    let db = Database::connect(&settings)
        .await
        .map_err(|_| Error::Database)?;
    db.verify_writer().await.map_err(|_| Error::Schema)?;
    let mut chains = [0i64; 2];
    let mut scopes = Vec::with_capacity(2);
    for (index, (role, endpoint)) in [("l1", &config.exporter.l1), ("l2", &config.exporter.l2)]
        .into_iter()
        .enumerate()
    {
        chains[index] = ledger::find_chain(&db, config, endpoint, role)
            .await?
            .ok_or(Error::Bootstrap)?;
        let hash: alloy_primitives::B256 = endpoint
            .genesis_hash
            .parse()
            .map_err(|_| Error::Inconsistent)?;
        scopes.push(ChainScope {
            chain_id: endpoint.chain_id,
            genesis_hash: Hash32(hash.0),
            rollup_id: if index == 0 { 0 } else { config.rollup_id },
        });
    }
    let scopes: [ChainScope; 2] = scopes.try_into().map_err(|_| Error::Inconsistent)?;
    let identities = json!(
        config
            .lifecycle
            .iter()
            .map(|source| json!({"service":source.service,"instance":source.instance}))
            .collect::<Vec<_>>()
    );
    let rows = db.client.query(
        "WITH candidates AS (SELECT e.source_id,e.epoch,e.sequence,e.first_seen_at,previous.last_attempt_at FROM eez.lifecycle_events e JOIN eez.lifecycle_sources s ON s.id=e.source_id JOIN eez.deployments d ON d.id=s.deployment_id LEFT JOIN eez.lifecycle_cost_checks previous ON previous.source_id=e.source_id AND previous.epoch=e.epoch AND previous.sequence=e.sequence AND previous.context_l1=$4 AND previous.context_l2=$5 WHERE d.environment=$1 AND d.name=$2 AND EXISTS(SELECT FROM jsonb_to_recordset($3::jsonb) configured(service text,instance text) WHERE configured.service=s.service AND configured.instance=s.instance) AND e.state='complete' AND e.payload->'event'->>'event'='reconciled') SELECT source_id,epoch::bytea,sequence::text FROM ((SELECT source_id,epoch,sequence,row_number() OVER(ORDER BY first_seen_at,source_id,epoch,sequence) AS turn,false AS repair FROM candidates WHERE last_attempt_at IS NULL ORDER BY first_seen_at,source_id,epoch,sequence LIMIT 24) UNION ALL (SELECT source_id,epoch,sequence,row_number() OVER(ORDER BY last_attempt_at,source_id,epoch,sequence) AS turn,true AS repair FROM candidates WHERE last_attempt_at IS NOT NULL ORDER BY last_attempt_at,source_id,epoch,sequence LIMIT 8)) scheduled ORDER BY turn,repair DESC",
        &[&config.environment,&config.deployment,&identities,&chains[0],&chains[1]],
    ).bounded().await?;
    for row in rows {
        let source: i64 = row.try_get(0)?;
        let epoch = Epoch(
            row.try_get::<_, Vec<u8>>(1)?
                .try_into()
                .map_err(|_| Error::Inconsistent)?,
        );
        let sequence: u64 = row
            .try_get::<_, String>(2)?
            .parse()
            .map_err(|_| Error::Inconsistent)?;
        db.client.execute(
            "INSERT INTO eez.lifecycle_cost_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs) VALUES($1,$2::bytea,$3::text::numeric,$4,$5,'missing_record','[]'::jsonb) ON CONFLICT(source_id,epoch,sequence,context_l1,context_l2) DO UPDATE SET last_attempt_at=now()",
            &[&source,&epoch.0.as_slice(),&sequence.to_string(),&chains[0],&chains[1]],
        ).bounded().await?;
        let checked = check(&db, &scopes, source, epoch, sequence).await?;
        db.client.execute(
            "UPDATE eez.lifecycle_cost_checks SET last_attempt_at=now(),state=$6,generation=$7::text::numeric,record_refs=$8,decision=$9 WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric AND context_l1=$4 AND context_l2=$5",
            &[&source,&epoch.0.as_slice(),&sequence.to_string(),&chains[0],&chains[1],&checked.state,&checked.generation,&checked.references,&checked.decision],
        ).bounded().await?;
    }
    Ok(())
}
