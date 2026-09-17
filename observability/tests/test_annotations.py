"""Annotation scopes and capability gating in generated dashboards."""
import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare_annotations', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class Annotations(unittest.TestCase):
    def test_profiles_retain_scoped_release_records_without_inventing_log_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = pathlib.Path(directory)
            for database, count in ((False, 3), (True, 7), (False, 3)):
                prepare.prepare_dashboards({'database_enabled': database}, runtime)
                dashboards = list((runtime / 'grafana/dashboards').glob('*.json'))
                self.assertEqual(len(dashboards), count)
                for path in dashboards:
                    entries = json.loads(path.read_text())['annotations']['list']
                    releases = [entry for entry in entries if entry['name'] == 'Verified release records']
                    self.assertEqual(len(releases), 1)
                    self.assertFalse(releases[0]['matchAny'])
                    self.assertEqual(releases[0]['tags'], ['eez-release', 'environment:$environment', 'deployment:$deployment'])
                    self.assertFalse(any(entry['datasource']['uid'] == 'eez-loki' for entry in entries))

    def test_policy_annotation_requires_actual_composer_diagnostic_source(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = pathlib.Path(directory)
            for kind, service, expected in [('lifecycle', 'eez-composer', False),
                    ('reth_json', 'eez-follower', False), ('reth_json', 'eez-composer', True)]:
                prepare.prepare_dashboards({'node_agent': {'sources': [{'kind': kind, 'service': service}]}}, runtime)
                entries = json.loads((runtime / 'grafana/dashboards/network.json').read_text())['annotations']['list']
                logs = [entry for entry in entries if entry['datasource']['uid'] == 'eez-loki']
                self.assertEqual(bool(logs), expected)
                if expected:
                    self.assertIn('environment="$environment",deployment="$deployment"', logs[0]['expr'])
                    self.assertIn('settlement.policy|emission.limits', logs[0]['expr'])
                    self.assertEqual(logs[0]['maxLines'], 100)


if __name__ == '__main__':
    unittest.main()
