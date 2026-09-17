#!/usr/bin/env python3
"""Owned native-test controller: real scrape, Grafana alert and OTLP recovery.

Linux host networking connects only explicitly leased loopback listeners. No
operator endpoint, published port, signing key or production configuration is used.
"""
import argparse
import base64
import hashlib
import http.server
import importlib.util
import json
import pathlib
import signal
import socket
import subprocess
import threading
import time
import urllib.parse
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)
IMAGES = json.loads((ROOT / 'images.lock.json').read_text())


def run(command, check=True):
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=90)
    if check and result.returncode:
        raise RuntimeError(result.stdout[-5000:])
    return result.stdout


def request(url, password=None):
    headers = {}
    if password:
        headers['Authorization'] = 'Basic ' + base64.b64encode(('admin:' + password).encode()).decode()
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=5) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory', type=pathlib.Path, required=True)
    parser.add_argument('--native-target', required=True)
    args = parser.parse_args()
    host, port = args.native_target.rsplit(':', 1)
    if host != '127.0.0.1' or not port.isdecimal() or not 0 < int(port) < 65536:
        parser.error('the owned native target must be IPv4 loopback with a nonzero port')
    directory = args.directory.resolve(strict=True)
    project = 'eez-native-loss-' + uuid.uuid4().hex[:10]
    report = {'status': 'failed', 'project': project, 'checks': [],
              'scope': 'actual native metrics and OTLP, real Prometheus/Grafana/local notification; test alert timings'}
    report['images'] = IMAGES
    inputs = [ROOT / 'prepare.py', pathlib.Path(__file__), ROOT / 'images.lock.json',
              ROOT / 'grafana/dashboards/infrastructure.json', *sorted((ROOT / 'runbooks').glob('*.md'))]
    report['source_sha256'] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                               for path in inputs}
    state = {'events': [], 'rejected': 0, 'accepted': 0, 'bytes': 0, 'error': None}
    lock = threading.Lock()
    owned = []
    stop = threading.Event()
    for signum in (signal.SIGTERM, signal.SIGINT):
        signal.signal(signum, lambda *_: stop.set())

    def marker(name, value):
        temporary = directory / (name + '.partial')
        temporary.write_text(json.dumps(value))
        temporary.replace(directory / (name + '.json'))

    class Handler(http.server.BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(3)

        def do_POST(self):
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 <= length <= 2 * 1024**2:
                with lock:
                    state['error'] = 'oversized collector request'
                self.send_error(413)
                return
            body = self.rfile.read(length)
            status = 200
            with lock:
                if self.path == '/v1/traces':
                    state['bytes'] += len(body)
                    if state['bytes'] > 32 * 1024**2:
                        state['error'] = 'collector byte budget exceeded'
                        status = 413
                    elif (directory / 'recover.json').exists():
                        state['accepted'] += 1
                    else:
                        state['rejected'] += 1
                        status = 503
                elif self.path == '/alerts':
                    if len(state['events']) >= 1000:
                        state['error'] = 'notification budget exceeded'
                        status = 429
                    else:
                        state['events'].append(json.loads(body))
                else:
                    status = 404
            self.send_response(status)
            self.send_header('Content-Type', 'application/x-protobuf')
            self.send_header('Content-Length', '0')
            self.end_headers()

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    receiver = f'127.0.0.1:{server.server_port}'

    def start(role, prefix, options, command=()):
        name = project + '-' + role
        image = next(key + '@' + value for key, value in IMAGES.items() if key.startswith(prefix))
        owned.append(name)
        marker('owned', {'project': project, 'containers': owned})
        run(['docker', 'run', '-d', '--name', name, '--network', 'host', '--cap-drop', 'ALL',
             '--security-opt', 'no-new-privileges:true', *options, image, *command])
        return name

    def until(label, callback, seconds=90):
        deadline = time.monotonic() + seconds
        last = None
        while time.monotonic() < deadline:
            if stop.is_set() or (directory / 'abort.json').exists():
                raise RuntimeError('native fixture stopped before ' + label)
            with lock:
                if state['error']:
                    raise RuntimeError(state['error'])
            try:
                value = callback()
                if value:
                    return value
            except (OSError, ValueError, KeyError) as error:
                last = str(error)
            time.sleep(.2)
        raise AssertionError(label + ' timed out: ' + str(last))

    leases = []
    try:
        for _ in range(2):
            lease = socket.socket()
            lease.bind(('127.0.0.1', 0))
            leases.append(lease)
        prom_port, grafana_port = [lease.getsockname()[1] for lease in leases]
        logs = directory / 'logs'
        logs.mkdir()
        config = {'environment': 'devnet', 'deployment': project, 'observer_instance': 'unused',
                  'observer_target': args.native_target, 'log_directory': str(logs),
                  'grafana_port': grafana_port,
                  'grafana_public_url': f'http://127.0.0.1:{grafana_port}/',
                  'alert_webhook_url': 'http://' + receiver + '/alerts',
                  'heartbeat_webhook_url': 'http://' + receiver + '/heartbeat',
                  'native_processes': [{'service': 'eez-composer', 'instance': 'fixture',
                      'metrics_target': args.native_target, 'metrics_listen': args.native_target,
                      'traces': {'receiver': receiver, 'sample_ratio': 1, 'queue_capacity': 128,
                                 'export_timeout_ms': 100}}]}
        runtime = prepare.prepare(config, directory / 'runtime', fast_test=True)
        name, _, _ = prepare.native_configs(config)[0]
        prom_file = runtime / 'prometheus/prometheus.yaml'
        prometheus = json.loads(prom_file.read_text())
        prometheus['scrape_configs'] = [job for job in prometheus['scrape_configs'] if job['job_name'] == name]
        prometheus.pop('rule_files', None)
        prepare.write_json(prom_file, prometheus)
        datasource = runtime / 'grafana/provisioning/datasources/eez.yaml'
        prepare.write_json(datasource, {'apiVersion': 1, 'datasources': [{'name': 'EEZ Prometheus',
            'uid': 'eez-prometheus', 'type': 'prometheus', 'access': 'proxy',
            'url': f'http://127.0.0.1:{prom_port}', 'isDefault': True}]})
        alerts_file = runtime / 'grafana/provisioning/alerting/eez.yaml'
        alerts = json.loads(alerts_file.read_text())
        rules = alerts['groups'][0]['rules']
        alerts['groups'][0]['rules'] = [rule for rule in rules if rule['uid'].startswith('eez-trace-native-')]
        assert len(alerts['groups'][0]['rules']) == 1
        rule = alerts['groups'][0]['rules'][0]
        title = rule['title']
        report['rule'] = rule
        prepare.write_json(alerts_file, alerts)
        password = (runtime / 'secrets/grafana-admin-password').read_text()
        leases[0].close()
        prometheus_container = start('prometheus', 'prom/prometheus:', ['--memory', '256m', '--cpus', '1',
            '--tmpfs', '/prometheus:rw,mode=1777,size=134217728', '--mount',
            f'type=bind,src={runtime / "prometheus"},dst=/etc/prometheus,readonly'],
            ['--config.file=/etc/prometheus/prometheus.yaml', '--storage.tsdb.path=/prometheus',
             '--storage.tsdb.retention.time=1h', f'--web.listen-address=127.0.0.1:{prom_port}'])
        leases[1].close()
        grafana_container = start('grafana', 'grafana/grafana:', ['--memory', '512m', '--cpus', '1',
            '--tmpfs', '/var/lib/grafana:rw,mode=1777,size=134217728', '--env', 'GOMEMLIMIT=384MiB',
            '--env', 'GF_SERVER_HTTP_ADDR=127.0.0.1', '--env', f'GF_SERVER_HTTP_PORT={grafana_port}',
            '--env', f'GF_SERVER_ROOT_URL=http://127.0.0.1:{grafana_port}/',
            '--env', 'GF_SECURITY_ADMIN_PASSWORD__FILE=/run/secrets/password',
            '--env', 'GF_ANALYTICS_REPORTING_ENABLED=false', '--env', 'GF_ANALYTICS_CHECK_FOR_UPDATES=false',
            '--env', 'GF_ANALYTICS_CHECK_FOR_PLUGIN_UPDATES=false',
            '--env', 'GF_PLUGINS_PREINSTALL_DISABLED=true', '--mount',
            f'type=bind,src={runtime / "secrets/grafana-admin-password"},dst=/run/secrets/password,readonly',
            '--mount', f'type=bind,src={runtime / "grafana/provisioning"},dst=/etc/grafana/provisioning,readonly',
            '--mount', f'type=bind,src={runtime / "grafana/dashboards"},dst=/var/lib/grafana/dashboards,readonly'])
        until('Grafana ready', lambda: request(f'http://127.0.0.1:{grafana_port}/api/health', password))
        until('native trace rule provisioned', lambda: request(
            f'http://127.0.0.1:{grafana_port}/api/v1/provisioning/alert-rules/{rule["uid"]}', password))
        report['checks'].append('actual_generated_native_loss_rule_and_private_backends_ready')
        marker('ready', {'config': str(runtime / 'native' / (name + '.json'))})

        def prom(expression):
            return request(f'http://127.0.0.1:{prom_port}/api/v1/query?' +
                           urllib.parse.urlencode({'query': expression}))['data']['result']

        def delivered(status):
            with lock:
                return any(alert.get('status') == status and alert.get('labels', {}).get('alertname') == title
                           and alert['labels'].get('deployment') == project
                           for event in state['events'] for alert in event.get('alerts', []))

        until('actual native scrape healthy', lambda: prom('up{job="' + name + '"} == 1'), seconds=180)
        until('native loss notification firing', lambda: delivered('firing'), seconds=180)
        drops = prom('sum(reth_eez_telemetry_dropped_events_total{signal="trace"})')
        assert drops and float(drops[0]['value'][1]) > 0
        report['drops_at_firing'] = float(drops[0]['value'][1])
        report['checks'].append('real_native_loss_counter_scraped_and_notification_delivered')
        marker('firing', {'drops': report['drops_at_firing']})
        until('native semantic gate permits collector recovery', lambda: (directory / 'recover.json').exists(), seconds=180)
        until('actual native export recovers', lambda: prom('reth_eez_trace_exported_spans_total > 0'))
        until('native loss recovery notification', lambda: delivered('resolved'))
        drops_after = prom('sum(reth_eez_telemetry_dropped_events_total{signal="trace"})')
        assert drops_after and float(drops_after[0]['value'][1]) >= report['drops_at_firing']
        assert prom('up{job="' + name + '"} == 1')
        with lock:
            assert state['rejected'] > 0 and state['accepted'] > 0 and state['error'] is None
            report['collector'] = {key: state[key] for key in ('rejected', 'accepted', 'bytes')}
            report['notifications'] = state['events']
        report['checks'].append('healthy_export_and_resolved_notification_without_counter_reset')
        origin = f'http://127.0.0.1:{grafana_port}'
        for event in report['notifications']:
            for alert in event.get('alerts', []):
                assert alert['labels']['alertname'] == title
                assert alert['annotations']['runbook_url'] == origin + '/d/eez-runbooks?viewPanel=1'
                assert alert['dashboardURL'].startswith(origin + '/d/eez-infrastructure?')
                assert alert['generatorURL'].startswith(origin + '/alerting/grafana/' + rule['uid'] + '/')
        runbooks = request(origin + '/api/dashboards/uid/eez-runbooks', password)['dashboard']
        assert any(panel['id'] == 1 and panel['type'] == 'text'
                   and 'NativeTelemetryUnavailable' in panel['options']['content']
                   for panel in runbooks['panels'])
        report['checks'].append('delivered_operator_links_match_actual_origin_and_provisioned_runbook')
        for container in (prometheus_container, grafana_container):
            info = json.loads(run(['docker', 'inspect', container]))[0]['State']
            assert info['Running'] and not info['OOMKilled'], info
        report['native_config_sha256'] = hashlib.sha256((runtime / 'native' / (name + '.json')).read_bytes()).hexdigest()
        report['status'] = 'passed'
        marker('resolved', {'drops': float(drops_after[0]['value'][1])})
        until('native test completed recovery checks', lambda: (directory / 'done.json').exists(), seconds=180)
    except BaseException as error:
        report.update(status='failed', error=str(error))
        marker('failed', {'error': str(error)})
        raise
    finally:
        cleanup_errors = []
        for container in reversed(owned):
            try:
                (directory / (container.rsplit('-', 1)[-1] + '.log')).write_text(run(['docker', 'logs', container], check=False))
            except Exception as error:
                cleanup_errors.append(str(error))
            try:
                run(['docker', 'rm', '--force', '--volumes', container])
            except Exception as error:
                cleanup_errors.append(str(error))
        try:
            remaining = set(run(['docker', 'ps', '--all', '--format', '{{.Names}}']).splitlines()) & set(owned)
            if remaining:
                cleanup_errors.append('owned containers remain: ' + ', '.join(sorted(remaining)))
        except Exception as error:
            cleanup_errors.append(str(error))
        finally:
            for lease in leases:
                lease.close()
            server.shutdown()
            server.server_close()
            report['owned_resource_cleanup'] = 'failed' if cleanup_errors else 'verified_absent'
            if cleanup_errors:
                report.update(status='failed', cleanup_errors=cleanup_errors)
            (directory / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        if cleanup_errors:
            raise RuntimeError('owned alert fixture cleanup failed')


if __name__ == '__main__':
    main()
