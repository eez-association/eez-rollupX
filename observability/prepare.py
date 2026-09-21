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


def grafana_public_url(config):
    """Browser/operator origin and optional path; dynamic ports have no inferred origin."""
    value = config.get('grafana_public_url')
    if 'grafana_public_url' not in config:
        port = config.get('grafana_port', 19300)
        return f'http://localhost:{port}' if port else None
    try:
        parsed = urlsplit(value)
        path_valid = (len(parsed.path) <= 256 and re.fullmatch(r'(?:/[a-zA-Z0-9_.-]+)*/?', parsed.path)
                      and all(segment not in ('.', '..') for segment in parsed.path.split('/')))
        valid = (isinstance(value, str) and parsed.scheme in ('http', 'https') and parsed.hostname
                 and path_valid and parsed.username is None and parsed.password is None
                 and '?' not in value and '#' not in value and parsed.port != 0
                 and re.fullmatch(r'[a-zA-Z0-9_.:-]+', parsed.hostname)
                 and not any(ord(c)<32 or ord(c)==127 or c.isspace() or c in '$"\\`' for c in value)
                 and 'REPLACE' not in value)
    except (ValueError, TypeError, AttributeError):
        valid = False
    if not valid:
        raise ValueError('grafana_public_url requires HTTP(S), a safe optional path and no credentials, query or fragment')
    return value.rstrip('/')


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
    required = {'environment', 'deployment', 'observer_instance', 'observer_target', 'log_directory'}
    receivers = {'alert_webhook_url', 'heartbeat_webhook_url'}
    allowed = required | receivers | {'alerting_enabled', 'node_exporter_target', 'public_rpc_url', 'signer_tcp_target',
                          'probe_exporter_target', 'grafana_public_url', 'grafana_port', 'prometheus_port', 'loki_port', 'native_processes', 'database_enabled', 'postgres_port', 'traces_enabled', 'tempo_port', 'otlp_http_port', 'explorer_urls', 'protocol_alerts', 'queue_source', 'client_targets', 'node_agent', 'native_currencies'}
    if set(config) - allowed or required - set(config):
        raise ValueError('missing or unknown stack configuration fields')
    alerting = config.get('alerting_enabled', True)
    if type(alerting) is not bool:
        raise ValueError('alerting_enabled must be a boolean')
    if not alerting and (any(config.get(key) is not None for key in receivers) or 'protocol_alerts' in config):
        raise ValueError('disabled alerting cannot configure receivers or protocol_alerts')
    currencies = config.get('native_currencies', {})
    if (not isinstance(currencies, dict) or set(currencies) - {'l1', 'l2'}
            or any(not isinstance(symbol, str) or not re.fullmatch(r'[A-Za-z][A-Za-z0-9]{0,11}', symbol)
                   for symbol in currencies.values())):
        raise ValueError('native_currencies must map l1/l2 to short currency symbols')
    protocol_alert_config(config)
    grafana_public_url(config)
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
        if value is None and key != 'public_rpc_url' and alerting:
            raise ValueError('enabled alerting requires operator and heartbeat receivers')
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
    recording_rules(config)  # Source choice is explicit and validated before writes.
    client_targets(config)
    node_agent_config(config)
    return path


def client_targets(config):
    targets = config.get('client_targets', [])
    if not isinstance(targets, list) or len(targets) > 4:
        raise ValueError('client_targets must be a list of at most four Lighthouse targets')
    result, identities = [], set()
    occupied = {target for _, target, _ in native_configs(config)}
    for target in targets:
        if not isinstance(target, dict) or set(target) != {'service', 'instance', 'metrics_target'} or target['service'] != 'lighthouse':
            raise ValueError('only explicit Lighthouse client targets are supported; embedded execution uses its existing native process target')
        instance, address = target['instance'], target['metrics_target']
        match = TARGET.fullmatch(address) if isinstance(address, str) else None
        if not isinstance(instance, str) or not LABEL.fullmatch(instance) or instance in identities or not match or not 0 < int(match[1]) < 65536 or 'REPLACE' in address or address in occupied:
            raise ValueError('invalid or duplicate client identity/metrics target')
        identities.add(instance); occupied.add(address)
        result.append(('lighthouse-' + instance, address, {'environment': config['environment'],
            'deployment': config['deployment'], 'service': 'lighthouse', 'instance': instance}))
    return result


def node_agent_config(config):
    agent = config.get('node_agent')
    if agent is None:
        return None
    required = {'instance', 'metrics_target', 'loki_url', 'read_gid', 'sources'}
    if not isinstance(agent, dict) or required - set(agent) or set(agent) - required - {'listen', 'allow_non_loopback', 'run_uid', 'run_gid'}:
        raise ValueError('missing or unknown node_agent fields')
    instance, target = agent['instance'], agent['metrics_target']
    match = TARGET.fullmatch(target) if isinstance(target, str) else None
    occupied = {target for _, target, _ in native_configs(config) + client_targets(config)}
    if not isinstance(instance, str) or not LABEL.fullmatch(instance) or not match or not 0 < int(match[1]) < 65536 or 'REPLACE' in target or target in occupied:
        raise ValueError('invalid or duplicate node-agent identity/target')
    if type(agent['read_gid']) is not int or not 0 <= agent['read_gid'] < 2**32:
        raise ValueError('node_agent.read_gid must be an explicit numeric read-only group')
    if ('run_uid' in agent) != ('run_gid' in agent):
        raise ValueError('node_agent.run_uid and run_gid must be configured together')
    for field in ('run_uid', 'run_gid'):
        if field in agent and (type(agent[field]) is not int or not 0 < agent[field] < 2**32):
            raise ValueError('node_agent.' + field + ' must be an explicit nonzero numeric identity')
    listen, override = agent.get('listen', '127.0.0.1:12346'), agent.get('allow_non_loopback', False)
    try:
        host, port = listen.rsplit(':', 1)
        address = ipaddress.ip_address(host.strip('[]'))
        valid = port.isascii() and port.isdigit() and 0 < int(port) < 65536 and ((address.version == 6) == (host.startswith('[') and host.endswith(']')))
    except (AttributeError, ValueError):
        valid = False
    if not valid or type(override) is not bool or (not address.is_loopback and not override):
        raise ValueError('node-agent listener must be IP:port; non-loopback needs explicit opt-in')
    try:
        url = urlsplit(agent['loki_url']); port = url.port
        valid = url.scheme in ('http', 'https') and url.hostname and url.path and (port is None or 0 < port < 65536) and not (url.username or url.password or url.query or url.fragment) and 'REPLACE' not in agent['loki_url'] and not any(c.isspace() or c in '\x00$' for c in agent['loki_url'])
    except (AttributeError, ValueError, TypeError):
        valid = False
    if not valid:
        raise ValueError('node_agent.loki_url requires an explicit private HTTP(S) push URL without embedded credentials/query/fragment')
    sources = agent['sources']
    if not isinstance(sources, list) or not 1 <= len(sources) <= 16:
        raise ValueError('node_agent.sources must contain 1..16 dedicated sources')
    paths, identities = set(), set()
    supported = {'reth_json': {'eez-composer', 'eez-follower'}, 'lighthouse_json': {'lighthouse'},
                 'lifecycle': {'eez-composer', 'eez-follower', 'eez-proof-signer'}}
    for source in sources:
        if not isinstance(source, dict) or set(source) != {'service', 'instance', 'kind', 'directory'}:
            raise ValueError('missing or unknown node-agent source fields')
        kind, service, instance, directory = (source[key] for key in ('kind', 'service', 'instance', 'directory'))
        if not isinstance(kind, str) or kind not in supported or not isinstance(service, str) or service not in supported[kind] or not isinstance(instance, str) or not LABEL.fullmatch(instance):
            raise ValueError('unsupported log source capability or identity; signer diagnostic files have no native adapter')
        if not isinstance(directory, str) or not directory.startswith('/') or directory == '/' or '..' in pathlib.PurePosixPath(directory).parts or any(c in directory for c in '\x00\n\r$') or 'REPLACE' in directory:
            raise ValueError('node-agent source needs an explicit dedicated absolute directory')
        directory = str(pathlib.PurePosixPath(directory))
        identity = (kind, service, instance)
        if directory in paths or identity in identities:
            raise ValueError('node-agent directories and source identities must be disjoint')
        paths.add(directory); identities.add(identity)
    return {**agent, 'listen': listen}


def node_log_projection(kind):
    """Export reviewed event names and constrained scalar fields, never raw bodies."""
    if kind == 'reth_json':
        events = json.loads((ROOT / 'contracts/native-log-events.json').read_text())['events']
        event_pattern = '|'.join(event.replace('.', '[.]') for event in events)
        expressions = {'event': 'fields.event_name', 'level': 'level', 'timestamp': 'timestamp',
                       'trace_id': 'fields.trace_id || span.trace_id || spans[-1].trace_id'}
        integers = ('rollup_id','nonce','sync_height','from','to','from_block','to_block','block','target_block','l1_block',
                    'height','accepted_count','deferred_count','attempts','max_attempts','target_timestamp','effective_blobs',
                    'blob_count','encoded_stream_bytes','admitted_groups','selection_limit','k','max_blocks','configured_gas_cap',
                    'max_user_txs_per_bundle','max_unsettled_l2_blocks','transaction_index','source_index')
        hashes = ('tx_hash','transaction_hash','post_batch_hash','sync_hash','l1_block_hash','l1_hash','l2_hash','source_tx_hash','source_hash','parent_hash','endpoint_hash','hash')
        booleans = ('policy_enabled','automatic_admission','due','applied','ready','success')
        enumerated = {
            'direction': 'inbound|outbound|Inbound|Outbound', 'pure_l2_mode':'always|interval',
            'constraint':'preparation_time|prior_queue_cleanup|composition_blob_limit|l1_tx_gas_budget|l1_block_gas_budget|blob_limit|block_gas|gas|da|count',
            'outcome':'committed|source_reverted|source_completed|source_completed_with_missing_effects|source_nonce_spent|attempts_exhausted|nonce_gap|pool_discarded|replaced|included|excluded|uncertain|prepared|cancelled|Included|Excluded|Uncertain',
            'status':'ready|blocked|unknown|included|excluded|uncertain|pending|committed|Ready|Blocked|Unknown|Included|Excluded|Uncertain',
        }
        prefix = 'fields.'
    else:
        events = ('Lighthouse started','Configured network','Metrics HTTP server started','Beacon node started','Low peer count',
                  'Searching for peers','Error during execution engine upcheck','Execution engine call failed','Failed to start beacon node','Shutting down..')
        event_pattern = '|'.join(event.replace('.', '[.]') for event in events)
        expressions = {'event':'msg','level':'level','timestamp':'ts'}
        integers = ('peer_count','peers','finalized_epoch','head_slot','current_slot')
        hashes, booleans = (), ()
        enumerated = {'network_name':'mainnet|gnosis|chiado|sepolia|holesky|hoodi', 'version':'Lighthouse/v[0-9][a-zA-Z0-9._+-]{0,63}'}
        prefix = ''
    patterns = {'timestamp': '[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+(Z|[+-][0-9]{2}:[0-9]{2})'}
    if kind == 'reth_json':
        patterns['trace_id'] = '[0-9a-f]{32}'
    for field in integers:
        expressions[field] = prefix + field
        patterns[field] = '[0-9]{1,20}'
    for field in hashes:
        expressions[field] = prefix + field
        patterns[field] = '0x[0-9a-fA-F]{64}'
    for field in booleans:
        expressions[field] = prefix + field
        patterns[field] = 'true|false'
    for field, pattern in enumerated.items():
        expressions[field] = prefix + field
        patterns[field] = pattern
    selector = '{collector_kind="' + kind + '"}'
    allowed = '{collector_kind="' + kind + '",review_event!~"' + event_pattern + '"}'
    level = '{collector_kind="' + kind + '",review_level!~"INFO|WARN|ERROR|CRIT"}'
    stages = ['  stage.match {', '    selector = ' + json.dumps(selector),
        '    stage.drop { longer_than = "16KiB"\n      drop_counter_reason = "diagnostic_oversized"\n    }',
        '    stage.json { expressions = { ' + ', '.join(json.dumps(key) + ' = ' + json.dumps(value) for key, value in expressions.items()) + ' } }',
        '    stage.labels { values = { review_event = "event", review_level = "level" } }',
        '    stage.match { selector = ' + json.dumps(allowed) + '\n      action = "drop"\n      drop_counter_reason = "unreviewed_event"\n    }',
        '    stage.match { selector = ' + json.dumps(level) + '\n      action = "drop"\n      drop_counter_reason = "unreviewed_level"\n    }']
    for field, pattern in patterns.items():
        stages.append('    stage.regex { source = ' + json.dumps(field) + '\n      expression = ' + json.dumps('^(?P<safe_' + field + '>' + pattern + ')$') + ' }')
    template = '{{ dict "event_name" .event "level" .level "projection" "reviewed_fields_v1" ' + ' '.join(json.dumps(field) + ' .safe_' + field for field in patterns) + ' | toJson }}'
    stages += ['    stage.timestamp { source = "timestamp"\n      format = "RFC3339Nano"\n      action_on_failure = "skip"\n    }',
        '    stage.template { source = "sanitized"\n      template = ' + json.dumps(template) + ' }',
        '    stage.output { source = "sanitized" }', '  }']
    return '\n'.join(stages)


def prepare_node_agent(config, runtime):
    directory = runtime / 'node-agent'
    agent = node_agent_config(config)
    if agent is None:
        for filename in ('config.alloy', 'compose.yaml', 'manifest.json'):
            (directory / filename).unlink(missing_ok=True)
        return
    patterns = {'reth_json': ('eez.jsonl*',), 'lighthouse_json': ('beacon.log*',), 'lifecycle': ('*.active', '*.closed')}
    targets, volumes = [], [
        {'type': 'bind', 'source': str(directory / 'config.alloy'), 'target': '/etc/alloy/config.alloy', 'read_only': True, 'bind': {'create_host_path': False}},
        {'type': 'volume', 'source': 'positions', 'target': '/var/lib/alloy'}]
    for index, source in enumerate(agent['sources']):
        mount = '/sources/' + str(index)
        volumes.append({'type': 'bind', 'source': source['directory'], 'target': mount, 'read_only': True, 'bind': {'create_host_path': False}})
        for pattern in patterns[source['kind']]:
            targets.append({'__path__': mount + '/' + pattern, 'job': 'eez-lifecycle' if source['kind'] == 'lifecycle' else 'eez-logs',
                'environment': config['environment'], 'deployment': config['deployment'], 'service': source['service'], 'instance': source['instance'], 'collector_kind': source['kind']})
    target_text = ',\n    '.join('{ ' + ', '.join(json.dumps(key) + ' = ' + json.dumps(value) for key, value in target.items()) + ' }' for target in targets)
    alloy = ('logging { level = "info" }\nlocal.file_match "node" {\n  path_targets = [\n    ' + target_text +
        ',\n  ]\n  sync_period = "5s"\n}\nloki.source.file "node" {\n  targets = local.file_match.node.targets\n  forward_to = [loki.relabel.node.receiver]\n  tail_from_end = false\n}\n'
        'loki.relabel "node" {\n  forward_to = [loki.write.monitor.receiver]\n  rule {\n    action = "labeldrop"\n    regex = "filename"\n  }\n}\n'
        'loki.write "monitor" {\n  endpoint { url = ' + json.dumps(agent['loki_url']) + ' }\n}\n')
    alloy = alloy.replace('forward_to = [loki.relabel.node.receiver]', 'forward_to = [loki.process.node.receiver]', 1)
    alloy += ('loki.process "node" {\n  forward_to = [loki.relabel.node.receiver]\n' +
              '\n'.join(node_log_projection(kind) for kind in ('reth_json','lighthouse_json')) +
              '\n  stage.label_drop { values = ["collector_kind", "review_event", "review_level"] }\n}\n')
    directory.mkdir(exist_ok=True)
    (directory / 'config.alloy').write_text(alloy)
    images = json.loads((ROOT / 'images.lock.json').read_text())
    image = next(name + '@' + digest for name, digest in images.items() if name.startswith('grafana/alloy:'))
    compose = {'services': {'alloy-node': {'image': image, 'user': 'alloy',
        'group_add': [str(agent['read_gid'])], 'network_mode': 'host', 'restart': 'unless-stopped',
        'command': ['run', '--disable-reporting', '--server.http.listen-addr=' + agent['listen'], '--storage.path=/var/lib/alloy', '/etc/alloy/config.alloy'],
        'volumes': volumes, 'mem_limit': '256m', 'cpus': 0.5, 'pids_limit': 128,
        'security_opt': ['no-new-privileges:true'], 'cap_drop': ['ALL'],
        'logging': {'driver': 'local', 'options': {'max-size': '10m', 'max-file': '3'}}}}, 'volumes': {'positions': {}}}
    identity = str(agent['run_uid']) + ':' + str(agent['run_gid']) if 'run_uid' in agent else 'alloy:alloy'
    compose['services']['alloy-node']['user'] = identity if 'run_uid' in agent else 'alloy'
    compose['services']['alloy-node']['depends_on'] = {'positions-owner': {'condition': 'service_completed_successfully'}}
    # Only the dedicated named volume is accessible; changing collector owner preserves checkpoints.
    compose['services']['positions-owner'] = {'image': image, 'user': '0:0',
        'entrypoint': ['chown'], 'command': ['-R', identity, '/var/lib/alloy'],
        'network_mode': 'none', 'cap_drop': ['ALL'], 'cap_add': ['CHOWN', 'DAC_READ_SEARCH'],
        'security_opt': ['no-new-privileges:true'], 'restart': 'no',
        'mem_limit': '32m', 'cpus': 0.1, 'pids_limit': 16,
        'volumes': [{'type': 'volume', 'source': 'positions', 'target': '/var/lib/alloy'}]}
    write_json(directory / 'compose.yaml', compose)
    write_json(directory / 'manifest.json', {'environment': config['environment'], 'deployment': config['deployment'],
        'instance': agent['instance'], 'metrics_listen': agent['listen'], 'source_capabilities': [
            {key: source[key] for key in ('service','instance','kind')} for source in agent['sources']],
        'execution_metric_attribution': 'process_wide', 'signer_diagnostic_file_logging': 'unavailable_native_adapter',
        'lighthouse_auxiliary_discv5_libp2p_logs': 'not_collected_without_separate_review',
        'diagnostic_export': 'reviewed_events_and_constrained_fields_only; opaque bodies/errors/debug omitted',
        'status': 'configured_not_runtime_verified'})


def recording_rules(config):
    """One current queue source; explicit native choice never falls back to RPC."""
    source = config.get('queue_source', 'rpc')
    if source not in ('rpc', 'native'):
        raise ValueError('queue_source must be rpc or native')
    rules = json.loads((ROOT / 'prometheus/recording-rules.yaml').read_text())
    if source == 'rpc':
        return rules
    composers = [item for item in native_configs(config) if item[2]['identity']['service'] == 'eez-composer']
    if len(composers) != 1:
        raise ValueError('native queue_source requires exactly one configured Composer target')
    job = composers[0][0]
    keys = 'environment, deployment, job, instance'
    select = '{job="' + job + '"}'
    freshness = (' and on (' + keys + ') (reth_eez_native_queue_sample_valid' + select + ' == 1)'
                 ' and on (' + keys + ') (up' + select + ' == 1)'
                 ' and on (' + keys + ') ((time() - reth_eez_native_queue_sample_timestamp_seconds' + select + ') <= 45)'
                 ' and on (' + keys + ') ((time() - reth_eez_native_queue_sample_timestamp_seconds' + select + ') >= -5)')
    evaluation = ' and on (' + keys + ') (reth_eez_native_queue_evaluation_valid' + select + ' == 1)'
    # An observed empty population needs no readiness decision. Require all eight
    # distinct direction/state pairs, each exactly once and actually zero; absent,
    # NaN, duplicate or unexpected series must not synthesize an empty queue.
    population = 'reth_eez_native_queue_transactions' + select
    zero_members = ('reth_eez_native_queue_transactions{job="' + job
                    + '",direction=~"inbound|outbound",state=~"queued_ready|queued_blocked|queued_unknown|in_flight"} == 0')
    empty = ('(count by (' + keys + ') (count by (' + keys + ', direction, state) ('
             + zero_members + ') == 1) == 8) and on (' + keys + ') '
             '(count by (' + keys + ') (' + population + ') == 8)')
    population_evaluation = (' and on (' + keys + ') ((reth_eez_native_queue_evaluation_valid'
                             + select + ' == 1) or on (' + keys + ') (' + empty + '))')
    replacements = {
        'eez:queue_transactions:current': population + freshness + population_evaluation,
        'eez:queue_oldest_age_seconds:current': 'reth_eez_native_queue_oldest_age_seconds' + select + freshness + evaluation,
        'eez:queue_evaluation_valid:current': 'reth_eez_native_queue_evaluation_valid' + select + freshness,
    }
    for group in rules['groups']:
        for rule in group['rules']:
            if rule['record'] in replacements:
                rule['expr'] = replacements[rule['record']]
    return rules


def prepare(config, runtime, *, fast_test=False):
    logs = validate(config)
    runtime = pathlib.Path(runtime).resolve()
    # .env values are paths, never executable shell text or interpolated secrets.
    if any(c in str(runtime) + str(logs) for c in '\n\r$"\\'):
        raise ValueError('runtime/log paths cannot contain environment-file metacharacters')
    alerts = alert_rules(config, fast_test=fast_test)
    if not config.get('alerting_enabled', True):
        pause_managed_alerts(alerts, runtime / 'grafana/provisioning/alerting/eez.yaml')
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
        if job == 'grafana' and (prefix := urlsplit(grafana_public_url(config) or '').path):
            scrape[-1]['metrics_path'] = prefix + '/metrics'
    if config.get('traces_enabled', False):
        for job, target in [('tempo', 'tempo:3200'), ('alloy-traces', 'alloy-traces:12345')]:
            scrape.append({'job_name': job, 'static_configs': [{'targets': [target], 'labels': {**labels, 'service': job}}]})
    scrape.append({'job_name': 'eez-observer', 'static_configs': [{'targets': [config['observer_target']],
                   'labels': {**labels, 'service': 'eez-observer', 'instance': config['observer_instance']}}]})
    for name, target, native in native_configs(config):
        write_json(runtime / f'native/{name}.json', native)
        scrape.append({'job_name': name, 'static_configs': [{'targets': [target], 'labels': native['identity']}],
            # Pinned upstream Reth emits hash/address-labelled families. Exclude
            # those complete samples; removing dimensions would collide series.
            'metric_relabel_configs': [{'source_labels': ['hash', 'address'], 'separator': '',
                'regex': '.+', 'action': 'drop'},
                # Pinned Reth strips argument values, but CLI flag combinations
                # are unnecessary configuration-shaped labels for monitoring.
                {'source_labels': ['__name__'], 'regex': '(reth_)?process_cli_args', 'action': 'drop'}]})
    for name, target, identity in client_targets(config):
        scrape.append({'job_name': name, 'static_configs': [{'targets': [target], 'labels': identity}],
            'metric_relabel_configs': [{'source_labels': ['__name__'], 'action': 'keep',
                'regex': 'lighthouse_info|beacon_head_slot|beacon_finalized_epoch|beacon_head_state_current_justified_epoch|sync_eth2_synced|sync_slots_per_second|libp2p_peers|network_node_bytes_total_(received|transmit)|process_(cpu_seconds_total|num_threads|resident_memory_bytes|shared_memory_bytes|start_time_seconds|virtual_memory_bytes)|sync_peers_per_(column|custody_column)_subnet'}]})
    agent = node_agent_config(config)
    if agent:
        scrape.append({'job_name': 'alloy-node-' + agent['instance'], 'static_configs': [{'targets': [agent['metrics_target']],
            'labels': {**labels, 'service': 'alloy-node', 'instance': agent['instance']}}],
            # Rotating per-file paths are not bounded metric dimensions. Keep those
            # diagnostics local instead of dropping labels and colliding series.
            'metric_relabel_configs': [{'source_labels': ['path', 'filename'], 'separator': '',
                'regex': '.+', 'action': 'drop'}]})
    prepare_node_agent(config, runtime)
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
    write_json(runtime / 'prometheus/recording-rules.yaml', recording_rules(config))
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
    # Grafana expands $ in provisioning values, including URLs. Escape literal dollars.
    literal = lambda value: value.replace('$', '$$')
    alerting = {'apiVersion': 1, 'groups': alerts}
    if config.get('alerting_enabled', True):
        alerting.update({'contactPoints': [
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
    write_json(provision / 'alerting/eez.yaml', alerting)
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
    public_url = grafana_public_url(config)
    environment = f'OBS_RUNTIME="{runtime}"\nOBS_LOG_DIR="{logs}"\n' + ''.join(f'{key.upper()}={value}\n' for key, value in ports.items())
    environment += 'GRAFANA_PUBLIC_URL=' + (public_url or '') + '\n'
    public_parts = urlsplit(public_url or '')
    environment += 'GRAFANA_SUB_PATH=' + public_parts.path + '\n'
    environment += 'GRAFANA_SERVE_FROM_SUB_PATH=' + str(bool(public_parts.path)).lower() + '\n'
    environment += 'GRAFANA_COOKIE_SECURE=' + str(public_parts.scheme == 'https').lower() + '\n'
    (runtime / 'compose.env').write_text(environment)
    (runtime / 'compose.env').chmod(0o600)
    write_json(runtime / 'manifest.json', {'environment': config['environment'], 'deployment': config['deployment'],
        'observer_instance': config['observer_instance'], 'test_timings': fast_test, 'grafana_public_url': public_url,
        'queue_source': config.get('queue_source', 'rpc'),
        'native_currencies': config.get('native_currencies', {}),
        'client_targets': [name for name, _, _ in client_targets(config)],
        'node_agent_enabled': node_agent_config(config) is not None,
        'native_processes': [name for name, _, _ in native_configs(config)],
        'probes_independent': (config.get('probe_exporter_target') or 'blackbox:9115') != 'blackbox:9115',
        'database_enabled': config.get('database_enabled', False),
        'alerting_enabled': config.get('alerting_enabled', True),
        'traces_enabled': config.get('traces_enabled', False),
        'node_metrics_configured': bool(config.get('node_exporter_target'))})
    return runtime


def pause_managed_alerts(groups, previous_path):
    """Pause current and previously rendered EEZ rules without resetting Grafana routing.

    Removing a provisioning file does not delete stored rules. Retain prior
    optional-profile rules in the same owned group so disabling their inputs
    together with alerting cannot leave those rules evaluating in Grafana.
    """
    current = groups[0]['rules']
    uids = {rule['uid'] for rule in current}
    if previous_path.exists():
        previous = json.loads(previous_path.read_text())
        if not isinstance(previous, dict) or not isinstance(previous.get('groups', []), list):
            raise ValueError('invalid previously generated alert groups')
        for group in previous.get('groups', []):
            if not isinstance(group, dict):
                raise ValueError('invalid previously generated alert group')
            if (group.get('orgId'), group.get('folder'), group.get('name')) != (1, 'EEZ', 'eez-operational'):
                continue
            if not isinstance(group.get('rules', []), list):
                raise ValueError('invalid previously generated alert rules')
            for rule in group.get('rules', []):
                if not isinstance(rule, dict):
                    raise ValueError('invalid previously generated alert rule')
                uid = rule.get('uid')
                if isinstance(uid, str) and uid.startswith('eez-') and uid not in uids:
                    current.append(rule)
                    uids.add(uid)
    for rule in current:
        rule['isPaused'] = True


def prepare_dashboards(config, runtime):
    """Provision only supported profiles, with optional links from explicit config."""
    destination = runtime / 'grafana/dashboards'
    destination.mkdir(parents=True, exist_ok=True)
    names = ['network.json', 'infrastructure.json', 'runbooks.json']
    if config.get('database_enabled', False):
        names += ['cross-chain.json', 'settlement.json', 'blobs.json', 'gas.json']
    # Delete only this generator's known dashboard files on profile downgrade.
    for name in ['network.json', 'infrastructure.json', 'runbooks.json', 'cross-chain.json', 'settlement.json', 'blobs.json', 'gas.json']:
        path = destination / name
        if name not in names and path.exists():
            path.unlink()
    explorers = config.get('explorer_urls', {})
    for name in names:
        dashboard = runbook_dashboard(config) if name == 'runbooks.json' else json.loads((ROOT / 'grafana/dashboards' / name).read_text())
        if not config.get('database_enabled', False):
            dashboard['panels'] = [panel for panel in dashboard['panels']
                                   if panel.get('datasource', {}).get('uid') != 'eez-postgres']
            if name == 'network.json':
                # SQL activity occupies rows38–69 only in the database profile.
                # Keep the original compact layout for exporter-only stacks.
                for panel in dashboard['panels']:
                    if panel['gridPos']['y'] >= 70:
                        panel['gridPos']['y'] -= 32
        if name == 'infrastructure.json':
            agent = config.get('node_agent')
            diagnostic_count = sum(source['kind'] in ('reth_json', 'lighthouse_json')
                                   for source in (agent or {}).get('sources', []))
            capability_rows = [
                ('Host metrics', 'Configured' if config.get('node_exporter_target') else 'Not configured'),
                ('Lighthouse metrics', str(len(config.get('client_targets', []))) + ' configured targets'),
                ('Public RPC probe', 'Configured' if config.get('public_rpc_url') else 'Not configured'),
                ('Node-host log collector', str(diagnostic_count) + ' diagnostic sources configured' if agent else 'Not configured'),
            ]
            content = '| Capability | Configuration |\n| --- | --- |\n' + '\n'.join(
                '| ' + label + ' | ' + state + ' |' for label, state in capability_rows)
            content += ('\n\nConfiguration does not establish collector health or readable files. '
                        'Unavailable capabilities have unknown data, not healthy zeros. '
                        'Lifecycle streams are separate and may be available without diagnostic log collection. '
                        'Use scrape health and the panel descriptions to interpret missing data.')
            for panel in dashboard['panels']:
                panel['gridPos']['y'] += 8
            dashboard['panels'].insert(0, {'id': 20, 'type': 'text', 'title': 'Configured infrastructure capabilities',
                'gridPos': {'x': 0, 'y': 0, 'w': 24, 'h': 8}, 'options': {'mode': 'markdown', 'content': content}})
        for variable in dashboard['templating']['list']:
            if variable['name'] == 'instance' and variable.get('datasource', {}).get('uid') == 'eez-postgres':
                # Built-in All expands into quoted SQL values and escaped PromQL
                # alternatives. Include configured processes before lifecycle
                # ingestion so an empty history cannot hide native metrics.
                instances = sorted({process['instance'] for process in config.get('native_processes', [])})
                original = variable['query']
                additions = ['SELECT ' + "'" + instance.replace("'", "''") + "' AS __text," +
                    "'" + instance.replace("'", "''") + "' AS __value" for instance in instances]
                catalog = ' UNION '.join(['(' + original + ')', *additions])
                variable['query'] = "WITH instances AS (" + catalog + ") SELECT * FROM instances UNION ALL SELECT 'Unavailable' AS __text,'' AS __value WHERE NOT EXISTS(SELECT FROM instances) ORDER BY 1"
                variable['definition'] = variable['query']
        annotations = dashboard.setdefault('annotations', {}).setdefault('list', [])
        annotations.append({'name': 'Verified release records', 'datasource': {'type': 'grafana', 'uid': '-- Grafana --'},
            'enable': True, 'hide': False, 'iconColor': '#5794F2', 'type': 'tags', 'matchAny': False,
            'tags': ['eez-release', 'environment:$environment', 'deployment:$deployment'], 'limit': 100})
        if name != 'runbooks.json' and any(source['kind'] == 'reth_json' and source['service'] == 'eez-composer'
               for source in (config.get('node_agent') or {}).get('sources', [])):
            annotations.append({'name': 'Composer startup and active policy', 'datasource': {'type': 'loki', 'uid': 'eez-loki'},
                'enable': True, 'hide': False, 'iconColor': '#FF9830',
                'expr': '{environment="$environment",deployment="$deployment",service="eez-composer"} | json | event_name=~"eez.composer.(settlement.policy|emission.limits)"',
                'maxLines': 100, 'titleFormat': '{{event_name}}', 'tagKeys': 'service,instance'})
        for panel in dashboard['panels']:
            if panel.get('fieldConfig', {}).get('defaults', {}).get('unit') == 'suffix: native units':
                # These panels already use whole native amounts. Currency labels
                # are explicit deployment metadata, never a price conversion.
                currencies = config.get('native_currencies', {})
                mixed = name == 'gas.json' and panel['id'] in (12, 18, 20)
                if mixed:
                    if currencies:
                        # Match query frames, not SQL display-name aliases:
                        # Grafana may keep chain labels outside the field name.
                        original = panel['targets'][0]
                        panel['targets'] = []
                        for ref_id, role in (('A', 'l1'), ('B', 'l2')):
                            target = dict(original, refId=ref_id)
                            if 'rawSql' in target:
                                target['rawSql'] = target['rawSql'].replace(' GROUP BY ', " AND c.role='" + role + "' GROUP BY ")
                            else:
                                target['expr'] = target['expr'].replace(
                                    'eez_observer_funding_balance_native_units{',
                                    'eez_observer_funding_balance_native_units{chain="' + role + '",', 1)
                            panel['targets'].append(target)
                            panel['fieldConfig']['overrides'].append({
                                'matcher': {'id': 'byFrameRefID', 'options': ref_id},
                                'properties': [{'id': 'unit', 'value': 'suffix: ' + currencies.get(role, 'native units')}]})
                    label = ' / '.join(dict.fromkeys(currencies.get(role, 'native units') for role in ('l1', 'l2')))
                else:
                    label = currencies.get('l1', 'native units')
                    panel['fieldConfig']['defaults']['unit'] = 'suffix: ' + label
                panel['title'] = panel['title'].replace('native units', label)
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
                        # The frontend dispatches hexadecimal TraceQL input to
                        # its backend traceId query; traceId is not an editor mode.
                        'url': '/explore?schemaVersion=1&panes={"trace":{"datasource":"eez-tempo","queries":[{"refId":"A","queryType":"traceql","query":"${__value.raw}"}]}}', 'targetBlank': True}]}]})
            if panel['type'] == 'table':
                # Preserve exact SQL decimal strings and all-numeric hex IDs.
                # Grafana's default short unit otherwise parses/scales them.
                panel['fieldConfig']['overrides'].append({'matcher': {'id': 'byType', 'options': 'string'},
                    'properties': [{'id': 'unit', 'value': 'string'}]})
        grafana_subpath_links(dashboard, urlsplit(grafana_public_url(config) or '').path)
        write_json(destination / name, dashboard)


def grafana_subpath_links(value, prefix):
    """Prefix only reviewed Grafana app URLs, never SQL, external or template links."""
    if not prefix:
        return
    if isinstance(value, dict):
        for key, child in value.items():
            if key == 'url' and isinstance(child, str) and (child.startswith(('/d/', '/explore?')) or child == '/explore'):
                value[key] = prefix + child
            else:
                grafana_subpath_links(child, prefix)
    elif isinstance(value, list):
        for child in value:
            grafana_subpath_links(child, prefix)


def runbook_dashboard(config):
    files = [('operations.md', 'Monitoring operations'), ('protocol-alerts.md', 'Protocol alert operations')]
    ids = {name: index for index, (name, _) in enumerate(files, 1)}
    prefix = urlsplit(grafana_public_url(config) or '').path
    def local_link(match):
        label, target = match.groups()
        filename = target.split('#', 1)[0]
        if filename in ids:
            return f'[{label}]({prefix}/d/eez-runbooks?viewPanel={ids[filename]})'
        if target.startswith(('http://', 'https://', '#')):
            return match.group(0)
        # Other repository references remain explicit local file paths, instead
        # of broken browser-relative links or an assumed external documentation host.
        path = pathlib.PurePosixPath('observability/runbooks') / filename
        return f'{label} (`{path}` in the checkout)'
    panels = []
    for index, (name, title) in enumerate(files, 1):
        content = (ROOT / 'runbooks' / name).read_text()
        content = re.sub(r'\[([^]\n]+)\]\(([^)\n]+)\)', local_link, content)
        panels.append({'id': index, 'title': title, 'type': 'text', 'gridPos': {'x': 0, 'y': (index-1)*32, 'w': 24, 'h': 32},
                       'options': {'mode': 'markdown', 'content': content}, 'fieldConfig': {'defaults': {}, 'overrides': []}})
    variables = [{'name': key, 'type': 'constant', 'hide': 2, 'query': config.get(key, ''),
                  'current': {'text': config.get(key, ''), 'value': config.get(key, '')}} for key in ('environment', 'deployment')]
    return {'uid': 'eez-runbooks', 'title': 'EEZ operator runbooks', 'tags': ['eez'], 'schemaVersion': 39,
            'version': 1, 'editable': False, 'timezone': 'utc', 'refresh': '', 'panels': panels,
            'templating': {'list': variables}, 'annotations': {'list': []},
            'description': 'Versioned local runbooks; independent of SQL, Prometheus and Loki availability.'}


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
        # Host/tunnel consumers keep their existing files. Managed Compose
        # services reach PostgreSQL only through the private ledger network.
        for suffix, host, port in [('', '127.0.0.1', config.get('postgres_port',19432)),
                                   ('-container', 'postgres', 5432)]:
            path = runtime / f'database/{role}{suffix}.json'
            write_json(path, {'connection_string': f"host={host} port={port} user={user} dbname=eez_observer password={passwords[role]} sslmode=disable",
                              'allow_private_plaintext': True})
            path.chmod(0o600)
    datasources.append({'name': 'EEZ Canonical ledger', 'uid': 'eez-postgres', 'type': 'postgres', 'access': 'proxy',
        'url': 'postgres:5432', 'user': 'eez_observer_reader', 'editable': False,
        'jsonData': {'database': 'eez_observer', 'sslmode': 'disable', 'postgresVersion': 1800,
                     # Concurrent expanded canonical plans exceeded the packaged
                     # 512 MiB database budget in a real browser workload.
                     'maxOpenConns': 1, 'maxIdleConns': 1, 'connMaxLifetime': 300, 'timescaledb': False},
        'secureJsonData': {'password': passwords['reader']}})


def protocol_alert_config(config):
    """Explicit capabilities prevent an absent optional producer looking healthy."""
    value = config.get('protocol_alerts')
    if value is None:
        if 'protocol_alerts' in config:
            raise ValueError('protocol_alerts must be an object when configured')
        return None
    defaults = {'index_lag_blocks': 64, 'gap_threshold': 1, 'progress_warning_seconds': 120,
                'progress_critical_seconds': 300, 'blob_collection': False, 'lifecycle_sources': [],
                'follower_comparison': False, 'signer_rejection_threshold': 3, 'signer_authorized_failure_threshold': 3,
                'pure_l2_inclusion_grace_seconds': None, 'policy_critical_delay_seconds': 300,
                'funding_accounts': [], 'poster_funding_estimates': False,
                'required_blob_derivation': False, 'required_blob_retry_threshold': 3}
    if (not isinstance(value, dict) or set(value) - defaults.keys() - {'owner'}
            or config.get('database_enabled') is not True):
        raise ValueError('protocol alerts require database_enabled and known fields')
    owner = value.get('owner')
    if not isinstance(owner, str) or not LABEL.fullmatch(owner) or 'REPLACE' in owner:
        raise ValueError('protocol alerts require an explicit bounded owner')
    result = {**defaults, **value}
    for key in ('index_lag_blocks', 'gap_threshold', 'progress_warning_seconds', 'progress_critical_seconds',
                'signer_rejection_threshold', 'signer_authorized_failure_threshold', 'policy_critical_delay_seconds'):
        if type(result[key]) is not int or not 1 <= result[key] <= 86400:
            raise ValueError('protocol alert thresholds must be positive bounded integers')
    if result['progress_critical_seconds'] <= result['progress_warning_seconds']:
        raise ValueError('critical progress age must exceed warning age')
    for key in ('blob_collection', 'follower_comparison', 'poster_funding_estimates', 'required_blob_derivation'):
        if type(result[key]) is not bool:
            raise ValueError('protocol alert capabilities must be boolean')
    grace = result['pure_l2_inclusion_grace_seconds']
    if grace is not None and (type(grace) is not int or not 0 <= grace <= 86400):
        raise ValueError('pure-L2 inclusion grace must be a bounded nonnegative integer')
    services = {native['identity']['service'] for _, _, native in native_configs(config)}
    if grace is not None and 'eez-composer' not in services:
        raise ValueError('policy-age alerts require a configured composer metrics target')
    if 'policy_critical_delay_seconds' in value and grace is None:
        raise ValueError('policy critical delay requires explicit inclusion grace')
    if any(key in value for key in ('signer_rejection_threshold', 'signer_authorized_failure_threshold')) and 'eez-proof-signer' not in services:
        raise ValueError('signer thresholds require a signer metrics target')
    if result['poster_funding_estimates'] and 'eez-composer' not in services:
        raise ValueError('poster candidate funding alerts require composer metrics')
    if type(result['required_blob_retry_threshold']) is not int or not 2 <= result['required_blob_retry_threshold'] <= 100:
        raise ValueError('required-blob retry threshold must be between two and100')
    if 'required_blob_retry_threshold' in value and not result['required_blob_derivation']:
        raise ValueError('required-blob retry threshold requires its explicit capability')
    accounts = result['funding_accounts']
    if not isinstance(accounts, list) or len(accounts) > 2:
        raise ValueError('at most two explicit funding account capabilities are supported')
    account_roles = set()
    for account in accounts:
        if (not isinstance(account, dict) or set(account) != {'account', 'chain'}
                or account['account'] not in ('poster', 'system') or account['chain'] not in ('l1', 'l2')
                or account['account'] in account_roles or account['account'] == 'poster' and account['chain'] != 'l1'):
            raise ValueError('funding capabilities require unique bounded account roles and chains')
        account_roles.add(account['account'])
    sources = result['lifecycle_sources']
    if not isinstance(sources, list) or len(sources) > 16:
        raise ValueError('at most 16 lifecycle sources may be monitored')
    seen = set()
    for source in sources:
        if (not isinstance(source, dict) or set(source) != {'service', 'instance'}
                or any(not isinstance(v, str) or not LABEL.fullmatch(v) for v in source.values())):
            raise ValueError('lifecycle source requires bounded service and instance')
        identity = (source['service'], source['instance'])
        if identity in seen:
            raise ValueError('duplicate lifecycle source')
        seen.add(identity)
    return result


def protocol_alert_specs(config, scope, observer, *, fast_test=False):
    options = protocol_alert_config(config)
    if options is None:
        return []
    specs = []
    keys = 'environment,deployment,job,instance'
    healthy = f'up{{{observer}}} == 1'
    def metric(name, extra=''):
        return f'{name}{{{observer}{"," + extra if extra else ""}}}'
    def available(value):
        return f'(({value} != bool 1) or absent({value})) and on({keys}) ({healthy})'
    def add(uid, title, expression, summary, severity='warning', hold=None, dashboard='eez-cross-chain'):
        category = next((section for prefix, section in [('eez-index-', 'canonical-index'),
            ('eez-progress-', 'canonical-progress'), ('eez-outcomes-', 'canonical-progress'),
            ('eez-lifecycle-', 'lifecycle'), ('eez-lc-', 'lifecycle'), ('eez-blob-', 'blobs'),
            ('eez-follower-', 'follower'), ('eez-signer-', 'signer'), ('eez-policy-', 'pure-l2-policy'), ('eez-funding-', 'funding')]
            if uid.startswith(prefix)), 'diagnosis')
        specs.append((uid, title, expression, 'OK', severity, summary,
                      {'owner': options['owner'], 'for': hold, 'dashboard': dashboard,
                       'runbook': 'observability/runbooks/protocol-alerts.md#' + category}))
    for chain in ('l1', 'l2'):
        valid = metric('eez_observer_index_collection_success', f'chain="{chain}"')
        add('eez-index-' + chain, 'EEZCanonicalIndexUnavailable_' + chain, available(valid),
            'Canonical index is failed, stale or absent. Inspect chain identity, persisted gaps and database access; dependent index values are unknown.')
        gate = f'and on({keys},chain) ({valid} == 1) and on({keys}) ({healthy})'
        add('eez-index-lag-' + chain, 'EEZCanonicalIndexLag_' + chain,
            f'({metric("eez_observer_lag_blocks", f"chain=\"{chain}\"")} > bool {options["index_lag_blocks"]}) {gate}',
            'Canonical cursor trails the observed head beyond the configured block threshold. Preserve bootstrap and bounded catch-up.')
        add('eez-index-gap-' + chain, 'EEZCanonicalIndexGaps_' + chain,
            f'({metric("eez_observer_index_open_gaps", f"chain=\"{chain}\"")} >= bool {options["gap_threshold"]}) {gate}',
            'Persisted canonical receipt/history gaps exceed the configured threshold. Missing receipts are unknown costs, not zero fees.')
    index_healthy = (
        f'({metric("eez_observer_index_collection_success", "chain=\"l1\"")} == 1) and on({keys}) '
        f'({metric("eez_observer_index_collection_success", "chain=\"l2\"")} == 1)')
    projection = metric('eez_observer_canonical_outcome_evaluation_valid')
    add('eez-outcomes-unavailable', 'EEZCanonicalOutcomeEvidenceUnavailable', f'({available(projection)}) and on({keys}) ({index_healthy})',
        'Canonical outcome evidence is stale, invalid or unavailable. Check projection and both canonical indexes; no completion assertion is available.')
    # The existing queue rule is aggregate. Do not invent a directional mapping
    # or silently combine native and RPC populations during the source transition.
    group = 'environment,deployment'
    active = f'sum by({group}) (eez:queue_transactions:current{{{scope},state=~"queued_ready|in_flight"}}) > 0'
    advancing = f'max by({group}) (delta(eez:chain_head_number:current{{{scope},source="l1"}}[2m])) > 0'
    progress = f'max by({group}) ({metric("eez_observer_last_canonical_terminal_timestamp_seconds")})'
    valid = f'max by({group}) (({projection}) and on({keys}) ({index_healthy}) and on({keys}) ({healthy})) == 1'
    gate = f'and on({group}) ({active}) and on({group}) ({advancing}) and on({group}) ({valid})'
    for severity, name in [('warning', 'Warning'), ('critical', 'Critical')]:
        threshold = options['progress_' + severity + '_seconds']
        add('eez-progress-' + severity, 'EEZCanonicalProgressStalled' + name,
            f'((time() - ({progress})) > bool {threshold}) {gate}',
            'Ready/in-flight work and advancing L1 coexist with old validated canonical terminal progress. This is aggregate handling, not successful-commit throughput.',
            severity=severity, hold='0s')
    add('eez-progress-unknown', 'EEZCanonicalProgressUnknown',
        f'(({active}) unless on({group}) ({progress})) and on({group}) ({valid})',
        'Work exists but no retained canonical terminal timestamp is known. Investigate lifecycle coverage; do not turn missing history into an epoch-zero stall.')
    for source in options['lifecycle_sources']:
        selector = f'source_service="{source["service"]}",source_instance="{source["instance"]}"'
        suffix = hashlib.sha256((source['service'] + ':' + source['instance']).encode()).hexdigest()[:12]
        valid = metric('eez_observer_lifecycle_collection_success', selector)
        add('eez-lifecycle-source-' + suffix, 'EEZLifecycleCollectionUnavailable_' + source['service'] + '_' + source['instance'], available(valid),
            'Configured lifecycle source is absent, unreadable, stale or failed. Inspect the read-only spool mount and independent source worker.')
        for category in ('gap_intervals', 'quarantined_records', 'lost_segments', 'stale_partial_segments', 'unreadable_segments', 'unknown_trailing_epochs'):
            data = metric('eez_observer_lifecycle_' + category, selector)
            add('eez-lc-' + hashlib.sha256((category + suffix).encode()).hexdigest()[:20],
                'EEZLifecycleCoverage_' + category + '_' + source['service'] + '_' + source['instance'],
                f'({data} >= bool {options["gap_threshold"]}) and on({keys},source_service,source_instance) ({valid} == 1) and on({keys}) ({healthy})',
                'Lifecycle coverage contains ' + category + '. Preserve raw records and checkpoints; transport completion alone is not canonical success.')
    if options['lifecycle_sources']:
        for chain in ('l1', 'l2'):
            valid = metric('eez_observer_source_receipt_collection_success', f'chain="{chain}"')
            add('eez-source-receipts-' + chain, 'EEZSourceReceiptCollectionUnavailable_' + chain, available(valid),
                'Lifecycle-referenced source receipt collection failed or is stale. This worker is independent of configured wallet selection and canonical scanner health.')
            names = 'eez_observer_source_receipt_(missing_receipts|accounting_gaps|lookup_errors|not_observed|outside_interval|unavailable_transactions|unindexed_blocks)'
            gaps = f'max by({keys},chain) ({{__name__=~"{names}",{observer},chain="{chain}"}})'
            add('eez-source-receipt-gaps-' + chain, 'EEZSourceReceiptCoverageGap_' + chain,
                f'({gaps} >= bool {options["gap_threshold"]}) and on({keys},chain) ({valid} == 1) and on({keys}) ({healthy})',
                'At least one source receipt coverage category exceeds threshold. Categories may overlap; inspect missing receipts, cost fields and archive/bootstrap coverage separately.')
    if options['blob_collection']:
        valid = metric('eez_observer_blob_collection_success')
        add('eez-blob-collection', 'EEZBlobCollectionUnavailable', available(valid),
            'Configured authenticated blob measurement worker is failed, stale or absent. Inspect Beacon identity/reachability and bounded error reason.', dashboard='eez-blobs')
        unresolved = metric('eez_observer_blob_posts', 'state!="measured"')
        add('eez-blob-coverage', 'EEZBlobMeasurementCoverageGap',
            f'(sum by({keys}) ({unresolved}) >= bool {options["gap_threshold"]}) and on({keys}) ({valid} == 1) and on({keys}) ({healthy})',
            'Historical blob measurements are incomplete. Inspect state-specific coverage. This warning does not assert that current derivation is blocked.', dashboard='eez-blobs')
    if options['follower_comparison']:
        known = metric('eez_observer_follower_comparison_known')
        agreement = metric('eez_observer_follower_state_root_agreement')
        add('eez-follower-unknown', 'EEZFollowerComparisonUnknown', available(known),
            'Enabled independent comparison is unavailable, stale, lagging or lacks exact canonical provenance. Unknown is not agreement.')
        add('eez-follower-mismatch', 'EEZFollowerStateRootMismatch',
            f'({agreement} == bool 0) and on({keys}) ({known} == 1) and on({keys}) ({healthy})',
            'Independent execution produced a different root at the same validated canonical settled block. Preserve exact evidence and investigate before overriding the follower.',
            severity='critical', hold='10s' if fast_test else '30s')
    if options['required_blob_derivation']:
        valid = metric('eez_observer_required_blob_evaluation_valid')
        blocked = metric('eez_observer_required_blob_blocked')
        retries = metric('eez_observer_required_blob_retries')
        attempted = metric('eez_observer_required_blob_last_attempt_timestamp_seconds')
        fresh = f'((time() - {attempted}) <= 90) and ((time() - {attempted}) >= -5)'
        unknown = f'(({available(valid)}) > 0 or absent({attempted}) or ((time() - {attempted}) > 90) or (({attempted} - time()) > 5)) and on({keys}) ({healthy})'
        add('eez-blob-required-unknown', 'EEZRequiredBlobEvidenceUnknown', unknown,
            'Configured current-derivation diagnostic is missing, stale, unsupported or lacks exact canonical dependency evidence. Historical blob coverage cannot replace it.', dashboard='eez-blobs')
        add('eez-blob-required-blocked', 'EEZRequiredBlobBlocksDerivation',
            f'({blocked} == 1) and on({keys}) ({valid} == 1) and on({keys}) ({retries} >= {options["required_blob_retry_threshold"]}) and on({keys}) ({fresh}) and on({keys}) ({healthy})',
            'Actual derivation repeatedly failed to retrieve a required blob for the same canonical posting occurrence. Inspect Beacon availability and the exact settlement evidence; preserve chain data.',
            severity='critical', hold='10s' if fast_test else '30s', dashboard='eez-blobs')
    for account in options['funding_accounts']:
        selector = f'account="{account["account"]}",chain="{account["chain"]}"'
        source = metric('eez_observer_funding_source_valid', selector)
        burn = metric('eez_observer_funding_burn_valid', selector)
        low = metric('eez_observer_funding_runway_below_day', selector)
        add('eez-funding-source-' + account['account'], 'EEZFundingBalanceUnknown_' + account['account'], available(source),
            'Configured account balance is missing, stale or unavailable at a verified canonical block. Check identity, RPC support and account configuration.', dashboard='eez-gas')
        unknown = metric('eez_observer_funding_burn_unknown', selector + ',reason!="zero_burn"')
        add('eez-funding-coverage-' + account['account'], 'EEZFundingBurnCoverageUnknown_' + account['account'],
            f'(({unknown} > 0) or absent({burn}) or (absent({low}) and on({keys},account,chain) ({burn} == 1))) and on({keys},account,chain) ({source} == 1) and on({keys}) ({healthy})',
            'Runway lacks complete canonical spend coverage or a matching cursor. Preserve unknown burn; zero observed spend separately remains unknown without this coverage warning.', dashboard='eez-gas')
        add('eez-funding-runway-' + account['account'], 'EEZFundingRunwayBelowDay_' + account['account'],
            f'({low} == 1) and on({keys},account,chain) ({burn} == 1) and on({keys},account,chain) ({source} == 1) and on({keys}) ({healthy})',
            'Exact covered positive canonical burn implies less than 24 hours of runway for the explicitly configured account. Verify its address and transfer procedure before funding; this account is not inferred from the runtime signer.', dashboard='eez-gas')
    for name, _, native in native_configs(config):
        target = f'{scope},job="{name}",instance="{native["identity"]["instance"]}"'
        native_up = f'up{{{target}}} == 1'
        suffix = hashlib.sha256(name.encode()).hexdigest()[:12]
        if native['identity']['service'] == 'eez-composer' and options['poster_funding_estimates']:
            estimate = f'reth_eez_poster_funding_estimate_valid{{{target}}}'
            balance = f'reth_eez_poster_funding_balance_valid{{{target}}}'
            at = f'reth_eez_poster_funding_estimate_timestamp_seconds{{{target}}}'
            affordable = f'reth_eez_poster_can_fund_candidate{{{target}}}'
            fresh = f'((time() - {at}) >= -5) and ((time() - {at}) <= 90)'
            known = f'({estimate} == 1) and on({keys}) ({balance} == 1) and on({keys}) ({fresh}) and on({keys}) ({native_up})'
            add('eez-funding-poster-' + suffix, 'EEZActualPosterCannotFundCandidate_' + native['identity']['instance'],
                f'({affordable} == bool 0) and on({keys}) ({known})',
                'The actual runtime poster cannot cover an observed candidate reserve (or even its proven blob minimum). This exact native decision is independent of observer-configured account identities.',
                severity='critical', hold='0s', dashboard='eez-gas')
            work = f'sum by(environment,deployment) (eez:queue_transactions:current{{{scope},state=~"queued_ready|in_flight"}}) > 0'
            # Filter zero values before union: a present healthy estimate must
            # not hide the unavailable balance/expired timestamp branch.
            unknown = f'((1 - {estimate}) > 0 or absent({estimate}) or (1 - {balance}) > 0 or absent({balance}) or absent({at}) or absent({affordable}) or (({affordable} != bool {affordable}) > 0) or ((time() - {at}) > 90) or (({at} - time()) > 5))'
            add('eez-funding-estimate-' + suffix, 'EEZPosterCandidateFundingUnknown_' + native['identity']['instance'],
                f'({unknown}) and on({keys}) ({native_up}) and on(environment,deployment) ({work})',
                'Work exists but no fresh actual-poster candidate/balance comparison is available. An old quote or unknown balance must not be interpreted as affordable.', dashboard='eez-gas')
        if native['identity']['service'] == 'eez-proof-signer':
            window = '30s' if fast_test else '5m'
            requests = f'eez_signer_requests_total{{{target},outcome=~"invalid|rejected"}}'
            upstream = (f'({metric("eez_observer_collection_success", "source=\"l1\"")} == 1) and on({keys}) '
                        f'({metric("eez_observer_collection_success", "source=\"l2\"")} == 1) and on({keys}) ({healthy})')
            count = f'sum by({keys},outcome) (increase({requests}[{window}]))'
            add('eez-signer-rejections-' + suffix, 'EEZSignerRepeatedRejections_' + native['identity']['instance'],
                f'({count} >= bool {options["signer_rejection_threshold"]}) and on({keys}) ({native_up}) and on(environment,deployment) ({upstream})',
                'Signer repeatedly rejected malformed or invalid proposals. Inspect bounded outcome and correlated logs; this warning does not prove valid work is blocked.', dashboard='eez-settlement')
            authorized = lambda outcome: f'eez_signer_authorized_requests_total{{{target},outcome={outcome}}}'
            failures = f'sum by({keys}) (increase({authorized('~"internal|deadline|panicked"')}[{window}]))'
            successes = f'sum by({keys}) (increase({authorized('"success"')}[{window}]))'
            add('eez-signer-authorized-' + suffix, 'EEZSignerAuthorizedAttestationFailures_' + native['identity']['instance'],
                f'({failures} >= bool {options["signer_authorized_failure_threshold"]}) and on({keys}) ({successes} == 0) and on({keys}) ({native_up}) and on(environment,deployment) ({upstream})',
                'Repeated fully validated requests failed to produce an authorized attestation response, with no successful authorized response in the same window. Invalid proposals and caller cancellation are excluded; this does not prove global queue causality.',
                severity='critical', dashboard='eez-settlement')
            missing_authorized = ' or '.join(f'absent({authorized(chr(34) + outcome + chr(34))})' for outcome in ('success','internal','deadline','panicked','cancelled'))
            add('eez-signer-auth-unknown-' + suffix, 'EEZSignerAuthorizedOutcomesUnknown_' + native['identity']['instance'],
                f'({missing_authorized}) and on({keys}) ({native_up})',
                'Signer metrics lack initialized authorized-response outcomes. Treat valid-work attestation status as unknown until this capability is present.', dashboard='eez-settlement')
            any_requests = f'eez_signer_requests_total{{{target}}}'
            add('eez-signer-metrics-' + suffix, 'EEZSignerRequestMetricsUnknown_' + native['identity']['instance'],
                f'absent({any_requests}) and on({keys}) ({native_up})',
                'Configured signer endpoint lacks finite request outcomes. Validate telemetry capability before interpreting an absent rejection counter as success.', dashboard='eez-settlement')
        if native['identity']['service'] != 'eez-composer' or options['pure_l2_inclusion_grace_seconds'] is None:
            continue
        def native_metric(metric, extra=''):
            return f'reth_eez_settlement_{metric}{{{target}{"," + extra if extra else ""}}}'
        enabled = native_metric('policy_enabled')
        evaluated = native_metric('policy_evaluation_valid')
        timestamp = native_metric('policy_evaluation_timestamp_seconds')
        age = native_metric('anchor_age_seconds')
        interval = native_metric('policy_setting', 'setting="pure_l2_interval_seconds"')
        pending = native_metric('pending_pure_l2_transactions')
        due = native_metric('due')
        fresh = f'((time() - {timestamp}) >= 0) and ((time() - {timestamp}) <= 45)'
        add('eez-policy-unknown-' + suffix, 'EEZPureL2PolicyEvaluationUnknown_' + native['identity']['instance'],
            f'(absent({enabled}) or (((1 - {evaluated}) > 0 or absent({evaluated}) or absent({timestamp}) or absent({pending}) or absent({due}) or absent({age}) or absent({interval}) or (({interval} != bool {interval}) > 0) or ((time() - {timestamp}) > 45) or (({timestamp} - time()) > 0)) and on({keys}) ({enabled} == 1))) and on({keys}) ({native_up})',
            'Configured policy-age monitoring lacks a fresh evaluation. Check node health and policy settings; an inactive policy has no overdue guarantee.', dashboard='eez-settlement')
        gate = (f'and on({keys},rollup) ({evaluated} == 1) and on({keys},rollup) ({fresh}) '
                f'and on({keys},rollup) ({pending} > 0) and on({keys},rollup) ({due} == 1) and on({keys}) ({enabled} == 1) and on({keys}) ({native_up})')
        for severity, title in [('warning', 'Warning'), ('critical', 'Critical')]:
            delay = options['pure_l2_inclusion_grace_seconds'] + (options['policy_critical_delay_seconds'] if severity == 'critical' else 0)
            add('eez-policy-' + severity + '-' + suffix, 'EEZPureL2PolicyOverdue' + title + '_' + native['identity']['instance'],
                f'(({age}) > bool on({keys}) group_left() ({interval})) {gate}',
                'Fresh due-and-pending pure-L2 work has persisted beyond its explicit inclusion grace (plus critical delay). The condition uses candidate-to-anchor policy age; new work after a long idle interval still receives the full grace.',
                severity=severity, hold=('10s' if fast_test and delay else f'{delay}s'), dashboard='eez-settlement')
    return specs


def alert_rules(config, *, fast_test=False):
    trace_window = '30s' if fast_test else '5m'
    labels = {'environment': config['environment'], 'deployment': config['deployment']}
    scope = ','.join(f'{k}="{v}"' for k, v in labels.items())
    observer = scope + f',job="eez-observer",instance="{config["observer_instance"]}"'
    up = f'up{{{observer}}}'
    queue_unknown = f'(1 - eez_observer_queue_evaluation_valid{{{scope}}}) and on(environment,deployment,job,instance) ({up} == 1)'
    queue_summary = 'Queue RPC may respond while readiness evaluation is stale or unknown.'
    if config.get('queue_source', 'rpc') == 'native':
        composers = [entry for entry in native_configs(config) if entry[2]['identity']['service'] == 'eez-composer']
        if len(composers) != 1:
            raise ValueError('native queue alerts require exactly one configured composer')
        name, _, native = composers[0]
        target = f'{scope},job="{name}",instance="{native["identity"]["instance"]}"'
        queue = f'eez:queue_evaluation_valid:current{{{target}}}'
        queue_unknown = f'((1 - {queue}) or absent({queue})) and on(environment,deployment,job,instance) (up{{{target}}} == 1)'
        queue_summary = 'Selected native queue evaluation is missing, invalid or older than its sample budget. Inspect the native producer; no RPC fallback is used.'
    specs = [
        ('eez-observer-down', 'EEZObserverUnavailable', f'(1 - {up}) or absent({up})', 'Alerting', 'critical',
         'Observer scrape is unavailable; inspect its process, private listener and network path.'),
        ('eez-queue-stale', 'EEZQueueEvaluationStale',
         queue_unknown, 'OK', 'warning', queue_summary),
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
    configured_agents = [(name, identity['instance']) for name, _, identity in client_targets(config)]
    node_agent = node_agent_config(config)
    if node_agent:
        configured_agents.append(('alloy-node-' + node_agent['instance'], node_agent['instance']))
    for job, instance in configured_agents:
        metric = f'up{{{scope},job="{job}",instance="{instance}"}}'
        specs.append(('eez-client-' + hashlib.sha256(job.encode()).hexdigest()[:16], 'EEZConfiguredAgentUnavailable_' + job,
            f'(1 - {metric}) or absent({metric})', 'Alerting', 'warning',
            'Configured client/node-host collector is unavailable. Check its private endpoint, process and forwarding/storage health; missing scrape data does not establish a healthy client.'))
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
        if native.get('lifecycle') is not None:
            prefix = '' if native['identity']['service'] == 'eez-proof-signer' else 'reth_'
            target = f'{scope},job="{name}",instance="{native["identity"]["instance"]}"'
            enabled = f'{prefix}eez_lifecycle_writer_enabled{{{target}}}'
            suffix = hashlib.sha256(name.encode()).hexdigest()[:16]
            specs.append(('eez-writer-' + suffix, 'EEZLifecycleWriterUnavailable_' + name,
                f'((1 - {enabled}) or absent({enabled})) and on(environment,deployment,job,instance) ({metric} == 1)',
                'OK', 'warning', 'Configured lifecycle writer is unavailable. Check spool ownership, disk space and writer errors; protocol operation does not prove event capture.'))
            failures = f'sum by(environment,deployment,job,instance) (increase({prefix}eez_telemetry_dropped_events_total{{{target},signal="lifecycle"}}[{trace_window}]))'
            errors = f'sum by(environment,deployment,job,instance) (increase({prefix}eez_telemetry_writer_errors_total{{{target}}}[{trace_window}]))'
            specs.append(('eez-lifecycle-loss-' + suffix, 'EEZLifecycleLoss_' + name,
                f'((({failures}) > 0) or (({errors}) > 0)) and on(environment,deployment,job,instance) ({metric} == 1)',
                'OK', 'warning', 'Lifecycle drops or writer errors were observed. Inspect cumulative control records and observer gaps; never reset protocol state to repair telemetry.'))
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
    specs.extend(protocol_alert_specs(config, scope, observer, fast_test=fast_test))
    protocol = protocol_alert_config(config)
    rules = []
    for spec in specs:
        uid, title, expr, nodata, severity, summary = spec[:6]
        extra = spec[6] if len(spec) > 6 else {}
        # Query models are raw JSON in Grafana provisioning; preserve the expression reference literally.
        rules.append({'uid': uid, 'title': title, 'condition': 'B', 'for': extra.get('for') or ('0s' if severity == 'heartbeat' else ('10s' if fast_test else '2m')),
            'noDataState': nodata, 'execErrState': 'Alerting', 'isPaused': not config.get('alerting_enabled', True),
            'labels': {**labels, 'severity': severity, **({'owner': protocol['owner']} if protocol else {})}, 'annotations': {'summary': summary,
                'runbook': extra.get('runbook', 'observability/runbooks/operations.md'),
                **({'runbook_url': grafana_public_url(config) + '/d/eez-runbooks?viewPanel=' + ('2' if 'protocol-alerts.md' in extra.get('runbook', '') else '1')} if grafana_public_url(config) else {}), '__dashboardUid__': extra.get('dashboard', 'eez-infrastructure'), '__panelId__': '1'},
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
