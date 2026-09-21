#!/usr/bin/env python3
"""Exercise the single-host Compose stack with simulated RPC data, never live nodes.

Default runs remove only their owned Compose resources and retain private diagnostics.
Use --runtime PATH --keep to leave a healthy, inspectable demo after successful checks.
"""
import argparse
import base64
import importlib.util
import json
import os
import pathlib
import shlex
import socket
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


def run(command, *, check=True, data=None, timeout=180):
    result = subprocess.run(command, input=data, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=timeout)
    if check and result.returncode:
        raise RuntimeError(f'Command failed ({result.returncode}): {shlex.join(command)}\n{result.stdout[-4000:]}')
    return result.stdout


def request(url, data=None, password=None):
    headers = {'Content-Type': 'application/json'}
    if password is not None:
        headers['Authorization'] = 'Basic ' + base64.b64encode(('admin:' + password).encode()).decode()
    body = None if data is None else json.dumps(data).encode()
    with urllib.request.urlopen(urllib.request.Request(url, data=body, headers=headers), timeout=15) as response:
        return json.load(response)


def until(description, check, seconds=150):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (urllib.error.URLError, TimeoutError, ConnectionError, json.JSONDecodeError):
            pass
        time.sleep(1)
    raise RuntimeError('Timed out: ' + description)


def available_port(address):
    with socket.socket() as listener:
        listener.bind((address, 0))
        return listener.getsockname()[1]


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')
    path.chmod(0o600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True, help='Existing verified local node image; no build or pull')
    parser.add_argument('--runtime', type=pathlib.Path, help='New or empty private demo directory')
    parser.add_argument('--keep', action='store_true', help='Keep the healthy demo after checks; requires --runtime')
    args = parser.parse_args()
    if args.keep and args.runtime is None:
        parser.error('--keep requires an explicit --runtime directory')
    if args.runtime is not None and (args.runtime.is_symlink() or
            args.runtime.exists() and (not args.runtime.is_dir() or any(args.runtime.iterdir()))):
        parser.error('--runtime must be a new or empty nonsymlink directory')
    directory = (args.runtime or pathlib.Path(tempfile.mkdtemp(prefix='eez-single-host-'))).resolve()
    if any(character in str(directory) for character in '\n\r$"\\'):
        parser.error('--runtime contains unsupported environment-file characters')
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    directory.chmod(0o700)
    project = 'eez-single-host-' + uuid.uuid4().hex[:12]
    runtime, logs, spool = (directory / name for name in ('runtime', 'logs', 'lifecycle'))
    logs.mkdir()
    spool.mkdir()
    report = {'scope': 'Single-machine Compose smoke with simulated RPC data; no live deployment',
              'project': project, 'checks': [], 'status': 'preparing'}
    compose = None
    retained = False
    try:
        image = json.loads(run(['docker', 'image', 'inspect', args.image]))[0]
        # Freeze the local image identity so a concurrently changed tag cannot
        # change which executable the migration and indexer consume.
        image_id = image['Id']
        report['node_image_id'] = image_id
        rpc_port = available_port('127.0.0.1')
        database_port = available_port('127.0.0.1')
        while database_port == rpc_port:
            database_port = available_port('127.0.0.1')
        config = {'environment': 'devnet', 'deployment': project, 'observer_instance': 'simulated-node',
                  'observer_target': 'observer:19100', 'log_directory': str(logs),
                  'alerting_enabled': False, 'database_enabled': True, 'postgres_port': database_port,
                  'grafana_port': 0, 'prometheus_port': 0, 'loki_port': 0}
        prepare.prepare(config, runtime)
        write_json(directory / 'stack-input.json', config)
        endpoint = lambda chain: {'url': f'http://simulated-rpc:9000/l{chain}', 'chain_id': chain,
                                  'genesis_hash': '0x' + f'{chain * 100:064x}'}
        indexer = {'exporter': {'l1': endpoint(1), 'l2': endpoint(2), 'listen': '0.0.0.0:19100',
                               'allow_non_loopback': True}, 'environment': 'devnet', 'deployment': project,
                   'rollup_id': 1, 'database_config': '/config/writer.json',
                   'l1': {'bootstrap': 1, 'wallets': ['0x' + 'aa' * 20], 'contracts': ['0x' + 'bb' * 20]},
                   'l2': {'bootstrap': 1, 'include_all_transactions': True}, 'batch_blocks': 16}
        indexer_path = directory / 'indexer.json'
        write_json(indexer_path, indexer)
        env_path = runtime / 'compose.env'
        with env_path.open('a') as stream:
            for key, value in {'EEZ_NODE_IMAGE': image_id, 'OBS_OBSERVER_UID': os.getuid(),
                               'OBS_OBSERVER_GID': os.getgid(), 'OBS_INDEXER_CONFIG': indexer_path,
                               'OBS_LIFECYCLE_DIR': spool}.items():
                stream.write(f'{key}="{value}"\n')
        fixtures = json.loads((ROOT / 'tests/images.lock.json').read_text())
        fixture_image = next(name + '@' + digest for name, digest in fixtures.items() if name.startswith('python:'))
        fixture_control = {'phase': 'healthy', 'post_success': True, 'correlation': 'valid',
                           'protocol_logs': 'valid', 'finalize': True, 'funding_base_time': int(time.time())}
        control_path = directory / 'fixture-control.json'
        write_json(control_path, fixture_control)
        # These public synthetic settings contain no credentials. The enclosing
        # private directory protects them on the host; the fixture UID reads the
        # individual read-only mount, including after a container restart.
        control_path.chmod(0o644)
        report['simulated_genesis_time'] = fixture_control['funding_base_time']
        mounts = {'indexer-rpc.py': '/rpc.py', 'indexer-rpc-start.py': '/start-rpc.py',
                  'fixtures/empty-post-calldata.hex': '/post-calldata.hex',
                  'fixtures/multi-rollup-post-calldata.hex': '/multi-rollup-post-calldata.hex',
                  'fixtures/correlation-page-digests.json': '/correlation-page-digests.json',
                  'fixtures/blob-carriers.json': '/blob-carriers.json'}
        fixture_file = directory / 'compose.fixture.json'
        write_json(fixture_file, {'services': {'simulated-rpc': {'image': fixture_image,
            'command': ['python3', '/start-rpc.py'], 'user': '65534', 'restart': 'unless-stopped',
            'ports': [f'127.0.0.1:{rpc_port}:9000'], 'read_only': True,
            'volumes': [str(ROOT / 'tests' / source) + ':' + target + ':ro' for source, target in mounts.items()]
                       + [str(control_path) + ':/fixture-control.json:ro'],
            'mem_limit': '128m', 'cpus': 0.5, 'pids_limit': 128, 'cap_drop': ['ALL'],
            'security_opt': ['no-new-privileges:true'],
            'logging': {'driver': 'local', 'options': {'max-size': '10m', 'max-file': '3'}}}}})
        compose = ['docker', 'compose', '--project-name', project, '--profile', 'database',
                   '--env-file', str(env_path), '-f', str(ROOT / 'compose.yaml'),
                   '-f', str(ROOT / 'compose.observer.yaml'), '-f', str(fixture_file)]
        wrapper = directory / 'compose.sh'
        wrapper.write_text('#!/usr/bin/env bash\nset -euo pipefail\nexec ' + shlex.join(compose) + ' "$@"\n')
        wrapper.chmod(0o700)
        rendered = json.loads(run(compose + ['config', '--format', 'json']))
        write_json(directory / 'compose.rendered.json', rendered)
        mounted = {entry['target'] for entry in rendered['services']['observer']['volumes']}
        assert mounted == {'/config/indexer.json', '/config/writer.json', '/lifecycle'}, mounted
        assert 'network_mode' not in rendered['services']['observer']
        assert set(rendered['services']['observer']['networks']) == {'default', 'ledger'}
        assert not rendered['services']['observer'].get('ports')
        run(compose + ['up', '-d', '--wait', 'postgres', 'simulated-rpc'])
        fixture_url = f'http://127.0.0.1:{rpc_port}'
        until('simulated RPC source', lambda: request(fixture_url + '/control', fixture_control)['ok'])

        def container(service):
            return run(compose + ['ps', '-q', service]).strip()

        def sql(statement):
            return run(['docker', 'exec', '-i', container('postgres'), 'psql', '-XAtq',
                        '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-d', 'eez_observer'], data=statement).strip()

        assert sql("SELECT to_regnamespace('eez') IS NULL;") == 't'
        run(compose + ['run', '--rm', 'observer-migrate'])
        report['checks'].append('explicit_compose_migration_before_writer_start')
        run(compose + ['up', '-d', '--wait'])
        actual = json.loads(run(['docker', 'inspect', container('observer')]))[0]
        assert actual['Image'] == image_id
        assert actual['HostConfig']['ReadonlyRootfs']
        assert actual['HostConfig']['NetworkMode'] != 'host'
        assert not actual['HostConfig']['PortBindings']
        assert {mount['Destination'] for mount in actual['Mounts']} == mounted
        assert all(not mount['RW'] for mount in actual['Mounts'])
        report['checks'].append('verified_image_readonly_observer_without_admin_credentials')
        totals_query = 'SELECT role,observed_receipts,known_total_paid::text FROM eez.canonical_cost_totals ORDER BY role;'
        expected_totals = 'l1|2|738216\nl2|2|738216'
        until('both canonical receipt ledgers', lambda: sql(totals_query) == expected_totals)
        until('complete paged settlement', lambda: sql('SELECT count(*) FROM eez.canonical_settled_l2_blocks;') == '2')
        report['canonical_cost_totals_wei'] = {'l1': '738216', 'l2': '738216'}
        report['checks'].append('real_observer_indexes_simulated_receipts_and_settlement_exactly')

        def base(service, port):
            return 'http://' + run(compose + ['port', service, str(port)]).strip()

        prometheus, grafana = base('prometheus', 9090), base('grafana', 3000)
        password = (runtime / 'secrets/grafana-admin-password').read_text()
        def prom(expression):
            return request(prometheus + '/api/v1/query?' + urllib.parse.urlencode({'query': expression}))['data']['result']
        until('Prometheus scrapes observer', lambda: prom('up{job="eez-observer"} == 1'))
        until('both receipt indexing metrics', lambda: len(prom('eez_observer_index_collection_success == 1')) == 2)
        dashboards = ['eez-blobs', 'eez-cross-chain', 'eez-gas', 'eez-infrastructure',
                      'eez-network', 'eez-runbooks', 'eez-settlement']
        until('Grafana dashboards', lambda: sorted(item['uid'] for item in request(
            grafana + '/api/search?tag=eez', password=password)) == dashboards)
        query = {'queries': [{'refId': 'A', 'datasource': {'uid': 'eez-postgres', 'type': 'postgres'},
                 'rawSql': 'SELECT role,known_total_paid::text AS total_wei FROM eez.canonical_cost_totals ORDER BY role',
                 'format': 'table', 'rawQuery': True}], 'from': 'now-24h', 'to': 'now'}
        response = request(grafana + '/api/ds/query', query, password)
        result = response['results']['A']
        assert not result.get('error'), result
        assert result['frames'][0]['data']['values'] == [['l1', 'l2'], ['738216', '738216']], result
        rules = request(grafana + '/api/v1/provisioning/alert-rules', password=password)
        assert rules and all(rule['isPaused'] for rule in rules)
        contacts = request(grafana + '/api/v1/provisioning/contact-points', password=password)
        assert not any(contact.get('uid') in ('eez-operator-webhook', 'eez-heartbeat-webhook') for contact in contacts)
        report['checks'].append('prometheus_grafana_sql_dashboards_and_paused_alerts_without_webhooks')
        before = sql('SELECT count(*) FROM eez.receipt_observations;')
        run(compose + ['restart', 'observer'])
        restarted_at = sql('SELECT extract(epoch FROM now());')
        until('observer restarted and indexing', lambda: sql(totals_query) == expected_totals and
              sql('SELECT count(*) FROM eez.chain_instances WHERE last_success_at > '
                  f'to_timestamp({restarted_at});') == '2' and prom('up{job="eez-observer"} == 1'))
        # Both cursors completed a collection after restart. A still-fresh old
        # scrape alone cannot establish that the process replayed its checkpoint.
        assert sql(totals_query) == expected_totals
        assert sql('SELECT count(*) FROM eez.receipt_observations;') == before
        report['checks'].append('observer_restart_and_repeated_ingestion_do_not_duplicate_fees')
        # Reusing the persisted fixture timestamp is essential: moving it on
        # restart would change historical headers while retaining their hashes.
        run(compose + ['stop'])
        run(compose + ['up', '-d', '--wait'])
        prometheus, grafana = base('prometheus', 9090), base('grafana', 3000)
        restarted_at = sql('SELECT extract(epoch FROM now());')
        until('full stack resumes original simulated history', lambda: sql(totals_query) == expected_totals and
              sql('SELECT count(*) FROM eez.chain_instances WHERE last_success_at > '
                  f'to_timestamp({restarted_at});') == '2' and prom('up{job="eez-observer"} == 1'))
        assert sql('SELECT count(*) FROM eez.receipt_observations;') == before
        assert request(grafana + '/api/ds/query', query, password)['results']['A']['frames'][0]['data']['values'] == [
            ['l1', 'l2'], ['738216', '738216']]
        report['checks'].append('full_compose_restart_preserves_fixture_history_and_dashboard_data')
        report.update(status='passed', grafana_url=grafana, prometheus_url=prometheus,
                      credentials_file=str(runtime / 'secrets/grafana-admin-password'), retained=args.keep)
        stop = 'bash ' + shlex.quote(str(wrapper)) + ' stop'
        cleanup = 'bash ' + shlex.quote(str(wrapper)) + ' down --volumes --remove-orphans'
        report.update(stop_command=stop, cleanup_command=cleanup)
        (directory / 'DEMO.md').write_text(
            '# Simulated EEZ monitoring demo\n\nThis isolated Compose project uses simulated RPC receipts; it does not monitor or modify live nodes.\n\n'
            f'Grafana: {grafana}\n\nUser: admin. Password file: `{runtime / "secrets/grafana-admin-password"}`.\n\n'
            'Open the network, settlement, gas and blob dashboards. Select this devnet deployment; queue and optional lifecycle signals remain unknown because the fixture does not invent them. Alert rules are paused and there are no configured webhook recipients.\n\n'
            f'Stop while preserving data:\n```bash\n{stop}\n```\n\nRestart with the same simulated history:\n```bash\nbash {shlex.quote(str(wrapper))} up -d --wait\n```\n\nRemove only this demo and its volumes:\n```bash\n{cleanup}\n```\n')
        retained = args.keep
        print(f'PASS: single-host Compose smoke. {"Healthy demo retained" if retained else "Owned stack will be removed"}: {directory}')
        if retained:
            print(f'Grafana: {grafana}\nPassword file: {report["credentials_file"]}\nStop: {stop}')
    except Exception:
        report['status'] = 'failed'
        if compose is not None:
            (directory / 'failure.log').write_text(run(compose + ['logs', '--tail', '80'], check=False))
        raise
    finally:
        if compose is not None and not retained:
            run(compose + ['down', '--volumes', '--remove-orphans'], check=False)
            containers = run(['docker', 'ps', '-aq', '--filter', 'label=com.docker.compose.project=' + project])
            volumes = run(['docker', 'volume', 'ls', '-q', '--filter', 'label=com.docker.compose.project=' + project])
            if containers.strip() or volumes.strip():
                report['status'] = 'cleanup_failed'
                write_json(directory / 'report.json', report)
                raise RuntimeError('Owned Compose resources remain; see private runtime ' + str(directory))
            report['owned_resource_cleanup'] = 'verified'
        write_json(directory / 'report.json', report)


if __name__ == '__main__':
    main()
