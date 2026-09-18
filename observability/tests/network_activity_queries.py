"""Actual network panel SQL over synthetic canonical header/body observations."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def fixture():
    panels = json.loads((ROOT / 'grafana/dashboards/network.json').read_text())['panels']
    queries = {p['id']: p['targets'][0]['rawSql'] for p in panels if 27 <= p.get('id', 0) <= 33}
    assert set(queries) == set(range(27, 34))
    quote = lambda value: "'" + value.replace("'", "''") + "'"
    rows = ','.join(f'({key},{quote(value)})' for key, value in queries.items())
    return r"""
BEGIN;
SET LOCAL jit=off;
SET LOCAL statement_timeout='10s';
CREATE FUNCTION pg_temp.h(value bigint) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$ SELECT decode(lpad(to_hex(value),64,'0'),'hex') $$;
CREATE FUNCTION pg_temp.assert(ok boolean,label text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 IF ok IS DISTINCT FROM true THEN RAISE EXCEPTION 'network activity: %',label; END IF; END $$;
INSERT INTO eez.deployments(environment,name) VALUES('fixture','network-activity') RETURNING id AS deployment \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at)
 VALUES(:deployment,'l1',1,pg_temp.h(1),0,0,now()) RETURNING id AS l1 \gset
INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number,last_success_at)
 VALUES(:deployment,'l2',2,pg_temp.h(2),1,0,now()) RETURNING id AS l2 \gset
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage,transaction_count,gas_used,gas_limit,blob_gas_used,excess_blob_gas)
 VALUES(:l2,pg_temp.h(1),1,pg_temp.h(0),100,true,'complete',NULL,NULL,NULL,NULL,NULL),
 (:l2,pg_temp.h(2),2,pg_temp.h(1),101,true,'complete',0,0,100,NULL,NULL),
 (:l2,pg_temp.h(3),3,pg_temp.h(2),103,true,'complete',2,50,100,NULL,NULL),
 (:l2,pg_temp.h(4),4,pg_temp.h(3),104,true,'complete',4,100,200,NULL,NULL),
 (:l2,pg_temp.h(6),6,pg_temp.h(5),106,true,'complete',NULL,NULL,NULL,NULL,NULL),
 (:l2,pg_temp.h(7),7,pg_temp.h(6),105,true,'complete',NULL,NULL,NULL,NULL,NULL),
 (:l1,pg_temp.h(12),12,pg_temp.h(11),110,true,'complete',1,25,100,262144,131072),
 (:l1,pg_temp.h(13),13,pg_temp.h(12),115,true,'complete',0,0,100,0,0),
 (:l1,pg_temp.h(14),14,pg_temp.h(13),120,true,'complete',NULL,NULL,NULL,NULL,NULL);
CREATE TEMP TABLE network_queries(id integer,query text);
INSERT INTO network_queries VALUES """ + rows + r""";
GRANT SELECT ON network_queries TO eez_observer_reader;
CREATE FUNCTION pg_temp.chart(panel integer,since bigint DEFAULT 100,until bigint DEFAULT 200,
 environment text DEFAULT 'fixture',deployment text DEFAULT 'network-activity') RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE query text; result jsonb; BEGIN
 SELECT q.query INTO STRICT query FROM network_queries q WHERE q.id=panel;
 query=replace(replace(replace(replace(query,
 '${environment:sqlstring}',quote_literal(environment)), '${deployment:sqlstring}',quote_literal(deployment)),
 '$__timeFrom()','to_timestamp('||since||')'), '$__timeTo()','to_timestamp('||until||')');
 EXECUTE 'SELECT coalesce(jsonb_agg(to_jsonb(row)),''[]''::jsonb) FROM ('||query||') row' INTO result;
 RETURN result;
END $$;
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert(abs((pg_temp.chart(27)->0->>'mean_seconds')::numeric-4.0/3)<0.000001
 AND abs((pg_temp.chart(27)->0->>'p95_seconds')::numeric-1.9)<0.000001,'cadence only exact consecutive non-backwards parents');
SELECT pg_temp.assert((pg_temp.chart(28)->0->>'mean_transactions')::numeric=2
 AND (pg_temp.chart(28)->0->>'maximum_transactions')::numeric=4,'all transaction counts zero/two/four, unknown excluded');
SELECT pg_temp.assert(abs((pg_temp.chart(29)->0->>'empty_percent')::numeric-100.0/3)<0.000001
 AND (pg_temp.chart(29)->0->>'covered_percent')::numeric=50,'empty denominator includes known zero and excludes old unknown');
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(30))=2
 AND pg_temp.chart(30)->0->>'metric'='l1 chain 1' AND (pg_temp.chart(30)->0->>'utilized_percent')::numeric=12.5
 AND pg_temp.chart(30)->1->>'metric'='l2 chain 2' AND (pg_temp.chart(30)->1->>'utilized_percent')::numeric=37.5,
 'whole-chain gas weighted by covered capacity, separate chains');
SELECT pg_temp.assert((pg_temp.chart(31)->0->>'mean_blobs')::numeric=1
 AND (pg_temp.chart(31)->0->>'maximum_blobs')::numeric=2,'global L1 blobs known zero/two excludes old unknown');
SELECT pg_temp.assert((pg_temp.chart(32)->0->>'mean_excess_blob_gas')::numeric=65536
 AND (pg_temp.chart(32)->0->>'maximum_excess_blob_gas')::numeric=131072,'excess blob gas is observed header state');
SELECT pg_temp.assert((pg_temp.chart(33)->1->>'execution_header_percent')::numeric=50
 AND pg_temp.chart(33)->1->'blob_header_percent'='null'::jsonb
 AND (pg_temp.chart(33)->1->>'parent_interval_percent')::numeric=50,'header and parent coverage, no L2 blob assumption');
SELECT pg_temp.assert(pg_temp.chart(28,since=>100,until=>100)->0->'mean_transactions'='null'::jsonb
 AND pg_temp.chart(29,since=>100,until=>100)->0->'empty_percent'='null'::jsonb
 AND (pg_temp.chart(29,since=>100,until=>100)->0->>'covered_percent')::numeric=0,'old-only range is unknown, never fabricated zero');
DO $$ DECLARE panel integer; BEGIN
 FOR panel IN 27..33 LOOP
  PERFORM pg_temp.assert(pg_temp.chart(panel,environment=>'other')='[]'::jsonb
    AND pg_temp.chart(panel,deployment=>''' OR true --')='[]'::jsonb,'scope and escaped deployment filter');
 END LOOP;
END $$;
RESET ROLE;
UPDATE eez.chain_instances SET last_success_at=now()-interval '91 seconds' WHERE deployment_id=:deployment;
SET LOCAL ROLE eez_observer_reader;
DO $$ DECLARE panel integer; BEGIN
 FOR panel IN 27..33 LOOP
  PERFORM pg_temp.assert(pg_temp.chart(panel)='[]'::jsonb,'stale chain suppresses observed values');
 END LOOP;
END $$;
RESET ROLE;
ROLLBACK;
SELECT 'network activity dashboard invariants passed';
"""
