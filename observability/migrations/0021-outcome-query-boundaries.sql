-- Reuse canonical settlement evidence within a statement, preserving its MVCC
-- snapshot, freshness gates and immediate conflict/reorg retraction. No state
-- survives the query. Keep migrations 1-20 immutable for existing installations.
-- Match the domain cast used by the admission view; the earlier numeric index
-- cannot use admission sequence as its third lookup key.
CREATE INDEX lifecycle_terminal_admission_u64 ON eez.lifecycle_events (
 source_id,(decode(payload#>>'{event,member,admission,epoch}','hex')),
 ((payload#>>'{event,member,admission,sequence}')::eez.u64)
) WHERE state='complete' AND payload->'event'->>'event'='terminal';
-- Canonical anchor checks know the block identity before the settlement ID.
CREATE INDEX settlement_blocks_hash_number_settlement
 ON eez.settlement_blocks(hash,number,settlement_id);

-- Keep each digest reference correlated with its exact record key. Flattening
-- this nested existence check can scan every chunk from the producer for each
-- terminal instead of probing the primary key. OFFSET 0 preserves all matches
-- and absence semantics while retaining that parameterized lookup boundary.
CREATE OR REPLACE VIEW eez.lifecycle_native_outcomes AS
 SELECT check_result.*,
   CASE WHEN state<>'coherent' THEN state
     WHEN EXISTS(SELECT FROM jsonb_array_elements(record_refs) ref WHERE NOT EXISTS(
       SELECT FROM eez.lifecycle_native_records record
       WHERE record.source_id=check_result.source_id
         AND record.epoch=decode(ref->>'epoch','hex')
         AND record.sequence=(ref->>'sequence')::eez.u64 AND record.digest=ref->>'digest' OFFSET 0))
       THEN 'changed_evidence'
     WHEN EXISTS(SELECT FROM eez.lifecycle_native_lineage record
       WHERE record.source_id=check_result.source_id AND record.epoch=check_result.epoch
         AND record.sequence<check_result.parent_sequence AND record.generation=check_result.generation::text
         AND NOT EXISTS(SELECT FROM jsonb_array_elements(record_refs) ref
           WHERE record.epoch=decode(ref->>'epoch','hex') AND record.sequence=(ref->>'sequence')::eez.u64
             AND record.digest=ref->>'digest')) THEN 'changed_lineage'
     ELSE 'coherent' END AS native_state,
   'unknown'::text AS evidence_class
 FROM eez.lifecycle_native_checks check_result;
CREATE OR REPLACE VIEW eez.lifecycle_outcome_anchors AS
 WITH settled_facts AS MATERIALIZED (
   SELECT l1_chain_id,l2_chain_id,hash,number,l1_block_number,source_valid
   FROM eez.canonical_settled_l2_blocks
 ), endpoint_facts AS MATERIALIZED (
   SELECT l1_chain_id,l2_chain_id,hash,number,l1_block_number,source_valid,
     transaction_hash,block_hash,transaction_index
   FROM eez.canonical_settlement_endpoints
 ), normalized AS MATERIALIZED (
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
       OR EXISTS(SELECT FROM settled_facts settled
         WHERE settled.l1_chain_id=n.context_l1 AND settled.l2_chain_id=n.context_l2
           AND settled.hash=n.l2_snapshot_hash AND settled.number=n.l2_snapshot_number
           AND settled.l1_block_number<=n.l1_snapshot_number AND settled.source_valid)
       OR EXISTS(SELECT FROM endpoint_facts endpoint
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
     WHEN n.applied_endpoint_hash IS NOT NULL AND NOT EXISTS(SELECT FROM endpoint_facts endpoint
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
       EXISTS(SELECT FROM settled_facts settled
         WHERE settled.l1_chain_id=n.context_l1 AND settled.l2_chain_id=n.context_l2
           AND settled.hash=n.source_block_hash AND settled.number=n.source_block_number
           AND settled.l1_block_number<=n.l1_snapshot_number AND settled.source_valid)
       OR EXISTS(SELECT FROM endpoint_facts endpoint
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
