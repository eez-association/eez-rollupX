"""Duplicate-history query regression; synthetic cache, real reader SQL budget."""
import importlib.util
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[1]


def fixture():
    spec = importlib.util.spec_from_file_location('cost_dashboard_fixture', ROOT / 'tests/smoke-protocol-dashboards.py')
    dashboard = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(dashboard)
    baseline = (ROOT / 'tests/canonical-outcomes.sql').read_text().split(
        "SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');")[0]
    claim = (ROOT / 'tests/cost-allocation.sql').read_text().split(
        'CREATE FUNCTION pg_temp.claim', 1)[1].split('SELECT pg_temp.claim(10,5,pg_temp.population());', 1)[0]
    return baseline + dashboard.BLOBS + dashboard.COSTS + r"""
CREATE TEMP TABLE costs_fixture AS SELECT :l1::bigint AS l1,:l2::bigint AS l2,:source_id::bigint AS source_id;
CREATE FUNCTION pg_temp.claim""" + claim + r"""
-- Different retained records agreeing on one actual population cannot multiply
-- receipt costs, and must not make a source lookup exceed the reader deadline.
SELECT pg_temp.claim(1000+value*10,5,(SELECT decision->'population' FROM eez.lifecycle_cost_checks
 WHERE source_id=:source_id AND sequence=10)) FROM generate_series(1,100) value;
GRANT SELECT ON costs_fixture TO eez_observer_reader;
SET LOCAL ROLE eez_observer_reader;
SET LOCAL statement_timeout='10s';
EXPLAIN (ANALYZE,BUFFERS,SETTINGS) SELECT count(*)>0 FROM eez.source_posting_allocations
 WHERE l1_chain_id=:l1 AND l2_chain_id=:l2
   AND source->>'transaction_hash'=lpad(to_hex(1002),64,'0')
   AND evidence_class='native_membership_receipt_verified';
DO $$ BEGIN
 IF (SELECT count(*) FROM eez.source_posting_allocations WHERE l1_chain_id=(SELECT l1 FROM costs_fixture)
   AND l2_chain_id=(SELECT l2 FROM costs_fixture) AND source->>'transaction_hash'=lpad(to_hex(1002),64,'0'))<>1 THEN
   RAISE EXCEPTION 'duplicate claim history changed the one-source allocation population';
 END IF;
 IF NOT EXISTS(SELECT FROM eez.posting_allocation_membership WHERE context_l1=(SELECT l1 FROM costs_fixture)
   AND context_l2=(SELECT l2 FROM costs_fixture) AND post_hash=decode(lpad(to_hex(5),64,'0'),'hex')
   AND active_claims=101 AND verified_claims=101 AND populations=1 AND evidence_class='native_membership_receipt_verified') THEN
   RAISE EXCEPTION 'every duplicate claim must still be verified and agree';
 END IF;
END $$;
SELECT 'cost query boundaries passed';
ROLLBACK;
"""


def exercise(run, psql, report):
    result = run(psql, data=fixture())
    assert 'cost query boundaries passed' in result.stdout
    report['checks'].append('cost_source_lookup_101_equivalent_claims_reader_deadline_and_exact_population')
