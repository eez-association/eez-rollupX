"""Production historical-fence and fair-retry SQL under actual PostgreSQL."""
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[2]


def fixture():
    ledger = (ROOT / 'crates/eez-observer/src/indexer/ledger.rs').read_text()
    settlement = (ROOT / 'crates/eez-observer/src/indexer/settlements.rs').read_text()
    lock = re.search(r'"(SELECT frozen_reason,canonical_revision FROM eez.chain_instances[^"]+)"', ledger).group(1)
    valid = re.search(r'"(SELECT EXISTS\(SELECT FROM eez.blocks anchor JOIN eez.scan_cursors[^"]+)"', ledger).group(1)
    schedule = re.search(r'const CANDIDATES_SQL: &str = "([^"]+)";', settlement).group(1)
    ranges = re.search(r'const VERIFY_RANGES_SQL: &str = "([^"]+)";', settlement).group(1)
    baseline = (ROOT / 'observability/tests/canonical-outcomes.sql').read_text().split(
        "SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');")[0]
    baseline = baseline.replace('pg_temp.h(601),true,now())', 'pg_temp.h(601),true,NULL)')
    return baseline + r'''
CREATE TEMP TABLE checkpoint_fixture AS SELECT :l1::bigint AS l1,:l2::bigint AS l2;
UPDATE eez.correlation_sources SET collection_error='rpc_transport' WHERE l1_chain_id=:l1 AND l2_chain_id=:l2;
''' + ranges.replace('$1', ':l1').replace('$2', ':l2') + r''';
DO $$ BEGIN
 IF NOT EXISTS(SELECT FROM eez.settlements WHERE l1_chain_id=(SELECT l1 FROM checkpoint_fixture) AND transaction_hash=pg_temp.h(6) AND range_verified_at IS NOT NULL) THEN
  RAISE EXCEPTION 'unrelated range validation did not progress';
 END IF;
 IF EXISTS(SELECT FROM eez.correlation_sources WHERE l1_chain_id=(SELECT l1 FROM checkpoint_fixture) AND collection_error IS NULL)
  OR EXISTS(SELECT FROM eez.canonical_settled_l2_blocks WHERE source_valid AND l1_chain_id=(SELECT l1 FROM checkpoint_fixture))
  OR EXISTS(SELECT FROM eez.settlement_finality WHERE l2_chain_id=(SELECT l2 FROM checkpoint_fixture)) THEN
  RAISE EXCEPTION 'partial collection was reported healthy or advanced finality';
 END IF;
END $$;
UPDATE eez.correlation_sources SET collection_error=NULL WHERE l1_chain_id=:l1 AND l2_chain_id=:l2;
CREATE FUNCTION pg_temp.history_state(chain_id bigint,revision bigint,anchor bytea,height text)
 RETURNS text LANGUAGE plpgsql AS $$ DECLARE state record; valid boolean; BEGIN
 EXECUTE $lock$''' + lock + r'''$lock$ INTO state USING chain_id;
 IF state.frozen_reason IS NOT NULL THEN RETURN 'frozen'; END IF;
 IF state.canonical_revision<>revision THEN RETURN 'changed_revision'; END IF;
 EXECUTE $valid$''' + valid + r'''$valid$ INTO valid USING chain_id,anchor,height;
 RETURN CASE WHEN valid THEN 'valid' ELSE 'invalid_anchor' END;
END $$;
CREATE FUNCTION pg_temp.assert_history(expected text,revision bigint,anchor bigint,height text)
 RETURNS void LANGUAGE plpgsql AS $$ DECLARE actual text; BEGIN
 SELECT pg_temp.history_state(l1,revision,pg_temp.h(anchor),height) INTO actual FROM checkpoint_fixture;
 IF actual<>expected THEN RAISE EXCEPTION 'historical fence expected %, got %',expected,actual; END IF;
END $$;
SELECT pg_temp.assert_history('valid',0,12,'12');
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l1,pg_temp.h(13),13,pg_temp.h(12),130,true,'complete');
UPDATE eez.scan_cursors SET block_hash=pg_temp.h(13) WHERE chain_instance_id=:l1;
-- Baseline exact-tip fencing rejects this unrelated append; the historical
-- fence still accepts the exact originally observed canonical hash/revision.
DO $$ BEGIN
 IF (SELECT block_hash=pg_temp.h(12) FROM eez.scan_cursors WHERE chain_instance_id=(SELECT l1 FROM checkpoint_fixture)) THEN
  RAISE EXCEPTION 'baseline exact cursor comparison should reject append';
 END IF;
END $$;
SELECT pg_temp.assert_history('valid',0,12,'12');
SELECT pg_temp.assert_history('invalid_anchor',0,12,'11');
DO $$ BEGIN
 IF (SELECT pg_temp.history_state(l2,0,pg_temp.h(12),'12') FROM checkpoint_fixture)<>'invalid_anchor' THEN
  RAISE EXCEPTION 'another chain context accepted historical anchor';
 END IF;
END $$;
-- A reorg above the anchor still changes the revision and invalidates in-flight
-- evidence. Restoring the same hash does not restore the old checkpoint token.
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(13);
UPDATE eez.scan_cursors SET block_hash=pg_temp.h(12) WHERE chain_instance_id=:l1;
SELECT pg_temp.assert_history('changed_revision',0,12,'12');
UPDATE eez.blocks SET canonical=true WHERE chain_instance_id=:l1 AND hash=pg_temp.h(13);
UPDATE eez.scan_cursors SET block_hash=pg_temp.h(13) WHERE chain_instance_id=:l1;
SELECT pg_temp.assert_history('changed_revision',0,12,'12');
SELECT pg_temp.assert_history('valid',1,12,'12');
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(12);
SELECT pg_temp.assert_history('invalid_anchor',2,12,'12');
UPDATE eez.blocks SET canonical=true WHERE chain_instance_id=:l1 AND hash=pg_temp.h(12);
UPDATE eez.chain_instances SET frozen_reason='conflicting_fact' WHERE id=:l1;
SELECT pg_temp.assert_history('frozen',2,12,'12');
UPDATE eez.chain_instances SET frozen_reason=NULL WHERE id=:l1;
UPDATE eez.scan_cursors SET block_hash=pg_temp.h(11) WHERE chain_instance_id=:l1;
SELECT pg_temp.assert_history('invalid_anchor',2,12,'12');
UPDATE eez.scan_cursors SET block_hash=pg_temp.h(13) WHERE chain_instance_id=:l1;

INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 SELECT :l1,pg_temp.h(n),n,pg_temp.h(n-1),n*10,true,'complete' FROM generate_series(20,30) n;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 SELECT :l1,pg_temp.h(n+10000),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),n,2 FROM generate_series(20,30) n;
INSERT INTO eez.selected_transactions SELECT chain_instance_id,hash FROM eez.transactions WHERE chain_instance_id=:l1 ON CONFLICT DO NOTHING;
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 SELECT :l1,pg_temp.h(n+10000),pg_temp.h(n),0,true,true,100,2,0,0,200,0,0 FROM generate_series(20,30) n;
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,verification_rollups,observed_at)
 SELECT :l1,pg_temp.h(n+10000),'decoded',pg_temp.h(n+20000),ARRAY[1]::eez.u64[],now() FROM generate_series(20,30) n;
INSERT INTO eez.correlation_runs(l1_chain_id,l2_chain_id,transaction_hash,block_hash,last_attempt_at,range_observed,collection_error)
 VALUES(:l1,:l2,pg_temp.h(10020),pg_temp.h(20),now(),false,'interrupted');
CREATE FUNCTION pg_temp.schedule(l1 bigint,l2 bigint) RETURNS TABLE(block_number text,block_hash bytea) LANGUAGE plpgsql AS $$ BEGIN
 RETURN QUERY EXECUTE $schedule$''' + schedule + r'''$schedule$ USING l1,l2;
END $$;
''' + r'''
DO $$ DECLARE chosen bigint[]; BEGIN
 SELECT array_agg(block_number::bigint) INTO chosen FROM pg_temp.schedule((SELECT l1 FROM checkpoint_fixture),(SELECT l2 FROM checkpoint_fixture));
 IF cardinality(chosen)<>4 OR NOT chosen @> ARRAY[11,20,21,22]::bigint[] THEN
  RAISE EXCEPTION 'bounded retry/new/complete lanes failed: %',chosen;
 END IF;
 -- Repeated failed attempts remain selectable even with continuous new posts.
 UPDATE eez.correlation_runs SET last_attempt_at=now()+interval '1 second' WHERE transaction_hash=pg_temp.h(10020);
 SELECT array_agg(block_number::bigint) INTO chosen FROM pg_temp.schedule((SELECT l1 FROM checkpoint_fixture),(SELECT l2 FROM checkpoint_fixture));
 IF NOT chosen @> ARRAY[20,21,22]::bigint[] THEN RAISE EXCEPTION 'retry or discovery starved: %',chosen; END IF;
END $$;
SELECT 'historical checkpoint and retry invariants passed';
ROLLBACK;
'''


def exercise(run, psql, report):
    result = run(psql, data=fixture())
    assert 'historical checkpoint and retry invariants passed' in result.stdout
    report['checks'].append('historical_enrichment_accepts_append_rejects_reorg_restoration_freeze_wrong_anchor_and_retries_fairly')
