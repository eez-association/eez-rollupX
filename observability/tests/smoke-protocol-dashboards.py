#!/usr/bin/env python3
"""Exercise generated protocol dashboards through real Grafana datasource APIs.

All chain/native observations are synthetic query fixtures, not protocol replay.
Uses pinned images, an isolated Docker network and exclusively owned resources.
"""
import finality_timing
from decimal import Decimal, localcontext
import argparse
import hashlib
import importlib.util
import json
import pathlib
import subprocess
import tempfile
import time
import uuid
import yaml
from urllib.parse import urlencode

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
        sql_value = 'canonical-outcomes' if name == 'instance' else value
        query = query.replace('${' + name + ':sqlstring}', "'" + sql_value.replace("'", "''") + "'")
        query = query.replace('${' + name + ':regex}', value)
    return query.replace('$__rate_interval', '1m').replace('$__range', '5m')


def gwei_text(wei):
    # Test oracle: enough precision for a U256 maximum plus an additional receipt.
    with localcontext() as context:
        context.prec = 120
        value = format(Decimal(wei).scaleb(-9), 'f')
    return value.rstrip('0').rstrip('.') if '.' in value else value


def rows(response):
    result = response['results']['A']
    assert not result.get('error'), result
    output = []
    for frame in result.get('frames', []):
        names = [field['name'] for field in frame['schema']['fields']]
        output.extend(dict(zip(names, values)) for values in zip(*frame['data']['values']))
    return output


METRICS = r'''
import http.server,time,json
count=0
follower='agree'
funding='healthy'
queue='active'
pending_age=60
clock_known=True
class Handler(http.server.BaseHTTPRequestHandler):
 def do_POST(self):
  global follower,funding,queue,pending_age,clock_known
  data=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
  follower=data.get('follower',follower);funding=data.get('funding',funding);queue=data.get('queue',queue)
  pending_age=data.get('pending_age',pending_age);clock_known=data.get('clock_known',clock_known)
  self.send_response(200);self.send_header('Content-Length','0');self.end_headers()
 def do_GET(self):
  global count
  count+=1
  lines=[f'reth_eez_native_queue_sample_valid {int(queue!="unknown")}', 'reth_eez_native_queue_evaluation_valid 1',f'reth_eez_native_queue_sample_timestamp_seconds {time.time()-(100 if queue=="stale" else 0)}']
  lines.extend(['reth_eez_native_terminal_capture_gaps_total{reason="capture_limit"} 0','reth_eez_native_terminal_capture_gaps_total{reason="capture_context"} 0'])
  for direction in ('inbound','outbound'):
   lines.append(f'reth_eez_native_admissions_total{{direction="{direction}"}} {5*count}')
   for outcome,multiplier in (('committed',2),('nonce_gap',1),('replaced',1)):
    lines.append(f'reth_eez_native_terminal_observations_total{{direction="{direction}",outcome="{outcome}"}} {multiplier*count}')
   for kind,seconds in (('blocked',10),('serviceable',20),('unknown',30)):
    lines.append(f'reth_eez_native_queue_readiness_duration_sum_seconds{{direction="{direction}",kind="{kind}"}} {seconds if queue!="idle" and direction=="inbound" else 0}')
    for bound in ('1','5','10','20','30','60','+Inf'):
     lines.append(f'reth_eez_request_readiness_duration_seconds_bucket{{direction="{direction}",outcome="committed",kind="{kind}",le="{bound}"}} {count if bound=="+Inf" or float(bound)>=seconds else 0}')
    lines.extend([f'reth_eez_request_readiness_duration_seconds_count{{direction="{direction}",outcome="committed",kind="{kind}"}} {count}',f'reth_eez_request_readiness_duration_seconds_sum{{direction="{direction}",outcome="committed",kind="{kind}"}} {seconds*count}'])
   lines.extend([f'reth_eez_native_terminal_readiness_clock_unknown_total{{direction="{direction}",outcome="committed"}} 0',f'reth_eez_native_terminal_duration_unknown_total{{direction="{direction}",outcome="committed"}} 0'])
   for state in ('queued_ready','queued_blocked','queued_unknown','in_flight'):
    lines.append(f'reth_eez_native_queue_transactions{{direction="{direction}",state="{state}"}} {int(queue!="idle" and direction=="inbound" and state=="queued_ready")}')
   for kind in ('pending','serviceable'):
    lines.append(f'reth_eez_native_queue_age_unknown_transactions{{direction="{direction}",kind="{kind}"}} {int(queue!="idle" and direction=="inbound" and not clock_known)}')
    lines.append(f'reth_eez_native_queue_oldest_age_seconds{{direction="{direction}",kind="{kind}"}} {pending_age if queue!="idle" and direction=="inbound" and clock_known else "NaN"}')
    for bound in ('0.1','1','5','10','30','60','120','300','600','1800','3600','21600','86400','+Inf'):
     lines.append(f'reth_eez_native_queue_age_seconds_bucket{{direction="{direction}",kind="{kind}",le="{bound}"}} {int(queue!="idle" and direction=="inbound" and clock_known and (bound=="+Inf" or float(bound)>=pending_age))}')
  lines.extend(['reth_eez_settlement_policy_enabled 1', 'reth_eez_settlement_policy_setting{setting="pure_l2_interval_seconds"} 60', 'reth_eez_settlement_policy_evaluation_valid{rollup="1"} 1', f'reth_eez_settlement_policy_evaluation_timestamp_seconds{{rollup="1"}} {time.time()}', 'reth_eez_settlement_anchor_age_seconds{rollup="1"} 75', 'reth_eez_settlement_pending_pure_l2_transactions{rollup="1"} 3', f'reth_eez_admission_cuts_total{{reason="gas"}} {count}', f'reth_eez_target_slot_misses_total{{reason="excluded"}} {count}'])
  lines.append(f'eez_observer_follower_comparison_known {int(follower in ("agree","mismatch","stale"))}')
  if follower!='disabled':lines.append(f'eez_observer_follower_last_attempt_timestamp_seconds {time.time()-(100 if follower=="stale" else 0)}')
  if follower in ('agree','mismatch','stale'):lines.append(f'eez_observer_follower_state_root_agreement {int(follower!="mismatch")}')
  if follower in ('lag','disabled'):lines.append(f'eez_observer_follower_unknown{{reason="{follower}"}} 1')
  if follower in ('agree','mismatch','lag'):lines.append(f'eez_observer_follower_settled_lag_blocks {2 if follower=="lag" else 0}')
  balance_known=funding!='balance_unknown';burn_known=funding not in ('zero_burn','balance_unknown')
  labels='account="poster",chain="l1"'
  lines.extend([f'eez_observer_funding_source_valid{{{labels}}} {int(balance_known)}',f'eez_observer_funding_burn_valid{{{labels}}} {int(burn_known)}'])
  if balance_known:lines.append(f'eez_observer_funding_balance_native_units{{{labels}}} {0 if funding=="underfunded" else 5}')
  if burn_known:lines.append(f'eez_observer_funding_runway_seconds{{{labels}}} {0 if funding=="underfunded" else 172800}')
  lines.extend(['reth_eez_poster_funding_estimate_valid 1',f'reth_eez_poster_funding_balance_valid {int(funding!="native_unknown")}',f'reth_eez_poster_funding_estimate_timestamp_seconds {time.time()-(100 if funding=="expired" else 0)}'])
  if funding!='native_unknown':lines.append(f'reth_eez_poster_can_fund_candidate {int(funding!="underfunded")}')
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


COSTS = """
-- Synthetic native-cache rows test dashboard SQL only, not wire/projection.
UPDATE eez.lifecycle_events e SET payload=jsonb_build_object('observed_at_unix_ms',119000,'event',
 jsonb_build_object('event','reconciled','attempt',n.decision->'owner','anchor',n.decision->'l1_snapshot',
 'details',jsonb_build_object('l1',jsonb_build_object('chain_id',1,'genesis_hash',pg_temp.jh(1),'rollup_id',0),
 'l2',jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1),'post',n.decision->'post')))
 FROM eez.lifecycle_native_checks n WHERE e.source_id=n.source_id AND n.source_id=:source_id AND e.sequence=10;
INSERT INTO eez.lifecycle_cost_checks(source_id,epoch,sequence,context_l1,context_l2,state,generation,record_refs,decision)
 SELECT source_id,epoch,10,context_l1,context_l2,'coherent',generation,record_refs,
 jsonb_build_object('owner',decision->'owner','snapshot',decision->'l1_snapshot','post',decision->'post',
 'population',jsonb_build_array(decision#>'{member,source}',jsonb_build_object('chain_id',2,'genesis_hash',pg_temp.jh(2),'rollup_id',1,'direction','outbound','transaction_hash',pg_temp.jh(1003))),
 'sources',jsonb_build_array(jsonb_build_object('transaction_hash',pg_temp.jh(1002),'direction','inbound','receipt',decision#>'{source,receipt}')))
 FROM eez.lifecycle_native_checks WHERE source_id=:source_id;
"""


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report', type=pathlib.Path, required=True)
    parser.add_argument('--browser', action='store_true', help='Also exercise actual Grafana navigation with installed Playwright/Chromium')
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    project = 'eez-dash-test-' + uuid.uuid4().hex[:10]
    owned = []
    grafana_config = yaml.safe_load((ROOT/'compose.yaml').read_text())['services']['grafana']
    report = {'project': project, 'scope': 'synthetic data, actual Grafana SQL/Prometheus datasource queries; not native protocol/replay coverage', 'checks': []}
    def image(prefix):
        return next(k + '@' + v for k, v in {**IMAGES, **FIXTURE_IMAGES}.items() if k.startswith(prefix))
    def start(name, image_name, options, command=()):
        full = project + '-' + name
        run(['docker', 'run', '-d', '--name', full, '--network', project, '--network-alias', name,
             '--security-opt', 'no-new-privileges:true', '--cap-drop', 'ALL', *options, image(image_name), *command])
        owned.append(full)
        return full
    try:
        with tempfile.TemporaryDirectory(prefix=project) as directory:
            path = pathlib.Path(directory); (path/'logs').mkdir()
            config = {'environment':'fixture','deployment':'canonical-outcomes','observer_instance':'fixture-observer','observer_target':'metrics-fixture:9000','log_directory':str(path/'logs'),'alert_webhook_url':'http://127.0.0.1:9/alerts','heartbeat_webhook_url':'http://127.0.0.1:9/heartbeat','database_enabled':True,'native_processes':[{'service':'eez-composer','instance':'canonical-outcomes','metrics_target':'metrics-fixture:9000','metrics_listen':'127.0.0.1:19101'}],'explorer_urls':{'l1':'https://l1-explorer.invalid','l2':'https://l2-explorer.invalid'}}
            runtime = prepare.prepare(config,path/'runtime',fast_test=True)
            # Record the provisioned bytes, not whichever sources happen to be
            # present after a long integration run or concurrent worktree edits.
            report['dashboard_sha256']={file.name:hashlib.sha256(file.read_bytes()).hexdigest()
                for file in sorted((runtime/'grafana/dashboards').glob('*.json'))}
            report['migration_sha256']={}
            (path/'metrics.py').write_text(METRICS)
            password = (runtime/'secrets/grafana-admin-password').read_text()
            run(['docker','network','create','--internal',project])
            db = start('postgres','postgres:', ['--user','postgres','--memory','512m','--cpus','1','--tmpfs','/var/lib/postgresql:rw,mode=1777,size=268435456','--env','POSTGRES_PASSWORD=fixture-only','--env','POSTGRES_DB=eez_observer'])
            until('PostgreSQL', lambda: 'accepting connections' in run(['docker','exec',db,'pg_isready','-h','127.0.0.1','-U','postgres','-d','eez_observer'],check=False))
            psql=['docker','exec','-i',db,'psql','-X','-v','ON_ERROR_STOP=1','-U','postgres','-d','eez_observer']
            for migration in sorted((ROOT/'migrations').glob('*.sql')):
                sql=migration.read_text()
                report['migration_sha256'][migration.name]=hashlib.sha256(sql.encode()).hexdigest()
                run(psql,sql)
            import block_dashboard_queries
            block_sql = run(psql, block_dashboard_queries.fixture())
            assert 'block dashboard query invariants passed' in block_sql
            report['checks'].append('per_block_source_dedup_gas_unknown_coverage_selectors_reorg_and_paid_post_membership')
            seed=(ROOT/'tests/canonical-outcomes.sql').read_text().split("SELECT pg_temp.assert_state('verified','historical P accepted with current Q snapshot');")[0]
            reader_password=(runtime/'secrets/postgres-reader-password').read_text()
            run(psql,seed+BLOBS+COSTS+"\nALTER ROLE eez_observer_reader LOGIN PASSWORD '"+reader_password+"';\nCOMMIT;\n")
            fixture=start('metrics-fixture','python:', ['--user','65534','--memory','64m','--mount',f'type=bind,src={path / "metrics.py"},dst=/metrics.py,readonly'],['python3','/metrics.py'])
            start('prometheus','prom/prometheus:', ['--memory','256m','--tmpfs','/prometheus:rw,mode=1777,size=134217728','--mount',f'type=bind,src={runtime / "prometheus"},dst=/etc/prometheus,readonly'],['--config.file=/etc/prometheus/prometheus.yaml','--storage.tsdb.path=/prometheus','--storage.tsdb.retention.time=1h'])
            grafana=start('grafana','grafana/grafana:', ['--memory',grafana_config['mem_limit'],'--cpus',str(grafana_config['cpus']),'--env','GOMEMLIMIT='+grafana_config['environment']['GOMEMLIMIT'],'--tmpfs','/var/lib/grafana:rw,mode=1777,size=134217728','--env','GF_SECURITY_ADMIN_PASSWORD__FILE=/run/secrets/password','--env','GF_AUTH_ANONYMOUS_ENABLED=false','--env','GF_ANALYTICS_REPORTING_ENABLED=false','--env','GF_ANALYTICS_CHECK_FOR_UPDATES=false','--env','GF_PLUGINS_PREINSTALL_DISABLED=true','--mount',f'type=bind,src={runtime / "secrets/grafana-admin-password"},dst=/run/secrets/password,readonly','--mount',f'type=bind,src={runtime / "grafana/provisioning"},dst=/etc/grafana/provisioning,readonly','--mount',f'type=bind,src={runtime / "grafana/dashboards"},dst=/var/lib/grafana/dashboards,readonly'])
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
            until('operational dashboards and operator runbooks provisioned', lambda:
                  sorted(item['uid'] for item in graf('/api/search?tag=eez')) ==
                  ['eez-blobs','eez-cross-chain','eez-gas','eez-infrastructure','eez-network','eez-runbooks','eez-settlement'])
            report['checks'].append('six_dashboards_loaded_by_grafana')
            for environment,deployment,text in [('fixture','canonical-outcomes','fixture release verified'),
                    ('other','canonical-outcomes','wrong environment'),('fixture','other','wrong deployment')]:
                created=graf('/api/annotations', {'time':150000,'tags':['eez-release','environment:'+environment,'deployment:'+deployment], 'text':text})
                assert isinstance(created.get('id'),int),created
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
                if target.get('rawSql', '').startswith('WITH scope AS MATERIALIZED') and 'canonical_settlement_endpoints' in target['rawSql']:
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
                    if annotation['datasource']['uid']=='-- Grafana --':
                        tags=[tag.replace('$environment','fixture').replace('$deployment','canonical-outcomes') for tag in annotation['tags']]
                        params=[('from','100000'),('to','200000'),('limit',str(annotation['limit']))]+[('tags',tag) for tag in tags]
                        annotation_rows=graf('/api/annotations?'+urlencode(params))
                        assert len(annotation_rows)==1 and annotation_rows[0]['text']=='fixture release verified',annotation_rows
                    else:
                        annotation_rows = query({'rawSql':annotation['rawSql'],'rawQuery':True,'format':'table'},annotation['datasource'])
                    total+=1
                    if annotation['name'] == 'Local policy dispositions':
                        assert len(annotation_rows) == 1 and annotation_rows[0]['text'] == 'Local disposition: nonce_gap', annotation_rows
                for panel in dashboard['panels']:
                    for target in panel.get('targets',[]):
                        results[panel['title']]=query(target,panel['datasource']);total+=1
            report['queries_executed']=total
            report['checks'].append('release_annotation_tag_filters_match_environment_and_deployment_without_cross_scope_leaks')
            if args.browser:
                # Linux host access to this owned internal bridge address needs
                # no published port or connection to any operator deployment.
                address=json.loads(run(['docker','inspect',grafana]))[0]['NetworkSettings']['Networks'][project]['IPAddress']
                browser_input=path/'browser.json'
                browser_output=args.report.with_name(args.report.stem+'-browser').resolve()
                browser_input.write_text(json.dumps({'url':'http://'+address+':3000','password':password,'output':str(browser_output)}))
                browser_input.chmod(0o600)
                run(psql,"UPDATE eez.chain_instances SET last_success_at=now(); UPDATE eez.lifecycle_sources SET last_success_at=now(); UPDATE eez.correlation_sources SET last_success_at=now();")
                report['browser_verifier_sha256']=hashlib.sha256((ROOT/'tests/verify-browser.cjs').read_bytes()).hexdigest()
                run(['node',str(ROOT/'tests/verify-browser.cjs'),str(browser_input)])
                browser_result=json.loads((browser_output/'browser.json').read_text())
                assert browser_result['status']=='passed',browser_result
                report['browser']=browser_result
                report['checks'].append('actual_browser_dashboards_and_source_links_preserve_context_and_time')
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

            activity = next(panel for panel in cross['panels'] if panel['id'] == 19)
            last_success = next(panel for panel in cross['panels'] if panel['id'] == 20)
            def activity_values():
                return sorted(value for row in query(activity['targets'][0],activity['datasource'])
                              for key,value in row.items() if key.lower()!='time' and isinstance(value,(float,int)))
            retained_success=query(last_success['targets'][0],last_success['datasource'])
            assert retained_success==[{'instance':'canonical-outcomes','direction':'inbound',
                'last_canonical_success':120000,'retained_committed_admissions':1}],retained_success
            for mode,expected in [('active',[0,1]),('idle',[0,0]),('unknown',[]),('stale',[]),('active',[0,1])]:
                run(['docker','exec','-i',client,'python3','-c',"import sys,urllib.request; urllib.request.urlopen(urllib.request.Request('http://metrics-fixture:9000/control',data=sys.stdin.buffer.read(),headers={'Content-Type':'application/json'})).read()"],json.dumps({'queue':mode}))
                until('current activity '+mode,lambda:activity_values()==expected,seconds=30)
                assert query(last_success['targets'][0],last_success['datasource'])==retained_success
                # Exercise the actual plotted state selectors at the current
                # sample, not a historical window containing preceding modes.
                for panel_id, direction in ((26, 'inbound'), (29, 'outbound')):
                    panel = next(p for p in cross['panels'] if p['id'] == panel_id)
                    for index, target in enumerate(panel['targets']):
                        current = {**target, 'instant': True, 'range': False}
                        values = [v for row in query(current, panel['datasource'])
                                  for key, v in row.items() if key.lower() != 'time' and isinstance(v, (float, int))]
                        expected_state = ([] if mode in ('unknown', 'stale') else
                                          [int(mode == 'active' and direction == 'inbound' and index == 0)])
                        assert values == expected_state, (mode, direction, index, values)
                age = next(p for p in cross['panels'] if p['id'] == 30)
                values = [v for row in query({**age['targets'][0], 'instant': True, 'range': False}, age['datasource'])
                          for key, v in row.items() if key.lower() != 'time' and isinstance(v, (float, int))]
                assert values == ([60, 60] if mode == 'active' else []), (mode, values)
                bands = next(p for p in cross['panels'] if p['id'] == 34)
                for index, target in enumerate(bands['targets']):
                    values = sorted(v for row in query({**target, 'instant': True, 'range': False}, bands['datasource'])
                                    for key, v in row.items() if key.lower() != 'time' and isinstance(v, (float, int)))
                    expected_band = ([] if mode in ('unknown', 'stale') else
                                     [0, int(mode == 'active' and index == 1)])
                    assert values == expected_band, (mode, index, values)

            bands = next(p for p in cross['panels'] if p['id'] == 34)
            def band_values():
                return [sorted(v for row in query({**target, 'instant': True, 'range': False}, bands['datasource'])
                               for key, v in row.items() if key.lower() != 'time' and isinstance(v, (float, int)))
                        for target in bands['targets']]
            for seconds, known, expected_band in [(30, True, 0), (120, True, 1), (121, True, 2), (240, False, 3), (60, True, 1)]:
                run(['docker','exec','-i',client,'python3','-c',"import sys,urllib.request; urllib.request.urlopen(urllib.request.Request('http://metrics-fixture:9000/control',data=sys.stdin.buffer.read(),headers={'Content-Type':'application/json'})).read()"],json.dumps({'pending_age':seconds,'clock_known':known}))
                expected_bands = [[0, 1] if i == expected_band else [0, 0] for i in range(4)]
                until('pending age band '+str(seconds)+'/'+str(known),lambda:band_values()==expected_bands,seconds=30)
            report['checks'].append('pending_age_bands_use_gauges_preserve_boundaries_and_unknown_clocks')
            report['checks'].append('directional_queue_charts_and_oldest_age_preserve_active_idle_unknown_stale_states')
            report['checks'].append('idle_activity_recent_history_unknown_and_stale_are_distinct_without_synthetic_health')

            outcome=results['Currently anchored outcomes · L1 watermark in selected range']
            assert outcome==[{'direction':'inbound','outcome':'committed','admissions':1}],outcome
            weighted=results['Weighted purchased-capacity utilization'][0]
            assert weighted['measured_bytes']=='108000' and weighted['measured_purchased_blobs']=='3',weighted
            assert abs(float(weighted['purchased_capacity_utilization'])-108000/(3*126976))<1e-12,weighted
            assert abs(float(weighted['measurement_coverage'])-2/3)<1e-12,weighted
            assert results['Purchased and measured populations'][0]['included_type3_posts']==3
            detail=results['Receipt detail · optional transaction hash']
            assert {r['role'] for r in detail}=={'l1','l2'},detail
            assert all(isinstance(r['execution_paid_gwei'],str) and r['explorer_url'].startswith('https://'+r['role']+'-explorer.invalid/tx/0x') for r in detail)
            assert len({(r['role'],r['transaction']) for r in detail})==len(detail)
            wallet = results['Configured wallet spend · separate chain populations'][0]
            assert isinstance(wallet['known_total_paid_gwei'], str), wallet
            wallet_panel = next(panel for panel in gas['panels'] if panel['id'] == 2)
            exact_expression = wallet_panel['targets'][0]['rawSql'].split(' AS known_execution_paid_gwei')[0].rsplit(',', 1)[1]
            assert 'trim_scale' in exact_expression and '* 0.000000001' in exact_expression
            # Execute the actual shipped display expression through Grafana's
            # PostgreSQL reader, not Python's float representation or a mirror.
            for amounts in [(1,), (0,), (2**256-1,), (2**256-1, 1)]:
                values = ','.join('(' + str(value) + '::numeric)' for value in amounts)
                exact = query({'rawSql': 'SELECT ' + exact_expression + ' AS amount_gwei FROM (VALUES ' + values + ') r(execution_paid)',
                               'rawQuery': True, 'format': 'table'}, wallet_panel['datasource'])
                assert exact == [{'amount_gwei': gwei_text(sum(amounts))}], exact
            missing_amount = query({'rawSql': 'SELECT ' + exact_expression + ' AS amount_gwei FROM (VALUES (NULL::numeric)) r(execution_paid)',
                                    'rawQuery': True, 'format': 'table'}, wallet_panel['datasource'])
            assert missing_amount == [{'amount_gwei': None}], missing_amount
            # Execution/blob/total use one receipt population; the total is not
            # an extra bill and is exact even in the decimal display units.
            for title in ('Configured wallet spend · separate chain populations',
                          'Paid posting calls · successful versus reverted',
                          'Daily selected receipt spend · chain-separated',
                          'Execution versus blob fees · exact included spend',
                          'Canonical paid posting calls'):
                for row in results[title]:
                    assert isinstance(row['known_total_paid_gwei'], str), row
                    assert Decimal(row['known_execution_paid_gwei']) + Decimal(row['known_blob_paid_gwei']) == Decimal(row['known_total_paid_gwei']), row
            report['checks'].append('gwei_exact_decimal_strings_u256_max_plus_one_wei_nulls_and_fee_components')
            allocations = results['Shared posting overhead estimates · per source']
            assert len(allocations) == 2 and {row['direction'] for row in allocations} == {'inbound','outbound'}, allocations
            assert all(row['allocation_rule'] == 'equal_share_v1' and row['estimated_total_gwei'] == gwei_text(100) for row in allocations), allocations
            assert sum(Decimal(row['estimated_total_gwei']) for row in allocations) == Decimal(gwei_text(200))
            companions = results['Attributed L1 companion receipts · exact charges']
            assert len(companions) == 1 and companions[0]['total_paid_gwei'] == gwei_text(200), companions
            coverage = results['Posting allocation coverage · per rollup context']
            assert sum(row['posting_calls'] for row in coverage if row['allocation_state'] == 'allocated') == 1, coverage
            completed_cost = results['Completed sources · known attributed posting cost']
            assert completed_cost == [{'direction':'inbound','committed_sources':1,'sources_with_allocation':1,'sources_without_allocation':0,'known_attributed_posting_gwei':gwei_text(100),'known_posting_per_measured_source_gwei':gwei_text(100)}], completed_cost
            allocation_panel = next(panel for panel in gas['panels'] if panel['id'] == 9)
            assert len(query(allocation_panel['targets'][0], allocation_panel['datasource'], source_hash=f'0x{1003:064x}')) == 1
            assert query(allocation_panel['targets'][0], allocation_panel['datasource'], source_hash="' OR true --") == []
            # A known completion without native cost membership has unknown
            # attributed fees, never a zero-cost contribution to the mean.
            saved_costs = json.loads(run(psql+['-Atq'],"SELECT jsonb_agg(to_jsonb(c)) FROM eez.lifecycle_cost_checks c;"))
            run(psql,"DELETE FROM eez.lifecycle_cost_checks;")
            completed_panel = next(panel for panel in gas['panels'] if panel['id'] == 11)
            missing_cost = query(completed_panel['targets'][0],completed_panel['datasource'])
            assert missing_cost == [{'direction':'inbound','committed_sources':1,'sources_with_allocation':0,'sources_without_allocation':1,'known_attributed_posting_gwei':None,'known_posting_per_measured_source_gwei':None}], missing_cost
            encoded_costs = json.dumps(saved_costs).replace("'","''")
            run(psql,"INSERT INTO eez.lifecycle_cost_checks SELECT * FROM jsonb_populate_recordset(NULL::eez.lifecycle_cost_checks,'"+encoded_costs+"'::jsonb);")
            report['checks'].append('receipt_deduplicated_companions_exact_shared_estimates_and_unallocated_coverage')
            report['checks'].append('canonical_outcome_weighted_blob_coverage_exact_costs_and_explorer_links')
            cross=json.loads((runtime/'grafana/dashboards/cross-chain.json').read_text())
            live=next(p for p in cross['panels'] if p['title']=='Current native queue · disjoint states')
            until('native queue has actual fixture samples',lambda:bool(query(live['targets'][0],live['datasource'])))
            readiness_panels = [panel for panel in cross['panels'] if panel['id'] in (21,22,23)]
            assert len(readiness_panels) == 3
            for panel in readiness_panels:
                for target in panel['targets']:
                    until('readiness timing / coverage query '+panel['title'], lambda panel=panel,target=target:
                          any(isinstance(value,(int,float)) and value >= 0
                              for row in query(target,panel['datasource'])
                              for name,value in row.items() if name.lower()!='time'))
            report['checks'].append('observed_readiness_partitions_percentiles_and_coverage_against_real_prometheus')
            flow = next(panel for panel in cross['panels'] if panel['id'] == 24)
            dispositions = next(panel for panel in cross['panels'] if panel['id'] == 35)
            flow_expressions = {target['refId']: target['expr'] for target in flow['targets']}
            assert set(flow_expressions) == {'A','B','C'}
            def current_prom_values(expression):
                target = {'refId':'A','expr':expression,'instant':True,'range':False,'format':'table'}
                return [value for row in query(target,flow['datasource']) for name,value in row.items()
                        if name.lower() != 'time' and isinstance(value,(float,int))]
            # Compare whole expressions at one evaluation timestamp, avoiding
            # sampling/extrapolation drift between separate rate queries.
            for numerator,denominator,expected_ratio in [('A','B',1.25),('A','C',2.5),('B','C',2.0)]:
                expression = ('abs((' + flow_expressions[numerator] + ')/('
                              + flow_expressions[denominator] + ')-' + str(expected_ratio) + ') < bool 0.000001')
                until('actual flow rate cohort '+numerator+'/'+denominator,
                      lambda expression=expression:current_prom_values(expression)==[1,1])
            terminal_expression = dispositions['targets'][0]['expr']
            until('typed noncommitted disposition series',lambda:
                  current_prom_values('count('+terminal_expression+')')==[4])
            # Grafana keeps Prometheus labels on field metadata, not guaranteed
            # table columns. Test label-preserving matching in PromQL itself.
            for direction in ('inbound','outbound'):
                for outcome in ('nonce_gap','replaced'):
                    selector = ('reth_eez_native_terminal_observations_total{environment="fixture",'
                                'deployment="canonical-outcomes",service="eez-composer",direction="'
                                + direction + '",outcome="' + outcome + '"}')
                    matching = '(' + terminal_expression + ') and on(direction,outcome) ' + selector
                    assert current_prom_values('count('+matching+')') == [1]
                    assert all(value>0 for value in current_prom_values(matching))
            assert current_prom_values('absent(('+terminal_expression+') and on(outcome) '
                'reth_eez_native_terminal_observations_total{outcome="committed"})') == [1]
            report['checks'].append('native_admission_total_committed_flow_ratios_and_typed_dispositions_preserve_distinct_populations')
            settlement=json.loads((runtime/'grafana/dashboards/settlement.json').read_text())
            for title, expected in [('Policy anchor age · candidate target',75),('Included pure-L2 transactions awaiting settlement',3)]:
                panel=next(p for p in settlement['panels'] if p['title']==title)
                observed=until('current policy '+title,lambda:query(panel['targets'][0],panel['datasource']))
                assert any(value==expected for row in observed for value in row.values()),observed
            for title in ('Stage p95 by typed outcome · native','Planning cuts by reason','Exact target misses by reason'):
                panel=next(p for p in settlement['panels'] if p['title']==title)
                until('actual native stage/cut '+title,lambda:query(panel['targets'][0],panel['datasource']))
            report['checks'].append('native_stage_cut_slot_queries_and_exact_policy_anchor_population')
            follower_panels={key:next(p for p in cross['panels'] if p['title'].startswith(prefix)) for key,prefix in [('agreement','Follower state-root agreement'),('unknown','Follower comparison unknown'),('lag','Follower lag · measured target deficit')]}
            def follower_values(key):
                panel=follower_panels[key]
                return [value for row in query(panel['targets'][0],panel['datasource']) for name,value in row.items() if name.lower()!='time' and isinstance(value,(float,int))]
            def follower_control(mode):
                run(['docker','exec','-i',client,'python3','-c',"import sys,urllib.request; urllib.request.urlopen(urllib.request.Request('http://metrics-fixture:9000/control',data=sys.stdin.buffer.read(),headers={'Content-Type':'application/json'})).read()"],json.dumps({'follower':mode}))
            for mode,agreement,lag,unknown in [('agree',[1],[0],[]),('mismatch',[0],[0],[]),('lag',[],[2],[1]),('disabled',[],[],[1]),('stale',[],[],[])]:
                follower_control(mode)
                until('follower '+mode+' exact current dashboard semantics',lambda:follower_values('agreement')==agreement and follower_values('lag')==lag and follower_values('unknown')==unknown)
            # Restore a populated comparison, so the later scrape-failure check
            # proves suppression instead of passing because the fixture was empty.
            follower_control('agree')
            until('follower agreement restored',lambda:follower_values('agreement')==[1])
            report['checks'].append('follower_agreement_actual_mismatch_lag_disabled_and_stale_values_never_coerced')

            funding_panels={number:next(panel for panel in gas['panels'] if panel['id']==number) for number in (12,13,14,15)}
            def funding_values(number):
                panel=funding_panels[number]
                return [value for row in query(panel['targets'][0],panel['datasource']) for key,value in row.items() if key.lower()!='time' and isinstance(value,(float,int))]
            def funding_control(mode):
                run(['docker','exec','-i',client,'python3','-c',"import sys,urllib.request; urllib.request.urlopen(urllib.request.Request('http://metrics-fixture:9000/control',data=sys.stdin.buffer.read(),headers={'Content-Type':'application/json'})).read()"],json.dumps({'funding':mode}))
            for mode,balance,runway,affordable in [('healthy',[5e9],[172800],[1]),('zero_burn',[5e9],[],[1]),('balance_unknown',[],[],[1]),('native_unknown',[5e9],[172800],[]),('expired',[5e9],[172800],[]),('underfunded',[0],[0],[0])]:
                funding_control(mode)
                until('funding '+mode+' exact current dashboard semantics',lambda:funding_values(12)==balance and funding_values(13)==runway and funding_values(14)==affordable)
            funding_control('healthy')
            until('funding evidence restored',lambda:funding_values(12)==[5e9] and funding_values(13)==[172800] and funding_values(14)==[1])
            report['checks'].append('funding_real_zero_unknown_burn_stale_candidate_and_distinct_account_populations')

            unit_seed=(ROOT/'tests/settled-transaction-costs.sql').read_text().split('SELECT pg_temp.assert(')[0]
            run(psql,unit_seed+'\nCOMMIT;\n')
            def unit_cost(number):
                panel=next(p for p in gas['panels'] if p['id']==number)
                target={**panel['targets'][0]}
                target['rawSql']=target['rawSql'].replace('${deployment:sqlstring}',"'settled-cost'")
                return query(target,panel['datasource'])
            measured=unit_cost(16)
            assert len(measured)==1 and measured[0]['posts']==3 and measured[0]['measured_posts']==3 and measured[0]['unmeasured_posts']==0,measured
            assert measured[0]['settled_transactions']=='3' and measured[0]['measured_posting_gwei']==gwei_text(551) and measured[0]['posting_per_settled_transaction_gwei']==gwei_text(183),measured
            details=unit_cost(17)
            assert [row['settled_transactions'] for row in details]==['2','1','0'],details
            assert [row['posting_per_settled_transaction_gwei'] for row in details]==[gwei_text(100),gwei_text(300),None],details
            run(psql,"UPDATE eez.blocks b SET receipt_coverage='gap' FROM eez.chain_instances c JOIN eez.deployments d ON d.id=c.deployment_id WHERE b.chain_instance_id=c.id AND c.role='l2' AND d.name='settled-cost';")
            missing=unit_cost(16)
            assert missing[0]['measured_posts']==1 and missing[0]['unmeasured_posts']==2 and missing[0]['settled_transactions']=='0' and missing[0]['posting_per_settled_transaction_gwei'] is None,missing
            report['checks'].append('settled_transaction_costs_exact_weighted_ratio_overlap_dedup_revert_spend_and_missing_denominator')

            finality_timing.exercise_dashboard(run, psql, query, cross, report)

            run(psql,"UPDATE eez.chain_instances SET collection_error='transport';")
            blobs=json.loads((runtime/'grafana/dashboards/blobs.json').read_text())
            for title in ('Purchased and measured populations','Weighted purchased-capacity utilization'):
                panel=next(p for p in blobs['panels'] if p['title']==title)
                assert query(panel['targets'][0],panel['datasource'])==[],title
            for panel in gas['panels']:
                if panel['id'] in (6,9,10,11,16,17):
                    assert query(panel['targets'][0],panel['datasource'])==[],panel['title']
            assert query(last_success['targets'][0],last_success['datasource'])==[]
            run(['docker','stop',fixture])
            until('failed native scrape removes current queue values',lambda:not query(live['targets'][0],live['datasource']),seconds=30)
            assert activity_values()==[]
            for title in ('Policy anchor age · candidate target','Included pure-L2 transactions awaiting settlement','Active process policy values'):
                panel=next(p for p in settlement['panels'] if p['title']==title)
                assert query(panel['targets'][0],panel['datasource'])==[],title
            until('failed observer scrape removes follower comparison',lambda:all(follower_values(key)==[] for key in follower_panels),seconds=30)
            until('failed scrape removes funding evidence',lambda:all(funding_values(number)==[] for number in funding_panels),seconds=30)
            report['checks'].append('stale_sql_sources_and_failed_native_scrape_are_unknown_not_zero')
            memory_events=dict(line.split() for line in run(['docker','exec',db,'cat','/sys/fs/cgroup/memory.events']).splitlines())
            report['postgres_memory_events']={key:int(value) for key,value in memory_events.items()}
            report['postgres_memory_peak_bytes']=int(run(['docker','exec',db,'cat','/sys/fs/cgroup/memory.peak']).strip())
            assert report['postgres_memory_events']['oom_kill']==0,report['postgres_memory_events']
            for service, container in [('postgres', db), ('grafana', grafana)]:
                state=json.loads(run(['docker','inspect',container]))[0]['State']
                report[service+'_state']={key:state[key] for key in ('Running','OOMKilled','ExitCode')}
                assert state['Running'] and not state['OOMKilled'],report[service+'_state']
                report[service+'_memory_evidence']=run(['docker','exec',container,'cat',
                    '/sys/fs/cgroup/memory.events','/sys/fs/cgroup/memory.peak'])
            report['status']='passed'
            print(json.dumps(report,indent=2),flush=True)
    except BaseException as error:
        report.update(status='failed',error=str(error))
        if 'db' in locals():
            report['postgres_memory_evidence']=run(['docker','exec',db,'cat',
                '/sys/fs/cgroup/memory.events','/sys/fs/cgroup/memory.peak'],check=False)
        report['container_states']={}
        for container in owned:
            inspection=run(['docker','inspect',container],check=False)
            try:
                state=json.loads(inspection)[0]['State']
                report['container_states'][container]={key:state[key] for key in ('Running','OOMKilled','ExitCode')}
            except (ValueError,KeyError,IndexError):
                report['container_states'][container]={'inspection_unavailable':True}
            log=args.report.with_name(args.report.stem+'-'+container.rsplit('-',1)[-1]+'.log')
            log.write_text(run(['docker','logs',container],check=False))
        raise
    finally:
        for container in reversed(owned): run(['docker','rm','--force','--volumes',container],check=False)
        run(['docker','network','rm',project],check=False)
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
