#!/usr/bin/env python3
"""Exercise generated protocol dashboards through real Grafana datasource APIs.

All chain/native observations are synthetic query fixtures, not protocol replay.
Uses pinned images, an isolated Docker network and exclusively owned resources.
"""
import argparse
import importlib.util
import json
import pathlib
import subprocess
import tempfile
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)
IMAGES = json.loads((ROOT / 'images.lock.json').read_text())
FIXTURE_IMAGES = json.loads((ROOT / 'tests/images.lock.json').read_text())


def run(command, data=None, check=True):
    result = subprocess.run(command, input=data, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
    if check and result.returncode:
        raise RuntimeError(result.stdout[-7000:])
    return result.stdout


def until(label, predicate, seconds=60):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, ValueError, KeyError, IndexError) as error:
            last = str(error)
        time.sleep(.5)
    raise AssertionError(f'{label} timed out: {last}')


def substitute(query, *, source_hash=''):
    values = {'environment': 'fixture', 'deployment': 'canonical-outcomes', 'chain': 'all', 'rollup': 'all', 'instance': '.*', 'source_hash': source_hash}
    for name, value in values.items():
        query = query.replace('${' + name + ':sqlstring}', "'" + value.replace("'", "''") + "'")
        query = query.replace('${' + name + ':regex}', value)
    return query.replace('$__rate_interval', '1m').replace('$__range', '5m')


def rows(response):
    result = response['results']['A']
    assert not result.get('error'), result
    output = []
    for frame in result.get('frames', []):
        names = [field['name'] for field in frame['schema']['fields']]
        output.extend(dict(zip(names, values)) for values in zip(*frame['data']['values']))
    return output


METRICS = r'''
import http.server,time
count=0
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  global count
  count+=1
  lines=['reth_eez_native_queue_sample_valid 1','reth_eez_native_queue_evaluation_valid 1',f'reth_eez_native_queue_sample_timestamp_seconds {time.time()}']
  for direction in ('inbound','outbound'):
   for state in ('queued_ready','queued_blocked','queued_unknown','in_flight'):
    lines.append(f'reth_eez_native_queue_transactions{{direction="{direction}",state="{state}"}} {int(direction=="inbound" and state=="queued_ready")}')
   for kind in ('pending','serviceable'):
    lines.append(f'reth_eez_native_queue_oldest_age_seconds{{direction="{direction}",kind="{kind}"}} {60 if direction=="inbound" else "NaN"}')
    for bound in ('0.1','1','5','10','30','60','120','300','600','1800','3600','21600','86400','+Inf'):
     lines.append(f'reth_eez_native_queue_age_seconds_bucket{{direction="{direction}",kind="{kind}",le="{bound}"}} {int(direction=="inbound" and (bound=="+Inf" or float(bound)>=60))}')
  lines.append('# TYPE reth_eez_stage_duration_seconds histogram')
  for bound in ('0.1','0.25','1','+Inf'):
   lines.append(f'reth_eez_stage_duration_seconds_bucket{{stage="preparation",outcome="prepared",le="{bound}"}} {0 if bound=="0.1" else count}')
  lines.extend([f'reth_eez_stage_duration_seconds_sum{{stage="preparation",outcome="prepared"}} {count*.2}',f'reth_eez_stage_duration_seconds_count{{stage="preparation",outcome="prepared"}} {count}'])
  body=('\n'.join(lines)+'\n').encode();self.send_response(200);self.send_header('Content-Type','text/plain; version=0.0.4');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def log_message(self,*args):pass
http.server.HTTPServer(('0.0.0.0',9000),Handler).serve_forever()
'''

BLOBS = """
UPDATE eez.lifecycle_epochs SET started_at_ms=100000 WHERE source_id=:source_id;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 VALUES(:source_id,decode(repeat('aa',16),'hex'),21,1,'complete',jsonb_build_object('observed_at_unix_ms',110000,'event',jsonb_build_object('event','admitted','source',jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1,'transaction_hash',pg_temp.jh(1004),'direction','outbound'))));
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,22,1,'complete',jsonb_build_object('observed_at_unix_ms',115000,'event',jsonb_build_object('event','terminal','member',jsonb_build_object('source',payload#>'{event,source}','admission',jsonb_build_object('epoch',repeat('aa',16),'sequence',21)),'attempt',NULL,'outcome','nonce_gap','assurance','local_policy','anchor',NULL,'reconciliation',NULL,'elapsed_ns',5000000000)) FROM eez.lifecycle_events WHERE source_id=:source_id AND sequence=21;

UPDATE eez.chain_instances SET selection=selection||jsonb_build_object('wallets',jsonb_build_array('0x'||repeat('aa',20))) WHERE id=:l1;
INSERT INTO eez.transactions(chain_instance_id,hash,sender,recipient,nonce,transaction_type)
 SELECT :l1,pg_temp.h(hash),decode(repeat('aa',20),'hex'),decode(repeat('ee',20),'hex'),hash,3 FROM (VALUES(70),(71),(72)) tx(hash);
INSERT INTO eez.selected_transactions SELECT :l1,pg_temp.h(hash) FROM (VALUES(70),(71),(72)) tx(hash);
INSERT INTO eez.receipt_observations(chain_instance_id,transaction_hash,block_hash,transaction_index,success,canonical,gas_used,effective_gas_price,blob_gas_used,blob_gas_price,execution_paid,blob_paid,purchased_blobs)
 SELECT :l1,pg_temp.h(hash),pg_temp.h(11),idx,success,true,100,2,blobs*131072,3,200,blobs*131072*3,blobs FROM (VALUES(70,8,true,2),(71,9,true,1),(72,10,false,1)) tx(hash,idx,success,blobs);
INSERT INTO eez.posting_analysis(chain_instance_id,transaction_hash,state,calldata_hash,observed_at,verification_rollups)
 SELECT :l1,pg_temp.h(hash),'decoded',pg_temp.h(hash*10),now(),ARRAY[2]::eez.u64[] FROM (VALUES(70),(71),(72)) tx(hash);
INSERT INTO eez.blob_metadata(chain_instance_id,transaction_hash,calldata_hash,blob_hashes,blob_indices,calldata_tail_hash)
 SELECT :l1,pg_temp.h(hash),pg_temp.h(hash*10),CASE WHEN hash=70 THEN ARRAY[pg_temp.h(700),pg_temp.h(701)] ELSE ARRAY[pg_temp.h(720)] END,ARRAY[0]::eez.u256[],pg_temp.h(0) FROM (VALUES(70),(72)) tx(hash);
INSERT INTO eez.blob_measurements(chain_instance_id,transaction_hash,codec_version,encoded_bytes,purchased_blobs,usable_bytes_per_blob,retrieval)
 VALUES(:l1,pg_temp.h(70),0,76000,2,126976,'beacon_blobs'),(:l1,pg_temp.h(72),0,32000,1,126976,'beacon_sidecars');
INSERT INTO eez.blob_attempts(chain_instance_id,transaction_hash,state,retrieval_duration_ns)
 VALUES(:l1,pg_temp.h(70),'measured',100000000),(:l1,pg_temp.h(71),'authentication_failed',200000000),(:l1,pg_temp.h(72),'measured',300000000);
UPDATE eez.lifecycle_events e SET payload=jsonb_build_object('observed_at_unix_ms',105000,'trace',jsonb_build_object('trace_id',repeat('1a',16),'span_id',repeat('2b',8)),'event',jsonb_build_object('event','attempt_prepared','attempt',n.decision->'owner','sync_height',3,'members',jsonb_build_array(n.decision->'member'),'encoded_bytes',1000,'blob_count',1,'blob_allowance',2,'details',jsonb_build_object('preparation',jsonb_build_object('epoch',repeat('aa',16),'sequence',2)))) FROM eez.lifecycle_native_checks n WHERE e.source_id=n.source_id AND n.source_id=:source_id AND e.sequence=3;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,2,1,'complete',jsonb_build_object('observed_at_unix_ms',104000,'event',jsonb_build_object('event','preparation_started','l2_chain_id',2,'l2_genesis_hash',pg_temp.jh(2),'rollup_id',1,'sync',pg_temp.block(33,3),'members',jsonb_build_array(decision->'member'))) FROM eez.lifecycle_native_checks WHERE source_id=:source_id;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,4,1,'complete',jsonb_build_object('observed_at_unix_ms',106000,'event',jsonb_build_object('event','preparation_finished','preparation',jsonb_build_object('epoch',repeat('aa',16),'sequence',2),'outcome','prepared','elapsed_ns',1000000000)) FROM eez.lifecycle_native_checks WHERE source_id=:source_id;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,5,1,'complete',jsonb_build_object('observed_at_unix_ms',107000,'event',jsonb_build_object('event','dispatched','attempt',decision->'owner','target_block',11,'target_timestamp',110,'transaction_hash',pg_temp.jh(5),'nonce',5)) FROM eez.lifecycle_native_checks WHERE source_id=:source_id;
INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
 SELECT source_id,epoch,6,1,'complete',jsonb_build_object('observed_at_unix_ms',108000,'event',jsonb_build_object('event','submission_observed','attempt',decision->'owner','observation','included')) FROM eez.lifecycle_native_checks WHERE source_id=:source_id;
INSERT INTO eez.lifecycle_source_links(source_id,epoch,sequence,context_l1,context_l2,source,state,chain_instance_id,transaction_hash)
 SELECT source_id,epoch,sequence,:l1,:l2,source,'linked',CASE WHEN source->>'direction'='inbound' THEN :l1 ELSE :l2 END,decode(source->>'transaction_hash','hex') FROM eez.lifecycle_declared_sources WHERE source_id=:source_id;

"""


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    project = 'eez-dash-test-' + uuid.uuid4().hex[:10]
    owned = []
    report = {'project': project, 'scope': 'synthetic data, actual Grafana SQL/Prometheus datasource queries; not native protocol/replay coverage', 'checks': []}
    def image(prefix):
        return next(k + '@' + v for k, v in {**IMAGES, **FIXTURE_IMAGES}.items() if k.startswith(prefix))
    def start(name, image_name, options, command=()):
        full = project + '-' + name
        run(['docker', 'run', '-d', '--rm', '--name', full, '--network', project, '--network-alias', name,
             '--security-opt', 'no-new-privileges:true', '--cap-drop', 'ALL', *options, image(image_name), *command])
        owned.append(full)
        return full
    try:
        with tempfile.TemporaryDirectory(prefix=project) as directory:
            path = pathlib.Path(directory); (path/'logs').mkdir()
            config = {'environment':'fixture','deployment':'canonical-outcomes','observer_instance':'fixture-observer','observer_target':'metrics-fixture:9000','log_directory':str(path/'logs'),'alert_webhook_url':'http://127.0.0.1:9/alerts','heartbeat_webhook_url':'http://127.0.0.1:9/heartbeat','database_enabled':True,'native_processes':[{'service':'eez-composer','instance':'canonical-outcomes','metrics_target':'metrics-fixture:9000','metrics_listen':'127.0.0.1:19101'}],'explorer_urls':{'l1':'https://l1-explorer.invalid','l2':'https://l2-explorer.invalid'}}
            runtime = prepare.prepare(config,path/'runtime',fast_test=True)
            (path/'metrics.py').write_text(METRICS)
            password = (runtime/'secrets/grafana-admin-password').read_text()
            run(['docker','network','create','--internal',project])
            db = start('postgres','postgres:', ['--user','postgres','--memory','512m','--cpus','1','--tmpfs','/var/lib/postgresql:rw,mode=1777,size=268435456','--env','POSTGRES_PASSWORD=fixture-only','--env','POSTGRES_DB=eez_observer'])
            until('PostgreSQL', lambda: 'accepting connections' in run(['docker','exec',db,'pg_isready','-h','127.0.0.1','-U','postgres','-d','eez_observer'],check=False))
            psql=['docker','exec','-i',db,'psql','-X','-v','ON_ERROR_STOP=1','-U','postgres','-d','eez_observer']
            for migration in sorted((ROOT/'migrations').glob('*.sql')):
                run(psql,migration.read_text())
            seed=(ROOT/'tests/canonical-outcomes.sql').read_text().split("SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');")[0]
            reader_password=(runtime/'secrets/postgres-reader-password').read_text()
            run(psql,seed+BLOBS+"\nALTER ROLE eez_observer_reader LOGIN PASSWORD '"+reader_password+"';\nCOMMIT;\n")
            fixture=start('metrics-fixture','python:', ['--user','65534','--memory','64m','--mount',f'type=bind,src={path / "metrics.py"},dst=/metrics.py,readonly'],['python3','/metrics.py'])
            start('prometheus','prom/prometheus:', ['--memory','256m','--tmpfs','/prometheus:rw,mode=1777,size=134217728','--mount',f'type=bind,src={runtime / "prometheus"},dst=/etc/prometheus,readonly'],['--config.file=/etc/prometheus/prometheus.yaml','--storage.tsdb.path=/prometheus','--storage.tsdb.retention.time=1h'])
            grafana=start('grafana','grafana/grafana:', ['--memory','512m','--tmpfs','/var/lib/grafana:rw,mode=1777,size=134217728','--env','GF_SECURITY_ADMIN_PASSWORD__FILE=/run/secrets/password','--env','GF_AUTH_ANONYMOUS_ENABLED=false','--env','GF_ANALYTICS_REPORTING_ENABLED=false','--env','GF_ANALYTICS_CHECK_FOR_UPDATES=false','--env','GF_PLUGINS_PREINSTALL_DISABLED=true','--mount',f'type=bind,src={runtime / "secrets/grafana-admin-password"},dst=/run/secrets/password,readonly','--mount',f'type=bind,src={runtime / "grafana/provisioning"},dst=/etc/grafana/provisioning,readonly','--mount',f'type=bind,src={runtime / "grafana/dashboards"},dst=/var/lib/grafana/dashboards,readonly'])
            client=start('query-client','python:', ['--user','65534','--memory','64m'],['python3','-c','import time; time.sleep(3600)'])
            def graf(path, data=None):
                # The client stays inside the isolated network: no exposed host
                # ports or public egress are needed to exercise real HTTP APIs.
                payload={'path':path,'data':data,'password':password}
                code = '''import sys,json,urllib.request,urllib.error,base64
p=json.load(sys.stdin)
h={'Authorization':'Basic '+base64.b64encode(('admin:'+p['password']).encode()).decode()}
if p['data'] is not None:h['Content-Type']='application/json'
r=urllib.request.Request('http://grafana:3000'+p['path'],headers=h,data=json.dumps(p['data']).encode() if p['data'] is not None else None)
try:
 print(urllib.request.urlopen(r,timeout=15).read().decode())
except urllib.error.HTTPError as error:
 print(error.read().decode())
'''
                result=run(['docker','exec','-i',client,'python3','-c',code],json.dumps(payload),check=False)
                try:return json.loads(result)
                except ValueError:raise ValueError('Grafana request failed: '+result[-1500:]) from None
            until('six dashboards provisioned',lambda:len(graf('/api/search?tag=eez'))==6)
            report['checks'].append('six_dashboards_loaded_by_grafana')
            results={}; total=0
            def query(target, datasource, source_hash=""):
                started = time.monotonic()
                target={**target,'refId':'A','datasource':datasource}
                if 'rawSql' in target: target['rawSql']=substitute(target['rawSql'], source_hash=source_hash)
                if 'expr' in target: target['expr']=substitute(target['expr'])
                now=int(time.time()*1000)
                bounds={'from':'100000','to':'200000'} if datasource['uid']=='eez-postgres' else {'from':str(now-300000),'to':str(now)}
                response = graf('/api/ds/query', {'queries': [target], **bounds})
                result = rows(response)
                if target.get('rawSql', '').startswith('WITH scope AS MATERIALIZED'):
                    report['endpoint_grafana_query_seconds'] = time.monotonic() - started
                    executed = response['results']['A']['frames'][0]['schema']['meta']['executedQueryString']
                    plan = run(psql, "SET ROLE eez_observer_reader; SET jit=off; SET statement_timeout='10s'; SET lock_timeout='5s'; EXPLAIN (ANALYZE,BUFFERS,SETTINGS) " + executed + ';')
                    args.report.with_name(args.report.stem + '-endpoint-plan.log').write_text(plan)
                    report['checks'].append('scoped_endpoint_query_plan_within_reader_budget')
                return result
            # Real endpoint queries include all SQL variables and annotations,
            # rather than merely linting static JSON strings.
            for filename in ('cross-chain.json','settlement.json','blobs.json','gas.json'):
                dashboard=json.loads((runtime/'grafana/dashboards'/filename).read_text())
                for variable in dashboard['templating']['list']:
                    if variable['type']=='query':
                        query({'rawSql':variable['query'],'rawQuery':True,'format':'table'},variable['datasource']);total+=1
                for annotation in dashboard['annotations']['list']:
                    annotation_rows = query({'rawSql':annotation['rawSql'],'rawQuery':True,'format':'table'},annotation['datasource']);total+=1
                    if annotation['name'] == 'Local policy dispositions':
                        assert len(annotation_rows) == 1 and annotation_rows[0]['text'] == 'Local disposition: nonce_gap', annotation_rows
                for panel in dashboard['panels']:
                    for target in panel.get('targets',[]):
                        results[panel['title']]=query(target,panel['datasource']);total+=1
            report['queries_executed']=total
            report['checks'].append('all_generated_panel_variable_annotation_queries_executed')
            endpoints = results['Canonical applied endpoints']
            assert len(endpoints) == 2 and {r['applied_endpoint'] for r in endpoints} == {f'0x{32:064x}', f'0x{33:064x}'}, endpoints
            assert all(r['l2_height'] == '3' for r in endpoints), endpoints
            gas = json.loads((runtime/'grafana/dashboards/gas.json').read_text())
            receipt_panel = next(p for p in gas['panels'] if p['title'].startswith('Receipt detail'))
            selected = query(receipt_panel['targets'][0], receipt_panel['datasource'], source_hash=f'0x{1002:064x}')
            assert len(selected) == 2 and {r['role'] for r in selected} == {'l1', 'l2'}, selected
            assert query(receipt_panel['targets'][0], receipt_panel['datasource'], source_hash="' OR true --") == []
            report['checks'].append('historical_prefix_and_current_endpoint_preserved_and_textbox_escaped')
            cross = json.loads((runtime/'grafana/dashboards/cross-chain.json').read_text())
            timeline = next(p for p in cross['panels'] if p['title'].startswith('Request timeline'))
            source_rows = query(timeline['targets'][0], timeline['datasource'], source_hash=f'0x{1002:064x}')
            assert {'admitted','preparation_started','preparation_finished','attempt_prepared','dispatched','submission_observed','terminal'} <= {row['event'] for row in source_rows}, source_rows
            assert not any(row['outcome'] == 'nonce_gap' for row in source_rows), source_rows
            assert len(results['Requests · distinct retained admissions']) == 2
            report['checks'].append('request_discovery_and_source_linked_attempt_preparation_timeline')

            outcome=results['Currently anchored outcomes · L1 watermark in selected range']
            assert outcome==[{'direction':'inbound','outcome':'committed','admissions':1}],outcome
            weighted=results['Weighted purchased-capacity utilization'][0]
            assert weighted['measured_bytes']=='108000' and weighted['measured_purchased_blobs']=='3',weighted
            assert abs(float(weighted['purchased_capacity_utilization'])-108000/(3*126976))<1e-12,weighted
            assert abs(float(weighted['measurement_coverage'])-2/3)<1e-12,weighted
            assert results['Purchased and measured populations'][0]['included_type3_posts']==3
            detail=results['Receipt detail · optional transaction hash']
            assert {r['role'] for r in detail}=={'l1','l2'},detail
            assert all(isinstance(r['execution_paid'],str) and r['explorer_url'].startswith('https://'+r['role']+'-explorer.invalid/tx/0x') for r in detail)
            assert len({(r['role'],r['transaction']) for r in detail})==len(detail)
            wallet = results['Configured wallet spend · separate chain populations'][0]
            assert isinstance(wallet['known_total_paid_wei'], str), wallet
            report['checks'].append('canonical_outcome_weighted_blob_coverage_exact_costs_and_explorer_links')
            cross=json.loads((runtime/'grafana/dashboards/cross-chain.json').read_text())
            live=next(p for p in cross['panels'] if p['title']=='Current native queue · disjoint states')
            until('native queue has actual fixture samples',lambda:bool(query(live['targets'][0],live['datasource'])))
            run(psql,"UPDATE eez.chain_instances SET collection_error='transport';")
            blobs=json.loads((runtime/'grafana/dashboards/blobs.json').read_text())
            for title in ('Purchased and measured populations','Weighted purchased-capacity utilization'):
                panel=next(p for p in blobs['panels'] if p['title']==title)
                assert query(panel['targets'][0],panel['datasource'])==[],title
            run(['docker','stop',fixture])
            until('failed native scrape removes current queue values',lambda:not query(live['targets'][0],live['datasource']),seconds=30)
            report['checks'].append('stale_sql_sources_and_failed_native_scrape_are_unknown_not_zero')
            report['status']='passed'
            print(json.dumps(report,indent=2),flush=True)
    except BaseException as error:
        report.update(status='failed',error=str(error))
        for container in owned:
            log=args.report.with_name(args.report.stem+'-'+container.rsplit('-',1)[-1]+'.log')
            log.write_text(run(['docker','logs',container],check=False))
        raise
    finally:
        for container in reversed(owned): run(['docker','rm','--force','--volumes',container],check=False)
        run(['docker','network','rm',project],check=False)
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
