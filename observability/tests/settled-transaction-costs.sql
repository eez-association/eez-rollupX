BEGIN;
SET LOCAL jit=off;
SET LOCAL statement_timeout='10s';
CREATE FUNCTION pg_temp.h(value bigint) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$ SELECT decode(lpad(to_hex(value),64,'0'),'hex') $$;
CREATE FUNCTION pg_temp.assert(ok boolean,label text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN IF ok IS DISTINCT FROM true THEN RAISE EXCEPTION 'settled transaction cost: %',label; END IF; END $$;
INSERT INTO eez.deployments(environment,name) VALUES('fixture','settled-cost') RETURNING id AS deployment \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l1',1,pg_temp.h(1),0,0,now(),jsonb_build_object('contracts',jsonb_build_array('0x'||repeat('ee',20)))) RETURNING id AS l1 \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l2',2,pg_temp.h(2),1,0,now(),'{"include_all_transactions":true}') RETURNING id AS l2 \gset
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(10),10,pg_temp.h(9),100,true,'complete'),
       (:l2,pg_temp.h(21),1,pg_temp.h(2),90,true,'complete'),
       (:l2,pg_temp.h(22),2,pg_temp.h(21),95,true,'complete');
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 SELECT :l1,pg_temp.h(hash),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),hash,2 FROM (VALUES(100),(101),(102)) t(hash);
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 SELECT :l2,pg_temp.h(hash),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),hash,2 FROM (VALUES(200),(201),(202)) t(hash);
INSERT INTO eez.selected_transactions SELECT chain_instance_id,hash FROM eez.transactions WHERE chain_instance_id IN (:l1,:l2);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 SELECT :l1,pg_temp.h(hash),pg_temp.h(10),idx,success,true,fee,1,0,0,fee,0,0 FROM (VALUES(100,0,true,201),(101,1,true,300),(102,2,false,50)) t(hash,idx,success,fee);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 SELECT :l2,pg_temp.h(hash),pg_temp.h(block),idx,success,true,10,1,0,0,10,0,0 FROM (VALUES(200,21,0,true),(201,21,1,false),(202,22,0,true)) t(hash,block,idx,success);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 SELECT :l1,pg_temp.h(hash),'decoded',pg_temp.h(hash+1000),now(),ARRAY[1]::eez.u64[] FROM (VALUES(100),(101),(102)) t(hash);
INSERT INTO eez.correlation_sources(l1_chain_id,l2_chain_id,last_success_at) VALUES(:l1,:l2,now());
INSERT INTO eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash,last_success_at,range_observed)
 SELECT :l1,:l2,pg_temp.h(hash),pg_temp.h(10),now(),true FROM (VALUES(100),(101)) t(hash);
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2,range_verified_at)
 VALUES(:l1,:l2,pg_temp.h(100),pg_temp.h(10),0,1,pg_temp.h(1100),true,now()) RETURNING id AS first_post \gset
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2,range_verified_at)
 VALUES(:l1,:l2,pg_temp.h(101),pg_temp.h(10),1,2,pg_temp.h(1101),true,now()) RETURNING id AS second_post \gset
INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash)
 VALUES(:first_post,0,1,pg_temp.h(21)),(:second_post,0,1,pg_temp.h(21)),(:second_post,1,2,pg_temp.h(22));
SELECT pg_temp.assert((SELECT settled_transactions=2 AND posting_wei_per_settled_transaction_floor=100 FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND transaction_hash=pg_temp.h(100)),'first batch counts success and reverted L2 transactions, exact floor');
SELECT pg_temp.assert((SELECT settled_transactions=1 AND posting_wei_per_settled_transaction_floor=300 FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND transaction_hash=pg_temp.h(101)),'overlapping applied range counts each transaction once');
SELECT pg_temp.assert((SELECT settled_transactions=0 AND execution_paid=50 AND posting_wei_per_settled_transaction_floor IS NULL FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND transaction_hash=pg_temp.h(102)),'reverted post retains spend without a fabricated denominator');
SAVEPOINT variant;
UPDATE eez.blocks SET receipt_coverage='gap' WHERE chain_instance_id=:l2 AND hash=pg_temp.h(21);
SELECT pg_temp.assert((SELECT bool_and(settled_transactions IS NULL) FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND success),'receipt gap suppresses incomplete transaction populations');
ROLLBACK TO variant;
UPDATE eez.chain_instances SET last_success_at=now()-interval '91 seconds' WHERE id=:l2;
SELECT pg_temp.assert((SELECT bool_and(denominator_status='stale_source' AND settled_transactions IS NULL) FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1),'stale chain means unknown');
ROLLBACK TO variant;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l2 AND hash=pg_temp.h(21);
SELECT pg_temp.assert((SELECT bool_and(settled_transactions IS NULL) FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND success),'same-height replacement cannot substitute for exact applied hash');
ROLLBACK TO variant;
-- A missing historical prefix cannot make all of Q newly settled.
DELETE FROM eez.settlement_blocks WHERE settlement_id=:first_post;
INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash) VALUES(:first_post,0,1,pg_temp.h(29));
SELECT pg_temp.assert((SELECT denominator_status='prior_applied_range_unknown' AND settled_transactions IS NULL FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND transaction_hash=pg_temp.h(101)),'historical P then Q remains unknown rather than double-counted');
ROLLBACK TO variant;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(10);
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1),'L1 reorg retracts both costs and denominators');
ROLLBACK TO variant;
SELECT pg_temp.assert((SELECT sum(settled_transactions)=3 AND sum(execution_paid)=551 FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1),'repeat query preserves exact receipt totals and deduplicated population');
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2,range_verified_at)
 VALUES(:l1,:l2,pg_temp.h(100),pg_temp.h(10),0,1,pg_temp.h(1199),true,now());
SELECT pg_temp.assert((SELECT bool_and(settled_transactions IS NULL) FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND success),'conflicting range claims invalidate this and subsequent denominators');
ROLLBACK TO variant;
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(9),9,pg_temp.h(8),80,true,'gap');
SELECT pg_temp.assert((SELECT bool_and(denominator_status='prior_receipt_gap') FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND success),'missing prior receipts cannot prove new settlement');
ROLLBACK TO variant;
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(9),9,pg_temp.h(8),80,true,'complete');
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(99),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),99,2);
INSERT INTO eez.selected_transactions VALUES(:l1,pg_temp.h(99));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(99),pg_temp.h(9),0,true,true,10,1,0,0,10,0,0);
SELECT pg_temp.assert((SELECT bool_and(denominator_status='prior_application_unknown') FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND success),'unclassified earlier contract call prevents false new-transaction counts');
ROLLBACK TO variant;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(103),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),103,2);
INSERT INTO eez.selected_transactions VALUES(:l1,pg_temp.h(103));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(103),pg_temp.h(10),3,true,true,10,1,0,0,10,0,0);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(103),'decoded',pg_temp.h(1103),now(),ARRAY[1,2]::eez.u64[]);
SELECT pg_temp.assert((SELECT denominator_status='shared_multi_rollup_fee' AND settled_transactions IS NULL FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND transaction_hash=pg_temp.h(103)),'shared multi-rollup fee has no fabricated per-rollup unit cost');
ROLLBACK TO variant;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(103),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),103,2);
INSERT INTO eez.selected_transactions VALUES(:l1,pg_temp.h(103));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(103),pg_temp.h(10),3,true,true,10,1,0,0,10,0,0);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(103),'decoded',pg_temp.h(1103),now(),ARRAY[1]::eez.u64[]);
INSERT INTO eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash,last_success_at,range_observed)
 VALUES(:l1,:l2,pg_temp.h(103),pg_temp.h(10),now(),true);
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2,range_verified_at)
 VALUES(:l1,:l2,pg_temp.h(103),pg_temp.h(10),3,1,pg_temp.h(1103),true,now()) RETURNING id AS repeat_post \gset
INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash) VALUES(:repeat_post,0,2,pg_temp.h(22));
SELECT pg_temp.assert((SELECT denominator_status='verified' AND settled_transactions=0 AND execution_paid=10 AND posting_wei_per_settled_transaction_floor IS NULL FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND transaction_hash=pg_temp.h(103)),'successful repeated range has known zero new transactions and no division by zero');
ROLLBACK TO variant;
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l2',3,pg_temp.h(3),1,0,now(),'{}') RETURNING id AS selective_l2 \gset
INSERT INTO eez.correlation_sources(l1_chain_id,l2_chain_id,last_success_at) VALUES(:l1,:selective_l2,now());
SELECT pg_temp.assert((SELECT bool_and(denominator_status='selective_l2_scan' AND settled_transactions IS NULL) FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND l2_chain_id=:selective_l2 AND success),'selective receipt scan cannot count all settled transactions');
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l2',4,pg_temp.h(4),1,1,now(),'{"include_all_transactions":true}') RETURNING id AS late_l2 \gset
INSERT INTO eez.correlation_sources(l1_chain_id,l2_chain_id,last_success_at) VALUES(:l1,:late_l2,now());
SELECT pg_temp.assert((SELECT bool_and(denominator_status='prior_history_unavailable' AND settled_transactions IS NULL) FROM eez.settled_transaction_costs WHERE l1_chain_id=:l1 AND l2_chain_id=:late_l2 AND success),'bootstrap gap is explicit rather than counting old transactions as new');
ROLLBACK;
