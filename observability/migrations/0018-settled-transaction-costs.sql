-- Transaction denominators come from complete all-transaction receipt scans and
-- exact applied hashes. Neither a proposed block nor a same-height successor is
-- a substitute for a missing applied prefix. Existing receipt costs are unchanged.
CREATE VIEW eez.settled_transaction_costs AS
WITH contexts AS MATERIALIZED (
 SELECT s.l1_chain_id,s.l2_chain_id,c.rollup_id,c.selection,c.bootstrap_number AS l2_bootstrap,
   c1.bootstrap_number AS l1_bootstrap,
   (c.frozen_reason IS NULL AND c.collection_error IS NULL
    AND c.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds'
    AND s.frozen_reason IS NULL AND s.collection_error IS NULL
    AND s.last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds') AS context_valid
 FROM eez.correlation_sources s JOIN eez.chain_instances c ON c.id=s.l2_chain_id
 JOIN eez.chain_instances c1 ON c1.id=s.l1_chain_id
), posts AS MATERIALIZED (
 SELECT p.*,c.l2_chain_id,c.rollup_id AS target_rollup,c.selection AS l2_selection,
   c.l1_bootstrap,c.l2_bootstrap,c.context_valid
 FROM contexts c JOIN eez.posting_rollup_evidence p ON p.chain_instance_id=c.l1_chain_id
 WHERE p.verification_rollups IS NULL OR c.rollup_id=ANY(p.verification_rollups)
), ranges AS MATERIALIZED (
 SELECT s.id,s.l1_chain_id,s.l2_chain_id,s.transaction_hash,s.block_hash,s.block_count,
   (s.source_valid AND s.claim_count=1 AND s.range_verified_at IS NOT NULL
    AND count(b.hash)=s.block_count
    AND bool_and(b.source_valid AND stored.receipt_coverage='complete')) AS complete
 FROM eez.settlement_evidence s
 LEFT JOIN eez.canonical_settled_l2_blocks b ON b.settlement_id=s.id
 LEFT JOIN eez.blocks stored ON stored.chain_instance_id=b.l2_chain_id AND stored.hash=b.hash
 GROUP BY s.id,s.l1_chain_id,s.l2_chain_id,s.transaction_hash,s.block_hash,s.block_count,
   s.source_valid,s.claim_count,s.range_verified_at
), members AS MATERIALIZED (
 SELECT DISTINCT p.chain_instance_id,p.l2_chain_id,p.transaction_hash AS post_hash,
   p.block_hash AS post_block_hash,p.block_number,p.transaction_index,r.transaction_hash
 FROM posts p JOIN ranges s ON s.l1_chain_id=p.chain_instance_id AND s.l2_chain_id=p.l2_chain_id
   AND s.transaction_hash=p.transaction_hash AND s.block_hash=p.block_hash AND s.complete
 JOIN eez.settlement_blocks applied ON applied.settlement_id=s.id
 JOIN eez.receipt_observations r ON r.chain_instance_id=p.l2_chain_id
   AND r.block_hash=applied.hash AND r.canonical
), first_members AS MATERIALIZED (
 SELECT DISTINCT ON(chain_instance_id,l2_chain_id,transaction_hash) * FROM members
 ORDER BY chain_instance_id,l2_chain_id,transaction_hash,block_number,transaction_index
), classified AS (
 SELECT p.*,
   CASE
    WHEN p.source_valid IS NOT TRUE OR p.context_valid IS NOT TRUE THEN 'stale_source'
    WHEN NOT p.success THEN 'reverted_post'
    WHEN p.verification_rollups IS NULL THEN 'unknown_rollup_population'
    WHEN cardinality(p.verification_rollups)<>1 THEN 'shared_multi_rollup_fee'
    WHEN p.l2_selection->'include_all_transactions' IS DISTINCT FROM 'true'::jsonb THEN 'selective_l2_scan'
    WHEN p.l1_bootstrap<>0 OR p.l2_bootstrap<>0 THEN 'prior_history_unavailable'
    WHEN NOT EXISTS(SELECT FROM ranges s WHERE s.l1_chain_id=p.chain_instance_id AND s.l2_chain_id=p.l2_chain_id
       AND s.transaction_hash=p.transaction_hash AND s.block_hash=p.block_hash AND s.complete)
      THEN 'incomplete_applied_range'
    WHEN EXISTS(SELECT FROM eez.blocks b WHERE b.chain_instance_id=p.chain_instance_id AND b.canonical
       AND b.number<=p.block_number AND b.receipt_coverage<>'complete') THEN 'prior_receipt_gap'
    WHEN EXISTS(SELECT FROM eez.canonical_receipts r LEFT JOIN eez.posting_analysis a
       ON a.chain_instance_id=r.chain_instance_id AND a.transaction_hash=r.transaction_hash
       WHERE r.chain_instance_id=p.chain_instance_id AND r.recipient=p.recipient AND r.success
       AND (r.block_number,r.transaction_index)<(p.block_number,p.transaction_index)
       AND (a.state IS NULL OR a.state NOT IN ('other','decoded')
         OR (a.state='decoded' AND a.verification_rollups IS NULL))) THEN 'prior_application_unknown'
    WHEN EXISTS(SELECT FROM posts prior WHERE prior.chain_instance_id=p.chain_instance_id
       AND prior.l2_chain_id=p.l2_chain_id AND prior.success
       AND (prior.block_number,prior.transaction_index)<(p.block_number,p.transaction_index)
       AND NOT EXISTS(SELECT FROM ranges s WHERE s.l1_chain_id=prior.chain_instance_id
         AND s.l2_chain_id=prior.l2_chain_id AND s.transaction_hash=prior.transaction_hash
         AND s.block_hash=prior.block_hash AND s.complete)) THEN 'prior_applied_range_unknown'
    WHEN p.accounting_gap IS NOT NULL THEN 'receipt_cost_gap'
    ELSE 'verified' END AS denominator_status,
   (SELECT count(*) FROM first_members m WHERE m.chain_instance_id=p.chain_instance_id
      AND m.l2_chain_id=p.l2_chain_id AND m.post_hash=p.transaction_hash
      AND m.post_block_hash=p.block_hash) AS first_applied_transactions
 FROM posts p
)
SELECT chain_instance_id AS l1_chain_id,l2_chain_id,deployment_id,target_rollup AS rollup_id,
 transaction_hash,block_hash,block_number,transaction_index,timestamp_seconds,
 success,execution_paid,blob_paid,accounting_gap,source_valid AND context_valid AS source_valid,
 denominator_status,
 CASE WHEN denominator_status='verified' THEN first_applied_transactions
      WHEN denominator_status='reverted_post' THEN 0::bigint END AS settled_transactions,
 CASE WHEN denominator_status='verified' AND first_applied_transactions>0
   THEN trunc((execution_paid+blob_paid)/first_applied_transactions) END AS posting_wei_per_settled_transaction_floor
FROM classified;

GRANT SELECT ON eez.settled_transaction_costs TO eez_observer_writer,eez_observer_reader;
