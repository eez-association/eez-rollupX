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

    def test_dashboards_use_stable_datasources_and_do_not_fill_missing_values(self):
        for path in (ROOT / 'grafana/dashboards').glob('*.json'):
            dashboard = json.loads(path.read_text())
            self.assertFalse(dashboard['editable'])
            for panel in dashboard['panels']:
                if panel['type'] in ('text', 'logs'):
                    continue
                self.assertEqual(panel['datasource']['uid'], 'eez-prometheus')
                self.assertEqual(panel['fieldConfig']['defaults']['noValue'], 'Unknown')
                for target in panel['targets']:
                    self.assertNotIn('or vector(0)', target['expr'])


if __name__ == '__main__':
    unittest.main()
