"""Duplicate-history query regression; synthetic cache, real reader SQL budget."""
import importlib.util
import json
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



def dashboard_equivalence_fixture():
    """Compare real panel SQL over every existing attribution/retraction case."""
    originals = json.loads((ROOT / 'tests/fixtures/cost-dashboard-baseline.json').read_text())['panels']
    dashboard = json.loads((ROOT / 'grafana/dashboards/gas.json').read_text())
    candidates = {str(p['id']): p['targets'][0]['rawSql'] for p in dashboard['panels'] if p['id'] in (6, 9)}
    assert set(originals) == set(candidates) == {'6', '9'}
    quote = lambda text: "'" + text.replace("'", "''") + "'"
    query_rows = ','.join('(' + key + ',' + quote(originals[key]) + ',' + quote(candidates[key]) + ')' for key in ('6', '9'))
    helpers = r"""
CREATE TEMP TABLE dashboard_queries(id integer,baseline text,candidate text);
INSERT INTO dashboard_queries VALUES """ + query_rows + r""";
GRANT SELECT ON dashboard_queries,costs_fixture TO eez_observer_reader;
CREATE FUNCTION pg_temp.bound_query(query text,chain text,rollup text,source_hash text,
 from_second bigint,to_second bigint,environment text,deployment text) RETURNS text LANGUAGE sql AS $$
 SELECT replace(replace(replace(replace(replace(replace(replace(query,
  '${environment:sqlstring}',quote_literal(environment)),
  '${deployment:sqlstring}',quote_literal(deployment)),
  '${chain:sqlstring}',quote_literal(chain)),
  '${rollup:sqlstring}',quote_literal(rollup)),
  '${source_hash:sqlstring}',quote_literal(source_hash)),
  '$__timeFrom()','to_timestamp('||from_second||')'),
  '$__timeTo()','to_timestamp('||to_second||')')
$$;
CREATE FUNCTION pg_temp.compare_dashboard(label text,chain text DEFAULT 'all',rollup text DEFAULT 'all',
 source_hash text DEFAULT '',from_second bigint DEFAULT 100,to_second bigint DEFAULT 200,
 environment text DEFAULT 'fixture',deployment text DEFAULT 'cost-allocation')
 RETURNS void LANGUAGE plpgsql AS $$ DECLARE panel dashboard_queries;baseline jsonb;candidate jsonb;BEGIN
 FOR panel IN SELECT * FROM dashboard_queries ORDER BY id LOOP
  EXECUTE 'SELECT COALESCE(jsonb_agg(to_jsonb(result) ORDER BY to_jsonb(result)::text),''[]''::jsonb) FROM ('
   ||pg_temp.bound_query(panel.baseline,chain,rollup,source_hash,from_second,to_second,environment,deployment)||') result'
   INTO baseline;
  EXECUTE 'SELECT COALESCE(jsonb_agg(to_jsonb(result) ORDER BY to_jsonb(result)::text),''[]''::jsonb) FROM ('
   ||pg_temp.bound_query(panel.candidate,chain,rollup,source_hash,from_second,to_second,environment,deployment)||') result'
   INTO candidate;
  IF label='initial complete/missing membership'
    AND (baseline='[]'::jsonb OR candidate='[]'::jsonb) THEN
   RAISE EXCEPTION 'initial dashboard % fixture is empty',panel.id;
  END IF;
  IF candidate IS DISTINCT FROM baseline THEN
   RAISE EXCEPTION 'dashboard % / % differs: baseline %, candidate %',panel.id,label,baseline,candidate;
  END IF;
 END LOOP;
END $$;
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.compare_dashboard('initial complete/missing membership');
SELECT pg_temp.compare_dashboard('exact L1 selector',(SELECT l1::text FROM costs_fixture));
SELECT pg_temp.compare_dashboard('exact L2 selector',(SELECT l2::text FROM costs_fixture));
SELECT pg_temp.compare_dashboard('unmatched chain','999999');
SELECT pg_temp.compare_dashboard('exact rollup','all','1');
SELECT pg_temp.compare_dashboard('unmatched rollup','all','999999');
SELECT pg_temp.compare_dashboard('source filter retains denominator','all','all','0x'||pg_temp.jh(1002));
SELECT pg_temp.compare_dashboard('post filter','all','all',pg_temp.jh(5));
SELECT pg_temp.compare_dashboard('escaped source filter','all','all',chr(39)||' OR true --');
SELECT pg_temp.compare_dashboard('receipt time inclusive boundary','all','all','',110,110);
SELECT pg_temp.compare_dashboard('empty receipt time window','all','all','',111,200);
SELECT pg_temp.compare_dashboard('unmatched environment','all','all','',100,200,'other');
SELECT pg_temp.compare_dashboard('unmatched deployment','all','all','',100,200,'fixture','other');
RESET ROLE;
SAVEPOINT dashboard_null;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(99),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),99,3);
INSERT INTO eez.selected_transactions VALUES(:l1,pg_temp.h(99));
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,accounting_gap)
 VALUES(:l1,pg_temp.h(99),pg_temp.h(11),20,false,true,'missing_blob_price');
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 VALUES(:l1,pg_temp.h(99),'decoded',pg_temp.h(990),now(),ARRAY[1]::eez.u64[]);
SELECT pg_temp.assert((SELECT allocation_state='accounting_gap' AND execution_paid IS NULL AND blob_paid IS NULL
 FROM eez.posting_allocation_status WHERE chain_instance_id=:l1 AND transaction_hash=pg_temp.h(99)),
 'dashboard null-cost fixture has actual unknown accounting');
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.compare_dashboard('NULL accounting preserves gap and missing estimates');
RESET ROLE;
ROLLBACK TO dashboard_null;
"""
    seed = (ROOT / 'tests/cost-allocation.sql').read_text()
    assert seed.count('SET CONSTRAINTS ALL IMMEDIATE;') == 1
    seed = seed.replace('SET CONSTRAINTS ALL IMMEDIATE;', 'SET CONSTRAINTS ALL IMMEDIATE;\n' + helpers)
    # Reuse existing meaningful claim/reorg fixtures without reproducing their
    # production predicates. Compare the complete rows before each rollback.
    for name in ('claim_invalidation', 'variant'):
        seed = seed.replace('ROLLBACK TO ' + name + ';',
            "SET LOCAL ROLE eez_observer_reader;\nSELECT pg_temp.compare_dashboard('" + name + " mutation');\nRESET ROLE;\nROLLBACK TO " + name + ';')
    return seed.replace('ROLLBACK;', "SET LOCAL ROLE eez_observer_reader;\nSELECT pg_temp.compare_dashboard('final state');\nRESET ROLE;\nROLLBACK;\nSELECT 'cost dashboard query equivalence passed';")


def exercise(run, psql, report):
    result = run(psql, data=fixture())
    assert 'cost query boundaries passed' in result.stdout
    report['checks'].append('cost_source_lookup_101_equivalent_claims_reader_deadline_and_exact_population')
    result = run(psql, data=dashboard_equivalence_fixture())
    assert 'cost dashboard query equivalence passed' in result.stdout
    report['checks'].append('cost_dashboard_materialization_preserves_exact_rows_selectors_claim_conflicts_unknowns_reorgs_and_populations')
