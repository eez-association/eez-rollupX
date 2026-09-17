#!/usr/bin/env python3
"""Generate isolated deployment configuration. No network calls or service changes."""
import argparse
import hashlib
import ipaddress
import json
import math
import pathlib
import re
import secrets
import shutil
from urllib.parse import quote, urlsplit

ROOT = pathlib.Path(__file__).resolve().parent
LABEL = re.compile(r'[a-zA-Z0-9][a-zA-Z0-9_.-]{0,62}\Z')
TARGET = re.compile(r'(?:[a-zA-Z0-9_.-]+|\[[a-fA-F0-9:]+\]):([0-9]{1,5})\Z')


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + '\n')


def native_configs(config):
    processes = config.get('native_processes', [])
    if not isinstance(processes, list) or len(processes) > 16:
        raise ValueError('native_processes must be a list of at most 16 processes')
    result, identities, targets = [], set(), set()
    for process in processes:
        required = {'service', 'instance', 'metrics_target', 'metrics_listen'}
        if not isinstance(process, dict) or required - set(process) or set(process) - required - {'allow_non_loopback', 'lifecycle_directory', 'traces'}:
            raise ValueError('missing or unknown native process fields')
        service, instance = process['service'], process['instance']
        if service not in ('eez-composer', 'eez-follower', 'eez-proof-signer') or not isinstance(instance, str) or not LABEL.fullmatch(instance):
            raise ValueError('invalid native service or instance')
        identity = (service, instance)
        target = process['metrics_target']
        match = TARGET.fullmatch(target) if isinstance(target, str) else None
        if not match or not 0 < int(match[1]) < 65536 or 'REPLACE' in target:
            raise ValueError('native metrics target must be explicit host:port')
        if identity in identities or target in targets:
            raise ValueError('duplicate native identity or metrics target')
        identities.add(identity)
        targets.add(target)
        listen = process['metrics_listen']
        try:
            host, port = listen.rsplit(':', 1)
            address = ipaddress.ip_address(host.strip('[]'))
            valid = port.isascii() and port.isdigit() and 0 < int(port) < 65536 and (
                address.version == 4 and host == str(address) or address.version == 6 and host.startswith('[') and host.endswith(']'))
        except (AttributeError, ValueError):
            valid = False
        private_override = process.get('allow_non_loopback', False)
        if not valid or type(private_override) is not bool or (not address.is_loopback and not private_override):
            raise ValueError('native listener must be an IP:port; non-loopback requires explicit opt-in')
        value = {'identity': {'environment': config['environment'], 'deployment': config['deployment'], 'service': service, 'instance': instance},
                 'metrics': {'listen': listen, 'allow_non_loopback': private_override}, 'lifecycle': None}
        spool = process.get('lifecycle_directory')
        if spool is not None:
            if not isinstance(spool, str) or not pathlib.PurePosixPath(spool).is_absolute() or any(c in spool for c in '\x00\n\r') or 'REPLACE' in spool:
                raise ValueError('lifecycle_directory must be an explicit absolute node-host path')
            value['lifecycle'] = {'directory': spool}
        if 'traces' in process:
            traces = process['traces']
            required_trace = {'receiver', 'sample_ratio'}
            if not isinstance(traces, dict) or required_trace - set(traces) or set(traces) - required_trace - {'allow_non_loopback', 'queue_capacity', 'export_timeout_ms'}:
                raise ValueError('missing or unknown native tracing fields')
            try:
                host, port = traces['receiver'].rsplit(':', 1)
                address = ipaddress.ip_address(host.strip('[]'))
                opt_in = traces.get('allow_non_loopback', False)
                bracketed = host.startswith('[') and host.endswith(']')
                if ((address.version == 6) != bracketed or not port.isdecimal() or type(opt_in) is not bool
                        or not 0 < int(port) < 65536 or (not address.is_loopback and not opt_in)):
                    raise ValueError('invalid tracing receiver')
            except (ValueError, TypeError, AttributeError):
                raise ValueError('native tracing receiver requires explicit IP:port and non-loopback opt-in') from None
            ratio = traces['sample_ratio']
            if type(ratio) not in (int, float) or not math.isfinite(ratio) or not 0 <= ratio <= 1:
                raise ValueError('trace sample_ratio must be between zero and one')
            for key, default, upper in [('queue_capacity', 2048, 16384), ('export_timeout_ms', 1000, 2000)]:
                setting = traces.get(key, default)
                if type(setting) is not int or not 1 <= setting <= upper:
                    raise ValueError('native trace queue or timeout outside supported bounds')
            value['traces'] = traces.copy()
        result.append((f'{service}-{instance}', target, value))
    return result


def validate(config):
    required = {'environment', 'deployment', 'observer_instance', 'observer_target', 'log_directory',
                'alert_webhook_url', 'heartbeat_webhook_url'}
    allowed = required | {'node_exporter_target', 'public_rpc_url', 'signer_tcp_target',
                          'probe_exporter_target', 'grafana_port', 'prometheus_port', 'loki_port', 'native_processes', 'database_enabled', 'postgres_port', 'traces_enabled', 'tempo_port', 'otlp_http_port', 'explorer_urls'}
    if set(config) - allowed or required - set(config):
        raise ValueError('missing or unknown stack configuration fields')
    for key in ('environment', 'deployment', 'observer_instance'):
        if not isinstance(config[key], str) or not LABEL.fullmatch(config[key]):
            raise ValueError('invalid deployment label')
    for key in ('observer_target', 'node_exporter_target', 'signer_tcp_target', 'probe_exporter_target'):
        value = config.get(key)
        if value is None and key == 'observer_target':
            raise ValueError('observer_target is required')
        if value is not None:
            match = TARGET.fullmatch(value) if isinstance(value, str) else None
            if not match or not 0 < int(match[1]) < 65536 or 'REPLACE' in value:
                raise ValueError('targets must be explicit private host:port values')
    for key in ('alert_webhook_url', 'heartbeat_webhook_url', 'public_rpc_url'):
        value = config.get(key)
        if value is None and key != 'public_rpc_url':
            raise ValueError('operator and independent heartbeat receivers are required')
        if value is not None:
            try:
                parsed = urlsplit(value)
                valid = parsed.scheme in ('http', 'https') and parsed.hostname and not parsed.fragment and 'REPLACE' not in value
                if key == 'public_rpc_url' and (parsed.username or parsed.password or parsed.query):
                    valid = False  # Probe targets are logged by the upstream probe process.
            except (ValueError, TypeError):
                valid = False
            if not valid:
                raise ValueError('invalid webhook/probe URL')
    if type(config.get('database_enabled', False)) is not bool:
        raise ValueError('database_enabled must be a boolean')
    if 'postgres_port' in config and not config.get('database_enabled', False):
        raise ValueError('postgres_port requires database_enabled')
    for key in ('grafana_port', 'prometheus_port', 'loki_port', 'postgres_port'):
        value = config.get(key, 0)
        if type(value) is not int or not 0 <= value <= 65535 or (key == 'postgres_port' and key in config and value == 0):
            raise ValueError('ports must be integers between 0 and 65535')
    path = pathlib.Path(config['log_directory']).resolve(strict=True)
    if not path.is_dir() or 'REPLACE' in str(path):
        raise ValueError('log_directory must be an existing dedicated directory')
    if type(config.get('traces_enabled', False)) is not bool:
        raise ValueError('traces_enabled must be boolean')
    if not config.get('traces_enabled', False) and any(key in config for key in ('tempo_port', 'otlp_http_port')):
        raise ValueError('trace ports require traces_enabled=true')
    for key in ('tempo_port', 'otlp_http_port'):
        if key in config and (type(config[key]) is not int or not 0 <= config[key] <= 65535):
            raise ValueError('trace ports must be integers between zero and 65535')
    explorers = config.get('explorer_urls', {})
    if not isinstance(explorers, dict) or set(explorers) - {'l1', 'l2'}:
        raise ValueError('explorer_urls must contain only l1/l2 base URLs')
    for value in explorers.values():
        if not isinstance(value, str):
            raise ValueError('explorer URLs must be strings')
        try:
            parsed = urlsplit(value)
            port = parsed.port
            valid = isinstance(value, str) and parsed.scheme in ('http', 'https') and parsed.hostname and not (parsed.username or parsed.password or parsed.query or parsed.fragment) and not any(c.isspace() or c in '\x00$' for c in value) and (port is None or 0 < port < 65536) and 'REPLACE' not in value
        except (ValueError, TypeError):
            valid = False
        if not valid:
            raise ValueError('explorer URLs must be explicit HTTP(S) bases without credentials, query or fragment')
    native_configs(config)  # Validate all generated process configs before writing anything.
    return path


def prepare(config, runtime, *, fast_test=False):
    logs = validate(config)
    runtime = pathlib.Path(runtime).resolve()
    # .env values are paths, never executable shell text or interpolated secrets.
    if any(c in str(runtime) + str(logs) for c in '\n\r$"\\'):
        raise ValueError('runtime/log paths cannot contain environment-file metacharacters')
    runtime.mkdir(parents=True, exist_ok=True)
    runtime.chmod(0o700)
    secret = runtime / 'secrets/grafana-admin-password'
    secret.parent.mkdir(exist_ok=True)
    if not secret.exists():
        with secret.open('x') as stream:
            stream.write(secrets.token_urlsafe(32))
        # Parent runtime is 0700; Grafana's container UID must read the individual mount.
        secret.chmod(0o644)
    labels = {'environment': config['environment'], 'deployment': config['deployment']}
    interval = '5s' if fast_test else '15s'
    scrape = []
    for job, target in [('prometheus', 'prometheus:9090'), ('grafana', 'grafana:3000'),
                        ('loki', 'loki:3100'), ('alloy', 'alloy:12345'), ('blackbox', 'blackbox:9115')]:
        scrape.append({'job_name': job, 'static_configs': [{'targets': [target], 'labels': {**labels, 'service': job}}]})
    if config.get('traces_enabled', False):
        for job, target in [('tempo', 'tempo:3200'), ('alloy-traces', 'alloy-traces:12345')]:
            scrape.append({'job_name': job, 'static_configs': [{'targets': [target], 'labels': {**labels, 'service': job}}]})
    scrape.append({'job_name': 'eez-observer', 'static_configs': [{'targets': [config['observer_target']],
                   'labels': {**labels, 'service': 'eez-observer', 'instance': config['observer_instance']}}]})
    for name, target, native in native_configs(config):
        write_json(runtime / f'native/{name}.json', native)
        scrape.append({'job_name': name, 'static_configs': [{'targets': [target], 'labels': native['identity']}]})
    if config.get('node_exporter_target'):
        scrape.append({'job_name': 'node-exporter', 'static_configs': [{'targets': [config['node_exporter_target']],
                       'labels': {**labels, 'service': 'node-exporter'}}]})
    for job, key, module in [('public-rpc', 'public_rpc_url', 'rpc_http'), ('signer-listener', 'signer_tcp_target', 'signer_tcp')]:
        if config.get(key):
            scrape.append({'job_name': job, 'metrics_path': '/probe', 'params': {'module': [module]},
                           'static_configs': [{'targets': [config[key]], 'labels': {**labels, 'service': job}}],
                           'relabel_configs': [
                               {'source_labels': ['__address__'], 'target_label': '__param_target'},
                               # Never expose credential-bearing probe URLs as instance labels.
                               {'target_label': 'instance', 'replacement': job},
                               {'target_label': '__address__', 'replacement': (config.get('probe_exporter_target') or 'blackbox:9115')}]})
    write_json(runtime / 'prometheus/prometheus.yaml', {'global': {'scrape_interval': interval,
        'scrape_timeout': '4s' if fast_test else '10s', 'evaluation_interval': interval},
        'rule_files': ['/etc/prometheus/recording-rules.yaml'], 'scrape_configs': scrape})
    shutil.copyfile(ROOT / 'prometheus/recording-rules.yaml', runtime / 'prometheus/recording-rules.yaml')
    provision = runtime / 'grafana/provisioning'
    datasources = [
        {'name': 'EEZ Prometheus', 'uid': 'eez-prometheus', 'type': 'prometheus', 'access': 'proxy',
         'url': 'http://prometheus:9090', 'isDefault': True, 'editable': False, 'jsonData': {'httpMethod': 'POST', 'timeInterval': interval}},
        {'name': 'EEZ Logs', 'uid': 'eez-loki', 'type': 'loki', 'access': 'proxy',
         'url': 'http://loki:3100', 'editable': False}]
    if config.get('traces_enabled', False):
        datasources[1]['jsonData'] = {'derivedFields': [{
            'name': 'TraceID', 'matcherRegex': r'"trace_id"\s*:\s*"([0-9a-f]{32})"',
            'datasourceUid': 'eez-tempo', 'url': '$${__value.raw}',
            'urlDisplayLabel': 'View trace (optional; may be unavailable)'}]}
        datasources.append({'name': 'EEZ Traces', 'uid': 'eez-tempo', 'type': 'tempo', 'access': 'proxy',
            'url': 'http://tempo:3200', 'editable': False, 'jsonData': {'httpMethod': 'GET',
            'tracesToLogsV2': {'datasourceUid': 'eez-loki', 'spanStartTimeShift': '-1m', 'spanEndTimeShift': '1m',
                'tags': [{'key': 'deployment.environment.name', 'value': 'environment'},
                         {'key': 'eez.deployment', 'value': 'deployment'}],
                'filterByTraceID': True, 'filterBySpanID': False}, 'nodeGraph': {'enabled': False}}})
    if config.get('database_enabled', False):
        prepare_database(config, runtime, datasources)
    write_json(provision / 'datasources/eez.yaml', {'apiVersion': 1, 'prune': True, 'datasources': datasources})
    write_json(provision / 'dashboards/eez.yaml', {'apiVersion': 1, 'providers': [{'name': 'eez', 'orgId': 1,
        'folder': 'EEZ', 'type': 'file', 'disableDeletion': False, 'updateIntervalSeconds': 10,
        'allowUiUpdates': False, 'options': {'path': '/var/lib/grafana/dashboards'}}]})
    prepare_dashboards(config, runtime)
    alerts = alert_rules(config, fast_test=fast_test)
    # Grafana expands $ in provisioning values, including URLs. Escape literal dollars.
    literal = lambda value: value.replace('$', '$$')
    write_json(provision / 'alerting/eez.yaml', {'apiVersion': 1, 'groups': alerts, 'contactPoints': [
        {'orgId': 1, 'name': 'eez-operator', 'receivers': [{'uid': 'eez-operator-webhook', 'type': 'webhook',
         'settings': {'url': literal(config['alert_webhook_url']), 'httpMethod': 'POST'}, 'disableResolveMessage': False}]},
        {'orgId': 1, 'name': 'eez-heartbeat', 'receivers': [{'uid': 'eez-heartbeat-webhook', 'type': 'webhook',
         'settings': {'url': literal(config['heartbeat_webhook_url']), 'httpMethod': 'POST'}, 'disableResolveMessage': True}]}],
        'policies': [{'orgId': 1, 'receiver': 'eez-operator', 'group_by': ['alertname', 'deployment'],
                     'group_wait': '1s' if fast_test else '15s', 'group_interval': '10s' if fast_test else '1m',
                     'repeat_interval': '10s' if fast_test else '4h', 'routes': [
                         {'receiver': 'eez-heartbeat', 'object_matchers': [['alertname', '=', 'EEZHeartbeat']],
                          'group_wait': '0s', 'group_interval': '10s' if fast_test else '1m',
                          'repeat_interval': '10s' if fast_test else '1m'}]}]})
    # Static paths only: applications write JSON into this dedicated directory.
    alloy = '''logging { level = "info" }
local.file_match "eez" {
  path_targets = [{ "__path__" = "/var/log/eez/*.jsonl", "job" = "eez-logs", "environment" = ENV, "deployment" = DEP, "service" = "eez", "instance" = INST }]
  sync_period = "5s"
}
loki.source.file "eez" {
  targets = local.file_match.eez.targets
  forward_to = [loki.relabel.eez.receiver]
  tail_from_end = false
}
loki.relabel "eez" {
  forward_to = [loki.write.local.receiver]
  rule {
    action = "labeldrop"
    regex = "filename"
  }
}
loki.write "local" {
  endpoint { url = "http://loki:3100/loki/api/v1/push" }
}
'''
    for token, value in [('ENV', config['environment']), ('DEP', config['deployment']), ('INST', config['observer_instance'])]:
        alloy = alloy.replace(token, json.dumps(value))
    (runtime / 'alloy').mkdir(exist_ok=True)
    (runtime / 'alloy/config.alloy').write_text(alloy)
    trace_config = runtime / 'alloy/traces.alloy'
    if config.get('traces_enabled', False):
        shutil.copyfile(ROOT / 'alloy/traces.alloy', trace_config)
    elif trace_config.exists():
        trace_config.unlink()

    ports = {key: config.get(key, default) for key, default in [('grafana_port', 19300), ('prometheus_port', 19090), ('loki_port', 19310), ('postgres_port', 19432), ('tempo_port', 19320), ('otlp_http_port', 14318)]}
    environment = f'OBS_RUNTIME="{runtime}"\nOBS_LOG_DIR="{logs}"\n' + ''.join(f'{key.upper()}={value}\n' for key, value in ports.items())
    (runtime / 'compose.env').write_text(environment)
    (runtime / 'compose.env').chmod(0o600)
    write_json(runtime / 'manifest.json', {'environment': config['environment'], 'deployment': config['deployment'],
        'observer_instance': config['observer_instance'], 'test_timings': fast_test,
        'native_processes': [name for name, _, _ in native_configs(config)],
        'probes_independent': (config.get('probe_exporter_target') or 'blackbox:9115') != 'blackbox:9115',
        'database_enabled': config.get('database_enabled', False),
        'traces_enabled': config.get('traces_enabled', False),
        'node_metrics_configured': bool(config.get('node_exporter_target'))})
    return runtime


def prepare_dashboards(config, runtime):
    """Provision only supported profiles, with optional links from explicit config."""
    destination = runtime / 'grafana/dashboards'
    destination.mkdir(parents=True, exist_ok=True)
    names = ['network.json', 'infrastructure.json']
    if config.get('database_enabled', False):
        names += ['cross-chain.json', 'settlement.json', 'blobs.json', 'gas.json']
    # Delete only this generator's known dashboard files on profile downgrade.
    for name in ['network.json', 'infrastructure.json', 'cross-chain.json', 'settlement.json', 'blobs.json', 'gas.json']:
        path = destination / name
        if name not in names and path.exists():
            path.unlink()
    explorers = config.get('explorer_urls', {})
    for name in names:
        dashboard = json.loads((ROOT / 'grafana/dashboards' / name).read_text())
        for panel in dashboard['panels']:
            if panel['title'].startswith('Request timeline'):
                panes = {'logs': {'datasource': 'eez-loki', 'queries': [{'refId': 'A',
                    'expr': '{environment="$environment",deployment="$deployment"} |= "$transaction"'}],
                    'range': {'from': '$__from', 'to': '$__to'}}}
                url = '/explore?schemaVersion=1&panes=' + quote(json.dumps(panes), safe='')
                for token, replacement in {'environment': '${environment:percentencode}',
                    'deployment': '${deployment:percentencode}', 'transaction': '${__value.raw}',
                    '__from': '${__from}', '__to': '${__to}'}.items():
                    url = url.replace('%24' + token, replacement)
                for override in panel['fieldConfig']['overrides']:
                    if override['matcher']['options'] == 'posting_transaction':
                        override['properties'][0]['value'].append({'title': 'Diagnostic logs (may be unavailable)',
                            'url': url, 'targetBlank': True})
            for target in panel.get('targets', []):
                if 'rawSql' not in target:
                    continue
                for chain in ('l1', 'l2'):
                    base = explorers.get(chain)
                    expression = "'" + base.rstrip('/').replace("'", "''") + "/tx/0x'||encode(r.transaction_hash,'hex')" if base else 'NULL::text'
                    target['rawSql'] = target['rawSql'].replace('__EEZ_' + chain.upper() + '_EXPLORER_SQL__', expression)
            if panel['title'].startswith('Receipt detail') and explorers:
                panel['fieldConfig']['overrides'].append({'matcher': {'id': 'byName', 'options': 'explorer_url'},
                    'properties': [{'id': 'links', 'value': [{'title': 'Canonical source-chain explorer', 'url': '${__value.raw}', 'targetBlank': True}]}]})
            if panel['title'].startswith('Retained trace references') and config.get('traces_enabled', False):
                panel['fieldConfig']['overrides'].append({'matcher': {'id': 'byName', 'options': 'trace_id'},
                    'properties': [{'id': 'links', 'value': [{'title': 'Trace (optional; may be unavailable)',
                        'url': '/explore?schemaVersion=1&panes={"trace":{"datasource":"eez-tempo","queries":[{"refId":"A","queryType":"traceId","query":"${__value.raw}"}]}}', 'targetBlank': True}]}]})
        write_json(destination / name, dashboard)


def prepare_database(config, runtime, datasources):
    passwords = {}
    for role in ('admin', 'writer', 'reader'):
        path = runtime / f'secrets/postgres-{role}-password'
        if not path.exists():
            with path.open('x') as stream:
                stream.write(secrets.token_urlsafe(32))
            # The private 0700 runtime encloses these individual container mounts.
            path.chmod(0o644)
        value = path.read_text()
        if not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', value):
            raise ValueError('invalid existing generated database password; restore the original secret file')
        passwords[role] = value
    for role in ('admin', 'writer'):
        user = 'postgres' if role == 'admin' else 'eez_observer_writer'
        path = runtime / f'database/{role}.json'
        write_json(path, {'connection_string': f"host=127.0.0.1 port={config.get('postgres_port',19432)} user={user} dbname=eez_observer password={passwords[role]} sslmode=disable",
                          'allow_private_plaintext': True})
        path.chmod(0o600)
    datasources.append({'name': 'EEZ Canonical ledger', 'uid': 'eez-postgres', 'type': 'postgres', 'access': 'proxy',
        'url': 'postgres:5432', 'user': 'eez_observer_reader', 'editable': False,
        'jsonData': {'database': 'eez_observer', 'sslmode': 'disable', 'postgresVersion': 1800,
                     'maxOpenConns': 5, 'maxIdleConns': 2, 'connMaxLifetime': 300, 'timescaledb': False},
        'secureJsonData': {'password': passwords['reader']}})


def alert_rules(config, *, fast_test=False):
    trace_window = '30s' if fast_test else '5m'
    labels = {'environment': config['environment'], 'deployment': config['deployment']}
    scope = ','.join(f'{k}="{v}"' for k, v in labels.items())
    observer = scope + f',job="eez-observer",instance="{config["observer_instance"]}"'
    up = f'up{{{observer}}}'
    specs = [
        ('eez-observer-down', 'EEZObserverUnavailable', f'(1 - {up}) or absent({up})', 'Alerting', 'critical',
         'Observer scrape is unavailable; inspect its process, private listener and network path.'),
        ('eez-queue-stale', 'EEZQueueEvaluationStale',
         f'(1 - eez_observer_queue_evaluation_valid{{{scope}}}) and on(environment,deployment,job,instance) ({up} == 1)',
         'OK', 'warning', 'Queue RPC may respond while readiness evaluation is stale or unknown.'),
        ('eez-heartbeat', 'EEZHeartbeat', 'vector(1)', 'Alerting', 'heartbeat',
         'Independent receiver must alarm after three missing one-minute heartbeats. This rule cannot detect its own host outage.'),
    ]
    for source in ['l1', 'l2', 'queue']:
        metric = f'eez_observer_collection_success{{{observer},source="{source}"}}'
        specs.append((f'eez-source-{source}', f'EEZCollectionUnavailable_{source}',
                      f'((1 - {metric}) or absent({metric})) and on(environment,deployment,job,instance) ({up} == 1)',
                      'OK', 'warning', f'{source} RPC collection failed or is stale. Check bounded error metrics; never treat missing data as zero.'))
    backend_jobs = ['prometheus', 'grafana', 'loki', 'alloy', 'blackbox']
    if config.get('traces_enabled', False):
        backend_jobs += ['tempo', 'alloy-traces']
    for job in backend_jobs:
        metric = f'up{{{scope},job="{job}"}}'
        specs.append((f'eez-backend-{job}', f'EEZBackendUnavailable_{job}', f'(1 - {metric}) or absent({metric})',
                      'Alerting', 'warning', f'{job} scrape failed. Check monitoring storage, process health and network.'))
    if config.get('traces_enabled', False):
        names = 'otelcol_exporter_enqueue_failed_spans_total|otelcol_exporter_send_failed_spans_total|otelcol_receiver_refused_spans_total'
        failures = f'sum by(environment,deployment) (increase({{__name__=~"{names}",{scope},job="alloy-traces"}}[{trace_window}]))'
        healthy = f'max by(environment,deployment) (up{{{scope},job="alloy-traces"}}) == 1'
        specs.append(('eez-trace-loss', 'EEZTraceCollectionLoss', f'({failures}) and on(environment,deployment) ({healthy})',
            'OK', 'warning', 'Trace collector rejected or failed spans. Counters can overlap; inspect queue, exporter and Tempo health. Receipt accounting is independent.'))
    for name, _, native in native_configs(config):
        metric = f'up{{{scope},job="{name}",instance="{native["identity"]["instance"]}"}}'
        uid = 'eez-native-' + hashlib.sha256(name.encode()).hexdigest()[:16]
        specs.append((uid, f'EEZNativeTelemetryUnavailable_{name}', f'(1 - {metric}) or absent({metric})',
                      'Alerting', 'warning', 'Configured native metrics are unavailable. Check the process, generated telemetry config, private listener and scrape path.'))
        if native.get('traces') is not None:
            prefix = '' if native['identity']['service'] == 'eez-proof-signer' else 'reth_'
            failures = f'sum by(environment,deployment,job,instance) (increase({prefix}eez_telemetry_dropped_events_total{{{scope},job="{name}",signal="trace",reason=~"queue_full|contention|shutdown|export_failure|collector_rejected"}}[{trace_window}]))'
            specs.append(('eez-trace-native-' + hashlib.sha256(name.encode()).hexdigest()[:16], f'EEZNativeTraceLoss_{name}',
                f'({failures}) and on(environment,deployment,job,instance) ({metric} == 1)', 'OK', 'warning',
                'Native trace exporter lost spans. Check the configured receiver, queue pressure and export timeout; lifecycle delivery is independent.'))
    for job, key in [('public-rpc', 'public_rpc_url'), ('signer-listener', 'signer_tcp_target')]:
        if config.get(key):
            metric = f'probe_success{{{scope},job="{job}"}}'
            specs.append((f'eez-probe-{job}', f'EEZProbeFailed_{job}', f'(1 - {metric}) or absent({metric})',
                          'Alerting', 'critical', f'{job} probe failed. A TCP probe proves listener availability only; verify probe placement.'))
    specs.append(('eez-l1-stale', 'EEZL1HeadStale',
                  f'(time() - eez:chain_head_timestamp_seconds:current{{{scope},source="l1"}}) > bool 45',
                  'OK', 'warning', 'Observed L1 head is older than three polling intervals. Compare the independent reference and consensus health.'))
    if config.get('node_exporter_target'):
        node = f'up{{{scope},job="node-exporter"}}'
        specs.append(('eez-host-unavailable', 'EEZHostMetricsUnavailable', f'(1 - {node}) or absent({node})',
                      'Alerting', 'warning', 'Configured host metrics are unavailable.'))
        disk = f'node_filesystem_avail_bytes{{{scope},job="node-exporter",fstype!~"tmpfs|overlay|squashfs"}}'
        specs.append(('eez-disk-low', 'EEZDiskSpaceLow', f'{disk} < bool 10737418240',
                      'OK', 'warning', 'A monitored filesystem has less than 10 GiB available; inspect mountpoint and retention.'))
        specs.append(('eez-disk-projection', 'EEZDiskProjectedFull', f'predict_linear({disk}[6h], 86400) < bool 0',
                      'OK', 'warning', 'Six-hour trend projects a full filesystem within 24 hours; inspect growth and retention.'))
    rules = []
    for uid, title, expr, nodata, severity, summary in specs:
        # Query models are raw JSON in Grafana provisioning; preserve the expression reference literally.
        rules.append({'uid': uid, 'title': title, 'condition': 'B', 'for': '0s' if severity == 'heartbeat' else ('10s' if fast_test else '2m'),
            'noDataState': nodata, 'execErrState': 'Alerting', 'isPaused': False,
            'labels': {**labels, 'severity': severity}, 'annotations': {'summary': summary,
                'runbook': 'observability/runbooks/operations.md', '__dashboardUid__': 'eez-infrastructure', '__panelId__': '1'},
            'data': [{'refId': 'A', 'relativeTimeRange': {'from': 600, 'to': 0}, 'datasourceUid': 'eez-prometheus',
                      'model': {'refId': 'A', 'expr': expr, 'instant': True, 'range': False, 'intervalMs': 1000, 'maxDataPoints': 43200}},
                     {'refId': 'B', 'relativeTimeRange': {'from': 0, 'to': 0}, 'datasourceUid': '__expr__',
                      'model': {'refId': 'B', 'type': 'math', 'expression': '$A > 0', 'datasource': {'type': '__expr__', 'uid': '__expr__'}}}]})
    return [{'orgId': 1, 'name': 'eez-operational', 'folder': 'EEZ', 'interval': '10s' if fast_test else '30s', 'rules': rules}]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=pathlib.Path, required=True)
    parser.add_argument('--runtime', type=pathlib.Path, required=True)
    args = parser.parse_args()
    try:
        config = json.loads(args.config.read_text())
        prepare(config, args.runtime)
    except (ValueError, OSError, KeyError, TypeError):
        parser.exit(2, 'Invalid configuration or inaccessible runtime/log directory; no service was started.\n')
    print('Prepared runtime configuration. Grafana password is stored in secrets/grafana-admin-password; no secrets printed.')


if __name__ == '__main__':
    main()
