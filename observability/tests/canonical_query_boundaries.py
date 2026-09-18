"""Canonical query capacity on synthetic repeated-transaction admission history.

This exercises thousands of distinct admission identities on one producer, not
thousands of unique transactions or native importer/checker throughput. Like the
owning canonical-outcomes.sql fixture, digest/FK stubs and cached decisions are
synthetic. Actual receipt, canonicality and conflict predicates still run. Runtime snapshots
use the writer role; the admission lookup uses the Grafana reader role.
"""
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]
PRIMARY_ADMISSIONS = 3000
OTHER_ADMISSIONS = 137


def fixture():
    source = (ROOT.parent / 'crates/eez-observer/src/indexer/outcomes.rs').read_text()
    snapshot, = re.findall(
        r'"(WITH outcomes AS \(SELECT \* FROM eez\.lifecycle_canonical_outcomes[^"\n]+)"',
        source,
    )
    baseline, _ = (ROOT / 'tests/canonical-outcomes.sql').read_text().split(
        "SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');",
        1,
    )
    _, seed = baseline.split('INSERT INTO eez.deployments', 1)
    # Reuse the same independent receipt/root fixtures under different database
    # contexts. Equal chain numbers/hashes make accidental cross-context joins
    # observable; producer IDs and deployment ownership remain disjoint.
    other = ('INSERT INTO eez.deployments' + seed).replace(
        'canonical-outcomes', 'canonical-outcomes-other')
    sql = baseline + '\nALTER TABLE outcomes_fixture RENAME TO primary_outcomes_fixture;\n' + other
    sql += rf"""
CREATE TEMP TABLE capacity_contexts AS
 SELECT 'primary'::text AS label,l1,l2,source_id,{PRIMARY_ADMISSIONS}::bigint AS admissions
 FROM primary_outcomes_fixture
 UNION ALL SELECT 'other',l1,l2,source_id,{OTHER_ADMISSIONS}::bigint FROM outcomes_fixture;
SELECT l1 AS primary_l1,l2 AS primary_l2 FROM capacity_contexts WHERE label='primary' \gset
SELECT l1 AS other_l1,l2 AS other_l2 FROM capacity_contexts WHERE label='other' \gset
CREATE TEMP TABLE capacity_members AS
 SELECT context.*,value*20 AS shift,value+1 AS generation
 FROM capacity_contexts context CROSS JOIN LATERAL generate_series(1,context.admissions-1) value;
"""
    sql += r"""
-- Preserve one producer/epoch and distinct admission, preparation, parent and
-- terminal sequence identities. Unique generations prevent unrelated prepared
-- records from invalidating the digest-bound lineage of another admission.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT event.source_id,event.epoch,event.sequence+member.shift,event.chunk_count,event.state,
 CASE event.sequence
  WHEN 1 THEN event.payload
  WHEN 11 THEN jsonb_set(jsonb_set(jsonb_set(event.payload,
    '{event,member,admission,sequence}',to_jsonb(member.shift+1)),
    '{event,attempt,generation}',to_jsonb(member.generation)),
    '{event,reconciliation,sequence}',to_jsonb(member.shift+10))
  ELSE jsonb_set(event.payload,'{event,attempt,generation}',to_jsonb(member.generation)) END
 FROM capacity_members member JOIN eez.lifecycle_events event ON event.source_id=member.source_id
 WHERE event.sequence IN (1,3,10,11);
INSERT INTO eez.lifecycle_records(source_id,epoch,segment_id,offset_bytes,digest,raw)
 SELECT event.source_id,event.epoch,decode(repeat('bb',16),'hex'),event.sequence+member.shift,
   pg_temp.h((event.sequence+member.shift)::bigint),'\x7b7d'::bytea
 FROM capacity_members member JOIN eez.lifecycle_events event ON event.source_id=member.source_id
 WHERE event.sequence IN (3,10,11);
INSERT INTO eez.lifecycle_chunks(source_id,epoch,sequence,chunk_index,segment_id,offset_bytes,record,raw_digest)
 SELECT event.source_id,event.epoch,event.sequence+member.shift,0,decode(repeat('bb',16),'hex'),
   event.sequence+member.shift,jsonb_build_object('complete',true,'digest',pg_temp.jh((event.sequence+member.shift)::bigint)),
   pg_temp.h((event.sequence+member.shift)::bigint)
 FROM capacity_members member JOIN eez.lifecycle_events event ON event.source_id=member.source_id
 WHERE event.sequence IN (3,10,11);
INSERT INTO eez.lifecycle_native_checks(source_id,epoch,sequence,context_l1,context_l2,state,
 record_refs,parent_sequence,generation,decision)
 SELECT original.source_id,original.epoch,original.sequence+member.shift,original.context_l1,
   original.context_l2,original.state,
   (SELECT jsonb_agg(jsonb_set(jsonb_set(ref,'{sequence}',
     to_jsonb((ref->>'sequence')::bigint+member.shift)),'{digest}',
     to_jsonb(pg_temp.jh((ref->>'sequence')::bigint+member.shift))) ORDER BY ordinal)
    FROM jsonb_array_elements(original.record_refs) WITH ORDINALITY reference(ref,ordinal)),
   original.parent_sequence+member.shift,member.generation,
   jsonb_set(jsonb_set(original.decision,'{member,admission,sequence}',to_jsonb(member.shift+1)),
     '{owner,generation}',to_jsonb(member.generation))
 FROM capacity_members member JOIN eez.lifecycle_native_checks original
   ON original.source_id=member.source_id AND original.sequence=11;
SET CONSTRAINTS ALL IMMEDIATE;
-- The bulk population lives in one rollback transaction, so autovacuum cannot
-- collect its statistics. Match a populated database's analyzed state explicitly.
ANALYZE eez.lifecycle_events;
ANALYZE eez.lifecycle_records;
ANALYZE eez.lifecycle_chunks;
ANALYZE eez.lifecycle_native_checks;
GRANT SELECT ON capacity_contexts TO eez_observer_reader,eez_observer_writer;
"""
    sql += r"""
CREATE FUNCTION pg_temp.capacity_snapshot(l1 bigint,l2 bigint)
 RETURNS TABLE(valid boolean,l1_cursor bytea,l2_cursor bytea,unresolved bigint,progress jsonb)
 LANGUAGE plpgsql AS $$ BEGIN
 RETURN QUERY EXECUTE $snapshot$""" + snapshot + r"""$snapshot$ USING l1,l2;
END $$;
CREATE FUNCTION pg_temp.assert_capacity(context_name text,committed bigint,unknown bigint,claims bigint,label text)
 RETURNS void LANGUAGE plpgsql AS $$
DECLARE context record; observed record; total bigint; canonical bigint; unresolved bigint;
        terminal_claims bigint; admitted bigint;
BEGIN
 SELECT * INTO STRICT context FROM capacity_contexts WHERE capacity_contexts.label=context_name;
 SELECT * INTO STRICT observed FROM pg_temp.capacity_snapshot(context.l1,context.l2);
 IF NOT observed.valid OR observed.unresolved<>unknown
    OR observed.l1_cursor IS DISTINCT FROM (SELECT block_hash::bytea FROM eez.scan_cursors WHERE chain_instance_id=context.l1)
    OR observed.l2_cursor IS DISTINCT FROM (SELECT block_hash::bytea FROM eez.scan_cursors WHERE chain_instance_id=context.l2)
    OR (committed=0 AND observed.progress<>'[]'::jsonb)
    OR (committed>0 AND observed.progress IS DISTINCT FROM jsonb_build_array(jsonb_build_object(
      'direction','inbound','outcome','committed','count',committed,'timestamp',120))) THEN
  RAISE EXCEPTION '%: wrong actual runtime snapshot for %: %',label,context_name,row_to_json(observed);
 END IF;
 SELECT count(*),count(*) FILTER(WHERE outcome.outcome='committed' AND outcome.evidence_class='node_reconciled'),
   count(*) FILTER(WHERE outcome.evidence_class='unknown'),sum(outcome.terminal_claims)
 INTO total,canonical,unresolved,terminal_claims FROM eez.lifecycle_canonical_outcomes outcome
 WHERE outcome.context_l1=context.l1 AND outcome.context_l2=context.l2;
 SELECT count(*) INTO admitted FROM eez.lifecycle_admission_observations WHERE source_id=context.source_id;
 IF total<>context.admissions OR admitted<>context.admissions OR canonical<>committed
    OR unresolved<>unknown OR terminal_claims IS DISTINCT FROM claims THEN
  RAISE EXCEPTION '%: wrong population for %: rows %, admitted %, committed %, unknown %, claims %',
    label,context_name,total,admitted,canonical,unresolved,terminal_claims;
 END IF;
END $$;

-- Retain the full real plan in returned stdout. Callers can save it before and
-- after a new migration; this helper does not modify installed view definitions,
-- raise statement deadlines, set work_mem or change PostgreSQL memory budgets.
"""
    sql += 'PREPARE capacity_snapshot(bigint,bigint) AS ' + snapshot + ';\n'
    sql += f"""
SET LOCAL ROLE eez_observer_writer;
SET LOCAL statement_timeout='10s';
SELECT 'canonical capacity snapshot plan: primary';
EXPLAIN (ANALYZE,BUFFERS,SETTINGS) EXECUTE capacity_snapshot(:primary_l1,:primary_l2);
SELECT 'canonical capacity snapshot plan: other';
EXPLAIN (ANALYZE,BUFFERS,SETTINGS) EXECUTE capacity_snapshot(:other_l1,:other_l2);
SET LOCAL ROLE eez_observer_reader;
SELECT 'canonical capacity admission lookup plan: primary';
EXPLAIN (ANALYZE,BUFFERS,SETTINGS) SELECT count(*)
 FROM eez.lifecycle_admission_status WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary')
 AND status='canonical' AND outcome='committed' AND evidence_class='node_reconciled';
DO $$ BEGIN
 IF (SELECT count(*) FROM eez.lifecycle_admission_status
     WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary')
       AND status='canonical' AND outcome='committed' AND evidence_class='node_reconciled') <> {PRIMARY_ADMISSIONS} THEN
   RAISE EXCEPTION 'reader admission lookup returned the wrong canonical population';
 END IF;
END $$;
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_capacity('primary',{PRIMARY_ADMISSIONS},0,{PRIMARY_ADMISSIONS},'initial population');
SELECT pg_temp.assert_capacity('other',{OTHER_ADMISSIONS},0,{OTHER_ADMISSIONS},'initial other context');
RESET ROLE;
SAVEPOINT original_population;
"""
    sql += r"""
-- Equivalent retained terminals share one admission. Adapt the existing
-- canonical-outcomes.sql copy_terminal helper to this fixture's primary scope.
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,100001,chunk_count,state,payload FROM eez.lifecycle_events
 WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary') AND sequence=11;
INSERT INTO eez.lifecycle_records(source_id,epoch,segment_id,offset_bytes,digest,raw)
 SELECT source_id,epoch,decode(repeat('bb',16),'hex'),100001,pg_temp.h(100001),'\x7b7d'::bytea
 FROM eez.lifecycle_events WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary') AND sequence=100001;
INSERT INTO eez.lifecycle_chunks(source_id,epoch,sequence,chunk_index,segment_id,offset_bytes,record,raw_digest)
 SELECT source_id,epoch,100001,0,decode(repeat('bb',16),'hex'),100001,
   jsonb_build_object('complete',true,'digest',pg_temp.jh(100001)),pg_temp.h(100001)
 FROM eez.lifecycle_events WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary') AND sequence=100001;
INSERT INTO eez.lifecycle_native_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs,parent_sequence,generation,decision)
 SELECT source_id,epoch,100001,context_l1,context_l2,state,
   (SELECT jsonb_agg(CASE WHEN (ref->>'sequence')::bigint=11 THEN
     jsonb_set(jsonb_set(ref,'{sequence}','100001'),'{digest}',to_jsonb(pg_temp.jh(100001)))
     ELSE ref END ORDER BY ordinal) FROM jsonb_array_elements(record_refs) WITH ORDINALITY reference(ref,ordinal)),
   parent_sequence,generation,decision FROM eez.lifecycle_native_checks
 WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary') AND sequence=11;
SET CONSTRAINTS ALL IMMEDIATE;
"""
    sql += f"""
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_capacity('primary',{PRIMARY_ADMISSIONS},0,{PRIMARY_ADMISSIONS+1},'duplicate retained without double count');
RESET ROLE;
SAVEPOINT duplicated_population;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,100002,1,'complete',jsonb_set(payload,'{{event,outcome}}','"source_reverted"')
 FROM eez.lifecycle_events WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary') AND sequence=11;
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_capacity('primary',{PRIMARY_ADMISSIONS-1},1,{PRIMARY_ADMISSIONS+2},'unprojected conflicting terminal retracts one admission');
SELECT pg_temp.assert_capacity('other',{OTHER_ADMISSIONS},0,{OTHER_ADMISSIONS},'unprojected conflict stays in its context');
ROLLBACK TO duplicated_population;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL
 WHERE source_id=(SELECT source_id FROM capacity_contexts WHERE label='primary') AND sequence=10;
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_capacity('primary',{PRIMARY_ADMISSIONS-1},1,{PRIMARY_ADMISSIONS+1},'late parent conflict retracts cached duplicates');
SELECT pg_temp.assert_capacity('other',{OTHER_ADMISSIONS},0,{OTHER_ADMISSIONS},'parent conflict stays in its context');
ROLLBACK TO duplicated_population;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:primary_l1 AND hash=pg_temp.h(12);
UPDATE eez.scan_cursors SET block_hash=pg_temp.h(11) WHERE chain_instance_id=:primary_l1;
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_capacity('primary',0,{PRIMARY_ADMISSIONS},{PRIMARY_ADMISSIONS+1},'orphaned snapshot retracts whole context');
SELECT pg_temp.assert_capacity('other',{OTHER_ADMISSIONS},0,{OTHER_ADMISSIONS},'same hashes in other context remain canonical');
ROLLBACK TO original_population;
SET LOCAL ROLE eez_observer_writer;
SELECT pg_temp.assert_capacity('primary',{PRIMARY_ADMISSIONS},0,{PRIMARY_ADMISSIONS},'restored population');
SELECT pg_temp.assert_capacity('other',{OTHER_ADMISSIONS},0,{OTHER_ADMISSIONS},'restored other context');
DEALLOCATE capacity_snapshot;
SELECT 'canonical query boundaries passed';
ROLLBACK;
"""
    return sql


def exercise(run, psql, report):
    """Return captured output so the caller can retain the EXPLAIN plans."""
    result = run(psql, data=fixture())
    assert 'canonical query boundaries passed' in result.stdout
    report['checks'].append(
        'canonical_query_3000_repeated_transaction_admissions_other_context_duplicates_conflicts_and_reorg')
    return result
