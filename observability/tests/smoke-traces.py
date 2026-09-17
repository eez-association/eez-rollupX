#!/usr/bin/env python3
"""Exercise the optional trace profile using an isolated Compose project only."""
import argparse
import base64
import datetime
import importlib.util
import json
import pathlib
import re
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


def run(command, check=True):
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if check and result.returncode:
        raise RuntimeError(result.stdout[-8000:])
    return result.stdout


def request(url, data=None, password=None, raw=False):
    headers = {'Accept': 'application/json'}
    if password:
        headers['Authorization'] = 'Basic ' + base64.b64encode(('admin:' + password).encode()).decode()
    body = None
    if data is not None:
        headers['Content-Type'] = 'application/json'
        body = json.dumps(data).encode()
    with urllib.request.urlopen(urllib.request.Request(url, data=body, headers=headers), timeout=5) as response:
        body = response.read()
        return body.decode() if raw else json.loads(body) if body else None


def until(label, predicate, seconds=100):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                print('PASS:', label, flush=True)
                return value
        except (OSError, ValueError, KeyError, IndexError, AssertionError) as error:
            last = repr(error)
        time.sleep(1)
    raise AssertionError(f'{label}: timeout; last={last}')


def trace_payload(trace_id, deployment, count=2):
    now = time.time_ns()
    resources = []
    for index in range(count):
        service = 'eez-composer' if index % 2 == 0 else 'eez-proof-signer'
        attributes = {'service.name': service, 'service.instance.id': 'fixture',
                      'deployment.environment.name': 'devnet', 'eez.deployment': deployment}
        span = {'traceId': trace_id, 'spanId': f'{index + 1:016x}',
                'name': 'composer_proving' if index == 0 else 'prove',
                'kind': 1 if index == 0 else 2,
                'startTimeUnixNano': str(now), 'endTimeUnixNano': str(now + 1000000),
                'attributes': [{'key': 'fixture', 'value': {'boolValue': True}}]}
        if index:
            span['parentSpanId'] = '0000000000000001'
        resources.append({'resource': {'attributes': [{'key': key, 'value': {'stringValue': value}} for key, value in attributes.items()]},
                          'scopeSpans': [{'scope': {'name': 'eez-test'}, 'spans': [span]}]})
    return {'resourceSpans': resources}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    project = 'eez-trace-test-' + uuid.uuid4().hex[:10]
    images = json.loads((ROOT / 'images.lock.json').read_text())
    report = {'project': project, 'timestamp': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'scope': 'isolated OTLP fixtures; no native devnet or operator service changes', 'images': images, 'checks': []}
    compose = None
    try:
        with tempfile.TemporaryDirectory(prefix=project + '-') as temporary:
            directory = pathlib.Path(temporary)
            logs = directory / 'logs'
            logs.mkdir()
            config = {'environment': 'devnet', 'deployment': project, 'observer_instance': 'fixture',
                      'observer_target': 'fixture:9000', 'log_directory': str(logs),
                      'alert_webhook_url': 'http://fixture:9000/alerts',
                      'heartbeat_webhook_url': 'http://fixture:9000/heartbeat',
                      'traces_enabled': True, 'grafana_port': 0, 'prometheus_port': 0,
                      'loki_port': 0, 'tempo_port': 0, 'otlp_http_port': 0}
            runtime = prepare.prepare(config, directory / 'runtime', fast_test=True)
            fixture_images = json.loads((ROOT / 'tests/images.lock.json').read_text())
            fixture_image = next(name + '@' + digest for name, digest in fixture_images.items())
            override = runtime / 'trace-test.json'
            override.write_text(json.dumps({'services': {
                'alloy-traces': {'ports': ['127.0.0.1:0:12345']},
                'fixture': {'image': fixture_image, 'command': ['python3', '/fixture.py'],
                            'ports': ['127.0.0.1:0:9000'],
                            'volumes': [str(ROOT / 'tests/fixture-server.py') + ':/fixture.py:ro'],
                            'mem_limit': '128m', 'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true']}
            }}))
            compose = ['docker', 'compose', '--project-name', project, '--profile', 'traces',
                       '--env-file', str(runtime / 'compose.env'), '-f', str(ROOT / 'compose.yaml'), '-f', str(override)]
            password = (runtime / 'secrets/grafana-admin-password').read_text()
            try:
                run(compose + ['config', '--quiet'])
                tempo_image = next(name + '@' + digest for name, digest in images.items() if name.startswith('grafana/tempo:'))
                run(['docker', 'run', '--rm', '--network', 'none', '-v', f'{ROOT / "tempo/config.yaml"}:/etc/tempo/config.yaml:ro',
                     tempo_image, '-config.file=/etc/tempo/config.yaml', '-config.verify=true'])
                alloy_image = next(name + '@' + digest for name, digest in images.items() if name.startswith('grafana/alloy:'))
                run(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', '/bin/alloy',
                     '-v', f'{runtime / "alloy/traces.alloy"}:/etc/alloy/traces.alloy:ro', alloy_image, 'validate', '/etc/alloy/traces.alloy'])
                report['checks'].append('pinned_compose_tempo_and_alloy_config')
                run(compose + ['up', '-d', '--wait', '--wait-timeout', '150'])

                def base(service, port):
                    return 'http://' + run(compose + ['port', service, str(port)]).strip()
                tempo, alloy = base('tempo', 3200), base('alloy-traces', 4318)
                collector_metrics = base('alloy-traces', 12345)
                grafana, loki, prometheus = base('grafana', 3000), base('loki', 3100), base('prometheus', 9090)
                fixture = base('fixture', 9000)
                def notification(title, status):
                    return any(event['path'] == '/alerts' and any(alert.get('labels', {}).get('alertname') == title and alert.get('status') == status
                        for alert in event['data'].get('alerts', [])) for event in request(fixture + '/events'))
                until('Tempo ready', lambda: request(tempo + '/ready', raw=True).strip() == 'ready')
                until('Alloy trace collector ready', lambda: request(collector_metrics + '/-/ready', raw=True))
                effective = request(tempo + '/status/config', raw=True)
                assert re.search(r'block_retention:\s*168h', effective), 'seven-day retention must reach Tempo'
                report['checks'].append('effective_seven_day_local_retention')
                source = request(grafana + '/api/datasources/uid/eez-tempo', password=password)
                assert source['jsonData']['tracesToLogsV2']['datasourceUid'] == 'eez-loki'
                log_source = request(grafana + '/api/datasources/uid/eez-loki', password=password)
                derived = log_source['jsonData']['derivedFields'][0]
                assert derived['datasourceUid'] == 'eez-tempo'
                assert derived['url'] == '${__value.raw}'
                assert 'unavailable' in derived['urlDisplayLabel']
                report['checks'].append('grafana_bidirectional_links_provisioned')

                trace_id = uuid.uuid4().hex
                request(alloy + '/v1/traces', trace_payload(trace_id, project))
                trace = until('OTLP spans queryable in Tempo through Grafana', lambda: request(grafana + '/api/datasources/proxy/uid/eez-tempo/api/traces/' + trace_id, password=password))
                assert 'composer_proving' in json.dumps(trace) and 'eez-proof-signer' in json.dumps(trace)
                line = json.dumps({'level': 'INFO', 'trace_id': trace_id, 'message': 'trace-backend-fixture'})
                assert re.search(derived['matcherRegex'], line).group(1) == trace_id
                (logs / 'trace.jsonl').write_text(line + '\n')
                def log_query():
                    query = '{environment="devnet",deployment="' + project + '"} |= "' + trace_id + '"'
                    return request(loki + '/loki/api/v1/query_range?' + urllib.parse.urlencode({'query': query, 'limit': 10}))['data']['result']
                until('trace identity finds original JSON through Alloy and Loki', log_query)
                report['checks'].append('otlp_ingestion_grafana_trace_query_and_loki_identity')
                def prom(expr):
                    return request(prometheus + '/api/v1/query?' + urllib.parse.urlencode({'query': expr}))['data']['result']
                until('trace backends are scraped', lambda: len(prom('up{job=~"tempo|alloy-traces"} == 1')) == 2)

                run(compose + ['stop', 'tempo'])
                queued = uuid.uuid4().hex
                request(alloy + '/v1/traces', trace_payload(queued, project))
                run(compose + ['start', 'tempo'])
                tempo = base('tempo', 3200)  # Docker can reassign ephemeral host ports on start.
                until('queued spans recover after brief Tempo outage', lambda: request(tempo + '/api/traces/' + queued))
                report['checks'].append('bounded_collector_queue_recovers_brief_outage')

                run(compose + ['stop', 'tempo'])
                # More items than the fixed collector queue. Rejection is visible,
                # and these diagnostic spans are intentionally not retried here.
                status = None
                try:
                    request(alloy + '/v1/traces', trace_payload(uuid.uuid4().hex, project, 2200))
                except urllib.error.HTTPError as error:
                    status = error.code
                assert status is not None and status >= 400, 'oversized queue admission must fail explicitly'
                metrics = until('collector refusal/drop counter visible', lambda: request(collector_metrics + '/metrics', raw=True))
                counters = [line for line in metrics.splitlines() if not line.startswith('#') and re.search(r'(enqueue_failed|refused)_spans', line)]
                assert any(float(line.rsplit(' ', 1)[1]) > 0 for line in counters), counters
                report['drop_metric_names'] = sorted({line.split('{', 1)[0].split(' ', 1)[0] for line in counters})
                request(alloy + '/v1/traces', trace_payload(uuid.uuid4().hex, project))
                def exhausted():
                    metrics = request(collector_metrics + '/metrics', raw=True)
                    return any(not line.startswith('#') and 'send_failed_spans' in line and float(line.rsplit(' ', 1)[1]) > 0 for line in metrics.splitlines())
                until('collector retries expire with visible send failure', exhausted, seconds=35)
                until('trace loss alert delivered', lambda: notification('EEZTraceCollectionLoss', 'firing'))
                until('Tempo backend outage alert delivered', lambda: notification('EEZBackendUnavailable_tempo', 'firing'))
                run(compose + ['start', 'tempo'])
                tempo = base('tempo', 3200)  # Docker can reassign ephemeral host ports on start.
                until('Tempo resumes after outage', lambda: request(tempo + '/ready', raw=True).strip() == 'ready')
                recovered = uuid.uuid4().hex
                request(alloy + '/v1/traces', trace_payload(recovered, project))
                until('fresh trace delivery resumes after loss', lambda: request(tempo + '/api/traces/' + recovered))
                until('Tempo outage recovery delivered', lambda: notification('EEZBackendUnavailable_tempo', 'resolved'))
                until('trace loss recovery delivered after quiet window', lambda: notification('EEZTraceCollectionLoss', 'resolved'))
                report['checks'].append('trace_loss_and_backend_alert_delivery_recovery')
                report['checks'].append('queue_overflow_retry_exhaustion_and_recovery')

                run(compose + ['up', '-d', '--force-recreate', '--no-deps', 'tempo'])
                tempo = base('tempo', 3200)
                until('Tempo trace survives container replacement', lambda: request(tempo + '/api/traces/' + trace_id))
                assert log_query()
                missing = uuid.uuid4().hex
                try:
                    request(tempo + '/api/traces/' + missing)
                    raise AssertionError('missing trace must not report a populated trace')
                except urllib.error.HTTPError as error:
                    assert error.code == 404
                report['checks'].append('persistent_trace_storage_and_explicit_unavailable_result')
                report['status'] = 'passed'
            except Exception:
                report['status'] = 'failed'
                diagnostic = run(compose + ['logs', '--no-color', '--tail', '100'], check=False)
                args.report.with_suffix('.logs.txt').write_text(diagnostic)
                print(diagnostic[-14000:], flush=True)
                raise
            finally:
                run(compose + ['down', '--volumes', '--remove-orphans'], check=False)
                for command in [['docker', 'ps', '-aq'], ['docker', 'volume', 'ls', '-q']]:
                    remaining = run(command + ['--filter', 'label=com.docker.compose.project=' + project])
                    if remaining.strip():
                        report['status'] = 'cleanup_failed'
                        raise RuntimeError('owned disposable resources remain')
    finally:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
