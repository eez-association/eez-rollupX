\set ON_ERROR_STOP on
SET ROLE eez_observer_writer;
INSERT INTO eez.deployments(environment, name) VALUES ('fixture', 'ledger');
INSERT INTO eez.chain_instances(deployment_id, role, chain_id, genesis_hash, rollup_id, bootstrap_number)
  VALUES (1, 'l1', 1, decode(repeat('00',32),'hex'), 0, 1);
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage) VALUES
  (1,decode(repeat('01',32),'hex'),1,decode(repeat('00',32),'hex'),100,true,'complete'),
  (1,decode(repeat('02',32),'hex'),2,decode(repeat('01',32),'hex'),105,true,'complete');
INSERT INTO eez.scan_cursors(chain_instance_id,block_hash) VALUES (1,decode(repeat('02',32),'hex'));
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type) VALUES
  (1,decode(repeat('aa',32),'hex'),decode(repeat('ab',20),'hex'),decode(repeat('cd',20),'hex'),1,3),
  (1,decode(repeat('bb',32),'hex'),decode(repeat('ab',20),'hex'),NULL,2,2);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,
  gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs) VALUES
  (1,decode(repeat('aa',32),'hex'),decode(repeat('02',32),'hex'),0,false,false,21000,7,131072,3,147000,393216,1);
DO $$ BEGIN
  IF (SELECT known_total_paid FROM eez.canonical_cost_totals WHERE chain_instance_id=1) IS DISTINCT FROM 540216 THEN
    RAISE EXCEPTION 'canonical reverted receipt must incur its exact charge';
  END IF;
  BEGIN
    INSERT INTO eez.receipt_observations SELECT chain_instance_id,transaction_hash,decode(repeat('01',32),'hex'),transaction_index,
      success,true,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs,accounting_gap,observed_at
      FROM eez.receipt_observations;
    RAISE EXCEPTION 'two canonical inclusions of one logical transaction were accepted';
  EXCEPTION WHEN unique_violation THEN NULL; END;
END $$;

-- Duplicate input preserves facts and charges; changed complete facts are refused.
UPDATE eez.receipt_observations SET success=success;
DO $$ BEGIN
  BEGIN
    UPDATE eez.receipt_observations SET gas_used=gas_used+1, execution_paid=execution_paid+effective_gas_price;
    RAISE EXCEPTION 'complete receipt charge was silently overwritten';
  EXCEPTION WHEN integrity_constraint_violation THEN NULL; END;
END $$;

-- Same-height replacement and reinclusion retain both inclusions, charging one.
UPDATE eez.blocks SET canonical=false WHERE hash=decode(repeat('02',32),'hex');
DO $$ BEGIN
  IF EXISTS (SELECT FROM eez.canonical_receipts) THEN RAISE EXCEPTION 'orphan remained billable'; END IF;
END $$;
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
  VALUES (1,decode(repeat('03',32),'hex'),2,decode(repeat('01',32),'hex'),106,true,'complete');
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,
  gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs) VALUES
  (1,decode(repeat('aa',32),'hex'),decode(repeat('03',32),'hex'),0,true,true,22000,7,131072,4,154000,524288,1);
UPDATE eez.scan_cursors SET block_hash=decode(repeat('03',32),'hex');
DO $$ BEGIN
  IF (SELECT count(*) FROM eez.receipt_observations) IS DISTINCT FROM 2
    OR (SELECT count(*) FROM eez.canonical_receipts) IS DISTINCT FROM 1
    OR (SELECT known_total_paid FROM eez.canonical_cost_totals) IS DISTINCT FROM 678288 THEN
    RAISE EXCEPTION 'reinclusion did not preserve history with one canonical charge';
  END IF;
END $$;

-- A rollback must include both newly observed blocks and cursor advancement.
BEGIN;
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
  VALUES (1,decode(repeat('04',32),'hex'),3,decode(repeat('03',32),'hex'),110,true,'complete');
UPDATE eez.scan_cursors SET block_hash=decode(repeat('04',32),'hex');
ROLLBACK;
DO $$ BEGIN
  IF EXISTS (SELECT FROM eez.blocks WHERE hash=decode(repeat('04',32),'hex'))
     OR (SELECT block_hash FROM eez.scan_cursors) IS DISTINCT FROM decode(repeat('03',32),'hex') THEN
    RAISE EXCEPTION 'aborted cursor transaction leaked state';
  END IF;
END $$;

-- Missing fee fields remain a gap and may be repaired without losing inclusion.
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,accounting_gap)
  VALUES (1,decode(repeat('bb',32),'hex'),decode(repeat('03',32),'hex'),1,false,true,'gas_used_missing_or_invalid');
DO $$ BEGIN
  IF (SELECT incomplete_receipts FROM eez.canonical_cost_totals) IS DISTINCT FROM 1
     OR (SELECT known_total_paid FROM eez.canonical_cost_totals) IS DISTINCT FROM 678288 THEN
    RAISE EXCEPTION 'accounting gap was charged or hidden';
  END IF;
END $$;
UPDATE eez.receipt_observations SET gas_used=21000,effective_gas_price=1,blob_gas_used=0,blob_gas_price=0,
  execution_paid=21000,blob_paid=0,purchased_blobs=0,accounting_gap=NULL WHERE transaction_hash=decode(repeat('bb',32),'hex');

UPDATE eez.blocks SET finalized=true WHERE canonical;
DO $$ BEGIN
  BEGIN
    UPDATE eez.blocks SET canonical=false WHERE hash=decode(repeat('03',32),'hex');
    RAISE EXCEPTION 'finalized canonical anchor was retracted';
  EXCEPTION WHEN integrity_constraint_violation THEN NULL; END;
  BEGIN
    UPDATE eez.blocks SET finalized=false WHERE hash=decode(repeat('03',32),'hex');
    RAISE EXCEPTION 'finality was silently downgraded';
  EXCEPTION WHEN integrity_constraint_violation THEN NULL; END;
  BEGIN
    PERFORM '-1'::eez.u256;
    RAISE EXCEPTION 'negative unsigned amount accepted';
  EXCEPTION WHEN check_violation THEN NULL; END;
  BEGIN
    PERFORM 'NaN'::eez.u256;
    RAISE EXCEPTION 'NaN unsigned amount accepted';
  EXCEPTION WHEN check_violation THEN NULL; END;
  BEGIN
    PERFORM decode('00','hex')::eez.hash32;
    RAISE EXCEPTION 'truncated hash accepted';
  EXCEPTION WHEN check_violation THEN NULL; END;
END $$;

-- Sums exceed a single full-width product without rounding or overflow.
INSERT INTO eez.transactions(chain_instance_id,hash,sender,nonce,transaction_type) VALUES
  (1,decode(repeat('cc',32),'hex'),decode(repeat('ab',20),'hex'),3,0),
  (1,decode(repeat('dd',32),'hex'),decode(repeat('ab',20),'hex'),4,0);
WITH maximum AS (SELECT 115792089237316195423570985008687907853269984665640564039457584007913129639935::numeric AS n)
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,
  gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
  SELECT 1,decode(repeat(v.hash,32),'hex'),decode(repeat('03',32),'hex'),v.position,true,true,n,n,0,0,n*n,0,0
  FROM maximum CROSS JOIN (VALUES ('cc',2),('dd',3)) AS v(hash,position);
DO $$ BEGIN
  IF COALESCE((SELECT known_execution_paid FROM eez.canonical_cost_totals),0) <=
     13407807929942597099574024998205846127479365820592393377723561443721764030073546976801874298166903427690031858186486050853753882811946569946433649006084095::numeric THEN
    RAISE EXCEPTION 'sum was constrained to a single product width';
  END IF;
END $$;

RESET ROLE;
SET ROLE eez_observer_reader;
SELECT count(*) AS readable_canonical_receipts FROM eez.canonical_receipts;
SELECT count(*) AS readable_coverage FROM eez.index_coverage;
DO $$ BEGIN
  BEGIN
    INSERT INTO eez.deployments(environment,name) VALUES ('forbidden','reader');
    RAISE EXCEPTION 'viewer modified accounting';
  EXCEPTION WHEN insufficient_privilege THEN NULL; END;
  BEGIN
    PERFORM count(*) FROM eez.transactions;
    RAISE EXCEPTION 'viewer bypassed the approved views';
  EXCEPTION WHEN insufficient_privilege THEN NULL; END;
END $$;
RESET ROLE;
SELECT 'canonical ledger invariants passed' AS result;
