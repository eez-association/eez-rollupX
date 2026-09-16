#!/usr/bin/env python3
"""Exercise the real migration CLI and SQL invariants in a disposable database."""
import argparse
import hashlib
from datetime import datetime, timezone
import concurrent.futures
import copy
import json
import importlib.util
import tempfile
import pathlib
import subprocess
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
IMAGES = json.loads((ROOT / 'images.lock.json').read_text())
IMAGE = next(key + '@' + value for key, value in IMAGES.items() if key.startswith('postgres:'))


def run(command, *, data=None, check=True):
    result = subprocess.run(command, input=data, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
    if check and result.returncode:
        raise RuntimeError('Isolated database command failed:\n' + result.stdout[-6000:])
    return result


def check_profile(binary, report):
    spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
    prepare = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(prepare)
    name = 'eez-obs-db-profile-' + uuid.uuid4().hex[:10]
    with tempfile.TemporaryDirectory(prefix=name) as temp:
        path = pathlib.Path(temp)
        (path / 'logs').mkdir()
        runtime = prepare.prepare({'environment':'fixture','deployment':'database-profile','observer_instance':'fixture',
            'observer_target':'fixture:19100','log_directory':str(path / 'logs'),
            'alert_webhook_url':'http://unused:9000/alerts','heartbeat_webhook_url':'http://unused:9000/heartbeat',
            'database_enabled':True}, path / 'runtime')
        compose = ['docker','compose','--project-name',name,'--env-file',str(runtime / 'compose.env'),'-f',str(ROOT / 'compose.yaml'),'--profile','database']
        try:
            resolved = json.loads(run(compose + ['config','--format','json']).stdout)
            assert resolved['services']['postgres']['ports'][0]['host_ip'] == '127.0.0.1'
            # Compose run does not publish service ports. No other stack service starts.
            run(compose + ['run','--detach','--no-deps','--name',name,'postgres'])
            deadline = time.monotonic() + 60
            while run(['docker','exec',name,'pg_isready','-h','127.0.0.1','-U','postgres','-d','eez_observer'],check=False).returncode:
                if time.monotonic() >= deadline:
                    raise RuntimeError('database profile failed to initialize')
                time.sleep(0.2)
            sql = ['docker','exec',name,'psql','-XAtq','-U','postgres','-d','eez_observer','-c']
            assert run(sql + ["SELECT to_regnamespace('eez') IS NULL;"]).stdout.strip() == 't'
            assert run(sql + ["SELECT count(*) FROM pg_roles WHERE rolname IN ('eez_observer_writer','eez_observer_reader') AND rolcanlogin AND NOT(rolsuper OR rolcreatedb OR rolcreaterole OR rolbypassrls);"]).stdout.strip() == '2'
            run(['docker','cp',str(binary.resolve()),name + ':/observer'])
            admin = json.loads((runtime / 'database/admin.json').read_text())
            admin['connection_string'] = admin['connection_string'].replace('port=19432','port=5432')
            run(['docker','exec','-i',name,'sh','-c','umask 077; cat > /tmp/admin.json'],data=json.dumps(admin))
            run(['docker','exec',name,'/observer','migrate','--database-config','/tmp/admin.json'])
            # Authenticate the actual provisioned reader over TCP; password is
            # read from its mounted secret, never sent as a command-line argument.
            run(['docker','exec',name,'sh','-c',"export PGPASSWORD=\"$(cat /run/secrets/postgres_reader_password)\"; exec psql -XAtq -h 127.0.0.1 -U eez_observer_reader -d eez_observer -c 'SELECT count(*) FROM eez.canonical_receipts'"])
            run(['docker','restart',name])
            deadline = time.monotonic() + 30
            while run(['docker','exec',name,'pg_isready','-h','127.0.0.1','-U','postgres','-d','eez_observer'],check=False).returncode:
                if time.monotonic() >= deadline:
                    raise RuntimeError('database profile did not recover after restart')
                time.sleep(0.2)
            assert run(sql + ['SELECT count(*) FROM eez.schema_migrations;']).stdout.strip() == '2'
            report['checks'].append('opt_in_compose_profile_private_port_secret_roles_explicit_migration_and_persistence')
        finally:
            run(['docker','rm','--force',name],check=False)
            run(compose + ['down','--volumes','--remove-orphans'],check=False)
            assert not run(['docker','ps','-aq','--filter','label=com.docker.compose.project=' + name]).stdout.strip()
            assert not run(['docker','volume','ls','-q','--filter','label=com.docker.compose.project=' + name]).stdout.strip()
            assert not run(['docker','network','ls','-q','--filter','label=com.docker.compose.project=' + name]).stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, default=ROOT.parent / 'target/debug/eez-observer')
    parser.add_argument('--previous-binary', type=pathlib.Path, help='Optional saved schema-v1 observer for upgrade compatibility checks')
    parser.add_argument('--report', type=pathlib.Path, default=ROOT / 'validation/database.json')
    args = parser.parse_args()
    name = 'eez-obs-db-test-' + uuid.uuid4().hex[:10]
    network, rpc_name = name + '-net', name + '-rpc'
    with args.binary.open('rb') as stream:
        binary_digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    report = {'scope': 'isolated database only; no live services', 'image': IMAGE, 'checks': [],
              'binary_sha256': binary_digest, 'validated_at': datetime.now(timezone.utc).isoformat(),
              'migration_sha256': {path.name:hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((ROOT / 'migrations').glob('*.sql'))}}
    try:
        check_profile(args.binary, report)
        run(['docker', 'network', 'create', '--internal', network])
        fixture_images = json.loads((ROOT / 'tests/images.lock.json').read_text())
        fixture_image = next(key + '@' + value for key, value in fixture_images.items() if key.startswith('python:'))
        run(['docker', 'run', '--detach', '--rm', '--name', rpc_name, '--network', network, '--network-alias', 'rpc',
             '--user', '65534', '--memory', '128m', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
             '--mount', f'type=bind,src={ROOT / "tests/indexer-rpc.py"},dst=/rpc.py,readonly',
             '--mount', f'type=bind,src={ROOT / "tests/fixtures/empty-post-calldata.hex"},dst=/post-calldata.hex,readonly', fixture_image, 'python3', '/rpc.py'])
        run(['docker', 'run', '--detach', '--rm', '--name', name, '--network', network, '--user', 'postgres',
             '--memory', '512m', '--cpus', '1', '--security-opt', 'no-new-privileges:true', '--cap-drop', 'ALL',
             '--env', 'POSTGRES_PASSWORD=fixture-only', '--env', 'POSTGRES_DB=eez_observer',
             '--mount', f'type=bind,src={args.binary.resolve()},dst=/observer,readonly', IMAGE])
        deadline = time.monotonic() + 60
        # The entrypoint's temporary bootstrap server is Unix-socket-only and can
        # report ready before the database exists. Wait for the final TCP server.
        while run(['docker', 'exec', name, 'pg_isready', '-h', '127.0.0.1', '-U', 'postgres', '-d', 'eez_observer'], check=False).returncode:
            if time.monotonic() >= deadline:
                raise RuntimeError('Isolated PostgreSQL did not become ready')
            time.sleep(0.5)
        config = {'connection_string': 'host=127.0.0.1 user=postgres dbname=eez_observer password=fixture-only sslmode=disable',
                  'allow_private_plaintext': True}
        # stdin avoids putting connection credentials in an exec command string.
        run(['docker', 'exec', '-i', name, 'sh', '-c', 'umask 077; cat > /tmp/database.json'], data=json.dumps(config))
        migrate = ['docker', 'exec', name, '/observer', 'migrate', '--database-config', '/tmp/database.json']
        psql = ['docker', 'exec', '-i', name, 'psql', '-X', '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-d', 'eez_observer']
        # Construct the released v1 schema and its exact history; a fresh install
        # of the current schema was already tested through the Compose profile.
        v1 = (ROOT / 'migrations/0001-canonical.sql').read_text()
        digest = hashlib.sha256(v1.encode()).hexdigest()
        if args.previous_binary:
            run(['docker','cp',str(args.previous_binary.resolve()),name + ':/observer-v1'])
            run(['docker','exec',name,'/observer-v1','migrate','--database-config','/tmp/database.json'])
        else:
            run(psql, data=v1 + "\nINSERT INTO eez.schema_migrations(version,checksum) VALUES(1,decode('" + digest + "','hex'));")
        result = run(psql, data=(ROOT / 'tests/accounting-ledger.sql').read_text())
        assert 'canonical ledger invariants passed' in result.stdout
        report['checks'].append('canonical_revert_reorg_reinclusion_missing_fees_full_width_sums_and_readonly_roles')
        before_upgrade = run(psql + ['-Atq'], data='SELECT known_total_paid FROM eez.canonical_cost_totals WHERE chain_instance_id=1;').stdout.strip()
        selector = (ROOT / 'tests/fixtures/empty-post-calldata.hex').read_text().strip()[2:10]
        run(psql, data=(ROOT / 'tests/legacy-posting.sql').read_text().replace('POST_SELECTOR', selector))
        run(migrate)
        run(migrate)
        assert run(psql + ['-Atq'], data='SELECT known_total_paid FROM eez.canonical_cost_totals WHERE chain_instance_id=1;').stdout.strip() == before_upgrade
        assert run(psql + ['-Atq'], data="SELECT count(*) FROM eez.posting_analysis WHERE state='pending';").stdout.strip() == '5'
        if args.previous_binary:
            assert run(['docker','exec',name,'/observer-v1','migrate','--database-config','/tmp/database.json'],check=False).returncode != 0
            report['checks'].append('saved_v1_binary_migrates_original_schema_and_refuses_v2_downgrade')
        report['checks'].append('explicit_forward_upgrade_preserves_costs_and_marks_old_method_evidence_unknown')
        report['checks'].append('explicit_migration_and_idempotent_reapplication')
        run(psql, data="ALTER ROLE eez_observer_writer LOGIN PASSWORD 'fixture-writer';")
        writer = {**config, 'connection_string': config['connection_string'].replace('user=postgres', 'user=eez_observer_writer').replace('password=fixture-only', 'password=fixture-writer')}
        run(['docker', 'exec', '-i', name, 'sh', '-c', 'umask 077; cat > /tmp/writer.json'], data=json.dumps(writer))
        endpoint = lambda chain: {'url': f'http://rpc:9000/l{chain}', 'chain_id': chain, 'genesis_hash': '0x' + f'{chain * 100:064x}'}
        index_config = {'exporter': {'l1': endpoint(1), 'l2': endpoint(2)}, 'environment': 'fixture', 'deployment': 'scanner',
            'rollup_id': 1, 'database_config': '/tmp/writer.json', 'l1': {'bootstrap': 1, 'wallets': ['0x' + 'aa' * 20]},
            'l2': {'bootstrap': 1, 'include_all_transactions': True}, 'batch_blocks': 16}
        def write_index(value):
            run(['docker', 'exec', '-i', name, 'sh', '-c', 'umask 077; cat > /tmp/index.json'], data=json.dumps(value))
        def control(**values):
            run(['docker', 'exec', '-i', rpc_name, 'python3', '-c',
                "import urllib.request,sys; urllib.request.urlopen(urllib.request.Request('http://localhost:9000/control', data=sys.stdin.buffer.read(),headers={'Content-Type':'application/json'})).read()"], data=json.dumps(values))
        def scalar(sql):
            return run(psql + ['-Atq'], data=sql).stdout.strip()
        def total():
            return scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner' AND role='l1';")
        index = ['docker', 'exec', name, '/observer', 'indexer', '--config', '/tmp/index.json', '--once']
        write_index(index_config)
        if args.previous_binary:
            assert run(['docker','exec',name,'/observer-v1','indexer','--config','/tmp/index.json','--once'],check=False).returncode != 0
            report['checks'].append('saved_v1_indexer_refuses_v2_schema_without_touching_receipts')
        run(psql, data='CREATE DATABASE empty_observer;')
        empty_config = {**writer, 'connection_string': writer['connection_string'].replace('dbname=eez_observer', 'dbname=empty_observer')}
        run(['docker', 'exec', '-i', name, 'sh', '-c', 'umask 077; cat > /tmp/empty.json'], data=json.dumps(empty_config))
        unmigrated = {**index_config, 'database_config': '/tmp/empty.json'}
        write_index(unmigrated)
        assert run(index, check=False).returncode != 0
        assert run(index[:-1], check=False).returncode != 0
        assert run(['docker', 'exec', name, 'psql', '-XAtq', '-U', 'postgres', '-d', 'empty_observer', '-c', "SELECT to_regnamespace('eez') IS NULL;"]).stdout.strip() == 't'
        write_index(index_config)
        report['checks'].append('unmigrated_schema_refused_without_implicit_ddl_in_both_runtime_modes')
        control(phase='healthy')
        upgrade = copy.deepcopy(index_config)
        upgrade['deployment'] = 'upgrade'
        upgrade['l1']['bootstrap'] = 2
        upgrade['l1']['contracts'] = ['0x' + 'bb' * 20]
        write_index(upgrade)
        assert scalar("SELECT unknown_method_receipts FROM eez.posting_coverage c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='upgrade';") == '1'
        control(phase='transaction_unavailable')
        run(index)
        assert scalar("SELECT unknown_method_receipts||':'||method_collection_errors FROM eez.posting_coverage c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='upgrade';") == '1:1'
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='upgrade' AND role='l1';") == '591216'
        control(phase='healthy')
        run(index)
        assert scalar("SELECT sum(execution_paid+blob_paid) FROM eez.posting_receipts c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='upgrade';") == '591216'
        assert scalar("SELECT unknown_method_receipts FROM eez.posting_coverage c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='upgrade';") == '0'
        report['checks'].append('legacy_posting_evidence_backfills_from_real_rpc_without_recharging_receipt')
        write_index(index_config)
        control(phase='missing')
        run(index)
        assert total() == '147000', total()
        assert scalar("SELECT count(*) FROM eez.collection_gaps g JOIN eez.chain_instances c ON c.id=g.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner' AND state='open';") == '2'
        control(phase='healthy')
        run(index)
        assert total() == '738216', total()
        run(index)
        assert total() == '738216'
        control(phase='fork')
        run(index)
        assert total() == '804216', total()
        assert scalar("SELECT count(*) FROM eez.receipt_observations r JOIN eez.chain_instances c ON c.id=r.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner' AND c.role='l1';") == '3'
        assert scalar("SELECT count(*) FROM eez.blocks b JOIN eez.chain_instances c ON c.id=b.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner' AND c.role='l2' AND b.finalized;") == '0'
        control(phase='healthy')
        run(index)
        assert total() == '738216'
        control(phase='fork')
        run(index)
        assert total() == '804216'
        control(phase='wrong_identity')
        assert run(index, check=False).returncode != 0
        assert total() == '804216'
        assert scalar("SELECT source_valid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner' AND role='l1';") == 'f'
        control(phase='fork')
        changed = copy.deepcopy(index_config)
        changed['l1']['bootstrap'] = 2
        write_index(changed)
        assert run(index, check=False).returncode != 0
        assert total() == '804216'
        changed = copy.deepcopy(index_config)
        changed['l1']['wallets'] = ['0x' + 'cc' * 20]
        write_index(changed)
        assert run(index, check=False).returncode != 0
        assert total() == '804216'
        changed['database_config'] = '/tmp/database.json'
        write_index(changed)
        assert run(index, check=False).returncode != 0
        write_index(index_config)
        report['checks'].append('changed_bootstrap_selection_and_administrative_indexer_credentials_refused')
        control(phase='fork', finalize=True)
        run(index)
        control(phase='contradiction')
        assert run(index, check=False).returncode != 0
        assert scalar("SELECT frozen_reason FROM eez.chain_instances c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner' AND role='l1';") == 'finalized_contradiction'
        assert total() == '804216'
        report['checks'].append('real_indexer_restart_receipt_retry_same_height_reorg_reinclusion_identity_and_finality_freeze')
        # A receipt from a fork that changed during collection cannot advance
        # that block's cursor. Retrying imports only the now-consistent chain.
        variant = copy.deepcopy(index_config)
        variant['deployment'] = 'mid-fetch'
        write_index(variant)
        control(phase='mid_fetch', finalize=False)
        assert run(index, check=False).returncode != 0
        def cursor(deployment):
            return scalar("SELECT indexed_through FROM eez.index_coverage c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='" + deployment + "' AND role='l1';")
        assert cursor('mid-fetch') == '1', cursor('mid-fetch')
        run(index)
        assert cursor('mid-fetch') == '3'
        report['checks'].append('mid_fetch_fork_never_commits_mixed_inclusion')
        variant['deployment'] = 'pruned'
        write_index(variant)
        control(phase='pruned')
        assert run(index, check=False).returncode != 0
        assert cursor('pruned') == '1'
        assert scalar("SELECT count(*) FROM eez.collection_gaps g JOIN eez.chain_instances c ON c.id=g.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='pruned' AND source='chain' AND state='open';") == '2'
        control(phase='healthy')
        run(index)
        assert cursor('pruned') == '3'
        assert scalar("SELECT count(*) FROM eez.collection_gaps g JOIN eez.chain_instances c ON c.id=g.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='pruned' AND state='open';") == '0'
        report['checks'].append('unavailable_block_range_persisted_and_resolved_on_recovery')
        variant['deployment'] = 'wrong-type'
        write_index(variant)
        control(phase='wrong_type')
        assert run(index, check=False).returncode != 0
        assert cursor('wrong-type') == '1'
        control(phase='healthy')
        run(index)
        assert cursor('wrong-type') == '3'
        report['checks'].append('contradictory_receipt_transaction_type_refused_and_retry_recovers')
        variant['deployment'] = 'posting'
        variant['l1']['contracts'] = ['0x' + 'bb' * 20]
        write_index(variant)
        run(index)
        assert scalar("SELECT known_total_paid FROM eez.wallet_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='posting' AND role='l1';") == '738216'
        assert scalar("SELECT sum(execution_paid+blob_paid) FROM eez.posting_receipts c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='posting' AND NOT success;") == '591216'
        assert scalar("SELECT decoded_post_receipts||':'||other_method_receipts||':'||unknown_method_receipts FROM eez.posting_coverage c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='posting';") == '1:1:0'
        assert scalar("SELECT count(*) FROM eez.posting_receipts c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner';") == '0'
        assert scalar("SELECT count(*) FROM eez.posting_receipts WHERE role='l2';") == '0'
        variant['deployment'] = 'invalid-posting'
        write_index(variant)
        control(phase='invalid_post')
        run(index)
        assert scalar("SELECT known_total_paid FROM eez.wallet_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='invalid-posting' AND role='l1';") == '738216'
        assert scalar("SELECT invalid_post_calldata_receipts FROM eez.posting_coverage c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='invalid-posting';") == '1'
        assert scalar("SELECT count(*) FROM eez.posting_receipts c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='invalid-posting';") == '0'
        control(phase='healthy')
        variant['deployment'] = 'unverified-posting'
        write_index(variant)
        control(phase='unverified_post')
        run(index)
        assert scalar("SELECT known_total_paid FROM eez.wallet_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='unverified-posting' AND role='l1';") == '738216'
        assert scalar("SELECT unknown_method_receipts FROM eez.posting_coverage c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='unverified-posting';") == '1'
        assert scalar("SELECT count(*) FROM eez.posting_receipts c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='unverified-posting';") == '0'
        control(phase='healthy')
        report['checks'].append('wallet_spend_decoded_contract_posts_reverts_and_invalid_calldata_remain_distinct')
        variant['deployment'] = 'bounded-batch'
        variant['batch_blocks'] = 1
        write_index(variant)
        for expected in ('1','2','3'):
            run(index)
            assert cursor('bounded-batch') == expected
        variant['batch_blocks'] = 16
        report['checks'].append('nondefault_batch_bound_reaches_runtime_and_advances_one_block_per_cycle')
        variant['deployment'] = 'concurrent'
        write_index(variant)
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(lambda _: run(index, check=False), range(2)))
        # Each process can win one chain and lose the other; an expected cursor
        # mismatch is retryable, not a requirement that one entire process wins.
        assert all(result.returncode == 0 or 'inconsistent_chain_facts' in result.stdout for result in outcomes), [result.stdout for result in outcomes]
        run(index)
        assert cursor('concurrent') == '3'
        assert scalar("SELECT count(*) FROM eez.receipt_observations r JOIN eez.chain_instances c ON c.id=r.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='concurrent' AND role='l1';") == '2'
        report['checks'].append('concurrent_scanners_preserve_single_inclusions_and_monotonic_cursor')
        variant['deployment'] = 'crash'
        write_index(variant)
        run(psql, data="""CREATE FUNCTION eez.fixture_pause_receipt() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN
          IF NEW.transaction_hash=decode(lpad(to_hex(1002),64,'0'),'hex') AND EXISTS(
            SELECT FROM eez.chain_instances c JOIN eez.deployments d ON d.id=c.deployment_id
            WHERE c.id=NEW.chain_instance_id AND d.name='crash') THEN PERFORM pg_sleep(8); END IF;
          RETURN NEW; END $$;
          CREATE TRIGGER fixture_pause BEFORE INSERT ON eez.receipt_observations FOR EACH ROW EXECUTE FUNCTION eez.fixture_pause_receipt();""")
        run(['docker', 'exec', '-d', name, 'sh', '-c', 'echo $$ > /tmp/observer.pid; exec /observer indexer --config /tmp/index.json --once > /tmp/observer.log 2>&1'])
        deadline = time.monotonic() + 20
        while scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='eez-observer' AND wait_event='PgSleep';") != '1':
            if time.monotonic() >= deadline:
                raise RuntimeError('scanner did not reach controlled pre-commit pause')
            time.sleep(0.1)
        run(['docker', 'exec', name, 'sh', '-c', 'kill -KILL "$(cat /tmp/observer.pid)"'])
        deadline = time.monotonic() + 12
        while scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='eez-observer';") != '0':
            if time.monotonic() >= deadline:
                raise RuntimeError('killed scanner left a database connection open')
            time.sleep(0.2)
        assert cursor('crash') == '1'
        assert scalar("SELECT count(*) FROM eez.blocks b JOIN eez.chain_instances c ON c.id=b.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='crash' AND role='l1' AND number=2;") == '0'
        run(psql, data='DROP TRIGGER fixture_pause ON eez.receipt_observations; DROP FUNCTION eez.fixture_pause_receipt();')
        run(index)
        assert cursor('crash') == '3'
        report['checks'].append('killed_scanner_rolls_back_block_receipt_and_cursor_then_recovers')
        variant['exporter']['listen'] = '0.0.0.0:19100'
        variant['exporter']['allow_non_loopback'] = True
        write_index(variant)
        run(['docker', 'exec', '-d', name, 'sh', '-c', 'echo $$ > /tmp/observer.pid; exec /observer indexer --config /tmp/index.json > /tmp/observer.log 2>&1'])
        fetch = ['docker', 'exec', rpc_name, 'python3', '-c', "import urllib.request; print(urllib.request.urlopen('http://" + name + ":19100/metrics',timeout=2).read().decode())"]
        deadline = time.monotonic() + 30
        while True:
            metrics = run(fetch, check=False).stdout
            if 'eez_observer_index_collection_success{chain="l1"} 1' in metrics and 'eez_observer_index_collection_success{chain="l2"} 1' in metrics:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('continuous scanner metrics did not become valid')
            time.sleep(0.2)
        # Hold an incomplete HTTP request across shutdown to exercise the drain bound.
        run(['docker', 'exec', '-d', rpc_name, 'python3', '-c',
             "import socket,time,pathlib; s=socket.create_connection(('" + name + "',19100)); s.sendall(b'GET /metrics HTTP/1.1\\r\\nHost: observer\\r\\n'); pathlib.Path('/tmp/slow-client').touch(); time.sleep(30)"])
        deadline = time.monotonic() + 5
        while run(['docker', 'exec', rpc_name, 'test', '-f', '/tmp/slow-client'], check=False).returncode:
            if time.monotonic() >= deadline:
                raise RuntimeError('slow scrape client did not connect')
            time.sleep(0.1)
        run(['docker', 'exec', name, 'sh', '-c', 'kill -TERM "$(cat /tmp/observer.pid)"'])
        deadline = time.monotonic() + 5
        while run(['docker', 'exec', name, 'sh', '-c', 'kill -0 "$(cat /tmp/observer.pid)"'], check=False).returncode == 0:
            if time.monotonic() >= deadline:
                raise RuntimeError('continuous observer did not shut down')
            time.sleep(0.2)
        report['checks'].append('continuous_indexer_serves_both_chain_metrics_and_exits_on_sigterm')
        run(psql, data="UPDATE eez.schema_migrations SET version=version+100;")
        assert run(migrate, check=False).returncode != 0
        assert run(index, check=False).returncode != 0
        run(psql, data="UPDATE eez.schema_migrations SET version=version-100;")
        report['checks'].append('newer_schema_refuses_migration_downgrade_and_old_indexer')
        run(psql, data="UPDATE eez.schema_migrations SET checksum=decode(repeat('00',32),'hex');")
        assert run(migrate, check=False).returncode != 0
        report['checks'].append('incompatible_migration_checksum_refused')
        report['status'] = 'passed'
        print('PASS: explicit migrations, canonical accounting invariants, exact sums, and reader permissions')
    except Exception:
        report['status'] = 'failed'
        print(run(['docker', 'logs', '--tail', '20', name], check=False).stdout)
        raise
    finally:
        run(['docker', 'rm', '--force', '--volumes', name], check=False)
        run(['docker', 'rm', '--force', '--volumes', rpc_name], check=False)
        run(['docker', 'network', 'rm', network], check=False)
        remaining = run(['docker', 'ps', '-aq', '--filter', 'name=^/' + name + '$']).stdout.strip()
        remaining_rpc = run(['docker', 'ps', '-aq', '--filter', 'name=^/' + rpc_name + '$']).stdout.strip()
        remaining_network = run(['docker', 'network', 'ls', '-q', '--filter', 'name=^' + network + '$']).stdout.strip()
        remaining = remaining or remaining_rpc or remaining_network
        if remaining:
            report['status'] = 'cleanup_failed'
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')
        if remaining:
            raise RuntimeError('Disposable database container remains')


if __name__ == '__main__':
    main()
