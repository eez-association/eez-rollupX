-- Synthetic logical-block boundary; no native throughput/capacity claim.
BEGIN;
SET LOCAL jit=off;
SET LOCAL statement_timeout='10s';
CREATE FUNCTION pg_temp.h(value bigint) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$ SELECT decode(lpad(to_hex(value),64,'0'),'hex') $$;
CREATE FUNCTION pg_temp.assert(ok boolean,label text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN IF ok IS DISTINCT FROM true THEN RAISE EXCEPTION 'derivable settlement range: %',label; END IF; END $$;
INSERT INTO eez.deployments(environment,name) VALUES('fixture','derivable-ranges') RETURNING id AS deployment \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l1',1,pg_temp.h(1),0,0,now(),jsonb_build_object('contracts',jsonb_build_array('0x'||repeat('ee',20)))) RETURNING id AS l1 \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at,selection)
 VALUES(:deployment,'l2',2,pg_temp.h(2),1,0,now(),'{"include_all_transactions":true}') RETURNING id AS l2 \gset
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(10),10,pg_temp.h(9),100,true,'complete');
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 SELECT :l2,pg_temp.h(1000000+n),n,CASE WHEN n=1 THEN pg_temp.h(2) ELSE pg_temp.h(999999+n) END,100+n,true,'complete' FROM generate_series(1,65536) n;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(100),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),0,2);
INSERT INTO eez.selected_transactions VALUES(:l1,pg_temp.h(100));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(100),pg_temp.h(10),0,true,true,201,1,0,0,201,0,0);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(100),'decoded',pg_temp.h(1100),now(),ARRAY[1]::eez.u64[]);
INSERT INTO eez.correlation_sources(l1_chain_id,l2_chain_id,last_success_at) VALUES(:l1,:l2,now());
INSERT INTO eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash,last_success_at,range_observed)
 VALUES(:l1,:l2,pg_temp.h(100),pg_temp.h(10),now(),true);
SAVEPOINT interrupted_collection;
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2)
 VALUES(:l1,:l2,pg_temp.h(100),pg_temp.h(10),0,65536,pg_temp.h(1200),true) RETURNING id AS partial \gset
INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash)
 SELECT :partial,n-1,n,pg_temp.h(1000000+n) FROM generate_series(1,1024) n;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.canonical_settled_l2_blocks WHERE settlement_id=:partial),'partial membership is not a verified range');
ROLLBACK TO interrupted_collection;
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.settlements WHERE l1_chain_id=:l1),'interrupted transaction leaves no partial claim');
INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2)
 VALUES(:l1,:l2,pg_temp.h(100),pg_temp.h(10),0,65536,pg_temp.h(1200),true) RETURNING id AS settled \gset
INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash)
 SELECT :settled,n-1,n,pg_temp.h(1000000+n) FROM generate_series(1,65536) n;
UPDATE eez.settlements SET range_verified_at=now() WHERE id=:settled;
SELECT pg_temp.assert((SELECT count(*)=65536 AND max(ordinal)=65535 FROM eez.settlement_blocks WHERE settlement_id=:settled),'full format ceiling and maximum ordinal accepted');
SELECT pg_temp.assert((SELECT count(*)=65536 FROM eez.canonical_settled_l2_blocks WHERE settlement_id=:settled),'exact full membership is canonical');
CREATE FUNCTION pg_temp.reject_invalid(s bigint,c1 bigint,c2 bigint) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    INSERT INTO eez.settlements(l1_chain_id,l2_chain_id,transaction_hash,block_hash,transaction_index,block_count,evidence_digest,reported_canonical_l2)
      VALUES(c1,c2,pg_temp.h(100),pg_temp.h(10),0,65537,pg_temp.h(1201),true);
    RAISE EXCEPTION 'accepted 65537 blocks';
  EXCEPTION WHEN check_violation THEN NULL; END;
  BEGIN
    INSERT INTO eez.settlement_blocks(settlement_id,ordinal,number,hash) VALUES(s,65536,65537,pg_temp.h(1065537));
    RAISE EXCEPTION 'accepted ordinal 65536';
  EXCEPTION WHEN check_violation THEN NULL; END;
END $$;
SELECT pg_temp.reject_invalid(:settled,:l1,:l2);
SAVEPOINT reorg;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l2 AND number=65536;
SELECT pg_temp.assert((SELECT count(*)=65535 FROM eez.canonical_settled_l2_blocks WHERE settlement_id=:settled),'L2 reorg retracts the exact orphaned member');
SELECT pg_temp.assert((SELECT count(*)=65536 FROM eez.settlement_blocks WHERE settlement_id=:settled),'L2 reorg preserves original immutable range evidence');
ROLLBACK TO reorg;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(10);
SELECT pg_temp.assert((SELECT count(*)=0 FROM eez.canonical_settled_l2_blocks WHERE settlement_id=:settled),'L1 reorg retracts the whole applied range');
SELECT pg_temp.assert((SELECT execution_paid=201 FROM eez.receipt_observations WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(100)),'historical receipt charge is never erased by range retraction');
ROLLBACK TO reorg;
SELECT pg_temp.assert((SELECT count(*)=65536 FROM eez.canonical_settled_l2_blocks WHERE settlement_id=:settled),'rollback restores exact canonical membership');
ROLLBACK;
SELECT 'derivable settlement range boundaries passed';
