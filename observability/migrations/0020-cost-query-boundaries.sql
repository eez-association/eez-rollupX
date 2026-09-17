-- A flattened claim/evidence join evaluated native digest/lineage validation
-- once for every candidate join pair, not once for each checked claim. With
-- 101 equivalent paid claims a source-filtered allocation query exceeded the
-- existing ten-second deadline. Materialize the thin validation population
-- once per exact configured chain pair before joining durable declarations.
-- Outer context filters still select chain pairs; new producers without a
-- scheduled check retain their explicit unchecked declarations. No paid fact,
-- scope, conflict, missing-evidence or positive-orphan predicate is weakened.
CREATE OR REPLACE VIEW eez.lifecycle_cost_claim_history AS
 SELECT claim.source_id,claim.epoch,claim.sequence,claim.l1,claim.l2,claim.post_hash,
   claim.has_post,claim.post,claim.snapshot,claim.sources,
   l1.id AS context_l1,l2.id AS context_l2,l1.deployment_id,l2.rollup_id,
   claim.evidence_state,claim.population
 FROM eez.chain_instances l1 JOIN eez.chain_instances l2
   ON l2.deployment_id=l1.deployment_id AND l2.role='l2'
 CROSS JOIN LATERAL (
   WITH evidence AS MATERIALIZED (
     SELECT checked.source_id,checked.epoch,checked.sequence,checked.evidence_state,
       checked.decision->'population' AS population
     FROM eez.lifecycle_cost_evidence checked
     WHERE checked.context_l1=l1.id AND checked.context_l2=l2.id
   )
   SELECT c.*,CASE WHEN snapshot.canonical IS FALSE THEN 'orphaned_snapshot'
     ELSE COALESCE(evidence.evidence_state,'unchecked') END AS evidence_state,evidence.population
   FROM eez.lifecycle_cost_claims c JOIN eez.lifecycle_sources producer ON producer.id=c.source_id
     AND producer.deployment_id=l1.deployment_id
   LEFT JOIN evidence ON evidence.source_id=c.source_id AND evidence.epoch=c.epoch AND evidence.sequence=c.sequence
   LEFT JOIN eez.blocks snapshot ON snapshot.chain_instance_id=l1.id
     AND encode(snapshot.hash,'hex')=c.snapshot->>'hash' AND snapshot.number::text=c.snapshot->>'number'
   WHERE c.has_post
     AND c.l1->>'chain_id'=l1.chain_id::text AND c.l1->>'genesis_hash'=encode(l1.genesis_hash,'hex')
     AND c.l1->>'rollup_id'=l1.rollup_id::text
     AND c.l2->>'chain_id'=l2.chain_id::text AND c.l2->>'genesis_hash'=encode(l2.genesis_hash,'hex')
     AND c.l2->>'rollup_id'=l2.rollup_id::text
 ) claim
 WHERE l1.role='l1';
