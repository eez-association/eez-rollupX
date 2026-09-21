#!/usr/bin/env python3
"""Validate a uniquely named disposable stack against local fixtures only."""
import argparse
import base64
import datetime
import importlib.util
import json
import pathlib
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
REPO = ROOT.parent
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)
parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=pathlib.Path, default=REPO / 'target/debug/eez-observer')
parser.add_argument('--report', type=pathlib.Path, default=REPO / '.runtime/observability-stack-validation.json')
args = parser.parse_args()
project = 'eez-obs-test-' + uuid.uuid4().hex[:10]
assert project.startswith('eez-obs-test-')
images = json.loads((ROOT / 'images.lock.json').read_text())
prom_image = next(k + '@' + v for k, v in images.items() if k.startswith('prom/prometheus:'))


def run(command, *, check=True):
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if check and result.returncode:
        raise RuntimeError('Local test command failed:\n' + result.stdout[-7000:])
    return result.stdout


def request(url, *, password=None, data=None):
    headers = {}
    if password:
        headers['Authorization'] = 'Basic ' + base64.b64encode(('admin:' + password).encode()).decode()
    if data is not None:
        headers['Content-Type'] = 'application/json'
    body = json.dumps(data).encode() if data is not None else None
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers, data=body), timeout=5) as response:
        data = response.read()
        return json.loads(data) if data else None


def until(description, predicate, seconds=100):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                print('PASS:', description, flush=True)
                return result
        except (OSError, ValueError, KeyError, IndexError, AssertionError) as error:
            last = type(error).__name__
        time.sleep(1)
    raise AssertionError(description + ' did not pass before deadline; last exception=' + str(last))


report = {'project': project, 'timestamp': datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'images': images, 'scope': 'isolated fixtures only; no Chiado service changed', 'checks': []}

try:
    with tempfile.TemporaryDirectory(prefix='eez-observability-stack-') as temp:
        temp = pathlib.Path(temp)
        log_dir = temp / 'logs'
        log_dir.mkdir()
        runtime = temp / 'runtime'
        config = {'environment': 'devnet', 'deployment': project, 'observer_instance': 'fixture-observer',
                  'observer_target': 'observer:19100', 'log_directory': str(log_dir),
                  'alert_webhook_url': 'http://fixture:9000/alerts',
                  'heartbeat_webhook_url': 'http://fixture:9000/heartbeat',
                  'public_rpc_url': 'http://fixture:9000/rpc',
                  'signer_tcp_target': 'fixture:9000',
                  'node_exporter_target': 'fixture:9000',
                  'client_targets': [{'service': 'lighthouse', 'instance': 'fixture-consensus',
                                      'metrics_target': 'consensus:9000'}],
                  'native_processes': [{'service': 'eez-proof-signer', 'instance': 'fixture-signer',
                      'metrics_target': 'fixture:9000', 'metrics_listen': '127.0.0.1:19102'}],
                  'grafana_port': 0, 'prometheus_port': 0, 'loki_port': 0}
        prepare.prepare(config, runtime, fast_test=True)
        endpoint = {'url': 'http://fixture:9000/rpc', 'chain_id': 1, 'genesis_hash': '0x' + 'aa' * 32}
        observer_config = temp / 'observer.json'
        observer_config.write_text(json.dumps({'l1': endpoint, 'l2': endpoint,
            'network': {'l2_safe_head': True, 'explorers': [
                {'chain': 'l2', 'profile': 'blockscout_v2', 'url': 'http://fixture:9000'}]},
            'listen': '0.0.0.0:19100', 'allow_non_loopback': True}))
        compose = ['docker', 'compose', '--project-name', project, '--env-file', str(runtime / 'compose.env'), '-f', str(ROOT / 'compose.yaml')]
        test_images = json.loads((ROOT / 'tests/images.lock.json').read_text())
        fixture_image = next(k + '@' + v for k, v in test_images.items())
        report['fixture_images'] = test_images
        override = runtime / 'compose.test.json'
        override.write_text(json.dumps({'services': {
            'consensus': {'image': fixture_image, 'command': ['python3', '/fixture.py'],
                          'volumes': [str(ROOT / 'tests/fixture-server.py') + ':/fixture.py:ro'],
                          'mem_limit': '128m', 'cap_drop': ['ALL'],
                          'security_opt': ['no-new-privileges:true']},
            'fixture': {'image': fixture_image, 'command': ['python3', '/fixture.py'],
                        'volumes': [str(ROOT / 'tests/fixture-server.py') + ':/fixture.py:ro'],
                        'ports': ['127.0.0.1:0:9000'], 'mem_limit': '128m', 'cap_drop': ['ALL'],
                        'security_opt': ['no-new-privileges:true']},
            'observer': {'image': fixture_image, 'entrypoint': ['/observer'],
                         'command': ['exporter', '--config', '/observer.json'],
                         'volumes': [str(args.binary.resolve()) + ':/observer:ro', str(observer_config) + ':/observer.json:ro'],
                         'mem_limit': '128m', 'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true']}
        }}))
        compose += ['-f', str(override)]
        password = (runtime / 'secrets/grafana-admin-password').read_text()
        try:
            run(compose + ['config', '--quiet'])
            run(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', '/bin/promtool',
                 '-v', f'{runtime / "prometheus"}:/etc/prometheus:ro', prom_image, 'check', 'config', '/etc/prometheus/prometheus.yaml'])
            run(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', '/bin/promtool',
                 '-v', f'{ROOT}:/work:ro', '-w', '/work/tests', prom_image, 'test', 'rules', 'rules.test.yaml'])
            alloy_image = next(k + '@' + v for k, v in images.items() if k.startswith('grafana/alloy:'))
            run(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', '/bin/alloy',
                 '-v', f'{runtime / "alloy/config.alloy"}:/etc/alloy/config.alloy:ro', alloy_image, 'validate', '/etc/alloy/config.alloy'])
            report['checks'].append('compose_config_promtool_and_alloy')
            print('PASS: Compose config, promtool config and recording-rule regression tests', flush=True)
            run(compose + ['up', '-d', '--wait', '--wait-timeout', '150'])
            def base(service, port):
                return 'http://' + run(compose + ['port', service, str(port)]).strip()
            prom, grafana, loki = base('prometheus', 9090), base('grafana', 3000), base('loki', 3100)
            fixture = base('fixture', 9000)
            def query(expr):
                result = request(prom + '/api/v1/query?' + urllib.parse.urlencode({'query': expr}))
                assert result['status'] == 'success'
                return result['data']['result']
            def graf(path):
                return request(grafana + path, password=password)
            phase_start = 0
            def notification(title, status):
                return any(event['path'] == '/alerts' and any(a.get('labels', {}).get('alertname') == title and a.get('status') == status for a in event['data'].get('alerts', [])) for event in request(fixture + '/events')[phase_start:])
            until('all configured Prometheus scrape targets healthy', lambda: query('min(up)')[0]['value'][1] == '1')
            until('read-only RPC and signer TCP probes succeed', lambda: query('min(probe_success)')[0]['value'][1] == '1')
            until('queue recording rules populated', lambda: query('sum(eez:queue_transactions:current)')[0]['value'][1] == '4')
            expected_dashboards = ['eez-infrastructure', 'eez-network', 'eez-runbooks']
            until('read-only dashboards and operator runbooks provisioned', lambda: sorted(item['uid'] for item in graf('/api/search?tag=eez')) == expected_dashboards)
            rules = graf('/api/v1/provisioning/alert-rules')
            assert any(rule['uid'] == 'eez-heartbeat' for rule in rules)
            assert next(rule for rule in rules if rule['uid'] == 'eez-observer-down')['data'][1]['model']['expression'] == '$A > 0'
            for filename in ('network.json', 'infrastructure.json'):
                dashboard = json.loads((ROOT / 'grafana/dashboards' / filename).read_text())
                for panel in dashboard['panels']:
                    if panel.get('datasource', {}).get('uid') != 'eez-prometheus':
                        continue
                    for target in panel.get('targets', []):
                        expr = target['expr'].replace('$environment', 'devnet').replace('$deployment', project).replace('$instance', 'fixture-observer').replace('${client_instance:regex}', '.*')
                        query(expr)
            report['checks'].append('dashboards_datasources_and_all_panel_queries')
            until('configured safe and explorer workers publish verified heights', lambda:
                  len(query('eez_observer_network_height{chain="l2"} == 100')) == 2)
            assert len(query('eez_observer_network_lag_blocks{chain="l2"} == 0')) == 2
            request(fixture + '/control', data={'mode': 'network_error'})
            until('optional source failures remove heights', lambda:
                  not query('eez_observer_network_height') and
                  len(query('eez_observer_network_evaluation_valid{chain="l2"} == 0')) == 2)
            assert query('eez:chain_head_number:current{source="l1"}')
            request(fixture + '/control', data={'mode': 'healthy'})
            until('optional network sources recover independently', lambda:
                  len(query('eez_observer_network_height{chain="l2"} == 100')) == 2)
            infrastructure = json.loads((ROOT / 'grafana/dashboards/infrastructure.json').read_text())
            for title in ('Host CPU utilization', 'Host network traffic', 'Host disk mean operation latency',
                          'Host disk busy time', 'Connected peers · reported process scope',
                          'Consensus synchronization', 'Consensus head slot', 'Consensus finalized epoch',
                          'Observed process restarts · past hour', 'Public RPC probe duration'):
                panel = next(panel for panel in infrastructure['panels'] if panel['title'] == title)
                expressions = [target['expr'].replace('$environment', 'devnet').replace('$deployment', project)
                               .replace('${client_instance:regex}', '.*') for target in panel['targets']]
                assert any(query(expr) for expr in expressions), title
            report['checks'].append('optional_network_worker_config_failure_recovery_and_positive_infrastructure_panels')
            (log_dir / 'fixture.jsonl').write_text(json.dumps({'level': 'INFO', 'message': 'eez-monitoring-smoke'}) + '\n')
            def logs_present():
                url = loki + '/loki/api/v1/query_range?' + urllib.parse.urlencode({'query': '{deployment="' + project + '"} |= "eez-monitoring-smoke"', 'limit': 10})
                return request(url)['data']['result']
            until('JSON file collected through Alloy into Loki', logs_present, seconds=120)
            until('independent process receives heartbeat', lambda: request(fixture + '/heartbeat-status')['count'] > 0)
            phase_start = len(request(fixture + '/events'))
            request(fixture + '/control', data={'mode': 'metrics_error'})
            until('configured native metrics failure alert delivered', lambda: notification('EEZNativeTelemetryUnavailable_eez-proof-signer-fixture-signer', 'firing'))
            assert query('eez:chain_head_number:current{source="l1"}')
            request(fixture + '/control', data={'mode': 'healthy'})
            until('configured native metrics recovery delivered', lambda: notification('EEZNativeTelemetryUnavailable_eez-proof-signer-fixture-signer', 'resolved'))
            report['checks'].append('native_metrics_target_failure_and_recovery')
            phase_start = len(request(fixture + '/events'))
            request(fixture + '/control', data={'mode': 'stale'})
            until('stale readiness removes queue recordings', lambda: query('eez_observer_queue_evaluation_valid')[0]['value'][1] == '0' and not query('eez:queue_transactions:current'))
            until('stale readiness alert delivered', lambda: notification('EEZQueueEvaluationStale', 'firing'))
            request(fixture + '/control', data={'mode': 'healthy'})
            until('stale readiness recovery delivered', lambda: notification('EEZQueueEvaluationStale', 'resolved'))
            phase_start = len(request(fixture + '/events'))
            request(fixture + '/control', data={'mode': 'error'})
            until('RPC collection failure alert delivered', lambda: notification('EEZCollectionUnavailable_queue', 'firing'))
            assert query('eez:chain_head_number:current{source="l1"}')
            assert not query('eez:queue_transactions:current')
            request(fixture + '/control', data={'mode': 'healthy'})
            until('RPC collection recovery delivered', lambda: notification('EEZCollectionUnavailable_queue', 'resolved'))
            report['checks'].append('stale_queue_and_rpc_failure_alert_delivery_and_recovery')
            phase_start = len(request(fixture + '/events'))
            run(compose + ['stop', 'observer'])
            until('stopped exporter removes recorded healthy data', lambda: query('up{job="eez-observer"}')[0]['value'][1] == '0' and not query('eez:chain_head_number:current'))
            until('stopped exporter alert delivered', lambda: notification('EEZObserverUnavailable', 'firing'))
            run(compose + ['start', 'observer'])
            until('exporter recovery delivered', lambda: notification('EEZObserverUnavailable', 'resolved'))
            until('queue recovers before restart test', lambda: query('eez:queue_transactions:current'))
            historical_time = int(time.time()) - 5
            run(compose + ['up', '-d', '--force-recreate', '--no-deps', 'prometheus', 'grafana', 'loki', 'alloy'])
            prom, grafana, loki = base('prometheus', 9090), base('grafana', 3000), base('loki', 3100)
            until('Grafana dashboards persist across container replacement', lambda: sorted(item['uid'] for item in graf('/api/search?tag=eez')) == expected_dashboards)
            until('Prometheus historical sample persists', lambda: query(f'eez_observer_chain_head_number{{source="l1"}} @ {historical_time}'))
            until('Loki logs persist across restart', logs_present, seconds=120)
            report['checks'].append('exporter_failure_and_restart_persistence')
            before_stop = request(fixture + '/heartbeat-status')['count']
            until('heartbeat resumed after container replacement', lambda: request(fixture + '/heartbeat-status')['count'] > before_stop)
            run(compose + ['stop', 'grafana', 'prometheus', 'loki', 'alloy', 'blackbox'])
            def expired():
                age = request(fixture + '/heartbeat-status')['age_seconds']
                return age is not None and age > 35
            until('receiver detects missing heartbeat after whole stack stops', expired, seconds=60)
            report['checks'].append('heartbeat_missing_after_stack_stop')
            report['status'] = 'passed'
        except Exception:
            report['status'] = 'failed'
            print(run(compose + ['ps'], check=False), flush=True)
            print(run(compose + ['logs', '--tail', '35'], check=False)[-18000:], flush=True)
            raise
        finally:
            run(compose + ['down', '--volumes', '--remove-orphans'], check=False)
            containers = run(['docker', 'ps', '-aq', '--filter', 'label=com.docker.compose.project=' + project])
            volumes = run(['docker', 'volume', 'ls', '-q', '--filter', 'label=com.docker.compose.project=' + project])
            if containers.strip() or volumes.strip():
                report['status'] = 'cleanup_failed'
                raise RuntimeError('Disposable project resources remain after cleanup')
finally:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
