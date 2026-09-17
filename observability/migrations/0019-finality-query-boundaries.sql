-- Bound planner expansion of canonical finality dependencies. Reusing the same
-- immutable SQL snapshot prevents the lateral dependency lookup from recursively
-- expanding the full canonical anchor selection per admission/claim. Existing
-- canonical selection and missing-data semantics stay authoritative; no clock,
-- finality, query-timeout or database-memory limit changes.
--
-- Keep schema17 unchanged for existing installations. These replacement views
-- preserve their column order/types and existing reader grants.
-- Canonical CASE expressions otherwise expand into every aggregate filter and
-- evidence-class projection. Evaluate their existing validation once per SQL
-- statement; these are ephemeral CTEs, not stale persisted materialized views.
CREATE OR REPLACE VIEW eez.lifecycle_outcome_anchors AS
 WITH normalized AS MATERIALIZED (
 SELECT n.source_id,n.epoch,n.sequence,n.context_l1,n.context_l2,n.native_state,n.decision,
   decode(n.decision#>>'{member,admission,epoch}','hex') AS admission_epoch,
   (n.decision#>>'{member,admission,sequence}')::eez.u64 AS admission_sequence,
   n.decision->'member' AS member,n.decision->'owner' AS owner,
   n.decision->>'outcome' AS outcome,n.decision#>>'{member,source,direction}' AS direction,
   (n.decision->>'elapsed_ns')::eez.u64 AS elapsed_ns,
   (n.decision->>'observed_at_ms')::eez.u64 AS observed_at_ms,
   decode(n.decision#>>'{member,source,transaction_hash}','hex') AS source_hash,
   decode(n.decision#>>'{owner,post_batch_hash}','hex') AS post_hash,
   decode(n.decision#>>'{l1_snapshot,hash}','hex') AS l1_snapshot_hash,
   (n.decision#>>'{l1_snapshot,number}')::eez.u64 AS l1_snapshot_number,
   decode(n.decision#>>'{l2_snapshot,hash}','hex') AS l2_snapshot_hash,
   (n.decision#>>'{l2_snapshot,number}')::eez.u64 AS l2_snapshot_number,
   decode(n.decision#>>'{post,block,hash}','hex') AS post_block_hash,
   (n.decision#>>'{post,block,number}')::eez.u64 AS post_block_number,
   (n.decision#>>'{post,transaction_index}')::eez.u64 AS post_index,
   (n.decision#>>'{post,success}')::boolean AS post_success,
   decode(n.decision#>>'{applied,block,hash}','hex') AS applied_block_hash,
   (n.decision#>>'{applied,block,number}')::eez.u64 AS applied_block_number,
   (n.decision#>>'{applied,transaction_index}')::eez.u64 AS applied_index,
   decode(n.decision#>>'{applied,calldata_hash}','hex') AS applied_calldata_hash,
   decode(n.decision#>>'{applied,endpoint,hash}','hex') AS applied_endpoint_hash,
   (n.decision#>>'{applied,endpoint,number}')::eez.u64 AS applied_endpoint_number,
   decode(n.decision#>>'{source,source_block,hash}','hex') AS source_block_hash,
   (n.decision#>>'{source,source_block,number}')::eez.u64 AS source_block_number,
   (n.decision#>>'{source,receipt,transaction_index}')::eez.u64 AS source_index,
   (n.decision#>>'{source,receipt,success}')::boolean AS source_success,
   n.decision#>>'{source,basis}' AS source_basis
 FROM eez.lifecycle_native_outcomes n
 ), checked AS MATERIALIZED (
 SELECT n.*,l1.deployment_id,l2.rollup_id,
   snapshot.timestamp_seconds AS progress_timestamp_seconds,
   source_receipt.success AS observed_source_receipt_success,
   source_receipt.block_hash AS observed_source_receipt_block_hash,
   CASE
     WHEN n.native_state<>'coherent' THEN n.native_state
     WHEN l1.frozen_reason IS NOT NULL OR l2.frozen_reason IS NOT NULL THEN 'frozen_chain'
     WHEN correlation.frozen_reason IS NOT NULL THEN 'frozen_settlement'
     WHEN correlation.l1_chain_id IS NOT NULL AND (correlation.collection_error IS NOT NULL
       OR correlation.last_success_at IS NULL
       OR correlation.last_success_at NOT BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') THEN 'stale_settlement'
     WHEN l1.collection_error IS NOT NULL OR l2.collection_error IS NOT NULL
       OR l1.last_success_at IS NULL OR l2.last_success_at IS NULL
       OR l1.last_success_at NOT BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds'
       OR l2.last_success_at NOT BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds' THEN 'stale_chain'
     WHEN snapshot.canonical IS FALSE THEN 'orphaned_l1_snapshot'
     WHEN snapshot.canonical IS DISTINCT FROM true OR snapshot.number<>n.l1_snapshot_number THEN 'missing_l1_snapshot'
     WHEN NOT (
       (n.l2_snapshot_number=0 AND n.l2_snapshot_hash=l2.genesis_hash)
       OR EXISTS(SELECT FROM eez.canonical_settled_l2_blocks settled
         WHERE settled.l1_chain_id=n.context_l1 AND settled.l2_chain_id=n.context_l2
           AND settled.hash=n.l2_snapshot_hash AND settled.number=n.l2_snapshot_number
           AND settled.l1_block_number<=n.l1_snapshot_number AND settled.source_valid)
       OR EXISTS(SELECT FROM eez.canonical_settlement_endpoints endpoint
         WHERE endpoint.l1_chain_id=n.context_l1 AND endpoint.l2_chain_id=n.context_l2
           AND endpoint.hash=n.l2_snapshot_hash AND endpoint.number=n.l2_snapshot_number
           AND endpoint.l1_block_number<=n.l1_snapshot_number AND endpoint.source_valid)
       ) THEN 'missing_l2_snapshot'
     WHEN n.post_block_hash IS NOT NULL AND NOT EXISTS(SELECT FROM eez.canonical_receipt_facts post
       WHERE post.chain_instance_id=n.context_l1 AND post.transaction_hash=n.post_hash
         AND post.block_hash=n.post_block_hash AND post.block_number=n.post_block_number
         AND post.transaction_index=n.post_index AND post.success=n.post_success AND post.source_valid)
       THEN 'missing_post_receipt'
     WHEN n.applied_block_hash IS NOT NULL AND (
       applied.transaction_hash IS NULL OR NOT applied.success OR NOT applied.source_valid
       OR applied.transaction_index<>n.applied_index OR applied.block_number<>n.applied_block_number
       OR applied.calldata_hash<>n.applied_calldata_hash
       OR applied.verification_rollups IS NULL OR NOT(l2.rollup_id=ANY(applied.verification_rollups)))
       THEN 'missing_applied_post'
     WHEN n.applied_endpoint_hash IS NOT NULL AND NOT EXISTS(SELECT FROM eez.canonical_settlement_endpoints endpoint
       WHERE endpoint.l1_chain_id=n.context_l1 AND endpoint.l2_chain_id=n.context_l2
         AND endpoint.transaction_hash=n.post_hash AND endpoint.block_hash=n.applied_block_hash
         AND endpoint.transaction_index=n.applied_index AND endpoint.number=n.applied_endpoint_number
         AND endpoint.hash=n.applied_endpoint_hash AND endpoint.source_valid)
       THEN 'missing_applied_endpoint'
     WHEN n.source_basis='receipt' AND (
       source_receipt.transaction_hash IS NULL OR NOT source_receipt.source_valid
       OR source_receipt.block_hash<>n.source_block_hash OR source_receipt.block_number<>n.source_block_number
       OR source_receipt.transaction_index<>n.source_index OR source_receipt.success<>n.source_success)
       THEN 'missing_source_receipt'
     WHEN n.direction='outbound' AND n.source_block_hash IS NOT NULL AND NOT (
       EXISTS(SELECT FROM eez.canonical_settled_l2_blocks settled
         WHERE settled.l1_chain_id=n.context_l1 AND settled.l2_chain_id=n.context_l2
           AND settled.hash=n.source_block_hash AND settled.number=n.source_block_number
           AND settled.l1_block_number<=n.l1_snapshot_number AND settled.source_valid)
       OR EXISTS(SELECT FROM eez.canonical_settlement_endpoints endpoint
         WHERE endpoint.l1_chain_id=n.context_l1 AND endpoint.l2_chain_id=n.context_l2
           AND endpoint.hash=n.source_block_hash AND endpoint.number=n.source_block_number
           AND endpoint.l1_block_number<=n.l1_snapshot_number AND endpoint.source_valid))
       THEN 'missing_source_settlement'
     -- Receipt facts remain independent. A known revert cannot be displayed as
     -- successful service merely because the native group was called committed.
     WHEN n.outcome='committed' AND source_receipt.success IS FALSE THEN 'contradictory_source_receipt'
     WHEN n.outcome='committed' AND n.direction='inbound' AND (
       NOT EXISTS(SELECT FROM eez.canonical_protocol_log_sets logs
         WHERE logs.chain_instance_id=n.context_l1 AND logs.transaction_hash=n.source_hash
           AND logs.block_hash=n.source_block_hash AND logs.source_valid)
       OR EXISTS(SELECT FROM jsonb_array_elements(n.decision#>'{plan,source_actions}') action
         WHERE NOT EXISTS(SELECT FROM eez.canonical_protocol_logs log
           WHERE log.chain_instance_id=n.context_l1 AND log.transaction_hash=n.source_hash
             AND log.block_hash=n.source_block_hash AND log.address=applied.recipient
             AND log.rollup_id=l2.rollup_id AND log.kind='consumed'
             AND log.call_hash=decode(action->>'call_hash','hex')
             AND log.queue_index=(action->>'source_index')::eez.u64 AND log.source_valid)))
       THEN 'missing_source_consumption'
     WHEN n.outcome='committed' AND n.direction='inbound' AND (
       EXISTS(SELECT FROM eez.canonical_receipt_facts candidate
         LEFT JOIN eez.posting_analysis analysis ON analysis.chain_instance_id=candidate.chain_instance_id
           AND analysis.transaction_hash=candidate.transaction_hash
         WHERE candidate.chain_instance_id=n.context_l1 AND candidate.block_hash=n.source_block_hash
           AND candidate.recipient=applied.recipient AND candidate.success
           AND candidate.transaction_index>n.applied_index AND candidate.transaction_index<n.source_index
           AND (analysis.state IS NULL OR analysis.state IN ('pending','invalid','unverified')
             OR (analysis.state='decoded' AND (analysis.verification_rollups IS NULL
               OR l2.rollup_id=ANY(analysis.verification_rollups))))))
       THEN 'ambiguous_source_window'
     ELSE 'verified'
   END AS canonical_state
 FROM normalized n
 JOIN eez.chain_instances l1 ON l1.id=n.context_l1
 JOIN eez.chain_instances l2 ON l2.id=n.context_l2
 LEFT JOIN eez.correlation_sources correlation ON correlation.l1_chain_id=n.context_l1 AND correlation.l2_chain_id=n.context_l2
 LEFT JOIN eez.blocks snapshot ON snapshot.chain_instance_id=n.context_l1 AND snapshot.hash=n.l1_snapshot_hash
 LEFT JOIN eez.canonical_receipt_facts source_receipt
   ON source_receipt.chain_instance_id=CASE WHEN n.direction='inbound' THEN n.context_l1 ELSE n.context_l2 END
     AND source_receipt.transaction_hash=n.source_hash
 LEFT JOIN eez.posting_rollup_evidence applied ON applied.chain_instance_id=n.context_l1
   AND applied.transaction_hash=n.post_hash AND applied.block_hash=n.applied_block_hash
 )
 SELECT checked.*,CASE WHEN canonical_state='verified' THEN 'node_reconciled' ELSE 'unknown' END AS evidence_class
 FROM checked;

CREATE OR REPLACE VIEW eez.lifecycle_canonical_outcomes AS
 WITH anchors AS MATERIALIZED (SELECT * FROM eez.lifecycle_outcome_anchors),
 contexts AS (
   SELECT DISTINCT source_id,context_l1,context_l2 FROM eez.lifecycle_native_checks
 ), grouped AS (
 SELECT contexts.source_id,contexts.context_l1,contexts.context_l2,
   t.admission_epoch,t.admission_sequence,
   min(t.source::text)::jsonb AS source,
   count(*) AS terminal_claims,
   count(*) FILTER(WHERE t.claimed_assurance='node_reconciled') AS native_claims,
   count(*) FILTER(WHERE a.canonical_state IS DISTINCT FROM 'orphaned_l1_snapshot') AS active_claims,
   count(*) FILTER(WHERE a.canonical_state='verified') AS verified_claims,
   count(DISTINCT t.member) FILTER(WHERE a.canonical_state IS DISTINCT FROM 'orphaned_l1_snapshot') AS members,
   count(DISTINCT (t.claimed_outcome,t.attempt,t.elapsed_ns)) FILTER(WHERE a.canonical_state='verified') AS decisions,
   min(t.claimed_outcome) FILTER(WHERE a.canonical_state='verified') AS verified_outcome,
   min(t.elapsed_ns) FILTER(WHERE a.canonical_state='verified') AS elapsed_ns,
   max(a.progress_timestamp_seconds) FILTER(WHERE a.canonical_state='verified') AS progress_timestamp_seconds
 FROM contexts JOIN eez.lifecycle_terminal_observations t ON t.source_id=contexts.source_id
 LEFT JOIN anchors a ON a.source_id=t.source_id AND a.epoch=t.epoch AND a.sequence=t.sequence
   AND a.context_l1=contexts.context_l1 AND a.context_l2=contexts.context_l2
 GROUP BY contexts.source_id,contexts.context_l1,contexts.context_l2,t.admission_epoch,t.admission_sequence
 )
 SELECT grouped.*,
   CASE WHEN verified_claims>0 AND active_claims=verified_claims AND members=1 AND decisions=1
     THEN verified_outcome END AS outcome,
   CASE WHEN verified_claims>0 AND active_claims=verified_claims AND members=1 AND decisions=1
     THEN 'node_reconciled' ELSE 'unknown' END AS evidence_class
 FROM grouped;

-- Expose the already-selected unique canonical context as additive columns.
-- Finality can reuse admission selection rather than independently re-evaluating
-- every canonical outcome and risking a different population join.
CREATE OR REPLACE VIEW eez.lifecycle_admission_status AS
 WITH canonical_outcomes AS MATERIALIZED (SELECT * FROM eez.lifecycle_canonical_outcomes)
 SELECT admission.*,terminal.claims AS terminal_claims,
   terminal.policy_claims,terminal.outcomes AS distinct_claimed_outcomes,terminal.decisions AS distinct_claimed_decisions,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN 'local_policy'
        WHEN canonical.verified_contexts=1 THEN 'canonical'
        WHEN admission.liveness='ended' THEN 'unresolved_restart' ELSE 'unresolved' END AS status,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN terminal.outcome WHEN canonical.verified_contexts=1 THEN canonical.outcome END AS outcome,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN 'local_policy' WHEN canonical.verified_contexts=1 THEN 'node_reconciled' ELSE 'unknown' END AS evidence_class,
   CASE WHEN canonical.verified_contexts=1 THEN canonical.context_l1 END AS canonical_context_l1,
   CASE WHEN canonical.verified_contexts=1 THEN canonical.context_l2 END AS canonical_context_l2
 FROM eez.lifecycle_admission_observations admission
 CROSS JOIN LATERAL (
   SELECT count(*) AS claims,count(*) FILTER(WHERE evidence_state='local_policy') AS policy_claims,
     count(DISTINCT claimed_outcome) AS outcomes,
     count(DISTINCT (claimed_outcome,attempt,reconciliation,elapsed_ns)) AS decisions,
     min(claimed_outcome) AS outcome
   FROM eez.lifecycle_local_policy_evidence t
   WHERE t.source_id=admission.source_id AND t.admission_epoch=admission.admission_epoch
     AND t.admission_sequence=admission.admission_sequence
 ) terminal
 CROSS JOIN LATERAL (
   SELECT count(*) AS verified_contexts,min(outcome) AS outcome,
     min(context_l1) AS context_l1,min(context_l2) AS context_l2
   FROM canonical_outcomes candidate
   WHERE candidate.source_id=admission.source_id AND candidate.admission_epoch=admission.admission_epoch
     AND candidate.admission_sequence=admission.admission_sequence AND candidate.evidence_class='node_reconciled'
     AND candidate.source=admission.source
 ) canonical;
CREATE OR REPLACE VIEW eez.lifecycle_finality_dependencies AS
 WITH anchors AS MATERIALIZED (SELECT * FROM eez.lifecycle_outcome_anchors)
 SELECT a.source_id,a.epoch,a.sequence,a.context_l1,a.context_l2,
   dependency.number,dependency.hash,
   (dependency.number=0 AND dependency.hash=l2.genesis_hash) AS genesis,
   proof.number IS NOT NULL AS finalized,
   proof.observed_at
 FROM anchors a
 JOIN eez.chain_instances l2 ON l2.id=a.context_l2
 CROSS JOIN LATERAL (
   SELECT a.l2_snapshot_number AS number,a.l2_snapshot_hash AS hash
   UNION SELECT a.applied_endpoint_number,a.applied_endpoint_hash WHERE a.applied_endpoint_hash IS NOT NULL
   UNION SELECT a.source_block_number,a.source_block_hash WHERE a.direction='outbound' AND a.source_block_hash IS NOT NULL
 ) dependency
 LEFT JOIN LATERAL (
   SELECT f.number,f.observed_at
   FROM eez.settlement_finality f
   JOIN eez.verified_l2_finality current ON current.l2_chain_id=f.l2_chain_id
     AND current.number=f.number AND current.hash=f.hash
   JOIN eez.settlements s ON s.id=f.settlement_id AND s.l1_chain_id=a.context_l1
   WHERE f.l2_chain_id=a.context_l2 AND f.number=dependency.number AND f.hash=dependency.hash
 ) proof ON true;

CREATE OR REPLACE VIEW eez.lifecycle_finality_claims AS
 WITH anchors AS MATERIALIZED (SELECT * FROM eez.lifecycle_outcome_anchors),
 finality AS MATERIALIZED (
   SELECT f.l2_chain_id,f.number,f.hash,f.observed_at,s.l1_chain_id
   FROM eez.settlement_finality f
   JOIN eez.verified_l2_finality current ON current.l2_chain_id=f.l2_chain_id
     AND current.number=f.number AND current.hash=f.hash
   JOIN eez.settlements s ON s.id=f.settlement_id
 )
 SELECT a.source_id,a.epoch,a.sequence,a.context_l1,a.context_l2,
   a.admission_epoch,a.admission_sequence,a.canonical_state,
   a.observed_at_ms AS terminal_observed_at_ms,a.elapsed_ns,
   CASE WHEN a.canonical_state<>'verified' THEN 'canonical_unknown'
     WHEN NOT snapshot.finalized THEN 'awaiting_l1_finality'
     WHEN NOT dependencies.complete THEN 'awaiting_exact_l2_finality'
     WHEN snapshot.finalized_observed_at IS NULL THEN 'historical_finality_time_unknown'
     ELSE 'finalized' END AS finality_state,
   CASE WHEN snapshot.finalized AND dependencies.complete AND snapshot.finalized_observed_at IS NOT NULL
     THEN GREATEST(snapshot.finalized_observed_at,dependencies.observed_at) END AS finality_observed_at
 FROM anchors a
 JOIN eez.chain_instances l2 ON l2.id=a.context_l2
 JOIN eez.blocks snapshot ON snapshot.chain_instance_id=a.context_l1 AND snapshot.hash=a.l1_snapshot_hash
   AND snapshot.number=a.l1_snapshot_number
 CROSS JOIN LATERAL (
   SELECT bool_and((dependency.number=0 AND dependency.hash=l2.genesis_hash)
      OR proof.number IS NOT NULL) AS complete,
     max(proof.observed_at) FILTER(WHERE NOT(dependency.number=0 AND dependency.hash=l2.genesis_hash)) AS observed_at
   FROM (
     SELECT a.l2_snapshot_number AS number,a.l2_snapshot_hash AS hash
     UNION SELECT a.applied_endpoint_number,a.applied_endpoint_hash WHERE a.applied_endpoint_hash IS NOT NULL
     UNION SELECT a.source_block_number,a.source_block_hash WHERE a.direction='outbound' AND a.source_block_hash IS NOT NULL
   ) dependency
   LEFT JOIN finality proof ON proof.l2_chain_id=a.context_l2 AND proof.l1_chain_id=a.context_l1
     AND proof.number=dependency.number AND proof.hash=dependency.hash
 ) dependencies;

CREATE OR REPLACE VIEW eez.lifecycle_admission_finality AS
 WITH admissions AS MATERIALIZED (SELECT * FROM eez.lifecycle_admission_status),
 finality_claims AS MATERIALIZED (SELECT * FROM eez.lifecycle_finality_claims),
 timings AS (
 SELECT admission.source_id,admission.deployment_id,admission.service,admission.instance,
   admission.admission_epoch,admission.admission_sequence,admission.source,
   admission.observed_at_ms AS admitted_at_ms,admission.first_seen_at,admission.source_valid,
   admission.status AS admission_status,admission.outcome,admission.evidence_class,
   admission.canonical_context_l1 AS context_l1,admission.canonical_context_l2 AS context_l2,
   claims.finality_state,claims.finality_observed_at,claims.clock_consistent
 FROM admissions admission
 CROSS JOIN LATERAL (
   SELECT CASE WHEN count(*)=0 THEN 'canonical_unknown'
     WHEN bool_and(claim.finality_state='finalized') THEN 'finalized'
     ELSE min(claim.finality_state) FILTER(WHERE claim.finality_state<>'finalized') END AS finality_state,
     max(claim.finality_observed_at) AS finality_observed_at,
     bool_and(claim.terminal_observed_at_ms>=admission.observed_at_ms
       AND abs((claim.terminal_observed_at_ms-admission.observed_at_ms)/1000.0-claim.elapsed_ns/1e9)<=5)
       AS clock_consistent
   FROM finality_claims claim WHERE claim.source_id=admission.source_id
     AND claim.admission_epoch=admission.admission_epoch AND claim.admission_sequence=admission.admission_sequence
     AND admission.status='canonical' AND claim.context_l1=admission.canonical_context_l1 AND claim.context_l2=admission.canonical_context_l2
     AND claim.canonical_state='verified'
 ) claims
 ), classified AS (
 SELECT timings.*,CASE
   WHEN NOT COALESCE(source_valid,false) THEN 'stale_lifecycle'
   WHEN admission_status='local_policy' THEN 'local_policy_not_applicable'
   WHEN admission_status<>'canonical' THEN 'canonical_unknown'
   WHEN finality_state<>'finalized' THEN finality_state
   WHEN admitted_at_ms>extract(epoch FROM statement_timestamp())*1000+5000
     OR finality_observed_at>statement_timestamp()+interval '5 seconds'
     OR admitted_at_ms>extract(epoch FROM first_seen_at)*1000+5000
     OR NOT COALESCE(clock_consistent,false) THEN 'clock_inconsistent'
   WHEN finality_observed_at<to_timestamp(admitted_at_ms/1000.0) THEN 'finality_observed_before_admission'
   ELSE 'observed_finalized' END AS timing_state
 FROM timings
 )
 SELECT classified.*,
   CASE WHEN timing_state='observed_finalized' THEN
     extract(epoch FROM finality_observed_at)-admitted_at_ms/1000.0 END AS accepted_to_observed_finality_seconds,
   'synchronized_utc_assumed_not_verified'::text AS clock_basis
 FROM classified;

