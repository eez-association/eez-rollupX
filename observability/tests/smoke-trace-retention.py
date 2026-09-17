#!/usr/bin/env python3
"""Prove real seven-day Tempo deletion without deleting durable SQL evidence."""
import argparse
import datetime
import importlib.util
import json
import hashlib
import subprocess
import pathlib
import re
import tempfile
import time
import urllib.error
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('restore_fixture', ROOT / 'tests/smoke-restore.py')
restore = importlib.util.module_from_spec(spec)
spec.loader.exec_module(restore)

# Read-only inspection proves block creation, retention marking and physical
# deletion independently of trace-query 404s or transient blocklist polling.
INSPECT = '''import pathlib,json
root=pathlib.Path('/volume/blocks')
rows=[]
for path in root.glob('*/*/meta*.json'):
    rows.append({'path':str(path.relative_to(root)),'meta':json.loads(path.read_text()),'files':sorted(p.name for p in path.parent.iterdir())})
print(json.dumps({'blocks':rows,'directories':sorted(str(p.relative_to(root)) for p in root.glob('*/*') if p.is_dir())}))
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--report', type=pathlib.Path, required=True)
    parser.add_argument('--browser', action='store_true', help='Follow the retained SQL trace link in actual Chromium after expiry')
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    project = 'eez-restore-src-retention-' + uuid.uuid4().hex[:10]
    started = time.monotonic()
    report = {'status': 'failed', 'project': project, 'binary_sha256': restore.digest(args.binary),
              'images': restore.IMAGES, 'checks': [], 'retention_hours': 168,
              'scope': 'historical synthetic spans, isolated volumes and SQL; production retention unchanged'}
    with tempfile.TemporaryDirectory(prefix=project) as temporary:
        stack = restore.Stack(project, pathlib.Path(temporary) / 'source')
        try:
            stack.create_source(args.binary, project)
            report['dashboard_sha256']={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((stack.runtime/'grafana/dashboards').glob('*.json'))}
            # Accept an eight-day-old fixture into a genuinely old block. Only
            # test cadence and timestamp admission differ; retention remains 168h.
            config = (ROOT / 'tempo/config.yaml').read_text()
            config = config.replace('    backend: local', '    blocklist_poll: 2s\n    backend: local')
            config = config.replace('      path: /var/tempo/wal', '      path: /var/tempo/wal\n      ingestion_time_range_slack: 240h')
            config = config.replace('backend_scheduler:\n', 'backend_scheduler:\n  maintenance_interval: 2s\n  backend_flush_interval: 2s\n')
            config = config.replace('  provider:\n', '  provider:\n    retention:\n      interval: 90s\n')
            config = config.replace('block_retention: 168h', 'block_retention: 168h\n' + ' '*8 + 'compacted_block_retention: 10s', 1)
            config = config.replace('backend_worker:\n  compaction:\n    block_retention: 168h', 'backend_worker:\n  compaction:\n    block_retention: 168h\n    compacted_block_retention: 10s')
            config += '''\nlive_store:
  wal:
    ingestion_time_range_slack: 240h
  flush_check_period: 1s
  flush_op_timeout: 2s
  max_trace_live: 2s
  max_trace_idle: 1s
  max_block_duration: 5s
  complete_block_timeout: 5s
'''
            config_path = stack.directory / 'tempo-retention.yaml'
            config_path.write_text(config)
            tempo_service = stack.rendered['services']['tempo']
            for volume in tempo_service['volumes']:
                if volume['target'] == '/etc/tempo/config.yaml':
                    volume['source'] = str(config_path)
            stack.compose_file.write_text(json.dumps(stack.rendered))
            restore.run(['docker', 'run', '--rm', '--network', 'none', '-v', str(config_path) + ':/config.yaml:ro',
                         tempo_service['image'], '-config.file=/config.yaml', '-config.verify=true'])
            restore.run(stack.compose + ['up', '-d', '--wait', '--wait-timeout', '150'])
            stack.configure_observer()
            stack.migrate()
            old_trace, new_trace = uuid.uuid4().hex, uuid.uuid4().hex
            stack.event(1, old_trace)
            stack.index()
            baseline = stack.snapshot()
            assert baseline['receipt_observations'] > 0 and baseline['events'], 'durable baseline must be populated'
            tempo = stack.base('tempo', 3200)
            alloy = stack.base('alloy-traces', 4318)
            restore.until('Tempo ready', lambda: restore.request(tempo + '/ready', raw=True).strip() == 'ready')
            effective = restore.request(tempo + '/status/config', raw=True)
            assert len(re.findall(r'block_retention:\s*168h', effective)) == 2
            report['checks'].append('effective_168h_retention_and_populated_sql')
            payload = restore.trace.trace_payload(old_trace, project)
            historical_ns = time.time_ns() - 8 * 86400 * 1_000_000_000
            for resource in payload['resourceSpans']:
                for span in resource['scopeSpans'][0]['spans']:
                    span['startTimeUnixNano'] = str(historical_ns)
                    span['endTimeUnixNano'] = str(historical_ns + 1_000_000)
            restore.request(alloy + '/v1/traces', payload)
            query = lambda identity: stack.grafana('/api/datasources/proxy/uid/eez-tempo/api/traces/' + identity)
            observed = restore.until('historical trace initially queryable through Grafana', lambda: query(old_trace))
            assert 'composer_proving' in json.dumps(observed)
            report['checks'].append('historical_otlp_ingestion_and_initial_trace_query')

            def storage_state():
                return json.loads(restore.run(['docker', 'run', '--rm', '--network', 'none', '--user', '0',
                    '--cap-drop', 'ALL', '--cap-add', 'DAC_READ_SEARCH', '--security-opt', 'no-new-privileges:true',
                    '-v', stack.volume('tempo-data') + ':/volume:ro', restore.FIXTURE_IMAGE,
                    'python3', '-c', INSPECT]))

            def blocks():
                return storage_state()['blocks']

            cutoff = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=7)
            def historical_blocks():
                return [row for row in blocks() if row['path'].endswith('/meta.json') and
                        datetime.datetime.fromisoformat(row['meta']['endTime'].replace('Z', '+00:00')) < cutoff]
            persisted = restore.until('old trace persisted in a block beyond the retention cutoff', historical_blocks, seconds=60)
            old_ids = {row['meta']['blockID'] for row in persisted}
            report['historical_blocks'] = persisted
            report['checks'].append('persisted_historical_block_metadata_before_deletion')
            # Keep a recent trace in a distinct block as a non-expiring control.
            restore.request(alloy + '/v1/traces', restore.trace.trace_payload(new_trace, project))
            restore.until('recent control trace queryable', lambda: query(new_trace))
            def marked():
                return [row for row in blocks() if row['meta']['blockID'] in old_ids and row['path'].endswith('/meta.compacted.json')]
            report['marked_blocks'] = restore.until('retention marks the known old block', marked, seconds=150)
            report['checks'].append('actual_retention_marked_known_block')
            restore.until('retention physically deletes marked old block',
                          lambda: not any(path.rsplit('/', 1)[-1] in old_ids
                                          for path in storage_state()['directories']), seconds=120)
            report['storage_after_expiry'] = storage_state()
            def missing():
                try:
                    query(old_trace)
                except urllib.error.HTTPError as error:
                    return error.code == 404
                return False
            restore.until('expired trace navigation returns unavailable', missing, seconds=60)
            assert query(new_trace), 'non-expired trace must remain available'
            assert stack.snapshot() == baseline, 'trace expiry must not alter receipts, cursors or lifecycle records'
            assert stack.sql("SELECT count(*) FROM eez.lifecycle_events WHERE payload::text LIKE '%" + old_trace + "%';").stdout.strip() == '1'
            report['checks'].append('expired_trace_unavailable_recent_trace_and_sql_evidence_preserved')
            if args.browser:
                browser_input=stack.directory/'browser.json'
                browser_output=args.report.with_name(args.report.stem+'-browser').resolve()
                browser_input.write_text(json.dumps({'url':stack.base('grafana',3000),
                    'password':(stack.runtime/'secrets/grafana-admin-password').read_text(),
                    'deployment':project,'expired_trace':old_trace,'output':str(browser_output)}))
                browser_input.chmod(0o600)
                verifier=ROOT/'tests/verify-trace-browser.cjs'
                report['browser_verifier_sha256']=hashlib.sha256(verifier.read_bytes()).hexdigest()
                subprocess.run(['node',str(verifier),str(browser_input)],check=True,timeout=90)
                report['browser']=json.loads((browser_output/'browser.json').read_text())
                assert report['browser']['status']=='passed'
                assert stack.snapshot()==baseline,'browser navigation must not alter durable accounting'
                assert query(new_trace),'recent control trace remains queryable after browser navigation'
                report['checks'].append('actual_browser_expired_trace_link_and_durable_sql_reference')

            report['elapsed_seconds'] = round(time.monotonic() - started, 2)
            report['test_overrides'] = {'ingestion_slack_hours': 240, 'blocklist_poll_seconds': 2,
                'retention_job_interval_seconds': 90, 'compacted_block_retention_seconds': 10,
                'live_store_flush_seconds': 1, 'live_store_cleanup_seconds': 2,
                'live_store_block_seconds': 5, 'complete_block_timeout_seconds': 5}
            report['status'] = 'passed'
        except Exception:
            args.report.with_suffix('.logs.txt').write_text(restore.run(stack.compose + ['logs', '--no-color', '--tail', '100'], check=False))
            raise
        finally:
            stack.cleanup()
            report['owned_resource_cleanup'] = 'verified'
            args.report.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
