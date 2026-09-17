//! Bounded native coherence projection. Canonical anchors are a separate gate;
//! neither a complete transport event nor a coherent decision means completion.
use super::{Config, Error, SharedStatus, ledger, ledger::Deadline};
use crate::{
    database::{Database, DatabaseConfig},
    outcomes::{self, Gap, MAX_LINEAGE, Record},
};
use alloy_primitives::B256;
use eez_telemetry::schema::{ChainScope, Direction, Epoch, Event, Hash32, Outcome, Payload};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

// JSONB's normalized text can exceed the original 8 MiB wire payload. Fetch
// dependencies individually, charging the normalized bytes before downloading.
const MAX_RECORD_BYTES: i64 = 16 * 1024 * 1024;
const MAX_CHECK_BYTES: i64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Status {
    last_success: Option<u64>,
    error: Option<Error>,
    canonical: Option<Canonical>,
}

pub fn render(state: Option<&Status>, now: u64) -> String {
    use std::fmt::Write;
    let Some(state) = state else {
        return String::new();
    };
    let valid = state.error.is_none()
        && state
            .last_success
            .is_some_and(|at| at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000);
    let mut text = format!(
        "eez_observer_native_outcome_projection_success {}\n",
        u8::from(valid)
    );
    if let Some(at) = state.last_success {
        writeln!(
            text,
            "eez_observer_native_outcome_projection_last_success_timestamp_seconds {}",
            at / 1000
        )
        .unwrap();
    }
    if let Some(error) = state.error {
        writeln!(
            text,
            "eez_observer_native_outcome_projection_error{{reason=\"{}\"}} 1",
            error.label()
        )
        .unwrap();
    }
    text
}

const OUTCOMES: &[(Outcome, &str)] = &[
    (Outcome::Committed, "committed"),
    (Outcome::SourceReverted, "source_reverted"),
    (Outcome::SourceCompleted, "source_completed"),
    (
        Outcome::SourceCompletedWithMissingEffects,
        "source_completed_with_missing_effects",
    ),
    (Outcome::SourceNonceSpent, "source_nonce_spent"),
];

#[derive(Debug, Clone, Deserialize)]
struct Progress {
    direction: Direction,
    outcome: Outcome,
    count: u64,
    timestamp: Option<u64>,
}
#[derive(Debug, Clone)]
struct Canonical {
    valid: bool,
    cursors: [B256; 2],
    unresolved: i64,
    rows: Vec<Progress>,
}

/// Recomputed admission totals, never process counters. Cursor fencing suppresses
/// an old snapshot as soon as the scanner publishes another canonical cursor.
pub fn render_progress(runtime: &super::RuntimeStatus, now: u64) -> String {
    use std::fmt::Write;
    let Some(state) = runtime.outcomes.as_ref() else {
        return String::new();
    };
    let fresh = |at: Option<u64>| {
        at.is_some_and(|at| at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000)
    };
    let valid = state.error.is_none()
        && fresh(state.last_success)
        && state.canonical.as_ref().is_some_and(|snapshot| {
            snapshot.valid
                && runtime
                    .chains
                    .iter()
                    .zip(snapshot.cursors)
                    .all(|(chain, cursor)| {
                        chain.error.is_none()
                            && fresh(chain.last_success)
                            && chain.cursor_hash == Some(cursor)
                    })
        });
    let mut text = format!(
        "eez_observer_canonical_outcome_evaluation_valid {}\n",
        u8::from(valid)
    );
    let Some(snapshot) = state.canonical.as_ref().filter(|_| valid) else {
        return text;
    };
    writeln!(
        text,
        "eez_observer_canonical_outcome_unresolved_admissions {}",
        snapshot.unresolved
    )
    .unwrap();
    for (direction, label) in [
        (Direction::Inbound, "inbound"),
        (Direction::Outbound, "outbound"),
    ] {
        let mut last = None;
        for (outcome, outcome_label) in OUTCOMES {
            let row = snapshot
                .rows
                .iter()
                .find(|row| row.direction == direction && row.outcome == *outcome);
            writeln!(text,"eez_observer_canonical_terminal_admissions{{direction=\"{label}\",outcome=\"{outcome_label}\"}} {}",row.map_or(0,|row|row.count)).unwrap();
            if let Some(timestamp) = row.and_then(|row| row.timestamp) {
                last = Some(last.map_or(timestamp, |old: u64| old.max(timestamp)));
                if *outcome == Outcome::Committed {
                    writeln!(text,"eez_observer_last_canonical_commit_timestamp_seconds{{direction=\"{label}\"}} {timestamp}").unwrap();
                }
            }
        }
        if let Some(timestamp) = last {
            writeln!(text,"eez_observer_last_canonical_terminal_timestamp_seconds{{direction=\"{label}\"}} {timestamp}").unwrap();
        }
    }
    text
}

// One SQL statement observes counts, health and cursors in the same MVCC snapshot.
async fn canonical_snapshot(db: &Database, chains: [i64; 2]) -> Result<Canonical, Error> {
    let row = db.client.query_one(
        "WITH outcomes AS (SELECT * FROM eez.lifecycle_canonical_outcomes WHERE context_l1=$1 AND context_l2=$2), progress AS (SELECT source->>'direction' AS direction,outcome,count(*) AS count,max(progress_timestamp_seconds) AS timestamp FROM outcomes WHERE evidence_class='node_reconciled' GROUP BY source->>'direction',outcome) SELECT (l1.frozen_reason IS NULL AND l2.frozen_reason IS NULL AND l1.collection_error IS NULL AND l2.collection_error IS NULL AND l1.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds' AND l2.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds' AND NOT EXISTS(SELECT FROM eez.correlation_sources c WHERE c.l1_chain_id=$1 AND c.l2_chain_id=$2 AND (c.frozen_reason IS NOT NULL OR c.collection_error IS NOT NULL OR c.last_success_at IS NULL OR c.last_success_at NOT BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds'))) IS TRUE, c1.block_hash::bytea,c2.block_hash::bytea,(SELECT count(*) FROM outcomes WHERE evidence_class='unknown' AND native_claims>0),COALESCE((SELECT jsonb_agg(to_jsonb(progress)) FROM progress),'[]'::jsonb) FROM eez.chain_instances l1 JOIN eez.chain_instances l2 ON l2.id=$2 LEFT JOIN eez.scan_cursors c1 ON c1.chain_instance_id=l1.id LEFT JOIN eez.scan_cursors c2 ON c2.chain_instance_id=l2.id WHERE l1.id=$1",
        &[&chains[0],&chains[1]],
    ).bounded().await?;
    let cursor = |column: usize| -> Result<B256, Error> {
        let bytes: Option<Vec<u8>> = row.try_get(column)?;
        Ok(B256::from(
            <[u8; 32]>::try_from(bytes.ok_or(Error::Bootstrap)?)
                .map_err(|_| Error::Inconsistent)?,
        ))
    };
    let rows: Vec<Progress> =
        serde_json::from_value(row.try_get(4)?).map_err(|_| Error::Inconsistent)?;
    if rows.len() > 10
        || rows
            .iter()
            .any(|row| !OUTCOMES.iter().any(|(outcome, _)| *outcome == row.outcome))
    {
        return Err(Error::Inconsistent);
    }
    Ok(Canonical {
        valid: row.try_get(0)?,
        cursors: [cursor(1)?, cursor(2)?],
        unresolved: row.try_get(3)?,
        rows,
    })
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
        .unwrap_or(Err(Error::OutcomeBudget))
    {
        Ok(canonical) => {
            status.write().await.outcomes = Some(Status {
                last_success: Some(crate::now_ms()),
                error: None,
                canonical: Some(canonical),
            });
            true
        }
        Err(error) => {
            status
                .write()
                .await
                .outcomes
                .get_or_insert_with(Status::default)
                .error = Some(error);
            tracing::warn!(reason = error.label(), "native outcome projection failed");
            false
        }
    }
}

struct Loaded {
    epoch: Epoch,
    sequence: u64,
    digest: String,
    payload: Payload,
}
impl Loaded {
    fn record(&self, source_id: i64) -> Record<'_> {
        Record {
            source_id,
            epoch: self.epoch,
            sequence: self.sequence,
            event: &self.payload.event,
        }
    }
    fn reference(&self) -> Value {
        json!({"epoch":self.epoch,"sequence":self.sequence,"digest":self.digest})
    }
}

// Expected absence/limits are evidence gaps; database failures abort the pass.
async fn load(
    db: &Database,
    source: i64,
    epoch: Epoch,
    sequence: u64,
    remaining: &mut i64,
) -> Result<Result<Loaded, &'static str>, Error> {
    let row = db.client.query_opt(
        "SELECT digest,octet_length(payload::text)::bigint,CASE WHEN octet_length(payload::text)::bigint<=$4 THEN payload::text END FROM eez.lifecycle_native_records WHERE source_id=$1 AND epoch=$2::bytea AND sequence=$3::text::numeric",
        &[&source, &epoch.0.as_slice(), &sequence.to_string(), &(*remaining).min(MAX_RECORD_BYTES)],
    ).bounded().await?;
    let Some(row) = row else {
        return Ok(Err("missing_record"));
    };
    let size: i64 = row.try_get(1)?;
    let Some(raw): Option<String> = row.try_get(2)? else {
        return Ok(Err("limit"));
    };
    *remaining -= size;
    let payload = serde_json::from_str(&raw).map_err(|_| Error::Inconsistent)?;
    Ok(Ok(Loaded {
        epoch,
        sequence,
        digest: row.try_get(0)?,
        payload,
    }))
}

struct Check {
    state: &'static str,
    references: Vec<Value>,
    parent_sequence: Option<String>,
    generation: Option<String>,
    decision: Option<Value>,
}
impl Check {
    fn gap(state: &'static str) -> Self {
        Self {
            state,
            references: Vec::new(),
            parent_sequence: None,
            generation: None,
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
    let mut remaining = MAX_CHECK_BYTES;
    let terminal = match load(db, source, epoch, sequence, &mut remaining).await? {
        Ok(record) => record,
        Err(state) => return Ok(Check::gap(state)),
    };
    let Event::Terminal {
        attempt: Some(owner),
        reconciliation: Some(parent_ref),
        ..
    } = &terminal.payload.event
    else {
        return Ok(Check::gap(Gap::UnsupportedClaim.label()));
    };
    // Fence the query before interpreting producer-provided references.
    if owner.epoch != epoch || parent_ref.epoch != epoch || parent_ref.sequence >= sequence {
        return Ok(Check::gap(Gap::Identity.label()));
    }
    let parent = match load(db, source, epoch, parent_ref.sequence, &mut remaining).await? {
        Ok(record) => record,
        Err(state) => return Ok(Check::gap(state)),
    };
    let rows = db.client.query(
        "SELECT sequence::text FROM eez.lifecycle_native_lineage WHERE source_id=$1 AND epoch=$2::bytea AND sequence<$3::text::numeric AND generation=$4 ORDER BY sequence LIMIT 129",
        &[&source, &epoch.0.as_slice(), &parent_ref.sequence.to_string(), &owner.generation.to_string()],
    ).bounded().await?;
    if rows.len() > MAX_LINEAGE {
        return Ok(Check::gap(Gap::Limit.label()));
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
    let decision = match outcomes::native_decision(
        scopes,
        terminal.record(source),
        parent.record(source),
        &records,
    ) {
        Ok(decision) => decision,
        Err(gap) => return Ok(Check::gap(gap.label())),
    };
    let mut references = vec![terminal.reference(), parent.reference()];
    references.extend(lineage.iter().map(Loaded::reference));
    Ok(Check {
        state: "coherent",
        references,
        parent_sequence: Some(parent.sequence.to_string()),
        generation: Some(owner.generation.to_string()),
        // Retain only the particular source's evidence. The shared full manifest
        // remains in the digest-bound parent/prepared records, without N copies.
        decision: Some(json!({
            "member":decision.member,"owner":decision.owner,"outcome":decision.outcome,
            "elapsed_ns":decision.elapsed_ns,"l1_snapshot":decision.l1_snapshot,
            "l2_snapshot":decision.details.l2_snapshot,"post":decision.details.post,
            "applied":decision.details.applied,"source":decision.source,"plan":decision.plan,
            "observed_at_ms":terminal.payload.observed_at_unix_ms
        })),
    })
}

async fn cycle(config: &Config) -> Result<Canonical, Error> {
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
        let genesis: B256 = endpoint
            .genesis_hash
            .parse()
            .map_err(|_| Error::Inconsistent)?;
        scopes.push(ChainScope {
            chain_id: endpoint.chain_id,
            genesis_hash: Hash32(genesis.0),
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
    // Interleave repairs and new work, starting with a repair. Merely appending
    // repairs after 24 new checks could starve them at the pass deadline.
    let rows = db.client.query(
        "WITH candidates AS (SELECT e.source_id,e.epoch,e.sequence,e.first_seen_at,previous.last_attempt_at FROM eez.lifecycle_events e JOIN eez.lifecycle_sources s ON s.id=e.source_id JOIN eez.deployments d ON d.id=s.deployment_id LEFT JOIN eez.lifecycle_native_checks previous ON previous.source_id=e.source_id AND previous.epoch=e.epoch AND previous.sequence=e.sequence AND previous.context_l1=$4 AND previous.context_l2=$5 WHERE d.environment=$1 AND d.name=$2 AND EXISTS(SELECT FROM jsonb_to_recordset($3::jsonb) configured(service text,instance text) WHERE configured.service=s.service AND configured.instance=s.instance) AND e.state='complete' AND e.payload->'event'->>'event'='terminal' AND e.payload->'event'->>'assurance'='node_reconciled') SELECT source_id,epoch::bytea,sequence::text FROM ((SELECT source_id,epoch,sequence,row_number() OVER(ORDER BY first_seen_at,source_id,epoch,sequence) AS turn,false AS repair FROM candidates WHERE last_attempt_at IS NULL ORDER BY first_seen_at,source_id,epoch,sequence LIMIT 24) UNION ALL (SELECT source_id,epoch,sequence,row_number() OVER(ORDER BY last_attempt_at,source_id,epoch,sequence) AS turn,true AS repair FROM candidates WHERE last_attempt_at IS NOT NULL ORDER BY last_attempt_at,source_id,epoch,sequence LIMIT 8)) scheduled ORDER BY turn,repair DESC",
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
        // Reserve the attempt before expensive reads. A cancelled/failed check
        // therefore moves to the fair retry lane instead of blocking new work.
        // Existing coherent evidence remains fenced by the reader view.
        db.client.execute(
            "INSERT INTO eez.lifecycle_native_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs) VALUES($1,$2::bytea,$3::text::numeric,$4,$5,'missing_record','[]'::jsonb) ON CONFLICT(source_id,epoch,sequence,context_l1,context_l2) DO UPDATE SET last_attempt_at=now()",
            &[&source,&epoch.0.as_slice(),&sequence.to_string(),&chains[0],&chains[1]],
        ).bounded().await?;
        let result = check(&db, &scopes, source, epoch, sequence).await?;
        let references = json!(result.references);
        db.client.execute(
            "INSERT INTO eez.lifecycle_native_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs,parent_sequence,generation,decision) VALUES($1,$2::bytea,$3::text::numeric,$4,$5,$6,$7,$8::text::numeric,$9::text::numeric,$10) ON CONFLICT(source_id,epoch,sequence,context_l1,context_l2) DO UPDATE SET last_attempt_at=now(),state=EXCLUDED.state,record_refs=EXCLUDED.record_refs,parent_sequence=EXCLUDED.parent_sequence,generation=EXCLUDED.generation,decision=EXCLUDED.decision",
            &[&source,&epoch.0.as_slice(),&sequence.to_string(),&chains[0],&chains[1],&result.state,&references,&result.parent_sequence,&result.generation,&result.decision],
        ).bounded().await?;
    }
    canonical_snapshot(&db, chains).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_progress_is_recomputed_and_fenced_by_cursor_and_health() {
        let cursors = [B256::repeat_byte(1), B256::repeat_byte(2)];
        let mut runtime = super::super::RuntimeStatus {
            chains: cursors.map(|cursor| super::super::Status {
                last_success: Some(100_000),
                cursor_hash: Some(cursor),
                ..Default::default()
            }),
            outcomes: Some(Status {
                last_success: Some(100_000),
                error: None,
                canonical: Some(Canonical {
                    valid: true,
                    cursors,
                    unresolved: 3,
                    rows: vec![
                        Progress {
                            direction: Direction::Inbound,
                            outcome: Outcome::Committed,
                            count: 2,
                            timestamp: Some(5),
                        },
                        Progress {
                            direction: Direction::Inbound,
                            outcome: Outcome::SourceReverted,
                            count: 1,
                            timestamp: Some(10),
                        },
                    ],
                }),
            }),
            ..Default::default()
        };
        let rendered = render_progress(&runtime, 100_000);
        assert!(rendered.contains("evaluation_valid 1"));
        assert!(rendered.contains("unresolved_admissions 3"));
        assert!(
            rendered.contains("last_canonical_commit_timestamp_seconds{direction=\"inbound\"} 5")
        );
        assert!(
            rendered
                .contains("last_canonical_terminal_timestamp_seconds{direction=\"inbound\"} 10")
        );
        assert!(rendered.contains(
            "canonical_terminal_admissions{direction=\"inbound\",outcome=\"committed\"} 2"
        ));
        assert!(!rendered.contains("timestamp_seconds{direction=\"outbound\"}"));
        runtime.chains[0].cursor_hash = Some(B256::repeat_byte(3));
        assert_eq!(
            render_progress(&runtime, 100_000),
            "eez_observer_canonical_outcome_evaluation_valid 0\n"
        );
        runtime.chains[0].cursor_hash = Some(cursors[0]);
        runtime.chains[1].error = Some(Error::Frozen);
        assert!(!render_progress(&runtime, 100_000).contains("terminal_admissions"));
        runtime.chains[1].error = None;
        assert!(!render_progress(&runtime, 190_001).contains("terminal_admissions"));
        let snapshot = runtime
            .outcomes
            .as_mut()
            .unwrap()
            .canonical
            .as_mut()
            .unwrap();
        snapshot.rows.clear();
        let rendered = render_progress(&runtime, 100_000);
        assert!(rendered.contains("outcome=\"committed\"} 0"));
        assert!(!rendered.contains("timestamp_seconds"));
        runtime
            .outcomes
            .as_mut()
            .unwrap()
            .canonical
            .as_mut()
            .unwrap()
            .valid = false;
        assert!(!render_progress(&runtime, 100_000).contains("terminal_admissions"));
    }

    #[test]
    fn projection_health_is_not_a_completion_counter_and_expires() {
        assert!(render(None, 0).is_empty());
        let mut status = Status {
            last_success: Some(100_000),
            error: None,
            canonical: None,
        };
        assert!(render(Some(&status), 190_000).contains("projection_success 1"));
        for now in [190_001, 94_999] {
            let text = render(Some(&status), now);
            assert!(text.contains("projection_success 0"));
            assert!(text.contains("timestamp_seconds 100"));
            assert!(!text.contains("committed"));
        }
        status.error = Some(Error::Database);
        assert!(render(Some(&status), 100_000).contains("reason=\"database\""));
        assert!(render(Some(&status), 100_000).contains("projection_success 0"));
    }
}
