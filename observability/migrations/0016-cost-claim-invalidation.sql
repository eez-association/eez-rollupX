-- Remember declared paid-attempt identity independently of asynchronous check
-- scheduling/outcome. Quarantining a parent must not erase a competing claim.
CREATE TABLE eez.lifecycle_cost_claims (
 source_id BIGINT NOT NULL,
 epoch eez.epoch16 NOT NULL,
 sequence eez.u64 NOT NULL,
 l1 JSONB NOT NULL,
 l2 JSONB NOT NULL,
 post_hash eez.hash32 NOT NULL,
 has_post BOOLEAN NOT NULL,
 post JSONB,
 snapshot JSONB,
 sources JSONB CHECK(sources IS NULL OR (jsonb_typeof(sources)='array' AND jsonb_array_length(sources)<=4096)),
 PRIMARY KEY(source_id,epoch,sequence),
 FOREIGN KEY(source_id,epoch,sequence) REFERENCES eez.lifecycle_events(source_id,epoch,sequence)
);

CREATE FUNCTION eez.remember_cost_claim(source BIGINT,ep eez.epoch16,seq eez.u64,event JSONB)
 RETURNS void LANGUAGE plpgsql AS $$
DECLARE scope1 jsonb=event#>'{details,l1}'; scope2 jsonb=event#>'{details,l2}'; members jsonb;
BEGIN
 IF event->>'event'<>'reconciled' OR event#>>'{attempt,post_batch_hash}' !~ '^[0-9a-f]{64}$'
   OR event#>>'{attempt,post_batch_hash}' IS NULL
   OR scope1->>'chain_id' IS NULL OR scope1->>'genesis_hash' !~ '^[0-9a-f]{64}$'
   OR scope1->>'genesis_hash' IS NULL OR scope1->>'rollup_id' IS NULL
   OR scope2->>'chain_id' IS NULL OR scope2->>'genesis_hash' !~ '^[0-9a-f]{64}$'
   OR scope2->>'genesis_hash' IS NULL OR scope2->>'rollup_id' IS NULL THEN RETURN; END IF;
 IF jsonb_typeof(event#>'{details,sources}')='array'
   AND jsonb_array_length(event#>'{details,sources}')<=4096 THEN
   SELECT COALESCE(jsonb_agg(jsonb_build_object('transaction_hash',entry->'transaction_hash',
     'direction',entry->'direction','receipt',entry->'receipt')),'[]'::jsonb)
   INTO members FROM jsonb_array_elements(event#>'{details,sources}') entry;
 END IF;
 INSERT INTO eez.lifecycle_cost_claims(source_id,epoch,sequence,l1,l2,post_hash,has_post,post,snapshot,sources)
 VALUES(source,ep,seq,
   jsonb_build_object('chain_id',scope1->'chain_id','genesis_hash',scope1->'genesis_hash','rollup_id',scope1->'rollup_id'),
   jsonb_build_object('chain_id',scope2->'chain_id','genesis_hash',scope2->'genesis_hash','rollup_id',scope2->'rollup_id'),
   decode(event#>>'{attempt,post_batch_hash}','hex'),
   COALESCE(jsonb_typeof(event#>'{details,post}')='object',false),event#>'{details,post}',NULLIF(event->'anchor','null'::jsonb),members)
 ON CONFLICT(source_id,epoch,sequence) DO UPDATE SET
   snapshot=COALESCE(eez.lifecycle_cost_claims.snapshot,EXCLUDED.snapshot),
   sources=COALESCE(eez.lifecycle_cost_claims.sources,EXCLUDED.sources)
 WHERE eez.lifecycle_cost_claims.l1=EXCLUDED.l1 AND eez.lifecycle_cost_claims.l2=EXCLUDED.l2
   AND eez.lifecycle_cost_claims.post_hash=EXCLUDED.post_hash
   AND eez.lifecycle_cost_claims.has_post=EXCLUDED.has_post
   AND eez.lifecycle_cost_claims.post IS NOT DISTINCT FROM EXCLUDED.post;
END $$;

CREATE FUNCTION eez.capture_cost_claim() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='complete' THEN PERFORM eez.remember_cost_claim(NEW.source_id,NEW.epoch,NEW.sequence,NEW.payload->'event'); END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER capture_cost_claim AFTER INSERT OR UPDATE OF state,payload ON eez.lifecycle_events
 FOR EACH ROW EXECUTE FUNCTION eez.capture_cost_claim();

-- Existing coherent cache decisions have explicitly validated configured scopes.
-- They can recover metadata where an older parent was already quarantined, and
-- fill optional fields in legacy synthetic/incomplete raw records. Gap decisions
-- never overwrite previously retained identity.
CREATE FUNCTION eez.remember_checked_cost_claim(source BIGINT,ep eez.epoch16,seq eez.u64,
 context1 BIGINT,context2 BIGINT,decision JSONB) RETURNS void LANGUAGE plpgsql AS $$
DECLARE scope1 jsonb;scope2 jsonb;
BEGIN
 IF decision IS NULL THEN RETURN; END IF;
 SELECT jsonb_build_object('chain_id',chain_id,'genesis_hash',encode(genesis_hash,'hex'),'rollup_id',rollup_id)
 INTO scope1 FROM eez.chain_instances WHERE id=context1 AND role='l1';
 SELECT jsonb_build_object('chain_id',chain_id,'genesis_hash',encode(genesis_hash,'hex'),'rollup_id',rollup_id)
 INTO scope2 FROM eez.chain_instances WHERE id=context2 AND role='l2';
 IF scope1 IS NULL OR scope2 IS NULL THEN RETURN; END IF;
 PERFORM eez.remember_cost_claim(source,ep,seq,jsonb_build_object('event','reconciled',
   'attempt',decision->'owner','anchor',decision->'snapshot','details',jsonb_build_object(
   'l1',scope1,'l2',scope2,'post',decision->'post','sources',decision->'sources')));
END $$;
CREATE FUNCTION eez.capture_checked_cost_claim() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='coherent' THEN PERFORM eez.remember_checked_cost_claim(NEW.source_id,NEW.epoch,NEW.sequence,
   NEW.context_l1,NEW.context_l2,NEW.decision); END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER capture_checked_cost_claim AFTER INSERT OR UPDATE OF state,decision ON eez.lifecycle_cost_checks
 FOR EACH ROW EXECUTE FUNCTION eez.capture_checked_cost_claim();

SELECT eez.remember_cost_claim(source_id,epoch,sequence,payload->'event')
 FROM eez.lifecycle_events WHERE state='complete' AND payload->'event'->>'event'='reconciled';
SELECT eez.remember_checked_cost_claim(source_id,epoch,sequence,context_l1,context_l2,decision)
 FROM eez.lifecycle_cost_checks WHERE state='coherent';

CREATE FUNCTION eez.guard_cost_claim() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF ROW(NEW.source_id,NEW.epoch,NEW.sequence,NEW.l1,NEW.l2,NEW.post_hash,NEW.has_post,NEW.post)
  IS DISTINCT FROM ROW(OLD.source_id,OLD.epoch,OLD.sequence,OLD.l1,OLD.l2,OLD.post_hash,OLD.has_post,OLD.post)
  OR (OLD.snapshot IS NOT NULL AND NEW.snapshot IS DISTINCT FROM OLD.snapshot)
  OR (OLD.sources IS NOT NULL AND NEW.sources IS DISTINCT FROM OLD.sources) THEN
  RAISE EXCEPTION 'declared cost claim identity is immutable' USING ERRCODE='23000';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER cost_claim_guard BEFORE UPDATE ON eez.lifecycle_cost_claims
 FOR EACH ROW EXECUTE FUNCTION eez.guard_cost_claim();

CREATE VIEW eez.lifecycle_cost_claim_history AS
 SELECT c.*,l1.id AS context_l1,l2.id AS context_l2,l1.deployment_id,l2.rollup_id,
   CASE WHEN snapshot.canonical IS FALSE THEN 'orphaned_snapshot'
     ELSE COALESCE(evidence.evidence_state,'unchecked') END AS evidence_state,
   evidence.decision->'population' AS population
 FROM eez.lifecycle_cost_claims c JOIN eez.lifecycle_sources producer ON producer.id=c.source_id
 JOIN eez.chain_instances l1 ON l1.deployment_id=producer.deployment_id AND l1.role='l1'
   AND c.l1->>'chain_id'=l1.chain_id::text AND c.l1->>'genesis_hash'=encode(l1.genesis_hash,'hex')
   AND c.l1->>'rollup_id'=l1.rollup_id::text
 JOIN eez.chain_instances l2 ON l2.deployment_id=producer.deployment_id AND l2.role='l2'
   AND c.l2->>'chain_id'=l2.chain_id::text AND c.l2->>'genesis_hash'=encode(l2.genesis_hash,'hex')
   AND c.l2->>'rollup_id'=l2.rollup_id::text
 LEFT JOIN eez.lifecycle_cost_evidence evidence ON evidence.source_id=c.source_id AND evidence.epoch=c.epoch
   AND evidence.sequence=c.sequence AND evidence.context_l1=l1.id AND evidence.context_l2=l2.id
 LEFT JOIN eez.blocks snapshot ON snapshot.chain_instance_id=l1.id
   AND encode(snapshot.hash,'hex')=c.snapshot->>'hash' AND snapshot.number::text=c.snapshot->>'number'
 WHERE c.has_post;

-- A legacy gap whose already-quarantined parent lost both identity and a prior
-- decision cannot safely be attached to any one post. Preserve that context's
-- uncertainty until explicit evidence is recovered; never infer a bare hash.
CREATE VIEW eez.unbound_cost_claim_contexts AS
 SELECT DISTINCT check_row.context_l1,check_row.context_l2
 FROM eez.lifecycle_cost_checks check_row JOIN eez.lifecycle_events event USING(source_id,epoch,sequence)
 LEFT JOIN eez.lifecycle_cost_claims claim USING(source_id,epoch,sequence)
 WHERE claim.source_id IS NULL AND event.state<>'complete';

CREATE OR REPLACE VIEW eez.posting_allocation_membership AS
 WITH grouped AS (
 SELECT context_l1,context_l2,post_hash::bytea AS post_hash,
   count(*) FILTER(WHERE evidence_state IS DISTINCT FROM 'orphaned_snapshot') AS active_claims,
   count(*) FILTER(WHERE evidence_state='verified') AS verified_claims,
   count(DISTINCT population) FILTER(WHERE evidence_state='verified') AS populations,
   min(population::text) FILTER(WHERE evidence_state='verified')::jsonb AS population
 FROM eez.lifecycle_cost_claim_history GROUP BY context_l1,context_l2,post_hash
 )
 SELECT *,CASE WHEN verified_claims>0 AND active_claims=verified_claims AND populations=1
   AND NOT EXISTS(SELECT FROM eez.unbound_cost_claim_contexts unknown
     WHERE unknown.context_l1=grouped.context_l1 AND unknown.context_l2=grouped.context_l2)
   THEN 'native_membership_receipt_verified' ELSE 'unknown' END AS evidence_class
 FROM grouped;

-- An unchecked explicit claim can invalidate uniqueness without authorizing an
-- attribution. Its source receipt identity must be explicit; height alone does
-- not invent an owner. Unknown/unbounded source populations conservatively
-- prevent a unique-owner claim within that exact declared posting occurrence.
CREATE VIEW eez.companion_possible_claims AS
 SELECT DISTINCT claim.context_l1,claim.context_l2,claim.post_hash,
   decode(claim.post#>>'{block,hash}','hex') AS post_block_hash,
   decode(source->>'transaction_hash','hex') AS source_hash,
   decode(source#>>'{receipt,block,hash}','hex') AS source_block_hash
 FROM eez.lifecycle_cost_claim_history claim
 CROSS JOIN LATERAL jsonb_array_elements(claim.sources) source
 WHERE claim.evidence_state<>'orphaned_snapshot' AND source->>'direction'='inbound'
   AND source#>'{receipt,block,hash}' IS NOT NULL
   AND source#>>'{receipt,block,hash}'=claim.post#>>'{block,hash}'
   AND (source#>>'{receipt,transaction_index}')::eez.u64>(claim.post->>'transaction_index')::eez.u64;

CREATE OR REPLACE VIEW eez.companion_receipts AS
 SELECT receipt.*,candidate.context_l2,candidate.post_hash,candidate.post_block_hash,
   'native_membership_receipt_verified'::text AS evidence_class
 FROM eez.canonical_receipt_facts receipt
 JOIN eez.companion_attribution_candidates candidate ON candidate.context_l1=receipt.chain_instance_id
   AND candidate.source_hash=receipt.transaction_hash AND candidate.source_block_hash=receipt.block_hash
 WHERE NOT EXISTS(SELECT FROM eez.companion_possible_claims other
   WHERE other.context_l1=candidate.context_l1 AND other.source_hash=candidate.source_hash
     AND other.source_block_hash=candidate.source_block_hash
     AND (other.context_l2,other.post_hash,other.post_block_hash)
       IS DISTINCT FROM (candidate.context_l2,candidate.post_hash,candidate.post_block_hash))
 AND NOT EXISTS(SELECT FROM eez.lifecycle_cost_claim_history unknown
   WHERE unknown.context_l1=candidate.context_l1 AND unknown.evidence_state<>'orphaned_snapshot'
     AND unknown.sources IS NULL AND unknown.post#>>'{block,hash}'=encode(candidate.post_block_hash,'hex')
     AND (unknown.context_l2,unknown.post_hash) IS DISTINCT FROM (candidate.context_l2,candidate.post_hash))
 AND NOT EXISTS(SELECT FROM eez.unbound_cost_claim_contexts unknown
   WHERE unknown.context_l1=candidate.context_l1);

GRANT SELECT,INSERT,UPDATE ON eez.lifecycle_cost_claims TO eez_observer_writer;
GRANT SELECT ON eez.lifecycle_cost_claim_history,eez.unbound_cost_claim_contexts,
 eez.companion_possible_claims TO eez_observer_writer,eez_observer_reader;
