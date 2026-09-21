-- Synthetic header facts exercise the reader view and invariants. The scanner
-- process fixture separately checks that metadata is populated from actual RPC.
BEGIN;
SET LOCAL jit=off;
SET LOCAL statement_timeout='10s';
CREATE FUNCTION pg_temp.h(value bigint) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$ SELECT decode(lpad(to_hex(value),64,'0'),'hex') $$;
INSERT INTO eez.deployments(environment,name) VALUES('fixture','block-activity') RETURNING id AS deployment \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at)
 VALUES(:deployment,'l2',2,pg_temp.h(2),1,0,now()) RETURNING id AS chain \gset
CREATE TEMP TABLE activity_fixture AS SELECT :chain::bigint AS chain;
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:chain,pg_temp.h(1),1,pg_temp.h(0),100,true,'complete');
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage,transaction_count,gas_used,gas_limit,blob_gas_used,excess_blob_gas)
 VALUES(:chain,pg_temp.h(2),2,pg_temp.h(1),101,true,'gap',0,0,30000000,0,0),
       (:chain,pg_temp.h(3),3,pg_temp.h(2),103,true,'complete',2,21000,30000000,262144,131072),
       (:chain,pg_temp.h(5),5,pg_temp.h(4),105,true,'complete',1,21000,30000000,NULL,NULL),
       (:chain,pg_temp.h(6),6,pg_temp.h(5),104,true,'complete',0,0,0,NULL,NULL);
GRANT SELECT ON activity_fixture TO eez_observer_reader;
SET LOCAL ROLE eez_observer_reader;
DO $$ BEGIN
 IF (SELECT count(*) FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture))<>5 THEN RAISE EXCEPTION 'wrong canonical population'; END IF;
 IF NOT EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND block_number=1 AND transaction_count IS NULL AND gas_used IS NULL AND gas_limit IS NULL AND blob_gas_used IS NULL AND excess_blob_gas IS NULL AND interval_seconds IS NULL AND source_valid) THEN RAISE EXCEPTION 'old unknown header fabricated measurements'; END IF;
 IF NOT EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND block_number=2 AND transaction_count=0 AND gas_used=0 AND blob_gas_used=0 AND interval_seconds=1 AND source_valid) THEN RAISE EXCEPTION 'known zero or receipt-independent header lost'; END IF;
 IF NOT EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND block_number=3 AND transaction_count=2 AND interval_seconds=2 AND blob_gas_used/131072=2 AND excess_blob_gas=131072) THEN RAISE EXCEPTION 'known activity or interval incorrect'; END IF;
 IF EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND block_number IN (5,6) AND interval_seconds IS NOT NULL) THEN RAISE EXCEPTION 'gap or backwards timestamp fabricated cadence'; END IF;
 IF (SELECT count(*) FILTER(WHERE transaction_count=0)::numeric/count(transaction_count) FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture))<>0.5 THEN RAISE EXCEPTION 'unknown transaction count included in empty denominator'; END IF;
END $$;
RESET ROLE;
-- Known facts cannot change or disappear, but unknown columns can be filled.
UPDATE eez.blocks SET transaction_count=1 WHERE chain_instance_id=:chain AND hash=pg_temp.h(1);
DO $$ DECLARE col text; BEGIN
 FOREACH col IN ARRAY ARRAY['transaction_count','gas_used','gas_limit','blob_gas_used','excess_blob_gas'] LOOP
  BEGIN
   EXECUTE format('UPDATE eez.blocks SET %I=NULL WHERE chain_instance_id=$1 AND hash=pg_temp.h(3)',col) USING (SELECT chain FROM activity_fixture);
   RAISE EXCEPTION 'known % unexpectedly erased',col;
  EXCEPTION WHEN integrity_constraint_violation THEN NULL;
  END;
 END LOOP;
 BEGIN
  UPDATE eez.blocks SET transaction_count=3 WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND hash=pg_temp.h(3);
  RAISE EXCEPTION 'known transaction count changed';
 EXCEPTION WHEN integrity_constraint_violation THEN NULL;
 END;
 BEGIN
  INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage,gas_used,gas_limit)
   VALUES((SELECT chain FROM activity_fixture),pg_temp.h(7),7,pg_temp.h(6),106,true,'complete',2,1);
  RAISE EXCEPTION 'gas above limit accepted';
 EXCEPTION WHEN check_violation THEN NULL;
 END;
 BEGIN
  INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage,blob_gas_used)
   VALUES((SELECT chain FROM activity_fixture),pg_temp.h(7),7,pg_temp.h(6),106,true,'complete',1);
  RAISE EXCEPTION 'fractional blob gas accepted';
 EXCEPTION WHEN check_violation THEN NULL;
 END;
END $$;
SAVEPOINT healthy;
UPDATE eez.chain_instances SET last_success_at=now()-interval '91 seconds' WHERE id=:chain;
DO $$ BEGIN IF EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND source_valid) THEN RAISE EXCEPTION 'stale headers reported current'; END IF; END $$;
ROLLBACK TO healthy;
UPDATE eez.chain_instances SET collection_error='fixture' WHERE id=:chain;
DO $$ BEGIN IF EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND source_valid) THEN RAISE EXCEPTION 'failed headers reported current'; END IF; END $$;
ROLLBACK TO healthy;
UPDATE eez.chain_instances SET frozen_reason='conflicting_fact' WHERE id=:chain;
DO $$ BEGIN IF EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND source_valid) THEN RAISE EXCEPTION 'frozen headers reported current'; END IF; END $$;
ROLLBACK TO healthy;
SELECT id FROM eez.chain_instances WHERE id=:chain FOR UPDATE;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:chain AND hash=pg_temp.h(3);
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage,transaction_count)
 VALUES(:chain,pg_temp.h(33),3,pg_temp.h(2),104,true,'complete',3);
DO $$ BEGIN
 IF EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND block_hash=pg_temp.h(3)) OR
 NOT EXISTS(SELECT FROM eez.canonical_block_activity WHERE chain_instance_id=(SELECT chain FROM activity_fixture) AND block_number=3 AND block_hash=pg_temp.h(33) AND transaction_count=3 AND interval_seconds=3) THEN RAISE EXCEPTION 'reorg failed to replace old observations'; END IF;
END $$;
ROLLBACK;
SELECT 'block activity invariants passed';
