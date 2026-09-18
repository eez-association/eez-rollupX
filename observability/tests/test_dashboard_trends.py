"""Trend panels retain sparse coverage, scoped populations and exact detail tables.

These are static/generated dashboard contracts. Real PostgreSQL/Prometheus
query values and browser rendering remain the existing datasource smoke gate.
"""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare_trends', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)
NEW = {'cross-chain': range(24, 29), 'gas': range(18, 22),
       'blobs': range(10, 14), 'settlement': range(15, 17), 'network': range(24, 27)}


def dashboard(name):
    return json.loads((ROOT / f'grafana/dashboards/{name}.json').read_text())


class DashboardTrends(unittest.TestCase):
    def test_prominent_history_charts_preserve_sparse_values_and_use_real_time_series(self):
        self.assertEqual(sum(len(ids) for ids in NEW.values()), 18)
        for name, ids in NEW.items():
            panels = {panel['id']: panel for panel in dashboard(name)['panels']}
            for panel_id in ids:
                with self.subTest(dashboard=name, panel=panel_id):
                    panel = panels[panel_id]
                    self.assertEqual(panel['type'], 'timeseries')
                    self.assertLessEqual(panel['gridPos']['y'], 31)
                    self.assertGreaterEqual(panel['gridPos']['h'], 7)
                    defaults = panel['fieldConfig']['defaults']
                    self.assertEqual(defaults['noValue'], 'Unknown')
                    self.assertIs(defaults['custom']['spanNulls'], False)
                    self.assertEqual(defaults['custom']['stacking']['mode'], 'none')
                    self.assertEqual(panel['options']['tooltip']['mode'], 'multi')
                    self.assertTrue(panel['description'])
                    for target in panel['targets']:
                        self.assertEqual(target['format'], 'time_series')
                        self.assertNotIn('or vector(0)', target.get('expr', ''))
                        self.assertNotIn('generate_series', target.get('rawSql', '').lower())
                        if 'expr' in target:
                            self.assertIs(target['range'], True)
                            self.assertIs(target['instant'], False)
                            self.assertTrue(target['legendFormat'])

    def test_sql_trends_keep_complete_scope_and_explicit_cohort_clock(self):
        for name, ids in NEW.items():
            for panel in dashboard(name)['panels']:
                if panel['id'] not in ids or panel['datasource']['uid'] != 'eez-postgres':
                    continue
                query = panel['targets'][0]['rawSql']
                with self.subTest(dashboard=name, panel=panel['id']):
                    self.assertIn('AS "time"', query)
                    self.assertIn("date_bin(interval '5 minutes'", query)
                    self.assertIn('ORDER BY 1,2', query)
                    for variable in ('environment', 'deployment', 'chain', 'rollup'):
                        self.assertIn('${' + variable + ':sqlstring}', query)
                    self.assertIn('$__timeFrom()', query)
                    self.assertIn('$__timeTo()', query)
                    self.assertNotIn('$__timeFilter(', query)
                    if name == 'cross-chain':
                        self.assertIn('IN (${instance:sqlstring})', query)
                    else:
                        self.assertIn('r.source_valid', query)
        panels = {p['id']: p for p in dashboard('cross-chain')['panels']}
        outcomes = panels[25]['targets'][0]['rawSql']
        self.assertIn("s.evidence_class='node_reconciled'", outcomes)
        self.assertIn("s.status='canonical'", outcomes)
        self.assertIn('s.observed_at_ms', outcomes)
        self.assertNotIn('progress_timestamp_seconds', outcomes)
        finality = panels[28]['targets'][0]['rawSql']
        self.assertIn("f.timing_state='observed_finalized'", finality)
        self.assertIn('f.admitted_at_ms BETWEEN extract(epoch FROM', finality)
        self.assertIn('f.accepted_to_observed_finality_seconds', finality)

    def test_cost_charts_scale_only_complete_exact_sums_and_keep_tables(self):
        populations = {'gas': {18: 'wallet_receipts', 19: 'posting_receipts', 20: 'canonical_receipts'},
                       'blobs': {12: 'posting_blob_measurements'}}
        for name, ids in populations.items():
            panels = {panel['id']: panel for panel in dashboard(name)['panels']}
            for panel_id, view in ids.items():
                query = panels[panel_id]['targets'][0]['rawSql']
                self.assertIn('FROM eez.' + view, query)
                self.assertIn('bool_and(r.accounting_gap IS NULL', query)
                self.assertIn('r.execution_paid IS NOT NULL AND r.blob_paid IS NOT NULL', query)
                self.assertIn('/1e18 END)::double precision', query)
                self.assertNotIn('UNION', query)
                self.assertNotIn('ELSE 0', query)
                self.assertIn('native units', panels[panel_id]['fieldConfig']['defaults']['unit'])
        gas = {p['id']: p for p in dashboard('gas')['panels']}
        for panel_id in (2, 3, 4, 5, 9, 10, 11, 16, 17):
            self.assertEqual(gas[panel_id]['type'], 'table')
            self.assertGreaterEqual(gas[panel_id]['gridPos']['y'], 20)
        self.assertIn('sum(r.execution_paid)::text AS known_execution_paid_wei', gas[2]['targets'][0]['rawSql'])
        blobs = {p['id']: p for p in dashboard('blobs')['panels']}
        self.assertIn('sum(r.execution_paid)::text', blobs[5]['targets'][0]['rawSql'])
        utilization = blobs[10]['targets'][0]['rawSql']
        self.assertIn("sum(r.encoded_bytes) FILTER(WHERE r.measurement_state='measured')", utilization)
        self.assertIn("sum(r.purchased_blobs*r.usable_bytes_per_blob) FILTER(WHERE r.measurement_state='measured')", utilization)
        self.assertIn('NULLIF(', utilization)

    def test_layouts_have_stable_unique_ids_and_no_overlapping_panels(self):
        for file in (ROOT / 'grafana/dashboards').glob('*.json'):
            panels = json.loads(file.read_text())['panels']
            self.assertEqual(len({p['id'] for p in panels}), len(panels), file.name)
            for i, panel in enumerate(panels):
                a = panel['gridPos']
                self.assertLessEqual(a['x'] + a['w'], 24)
                for other in panels[i + 1:]:
                    b = other['gridPos']
                    overlap = (a['x'] < b['x'] + b['w'] and b['x'] < a['x'] + a['w']
                               and a['y'] < b['y'] + b['h'] and b['y'] < a['y'] + a['h'])
                    self.assertFalse(overlap, (file.name, panel['id'], other['id']))

    def test_generated_profiles_retain_charts_and_standard_datasource_uids(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory)
            prepare.prepare_dashboards({'database_enabled': True,
                                        'native_processes': [{'instance': 'composer.one'}]}, runtime)
            for name, ids in NEW.items():
                generated = json.loads((runtime / f'grafana/dashboards/{name}.json').read_text())
                panels = {p['id']: p for p in generated['panels']}
                for panel_id in ids:
                    self.assertEqual(panels[panel_id]['type'], 'timeseries')
                    self.assertIn(panels[panel_id]['datasource']['uid'], ('eez-prometheus', 'eez-postgres'))
            prepare.prepare_dashboards({'database_enabled': False}, runtime)
            self.assertTrue((runtime / 'grafana/dashboards/network.json').exists())
            for name in ('cross-chain', 'gas', 'blobs', 'settlement'):
                self.assertFalse((runtime / f'grafana/dashboards/{name}.json').exists())


if __name__ == '__main__':
    unittest.main()
