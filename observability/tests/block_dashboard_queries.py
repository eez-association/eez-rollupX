"""Real reader SQL regression for observed-source and paid-post bar charts."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def fixture():
    panels = json.loads((ROOT / 'grafana/dashboards/cross-chain.json').read_text())['panels']
    queries = {p['id']: p['targets'][0]['rawSql'] for p in panels if p.get('id') in (31, 32, 33)}
    assert set(queries) == {31, 32, 33}
    quote = lambda text: "'" + text.replace("'", "''") + "'"
    rows = ','.join(f'({key},{quote(value)})' for key, value in queries.items())
    seed = (ROOT / 'tests/cost-allocation.sql').read_text().split('SET CONSTRAINTS ALL IMMEDIATE;')[0]
    return seed + r"""
CREATE TEMP TABLE block_queries(id integer,query text);
INSERT INTO block_queries VALUES """ + rows + r""";
GRANT SELECT ON block_queries,costs_fixture TO eez_observer_reader;
CREATE FUNCTION pg_temp.chart(panel integer,chain text DEFAULT 'all',rollup text DEFAULT 'all',
 instance text DEFAULT 'cost-allocation',source_hash text DEFAULT '',since bigint DEFAULT 100,until bigint DEFAULT 200,
 environment text DEFAULT 'fixture',deployment text DEFAULT 'cost-allocation') RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE query text; result jsonb;BEGIN
 SELECT q.query INTO STRICT query FROM block_queries q WHERE q.id=panel;
 query=replace(replace(replace(replace(replace(replace(replace(replace(replace(query,
 '${environment:sqlstring}',quote_literal(environment)), '${deployment:sqlstring}',quote_literal(deployment)),
 '${chain:sqlstring}',quote_literal(chain)), '${rollup:sqlstring}',quote_literal(rollup)),
 '${instance:sqlstring}',quote_literal(instance)), '${source_hash:sqlstring}',quote_literal(source_hash)),
 '$__timeFrom()','to_timestamp('||since||')'), '$__timeTo()','to_timestamp('||until||')'),E'\n',' ');
 EXECUTE 'SELECT coalesce(jsonb_agg(to_jsonb(row)),''[]''::jsonb) FROM ('||query||') row' INTO result;
 RETURN result;
END $$;
CREATE FUNCTION pg_temp.reference(seq bigint,hash bigint,direction text,kind text DEFAULT 'admitted') RETURNS void LANGUAGE plpgsql AS $$
DECLARE f costs_fixture; source jsonb; event jsonb; chain bigint;BEGIN
 SELECT * INTO f FROM costs_fixture;
 chain=CASE direction WHEN 'inbound' THEN f.l1 ELSE f.l2 END;
 source=jsonb_build_object('chain_id',CASE direction WHEN 'inbound' THEN 1 ELSE 2 END,
 'genesis_hash',pg_temp.jh(CASE direction WHEN 'inbound' THEN 1 ELSE 2 END),
 'rollup_id',CASE direction WHEN 'inbound' THEN 0 ELSE 1 END,'transaction_hash',pg_temp.jh(hash),'direction',direction);
 event=CASE kind WHEN 'admitted' THEN jsonb_build_object('event',kind,'source',source)
 ELSE jsonb_build_object('event',kind,'member',jsonb_build_object('source',source,'admission',jsonb_build_object('epoch',repeat('aa',16),'sequence',seq))) END;
 -- Deliberately outside receipt-time range: event UTC must not filter source blocks.
 INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 VALUES(f.source_id,decode(repeat('aa',16),'hex'),seq,1,'complete',jsonb_build_object('event',event,'observed_at_unix_ms',999999));
 INSERT INTO eez.lifecycle_source_links(source_id,epoch,sequence,context_l1,context_l2,source,state,chain_instance_id,transaction_hash)
 VALUES(f.source_id,decode(repeat('aa',16),'hex'),seq,f.l1,f.l2,source,'linked',chain,pg_temp.h(hash));
END $$;
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 VALUES(:l2,pg_temp.h(21),21,pg_temp.h(20),115,true,'complete');
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(1003),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),1003,2),
 (:l2,pg_temp.h(2002),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),0,2);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 VALUES(:l1,pg_temp.h(1003),pg_temp.h(11),3,true,true,17,3,0,0,51,0,0),
 (:l2,pg_temp.h(2002),pg_temp.h(21),0,true,true,23,0,0,0,0,0,0);
SELECT pg_temp.reference(601,1002,'inbound');
SELECT pg_temp.reference(602,1002,'inbound');
SELECT pg_temp.reference(603,1002,'inbound','readiness');
SELECT pg_temp.reference(604,1003,'inbound','readiness');
SELECT pg_temp.reference(605,2002,'outbound');
SELECT pg_temp.reference(606,2003,'outbound'); -- Missing receipt must not invent a block.
SET CONSTRAINTS ALL IMMEDIATE;
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(31))=2 AND
 (pg_temp.chart(31)->0->>'source_transactions')::int=2 AND (pg_temp.chart(31)->1->>'source_transactions')::int=1,
 'repeated admissions/later references deduplicate; multiple transactions share exact block; no missing-receipt zero');
SELECT pg_temp.assert((pg_temp.chart(32)->0->>'source_gas_used')::int=34 AND
 (pg_temp.chart(32)->1->>'source_gas_used')::int=23,'source gas excludes settlement gas; zero price retains known gas');
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(31,source_hash=>'0x'||pg_temp.jh(1002)))=1 AND
 (pg_temp.chart(31,source_hash=>'0x'||pg_temp.jh(1002))->0->>'source_transactions')::int=1,'source hash restricts unique source population');
SELECT pg_temp.assert(pg_temp.chart(31,source_hash=>''' OR true --')='[]'::jsonb AND
 pg_temp.chart(31,instance=>'other')='[]'::jsonb AND pg_temp.chart(31,environment=>'other')='[]'::jsonb AND
 pg_temp.chart(31,deployment=>'other')='[]'::jsonb,'escaped and nonmatching selectors cannot leak rows');
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(31,chain=>(SELECT l2::text FROM costs_fixture)))=1 AND
 jsonb_array_length(pg_temp.chart(31,rollup=>'999'))=1,'exact chain scope; inbound has no inferred target rollup');
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(31,since=>110,until=>110))=1 AND
 pg_temp.chart(31,since=>116)='[]'::jsonb,'inclusive receipt-time boundary, no event timestamp substitution');
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(33))=2 AND
 (pg_temp.chart(33)->0->>'verified_source_members')::int=2 AND
 pg_temp.chart(33)->1->'verified_source_members'='null'::jsonb,
 'two posts in same block remain separate; verified full membership versus unavailable');
RESET ROLE;
SAVEPOINT variant;
SELECT pg_temp.claim(20,5,pg_temp.population());
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert((pg_temp.chart(33)->0->>'verified_source_members')::int=2,'duplicate paid-post claims do not multiply unique sources');
RESET ROLE;
ROLLBACK TO variant;
SELECT pg_temp.claim(20,5,'[]'::jsonb);
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert(pg_temp.chart(33)->0->'verified_source_members'='null'::jsonb,
 'conflicting paid-post population remains unknown, never chooses one producer');
RESET ROLE;
ROLLBACK TO variant;
UPDATE eez.lifecycle_cost_checks SET decision=jsonb_set(decision,'{population}','[]'::jsonb) WHERE source_id=:source_id;
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert((pg_temp.chart(33)->0->>'verified_source_members')::int=0 AND
 pg_temp.chart(33)->0->>'allocation_state'='no_source_members','known empty population is zero, not unavailable');
RESET ROLE;
ROLLBACK TO variant;
UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=:source_id AND sequence IN (601,602,603);
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert((pg_temp.chart(31)->0->>'source_transactions')::int=1,'late reference conflict retracts only that source');
RESET ROLE;
ROLLBACK TO variant;
UPDATE eez.blocks SET canonical=false WHERE chain_instance_id=:l1 AND hash=pg_temp.h(11);
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(31))=1 AND pg_temp.chart(33)='[]'::jsonb,'source and paid-post reorg retract canonical rows');
RESET ROLE;
ROLLBACK TO variant;
UPDATE eez.chain_instances SET collection_error='transport' WHERE id=:l1;
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(31))=1 AND pg_temp.chart(33)='[]'::jsonb,'chain health unknown cannot remain confirmed');
RESET ROLE;
ROLLBACK TO variant;
-- Bound result rows by actual blocks, with deterministic chronological display.
INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage)
 SELECT :l2,pg_temp.h(3000+i),i,pg_temp.h(2999+i),100+i,true,'complete' FROM generate_series(30,80) i;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 SELECT :l2,pg_temp.h(4000+i),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),i,2 FROM generate_series(30,80) i;
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 SELECT :l2,pg_temp.h(4000+i),pg_temp.h(3000+i),0,true,true,1,0,0,0,0,0,0 FROM generate_series(30,80) i;
SELECT pg_temp.reference(700+i,4000+i,'outbound') FROM generate_series(30,80) i;
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert(jsonb_array_length(pg_temp.chart(31))=50 AND
 pg_temp.chart(31)->0->>'source_block' LIKE 'outbound L2#31 %' AND
 pg_temp.chart(31)->49->>'source_block' LIKE 'outbound L2#80 %','latest 50 actual blocks, chronological deterministic display');
RESET ROLE;
ROLLBACK TO variant;
-- Stored price gaps also lack gas by schema invariant; never turn them into zero.
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 VALUES(:l1,pg_temp.h(1004),decode(repeat('aa',20),'hex'),decode(repeat('bb',20),'hex'),1004,3);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,accounting_gap)
 VALUES(:l1,pg_temp.h(1004),pg_temp.h(11),4,true,true,'missing_blob_price');
SELECT pg_temp.reference(607,1004,'inbound');
SET LOCAL ROLE eez_observer_reader;
SELECT pg_temp.assert((pg_temp.chart(31)->0->>'source_transactions')::int=3 AND
 pg_temp.chart(32)->0->'source_gas_used'='null'::jsonb AND (pg_temp.chart(32)->0->>'gas_unknown_transactions')::int=1,
 'known transaction count survives missing price/gas; incomplete gas total NULL with coverage');
RESET ROLE;
ROLLBACK;
SELECT 'block dashboard query invariants passed';
"""
