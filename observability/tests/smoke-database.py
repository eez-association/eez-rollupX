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
import sys

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
            assert run(sql + ['SELECT count(*) FROM eez.schema_migrations;']).stdout.strip() == '6'
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
    parser.add_argument('--previous-v4-binary', type=pathlib.Path, help='Optional saved schema-v4 observer for populated lifecycle upgrade checks')
    parser.add_argument('--previous-v5-binary', type=pathlib.Path, help='Optional saved schema-v5 observer for receipt/log upgrade checks')
    parser.add_argument('--report', type=pathlib.Path, default=ROOT / 'validation/database.json')
    args = parser.parse_args()
    name = 'eez-obs-db-test-' + uuid.uuid4().hex[:10]
    network, rpc_name = name + '-net', name + '-rpc'
    with args.binary.open('rb') as stream:
        binary_digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    report = {'scope': 'isolated database only; no live services', 'image': IMAGE, 'checks': [],
              'binary_sha256': binary_digest, 'validated_at': datetime.now(timezone.utc).isoformat(),
              'migration_sha256': {path.name:hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((ROOT / 'migrations').glob('*.sql'))}}
    spool_temp = tempfile.TemporaryDirectory(prefix=name + '-spool-')
    spool = pathlib.Path(spool_temp.name)
    spool.chmod(0o755)  # Public synthetic records only; container reads this mount read-only.
    writer_process = None
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
             '--mount', f'type=bind,src={args.binary.resolve()},dst=/observer,readonly',
             '--mount', f'type=bind,src={spool},dst=/lifecycle,readonly', IMAGE])
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
        if args.previous_v4_binary:
            run(['docker','cp',str(args.previous_v4_binary.resolve()),name + ':/observer-v4'])
            run(['docker','exec',name,'/observer-v4','migrate','--database-config','/tmp/database.json'])
            assert run(psql + ['-Atq'],data='SELECT count(*) FROM eez.schema_migrations;').stdout.strip() == '4'
            run(psql,data="""INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory) VALUES(1,'composer','upgrade','/upgrade');
              INSERT INTO eez.lifecycle_epochs(source_id,epoch) SELECT id,decode(repeat('12',16),'hex') FROM eez.lifecycle_sources WHERE instance='upgrade';
              INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state,payload)
              SELECT id,decode(repeat('12',16),'hex'),1,1,'complete','{"observed_at_unix_ms":1000,"trace":null,"event":{"event":"reorg","ancestor":{"number":1,"hash":"0101010101010101010101010101010101010101010101010101010101010101"}}}'::jsonb FROM eez.lifecycle_sources WHERE instance='upgrade';""")
        if args.previous_v5_binary:
            run(['docker','cp',str(args.previous_v5_binary.resolve()),name + ':/observer-v5'])
            run(['docker','exec',name,'/observer-v5','migrate','--database-config','/tmp/database.json'])
            assert run(psql + ['-Atq'],data='SELECT count(*) FROM eez.schema_migrations;').stdout.strip() == '5'
        run(migrate)
        run(migrate)
        if args.previous_v5_binary:
            assert run(['docker','exec',name,'/observer-v5','migrate','--database-config','/tmp/database.json'],check=False).returncode != 0
            report['checks'].append('saved_v5_binary_upgrade_preserves_receipts_and_refuses_downgrade')
        if args.previous_v4_binary:
            assert run(psql + ['-Atq'],data="SELECT count(*) FROM eez.lifecycle_event_history WHERE instance='upgrade';").stdout.strip() == '1'
            assert run(['docker','exec',name,'/observer-v4','migrate','--database-config','/tmp/database.json'],check=False).returncode != 0
            report['checks'].append('saved_v4_binary_upgrade_preserves_lifecycle_events_and_refuses_downgrade')
        assert run(psql + ['-Atq'], data='SELECT known_total_paid FROM eez.canonical_cost_totals WHERE chain_instance_id=1;').stdout.strip() == before_upgrade
        assert run(psql + ['-Atq'], data="SELECT count(*) FROM eez.posting_analysis WHERE state='pending';").stdout.strip() == '5'
        if args.previous_binary:
            assert run(['docker','exec',name,'/observer-v1','migrate','--database-config','/tmp/database.json'],check=False).returncode != 0
            report['checks'].append('saved_v1_binary_migrates_original_schema_and_refuses_current_downgrade')
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
            report['checks'].append('saved_v1_indexer_refuses_current_schema_without_touching_receipts')
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
        # Applied ranges require exact hashes and a successful post; the node's
        # height-based l2Finalized flag is never used as finality evidence.
        variant['deployment'] = 'settlement'
        write_index(variant)
        control(phase='healthy',correlation='valid',post_success=True,finalize=False,post_count=2)
        run(index)
        def settled(sql, deployment='settlement'):
            return scalar(sql + " AND c.deployment_id=(SELECT id FROM eez.deployments WHERE name='" + deployment + "');")
        assert settled("SELECT count(*) FROM eez.settlements s JOIN eez.chain_instances c ON c.id=s.l2_chain_id WHERE true") == '2'
        assert settled("SELECT count(*) FROM eez.verified_l2_finality f JOIN eez.chain_instances c ON c.id=f.l2_chain_id WHERE true") == '0'
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='settlement' AND role='l1';") == '1329432'
        run(index)
        assert settled("SELECT count(*) FROM eez.settlements s JOIN eez.chain_instances c ON c.id=s.l2_chain_id WHERE true") == '2'
        control(phase='fork')
        run(index)
        assert settled("SELECT count(*) FROM eez.settlements s JOIN eez.chain_instances c ON c.id=s.l2_chain_id WHERE true") == '4'
        assert settled("SELECT count(DISTINCT b.hash) FROM eez.canonical_settled_l2_blocks b JOIN eez.chain_instances c ON c.id=b.l2_chain_id WHERE b.source_valid") == '2'
        control(finalize=True,correlation='reported_unfinalized')
        run(index)
        assert settled("SELECT count(*) FROM eez.verified_l2_finality f JOIN eez.chain_instances c ON c.id=f.l2_chain_id WHERE true") == '2'
        # Only L2 changes next: derived finality freezes, while both independent
        # receipt scanners may continue reconciling paid facts.
        # Test the contradiction in a separate namespace with unchanged L1 history.
        variant['deployment'] = 'settlement-finality'
        write_index(variant)
        control(phase='healthy',correlation='valid',post_count=1,finalize=True)
        run(index)
        assert settled("SELECT count(*) FROM eez.verified_l2_finality f JOIN eez.chain_instances c ON c.id=f.l2_chain_id WHERE true",'settlement-finality') == '2'
        control(phase='l2_fork')
        run(index)
        assert settled("SELECT s.frozen_reason FROM eez.correlation_sources s JOIN eez.chain_instances c ON c.id=s.l2_chain_id WHERE true",'settlement-finality') == 'l2_finalized_contradiction'
        assert settled("SELECT count(*) FROM eez.verified_l2_finality f JOIN eez.chain_instances c ON c.id=f.l2_chain_id WHERE true",'settlement-finality') == '0'
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='settlement-finality' AND role='l2';") == '804216'
        assert scalar("SELECT source_valid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='settlement-finality' AND role='l2';") == 't'
        assert settled("SELECT c.frozen_reason IS NULL FROM eez.correlation_sources s JOIN eez.chain_instances c ON c.id=s.l2_chain_id WHERE true",'settlement-finality') == 't'
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='settlement-finality' AND role='l1';") == '738216'
        report['checks'].append('same_block_posts_reorg_history_exact_l2_finality_and_derived_only_freeze')
        for mode in ('missing','wrong_anchor','duplicate','wrong_hash'):
            variant['deployment'] = 'settlement-' + mode.replace('_','-')
            write_index(variant)
            control(phase='healthy',correlation=mode,finalize=False)
            run(index)
            assert settled("SELECT count(*) FROM eez.verified_l2_finality f JOIN eez.chain_instances c ON c.id=f.l2_chain_id WHERE true",variant['deployment']) == '0'
            assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='" + variant['deployment'] + "' AND role='l1';") == '738216'
            control(correlation='valid',finalize=True)
            run(index)
            expected = '0' if mode == 'wrong_hash' else '2'
            assert settled("SELECT count(*) FROM eez.verified_l2_finality f JOIN eez.chain_instances c ON c.id=f.l2_chain_id WHERE true",variant['deployment']) == expected
            if mode == 'wrong_hash':
                assert settled("SELECT count(*) FROM eez.settlements s JOIN eez.chain_instances c ON c.id=s.l2_chain_id WHERE true",variant['deployment']) == '2'
        report['checks'].append('empty_duplicate_mismatched_or_conflicting_ranges_never_fabricate_finality_or_change_receipt_costs')
        # More pending ranges than a cycle's validation/finality budget must
        # advance fairly on restart, without charging duplicate range membership.
        variant['deployment'] = 'settlement-budget'
        write_index(variant)
        control(phase='healthy',correlation='valid',finalize=True,post_count=17)
        run(index)
        scope = " JOIN eez.chain_instances c ON c.id=s.l2_chain_id WHERE true"
        assert settled("SELECT count(*) FROM eez.settlements s" + scope,'settlement-budget') == '17'
        assert settled("SELECT count(*) FROM eez.settlements s" + scope + " AND s.range_verified_at IS NOT NULL",'settlement-budget') == '16'
        assert settled("SELECT count(*) FROM eez.settlements s" + scope + " AND s.finality_done",'settlement-budget') == '16'
        run(index)
        assert settled("SELECT count(*) FROM eez.settlements s" + scope + " AND s.finality_done",'settlement-budget') == '17'
        assert settled("SELECT count(*) FROM eez.verified_l2_finality f JOIN eez.chain_instances c ON c.id=f.l2_chain_id WHERE true",'settlement-budget') == '2'
        report['checks'].append('bounded_range_validation_and_finality_backlog_progress_without_duplicate_blocks')
        # Lifecycle files are local public fixtures, mounted read-only into the
        # observer container. A separate owned process holds the real writer lock.
        variant['deployment'] = 'lifecycle'
        variant['lifecycle'] = [{'service':'composer','instance':'node-1','directory':'/lifecycle'}]
        write_index(variant)
        control(phase='healthy',correlation='valid',finalize=False,post_count=1)
        identity = {'environment':'fixture','deployment':'lifecycle','service':'composer','instance':'node-1'}
        epoch, segment = '01' * 16, '02' * 16
        path = spool / (epoch + '-' + segment + '.active')
        lock_path = spool / (epoch + '.lock')
        lock_path.touch(mode=0o644)
        writer_process = subprocess.Popen([sys.executable,'-c',
            "import fcntl,sys,time; f=open(sys.argv[1],'r'); fcntl.flock(f,fcntl.LOCK_EX); print('ready',flush=True); time.sleep(3600)",str(lock_path)],stdout=subprocess.PIPE,text=True)
        assert writer_process.stdout.readline().strip() == 'ready'
        def line(value):
            return json.dumps(value,separators=(',',':')).encode() + b'\n'
        def header(epoch_value=epoch, segment_value=segment):
            return line({'record':'header','version':1,'identity':identity,'epoch':epoch_value,'segment_id':segment_value,
                'process_id':123,'started_at_unix_ms':10000,'segment_created_at_unix_ms':10001})
        def chunks(sequence, pieces=1, payload=None, epoch_value=epoch):
            if payload is None:
                payload = {'observed_at_unix_ms':10002,'trace':None,'event':{'event':'reorg','ancestor':{'number':1,'hash':'03' * 32}}}
            encoded = json.dumps(payload,separators=(',',':')).encode()
            step = (len(encoded) + pieces - 1) // pieces
            parts = [encoded[offset:offset + step] for offset in range(0,len(encoded),step)]
            return [line({'record':'data','version':1,'identity':identity,'epoch':epoch_value,'sequence':sequence,'chunk_index':i,
                'chunk_count':len(parts),'payload_hex':part.hex(),'complete':i + 1 == len(parts),
                'digest':hashlib.sha256(encoded).hexdigest() if i + 1 == len(parts) else None}) for i,part in enumerate(parts)]
        def control_line(sequence, highest, epoch_value=epoch):
            return line({'record':'control','version':1,'identity':identity,'epoch':epoch_value,'control_sequence':sequence,
                'observed_at_unix_ms':10003,'status':'active','stats':{'attempted_records':highest,'dropped_records':0,
                'written_records':highest,'last_assigned_sequence':highest,'dropped_sequence_min':None,'dropped_sequence_max':None,'io_errors':0}})
        first = chunks(1,2)
        three = chunks(3)[0]
        path.write_bytes(header() + control_line(1,0) + control_line(2,5) + three + first[0] + three)
        def lifecycle(sql):
            return scalar(sql + " AND source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='node-1');")
        def rescan():
            # Advance only the test scheduler clock; no production timeout is weakened.
            run(psql,data="UPDATE eez.lifecycle_segments SET last_scan_at='epoch';")
            run(index)
        run(index)
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE state='complete'") == '1'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_chunks WHERE true") == '2'
        assert lifecycle("SELECT liveness FROM eez.lifecycle_epochs WHERE true") == 'live'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_gaps WHERE status='provisional'") == '3'
        offset = lifecycle("SELECT offset_bytes FROM eez.lifecycle_segments WHERE true")
        run(index)
        assert lifecycle("SELECT offset_bytes FROM eez.lifecycle_segments WHERE true") == offset
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_chunks WHERE true") == '2'
        run(psql,data="UPDATE eez.lifecycle_events SET first_seen_at=now()-interval '61 seconds'; UPDATE eez.lifecycle_controls SET observed_at=now()-interval '61 seconds';")
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_gaps WHERE status='suspected_missing'") == '3'
        closed = path.with_suffix('.closed')
        with path.open('ab') as stream:
            stream.write(first[1] + chunks(2)[0] + chunks(5)[0])
        path.rename(closed)
        rescan()
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE state='complete'") == '4'
        assert lifecycle("SELECT first_sequence::text||':'||last_sequence::text FROM eez.lifecycle_gaps WHERE true") == '4:4'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_segments WHERE true") == '1'
        report['checks'].append('lifecycle_atomic_offsets_rotation_duplicate_chunks_out_of_order_reassembly_and_late_gap_recovery')
        # Unknown wire and payload versions retain their bytes and positions.
        unknown = json.loads(chunks(6)[0]); unknown['version'] = 99
        future_payload = {'observed_at_unix_ms':10002,'trace':None,'event':{'event':'future_event'}}
        partial = chunks(4)[0][:-8]
        with closed.open('ab') as stream:
            stream.write(line(unknown) + chunks(7,payload=future_payload)[0] + control_line(3,7) + partial)
        rescan()
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_records WHERE reason='invalid_or_unknown_record'") == '1'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_records WHERE reason='invalid_payload'") == '1'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE state='invalid'") == '1'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_records WHERE reason='truncated_ended_epoch'") == '0'
        assert lifecycle("SELECT pending_segments FROM eez.lifecycle_coverage WHERE true") == '1'
        held_offset = lifecycle("SELECT offset_bytes FROM eez.lifecycle_segments WHERE true")
        run(psql,data="UPDATE eez.lifecycle_segments SET partial_since=now()-interval '61 seconds';")
        assert lifecycle("SELECT stale_partial_segments FROM eez.lifecycle_coverage WHERE true") == '1'
        writer_process.kill(); writer_process.wait(timeout=5); writer_process = None
        rescan()
        assert lifecycle("SELECT liveness FROM eez.lifecycle_epochs WHERE true") == 'ended'
        assert lifecycle("SELECT offset_bytes FROM eez.lifecycle_segments WHERE true") == held_offset
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_records WHERE reason='truncated_ended_epoch'") == '1'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_gaps WHERE status='quarantined'") == '3'
        assert lifecycle("SELECT unknown_trailing_epochs FROM eez.lifecycle_coverage WHERE true") == '1'
        report['checks'].append('unknown_schema_payload_quarantine_and_real_writer_crash_partial_tail_and_unknown_trailing_window')
        # A later segment contradicting an already complete event retracts only
        # that observation. Other lifecycle events and paid receipts survive.
        conflict_segment = '04' * 16
        conflicting = json.loads(three); conflicting['payload_hex'] = '00' + conflicting['payload_hex'][2:]
        (spool / (epoch + '-' + conflict_segment + '.closed')).write_bytes(header(segment_value=conflict_segment) + line(conflicting))
        rescan()
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE state='conflict'") == '1'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE state='complete'") == '3'
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='lifecycle' AND role='l1';") == '738216'
        # Missing locks and age do not prove epoch death. Two complete inventories
        # are needed before an unseen partially read segment becomes a retention gap.
        unknown_epoch, unknown_segment = '05' * 16, '06' * 16
        unknown_path = spool / (unknown_epoch + '-' + unknown_segment + '.active')
        unknown_path.write_bytes(header(unknown_epoch,unknown_segment) + chunks(1,epoch_value=unknown_epoch)[0][:-8])
        rescan()
        assert lifecycle("SELECT liveness FROM eez.lifecycle_epochs WHERE epoch=decode('" + unknown_epoch + "','hex')") == 'unknown'
        unknown_path.unlink()
        run(index)
        assert lifecycle("SELECT lost_segments FROM eez.lifecycle_coverage WHERE true") == '0'
        run(index)
        assert lifecycle("SELECT lost_segments FROM eez.lifecycle_coverage WHERE true") == '1'
        report['checks'].append('conflicting_late_chunk_retracts_transport_success_and_retention_never_infers_writer_death')
        # Administrative/read-only contracts remain enforced for the new schema.
        assert run(psql,data="SET ROLE eez_observer_reader; SELECT raw FROM eez.lifecycle_records;",check=False).returncode != 0
        run(psql,data="SET ROLE eez_observer_reader; SELECT count(*) FROM eez.lifecycle_coverage; SELECT count(*) FROM eez.lifecycle_event_history;")
        # Repeated/concurrent readers leave one copy of each logical event.
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(lambda _: run(index,check=False),range(2)))
        run(index)
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE state='complete'") == '3'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_chunks WHERE sequence=3") == '1'
        report['checks'].append('lifecycle_reader_permissions_and_concurrent_replay_preserve_unique_events')
        # Keep unresolved opaque records quarantined, not declared lost. A clean
        # ended epoch proves one missing interval even at the full u64 boundary.
        boundary_epoch, boundary_segment = '08' * 16, '09' * 16
        (spool / (boundary_epoch + '.lock')).touch(mode=0o644)
        (spool / (boundary_epoch + '-' + boundary_segment + '.closed')).write_bytes(header(boundary_epoch,boundary_segment) + control_line(1,18446744073709551615,boundary_epoch))
        rescan()
        assert lifecycle("SELECT first_sequence::text||':'||last_sequence::text FROM eez.lifecycle_gaps WHERE epoch=decode('" + boundary_epoch + "','hex') AND status='unrecoverable_from_spool'") == '1:18446744073709551615'
        assert lifecycle("SELECT unknown_liveness_epochs FROM eez.lifecycle_coverage WHERE true") == '1'
        report['checks'].append('opaque_evidence_stays_quarantined_and_full_u64_sequence_gap_is_one_bounded_interval')

        # Kill the reader after staging a data chunk but before the file offset
        # commits. Neither the chunk/event nor any byte progress may survive.
        crash_segment = '07' * 16
        (spool / (epoch + '-' + crash_segment + '.closed')).write_bytes(header(segment_value=crash_segment) + chunks(8)[0])
        run(psql,data="""CREATE FUNCTION eez.fixture_pause_lifecycle() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN
          IF NEW.segment_id=decode(repeat('07',16),'hex') AND NEW.offset_bytes>0 THEN PERFORM pg_sleep(8); END IF;
          RETURN NEW; END $$;
          CREATE TRIGGER fixture_pause_lifecycle BEFORE INSERT ON eez.lifecycle_records FOR EACH ROW EXECUTE FUNCTION eez.fixture_pause_lifecycle();""")
        run(['docker','exec','-d',name,'sh','-c','echo $$ > /tmp/observer.pid; exec /observer indexer --config /tmp/index.json --once > /tmp/observer.log 2>&1'])
        deadline = time.monotonic() + 20
        while scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='eez-observer' AND wait_event='PgSleep';") != '1':
            if time.monotonic() >= deadline:
                raise RuntimeError('lifecycle importer did not reach controlled pre-commit pause')
            time.sleep(0.1)
        run(['docker','exec',name,'sh','-c','kill -KILL "$(cat /tmp/observer.pid)"'])
        deadline = time.monotonic() + 12
        while scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='eez-observer';") != '0':
            if time.monotonic() >= deadline:
                raise RuntimeError('killed lifecycle reader left a connection open')
            time.sleep(0.1)
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE sequence=8") == '0'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_chunks WHERE sequence=8") == '0'
        assert lifecycle("SELECT offset_bytes FROM eez.lifecycle_segments WHERE segment_id=decode('" + crash_segment + "','hex')") == '0'
        run(psql,data='DROP TRIGGER fixture_pause_lifecycle ON eez.lifecycle_records; DROP FUNCTION eez.fixture_pause_lifecycle();')
        rescan()
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_events WHERE sequence=8 AND state='complete'") == '1'
        assert lifecycle("SELECT count(*) FROM eez.lifecycle_chunks WHERE sequence=8") == '1'
        report['checks'].append('killed_lifecycle_reader_rolls_back_chunks_events_and_offset_then_replays_once')
        # New source failures do not disable canonical scans or another spool.
        variant['lifecycle'].append({'service':'signer','instance':'missing-files','directory':'/missing-lifecycle'})
        write_index(variant)
        run(['docker','exec','-d',name,'sh','-c','echo $$ > /tmp/observer.pid; exec /observer indexer --config /tmp/index.json > /tmp/observer.log 2>&1'])
        deadline = time.monotonic() + 30
        good = 'eez_observer_lifecycle_collection_success{source_service="composer",source_instance="node-1"} 1'
        bad = 'eez_observer_lifecycle_error{source_service="signer",source_instance="missing-files",reason="spool_inventory"} 1'
        while True:
            metrics = run(fetch,check=False).stdout
            if good in metrics and bad in metrics and 'eez_observer_index_collection_success{chain="l1"} 1' in metrics:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('lifecycle failure isolation or process metrics did not become visible')
            time.sleep(0.2)
        run(['docker','exec',name,'sh','-c','kill -TERM "$(cat /tmp/observer.pid)"'])
        deadline = time.monotonic() + 5
        while run(['docker','exec',name,'sh','-c','kill -0 "$(cat /tmp/observer.pid)"'],check=False).returncode == 0:
            if time.monotonic() >= deadline:
                raise RuntimeError('lifecycle observer did not shut down')
            time.sleep(0.1)
        report['checks'].append('continuous_lifecycle_metrics_isolate_missing_spool_from_other_sources_and_shutdown')
        variant.pop('lifecycle')
        # Source dependencies share immutable receipt facts, while preserving the
        # original configured selection and deduplicating repeated admissions.
        source_dir = spool / 'source-receipts'
        source_dir.mkdir(mode=0o755)
        source_epoch, source_segment = '61' * 16, '62' * 16
        identity = {'environment':'fixture','deployment':'source-receipts','service':'composer','instance':'source-node'}
        source_config = copy.deepcopy(index_config)
        source_config['deployment'] = identity['deployment']
        source_config['lifecycle'] = [{'service':'composer','instance':'source-node','directory':'/lifecycle/source-receipts'}]
        def admission(tx_hash, *, genesis=100, previous=None):
            return {'observed_at_unix_ms':10002,'trace':None,'event':{'event':'admitted',
                'source':{'chain_id':1,'genesis_hash':f'{genesis:064x}','rollup_id':0,'transaction_hash':f'{tx_hash:064x}','direction':'inbound'},
                'previous':previous,'replaced':None}}
        source_path = source_dir / f'{source_epoch}-{source_segment}.closed'
        source_path.write_bytes(header(source_epoch,source_segment) + b''.join([
            *chunks(1,payload=admission(1001),epoch_value=source_epoch),
            *chunks(2,payload=admission(1002),epoch_value=source_epoch),
            *chunks(3,payload=admission(1002,previous={'epoch':source_epoch,'sequence':2}),epoch_value=source_epoch),
            *chunks(4,payload=admission(9999,genesis=999),epoch_value=source_epoch)]))
        write_index(source_config)
        control(phase='healthy',finalize=False,correlation='none',post_count=1,post_success=False,
            source_unselected=True,source_receipt_missing=True)
        run(index)
        def source_scalar(expression, view, extra=''):
            return scalar(f"SELECT {expression} FROM eez.{view} c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-receipts' AND role='l1' {extra};")
        assert source_scalar('known_total_paid','canonical_cost_totals') == '147000'
        assert source_scalar('known_total_paid','wallet_cost_totals') == '147000'
        assert source_scalar('known_total_paid','source_receipt_cost_totals') == '147000'
        assert source_scalar("known_sources||':'||missing_receipts||':'||accounting_gaps||':'||lookup_errors",'source_receipt_coverage') == '2:1:0:0'
        assert scalar("SELECT count(*) FROM eez.lifecycle_source_links WHERE source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='source-node') AND state='identity_mismatch';") == '1'
        assert scalar("SELECT count(*) FROM eez.source_receipt_requests request JOIN eez.chain_instances c ON c.id=request.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-receipts';") == '2'
        assert source_scalar('not_observed','source_receipt_coverage') == '1'
        control(source_receipt_missing=False,source_cost_missing=True)
        run(index)
        assert source_scalar('known_total_paid','canonical_cost_totals') == '147000'
        assert source_scalar("known_sources||':'||missing_receipts||':'||accounting_gaps",'source_receipt_coverage') == '2:0:1'
        assert scalar("SELECT count(*) FROM eez.blocks b JOIN eez.chain_instances c ON c.id=b.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-receipts' AND receipt_coverage='gap';") == '0'
        control(source_cost_missing=False)
        run(index)
        assert source_scalar('known_total_paid','source_receipt_cost_totals') == '738216'
        assert source_scalar("observed_receipts||':'||reverted_receipts",'source_receipt_cost_totals') == '2:1'
        assert source_scalar('known_total_paid','canonical_cost_totals') == '147000'
        assert source_scalar('count(*)','canonical_receipt_facts') == '2'
        assert source_scalar('count(*)','canonical_receipts') == '1'
        assert source_scalar('count(*)','source_receipts',"AND evidence_class='receipt_verified' AND l2_settlement_verified IS NULL") == '2'
        assert scalar("SELECT count(*) FROM eez.lifecycle_source_admissions WHERE source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='source-node');") == '3'
        run(index)
        assert source_scalar('known_total_paid','source_receipt_cost_totals') == '738216'
        # One chain's receipt failure cannot stop the other chain's lookups.
        outbound=admission(2002,genesis=200)
        outbound['event']['source'].update(chain_id=2,rollup_id=1,direction='outbound')
        source_segment_2='65'*16
        (source_dir/f'{source_epoch}-{source_segment_2}.closed').write_bytes(header(source_epoch,source_segment_2)+chunks(5,payload=outbound,epoch_value=source_epoch)[0])
        run(psql,data="UPDATE eez.source_receipt_requests SET last_attempt_at='epoch' WHERE transaction_hash=decode('"+f'{1002:064x}'+"','hex');")
        control(source_receipt_failure=True)
        result=run(index)
        assert 'source receipt chain collection failed' in result.stdout
        assert scalar("SELECT request.last_success_at IS NOT NULL FROM eez.source_receipt_requests request JOIN eez.chain_instances c ON c.id=request.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-receipts' AND c.role='l2';") == 't'
        assert scalar("SELECT evidence_class||':'||(l2_settlement_verified IS NULL)::text||':'||(l2_finality_verified IS NULL)::text FROM eez.source_receipts c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-receipts' AND role='l2';") == 'receipt_verified:true:true'
        control(source_receipt_failure=False)
        report['checks'].append('source_receipt_failures_are_chain_isolated_and_unverified_l2_settlement_remains_unknown')
        # A mismatched configured rollup must not poison another indexer's
        # correct projection of the same native admission.
        alternate=copy.deepcopy(outbound);alternate['event']['source']['transaction_hash']=f'{2001:064x}'
        source_segment_3='68'*16
        (source_dir/f'{source_epoch}-{source_segment_3}.closed').write_bytes(header(source_epoch,source_segment_3)+chunks(6,payload=alternate,epoch_value=source_epoch)[0])
        other_context=copy.deepcopy(source_config);other_context['rollup_id']=2
        write_index(other_context)
        run(index)
        assert scalar("SELECT count(*) FROM eez.lifecycle_source_links WHERE source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='source-node') AND sequence=6 AND state='identity_mismatch';") == '1'
        write_index(source_config)
        run(index)
        assert scalar("SELECT count(*) FROM eez.lifecycle_source_admissions WHERE source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='source-node') AND sequence=6;") == '1'
        assert scalar("SELECT count(DISTINCT context_l2) FROM eez.lifecycle_source_links WHERE source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='source-node') AND sequence=6;") == '2'
        report['checks'].append('source_projection_context_fence_prevents_wrong_rollup_poisoning_of_a_valid_admission')
        # Force the unselected receipt's next recheck to exercise exact indexed
        # block-body membership, independently of the normal selected scan.
        run(psql,data="UPDATE eez.source_receipt_requests SET last_attempt_at='epoch' WHERE transaction_hash=decode('" + f'{1002:064x}' + "','hex');")
        control(source_hash_mismatch=True)
        run(index)
        assert scalar("SELECT request.collection_error FROM eez.source_receipt_requests request JOIN eez.chain_instances c ON c.id=request.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-receipts' AND request.transaction_hash=decode('" + f'{1002:064x}' + "','hex');") == 'inconsistent_chain_facts'
        assert source_scalar('known_total_paid','canonical_cost_totals') == '147000'
        control(source_hash_mismatch=False,phase='fork')
        run(index)
        assert source_scalar('known_total_paid','source_receipt_cost_totals') == '804216'
        assert source_scalar('known_total_paid','canonical_cost_totals') == '147000'
        assert scalar("SELECT count(*) FROM eez.receipt_observations r JOIN eez.chain_instances c ON c.id=r.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-receipts' AND r.transaction_hash=decode('" + f'{1002:064x}' + "','hex');") == '2'
        assert source_scalar('count(*)','source_receipts',"AND transaction_hash=decode('" + f'{1002:064x}' + "','hex')") == '1'
        # Conflicted admission provenance disappears without deleting verified
        # receipt facts. Another valid admission still identifies the same source.
        run(psql,data="UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='source-node') AND sequence=2;")
        assert source_scalar('known_total_paid','source_receipt_cost_totals') == '804216'
        run(psql,data="UPDATE eez.lifecycle_events SET state='conflict',payload=NULL WHERE source_id=(SELECT id FROM eez.lifecycle_sources WHERE instance='source-node') AND sequence=3;")
        assert source_scalar('known_total_paid','source_receipt_cost_totals') == '147000'
        assert source_scalar('count(*)','canonical_receipt_facts') == '2'
        run(psql,data="SET ROLE eez_observer_reader; SELECT count(*) FROM eez.source_receipts; SELECT count(*) FROM eez.source_receipt_coverage; SELECT count(*) FROM eez.lifecycle_source_link_status;")
        report['checks'].append('source_receipt_dependencies_preserve_selection_and_fee_gaps_deduplicate_readmissions_verify_membership_and_retract_reorg_or_conflicted_links')
        # Source lookup cannot quietly extend the configured bootstrap interval.
        bounds_dir=spool/'source-bounds';bounds_dir.mkdir(mode=0o755)
        identity={**identity,'deployment':'source-bounds','instance':'bounds-node'}
        bounds_epoch,bounds_segment='63'*16,'64'*16
        (bounds_dir/f'{bounds_epoch}-{bounds_segment}.closed').write_bytes(header(bounds_epoch,bounds_segment)+chunks(1,payload=admission(1001),epoch_value=bounds_epoch)[0])
        bounded=copy.deepcopy(source_config);bounded['deployment']='source-bounds';bounded['l1']['bootstrap']=2
        bounded['lifecycle']=[{'service':'composer','instance':'bounds-node','directory':'/lifecycle/source-bounds'}]
        write_index(bounded)
        run(index)
        assert scalar("SELECT request.observation_status FROM eez.source_receipt_requests request JOIN eez.chain_instances c ON c.id=request.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-bounds';") == 'outside_indexed_interval'
        assert scalar("SELECT count(*) FROM eez.blocks b JOIN eez.chain_instances c ON c.id=b.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-bounds' AND role='l1' AND number<2;") == '0'
        assert scalar("SELECT count(*) FROM eez.receipt_observations r JOIN eez.chain_instances c ON c.id=r.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-bounds' AND role='l1';") == '0'
        report['checks'].append('source_receipt_lookup_reports_bootstrap_boundary_without_extending_the_indexed_interval')
        budget_dir=spool/'source-budget';budget_dir.mkdir(mode=0o755)
        identity={**identity,'deployment':'source-budget','instance':'budget-node'}
        budget_epoch,budget_segment='66'*16,'67'*16
        budget_records=header(budget_epoch,budget_segment)
        for index_value,tx_hash in enumerate(range(1002,1033),1):
            budget_records+=chunks(index_value,payload=admission(tx_hash),epoch_value=budget_epoch)[0]
        budget_records+=chunks(32,payload=outbound,epoch_value=budget_epoch)[0]
        (budget_dir/f'{budget_epoch}-{budget_segment}.closed').write_bytes(budget_records)
        budget=copy.deepcopy(source_config);budget['deployment']='source-budget'
        budget['lifecycle']=[{'service':'composer','instance':'budget-node','directory':'/lifecycle/source-budget'}]
        write_index(budget)
        control(phase='healthy',post_count=31,source_receipt_delay_ms=1000)
        result=run(index)
        assert 'source_receipt_budget_exhausted' in result.stdout
        assert scalar("SELECT request.last_success_at IS NOT NULL FROM eez.source_receipt_requests request JOIN eez.chain_instances c ON c.id=request.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-budget' AND c.role='l2';") == 't'
        control(source_receipt_delay_ms=0)
        run(index)
        assert scalar("SELECT known_total_paid FROM eez.source_receipt_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-budget' AND role='l1';") == str(31*591216)
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='source-budget' AND role='l1';") == '147000'
        report['checks'].append('source_receipt_deadline_preserves_partial_commits_and_other_chain_progress_then_recovers_without_duplicate_fees')
        control(source_unselected=False,source_receipt_missing=False,source_cost_missing=False,source_hash_mismatch=False,source_receipt_failure=False,source_receipt_delay_ms=0,post_count=1)
        write_index(index_config)
        # Protocol logs have their own gap/conflict coverage and do not alter
        # selected receipt costs, even when their RPC capture is unavailable.
        logs_config = copy.deepcopy(index_config)
        logs_config['deployment'] = 'protocol-logs'
        logs_config['l1']['contracts'] = ['0x' + 'bb'*20]
        write_index(logs_config)
        control(phase='healthy',finalize=False,post_success=True,post_count=1,source_unselected=False,source_receipt_missing=False,source_cost_missing=False,source_hash_mismatch=False,source_receipt_failure=False,source_receipt_delay_ms=0,protocol_logs='missing',correlation='none')
        initial_logs = run(index).stdout
        def log_scalar(expression,table='protocol_log_coverage',where=''):
            return scalar(f"SELECT {expression} FROM eez.{table} l JOIN eez.chain_instances c ON c.id=l.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='protocol-logs' AND c.role='l1' {where};")
        assert log_scalar("count(*) FILTER(WHERE state='gap')") == '2', (initial_logs, log_scalar("string_agg(state||':'||COALESCE(l.collection_error,'none'),',')"))
        assert log_scalar('known_total_paid','canonical_cost_totals') == '738216'
        control(protocol_logs='malformed')
        run(index)
        assert log_scalar("count(*) FILTER(WHERE state='gap')") == '1'
        control(protocol_logs='limit')
        run(index)
        assert log_scalar("count(*) FILTER(WHERE l.collection_error='logs_capture_limit')") == '1'
        assert log_scalar('known_total_paid','canonical_cost_totals') == '738216'
        control(protocol_logs='valid')
        run(index)
        run(index)
        assert log_scalar("count(*) FILTER(WHERE state='complete')") == '2'
        assert log_scalar('count(*)','canonical_protocol_logs') == '3'
        assert log_scalar("encode(root,'hex')",'canonical_protocol_logs',"AND kind='root'") == f'{202:064x}'
        assert log_scalar("string_agg(queue_index::text,',' ORDER BY log_index)",'canonical_protocol_logs',"AND kind='consumed'") == '1,2'
        assert log_scalar('count(*)','protocol_log_claims') == '2'
        control(protocol_logs='missing')
        run(index)
        assert log_scalar("count(*) FILTER(WHERE state='complete')") == '2'
        assert log_scalar('count(*)','canonical_protocol_logs') == '3'
        control(protocol_logs='conflict')
        run(index)
        assert log_scalar("count(*) FILTER(WHERE state='conflict')") == '1'
        assert log_scalar('count(*)','canonical_protocol_logs') == '0'
        assert log_scalar('known_total_paid','canonical_cost_totals') == '738216'
        control(protocol_logs='third')
        run(index)
        assert log_scalar('count(*)','protocol_log_claims') == '3'
        control(protocol_logs='valid')
        run(index)
        assert log_scalar("count(*) FILTER(WHERE state='conflict')") == '1'
        control(phase='fork')
        run(index)
        assert log_scalar('count(*)','canonical_protocol_logs') == '3'
        assert log_scalar('count(*)','protocol_log_claims') == '4'
        assert log_scalar('known_total_paid','canonical_cost_totals') == '804216'
        assert log_scalar("count(*) FILTER(WHERE state='conflict')") == '0'
        denied_claim = run(['docker','exec',name,'psql','-XAtq','-v','ON_ERROR_STOP=1','-U','eez_observer_writer','-d','eez_observer','-c',"UPDATE eez.protocol_log_claims SET events='[]'::jsonb"],check=False)
        assert denied_claim.returncode != 0 and 'permission denied for table protocol_log_claims' in denied_claim.stdout
        assert scalar("SET ROLE eez_observer_reader; SELECT count(*) FROM eez.protocol_log_claims;") != '0'
        scalar("SET ROLE eez_observer_reader; SELECT count(*) FROM eez.protocol_log_attempts;")
        control(source_receipt_failure=True)
        run(index)
        assert log_scalar("count(*) FILTER(WHERE l.collection_error='rpc')") == '1'
        assert log_scalar('known_total_paid','canonical_cost_totals') == '804216'
        assert log_scalar('count(*)','canonical_protocol_logs') == '3'
        control(source_receipt_failure=False)
        run(index)
        assert log_scalar("count(*) FILTER(WHERE l.collection_error IS NOT NULL)") == '0'
        assert scalar("SELECT count(*) FROM eez.protocol_log_claims l JOIN eez.chain_instances c ON c.id=l.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='scanner';") == '0'
        report['checks'].append('protocol_logs_missing_malformed_limit_recovery_empty_sets_duplicates_queue_positions_conflict_retention_and_reorg_preserve_costs')
        logs_config['deployment'] = 'protocol-log-budget'
        write_index(logs_config)
        control(phase='healthy',post_count=32)
        run(index)
        def budget_count():
            return scalar("SELECT count(*) FILTER(WHERE state='pending')||':'||count(*) FILTER(WHERE state='complete') FROM eez.protocol_log_coverage l JOIN eez.chain_instances c ON c.id=l.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='protocol-log-budget';")
        assert budget_count() == '1:32'
        run(index)
        assert budget_count() == '0:33'
        report['checks'].append('protocol_logs_bounded_pass_restarts_resume_persisted_backlog_and_disabled_config_has_no_claims')
        control(protocol_logs='missing')
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
        if writer_process is not None and writer_process.poll() is None:
            writer_process.kill()
            writer_process.wait(timeout=5)
        run(['docker', 'rm', '--force', '--volumes', name], check=False)
        run(['docker', 'rm', '--force', '--volumes', rpc_name], check=False)
        run(['docker', 'network', 'rm', network], check=False)
        remaining = run(['docker', 'ps', '-aq', '--filter', 'name=^/' + name + '$']).stdout.strip()
        remaining_rpc = run(['docker', 'ps', '-aq', '--filter', 'name=^/' + rpc_name + '$']).stdout.strip()
        remaining_network = run(['docker', 'network', 'ls', '-q', '--filter', 'name=^' + network + '$']).stdout.strip()
        remaining = remaining or remaining_rpc or remaining_network
        if remaining:
            report['status'] = 'cleanup_failed'
        spool_temp.cleanup()
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')
        if remaining:
            raise RuntimeError('Disposable database container remains')


if __name__ == '__main__':
    main()
