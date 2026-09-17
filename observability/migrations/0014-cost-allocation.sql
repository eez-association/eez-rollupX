-- Membership is a digest-bound native claim; paid quantities always come from
-- the independent receipt ledger. Failed/requeued attempts have no terminal row.
CREATE TABLE eez.lifecycle_cost_checks (
 source_id BIGINT NOT NULL,
 epoch eez.epoch16 NOT NULL,
 sequence eez.u64 NOT NULL,
 context_l1 BIGINT NOT NULL REFERENCES eez.chain_instances(id),
 context_l2 BIGINT NOT NULL REFERENCES eez.chain_instances(id),
 last_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 state TEXT NOT NULL CHECK(state IN ('coherent','missing_record','missing_parent','identity',
   'missing_membership','lineage','references','limit')),
 generation eez.u64,
 record_refs JSONB NOT NULL CHECK(jsonb_typeof(record_refs)='array' AND jsonb_array_length(record_refs)<=129),
 decision JSONB,
 PRIMARY KEY(source_id,epoch,sequence,context_l1,context_l2),
 FOREIGN KEY(source_id,epoch,sequence) REFERENCES eez.lifecycle_events(source_id,epoch,sequence),
 CHECK((state='coherent')=(decision IS NOT NULL)),
 CHECK(state<>'coherent' OR (generation IS NOT NULL AND jsonb_array_length(record_refs)>=2))
);

CREATE VIEW eez.lifecycle_cost_evidence AS
 WITH normalized AS (
 SELECT c.*,
   decode(decision#>>'{owner,post_batch_hash}','hex') AS post_hash,
   decode(decision#>>'{snapshot,hash}','hex') AS snapshot_hash,
   (decision#>>'{snapshot,number}')::eez.u64 AS snapshot_number,
   decode(decision#>>'{post,block,hash}','hex') AS post_block_hash,
   (decision#>>'{post,block,number}')::eez.u64 AS post_block_number,
   (decision#>>'{post,transaction_index}')::eez.u64 AS post_index,
   (decision#>>'{post,success}')::boolean AS post_success,
   CASE WHEN c.state<>'coherent' THEN c.state
     WHEN EXISTS(SELECT FROM jsonb_array_elements(c.record_refs) ref WHERE NOT EXISTS(
       SELECT FROM eez.lifecycle_native_records record
       WHERE record.source_id=c.source_id AND record.epoch=decode(ref->>'epoch','hex')
         AND record.sequence=(ref->>'sequence')::eez.u64 AND record.digest=ref->>'digest'))
       THEN 'changed_evidence'
     WHEN EXISTS(SELECT FROM eez.lifecycle_native_lineage record
       WHERE record.source_id=c.source_id AND record.epoch=c.epoch AND record.sequence<c.sequence
         AND record.generation=c.generation::text
         AND NOT EXISTS(SELECT FROM jsonb_array_elements(c.record_refs) ref
           WHERE record.epoch=decode(ref->>'epoch','hex') AND record.sequence=(ref->>'sequence')::eez.u64
             AND record.digest=ref->>'digest')) THEN 'changed_lineage'
     ELSE 'coherent' END AS native_state
 FROM eez.lifecycle_cost_checks c
 )
 SELECT n.*,l1.deployment_id,l2.rollup_id,
   CASE
     WHEN n.native_state<>'coherent' THEN n.native_state
     WHEN l1.frozen_reason IS NOT NULL OR l2.frozen_reason IS NOT NULL THEN 'frozen_chain'
     WHEN l1.collection_error IS NOT NULL OR l2.collection_error IS NOT NULL
       OR l1.last_success_at IS NULL OR l2.last_success_at IS NULL
       OR l1.last_success_at NOT BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds'
       OR l2.last_success_at NOT BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds' THEN 'stale_chain'
     WHEN snapshot.canonical IS FALSE THEN 'orphaned_snapshot'
     WHEN snapshot.canonical IS DISTINCT FROM true OR snapshot.number<>n.snapshot_number THEN 'missing_snapshot'
     WHEN post.transaction_hash IS NULL OR NOT post.source_valid
       OR post.block_number<>n.post_block_number OR post.transaction_index<>n.post_index
       OR post.success<>n.post_success THEN 'missing_post_receipt'
     WHEN post.verification_rollups IS NULL OR NOT(l2.rollup_id=ANY(post.verification_rollups))
       THEN 'missing_rollup_membership'
     WHEN post.success AND NOT EXISTS(SELECT FROM eez.canonical_settlement_endpoints endpoint
       WHERE endpoint.l1_chain_id=n.context_l1 AND endpoint.l2_chain_id=n.context_l2
         AND endpoint.transaction_hash=n.post_hash AND endpoint.block_hash=n.post_block_hash
         AND endpoint.transaction_index=n.post_index AND endpoint.source_valid)
       THEN 'missing_settlement_evidence'
     ELSE 'verified' END AS evidence_state
 FROM normalized n
 JOIN eez.chain_instances l1 ON l1.id=n.context_l1
 JOIN eez.chain_instances l2 ON l2.id=n.context_l2
 LEFT JOIN eez.blocks snapshot ON snapshot.chain_instance_id=n.context_l1 AND snapshot.hash=n.snapshot_hash
 LEFT JOIN eez.posting_rollup_evidence post ON post.chain_instance_id=n.context_l1
   AND post.transaction_hash=n.post_hash AND post.block_hash=n.post_block_hash;

-- A new complete reconciliation must be checked before a previously cached
-- population can remain authoritative. Exact duplicate claims cannot multiply a
-- receipt; positively orphaned snapshots alone leave the active claim set.
CREATE VIEW eez.posting_allocation_membership AS
 WITH contexts AS (
   SELECT DISTINCT source_id,context_l1,context_l2 FROM eez.lifecycle_cost_checks
 ), claims AS (
 SELECT c.context_l1,c.context_l2,decode(e.payload#>>'{event,attempt,post_batch_hash}','hex') AS post_hash,
   evidence.evidence_state,evidence.decision->'population' AS population
 FROM contexts c JOIN eez.lifecycle_events e ON e.source_id=c.source_id
 JOIN eez.chain_instances l1 ON l1.id=c.context_l1
 JOIN eez.chain_instances l2 ON l2.id=c.context_l2
 LEFT JOIN eez.lifecycle_cost_evidence evidence ON evidence.source_id=e.source_id
   AND evidence.epoch=e.epoch AND evidence.sequence=e.sequence
   AND evidence.context_l1=c.context_l1 AND evidence.context_l2=c.context_l2
 WHERE e.state='complete' AND e.payload->'event'->>'event'='reconciled'
   AND e.payload#>'{event,details,post}' IS NOT NULL AND e.payload#>'{event,details,post}'<>'null'::jsonb
   AND e.payload#>>'{event,details,l1,chain_id}'=l1.chain_id::text
   AND e.payload#>>'{event,details,l1,genesis_hash}'=encode(l1.genesis_hash,'hex')
   AND e.payload#>>'{event,details,l2,chain_id}'=l2.chain_id::text
   AND e.payload#>>'{event,details,l2,genesis_hash}'=encode(l2.genesis_hash,'hex')
   AND e.payload#>>'{event,details,l2,rollup_id}'=l2.rollup_id::text
 ), grouped AS (
 SELECT context_l1,context_l2,post_hash,
   count(*) FILTER(WHERE evidence_state IS DISTINCT FROM 'orphaned_snapshot') AS active_claims,
   count(*) FILTER(WHERE evidence_state='verified') AS verified_claims,
   count(DISTINCT population) FILTER(WHERE evidence_state='verified') AS populations,
   min(population::text) FILTER(WHERE evidence_state='verified')::jsonb AS population
 FROM claims GROUP BY context_l1,context_l2,post_hash
 )
 SELECT *,CASE WHEN verified_claims>0 AND active_claims=verified_claims AND populations=1
   THEN 'native_membership_receipt_verified' ELSE 'unknown' END AS evidence_class
 FROM grouped;

-- One row per paid post and context. Multi-rollup overhead remains unallocated
-- until the entire posting population is known; a local rollup cannot claim the
-- full shared bill. Empty (pure-L2) populations also remain explicitly unallocated.
CREATE VIEW eez.posting_allocation_status AS
 SELECT p.*,l2.id AS l2_chain_id,m.population,
   CASE WHEN NOT p.source_valid THEN 'stale_receipt'
     WHEN p.accounting_gap IS NOT NULL THEN 'accounting_gap'
     WHEN m.evidence_class IS DISTINCT FROM 'native_membership_receipt_verified' THEN 'missing_membership'
     WHEN p.verification_rollups<>ARRAY[l2.rollup_id]::eez.u64[] THEN 'shared_rollup_population_unknown'
     WHEN jsonb_array_length(m.population)=0 THEN 'no_source_members'
     ELSE 'allocated' END AS allocation_state
 FROM eez.posting_rollup_evidence p
 JOIN eez.chain_instances l2 ON l2.deployment_id=p.deployment_id AND l2.role='l2'
   AND (p.verification_rollups IS NULL OR cardinality(p.verification_rollups)=0
     OR l2.rollup_id=ANY(p.verification_rollups))
 LEFT JOIN eez.posting_allocation_membership m ON m.context_l1=p.chain_instance_id
   AND m.context_l2=l2.id AND m.post_hash=p.transaction_hash;

-- Versioned equal shares of each integer fee component. The deterministic
-- lexicographic source order receives quotient remainders; each component sums
-- exactly to its one receipt. Values are allocation estimates, not user charges.
CREATE VIEW eez.source_posting_allocations AS
 SELECT p.chain_instance_id AS l1_chain_id,p.l2_chain_id,p.deployment_id,p.transaction_hash AS post_hash,
   p.block_hash AS post_block_hash,p.block_number,p.timestamp_seconds,p.finalized,p.success,
   p.source_valid,entry.source,entry.ordinality AS allocation_position,
   jsonb_array_length(p.population) AS member_count,'equal_share_v1'::text AS allocation_rule,
   'native_membership_receipt_verified'::text AS evidence_class,
   div(p.execution_paid,NULLIF(jsonb_array_length(p.population),0))
     +CASE WHEN entry.ordinality<=mod(p.execution_paid,NULLIF(jsonb_array_length(p.population),0)) THEN 1 ELSE 0 END AS allocated_execution_paid,
   div(p.blob_paid,NULLIF(jsonb_array_length(p.population),0))
     +CASE WHEN entry.ordinality<=mod(p.blob_paid,NULLIF(jsonb_array_length(p.population),0)) THEN 1 ELSE 0 END AS allocated_blob_paid
 FROM eez.posting_allocation_status p
 CROSS JOIN LATERAL jsonb_array_elements(p.population) WITH ORDINALITY entry(source,ordinality)
 WHERE p.allocation_state='allocated';

-- Explicit inbound membership plus independently matched inclusion references
-- and the same rollup-specific receipt window as native cost accounting. Missing
-- post evidence or two candidate owners never becomes height-only attribution.
CREATE VIEW eez.companion_attribution_candidates AS
 SELECT DISTINCT n.context_l1,n.context_l2,n.post_hash,n.post_block_hash,
   source.transaction_hash AS source_hash,source.block_hash AS source_block_hash
 FROM eez.lifecycle_cost_evidence n
 JOIN eez.posting_allocation_membership membership ON membership.context_l1=n.context_l1
   AND membership.context_l2=n.context_l2 AND membership.post_hash=n.post_hash
   AND membership.evidence_class='native_membership_receipt_verified'
 JOIN eez.posting_rollup_evidence post ON post.chain_instance_id=n.context_l1
   AND post.transaction_hash=n.post_hash AND post.block_hash=n.post_block_hash
 CROSS JOIN LATERAL jsonb_array_elements(n.decision->'sources') observed
 JOIN eez.canonical_receipt_facts source ON source.chain_instance_id=n.context_l1
   AND source.transaction_hash=decode(observed->>'transaction_hash','hex')
   AND source.block_hash=decode(observed#>>'{receipt,block,hash}','hex')
   AND source.block_number=(observed#>>'{receipt,block,number}')::eez.u64
   AND source.transaction_index=(observed#>>'{receipt,transaction_index}')::eez.u64
   AND source.success=(observed#>>'{receipt,success}')::boolean
 WHERE n.evidence_state='verified' AND observed->>'direction'='inbound' AND source.source_valid
   AND source.block_hash=post.block_hash AND source.transaction_index>post.transaction_index
   AND NOT EXISTS(SELECT FROM eez.canonical_receipt_facts candidate
     LEFT JOIN eez.posting_analysis analysis ON analysis.chain_instance_id=candidate.chain_instance_id
       AND analysis.transaction_hash=candidate.transaction_hash
     WHERE candidate.chain_instance_id=n.context_l1 AND candidate.block_hash=post.block_hash
       AND candidate.recipient=post.recipient AND candidate.success
       AND candidate.transaction_index>post.transaction_index AND candidate.transaction_index<source.transaction_index
       AND (analysis.state IS NULL OR analysis.state IN ('pending','invalid','unverified')
         OR (analysis.state='decoded' AND (analysis.verification_rollups IS NULL
           OR n.rollup_id=ANY(analysis.verification_rollups)))));

-- Receipt population is deduplicated before attribution. Several observations
-- of one source, or several admissions, never add another charge.
CREATE VIEW eez.companion_receipts AS
 SELECT receipt.*,candidate.context_l2,candidate.post_hash,candidate.post_block_hash,
   'native_membership_receipt_verified'::text AS evidence_class
 FROM eez.canonical_receipt_facts receipt
 JOIN eez.companion_attribution_candidates candidate ON candidate.context_l1=receipt.chain_instance_id
   AND candidate.source_hash=receipt.transaction_hash AND candidate.source_block_hash=receipt.block_hash
 WHERE NOT EXISTS(SELECT FROM eez.companion_attribution_candidates other
   WHERE other.context_l1=candidate.context_l1 AND other.source_hash=candidate.source_hash
     AND other.source_block_hash=candidate.source_block_hash
     AND (other.context_l2,other.post_hash,other.post_block_hash)
       IS DISTINCT FROM (candidate.context_l2,candidate.post_hash,candidate.post_block_hash));

GRANT SELECT,INSERT,UPDATE ON eez.lifecycle_cost_checks TO eez_observer_writer;
GRANT SELECT ON eez.lifecycle_cost_evidence,eez.posting_allocation_membership,
 eez.posting_allocation_status,eez.source_posting_allocations,
 eez.companion_attribution_candidates,eez.companion_receipts
 TO eez_observer_writer,eez_observer_reader;
