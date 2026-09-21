-- Local decision/history queries only. Canonical terminal promotion is separate.
BEGIN;
-- Match the actual writer/reader sessions. SET ROLE alone does not inherit
-- their jit setting; this fixture runs mutations as the administrative login.
SET LOCAL jit=off;
SET LOCAL statement_timeout='10s';
INSERT INTO eez.deployments(environment,name) VALUES('fixture','admission-status') RETURNING id AS deployment \gset
INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory,last_success_at)
 VALUES(:deployment,'composer','history','/history',now()) RETURNING id AS source_id \gset
INSERT INTO eez.lifecycle_epochs(source_id,epoch,liveness,last_probe_at,fully_scanned)
 VALUES(:source_id,decode(repeat('aa',16),'hex'),'live',now(),true),
       (:source_id,decode(repeat('bb',16),'hex'),'unknown',now(),false);
CREATE TEMP TABLE admission_fixture(source_id bigint,source jsonb,member jsonb,attempt jsonb);
INSERT INTO admission_fixture SELECT :source_id,
 jsonb_build_object('chain_id',1,'genesis_hash',repeat('11',32),'rollup_id',0,'transaction_hash',repeat('22',32),'direction','inbound'),NULL,
 jsonb_build_object('epoch',repeat('aa',16),'generation',1,'post_batch_hash',repeat('33',32));
UPDATE admission_fixture SET member=jsonb_build_object('source',source,'admission',jsonb_build_object('epoch',repeat('aa',16),'sequence',1));
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,decode(repeat('aa',16),'hex'),1,1,'complete',jsonb_build_object('observed_at_unix_ms',100,'trace',NULL,'event',jsonb_build_object('event','admitted','source',source,'previous',NULL,'replaced',NULL)) FROM admission_fixture;
DO $$ BEGIN
 IF (SELECT status FROM eez.lifecycle_admission_status WHERE instance='history')<>'unresolved' THEN RAISE EXCEPTION 'live admission is unresolved'; END IF;
END $$;
UPDATE eez.lifecycle_epochs SET last_probe_at=now()-interval '100 seconds' WHERE source_id=:source_id;
DO $$ BEGIN
 IF (SELECT liveness||':'||status FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown:unresolved' THEN RAISE EXCEPTION 'stale liveness must not imply an ended process'; END IF;
END $$;
-- An unknown/missing writer is not an ended epoch, and UTC is not causal order.
UPDATE eez.lifecycle_epochs SET liveness='unknown' WHERE source_id=:source_id;
DO $$ BEGIN
 IF (SELECT status FROM eez.lifecycle_admission_status WHERE instance='history')<>'unresolved' THEN RAISE EXCEPTION 'unknown writer must not imply restart'; END IF;
END $$;
UPDATE eez.lifecycle_epochs SET liveness='ended',ended_at=now() WHERE source_id=:source_id AND epoch=decode(repeat('aa',16),'hex');
DO $$ BEGIN
 IF (SELECT status FROM eez.lifecycle_admission_status WHERE instance='history')<>'unresolved_restart' THEN RAISE EXCEPTION 'ended unresolved admission'; END IF;
END $$;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,decode(repeat('aa',16),'hex'),3,1,'complete',jsonb_build_object('observed_at_unix_ms',50,'trace',NULL,'event',jsonb_build_object('event','terminal','member',member,'attempt',NULL,'outcome','pool_discarded','assurance','local_policy','anchor',NULL,'elapsed_ns',1)) FROM admission_fixture;
DO $$ BEGIN
 IF (SELECT status||':'||outcome||':'||evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'local_policy:pool_discarded:local_policy' THEN RAISE EXCEPTION 'late local policy evidence resolves ended history'; END IF;
END $$;
-- All local outcomes use the same explicit ownership contract. Invalid source,
-- epoch, owner, anchor or missing admission never acquires local assurance.
DO $$
DECLARE base jsonb; changed jsonb; expected text; target text;
BEGIN
 SELECT payload INTO base FROM eez.lifecycle_events WHERE source_id=(SELECT source_id FROM admission_fixture) AND sequence=3;
 FOREACH target IN ARRAY ARRAY['attempts_exhausted','nonce_gap','pool_discarded','replaced'] LOOP
   UPDATE eez.lifecycle_events SET payload=jsonb_set(base,'{event,outcome}',to_jsonb(target)) WHERE source_id=(SELECT source_id FROM admission_fixture) AND sequence=3;
   IF (SELECT outcome FROM eez.lifecycle_admission_status WHERE instance='history')<>target THEN RAISE EXCEPTION 'local outcome omitted: %',target; END IF;
 END LOOP;
 FOR changed,expected IN SELECT * FROM (VALUES
   (base #- '{event,assurance}','requires_canonical_validation'),
   (base #- '{event,outcome}','requires_canonical_validation'),
   (jsonb_set(base,'{event,member,source,transaction_hash}',to_jsonb(repeat('99',32))),'source_mismatch'),
   (jsonb_set(base,'{event,member,admission,sequence}','99'::jsonb),'missing_admission'),
   (jsonb_set(base,'{event,attempt}',jsonb_build_object('epoch',repeat('bb',16),'generation',1,'post_batch_hash',repeat('33',32))),'invalid_ownership_scope'),
   (jsonb_set(base,'{event,anchor}',jsonb_build_object('number',1,'hash',repeat('44',32))),'unexpected_anchor')
 ) cases(payload,reason) LOOP
   UPDATE eez.lifecycle_events SET payload=changed WHERE source_id=(SELECT source_id FROM admission_fixture) AND sequence=3;
   IF (SELECT evidence_state FROM eez.lifecycle_local_policy_evidence WHERE source_id=(SELECT source_id FROM admission_fixture))<>expected THEN RAISE EXCEPTION 'invalid policy evidence accepted: %',expected; END IF;
 END LOOP;
 UPDATE eez.lifecycle_events SET payload=base WHERE source_id=(SELECT source_id FROM admission_fixture) AND sequence=3;
END $$;
UPDATE eez.lifecycle_events SET epoch=decode(repeat('bb',16),'hex') WHERE source_id=:source_id AND sequence=3;
DO $$ BEGIN
 IF (SELECT evidence_state FROM eez.lifecycle_local_policy_evidence WHERE source_id=(SELECT source_id FROM admission_fixture))<>'invalid_admission_order' THEN RAISE EXCEPTION 'cross-epoch terminal ownership accepted'; END IF;
END $$;
UPDATE eez.lifecycle_events SET epoch=decode(repeat('aa',16),'hex') WHERE source_id=:source_id AND sequence=3;
INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory) VALUES(:deployment,'composer','other-history','/other-history') RETURNING id AS other_source \gset
INSERT INTO eez.lifecycle_epochs(source_id,epoch) VALUES(:other_source,decode(repeat('aa',16),'hex'));
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT :other_source,epoch,sequence,chunk_count,state,payload FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=3;
DO $$ BEGIN
 IF EXISTS(SELECT FROM eez.lifecycle_local_policy_evidence t JOIN eez.lifecycle_sources s ON s.id=t.source_id WHERE s.instance='other-history' AND t.evidence_state<>'missing_admission') OR
    (SELECT terminal_claims FROM eez.lifecycle_admission_status WHERE instance='history')<>1 THEN RAISE EXCEPTION 'instance ownership fence failed'; END IF;
END $$;
-- A second consistent observation cannot duplicate the admission itself.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,4,chunk_count,state,payload FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=3;
DO $$ BEGIN
 IF (SELECT count(*) FROM eez.lifecycle_admission_status WHERE instance='history')<>1 OR
    (SELECT status FROM eez.lifecycle_admission_status WHERE instance='history')<>'local_policy' THEN RAISE EXCEPTION 'duplicate observation multiplied or contradicted admission'; END IF;
END $$;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,elapsed_ns}','99') WHERE source_id=:source_id AND sequence=4;
DO $$ BEGIN
 IF (SELECT evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown' THEN RAISE EXCEPTION 'conflicting elapsed duration promoted'; END IF;
END $$;
UPDATE eez.lifecycle_events SET payload=jsonb_set(jsonb_set(payload,'{event,elapsed_ns}','1'),'{event,attempt}',f.attempt) FROM admission_fixture f WHERE eez.lifecycle_events.source_id=f.source_id AND sequence=4;
DO $$ BEGIN
 IF (SELECT evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown' THEN RAISE EXCEPTION 'conflicting owner promoted'; END IF;
END $$;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,attempt}','null') WHERE source_id=:source_id AND sequence=4;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,outcome}','"nonce_gap"') WHERE source_id=:source_id AND sequence=4;
DO $$ BEGIN
 IF (SELECT evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown' THEN RAISE EXCEPTION 'conflicting outcomes promoted'; END IF;
END $$;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=4;
DO $$ BEGIN
 IF (SELECT status FROM eez.lifecycle_admission_status WHERE instance='history')<>'local_policy' THEN RAISE EXCEPTION 'transport conflict not retracted'; END IF;
END $$;
-- A canonical-looking terminal remains a claim until separate anchor validation.
UPDATE eez.lifecycle_events SET payload=jsonb_set(jsonb_set(payload,'{event,outcome}','"committed"'),'{event,assurance}','"node_reconciled"') WHERE source_id=:source_id AND sequence=3;
DO $$ BEGIN
 IF (SELECT evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown' OR
    (SELECT evidence_state FROM eez.lifecycle_local_policy_evidence WHERE source_id=(SELECT source_id FROM admission_fixture))<>'requires_canonical_validation' THEN RAISE EXCEPTION 'canonical claim promoted without anchors'; END IF;
END $$;
-- Linked local policy requires one matching parent resolution, not a matching
-- admission somewhere in a contradictory or incomplete partition.
UPDATE eez.lifecycle_events SET payload=jsonb_build_object('observed_at_unix_ms',50,'trace',NULL,'event',jsonb_build_object('event','terminal','member',f.member,'attempt',f.attempt,'outcome','nonce_gap','assurance','local_policy','anchor',NULL,'elapsed_ns',7,'reconciliation',jsonb_build_object('epoch',repeat('aa',16),'sequence',2))) FROM admission_fixture f WHERE eez.lifecycle_events.source_id=f.source_id AND sequence=3;
DO $$ BEGIN
 IF (SELECT evidence_state FROM eez.lifecycle_local_policy_evidence WHERE source_id=(SELECT source_id FROM admission_fixture))<>'unresolved_reconciliation' THEN RAISE EXCEPTION 'missing parent promoted'; END IF;
END $$;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,decode(repeat('aa',16),'hex'),2,1,'complete',jsonb_build_object('observed_at_unix_ms',60,'trace',NULL,'event',jsonb_build_object('event','reconciled','attempt',attempt,'anchor',jsonb_build_object('number',9,'hash',repeat('44',32)),'details',jsonb_build_object('l1',jsonb_build_object('chain_id',1,'genesis_hash',repeat('11',32),'rollup_id',0),'l2',jsonb_build_object('chain_id',2,'genesis_hash',repeat('55',32),'rollup_id',1),'l2_snapshot',NULL,'post',NULL,'applied',NULL,'sources','[]'::jsonb,'resolutions',jsonb_build_array(jsonb_build_object('member',member,'owner',attempt,'elapsed_ns',7,'disposition',jsonb_build_object('state','terminal','outcome','nonce_gap','assurance','local_policy')))))) FROM admission_fixture;
DO $$ BEGIN
 IF (SELECT status FROM eez.lifecycle_admission_status WHERE instance='history')<>'local_policy' THEN RAISE EXCEPTION 'exact parent did not resolve policy'; END IF;
END $$;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,details,resolutions,0,elapsed_ns}','8') WHERE source_id=:source_id AND sequence=2;
DO $$ BEGIN
 IF (SELECT evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown' THEN RAISE EXCEPTION 'contradictory child/parent elapsed time accepted'; END IF;
END $$;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,details,resolutions,0,elapsed_ns}','7') WHERE source_id=:source_id AND sequence=2;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,details,resolutions}',(payload#>'{event,details,resolutions}')||(payload#>'{event,details,resolutions}')) WHERE source_id=:source_id AND sequence=2;
DO $$ BEGIN
 IF (SELECT evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown' THEN RAISE EXCEPTION 'duplicate partition accepted'; END IF;
END $$;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=2;
DO $$ BEGIN
 IF (SELECT evidence_class FROM eez.lifecycle_admission_status WHERE instance='history')<>'unknown' THEN RAISE EXCEPTION 'conflicted parent accepted'; END IF;
END $$;
-- Explicit readmission is another identity, even for exactly the same tx hash.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,decode(repeat('bb',16),'hex'),1,1,'complete',jsonb_build_object('observed_at_unix_ms',1,'trace',NULL,'event',jsonb_build_object('event','admitted','source',source,'previous',jsonb_build_object('epoch',repeat('aa',16),'sequence',1),'replaced',NULL)) FROM admission_fixture;
DO $$ BEGIN
 IF (SELECT count(*) FROM eez.lifecycle_admission_status WHERE instance='history')<>2 OR
    (SELECT count(*) FROM eez.lifecycle_admission_status WHERE instance='history' AND status='unresolved')<>1 THEN RAISE EXCEPTION 'readmission identities collapsed'; END IF;
END $$;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND epoch=decode(repeat('aa',16),'hex') AND sequence=1;
DO $$ BEGIN
 IF (SELECT count(*) FROM eez.lifecycle_admission_status WHERE instance='history')<>1 OR
    (SELECT evidence_state FROM eez.lifecycle_local_policy_evidence WHERE source_id=(SELECT source_id FROM admission_fixture))<>'missing_admission' THEN RAISE EXCEPTION 'terminal or readmission reference fabricated a missing admission'; END IF;
END $$;
SET ROLE eez_observer_reader;
SELECT count(*) FROM eez.lifecycle_admission_status WHERE instance='history';
RESET ROLE;
ROLLBACK;
SELECT 'admission status invariants passed';
