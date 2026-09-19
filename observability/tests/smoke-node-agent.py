#!/usr/bin/env python3
"""Execute the generated readonly node-host collector against isolated Loki."""
import argparse
import datetime
import importlib.util
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT/'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)
IMAGES = json.loads((ROOT/'images.lock.json').read_text())


def run(command, check=True):
    value = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=90)
    if check and value.returncode:
        raise RuntimeError(value.stdout[-5000:])
    return value


def port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1',0))
        return stream.getsockname()[1]


def request(url):
    with urllib.request.urlopen(url,timeout=5) as response:
        return response.read().decode()


def eventually(function, seconds=45):
    deadline = time.monotonic()+seconds
    last = None
    while time.monotonic()<deadline:
        try:
            return function()
        except (AssertionError, OSError) as error:
            last = error
            time.sleep(.3)
    raise AssertionError(f'condition did not hold: {last}')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report',type=pathlib.Path,required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True,exist_ok=True)
    report = {'status':'failed','scope':'generated node-host collector, isolated Loki; synthetic diagnostic records, no native protocol acceptance','checks':[]}
    project = 'eez-node-agent-'+uuid.uuid4().hex[:10]
    loki = project+'-loki'
    compose = None
    owned = []
    with tempfile.TemporaryDirectory(prefix=project+'-') as directory:
        root = pathlib.Path(directory)
        sources = {}
        for kind in ('reth_json','lighthouse_json','lifecycle'):
            source = root/kind
            source.mkdir(mode=0o750)
            sources[kind] = source
        # Match a real unprivileged producer owner. No writable host directory enters the collector.
        assert os.getuid()>0 and os.getgid()>0, 'run fixture as an ordinary user with Docker access'
        uid,gid = os.getuid(),os.getgid()
        loki_port,agent_port = port(),port()
        config = {'environment':'fixture','deployment':project,'observer_instance':'observer',
            'observer_target':'127.0.0.1:19100','log_directory':str(sources['lifecycle']),
            'alert_webhook_url':'http://127.0.0.1:9/alerts','heartbeat_webhook_url':'http://127.0.0.1:9/heartbeat',
            'node_agent': {'instance':'node-one','metrics_target':f'127.0.0.1:{agent_port}',
                'listen':f'127.0.0.1:{agent_port}','loki_url':f'http://127.0.0.1:{loki_port}/loki/api/v1/push',
                'read_gid':gid,'run_uid':uid,'run_gid':gid,'sources':[
                    {'service':'lighthouse' if kind=='lighthouse_json' else 'eez-composer',
                     'instance':'consensus' if kind=='lighthouse_json' else 'composer',
                     'kind':kind,'directory':str(source)} for kind,source in sources.items()]}}
        runtime = prepare.prepare(config,root/'runtime')
        compose = ['docker','compose','-p',project,'-f',str(runtime/'node-agent/compose.yaml')]
        image = next(name+'@'+digest for name,digest in IMAGES.items() if name.startswith('grafana/loki:'))
        try:
            run(['docker','run','-d','--name',loki,'--memory','512m','--memory-swap','512m','--cpus','1','--cap-drop','ALL',
                 '--security-opt','no-new-privileges:true','--tmpfs','/loki:rw,uid=10001,gid=10001,size=134217728',
                 '-p',f'127.0.0.1:{loki_port}:3100','--mount',f'type=bind,src={ROOT}/loki/config.yaml,dst=/etc/loki/config.yaml,readonly',
                 image,'-config.file=/etc/loki/config.yaml'])
            owned.append(loki)
            eventually(lambda: request(f'http://127.0.0.1:{loki_port}/ready'))
            run(compose+['up','-d'])
            eventually(lambda: request(f'http://127.0.0.1:{agent_port}/-/ready'))
            report['checks'].append('generated_compose_owner_only_sources_and_persistent_positions_started')
            timestamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
            safe_hash = '0x'+'12'*32
            safe_trace = '34'*16
            secret = 'SECRET_MESSAGE_SIGNED_PENDING_ENVELOPE'
            url_secret = 'https://user:SECRET_URL_TOKEN@private.invalid/'
            def native(event,level='INFO',**fields):
                return {'timestamp':timestamp,'level':level,'fields':{'event_name':event,'message':secret,'error':url_secret,
                    'raw_signed_transaction':secret,'private_key':secret,**fields},'span':{'trace_id':safe_trace,'secret':secret}}
            records = [native('eez.composer.emission.limits',policy_enabled=True,automatic_admission=False,max_user_txs_per_bundle=128),
                native('eez.held_pool.source_ready',tx_hash=safe_hash,l1_hash=safe_hash,l2_hash=safe_hash,nonce=4,direction='Inbound'),
                native('eez.composer.bundle.dispatched',post_batch_hash=safe_hash,sync_hash=safe_hash,rollup_id=1,sync_height=8),
                native('eez.composer.admission.cut',constraint='composition_blob_limit',accepted_count=2,deferred_count=3,tx_hash=secret),
                native('eez.held_pool.source_ready',level='DEBUG',tx_hash='0x'+'56'*32),
                native('unreviewed.event',tx_hash='0x'+'78'*32)]
            # Native Reth's filtered stdout can omit span fields. The reviewed
            # attestation event carries the same optional ID directly in fields.
            attested = native('eez.prover_client.attested',trace_id=safe_trace,hash=safe_hash,**{'from':10,'to':12})
            del attested['span']
            records.append(attested)
            reth_file = sources['reth_json']/'eez.jsonl'
            reth_file.write_text(''.join(json.dumps(record)+'\n' for record in records))
            reth_file.chmod(0o600)
            lighthouse = sources['lighthouse_json']/'beacon.log'
            lighthouse.write_text(json.dumps({'ts':timestamp,'level':'INFO','msg':'Low peer count','peer_count':2,'error':secret,'endpoint':url_secret})+'\n')
            lighthouse.chmod(0o600)
            # Dedicated lifecycle JSON is not the diagnostic projection and keeps its reviewed typed payload.
            life_file = sources['lifecycle']/'fixture.active'
            life_file.write_text(json.dumps({'schema_version':1,'event':'fixture_lifecycle_transport','sequence':1})+'\n')
            life_file.chmod(0o600)
            def read_rows():
                query = urllib.parse.urlencode({'query':'{environment="fixture",deployment="'+project+'"}','limit':100})
                data = json.loads(request(f'http://127.0.0.1:{loki_port}/loki/api/v1/query_range?'+query))
                rows = [(stream['stream'],json.loads(line)) for stream in data['data']['result'] for _,line in stream['values']]
                assert len(rows)==7, f'expected seven reviewed records, got {len(rows)}'
                return rows
            rows = eventually(read_rows)
            encoded = json.dumps(rows)
            assert secret not in encoded and url_secret not in encoded
            assert '0x'+'56'*32 not in encoded and '0x'+'78'*32 not in encoded
            assert safe_hash in encoded and safe_trace in encoded
            for labels,_ in rows:
                assert {'environment','deployment','service','instance','job'} <= set(labels) <= {'environment','deployment','service','instance','job','detected_level','service_name'},labels
                assert labels.get('service_name',labels['service'])==labels['service']
                assert labels.get('detected_level','unknown') in {'info','warn','error','unknown'}
            by_event = {record.get('event_name',record.get('event')):record for _,record in rows}
            assert by_event['eez.held_pool.source_ready']['nonce']=='4'
            assert by_event['eez.held_pool.source_ready']['l1_hash']==safe_hash
            assert by_event['eez.composer.admission.cut']['constraint']=='composition_blob_limit'
            assert by_event['eez.composer.admission.cut']['tx_hash'] is None
            assert by_event['eez.composer.emission.limits']['policy_enabled']=='true'
            assert by_event['Low peer count']['peer_count']=='2'
            assert by_event['eez.prover_client.attested']['trace_id']==safe_trace
            assert by_event['eez.prover_client.attested']['hash']==safe_hash
            report['checks'].append('actual_loki_preserves_reviewed_decision_hash_position_cut_trace_fields')
            report['checks'].append('secret_url_signed_body_invalid_scalar_debug_unknown_event_excluded')
            report['checks'].append('loki_index_contains_only_bounded_common_identity_labels')
            # Rotation is a new file, then restart proves persisted positions avoid rereading records.
            reth_file.rename(reth_file.with_name('eez.jsonl.1'))
            reth_file.write_text(json.dumps(native('eez.composer.settlement.policy',pure_l2_mode='interval',max_unsettled_l2_blocks=16))+'\n')
            reth_file.chmod(0o600)
            dashboard = json.loads((runtime/'grafana/dashboards/network.json').read_text())
            annotation = next(item for item in dashboard['annotations']['list'] if item['name']=='Composer startup and active policy')
            assert annotation['datasource']=={'type':'loki','uid':'eez-loki'}
            expression = annotation['expr'].replace('$environment','fixture').replace('$deployment',project)
            query = urllib.parse.urlencode({'query':expression,'limit':annotation['maxLines']})
            def policy_rows():
                data = json.loads(request(f'http://127.0.0.1:{loki_port}/loki/api/v1/query_range?'+query))
                values = [entry for stream in data['data']['result'] for entry in stream['values']]
                events = {json.loads(line).get('event_name') for _,line in values}
                assert {'eez.composer.settlement.policy','eez.composer.emission.limits'} <= events
                policies = [value for value in values if json.loads(value[1]).get('event_name')=='eez.composer.settlement.policy']
                assert policies and json.loads(policies[0][1])['max_unsettled_l2_blocks']=='16'
                return policies
            eventually(policy_rows)
            report['checks'].append('actual_generated_grafana_loki_annotation_matches_startup_and_policy_markers')
            # A renamed path first discovered after restart can legitimately replay.
            # Establish both path checkpoints before testing restart persistence.
            def rotation_discovered():
                metrics = request(f'http://127.0.0.1:{agent_port}/metrics')
                assert any(line.startswith('loki_source_file_read_lines_total{') and 'path="/sources/0/eez.jsonl.1"' in line for line in metrics.splitlines())
            eventually(rotation_discovered)
            run(compose+['restart','alloy-node'])
            eventually(lambda: request(f'http://127.0.0.1:{agent_port}/-/ready'))
            time.sleep(6)
            assert len(policy_rows())==1
            restarted_metrics = request(f'http://127.0.0.1:{agent_port}/metrics')
            replayed_lines = sum(float(line.rsplit(' ',1)[1]) for line in restarted_metrics.splitlines() if line.startswith('loki_source_file_read_lines_total{'))
            assert replayed_lines==0, 'collector replayed retained files after restarting with saved positions'
            report['checks'].append('rotated_policy_annotation_is_queryable_after_collector_restart')
            metrics = request(f'http://127.0.0.1:{agent_port}/metrics')
            from client_metrics import inventory
            report['collector_metrics_inventory'] = inventory(metrics)
            report['checks'].append('collector_metrics_actual_name_and_label_key_inventory')
            prom_name = project+'-prometheus'
            prom_port = port()
            # Reuse the stack's existing HTTP fixture for exact upstream label shapes.
            fixture_name,fixture_port = project+'-fixture',port()
            test_images = json.loads((ROOT/'tests/images.lock.json').read_text())
            python_image = next(name+'@'+digest for name,digest in test_images.items() if name.startswith('python:'))
            run(['docker','run','-d','--name',fixture_name,'--memory','128m','--memory-swap','128m',
                 '--cap-drop','ALL','--security-opt','no-new-privileges:true','-p',f'127.0.0.1:{fixture_port}:9000',
                 '--mount',f'type=bind,src={ROOT}/tests/fixture-server.py,dst=/fixture.py,readonly',python_image,'python3','/fixture.py'])
            owned.append(fixture_name)
            eventually(lambda: request(f'http://127.0.0.1:{fixture_port}/'))
            fixture_lines = [
                'reth_builder_validation_disallow_hash{hash="0x'+'aa'*32+'"} 1',
                'reth_sync_caching_precompile_cache_hits{address="0x'+'bb'*20+'"} 1',
                'reth_process_cli_args{args="--fixture --metrics"} 1',
                'process_cli_args{args="--fixture"} 1',
                'reth_eez_native_admissions_total{direction="inbound"} 1',
                'process_resident_memory_bytes 11',
                'reth_process_resident_memory_bytes 22',
                'process_cpu_seconds_total 4',
                'reth_process_cpu_seconds_total 12',
                'reth_transaction_pool_pending_pool_transactions 2',
            ]
            body = json.dumps({'mode':'healthy','metric_lines':fixture_lines}).encode()
            with urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{fixture_port}/control',data=body,headers={'Content-Type':'application/json'}),timeout=5) as response:
                assert response.status==200
            config['native_processes'] = [{'service':'eez-composer','instance':'native',
                'metrics_target':f'127.0.0.1:{fixture_port}','metrics_listen':f'127.0.0.1:{fixture_port}'}]
            prepare.prepare(config,runtime)
            generated = json.loads((runtime/'prometheus/prometheus.yaml').read_text())
            jobs = [job for job in generated['scrape_configs'] if job['job_name'] in ('alloy-node-node-one','eez-composer-native')]
            assert len(jobs)==2
            prom_config = root/'prometheus.json'
            prom_config.write_text(json.dumps({'global':{'scrape_interval':'1s'},'scrape_configs':jobs}))
            prom_config.chmod(0o644)
            prom_image = next(name+'@'+digest for name,digest in IMAGES.items() if name.startswith('prom/prometheus:'))
            run(['docker','run','-d','--name',prom_name,'--network','host','--memory','256m','--memory-swap','256m',
                 '--cap-drop','ALL','--security-opt','no-new-privileges:true','--tmpfs','/prometheus:rw,uid=65534,gid=65534,size=67108864',
                 '--mount',f'type=bind,src={prom_config},dst=/etc/prometheus/config.json,readonly',prom_image,
                 '--config.file=/etc/prometheus/config.json',f'--web.listen-address=127.0.0.1:{prom_port}',
                 '--storage.tsdb.path=/prometheus','--storage.tsdb.retention.time=1h'])
            owned.append(prom_name)
            def prom_query(expression):
                return json.loads(request(f'http://127.0.0.1:{prom_port}/api/v1/query?'+urllib.parse.urlencode({'query':expression})))['data']['result']
            def scraped():
                result = prom_query('up{job="alloy-node-node-one"}')
                assert result and result[0]['value'][1]=='1'
                assert prom_query('go_threads{job="alloy-node-node-one"}')
            eventually(scraped)
            def native_scraped():
                assert prom_query('reth_eez_native_admissions_total{job="eez-composer-native",direction="inbound"}')
                assert prom_query('reth_transaction_pool_pending_pool_transactions{job="eez-composer-native"}')
            eventually(native_scraped)
            assert not prom_query('{job="eez-composer-native",hash!=""}')
            assert not prom_query('{job="eez-composer-native",address!=""}')
            assert not prom_query('reth_builder_validation_disallow_hash')
            assert not prom_query('reth_sync_caching_precompile_cache_hits')
            assert not prom_query('reth_process_cli_args')
            assert not prom_query('process_cli_args')
            report['checks'].append('actual_prometheus_drops_native_cli_flag_series_without_dropping_finite_health')
            report['checks'].append('actual_prometheus_native_scrape_drops_hash_address_samples_keeps_finite_eez_and_client_series')
            infra = json.loads((runtime/'grafana/dashboards/infrastructure.json').read_text())
            panels = {p['id']:p for p in infra['panels']}
            def process_values(panel_id):
                expression = panels[panel_id]['targets'][0]['expr'].replace('$environment',config['environment']).replace('$deployment',config['deployment'])
                return [row for row in prom_query(expression) if row['metric'].get('job')=='eez-composer-native']
            def process_aliases():
                memory = process_values(4)
                assert len(memory)==1 and memory[0]['value'][1]=='11'
                cpu = process_values(5)
                assert len(cpu)==1 and cpu[0]['value'][1]=='0'
            eventually(process_aliases)
            # Real scrape disappearance marks the left alias stale; the existing
            # prefixed producer must still supply one value for the same target.
            prefixed_only = [line for line in fixture_lines if not line.startswith(('process_resident_memory_bytes ', 'process_cpu_seconds_total '))]
            body = json.dumps({'mode':'healthy','metric_lines':prefixed_only}).encode()
            with urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{fixture_port}/control',data=body,headers={'Content-Type':'application/json'}),timeout=5) as response:
                assert response.status==200
            def process_fallback():
                memory = process_values(4)
                assert len(memory)==1 and memory[0]['value'][1]=='22'
            eventually(process_fallback)
            report['checks'].append('actual_process_panels_prefer_single_unprefixed_target_and_fall_back_to_existing_reth_family')
            assert not prom_query('{job="alloy-node-node-one",path!=""}')
            assert not prom_query('{job="alloy-node-node-one",filename!=""}')
            report['checks'].append('actual_prometheus_generated_scrape_drops_rotating_path_series_preserves_health')
            # Switch to the default image user while preserving the positions volume.
            # Only fixture files become group-readable; no operator permissions change.
            for source in sources.values():
                for path in source.iterdir():
                    path.chmod(0o640)
            run(compose+['stop','alloy-node'])
            config['node_agent'].pop('run_uid')
            config['node_agent'].pop('run_gid')
            prepare.prepare(config,runtime)
            run(compose+['up','-d'])
            eventually(lambda: request(f'http://127.0.0.1:{agent_port}/-/ready'))
            with reth_file.open('a') as stream:
                stream.write(json.dumps(native('eez.composer.emission.historical_chunk',from_block=10,to_block=11))+'\n')
            marker = urllib.parse.urlencode({'query':'{deployment="'+project+'",job="eez-logs"} | json | event_name="eez.composer.emission.historical_chunk"','limit':20})
            def default_owner_read():
                data = json.loads(request(f'http://127.0.0.1:{loki_port}/loki/api/v1/query_range?'+marker))
                records = [json.loads(line) for item in data['data']['result'] for _,line in item['values']]
                assert len(records)==1 and records[0]['from_block']=='10'
            eventually(default_owner_read)
            assert len(policy_rows())==1
            report['checks'].append('default_group_reader_after_owner_reconfiguration_preserves_positions_and_reads_new_event')
            report['status']='passed'
        except BaseException as error:
            report['error']=repr(error)
            raise
        finally:
            if compose:
                args.report.with_name(args.report.stem+'-alloy.log').write_text(run(compose+['logs','--no-color'],False).stdout)
                run(compose+['down','--volumes','--remove-orphans'],False)
            for name in owned:
                args.report.with_name(args.report.stem+'-'+name.rsplit('-',1)[1]+'.log').write_text(run(['docker','logs',name],False).stdout)
                run(['docker','rm','--force','--volumes',name],False)
            diagnostics = [path.read_text() for path in args.report.parent.glob(args.report.stem+'-*.log')]
            args.report.with_suffix('.logs.txt').write_text('\n'.join(diagnostics))
            args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'status':report['status'],'checks':report['checks']},indent=2))


if __name__=='__main__':
    main()
