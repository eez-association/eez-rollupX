"""Observed-finality SQL transitions using existing canonical outcome fixtures."""
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[1]


def fixture():
    baseline = (ROOT / 'tests/canonical-outcomes.sql').read_text().split(
        "SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');")[0]
    return baseline + r"""
CREATE FUNCTION pg_temp.assert_finality(expected text,label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE actual text; total bigint; duration numeric;
BEGIN
 SELECT count(*),min(timing_state),min(accepted_to_observed_finality_seconds)
 INTO total,actual,duration FROM eez.lifecycle_admission_finality WHERE source_id=(SELECT source_id FROM outcomes_fixture);
 IF total<>1 OR actual IS DISTINCT FROM expected OR
    (expected='observed_finalized' AND (duration IS NULL OR duration<0)) OR
    (expected<>'observed_finalized' AND duration IS NOT NULL) THEN
  RAISE EXCEPTION '%: expected %, actual % rows, %, duration %',label,expected,total,actual,duration;
 END IF;
END $$;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{observed_at_unix_ms}',to_jsonb(floor(extract(epoch FROM now()-interval '20 seconds')*1000))) WHERE source_id=:source_id AND sequence=1;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(jsonb_set(decision,'{observed_at_ms}',to_jsonb(floor(extract(epoch FROM now()-interval '10 seconds')*1000))),'{elapsed_ns}','10000000000') WHERE source_id=:source_id;
UPDATE eez.lifecycle_events SET payload=jsonb_set(jsonb_set(payload,'{observed_at_unix_ms}',to_jsonb(floor(extract(epoch FROM now()-interval '10 seconds')*1000))),'{event,elapsed_ns}','10000000000') WHERE source_id=:source_id AND sequence=11;
SELECT pg_temp.assert_finality('awaiting_l1_finality','canonical completion precedes finality');
SAVEPOINT before_finality;
UPDATE eez.blocks SET finalized=true WHERE chain_instance_id=:l1;
INSERT INTO eez.settlement_finality(settlement_id,l2_chain_id,number,hash) VALUES(:settlement_q,:l2,3,pg_temp.h(33));
SELECT pg_temp.assert_finality('awaiting_exact_l2_finality','historical P cannot borrow current same-height Q finality');
ROLLBACK TO before_finality;
-- A canonical outbound revert has exact Q source/snapshot dependencies and no
-- historical applied endpoint. Outcome taxonomy remains source-only revert.
UPDATE eez.lifecycle_native_checks SET decision=decision || jsonb_build_object(
 'member',jsonb_build_object('source',jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1,'transaction_hash',pg_temp.jh(1003),'direction','outbound'),'admission',jsonb_build_object('epoch',repeat('aa',16),'sequence',1)),
 'outcome','source_reverted','applied',NULL,
 'source',jsonb_build_object('basis','receipt','source_block',pg_temp.block(33,3),'receipt',jsonb_build_object('block',pg_temp.block(33,3),'transaction_index',1,'success',false))) WHERE source_id=:source_id;
UPDATE eez.lifecycle_events e SET payload=jsonb_set(e.payload,'{event,source}',n.decision#>'{member,source}') FROM eez.lifecycle_native_checks n WHERE e.source_id=n.source_id AND e.source_id=:source_id AND e.sequence=1;
UPDATE eez.lifecycle_events e SET payload=jsonb_set(jsonb_set(e.payload,'{event,member}',n.decision->'member'),'{event,outcome}','"source_reverted"') FROM eez.lifecycle_native_checks n WHERE e.source_id=n.source_id AND e.source_id=:source_id AND e.sequence=11;
SELECT pg_temp.assert_state('verified','outbound reverted source fixture');
SAVEPOINT ready;
UPDATE eez.blocks SET finalized=true WHERE chain_instance_id=:l1;
SELECT pg_temp.assert_finality('awaiting_exact_l2_finality','L1 finality alone cannot finalize L2');
INSERT INTO eez.settlement_finality(settlement_id,l2_chain_id,number,hash) VALUES(:settlement_q,:l2,3,pg_temp.h(33));
SELECT pg_temp.assert_finality('observed_finalized','exact finality has measured observation duration');
CREATE TEMP TABLE first_time AS SELECT finalized_observed_at FROM eez.blocks WHERE chain_instance_id=:l1 AND number=12;
UPDATE eez.blocks SET finalized=true,finalized_observed_at=now()+interval '1 hour' WHERE chain_instance_id=:l1;
DO $$ BEGIN IF (SELECT finalized_observed_at FROM eez.blocks WHERE chain_instance_id=(SELECT l1 FROM outcomes_fixture) AND number=12)<>(SELECT finalized_observed_at FROM first_time) THEN RAISE EXCEPTION 'repeated finalization rewrote first observation'; END IF; END $$;
SELECT pg_temp.assert_finality('observed_finalized','repeat collection/restart remains idempotent');
SAVEPOINT finalized;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=10;
SELECT pg_temp.assert_finality('canonical_unknown','late parent conflict retracts timing');
ROLLBACK TO finalized;
UPDATE eez.correlation_sources SET frozen_reason='l2_finalized_contradiction' WHERE l1_chain_id=:l1;
SELECT pg_temp.assert_finality('canonical_unknown','finality contradiction retracts timing');
ROLLBACK TO finalized;
UPDATE eez.lifecycle_sources SET last_success_at=now()-interval '91 seconds' WHERE id=:source_id;
SELECT pg_temp.assert_finality('stale_lifecycle','stale lifecycle cannot look current');
ROLLBACK TO finalized;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{observed_at_unix_ms}',to_jsonb(floor(extract(epoch FROM now()+interval '10 seconds')*1000))) WHERE source_id=:source_id AND sequence=1;
SELECT pg_temp.assert_finality('clock_inconsistent','future admission must not fabricate negative duration');
ROLLBACK TO finalized;
UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{observed_at_unix_ms}','18446744073709551615') WHERE source_id=:source_id AND sequence=1;
SELECT pg_temp.assert_finality('clock_inconsistent','maximum u64 timestamp remains bounded clock uncertainty');
ROLLBACK TO finalized;
UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{elapsed_ns}','1000000000') WHERE source_id=:source_id;
SELECT pg_temp.assert_finality('clock_inconsistent','UTC drift against monotonic duration is explicit');
ROLLBACK TO finalized;
ALTER TABLE eez.blocks DISABLE TRIGGER capture_finalization_time;
UPDATE eez.blocks SET finalized_observed_at=now()+interval '30 seconds' WHERE chain_instance_id=:l1;
ALTER TABLE eez.blocks ENABLE TRIGGER capture_finalization_time;
SELECT pg_temp.assert_finality('clock_inconsistent','observer wallclock reversal cannot expose a future duration');
ROLLBACK TO finalized;
-- Finality can predate a late admission/resubmission; preserve that state, not
-- zero-clamp or label it a successful zero-latency completion.
ALTER TABLE eez.blocks DISABLE TRIGGER capture_finalization_time;
UPDATE eez.blocks SET finalized_observed_at=now()-interval '30 seconds' WHERE chain_instance_id=:l1;
ALTER TABLE eez.blocks ENABLE TRIGGER capture_finalization_time;
ALTER TABLE eez.settlement_finality DISABLE TRIGGER settlement_finality_guard;
UPDATE eez.settlement_finality SET observed_at=now()-interval '30 seconds' WHERE l2_chain_id=:l2;
ALTER TABLE eez.settlement_finality ENABLE TRIGGER settlement_finality_guard;
SELECT pg_temp.assert_finality('finality_observed_before_admission','late admission retains explicit prior-finality state');
ROLLBACK TO finalized;
-- Simulate rows finalized before schema17: no migration/backfill invented time.
ALTER TABLE eez.blocks DISABLE TRIGGER capture_finalization_time;
UPDATE eez.blocks SET finalized_observed_at=NULL WHERE chain_instance_id=:l1;
ALTER TABLE eez.blocks ENABLE TRIGGER capture_finalization_time;
UPDATE eez.blocks SET finalized=true WHERE chain_instance_id=:l1;
SELECT pg_temp.assert_finality('historical_finality_time_unknown','legacy finalized rows remain timing unknown');
ROLLBACK TO ready;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND number=12;
SELECT pg_temp.assert_finality('canonical_unknown','unfinalized reorg retracts canonical outcome');
ROLLBACK TO ready;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence=1;
DO $$ BEGIN IF EXISTS(SELECT FROM eez.lifecycle_admission_finality WHERE source_id=(SELECT source_id FROM outcomes_fixture)) THEN RAISE EXCEPTION 'missing admission fabricated finality latency'; END IF; END $$;
SELECT 'observed finality timing invariants passed';
ROLLBACK;
"""


def exercise(run, psql, report):
    result = run(psql, data=fixture())
    assert 'observed finality timing invariants passed' in result.stdout
    report['checks'].append('observed_finality_exact_hash_clock_history_reorg_and_idempotency')


def exercise_dashboard(run, psql, query, cross, report):
    """Run after other semantic fixture assertions; mutate only owned fixtures."""
    panels = {p['id']: p for p in cross['panels'] if p['id'] in (17, 18)}

    def rows(number):
        # Earlier real Prometheus state transitions can exceed the production
        # 90s freshness window. Refresh only this owned synthetic producer/chain
        # observation, retaining intentional errors and finality freezes.
        run(psql, """
UPDATE eez.lifecycle_sources SET last_success_at=now() WHERE instance='canonical-outcomes';
UPDATE eez.chain_instances SET last_success_at=now() WHERE deployment_id IN
 (SELECT deployment_id FROM eez.lifecycle_sources WHERE instance='canonical-outcomes');
UPDATE eez.correlation_sources SET last_success_at=now() WHERE l1_chain_id IN
 (SELECT id FROM eez.chain_instances WHERE deployment_id IN
  (SELECT deployment_id FROM eez.lifecycle_sources WHERE instance='canonical-outcomes'));
""")
        panel = panels[number]
        return query(panel['targets'][0], panel['datasource'])

    assert rows(17) == []
    coverage = rows(18)
    assert any(r['timing_state'] == 'awaiting_l1_finality' and r['admissions'] == 1 for r in coverage), coverage
    assert any(r['timing_state'] == 'local_policy_not_applicable' for r in coverage), coverage
    # The existing panel fixture has historical admission UTC (100s) and terminal
    # UTC (120s). Keep that cohort and its exact 20s monotonic clock difference;
    # the observer's real finalization time is deliberately much later.
    run(psql, """
DO $$ DECLARE producer bigint; l1 bigint; l2 bigint; candidate jsonb; source jsonb;
BEGIN
 SELECT id INTO producer FROM eez.lifecycle_sources WHERE instance='canonical-outcomes';
 SELECT n.context_l1,n.context_l2,n.decision INTO l1,l2,candidate FROM eez.lifecycle_native_checks n WHERE n.source_id=producer;
 source=jsonb_build_object('chain_id',2,'genesis_hash',lpad('2',64,'0'),'rollup_id',1,'transaction_hash',lpad(to_hex(1003),64,'0'),'direction','outbound');
 candidate=candidate||jsonb_build_object('member',jsonb_build_object('source',source,'admission',jsonb_build_object('epoch',repeat('aa',16),'sequence',1)),
  'outcome','source_reverted','elapsed_ns',20000000000,'applied',NULL,
  'source',jsonb_build_object('basis','receipt','source_block',candidate->'l2_snapshot','receipt',jsonb_build_object('block',candidate->'l2_snapshot','transaction_index',1,'success',false)));
 UPDATE eez.lifecycle_native_checks SET decision=candidate WHERE source_id=producer;
 UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{event,source}',source) WHERE source_id=producer AND sequence=1;
 UPDATE eez.lifecycle_events SET payload=jsonb_set(jsonb_set(jsonb_set(payload,'{event,member}',candidate->'member'),'{event,outcome}','"source_reverted"'),'{event,elapsed_ns}','20000000000') WHERE source_id=producer AND sequence=11;
 UPDATE eez.blocks SET finalized=true WHERE chain_instance_id=l1;
 INSERT INTO eez.settlement_finality(settlement_id,l2_chain_id,number,hash)
 SELECT s.id,l2,b.number,b.hash FROM eez.settlements s JOIN eez.settlement_blocks b ON b.settlement_id=s.id
 WHERE s.l1_chain_id=l1 AND s.l2_chain_id=l2 AND s.reported_canonical_l2;
END $$;
""")
    measured = rows(17)
    assert len(measured) == 1 and measured[0]['direction'] == 'outbound' and measured[0]['outcome'] == 'source_reverted', measured
    assert measured[0]['measured_admissions'] == 1 and measured[0]['p50_seconds'] > 0, measured
    assert measured[0]['p50_seconds'] == measured[0]['p95_seconds'] == measured[0]['p99_seconds'], measured
    run(psql, "UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{elapsed_ns}','1');")
    assert rows(17) == []
    assert any(r['timing_state'] == 'clock_inconsistent' for r in rows(18))
    run(psql, "UPDATE eez.lifecycle_native_checks SET decision=jsonb_set(decision,'{elapsed_ns}','20000000000');")
    assert len(rows(17)) == 1
    run(psql, "UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{observed_at_unix_ms}','18446744073709551615') WHERE sequence=1;")
    assert rows(17) == []
    # Cohort filtering compares numeric milliseconds, never converts arbitrary
    # u64 producer input to an out-of-range PostgreSQL timestamp.
    assert all(r['timing_state'] == 'local_policy_not_applicable' for r in rows(18))
    run(psql, "UPDATE eez.lifecycle_events SET payload=jsonb_set(payload,'{observed_at_unix_ms}','100000') WHERE sequence=1;")
    assert len(rows(17)) == 1
    run(psql, "UPDATE eez.correlation_sources SET frozen_reason='l2_finalized_contradiction';")
    assert rows(17) == []
    assert any(r['timing_state'] == 'canonical_unknown' for r in rows(18))
    report['checks'].append('observed_finality_actual_grafana_percentiles_counts_clock_gap_and_finality_freeze')
