-- Short evidence queries must fit the existing 10-second deadline. Expanded
-- views trigger >13s of JIT compilation even for a one-claim fixture; interpreted
-- execution takes ~160ms. Scope this to the dedicated roles in this database.
-- Observer connections also set it explicitly for custom writer login members.
DO $$ BEGIN
 EXECUTE format('ALTER ROLE eez_observer_reader IN DATABASE %I SET jit = off',current_database());
 EXECUTE format('ALTER ROLE eez_observer_writer IN DATABASE %I SET jit = off',current_database());
END $$;

-- A historical applied prefix need not remain the current L2 hash at its height.
-- Bind it to the canonical L1 occurrence and exact EEZ root within that batch's
-- rollup-specific window. This remains node-reconciled correlation, not replay.
CREATE VIEW eez.canonical_settlement_endpoints AS
 SELECT s.id AS settlement_id,s.l1_chain_id,s.l2_chain_id,s.transaction_hash,
   s.block_hash,s.transaction_index,s.l1_block_number,s.l1_finalized,
   endpoint.number,endpoint.hash,p.recipient,p.calldata_hash,
   logs.transaction_hash AS root_transaction_hash,logs.transaction_index AS root_transaction_index,
   logs.log_index AS root_log_index,
   (s.source_valid AND logs.source_valid) AS source_valid
 FROM eez.settlement_evidence s
 JOIN eez.settlement_blocks endpoint ON endpoint.settlement_id=s.id AND endpoint.ordinal=s.block_count-1
 JOIN eez.chain_instances l2 ON l2.id=s.l2_chain_id
 JOIN eez.posting_rollup_evidence p ON p.chain_instance_id=s.l1_chain_id
   AND p.transaction_hash=s.transaction_hash AND p.block_hash=s.block_hash
 JOIN eez.canonical_protocol_logs logs ON logs.chain_instance_id=s.l1_chain_id
   AND logs.block_hash=s.block_hash AND logs.address=p.recipient
   AND logs.rollup_id=l2.rollup_id AND logs.kind='root' AND logs.root=endpoint.hash
 WHERE s.claim_count=1 AND p.success AND l2.rollup_id=ANY(p.verification_rollups)
   AND logs.transaction_index>=s.transaction_index
   -- Exact relevant posts and unknown successful calls end a provable window.
   -- Use known receipt facts independently of their billing selection; decoded
   -- posts listing only other rollups do not reset this queue.
   AND NOT EXISTS(SELECT FROM eez.canonical_receipt_facts candidate
     LEFT JOIN eez.posting_analysis analysis ON analysis.chain_instance_id=candidate.chain_instance_id
       AND analysis.transaction_hash=candidate.transaction_hash
     WHERE candidate.chain_instance_id=s.l1_chain_id AND candidate.block_hash=s.block_hash
       AND candidate.recipient=p.recipient AND candidate.success
       AND candidate.transaction_index>s.transaction_index AND candidate.transaction_index<=logs.transaction_index
       AND (analysis.state IS NULL OR analysis.state IN ('pending','invalid','unverified')
         OR (analysis.state='decoded' AND (analysis.verification_rollups IS NULL
           OR l2.rollup_id=ANY(analysis.verification_rollups)))));

-- Decode only the bounded source-specific projection. All JSON came through the
-- typed native checker; gaps retain NULL references and cannot acquire assurance.
CREATE VIEW eez.lifecycle_outcome_anchors AS
 WITH normalized AS (
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
 ), checked AS (
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

GRANT SELECT ON eez.canonical_settlement_endpoints,eez.lifecycle_outcome_anchors
 TO eez_observer_writer,eez_observer_reader;

-- Include every current terminal claim, including one not projected yet. A late
-- contradictory terminal retracts stronger assurance before the next worker pass.
-- Only a positively orphaned scan watermark removes an old claim from selection.
CREATE VIEW eez.lifecycle_canonical_outcomes AS
 WITH contexts AS (
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
 LEFT JOIN eez.lifecycle_outcome_anchors a ON a.source_id=t.source_id AND a.epoch=t.epoch AND a.sequence=t.sequence
   AND a.context_l1=contexts.context_l1 AND a.context_l2=contexts.context_l2
 GROUP BY contexts.source_id,contexts.context_l1,contexts.context_l2,t.admission_epoch,t.admission_sequence
 )
 SELECT grouped.*,
   CASE WHEN verified_claims>0 AND active_claims=verified_claims AND members=1 AND decisions=1
     THEN verified_outcome END AS outcome,
   CASE WHEN verified_claims>0 AND active_claims=verified_claims AND members=1 AND decisions=1
     THEN 'node_reconciled' ELSE 'unknown' END AS evidence_class
 FROM grouped;

-- Preserve existing columns and local-policy semantics. Canonical state joins by
-- actual admission identity, so later membership cannot fabricate an admission.
CREATE OR REPLACE VIEW eez.lifecycle_admission_status AS
 SELECT admission.*,terminal.claims AS terminal_claims,
   terminal.policy_claims,terminal.outcomes AS distinct_claimed_outcomes,terminal.decisions AS distinct_claimed_decisions,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN 'local_policy'
        WHEN canonical.verified_contexts=1 THEN 'canonical'
        WHEN admission.liveness='ended' THEN 'unresolved_restart' ELSE 'unresolved' END AS status,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN terminal.outcome WHEN canonical.verified_contexts=1 THEN canonical.outcome END AS outcome,
   CASE WHEN terminal.claims>0 AND terminal.claims=terminal.policy_claims AND terminal.decisions=1
        THEN 'local_policy' WHEN canonical.verified_contexts=1 THEN 'node_reconciled' ELSE 'unknown' END AS evidence_class
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
   SELECT count(*) AS verified_contexts,min(outcome) AS outcome
   FROM eez.lifecycle_canonical_outcomes candidate
   WHERE candidate.source_id=admission.source_id AND candidate.admission_epoch=admission.admission_epoch
     AND candidate.admission_sequence=admission.admission_sequence AND candidate.evidence_class='node_reconciled'
     AND candidate.source=admission.source
 ) canonical;
GRANT SELECT ON eez.lifecycle_canonical_outcomes TO eez_observer_writer,eez_observer_reader;
