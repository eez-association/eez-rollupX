-- SQL anchor-selection regression only. Native-coherence decisions below are
-- synthetic trusted-cache fixtures, not proof of the Rust checker/importer or
-- independent protocol replay. Raw/chunk FK stubs deliberately contain no wire
-- encoding: that boundary has separate typed/native process tests.
BEGIN;
-- Matches the observer/reader session policy: these expanded views must execute
-- within the normal query deadline; JIT compilation alone exceeded it in review.
SET LOCAL jit=off;
SET LOCAL statement_timeout='10s';
CREATE FUNCTION pg_temp.h(value bigint) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$ SELECT decode(lpad(to_hex(value),64,'0'),'hex') $$;
CREATE FUNCTION pg_temp.jh(value bigint) RETURNS text LANGUAGE sql IMMUTABLE AS $$ SELECT encode(pg_temp.h(value),'hex') $$;
CREATE FUNCTION pg_temp.block(value bigint,height bigint) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$ SELECT jsonb_build_object('number',height,'hash',pg_temp.jh(value)) $$;
CREATE FUNCTION pg_temp.assert_state(expected text, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE actual text; assurance text; rows bigint;
BEGIN
 SELECT count(*),min(canonical_state),min(evidence_class) INTO rows,actual,assurance FROM eez.lifecycle_outcome_anchors a JOIN eez.lifecycle_sources s ON s.id=a.source_id WHERE s.instance='canonical-outcomes';
 IF rows<>1 OR actual IS DISTINCT FROM expected OR assurance IS DISTINCT FROM (CASE WHEN expected='verified' THEN 'node_reconciled' ELSE 'unknown' END) THEN
  RAISE EXCEPTION '%: expected %, got % rows, state %, assurance %',label,expected,rows,actual,assurance;
 END IF;
END $$;
INSERT INTO eez.deployments(environment,name) VALUES('fixture','canonical-outcomes') RETURNING id AS deployment \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l1',1,pg_temp.h(1),0,10,now(),jsonb_build_object('contracts',jsonb_build_array('0x'||repeat('ee',20)))) RETURNING id AS l1 \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at)
 VALUES(:deployment,'l2',2,pg_temp.h(2),1,3,now()) RETURNING id AS l2 \gset
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(11),11,pg_temp.h(10),110,true,'complete'),(:l1,pg_temp.h(12),12,pg_temp.h(11),120,true,'complete'),
       (:l2,pg_temp.h(33),3,pg_temp.h(30),115,true,'complete');
INSERT INTO eez.scan_cursors(chain_instance_id,block_hash) VALUES(:l1,pg_temp.h(12)),(:l2,pg_temp.h(33));
-- P(hash32,height3) is NEVER in blocks. Q(hash33,height3) owns current L2 height3.
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 SELECT :l1,pg_temp.h(hash),decode(repeat('aa',20),'hex'),decode(repeat(recipient,20),'hex'),hash,2
 FROM (VALUES(5,'ee'),(6,'ee'),(1002,'bb'),(1003,'bb')) tx(hash,recipient);
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l2,pg_temp.h(1002),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),0,2),
       (:l2,pg_temp.h(1003),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),1,2);
INSERT INTO eez.selected_transactions SELECT chain_instance_id,hash FROM eez.transactions WHERE chain_instance_id IN (:l1,:l2);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 SELECT :l1,pg_temp.h(hash),pg_temp.h(11),idx,success,true,100,2,0,0,200,0,0
 FROM (VALUES(5,0,true),(1002,3,true),(6,4,true),(1003,7,false)) receipt(hash,idx,success);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l2,pg_temp.h(1002),pg_temp.h(33),0,true,true,100,2,0,0,200,0,0),
       (:l2,pg_temp.h(1003),pg_temp.h(33),1,false,true,100,2,0,0,200,0,0);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(5),'decoded',pg_temp.h(50),now(),ARRAY[1]::eez.u64[]),
       (:l1,pg_temp.h(6),'decoded',pg_temp.h(60),now(),ARRAY[1]::eez.u64[]);
INSERT INTO eez.correlation_sources(l1_chain_id,l2_chain_id,last_success_at) VALUES(:l1,:l2,now());
INSERT INTO eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash,last_success_at,range_observed)
 VALUES(:l1,:l2,pg_temp.h(5),pg_temp.h(11),now(),true),(:l1,:l2,pg_temp.h(6),pg_temp.h(11),now(),true);
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2,range_verified_at)
 VALUES(:l1,:l2,pg_temp.h(5),pg_temp.h(11),0,1,pg_temp.h(501),false,NULL) RETURNING id AS settlement_p \gset
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2,range_verified_at)
 VALUES(:l1,:l2,pg_temp.h(6),pg_temp.h(11),4,1,pg_temp.h(601),true,now()) RETURNING id AS settlement_q \gset
INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash) VALUES(:settlement_p,0,3,pg_temp.h(32)),(:settlement_q,0,3,pg_temp.h(33));
INSERT INTO eez.protocol_log_claims(chain_instance_id,transaction_hash,block_hash,digest,events)
 VALUES(:l1,pg_temp.h(1002),pg_temp.h(11),pg_temp.h(10020),jsonb_build_array(
   jsonb_build_object('index',0,'address',repeat('ee',20),'rollup_id',1,'kind','root','root',pg_temp.jh(32)),
   jsonb_build_object('index',1,'address',repeat('ee',20),'rollup_id',1,'kind','consumed','call_hash',pg_temp.jh(8),'queue_index','0'))),
 (:l1,pg_temp.h(6),pg_temp.h(11),pg_temp.h(60),jsonb_build_array(
   jsonb_build_object('index',2,'address',repeat('ee',20),'rollup_id',1,'kind','root','root',pg_temp.jh(33))));
INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory,last_success_at)
 VALUES(:deployment,'composer','canonical-outcomes','/canonical-outcomes',now()) RETURNING id AS source_id \gset
INSERT INTO eez.lifecycle_epochs(source_id,epoch,liveness,last_probe_at) VALUES(:source_id,decode(repeat('aa',16),'hex'),'live',now());
INSERT INTO eez.lifecycle_segments(source_id,epoch,segment_id) VALUES(:source_id,decode(repeat('aa',16),'hex'),decode(repeat('bb',16),'hex'));
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT :source_id,decode(repeat('aa',16),'hex'),sequence,1,'complete',jsonb_build_object('event',jsonb_build_object('event',kind,'attempt',jsonb_build_object('generation',1)))
 FROM (VALUES(3,'attempt_prepared'),(10,'reconciled'),(11,'terminal')) event(sequence,kind);
INSERT INTO eez.lifecycle_records(source_id,epoch,segment_id,offset_bytes,digest,raw)
 SELECT :source_id,decode(repeat('aa',16),'hex'),decode(repeat('bb',16),'hex'),sequence,pg_temp.h(sequence),'\x7b7d' FROM (VALUES(3),(10),(11)) event(sequence);
INSERT INTO eez.lifecycle_chunks(source_id,epoch,sequence,chunk_index,segment_id,offset_bytes,record,raw_digest)
 SELECT :source_id,decode(repeat('aa',16),'hex'),sequence,0,decode(repeat('bb',16),'hex'),sequence,jsonb_build_object('complete',true,'digest',pg_temp.jh(sequence)),pg_temp.h(sequence) FROM (VALUES(3),(10),(11)) event(sequence);
INSERT INTO eez.lifecycle_native_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs,parent_sequence,generation,decision)
 SELECT :source_id,decode(repeat('aa',16),'hex'),11,:l1,:l2,'coherent',
   (SELECT jsonb_agg(jsonb_build_object('epoch',repeat('aa',16),'sequence',sequence,'digest',pg_temp.jh(sequence))) FROM (VALUES(3),(10),(11)) refs(sequence)),10,1,
 jsonb_build_object(
  'member',jsonb_build_object('source',jsonb_build_object('chain_id',1,'genesis_hash',pg_temp.jh(1),'rollup_id',0,'transaction_hash',pg_temp.jh(1002),'direction','inbound'),'admission',jsonb_build_object('epoch',repeat('aa',16),'sequence',1)),
  'owner',jsonb_build_object('epoch',repeat('aa',16),'generation',1,'post_batch_hash',pg_temp.jh(5)),
  'outcome','committed','elapsed_ns',100,'observed_at_ms',120000,
  'l1_snapshot',pg_temp.block(12,12),'l2_snapshot',pg_temp.block(33,3),
  'post',jsonb_build_object('block',pg_temp.block(11,11),'transaction_index',0,'success',true),
  'applied',jsonb_build_object('block',pg_temp.block(11,11),'transaction_hash',pg_temp.jh(5),'transaction_index',0,'calldata_hash',pg_temp.jh(50),'endpoint',pg_temp.block(32,3)),
  'source',jsonb_build_object('basis','receipt','source_block',pg_temp.block(11,11),'receipt',jsonb_build_object('block',pg_temp.block(11,11),'transaction_index',3,'success',true)),
  'plan',jsonb_build_object('source_actions',jsonb_build_array(jsonb_build_object('call_hash',pg_temp.jh(8),'source_index',0))));
-- Actual admission and terminal rows exercise the admission aggregation layer.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,1,1,'complete',jsonb_build_object('observed_at_unix_ms',100000,'event',jsonb_build_object('event','admitted','source',decision#>'{member,source}','previous',NULL,'replaced',NULL)) FROM eez.lifecycle_native_checks WHERE source_id=:source_id;
UPDATE eez.lifecycle_events event SET payload=jsonb_build_object('observed_at_unix_ms',120000,'event',jsonb_build_object('event','terminal','member',n.decision->'member','attempt',n.decision->'owner','outcome','committed','assurance','node_reconciled','elapsed_ns',100,'anchor',NULL,'reconciliation',jsonb_build_object('epoch',repeat('aa',16),'sequence',10)))
 FROM eez.lifecycle_native_checks n WHERE event.source_id=n.source_id AND event.epoch=n.epoch AND event.sequence=n.sequence AND event.source_id=:source_id;
CREATE TEMP TABLE outcomes_fixture AS SELECT :l1::bigint AS l1,:l2::bigint AS l2,:source_id::bigint AS source_id,decision AS baseline FROM eez.lifecycle_native_checks WHERE source_id=:source_id;
SET CONSTRAINTS ALL IMMEDIATE;
SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');
DO $$ BEGIN
 IF (SELECT count(*) FROM eez.canonical_settlement_endpoints WHERE l1_chain_id=(SELECT l1 FROM outcomes_fixture))<>2 OR
    EXISTS(SELECT FROM eez.blocks WHERE chain_instance_id=(SELECT l2 FROM outcomes_fixture) AND hash=pg_temp.h(32)) OR
    EXISTS(SELECT FROM eez.canonical_settled_l2_blocks WHERE l2_chain_id=(SELECT l2 FROM outcomes_fixture) AND hash=pg_temp.h(32)) THEN RAISE EXCEPTION 'fixture must distinguish historical root P from currently indexed Q'; END IF;
END $$;

SAVEPOINT variant;
UPDATE eez.chain_instances SET last_success_at=now()-interval '91 seconds' WHERE id=:l1;
SELECT pg_temp.assert_state('stale_chain','stale L1');
ROLLBACK TO variant;
UPDATE eez.chain_instances SET collection_error='transport' WHERE id=:l2;
SELECT pg_temp.assert_state('stale_chain','failed L2 collection');
ROLLBACK TO variant;
UPDATE eez.chain_instances SET frozen_reason='conflicting_fact' WHERE id=:l2;
SELECT pg_temp.assert_state('frozen_chain','frozen L2');
ROLLBACK TO variant;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(12);
SELECT pg_temp.assert_state('orphaned_l1_snapshot','orphaned scan watermark');
ROLLBACK TO variant;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(11);
SELECT pg_temp.assert_state('missing_l2_snapshot','orphaned settlement retracts Q and P');
ROLLBACK TO variant;
UPDATE eez.correlation_sources SET collection_error='transport' WHERE l1_chain_id=:l1 AND l2_chain_id=:l2;
SELECT pg_temp.assert_state('stale_settlement','stale correlation cannot anchor snapshot');
ROLLBACK TO variant;
UPDATE eez.correlation_sources SET frozen_reason='l2_finalized_contradiction' WHERE l1_chain_id=:l1 AND l2_chain_id=:l2;
SELECT pg_temp.assert_state('frozen_settlement','frozen correlation prevents promotion');
ROLLBACK TO variant;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=10;
SELECT pg_temp.assert_state('changed_evidence','late parent conflict retracts cached promotion');
ROLLBACK TO variant;

UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{l1_snapshot,number}','13') WHERE source_id=:source_id;
SELECT pg_temp.assert_state('missing_l1_snapshot','exact L1 height/hash pair required');
ROLLBACK TO variant;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{l2_snapshot,hash}',to_jsonb(pg_temp.jh(40))) WHERE source_id=:source_id;
SELECT pg_temp.assert_state('missing_l2_snapshot','same L2 height with wrong hash is insufficient');
ROLLBACK TO variant;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{post,success}','false') WHERE source_id=:source_id;
SELECT pg_temp.assert_state('missing_post_receipt','posting receipt success must match');
ROLLBACK TO variant;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{applied,calldata_hash}',to_jsonb(pg_temp.jh(51))) WHERE source_id=:source_id;
SELECT pg_temp.assert_state('missing_applied_post','exact applied calldata identity');
ROLLBACK TO variant;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{source,receipt,transaction_index}','2') WHERE source_id=:source_id;
SELECT pg_temp.assert_state('missing_source_receipt','source receipt position must match');
ROLLBACK TO variant;
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2)
 VALUES(:l1,:l2,pg_temp.h(5),pg_temp.h(11),0,1,pg_temp.h(502),false);
SELECT pg_temp.assert_state('missing_applied_endpoint','conflicting correlation claim retracts P');
ROLLBACK TO variant;

-- A conflicting complete log claim retracts endpoints/consumption, never fees.
INSERT INTO eez.protocol_log_claims SELECT chain_instance_id,transaction_hash,block_hash,pg_temp.h(10021),'[]'::jsonb,now() FROM eez.protocol_log_claims WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_applied_endpoint','conflicting source/root logs');
DO $$ BEGIN IF (SELECT known_total_paid FROM eez.canonical_cost_totals WHERE chain_instance_id=(SELECT l1 FROM outcomes_fixture))<>800 THEN RAISE EXCEPTION 'log conflict changed immutable receipt costs'; END IF; END $$;
ROLLBACK TO variant;

-- Admin fixture mutation of decoded claims simulates independently collected
-- alternate data. Production writer permissions remain INSERT-only.
UPDATE eez.protocol_log_claims SET events=jsonb_set(events,'{0,root}',to_jsonb(pg_temp.jh(99))) WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_applied_endpoint','wrong root');
ROLLBACK TO variant;
UPDATE eez.protocol_log_claims SET events=jsonb_set(events,'{0,address}',to_jsonb(repeat('ab',20))) WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_applied_endpoint','wrong root emitter');
ROLLBACK TO variant;
UPDATE eez.protocol_log_claims SET events=jsonb_set(events,'{0,rollup_id}','2') WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_applied_endpoint','wrong root rollup');
ROLLBACK TO variant;
UPDATE eez.protocol_log_claims SET events=events-'1'::int WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_source_consumption','missing consumption');
ROLLBACK TO variant;
UPDATE eez.protocol_log_claims SET events=jsonb_set(events,'{1,queue_index}','"1"') WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_source_consumption','wrong consumption coordinate');
ROLLBACK TO variant;
UPDATE eez.protocol_log_claims SET events=jsonb_set(events,'{1,address}',to_jsonb(repeat('ab',20))) WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_source_consumption','wrong consumption emitter');
ROLLBACK TO variant;
UPDATE eez.protocol_log_claims SET events=jsonb_set(events,'{1,rollup_id}','2') WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
SELECT pg_temp.assert_state('missing_source_consumption','wrong consumption rollup');
ROLLBACK TO variant;

-- A direct successful EEZ call between post and companion is only harmless when
-- its exact decoded verification table proves it does not wipe this rollup.
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type) VALUES(:l1,pg_temp.h(7),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),7,2);
INSERT INTO eez.selected_transactions(chain_instance_id,transaction_hash) VALUES(:l1,pg_temp.h(7));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(7),pg_temp.h(11),2,true,true,100,2,0,0,200,0,0);
SAVEPOINT intervening;
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups) VALUES(:l1,pg_temp.h(7),'decoded',pg_temp.h(70),now(),ARRAY[2]::eez.u64[]);
SELECT pg_temp.assert_state('verified','unrelated rollup post preserves P window');
ROLLBACK TO intervening;
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups) VALUES(:l1,pg_temp.h(7),'decoded',pg_temp.h(70),now(),ARRAY[1]::eez.u64[]);
SELECT pg_temp.assert_state('missing_applied_endpoint','same rollup post closes P root window');
DELETE FROM eez.selected_transactions WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(7);
SELECT pg_temp.assert_state('missing_applied_endpoint','known unselected same rollup post still closes root window');
ROLLBACK TO intervening;
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at) VALUES(:l1,pg_temp.h(7),'decoded',pg_temp.h(70),now());
SELECT pg_temp.assert_state('missing_applied_endpoint','unknown verification table cannot prove P window');
ROLLBACK TO intervening;
SELECT pg_temp.assert_state('missing_applied_endpoint','missing classification cannot prove P window');
ROLLBACK TO intervening;
-- When P's root is already emitted by the original post, an intervening post
-- still invalidates later inbound source consumption, independently of endpoints.
UPDATE eez.protocol_log_claims SET events=events-0 WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(1002);
INSERT INTO eez.protocol_log_claims(chain_instance_id,transaction_hash,block_hash,digest,events)
 VALUES(:l1,pg_temp.h(5),pg_temp.h(11),pg_temp.h(500),jsonb_build_array(jsonb_build_object('index',0,'address',repeat('ee',20),'rollup_id',1,'kind','root','root',pg_temp.jh(32))));
SAVEPOINT source_window;
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups) VALUES(:l1,pg_temp.h(7),'decoded',pg_temp.h(70),now(),ARRAY[1]::eez.u64[]);
SELECT pg_temp.assert_state('ambiguous_source_window','same rollup wipes consumption after already emitted P');
DELETE FROM eez.selected_transactions WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(7);
SELECT pg_temp.assert_state('ambiguous_source_window','known unselected same rollup post still closes source window');
ROLLBACK TO source_window;
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups) VALUES(:l1,pg_temp.h(7),'decoded',pg_temp.h(70),now(),ARRAY[2]::eez.u64[]);
SELECT pg_temp.assert_state('verified','unrelated rollup preserves source window after already emitted P');
ROLLBACK TO source_window;
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at) VALUES(:l1,pg_temp.h(7),'decoded',pg_temp.h(70),now());
SELECT pg_temp.assert_state('ambiguous_source_window','unknown rollup table blocks source window even with known P');
ROLLBACK TO variant;

-- Preserve all native canonical outcome labels; non-committed source handling
-- requires its own available receipt facts, not a fabricated original post.
DO $$ DECLARE result text; BEGIN
 FOREACH result IN ARRAY ARRAY['source_completed','source_completed_with_missing_effects','source_nonce_spent'] LOOP
   UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(jsonb_set(jsonb_set(f.baseline,'{outcome}',to_jsonb(result)),'{post}','null'),'{applied}','null') FROM outcomes_fixture f WHERE eez.lifecycle_native_checks.source_id=f.source_id;
   IF result='source_nonce_spent' THEN
     UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{source}',jsonb_build_object('basis','nonce','source_block',NULL,'receipt',NULL)) WHERE source_id=(SELECT source_id FROM outcomes_fixture);
   END IF;
   PERFORM pg_temp.assert_state('verified',result);
   IF (SELECT outcome FROM eez.lifecycle_outcome_anchors WHERE source_id=(SELECT source_id FROM outcomes_fixture))<>result THEN RAISE EXCEPTION 'native outcome label changed'; END IF;
 END LOOP;
END $$;
ROLLBACK TO variant;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(jsonb_set(jsonb_set(jsonb_set(jsonb_set(decision,'{outcome}','"source_reverted"'),'{post}','null'),'{applied}','null'),'{member,source,transaction_hash}',to_jsonb(pg_temp.jh(1003))),'{source,receipt}',jsonb_build_object('block',pg_temp.block(11,11),'transaction_index',7,'success',false)) WHERE source_id=:source_id;
SELECT pg_temp.assert_state('verified','exact reverted source preserves source_reverted');
ROLLBACK TO variant;

-- Outbound own-group P is an applied prefix. Its independent canonical receipt
-- can reside in Q at the same height; neither reference is silently rewritten.
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(jsonb_set(decision,'{member,source}',jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1,'direction','outbound','transaction_hash',pg_temp.jh(1002))),'{source}',jsonb_build_object('basis','applied_outbound_group','source_block',pg_temp.block(32,3),'receipt',NULL)) WHERE source_id=:source_id;
SELECT pg_temp.assert_state('verified','own outbound P with current source receipt Q');
DO $$ BEGIN IF (SELECT observed_source_receipt_block_hash FROM eez.lifecycle_outcome_anchors WHERE source_id=(SELECT source_id FROM outcomes_fixture))<>pg_temp.h(33) THEN RAISE EXCEPTION 'source receipt Q was confused with applied P'; END IF; END $$;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{member,source,transaction_hash}',to_jsonb(pg_temp.jh(1003))) WHERE source_id=:source_id;
SELECT pg_temp.assert_state('contradictory_source_receipt','known outbound revert cannot claim successful service');
ROLLBACK TO variant;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(jsonb_set(jsonb_set(jsonb_set(decision,'{member,source}',jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1,'direction','outbound','transaction_hash',pg_temp.jh(1002))),'{source}',jsonb_build_object('basis','canonical_inclusion','source_block',pg_temp.block(33,3),'receipt',NULL)),'{post}','null'),'{applied}','null') WHERE source_id=:source_id;
SELECT pg_temp.assert_state('verified','competing canonical Q without original post/applied evidence');
ROLLBACK TO variant;
SELECT pg_temp.assert_state('verified','baseline fully restored after every mutation');
CREATE FUNCTION pg_temp.assert_admission(expected text,label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE actual text; rows bigint;
BEGIN
 SELECT count(*),min(status||':'||COALESCE(outcome,'null')||':'||evidence_class) INTO rows,actual FROM eez.lifecycle_admission_status WHERE instance='canonical-outcomes';
 IF rows<>1 OR actual IS DISTINCT FROM expected THEN RAISE EXCEPTION '%: expected %, got % rows, %',label,expected,rows,actual; END IF;
END $$;
SELECT pg_temp.assert_admission('canonical:committed:node_reconciled','actual admission promoted');
SAVEPOINT admission_variant;
-- This SQL helper seeds complete-cache FK stubs, not real lifecycle wire bytes.
CREATE FUNCTION pg_temp.copy_terminal(original bigint,replacement bigint) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
  SELECT source_id,epoch,replacement,chunk_count,state,payload FROM eez.lifecycle_events WHERE source_id=(SELECT source_id FROM outcomes_fixture) AND sequence=original;
 INSERT INTO eez.lifecycle_records(source_id,epoch,segment_id,offset_bytes,digest,raw)
  SELECT source_id,epoch,decode(repeat('bb',16),'hex'),replacement,pg_temp.h(replacement),'\x7b7d'::bytea FROM eez.lifecycle_events WHERE source_id=(SELECT source_id FROM outcomes_fixture) AND sequence=replacement;
 INSERT INTO eez.lifecycle_chunks(source_id,epoch,sequence,chunk_index,segment_id,offset_bytes,record,raw_digest)
  SELECT source_id,epoch,replacement,0,decode(repeat('bb',16),'hex'),replacement,jsonb_build_object('complete',true,'digest',pg_temp.jh(replacement)),pg_temp.h(replacement) FROM eez.lifecycle_events WHERE source_id=(SELECT source_id FROM outcomes_fixture) AND sequence=replacement;
 INSERT INTO eez.lifecycle_native_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs,parent_sequence,generation,decision)
  SELECT source_id,epoch,replacement,context_l1,context_l2,state,
   (SELECT jsonb_agg(CASE WHEN (ref->>'sequence')::bigint=original THEN jsonb_set(jsonb_set(ref,'{sequence}',to_jsonb(replacement)),'{digest}',to_jsonb(pg_temp.jh(replacement))) ELSE ref END) FROM jsonb_array_elements(record_refs) ref),
   parent_sequence,generation,decision FROM eez.lifecycle_native_checks WHERE source_id=(SELECT source_id FROM outcomes_fixture) AND sequence=original;
END $$;
SELECT pg_temp.copy_terminal(11,12);
SELECT pg_temp.assert_admission('canonical:committed:node_reconciled','identical terminal observations remain one admission');
DO $$ BEGIN IF (SELECT terminal_claims FROM eez.lifecycle_canonical_outcomes WHERE source_id=(SELECT source_id FROM outcomes_fixture))<>2 THEN RAISE EXCEPTION 'duplicate terminal history was discarded'; END IF; END $$;
-- Unprojected claims immediately block stronger assurance; no cache is needed.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,13,1,'complete',jsonb_set(payload,'{event,outcome}','"source_reverted"') FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=11;
SELECT pg_temp.assert_admission('unresolved:null:unknown','late unprojected conflicting terminal retracts promotion');
DELETE FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=13;
SELECT pg_temp.assert_admission('canonical:committed:node_reconciled','quarantined/unretained conflict no longer selected');
-- Positively orphaned old watermark may retire a previous claim, but stale or
-- missing watermark evidence must never be used as an inferred orphan proof.
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage) VALUES(:l1,pg_temp.h(13),13,pg_temp.h(12),130,true,'complete');
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(jsonb_set(decision,'{l1_snapshot}',pg_temp.block(13,13)),'{elapsed_ns}','101') WHERE source_id=:source_id AND sequence=12;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,elapsed_ns}','101') WHERE source_id=:source_id AND sequence=12;
SELECT pg_temp.assert_admission('unresolved:null:unknown','conflicting active native decisions remain unknown');
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash IN (pg_temp.h(12),pg_temp.h(13));
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(112),12,pg_temp.h(11),120,true,'complete'),(:l1,pg_temp.h(113),13,pg_temp.h(112),130,true,'complete');
UPDATE eez.scan_cursors SET block_hash=pg_temp.h(113) WHERE chain_instance_id=:l1;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{l1_snapshot}',pg_temp.block(113,13)) WHERE source_id=:source_id AND sequence=12;
SELECT pg_temp.assert_admission('canonical:committed:node_reconciled','new valid claim replaces positively orphaned watermark');
UPDATE eez.chain_instances SET collection_error='transport' WHERE id=:l1;
SELECT pg_temp.assert_admission('unresolved:null:unknown','stale source cannot select even a previously verified claim');
ROLLBACK TO admission_variant;
DELETE FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=1;
DO $$ BEGIN
 IF EXISTS(SELECT FROM eez.lifecycle_admission_status WHERE instance='canonical-outcomes') THEN RAISE EXCEPTION 'terminal fabricated missing admission'; END IF;
 IF (SELECT evidence_class FROM eez.lifecycle_canonical_outcomes WHERE source_id=(SELECT source_id FROM outcomes_fixture))<>'node_reconciled' THEN RAISE EXCEPTION 'available terminal evidence should remain separate from missing admission'; END IF;
END $$;
ROLLBACK;
SELECT 'canonical outcome invariants passed';
