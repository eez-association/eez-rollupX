#!/usr/bin/env python3
"""Run the shared funding process assertions on fresh isolated database fixtures."""
import argparse
import importlib.util
import json
import pathlib
import tempfile
import uuid

import funding
import canonical_revision

ROOT = pathlib.Path(__file__).resolve().parents[1]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'tests' / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    restore, database = module('restore_fixture', 'smoke-restore.py'), module('database_fixture', 'smoke-database.py')
    name = 'eez-restore-src-funding-' + uuid.uuid4().hex[:10]
    report = {'scope': 'actual optional funding worker with private synthetic RPC/PostgreSQL; no operator accounts',
              'binary_sha256': restore.digest(args.binary), 'checks': [], 'status': 'failed'}
    with tempfile.TemporaryDirectory(prefix='eez-funding-') as temporary:
        stack = restore.Stack(name, pathlib.Path(temporary) / 'source')
        try:
            stack.create_source(args.binary, 'funding')
            restore.run(stack.compose + ['up', '-d', '--wait', '--wait-timeout', '120', 'postgres', 'rpc'])
            stack.configure_observer()
            stack.migrate()
            psql = ['docker','exec','-i',stack.container('postgres'),'psql','-X','-v','ON_ERROR_STOP=1','-U','postgres','-d','eez_observer']
            canonical_revision.exercise(database.run, psql, lambda sql: stack.sql(sql).stdout.strip(), report)
            config = json.loads((stack.runtime / 'indexer.json').read_text())
            config['lifecycle'] = []
            def control(**values):
                script = "import urllib.request,sys; urllib.request.urlopen(urllib.request.Request('http://localhost:9000/control',data=sys.stdin.buffer.read(),headers={'Content-Type':'application/json'})).read()"
                database.run(['docker','exec','-i',stack.container('rpc'),'python3','-c',script],data=json.dumps(values))
            postgres = restore.run(['docker','inspect','--format','{{.Name}}',stack.container('postgres')]).strip().lstrip('/')
            funding.exercise(database.run, lambda sql: stack.sql(sql).stdout.strip(), config, control,
                             postgres, stack.container('rpc'), report)
            report['status'] = 'passed'
        except Exception:
            args.report.with_suffix('.logs.txt').write_text(restore.run(stack.compose + ['logs', '--no-color', '--tail', '50'],check=False))
            raise
        finally:
            stack.cleanup()
            report['owned_resource_cleanup'] = 'verified'
            args.report.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
