#!/usr/bin/env python3
import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class Preparation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        (self.root / 'logs').mkdir()
        self.config = {'environment': 'devnet', 'deployment': 'test', 'observer_instance': 'node-1',
                       'observer_target': 'host.docker.internal:19100', 'log_directory': str(self.root / 'logs'),
                       'alert_webhook_url': 'http://receiver:9000/alerts?token=$literal',
                       'heartbeat_webhook_url': 'http://independent:9000/heartbeat'}

    def test_configuration_rerun_preserves_password_and_explicit_target_identity(self):
        runtime = prepare.prepare(self.config, self.root / 'runtime')
        secret = (runtime / 'secrets/grafana-admin-password').read_text()
        prepare.prepare(self.config, runtime)
        self.assertEqual(secret, (runtime / 'secrets/grafana-admin-password').read_text())
        self.assertEqual(runtime.stat().st_mode & 0o777, 0o700)
        prom = json.loads((runtime / 'prometheus/prometheus.yaml').read_text())
        observer = next(job for job in prom['scrape_configs'] if job['job_name'] == 'eez-observer')
        self.assertEqual(observer['static_configs'][0]['labels']['instance'], 'node-1')
        self.assertFalse(any(job['job_name'] == 'node-exporter' for job in prom['scrape_configs']))
        alerts = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())
        self.assertIn('$$literal', alerts['contactPoints'][0]['receivers'][0]['settings']['url'])
        self.assertEqual(alerts['groups'][0]['rules'][0]['data'][1]['model']['expression'], '$A > 0')
        self.assertNotIn(secret, (runtime / 'compose.env').read_text())
        self.assertNotIn(secret, (runtime / 'manifest.json').read_text())

    def test_required_receivers_and_placeholder_targets_fail_before_writing(self):
        for key, value in [('observer_target', 'REPLACE_WITH_PRIVATE_HOST:19100'),
                           ('deployment', 'bad"label'), ('alert_webhook_url', 'ftp://receiver'),
                           ('grafana_port', True), ('alert_webhook_url', None),
                           ('observer_target', None), ('public_rpc_url', 'http://rpc/?api_key=secret')]:
            config = {**self.config, key: value}
            with self.assertRaises(ValueError):
                prepare.prepare(config, self.root / 'invalid')
            self.assertFalse((self.root / 'invalid').exists())

    def test_dependency_alerts_suppress_missing_data_but_alert_on_query_errors(self):
        rules = prepare.alert_rules(self.config)[0]['rules']
        for rule in rules:
            self.assertEqual(rule['execErrState'], 'Alerting')
            if rule['uid'].startswith('eez-source-'):
                self.assertEqual(rule['noDataState'], 'OK')
                self.assertIn('up{', rule['data'][0]['model']['expr'])
        self.assertEqual(next(r for r in rules if r['uid'] == 'eez-observer-down')['noDataState'], 'Alerting')
        self.assertEqual(next(r for r in rules if r['uid'] == 'eez-heartbeat')['for'], '0s')

    def test_native_config_and_scrape_share_identity_with_explicit_private_endpoints(self):
        self.config['native_processes'] = [{'service': 'eez-composer', 'instance': 'node-1',
            'metrics_target': 'private-node:19101', 'metrics_listen': '127.0.0.1:19101',
            'lifecycle_directory': '/var/lib/eez-lifecycle/composer'}]
        runtime = prepare.prepare(self.config, self.root / 'runtime')
        native = json.loads((runtime / 'native/eez-composer-node-1.json').read_text())
        prom = json.loads((runtime / 'prometheus/prometheus.yaml').read_text())
        job = next(job for job in prom['scrape_configs'] if job['job_name'] == 'eez-composer-node-1')
        self.assertEqual(job['static_configs'][0]['labels'], native['identity'])
        self.assertEqual(native['metrics']['listen'], '127.0.0.1:19101')
        self.assertFalse(native['metrics']['allow_non_loopback'])
        self.assertEqual(native['lifecycle']['directory'], '/var/lib/eez-lifecycle/composer')
        self.assertNotIn('webhook', json.dumps(native))
        rules = prepare.alert_rules(self.config)[0]['rules']
        rule = next(rule for rule in rules if rule['uid'].startswith('eez-native-'))
        self.assertLessEqual(len(rule['uid']), 40)
        self.assertEqual(rule['noDataState'], 'Alerting')
        self.assertIn('job="eez-composer-node-1"', rule['data'][0]['model']['expr'])
        process = self.config['native_processes'][0]
        for field, value in [('service', 'unknown'), ('metrics_listen', '0.0.0.0:19101'),
                             ('metrics_listen', 'host:19101'), ('allow_non_loopback', 'false'),
                             ('lifecycle_directory', '../relative')]:
            with self.assertRaises(ValueError):
                prepare.prepare({**self.config, 'native_processes': [{**process, field: value}]}, self.root / 'invalid')
            self.assertFalse((self.root / 'invalid').exists())
        with self.assertRaises(ValueError):
            prepare.prepare({**self.config, 'native_processes': [process, process]}, self.root / 'invalid')

    def test_trace_config_is_explicit_and_reaches_native_process(self):
        process = {'service': 'eez-proof-signer', 'instance': 'signer-1',
                   'metrics_target': 'signer:19103', 'metrics_listen': '127.0.0.1:19103'}
        base = {**self.config, 'native_processes': [process]}
        runtime = prepare.prepare(base, self.root / 'runtime')
        path = runtime / 'native/eez-proof-signer-signer-1.json'
        self.assertNotIn('traces', json.loads(path.read_text()))
        traces = {'receiver': '127.0.0.1:4318', 'sample_ratio': 0.1,
                  'queue_capacity': 512, 'export_timeout_ms': 200}
        process['traces'] = traces
        prepare.prepare(base, runtime)
        self.assertEqual(json.loads(path.read_text())['traces'], traces)
        for change in [{'sample_ratio': True}, {'sample_ratio': -1}, {'sample_ratio': float('nan')},
                       {'receiver': 'http://secret@host:4318'}, {'receiver': '10.0.0.1:4318'},
                       {'receiver': '::1:4318'}, {'receiver': '[127.0.0.1]:4318'}, {'receiver': '127.0.0.1: 4318'},
                       {'queue_capacity': 0}, {'queue_capacity': 16385}, {'export_timeout_ms': 2001},
                       {'headers': {'authorization': 'secret'}}, {'allow_non_loopback': 'true'}]:
            candidate = {**self.config, 'native_processes': [{**process, 'traces': {**traces, **change}}]}
            with self.assertRaises(ValueError):
                prepare.prepare(candidate, self.root / 'invalid')
            self.assertFalse((self.root / 'invalid').exists())

    def test_trace_backend_profile_and_navigation_are_opt_in_and_reversible(self):
        runtime = prepare.prepare(self.config, self.root / 'runtime')
        self.assertFalse((runtime / 'alloy/traces.alloy').exists())
        self.config.update(traces_enabled=True, tempo_port=19420, otlp_http_port=14418)
        prepare.prepare(self.config, runtime)
        self.assertEqual((runtime / 'alloy/traces.alloy').read_text(), (ROOT / 'alloy/traces.alloy').read_text())
        sources = json.loads((runtime / 'grafana/provisioning/datasources/eez.yaml').read_text())['datasources']
        tempo = next(source for source in sources if source['uid'] == 'eez-tempo')
        loki = next(source for source in sources if source['uid'] == 'eez-loki')
        self.assertEqual(tempo['url'], 'http://tempo:3200')
        self.assertEqual(tempo['jsonData']['tracesToLogsV2']['datasourceUid'], 'eez-loki')
        self.assertEqual(loki['jsonData']['derivedFields'][0]['url'], '$${__value.raw}')
        self.assertIn('TEMPO_PORT=19420', (runtime / 'compose.env').read_text())
        self.assertIn('OTLP_HTTP_PORT=14418', (runtime / 'compose.env').read_text())
        jobs = json.loads((runtime / 'prometheus/prometheus.yaml').read_text())['scrape_configs']
        self.assertTrue(any(job['job_name'] == 'tempo' for job in jobs))
        self.assertTrue(any(job['job_name'] == 'alloy-traces' for job in jobs))
        rules = prepare.alert_rules(self.config)[0]['rules']
        loss = next(rule for rule in rules if rule['uid'] == 'eez-trace-loss')
        self.assertEqual(loss['noDataState'], 'OK')
        self.assertEqual(loss['execErrState'], 'Alerting')
        self.assertIn('[5m]', loss['data'][0]['model']['expr'])
        self.assertIn('otelcol_receiver_refused_spans_total', loss['data'][0]['model']['expr'])
        self.assertTrue(any(rule['title'] == 'EEZBackendUnavailable_tempo' for rule in rules))
        for fields in [{'traces_enabled': 'true'}, {'traces_enabled': False, 'tempo_port': 19420},
                       {'traces_enabled': True, 'otlp_http_port': True}, {'traces_enabled': True, 'tempo_port': 65536}]:
            with self.assertRaises(ValueError):
                prepare.prepare({**self.config, **fields}, self.root / 'invalid')
        self.config.pop('tempo_port')
        self.config.pop('otlp_http_port')
        self.config['traces_enabled'] = False
        prepare.prepare(self.config, runtime)
        self.assertFalse((runtime / 'alloy/traces.alloy').exists())
        sources = json.loads((runtime / 'grafana/provisioning/datasources/eez.yaml').read_text())
        self.assertTrue(sources['prune'])
        self.assertFalse(any(source['uid'] == 'eez-tempo' for source in sources['datasources']))
        self.assertFalse(any('derivedFields' in source.get('jsonData', {}) for source in sources['datasources']))

    def test_database_profile_is_opt_in_and_credentials_are_separate_and_stable(self):
        runtime = prepare.prepare(self.config, self.root / 'runtime')
        self.assertFalse((runtime / 'database').exists())
        self.assertFalse((runtime / 'secrets/postgres-admin-password').exists())
        self.config.update(database_enabled=True, postgres_port=19433)
        prepare.prepare(self.config, runtime)
        secret = (runtime / 'secrets/postgres-reader-password').read_text()
        datasources = json.loads((runtime / 'grafana/provisioning/datasources/eez.yaml').read_text())
        postgres = next(value for value in datasources['datasources'] if value['uid'] == 'eez-postgres')
        self.assertEqual(postgres['user'], 'eez_observer_reader')
        self.assertEqual(postgres['secureJsonData']['password'], secret)
        writer = json.loads((runtime / 'database/writer.json').read_text())
        self.assertIn('port=19433', writer['connection_string'])
        self.assertIn('user=eez_observer_writer', writer['connection_string'])
        self.assertNotIn(secret, writer['connection_string'])
        self.assertEqual((runtime / 'database/writer.json').stat().st_mode & 0o777, 0o600)
        prepare.prepare(self.config, runtime)
        self.assertEqual((runtime / 'secrets/postgres-reader-password').read_text(), secret)
        for role in ('admin','writer','reader'):
            value = (runtime / f'secrets/postgres-{role}-password').read_text()
            self.assertNotIn(value, (runtime / 'compose.env').read_text())
            self.assertNotIn(value, (runtime / 'manifest.json').read_text())
        for fields in ({'database_enabled':'true'}, {'database_enabled':False,'postgres_port':19432},
                       {'database_enabled':True,'postgres_port':0}):
            with self.assertRaises(ValueError):
                prepare.prepare({**self.config,**fields}, self.root / 'invalid')
        self.config.pop('postgres_port')
        self.config['database_enabled'] = False
        prepare.prepare(self.config, runtime)
        datasources = json.loads((runtime / 'grafana/provisioning/datasources/eez.yaml').read_text())
        self.assertTrue(datasources['prune'])
        self.assertFalse(any(value['uid'] == 'eez-postgres' for value in datasources['datasources']))

    def test_protocol_dashboards_follow_database_profile_and_optional_links(self):
        runtime = prepare.prepare(self.config, self.root / 'runtime')
        dashboards = runtime / 'grafana/dashboards'
        self.assertEqual(len(list(dashboards.glob('*.json'))), 2)
        self.config['database_enabled'] = True
        prepare.prepare(self.config, runtime)
        self.assertEqual(len(list(dashboards.glob('*.json'))), 6)
        gas = json.loads((dashboards / 'gas.json').read_text())
        detail = next(p for p in gas['panels'] if p['title'].startswith('Receipt detail'))
        self.assertNotIn('__EEZ_', detail['targets'][0]['rawSql'])
        self.assertIn('NULL::text', detail['targets'][0]['rawSql'])
        self.assertEqual(detail['fieldConfig']['overrides'], [])
        self.config.update(explorer_urls={'l1': "https://explorer.test/it's"}, traces_enabled=True)
        prepare.prepare(self.config, runtime)
        gas = json.loads((dashboards / 'gas.json').read_text())
        detail = next(p for p in gas['panels'] if p['title'].startswith('Receipt detail'))
        self.assertIn("https://explorer.test/it''s/tx/0x", detail['targets'][0]['rawSql'])
        self.assertEqual(len(detail['fieldConfig']['overrides']), 1)
        cross = json.loads((dashboards / 'cross-chain.json').read_text())
        trace = next(p for p in cross['panels'] if p['title'].startswith('Retained trace references'))
        self.assertIn('eez-tempo', json.dumps(trace['fieldConfig']['overrides']))
        timeline = next(p for p in cross['panels'] if p['title'].startswith('Request timeline'))
        links = timeline['fieldConfig']['overrides'][0]['properties'][0]['value']
        self.assertEqual(len(links), 2)
        self.assertIn('eez-loki', links[1]['url'])
        self.assertIn('${environment:percentencode}', links[1]['url'])
        self.assertIn('${__value.raw}', links[1]['url'])
        self.config.pop('explorer_urls')
        self.config['traces_enabled'] = False
        prepare.prepare(self.config, runtime)
        cross = json.loads((dashboards / 'cross-chain.json').read_text())
        self.assertEqual(next(p for p in cross['panels'] if p['title'].startswith('Retained trace references'))['fieldConfig']['overrides'], [])
        (dashboards / 'operator-owned.json').write_text('{}')
        self.config['database_enabled'] = False
        prepare.prepare(self.config, runtime)
        self.assertEqual({p.name for p in dashboards.glob('*.json')}, {'network.json', 'infrastructure.json', 'operator-owned.json'})

    def test_explorer_urls_reject_invalid_and_secret_bearing_configuration(self):
        for value in [None, [], {'l3': 'https://example.test'}, {'l1': None}, {'l1': 1},
                      {'l1': 'javascript:alert(1)'}, {'l1': 'https://user:secret@example.test'},
                      {'l1': 'https://example.test?secret=1'}, {'l1': 'https://example.test/#fragment'},
                      {'l1': 'https://example.test:bad'}, {'l1': 'http://bad host'},
                      {'l1': 'https://example.test/\n'}, {'l1': 'https://example.test/$secret'}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                prepare.validate({**self.config, 'explorer_urls': value})

    def test_dashboards_use_stable_datasources_and_do_not_fill_missing_values(self):
        for path in (ROOT / 'grafana/dashboards').glob('*.json'):
            dashboard = json.loads(path.read_text())
            self.assertFalse(dashboard['editable'])
            for panel in dashboard['panels']:
                if panel['type'] in ('text', 'logs'):
                    continue
                self.assertIn(panel['datasource']['uid'], ('eez-prometheus', 'eez-postgres'))
                self.assertEqual(panel['fieldConfig']['defaults']['noValue'], 'Unknown')
                for target in panel['targets']:
                    self.assertNotIn('or vector(0)', target.get('expr', ''))
                    self.assertNotIn(':raw}', target.get('rawSql', ''))


if __name__ == '__main__':
    unittest.main()
