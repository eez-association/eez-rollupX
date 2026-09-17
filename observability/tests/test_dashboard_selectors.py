"""Grafana custom All bypasses formatters: SQL and PromQL need distinct handling."""
import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare_selectors', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class Selectors(unittest.TestCase):
    def test_all_is_valid_sql_and_instance_lists_keep_regular_escaping(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = pathlib.Path(directory)
            for native in ([], [{'instance': 'composer.one'}, {'instance': 'signer-two'}]):
                prepare.prepare_dashboards({'database_enabled': True, 'native_processes': native}, runtime)
                for name in ('cross-chain', 'settlement', 'blobs', 'gas'):
                    dashboard = json.loads((runtime / f'grafana/dashboards/{name}.json').read_text())
                    variables = {value['name']: value for value in dashboard['templating']['list']}
                    self.assertEqual(variables['chain']['allValue'], "'all'")
                    self.assertEqual(variables['rollup']['allValue'], "'all'")
                    self.assertNotIn('allValue', variables['instance'])
                    self.assertIn("'Unavailable' AS __text,'' AS __value", variables['instance']['query'])
                    if native:
                        self.assertIn("'composer.one'", variables['instance']['query'])
                    self.assertNotIn("${instance:sqlstring}='.*'", json.dumps(dashboard))
                    self.assertIn('IN (${instance:sqlstring})', json.dumps(dashboard))


if __name__ == '__main__':
    unittest.main()
