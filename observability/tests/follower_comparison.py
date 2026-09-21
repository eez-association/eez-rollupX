"""Run the actual Rust follower target SELECT against canonical SQL fixtures.

Only target selection is tested here. Header/provenance verification has private
HTTP tests; actual STF capture/restart is tested with the native follower.
"""
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]


def exercise(run, psql, report):
    source = (ROOT.parent / 'crates/eez-observer/src/indexer/follower.rs').read_text()
    query, = re.findall(r'const TARGET_SQL: &str = "([^"]+)";', source)
    query = query.replace('$1', ':l1').replace('$2', ':l2').replace('$3', "decode(repeat('ee',20),'hex')")
    baseline = (ROOT / 'tests/canonical-outcomes.sql').read_text().split('INSERT INTO eez.protocol_log_claims', 1)[0]
    assertions = """
DO $$ BEGIN
 IF (SELECT count(*) FROM follower_target)<>1 OR
    (SELECT tx_hash FROM follower_target)<>pg_temp.h(6) OR
    (SELECT block_hash FROM follower_target)<>pg_temp.h(33) OR
    (SELECT l2_number FROM follower_target)<>'3' OR
    NOT(SELECT source_valid FROM follower_target) THEN
   RAISE EXCEPTION 'follower must select exact current Q, not superseded same-height P';
 END IF;
 IF EXISTS(SELECT FROM follower_wrong_scope) THEN RAISE EXCEPTION 'follower EEZ scope crossed'; END IF;
END $$;
UPDATE eez.chain_instances SET last_success_at=now()-interval '91 seconds' WHERE id=:l1;
DO $$ BEGIN
 IF (SELECT count(*) FROM follower_target)<>1 OR (SELECT source_valid FROM follower_target) THEN
  RAISE EXCEPTION 'stale newest source must remain an explicit invalid target, not silently fall back';
 END IF;
END $$;
UPDATE eez.chain_instances SET last_success_at=now() WHERE id=:l1;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l2 AND hash=pg_temp.h(33);
DO $$ BEGIN IF EXISTS(SELECT FROM follower_target) THEN RAISE EXCEPTION 'reorged target still compared'; END IF; END $$;
SELECT 'follower target invariants passed';
ROLLBACK;
"""
    columns = '(l1_number,l1_hash,tx_hash,tx_index,l2_number,block_hash,source_valid)'
    sql = baseline + '\nCREATE TEMP VIEW follower_target' + columns + ' AS ' + query + ';\n'
    sql += 'CREATE TEMP VIEW follower_wrong_scope' + columns + ' AS ' + query.replace("repeat('ee',20)", "repeat('bb',20)") + ';\n'
    result = run(psql, data=sql + assertions)
    assert 'follower target invariants passed' in result.stdout
    report['checks'].append('follower_actual_target_query_exact_same_height_scope_stale_and_reorg')
