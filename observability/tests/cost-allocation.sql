-- SQL-only canonical attribution fixture. Cache/wire stubs are synthetic;
-- the separate real importer tests exercise typed membership and digest binding.
BEGIN;
SET LOCAL jit=off;
SET LOCAL statement_timeout='10s';
CREATE FUNCTION pg_temp.h(value bigint) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$ SELECT decode(lpad(to_hex(value),64,'0'),'hex') $$;
CREATE FUNCTION pg_temp.jh(value bigint) RETURNS text LANGUAGE sql IMMUTABLE AS $$ SELECT encode(pg_temp.h(value),'hex') $$;
CREATE FUNCTION pg_temp.block(value bigint,height bigint) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$ SELECT jsonb_build_object('number',height,'hash',pg_temp.jh(value)) $$;
CREATE FUNCTION pg_temp.assert(ok boolean,label text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN IF ok IS DISTINCT FROM true THEN RAISE EXCEPTION 'cost allocation: %',label; END IF; END $$;
INSERT INTO eez.deployments(environment,name) VALUES('fixture','cost-allocation') RETURNING id AS deployment \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l1',1,pg_temp.h(1),0,10,now(),jsonb_build_object('contracts',jsonb_build_array('0x'||repeat('ee',20)))) RETURNING id AS l1 \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at)
 VALUES(:deployment,'l2',2,pg_temp.h(2),1,0,now()) RETURNING id AS l2 \gset
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(11),11,pg_temp.h(10),110,true,'complete'),(:l1,pg_temp.h(12),12,pg_temp.h(11),120,true,'complete');
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 SELECT :l1,pg_temp.h(hash),decode(repeat('aa',20),'hex'),decode(repeat(recipient,20),'hex'),hash,3
 FROM (VALUES(5,'ee'),(6,'ee'),(1002,'bb')) tx(hash,recipient);
INSERT INTO eez.selected_transactions SELECT chain_instance_id,hash FROM eez.transactions WHERE chain_instance_id=:l1;
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(5),pg_temp.h(11),0,false,true,101,3,131072,7,303,917504,1),
       (:l1,pg_temp.h(1002),pg_temp.h(11),2,false,true,17,3,0,0,51,0,0),
       (:l1,pg_temp.h(6),pg_temp.h(11),1,false,true,101,3,131072,7,303,917504,1);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(5),'decoded',pg_temp.h(50),now(),ARRAY[1]::eez.u64[]),
       (:l1,pg_temp.h(6),'decoded',pg_temp.h(60),now(),ARRAY[1]::eez.u64[]);
INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory,last_success_at)
 VALUES(:deployment,'composer','cost-allocation','/cost-allocation',now()) RETURNING id AS source_id \gset
INSERT INTO eez.lifecycle_epochs(source_id,epoch,liveness,last_probe_at) VALUES(:source_id,decode(repeat('aa',16),'hex'),'live',now());
INSERT INTO eez.lifecycle_segments(source_id,epoch,segment_id) VALUES(:source_id,decode(repeat('aa',16),'hex'),decode(repeat('bb',16),'hex'));
CREATE TEMP TABLE costs_fixture AS SELECT :l1::bigint AS l1,:l2::bigint AS l2,:source_id::bigint AS source_id;
CREATE FUNCTION pg_temp.population() RETURNS jsonb LANGUAGE sql AS $$
 SELECT jsonb_build_array(jsonb_build_object('chain_id',1,'genesis_hash',pg_temp.jh(1),'rollup_id',0,'direction','inbound','transaction_hash',pg_temp.jh(1002)),
   jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1,'direction','outbound','transaction_hash',pg_temp.jh(2002)))
$$;
CREATE FUNCTION pg_temp.claim(seq bigint,post_hash bigint,population jsonb) RETURNS void LANGUAGE plpgsql AS $$
DECLARE f costs_fixture; ep bytea=decode(repeat('aa',16),'hex'); owner jsonb; facts jsonb;
BEGIN
 SELECT * INTO f FROM costs_fixture;
 owner=jsonb_build_object('epoch',encode(ep,'hex'),'generation',seq,'post_batch_hash',pg_temp.jh(post_hash));
 facts=jsonb_build_object('l1',jsonb_build_object('chain_id',1,'genesis_hash',pg_temp.jh(1),'rollup_id',0),
   'l2',jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1),
   'post',(SELECT jsonb_build_object('block',pg_temp.block(11,11),'transaction_index',receipt.transaction_index,'success',receipt.success)
     FROM eez.receipt_observations receipt WHERE receipt.chain_instance_id=f.l1 AND receipt.transaction_hash=pg_temp.h(post_hash)));
 INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload) VALUES
 (f.source_id,ep,seq-1,1,'complete',jsonb_build_object('event',jsonb_build_object('event','attempt_prepared','attempt',owner))),
 (f.source_id,ep,seq,1,'complete',jsonb_build_object('event',jsonb_build_object('event','reconciled','attempt',owner,'details',facts)));
 INSERT INTO eez.lifecycle_records(source_id,epoch,segment_id,offset_bytes,digest,raw)
 SELECT f.source_id,ep,decode(repeat('bb',16),'hex'),value,pg_temp.h(value),'\x7b7d' FROM (VALUES(seq-1),(seq)) refs(value);
 INSERT INTO eez.lifecycle_chunks(source_id,epoch,sequence,chunk_index,segment_id,offset_bytes,record,raw_digest)
 SELECT f.source_id,ep,value,0,decode(repeat('bb',16),'hex'),value,jsonb_build_object('complete',true,'digest',pg_temp.jh(value)),pg_temp.h(value) FROM (VALUES(seq-1),(seq)) refs(value);
 INSERT INTO eez.lifecycle_cost_checks(source_id,epoch,sequence,context_l1,context_l2,state,generation,record_refs,decision)
 SELECT f.source_id,ep,seq,f.l1,f.l2,'coherent',seq,
   (SELECT jsonb_agg(jsonb_build_object('epoch',encode(ep,'hex'),'sequence',value,'digest',pg_temp.jh(value))) FROM (VALUES(seq-1),(seq)) refs(value)),
   jsonb_build_object('owner',owner,'snapshot',pg_temp.block(12,12),'post',facts->'post','population',population,
     'sources',jsonb_build_array(jsonb_build_object('transaction_hash',pg_temp.jh(1002),'direction','inbound',
       'receipt',jsonb_build_object('block',pg_temp.block(11,11),'transaction_index',2,'success',false))));
END $$;
SELECT pg_temp.claim(10,5,pg_temp.population());
SET CONSTRAINTS ALL IMMEDIATE;
SELECT pg_temp.assert((SELECT count(*)=2 AND sum(allocated_execution_paid)=303 AND sum(allocated_blob_paid)=917504
 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'execution/blob exact sums and two distinct source shares');
SELECT pg_temp.assert((SELECT allocated_execution_paid=152 AND allocated_blob_paid=458752
 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1 AND allocation_position=1),'deterministic odd remainder');
SELECT pg_temp.assert((SELECT count(*)=1 AND sum(execution_paid+blob_paid)=51 AND bool_and(NOT success)
 FROM eez.companion_receipts WHERE chain_instance_id=:l1),'reverted companion charged once');
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.lifecycle_terminal_observations WHERE source_id=:source_id),'failed paid attempts need no terminal event');
SELECT pg_temp.claim(20,5,pg_temp.population());
SELECT pg_temp.assert((SELECT count(*)=2 AND sum(allocated_execution_paid+allocated_blob_paid)=917807
 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'duplicate native claim does not duplicate posting fee');
SELECT pg_temp.assert((SELECT count(*)=1 FROM eez.companion_receipts WHERE chain_instance_id=:l1),'duplicate claim does not duplicate companion');
-- Schema16: an importer can expose a new producer before any cost-check row.
SAVEPOINT claim_invalidation;
INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory,last_success_at)
 VALUES(:deployment,'composer','new-unchecked-producer','/new-unchecked-producer',now()) RETURNING id AS fresh_source \gset
INSERT INTO eez.lifecycle_epochs(source_id,epoch) VALUES(:fresh_source,decode(repeat('aa',16),'hex'));
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT :fresh_source,epoch,10,1,'complete',payload FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=10;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.lifecycle_cost_checks WHERE source_id=:fresh_source),'new producer has never been projected');
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'unchecked new producer immediately invalidates older population');
ROLLBACK TO claim_invalidation;

UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=10;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'quarantined parent cannot disappear behind another verified duplicate');
ROLLBACK TO claim_invalidation;

-- No coherent decision ever existed for this parent: identity must have been
-- retained at import, independently of the initial failed native check.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,30,1,'complete',payload FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=10;
INSERT INTO eez.lifecycle_cost_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs)
 VALUES(:source_id,decode(repeat('aa',16),'hex'),30,:l1,:l2,'missing_membership','[]');
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=30;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'initial gap then parent conflict preserves claimed post identity');
ROLLBACK TO claim_invalidation;

SELECT pg_temp.claim(30,5,pg_temp.population());
UPDATE eez.lifecycle_cost_checks SET state='missing_membership',generation=NULL,record_refs='[]',decision=NULL WHERE source_id=:source_id AND sequence=30;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=30;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'coherent to gap to quarantine cannot erase an active claim');
ROLLBACK TO claim_invalidation;

-- A competing reverted post can explicitly claim the same companion before
-- native checking. It blocks uniqueness without becoming an authorized owner.
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(13),13,pg_temp.h(12),130,true,'complete');
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,30,1,'complete',jsonb_set(jsonb_set(jsonb_set(jsonb_set(payload,
   '{event,attempt,post_batch_hash}',to_jsonb(pg_temp.jh(6))),'{event,details,post,transaction_index}','1'),
   '{event,anchor}',pg_temp.block(13,13)),
   '{event,details,sources}',jsonb_build_array(jsonb_build_object('transaction_hash',pg_temp.jh(1002),'direction','inbound',
      'receipt',jsonb_build_object('block',pg_temp.block(11,11),'transaction_index',2,'success',false))))
 FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=10;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.lifecycle_cost_checks WHERE source_id=:source_id AND sequence=30),'competing post is genuinely unchecked');
SELECT pg_temp.assert((SELECT count(*)=2 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1 AND post_hash=pg_temp.h(5)),'other post uncertainty does not duplicate or erase known posting shares');
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.companion_receipts WHERE chain_instance_id=:l1),'unchecked competing explicit companion claim blocks falsely unique attribution');
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=30;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.companion_receipts WHERE chain_instance_id=:l1),'quarantine retains competing companion uncertainty');
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(13);
SELECT pg_temp.assert((SELECT count(*)=1 FROM eez.companion_receipts WHERE chain_instance_id=:l1),'positive orphaning retires the competing claim');
ROLLBACK TO claim_invalidation;

SAVEPOINT variant;
SELECT pg_temp.claim(30,5,jsonb_build_array(pg_temp.population()->0));
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'contradictory population retracts allocation');
SELECT pg_temp.assert((SELECT sum(execution_paid+blob_paid)=1835614 FROM eez.posting_receipts WHERE chain_instance_id=:l1),'contradiction retains paid posting facts');
ROLLBACK TO variant;
SELECT pg_temp.claim(30,5,pg_temp.population());
DELETE FROM eez.lifecycle_cost_checks WHERE source_id=:source_id AND sequence=30;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'new unchecked claim immediately retracts cached allocation');
ROLLBACK TO variant;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=9;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'one changed digest dependency makes population unknown');
ROLLBACK TO variant;
UPDATE eez.chain_instances SET last_success_at=now()-interval '91 seconds' WHERE id=:l1;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'stale chain retracts estimates');
ROLLBACK TO variant;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(12);
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'orphaned watermark retracts population');
ROLLBACK TO variant;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(11);
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'orphaned paid post retracts estimates');
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.companion_receipts WHERE chain_instance_id=:l1),'orphaned post retracts companion attribution');
ROLLBACK TO variant;
SELECT pg_temp.claim(30,6,pg_temp.population());
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.companion_receipts WHERE chain_instance_id=:l1),'two reverted candidate posts cannot both own one source receipt');
SELECT pg_temp.assert((SELECT sum(allocated_execution_paid+allocated_blob_paid)=1835614 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'two actually paid posts each charged once');
ROLLBACK TO variant;
UPDATE eez.lifecycle_cost_checks SET decision=jsonb_set(jsonb_set(decision,'{population}','[]'),'{sources}','[]') WHERE source_id=:source_id;
SELECT pg_temp.assert((SELECT allocation_state='no_source_members' FROM eez.posting_allocation_status
 WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(5)),'pure L2 posting overhead remains unallocated');
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1),'empty population never divides by zero');
ROLLBACK TO variant;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(7),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),7,2);
INSERT INTO eez.selected_transactions VALUES(:l1,pg_temp.h(7));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(7),pg_temp.h(11),5,false,true,1,5,0,0,5,0,0);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(7),'decoded',pg_temp.h(70),now(),ARRAY[1,7]::eez.u64[]);
SELECT pg_temp.claim(30,7,pg_temp.population());
SELECT pg_temp.assert((SELECT allocation_state='shared_rollup_population_unknown' FROM eez.posting_allocation_status
 WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(7)),'one rollup cannot claim a shared multi-rollup fee');
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.source_posting_allocations WHERE l1_chain_id=:l1 AND post_hash=pg_temp.h(7)),'unknown other-rollup members cannot receive assumed zero');
ROLLBACK TO variant;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(8),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),8,2);
INSERT INTO eez.selected_transactions VALUES(:l1,pg_temp.h(8));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(8),pg_temp.h(11),6,false,true,1,5,0,0,5,0,0);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(8),'decoded',pg_temp.h(80),now(),ARRAY[7]::eez.u64[]);
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.posting_allocation_status
 WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(8)),'explicit other-rollup posts are outside this allocation population');
SELECT pg_temp.assert((SELECT execution_paid=5 FROM eez.posting_receipts
 WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(8)),'other-rollup paid receipt remains visible independently');
ROLLBACK TO variant;
-- Reader sees estimates and provenance but receives no write or raw table grant.
SET LOCAL ROLE eez_observer_reader;
SELECT count(*) FROM eez.source_posting_allocations;
SELECT count(*) FROM eez.companion_receipts;
RESET ROLE;
ROLLBACK;
