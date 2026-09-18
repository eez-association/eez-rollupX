"""Actual canonical snapshot SQL against transactional PostgreSQL fixtures.

Synthetic native-coherence cache entries reuse canonical-outcomes.sql; this does
not establish importer correctness or independent protocol replay. Rust tests
exercise published rendering fences; this fixture verifies their SQL inputs.
"""
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]


def fixture():
    source = (ROOT.parent / 'crates/eez-observer/src/indexer/outcomes.rs').read_text()
    snapshot, = re.findall(
        r'"(WITH outcomes AS \(SELECT \* FROM eez\.lifecycle_canonical_outcomes[^"\n]+)"', source)
    seed, _ = (ROOT / 'tests/canonical-outcomes.sql').read_text().split(
        "SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');", 1)
    sql = seed + r'''
-- The production query itself supplies all nine values in one SQL snapshot.
CREATE FUNCTION pg_temp.progress_snapshot(l1 bigint,l2 bigint)
 RETURNS TABLE(valid boolean,l1_cursor bytea,l2_cursor bytea,unresolved bigint,progress jsonb,
               l1_height text,l2_height text,l1_revision bigint,l2_revision bigint)
 LANGUAGE plpgsql AS $$ BEGIN
 RETURN QUERY EXECUTE $snapshot$''' + snapshot + r'''$snapshot$ USING l1,l2;
END $$;
GRANT SELECT ON outcomes_fixture TO eez_observer_writer;
CREATE FUNCTION pg_temp.assert_progress(label text, expected jsonb)
 RETURNS void LANGUAGE plpgsql AS $$ DECLARE actual jsonb; BEGIN
 SELECT to_jsonb(snapshot) INTO STRICT actual FROM outcomes_fixture context,
   LATERAL pg_temp.progress_snapshot(context.l1,context.l2) snapshot;
 IF NOT actual @> expected
    OR (expected ? 'progress' AND actual->'progress' IS DISTINCT FROM expected->'progress') THEN
  RAISE EXCEPTION '%: expected %, actual %',label,expected,actual;
 END IF;
END $$;
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_progress('baseline complete same-statement fences',jsonb_build_object(
 'valid',true,'l1_cursor',pg_temp.h(12),'l2_cursor',pg_temp.h(33),
 'l1_height','12','l2_height','3','l1_revision',0,'l2_revision',0,'unresolved',0,
 'progress',jsonb_build_array(jsonb_build_object('direction','inbound','outcome','committed','count',1,'timestamp',120))));
RESET ROLE;
SAVEPOINT baseline;
'''
    for chain, old_hash, old_height, new_hash, new_height in (
            ('l1', 12, 12, 13, 13), ('l2', 33, 3, 34, 4)):
        sql += f'''
-- Ordinary append retains the reorg token and every prior verified admission.
SELECT id FROM eez.chain_instances WHERE id=:{chain} FOR UPDATE;
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:{chain},pg_temp.h({new_hash}),{new_height},pg_temp.h({old_hash}),130,true,'complete');
UPDATE eez.scan_cursors SET block_hash=pg_temp.h({new_hash}) WHERE chain_instance_id=:{chain};
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_progress('{chain} append',jsonb_build_object(
 '{chain}_cursor',pg_temp.h({new_hash}),'{chain}_height','{new_height}','{chain}_revision',0,
 'valid',true,'unresolved',0,'progress',jsonb_build_array(jsonb_build_object(
 'direction','inbound','outcome','committed','count',1,'timestamp',120))));
RESET ROLE;
-- Even restoring precisely the original sampled cursor cannot restore revision0.
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:{chain} AND hash=pg_temp.h({new_hash});
UPDATE eez.scan_cursors SET block_hash=pg_temp.h({old_hash}) WHERE chain_instance_id=:{chain};
SELECT pg_temp.assert_progress('{chain} reorg restored original cursor',jsonb_build_object(
 '{chain}_cursor',pg_temp.h({old_hash}),'{chain}_height','{old_height}','{chain}_revision',1));
UPDATE eez.blocks SET canonical=true WHERE chain_instance_id=:{chain} AND hash=pg_temp.h({new_hash});
UPDATE eez.scan_cursors SET block_hash=pg_temp.h({new_hash}) WHERE chain_instance_id=:{chain};
SELECT pg_temp.assert_progress('{chain} reorg restored appended cursor',jsonb_build_object(
 '{chain}_cursor',pg_temp.h({new_hash}),'{chain}_height','{new_height}','{chain}_revision',1));
ROLLBACK TO baseline;
SELECT pg_temp.assert_progress('{chain} rollback restores token','{{"l1_revision":0,"l2_revision":0}}');
-- The LEFT JOIN must not fabricate a height for an absent/orphaned cursor.
DELETE FROM eez.scan_cursors WHERE chain_instance_id=:{chain};
SELECT pg_temp.assert_progress('{chain} missing cursor','{{"{chain}_cursor":null,"{chain}_height":null}}');
ROLLBACK TO baseline;
SELECT id FROM eez.chain_instances WHERE id=:{chain} FOR UPDATE;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:{chain} AND hash=pg_temp.h({old_hash});
SELECT pg_temp.assert_progress('{chain} orphaned cursor','{{"{chain}_height":null,"{chain}_revision":1}}');
ROLLBACK TO baseline;
'''
    for source_name, table, condition in (
            ('l1', 'chain_instances', 'id=:l1'),
            ('l2', 'chain_instances', 'id=:l2'),
            ('correlation', 'correlation_sources', 'l1_chain_id=:l1 AND l2_chain_id=:l2')):
        for label, assignment in (
                ('frozen', "frozen_reason='conflicting_fact'"),
                ('error', "collection_error='fixture'"),
                ('stale', "last_success_at=now()-interval '91 seconds'"),
                ('future', "last_success_at=now()+interval '6 seconds'"),
                ('missing freshness', 'last_success_at=NULL')):
            if source_name == 'correlation' and label == 'frozen':
                assignment = "frozen_reason='l2_finalized_contradiction'"
            sql += f"""
UPDATE eez.{table} SET {assignment} WHERE {condition};
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_progress('{source_name} {label}','{{"valid":false}}');
RESET ROLE;
ROLLBACK TO baseline;
"""
    sql += r'''
-- An imported but unprojected contradictory terminal suppresses completion,
-- without altering chain revisions. This remains a bounded refresh, not an
-- immediate lifecycle-change fence on every Prometheus scrape.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,100002,1,'complete',jsonb_set(payload,'{event,outcome}','"source_reverted"')
 FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=11;
SELECT pg_temp.assert_progress('late contradictory claim','{"valid":true,"unresolved":1,"progress":[],"l1_revision":0,"l2_revision":0}');
ROLLBACK TO baseline;
SELECT pg_temp.assert_progress('restored canonical count','{"valid":true,"unresolved":0,"progress":[{"direction":"inbound","outcome":"committed","count":1,"timestamp":120}]}');
ROLLBACK;
SELECT 'canonical progress snapshot invariants passed';
'''
    return sql


def exercise(run, psql, report):
    result = run(psql, data=fixture())
    assert 'canonical progress snapshot invariants passed' in result.stdout, result.stdout
    report['checks'].append('canonical_progress_actual_snapshot_append_reorg_restored_hash_missing_cursor_health_and_counts')
