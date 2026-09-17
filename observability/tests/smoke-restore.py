#!/usr/bin/env python3
"""Restore standard backups into a new, isolated monitoring project and database."""
import argparse
import copy
import hashlib
import importlib.util
import json
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import time
import uuid
from datetime import datetime, timezone

import lifecycle_wire

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('trace_test', ROOT / 'tests/smoke-traces.py')
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)
run, request, until = trace.run, trace.request, trace.until
IMAGES = json.loads((ROOT / 'images.lock.json').read_text())
FIXTURE_IMAGE = next(name + '@' + digest for name, digest in json.loads((ROOT / 'tests/images.lock.json').read_text()).items())
DIAGNOSTIC_VOLUMES = ['grafana-data', 'prometheus-data', 'loki-data', 'alloy-data', 'tempo-data', 'alloy-traces-data']
DIAGNOSTIC_SERVICES = ['grafana', 'prometheus', 'loki', 'alloy', 'tempo', 'alloy-traces', 'blackbox', 'fixture']


def digest(path):
    with pathlib.Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def utc():
    return datetime.now(timezone.utc).isoformat()


def docker_input(container, path, value):
    subprocess.run(['docker', 'exec', '-i', container, 'sh', '-c', 'umask 077; cat > "$1"', 'sh', path],
                   input=json.dumps(value).encode(), check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def safe_extract(archive, destination):
    destination = pathlib.Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as source:
        source.extractall(destination, filter='data')


# Cold data volumes require their numerical ownership on the new host. This
# helper has access only to the named test volume and private backup directory.
VOLUME_SCRIPT = '''import pathlib,sys,tarfile
mode,path=sys.argv[1:]
if mode=='backup':
    with tarfile.open(path,'w:gz') as archive: archive.add('/volume',arcname='.')
else:
    with tarfile.open(path) as archive:
        for member in archive.getmembers():
            name=pathlib.PurePosixPath(member.name)
            if name.is_absolute() or '..' in name.parts or not (member.isfile() or member.isdir()):
                raise ValueError('unexpected archive entry')
            member.mode &= 0o777
        archive.extractall('/volume',numeric_owner=True,filter='fully_trusted')
'''


def volume_archive(volume, directory, filename, mode):
    assert volume.startswith(('eez-restore-src-', 'eez-restore-dst-'))
    run(['docker', 'run', '--rm', '--network', 'none', '--user', '0', '--cap-drop', 'ALL',
         '--cap-add', 'DAC_OVERRIDE', '--cap-add', 'CHOWN', '--cap-add', 'FOWNER',
         '--security-opt', 'no-new-privileges:true',
         '-v', volume + ':/volume' + (':ro' if mode == 'backup' else ''),
         '-v', str(directory) + ':/backup', FIXTURE_IMAGE, 'python3', '-c', VOLUME_SCRIPT, mode, '/backup/' + filename])


class Stack:
    def __init__(self, name, directory):
        assert name.startswith(('eez-restore-src-', 'eez-restore-dst-'))
        self.name, self.directory = name, directory
        self.runtime = directory / 'runtime'
        self.logs, self.spool = directory / 'logs', directory / 'spool'
        self.compose_file = directory / 'compose.json'
        self.compose = ['docker', 'compose', '--project-name', name, '--profile', 'database', '--profile', 'traces', '-f', str(self.compose_file)]
        self.rendered = None

    def create_source(self, binary, logical_deployment):
        self.directory.mkdir()
        self.logs.mkdir()
        self.spool.mkdir()
        shutil.copyfile(binary, self.directory / 'observer')
        (self.directory / 'observer').chmod(0o755)
        self.config = {'environment': 'devnet', 'deployment': logical_deployment, 'observer_instance': 'fixture',
                       'observer_target': 'fixture:9000', 'log_directory': str(self.logs),
                       'alert_webhook_url': 'http://fixture:9000/alerts', 'heartbeat_webhook_url': 'http://fixture:9000/heartbeat',
                       'database_enabled': True, 'traces_enabled': True,
                       'grafana_port': 0, 'prometheus_port': 0, 'loki_port': 0, 'tempo_port': 0, 'otlp_http_port': 0}
        trace.prepare.prepare(self.config, self.runtime, fast_test=True)
        (self.runtime / 'stack-input.json').write_text(json.dumps(self.config))
        endpoint = lambda chain: {'url': f'http://rpc:9000/l{chain}', 'chain_id': chain, 'genesis_hash': '0x' + f'{chain * 100:064x}'}
        index = {'exporter': {'l1': endpoint(1), 'l2': endpoint(2)}, 'environment': 'devnet', 'deployment': logical_deployment,
                 'rollup_id': 1, 'database_config': '/tmp/writer.json', 'l1': {'bootstrap': 1, 'wallets': ['0x' + 'aa' * 20]},
                 'l2': {'bootstrap': 1, 'include_all_transactions': True}, 'batch_blocks': 16,
                 'lifecycle': [{'service': 'composer', 'instance': 'fixture-node', 'directory': '/lifecycle'}]}
        (self.runtime / 'indexer.json').write_text(json.dumps(index))
        override = self.directory / 'override.json'
        override.write_text(json.dumps({'services': {
            'postgres': {'volumes': [str(self.directory / 'observer') + ':/observer:ro', str(self.spool) + ':/lifecycle:ro']},
            'fixture': {'image': FIXTURE_IMAGE, 'command': ['python3', '/fixture.py'], 'ports': ['127.0.0.1:0:9000'],
                        'volumes': [str(ROOT / 'tests/fixture-server.py') + ':/fixture.py:ro'],
                        'mem_limit': '128m', 'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true']},
            'rpc': {'image': FIXTURE_IMAGE, 'command': ['python3', '/rpc.py'], 'networks': ['ledger'],
                    'volumes': [str(ROOT / 'tests/indexer-rpc.py') + ':/rpc.py:ro',
                                str(ROOT / 'tests/fixtures/empty-post-calldata.hex') + ':/post-calldata.hex:ro',
                                str(ROOT / 'tests/fixtures/multi-rollup-post-calldata.hex') + ':/multi-rollup-post-calldata.hex:ro',
                                str(ROOT / 'tests/fixtures/blob-carriers.json') + ':/blob-carriers.json:ro'],
                    'mem_limit': '128m', 'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true']}
        }}))
        self.rendered = json.loads(run(['docker', 'compose', '--project-name', self.name, '--profile', 'database', '--profile', 'traces',
            '--env-file', str(self.runtime / 'compose.env'), '-f', str(ROOT / 'compose.yaml'), '-f', str(override), 'config', '--format', 'json']))
        # The fixture's observer runs inside PostgreSQL's container; no database
        # host port is needed. Rendered Compose avoids ambiguous override merging.
        self.rendered['services']['postgres']['ports'] = []
        self.compose_file.write_text(json.dumps(self.rendered))

    def container(self, service):
        return run(self.compose + ['ps', '-q', service]).strip()

    def base(self, service, port):
        return 'http://' + run(self.compose + ['port', service, str(port)]).strip()

    def sql(self, statement, check=True):
        result = subprocess.run(['docker', 'exec', '-i', self.container('postgres'), 'psql', '-XAtq', '-v', 'ON_ERROR_STOP=1',
                                 '-U', 'postgres', '-d', 'eez_observer'], input=statement, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
        if check and result.returncode:
            raise RuntimeError(result.stdout[-4000:])
        return result

    def role_sql(self, role, statement, check=True):
        assert role in ('reader', 'writer')
        command = f'export PGPASSWORD="$(cat /run/secrets/postgres_{role}_password)"; exec psql -XAtq -v ON_ERROR_STOP=1 -h 127.0.0.1 -U eez_observer_{role} -d eez_observer -c "$1"'
        result = subprocess.run(['docker', 'exec', self.container('postgres'), 'sh', '-c', command, 'sh', statement],
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
        if check and result.returncode:
            raise RuntimeError(result.stdout[-4000:])
        return result

    def configure_observer(self):
        for role in ('admin', 'writer'):
            config = json.loads((self.runtime / f'database/{role}.json').read_text())
            config['connection_string'] = config['connection_string'].replace('port=19432', 'port=5432')
            docker_input(self.container('postgres'), '/tmp/' + role + '.json', config)
        docker_input(self.container('postgres'), '/tmp/indexer.json', json.loads((self.runtime / 'indexer.json').read_text()))

    def migrate(self):
        run(['docker', 'exec', self.container('postgres'), '/observer', 'migrate', '--database-config', '/tmp/admin.json'])

    def index(self):
        run(['docker', 'exec', self.container('postgres'), '/observer', 'indexer', '--config', '/tmp/indexer.json', '--once'])

    def control(self, phase):
        script = "import urllib.request,sys; urllib.request.urlopen(urllib.request.Request('http://localhost:9000/control',data=sys.stdin.buffer.read(),headers={'Content-Type':'application/json'})).read()"
        subprocess.run(['docker', 'exec', '-i', self.container('rpc'), 'python3', '-c', script],
                       input=json.dumps({'phase': phase}).encode(), check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def event(self, sequence, trace_id):
        identity = {'environment': 'devnet', 'deployment': self.config['deployment'], 'service': 'composer', 'instance': 'fixture-node'}
        epoch, segment = '31' * 16, f'{sequence:032x}'
        payload = {'observed_at_unix_ms': int(time.time() * 1000), 'trace': {'trace_id': trace_id, 'span_id': '0000000000000001'},
                   'event': {'event': 'reorg', 'ancestor': {'number': 1, 'hash': '03' * 32}}}
        (self.spool / (epoch + '.lock')).touch(mode=0o644)
        (self.spool / f'{epoch}-{segment}.closed').write_bytes(lifecycle_wire.header(identity, epoch, segment) + b''.join(lifecycle_wire.chunks(identity, epoch, sequence, payload=payload)))

    def snapshot(self):
        result = self.sql("""SELECT jsonb_build_object(
          'migrations',(SELECT jsonb_agg(jsonb_build_array(version,encode(checksum,'hex')) ORDER BY version) FROM eez.schema_migrations),
          'chains',(SELECT jsonb_agg(jsonb_build_array(role,chain_id::text,encode(genesis_hash,'hex'),rollup_id::text,bootstrap_number::text,selection) ORDER BY role) FROM eez.chain_instances),
          'cursors',(SELECT jsonb_agg(jsonb_build_array(c.role,encode(s.block_hash,'hex')) ORDER BY c.role) FROM eez.scan_cursors s JOIN eez.chain_instances c ON c.id=s.chain_instance_id),
          'costs',(SELECT jsonb_agg(jsonb_build_array(role,known_total_paid::text) ORDER BY role) FROM eez.canonical_cost_totals),
          'receipt_observations',(SELECT count(*) FROM eez.receipt_observations),
          'events',(SELECT jsonb_agg(jsonb_build_array(source_id,encode(epoch,'hex'),sequence::text,state,payload) ORDER BY source_id,epoch,sequence) FROM eez.lifecycle_events),
          'checkpoints',(SELECT jsonb_agg(jsonb_build_array(source_id,encode(epoch,'hex'),encode(segment_id,'hex'),offset_bytes,encode(header_digest,'hex')) ORDER BY source_id,epoch,segment_id) FROM eez.lifecycle_segments));""").stdout.strip()
        return json.loads(result)

    def grafana(self, path):
        password = (self.runtime / 'secrets/grafana-admin-password').read_text()
        return request(self.base('grafana', 3000) + path, password=password)

    def volume(self, key):
        value = self.rendered['volumes'][key]
        assert not value.get('external', False)
        return value['name']

    def cleanup(self):
        if self.compose_file.exists():
            run(self.compose + ['down', '--volumes', '--remove-orphans'], check=False)
            for command in (['docker', 'ps', '-aq'], ['docker', 'volume', 'ls', '-q'], ['docker', 'network', 'ls', '-q']):
                assert not run(command + ['--filter', 'label=com.docker.compose.project=' + self.name]).strip(), 'owned resources remain'


def verify_roles(stack):
    assert stack.sql("SELECT count(*) FROM pg_roles WHERE rolname IN ('eez_observer_writer','eez_observer_reader') AND rolcanlogin AND NOT(rolsuper OR rolcreatedb OR rolcreaterole OR rolbypassrls);").stdout.strip() == '2'
    settings = stack.role_sql('reader', 'SHOW jit; SHOW statement_timeout; SHOW lock_timeout; SHOW idle_in_transaction_session_timeout;').stdout.strip().splitlines()
    assert settings == ['off', '10s', '5s', '10s'], settings
    assert stack.role_sql('writer', 'SHOW jit;').stdout.strip() == 'off'
    stack.role_sql('reader', 'SELECT count(*) FROM eez.canonical_receipts;')
    denied = stack.role_sql('reader', 'SELECT count(*) FROM eez.lifecycle_records;', check=False)
    assert denied.returncode != 0 and 'permission denied' in denied.stdout
    denied = stack.role_sql('reader', 'DELETE FROM eez.receipt_observations;', check=False)
    assert denied.returncode != 0 and 'permission denied' in denied.stdout


def archive_tree(source, output):
    with tarfile.open(output, 'w:gz') as archive:
        archive.add(source, arcname='.')
    output.chmod(0o600)


def backup(stack, destination):
    destination.mkdir(mode=0o700)
    with (destination / 'ledger.dump').open('wb') as output:
        subprocess.run(['docker', 'exec', stack.container('postgres'), 'pg_dump', '-U', 'postgres', '-d', 'eez_observer',
                        '--format=custom', '--no-owner'], stdout=output, stderr=subprocess.PIPE, check=True)
    (destination / 'ledger.dump').chmod(0o600)
    run(stack.compose + ['stop'] + DIAGNOSTIC_SERVICES)
    for key in DIAGNOSTIC_VOLUMES:
        volume_archive(stack.volume(key), destination, key + '.tar.gz', 'backup')
    for name in ('runtime', 'logs', 'spool'):
        archive_tree(stack.directory / name, destination / (name + '.tar.gz'))
    shutil.copyfile(stack.directory / 'observer', destination / 'observer')
    assets = destination / 'assets'
    assets.mkdir()
    bind_assets = {}
    for service in stack.rendered['services'].values():
        for volume in service.get('volumes', []):
            if volume.get('type') != 'bind':
                continue
            path = pathlib.Path(volume['source'])
            if path.is_relative_to(stack.directory):
                continue
            assert path.is_relative_to(ROOT) and path.is_file(), 'backup must include each static mounted asset'
            relative = path.relative_to(ROOT)
            target = assets / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
            bind_assets[str(path)] = str(pathlib.Path('assets') / relative)
    (destination / 'compose-source.json').write_text(json.dumps(stack.rendered))
    manifest = {'source_project': stack.name, 'source_directory': str(stack.directory), 'bind_assets': bind_assets,
                'files': {str(path.relative_to(destination)): digest(path) for path in destination.rglob('*') if path.is_file()}}
    (destination / 'manifest.json').write_text(json.dumps(manifest))
    (destination / 'manifest.json').chmod(0o600)
    return manifest


def restore_files(stack, backup_directory):
    manifest = json.loads((backup_directory / 'manifest.json').read_text())
    for name, expected in manifest['files'].items():
        if digest(backup_directory / name) != expected:
            raise ValueError('backup checksum mismatch')
    stack.directory.mkdir(mode=0o700)
    for name in ('runtime', 'logs', 'spool'):
        safe_extract(backup_directory / (name + '.tar.gz'), stack.directory / name)
    shutil.copyfile(backup_directory / 'observer', stack.directory / 'observer')
    (stack.directory / 'observer').chmod(0o755)
    shutil.copytree(backup_directory / 'assets', stack.directory / 'assets')
    stack.config = json.loads((stack.runtime / 'stack-input.json').read_text())
    stack.config['log_directory'] = str(stack.logs)
    stack.rendered = json.loads((backup_directory / 'compose-source.json').read_text())
    old_project, old_directory = manifest['source_project'], pathlib.Path(manifest['source_directory'])
    stack.rendered['name'] = stack.name
    for category in ('volumes', 'networks'):
        for value in stack.rendered[category].values():
            assert value['name'].startswith(old_project + '_') and not value.get('external', False)
            value['name'] = stack.name + value['name'][len(old_project):]
    for value in stack.rendered.get('secrets', {}).values():
        path = pathlib.Path(value['file'])
        value['file'] = str(stack.directory / path.relative_to(old_directory))
    for service in stack.rendered['services'].values():
        for volume in service.get('volumes', []):
            if volume.get('type') != 'bind':
                continue
            path = pathlib.Path(volume['source'])
            volume['source'] = str(stack.directory / path.relative_to(old_directory)) if path.is_relative_to(old_directory) else str(stack.directory / manifest['bind_assets'][str(path)])
    env = stack.runtime / 'compose.env'
    env.write_text(env.read_text().replace(str(old_directory), str(stack.directory)))
    (stack.runtime / 'stack-input.json').write_text(json.dumps(stack.config))
    assert str(old_directory) not in json.dumps(stack.rendered), 'restoration must not retain source-host mounts'
    for service in stack.rendered['services'].values():
        for volume in service.get('volumes', []):
            if volume.get('type') == 'bind':
                assert pathlib.Path(volume['source']).is_relative_to(stack.directory)
    stack.compose_file.write_text(json.dumps(stack.rendered))
    for key in stack.rendered['volumes']:
        result = subprocess.run(['docker', 'volume', 'inspect', stack.volume(key)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        assert result.returncode != 0, 'destination must use entirely new volumes'
    run(stack.compose + ['create'])
    for key in DIAGNOSTIC_VOLUMES:
        volume_archive(stack.volume(key), backup_directory, key + '.tar.gz', 'restore')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    suffix = uuid.uuid4().hex[:10]
    report = {'scope': 'new isolated project and empty volumes on the same Docker host; no operator services',
              'started_at': utc(), 'binary_sha256': digest(args.binary), 'images': IMAGES, 'checks': [], 'status': 'failed'}
    with tempfile.TemporaryDirectory(prefix='eez-restore-test-') as temporary:
        directory = pathlib.Path(temporary)
        source = Stack('eez-restore-src-' + suffix, directory / 'source')
        restored = Stack('eez-restore-dst-' + suffix, directory / 'restored')
        report['projects'] = [source.name, restored.name]
        try:
            source.create_source(args.binary, 'restore-' + suffix)
            assert digest(source.directory / 'observer') == report['binary_sha256']
            trace_id = uuid.uuid4().hex
            source.event(1, trace_id)
            run(source.compose + ['up', '-d', '--wait', '--wait-timeout', '150', 'postgres', 'rpc'])
            source.configure_observer()
            source.migrate()
            expected_migrations = [[int(path.name.split('-')[0]), digest(path)] for path in sorted((ROOT / 'migrations').glob('*.sql'))]
            report['migration_sha256'] = {path.name: digest(path) for path in sorted((ROOT / 'migrations').glob('*.sql'))}
            source.control('healthy')
            source.index()
            verify_roles(source)
            baseline = source.snapshot()
            assert baseline['migrations'] == expected_migrations, 'binary and checked-out migrations must match'
            assert len(baseline['events']) == 1 and baseline['events'][0][3] == 'complete'
            assert dict(baseline['costs'])['l1'] == '738216'
            run(source.compose + ['up', '-d', '--wait', '--wait-timeout', '150'] + DIAGNOSTIC_SERVICES)
            until('source Tempo ready', lambda: request(source.base('tempo', 3200) + '/ready', raw=True).strip() == 'ready')
            request(source.base('alloy-traces', 4318) + '/v1/traces', trace.trace_payload(trace_id, source.config['deployment']))
            until('source trace indexed', lambda: source.grafana('/api/datasources/proxy/uid/eez-tempo/api/traces/' + trace_id))
            (source.logs / 'restore.jsonl').write_text(json.dumps({'trace_id': trace_id, 'message': 'restore-fixture'}) + '\n')
            log_expression = '{deployment="' + source.config['deployment'] + '"} |= "' + trace_id + '"'
            def logs(stack):
                return request(stack.base('loki', 3100) + '/loki/api/v1/query_range?' + trace.urllib.parse.urlencode({'query': log_expression}))['data']['result']
            until('source diagnostic log retained', lambda: logs(source))
            dashboards = sorted(item['uid'] for item in source.grafana('/api/search?tag=eez'))
            assert len(dashboards) == 6, dashboards
            contacts = source.grafana('/api/v1/provisioning/contact-points')
            policies = source.grafana('/api/v1/provisioning/policies')
            def prom(stack, expression):
                return request(stack.base('prometheus', 9090) + '/api/v1/query?' + trace.urllib.parse.urlencode({'query': expression}))['data']['result']
            until('source Prometheus sample', lambda: prom(source, 'up{job="prometheus"}'))
            historical_time = time.time()
            snapshot_started = time.monotonic()
            report['canonical_snapshot_at'] = utc()
            backup_directory = directory / 'backup'
            manifest = backup(source, backup_directory)
            report['backup_seconds'] = time.monotonic() - snapshot_started
            report['archive_bytes'] = {name: (backup_directory / name).stat().st_size for name in manifest['files'] if name.endswith(('.dump', '.tar.gz'))}
            report['checks'].append('consistent_sql_dump_and_cold_configuration_diagnostic_archives')
            # Reject incomplete/corrupt backup sets before creating destination
            # directories or Docker resources. The original archive stays intact.
            manifest_path = backup_directory / 'manifest.json'
            original_manifest = manifest_path.read_bytes()
            broken = copy.deepcopy(manifest)
            broken['files']['ledger.dump'] = '0' * 64
            manifest_path.write_text(json.dumps(broken))
            rejected = Stack('eez-restore-dst-reject-' + suffix, directory / 'rejected')
            try:
                try:
                    restore_files(rejected, backup_directory)
                    raise AssertionError('incorrect backup checksum accepted')
                except ValueError as error:
                    assert str(error) == 'backup checksum mismatch'
                assert not rejected.directory.exists()
            finally:
                manifest_path.write_bytes(original_manifest)
            dump_path = backup_directory / 'ledger.dump'
            missing = backup_directory / 'ledger.saved'
            dump_path.rename(missing)
            try:
                try:
                    restore_files(rejected, backup_directory)
                    raise AssertionError('missing database dump accepted')
                except FileNotFoundError:
                    pass
                assert not rejected.directory.exists()
            finally:
                missing.rename(dump_path)
            report['checks'].append('missing_or_corrupt_backup_refused_before_destination_mutation')

            # Changes after the backup establish a controlled, nonzero recovery
            # point. Chain history remains fetchable; unsaved lifecycle bytes do not.
            source.control('fork')
            source.event(2, trace_id)
            source.index()
            latest = source.snapshot()
            assert dict(latest['costs'])['l1'] == '804216'
            assert len(latest['events']) == 2
            run(source.compose + ['stop', 'postgres', 'rpc'])
            restore_started = time.monotonic()
            report['snapshot_age_at_restore_seconds'] = restore_started - snapshot_started
            restore_files(restored, backup_directory)
            assert {source.volume(key) for key in DIAGNOSTIC_VOLUMES}.isdisjoint({restored.volume(key) for key in DIAGNOSTIC_VOLUMES})
            run(restored.compose + ['up', '-d', '--wait', '--wait-timeout', '90', 'postgres', 'rpc'])
            assert restored.sql("SELECT to_regnamespace('eez') IS NULL;").stdout.strip() == 't'
            with (backup_directory / 'ledger.dump').open('rb') as archive:
                result = subprocess.run(['docker', 'exec', '-i', restored.container('postgres'), 'pg_restore', '-U', 'postgres',
                    '--dbname=postgres', '--create', '--clean', '--if-exists', '--exit-on-error', '--no-owner'],
                    stdin=archive, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if result.returncode:
                raise RuntimeError(result.stderr.decode()[-5000:])
            restored.configure_observer()
            restored.migrate()
            assert restored.snapshot() == baseline
            verify_roles(restored)
            report['checks'].append('fresh_instance_sql_identity_checksums_costs_cursors_roles_deadlines_and_lifecycle_checkpoints')
            restored.control('healthy')
            restored.index()
            assert restored.snapshot() == baseline, 'rescan must not duplicate receipt charges or lifecycle imports'
            restored.control('fork')
            restored.index()
            caught_up = restored.snapshot()
            assert caught_up['costs'] == latest['costs'] and caught_up['cursors'] == latest['cursors']
            assert caught_up['receipt_observations'] == latest['receipt_observations']
            restored.index()
            assert restored.snapshot() == caught_up
            # A later retained record exposes the intentionally lost sequence2;
            # recovery must not fabricate its content from receipt history.
            restored.event(3, trace_id)
            restored.index()
            assert restored.sql('SELECT first_sequence::text||\':\'||last_sequence::text FROM eez.lifecycle_gaps;').stdout.strip() == '2:2'
            report['checks'].append('idempotent_replay_canonical_post_backup_catchup_and_explicit_lifecycle_loss_gap')
            run(restored.compose + ['up', '-d', '--wait', '--wait-timeout', '150'] + DIAGNOSTIC_SERVICES)
            until('restored dashboards and admin access', lambda: sorted(item['uid'] for item in restored.grafana('/api/search?tag=eez')) == dashboards)
            assert restored.grafana('/api/v1/provisioning/contact-points') == contacts
            assert restored.grafana('/api/v1/provisioning/policies') == policies
            assert restored.grafana('/api/datasources/uid/eez-postgres')['user'] == 'eez_observer_reader'
            until('restored trace accessible through Grafana', lambda: restored.grafana('/api/datasources/proxy/uid/eez-tempo/api/traces/' + trace_id))
            until('restored Loki log and trace identity', lambda: logs(restored))
            until('restored historical Prometheus sample', lambda: prom(restored, f'up{{job="prometheus"}} @ {historical_time:.6f}'))
            assert restored.sql("SELECT payload#>>'{trace,trace_id}' FROM eez.lifecycle_event_history WHERE sequence=1;").stdout.strip() == trace_id
            report['checks'].append('grafana_credentials_dashboards_datasources_contact_points_routes_and_retained_trace_log_metric_navigation')
            fixture = restored.base('fixture', 9000)
            until('restored heartbeat routing actually delivers', lambda: request(fixture + '/heartbeat-status')['count'] > 0)
            def notification(status):
                return any(event['path'] == '/alerts' and any(alert.get('labels', {}).get('alertname') == 'EEZObserverUnavailable' and alert.get('status') == status for alert in event['data'].get('alerts', [])) for event in request(fixture + '/events'))
            request(fixture + '/control', data={'mode': 'metrics_error'})
            until('restored operator route delivers a fault', lambda: notification('firing'))
            request(fixture + '/control', data={'mode': 'healthy'})
            until('restored operator route delivers recovery', lambda: notification('resolved'))
            report['checks'].append('restored_heartbeat_and_operator_fault_recovery_delivery')
            report['restore_seconds_until_all_acceptance_checks'] = time.monotonic() - restore_started
            report['controlled_rpo'] = {'fork_scenarios_after_snapshot_recovered': 1, 'affected_chain_cursors': 2,
                                        'post_snapshot_lifecycle_records_not_in_backup': 1,
                                        'later_gap_detected': [2, 2], 'production_schedule_or_target_established': False}
            report['status'] = 'passed'
        except Exception:
            for stack in (source, restored):
                if stack.compose_file.exists():
                    diagnostic = run(stack.compose + ['logs', '--no-color', '--tail', '50'], check=False)
                    args.report.with_name(args.report.stem + '-' + stack.name + '.logs.txt').write_text(diagnostic)
            raise
        finally:
            cleanup_errors = []
            for stack in (restored, source):
                try:
                    stack.cleanup()
                except Exception as error:
                    cleanup_errors.append(type(error).__name__)
            report['owned_resource_cleanup'] = 'failed' if cleanup_errors else 'verified'
            if cleanup_errors:
                report['status'] = 'cleanup_failed'
            args.report.write_text(json.dumps(report, indent=2) + '\n')
            if cleanup_errors:
                raise RuntimeError('owned-resource cleanup failed: ' + ','.join(cleanup_errors))


if __name__ == '__main__':
    main()
