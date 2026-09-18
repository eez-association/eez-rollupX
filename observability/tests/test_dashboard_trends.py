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
NEW = {'cross-chain': tuple(range(24, 31)) + (34, 35), 'gas': range(18, 22),
       'blobs': range(10, 14), 'settlement': range(15, 17), 'network': range(24, 27)}


def dashboard(name):
    return json.loads((ROOT / f'grafana/dashboards/{name}.json').read_text())


class DashboardTrends(unittest.TestCase):
    def test_prominent_history_charts_preserve_sparse_values_and_use_real_time_series(self):
        self.assertEqual(sum(len(ids) for ids in NEW.values()), 22)
        for name, ids in NEW.items():
            panels = {panel['id']: panel for panel in dashboard(name)['panels']}
            for panel_id in ids:
                with self.subTest(dashboard=name, panel=panel_id):
                    panel = panels[panel_id]
                    self.assertEqual(panel['type'], 'timeseries')
                    self.assertLessEqual(panel['gridPos']['y'], 68 if name == 'cross-chain' else 31)
                    self.assertGreaterEqual(panel['gridPos']['h'], 7)
                    defaults = panel['fieldConfig']['defaults']
                    self.assertEqual(defaults['noValue'], 'Unknown')
                    self.assertIs(defaults['custom']['spanNulls'], False)
                    self.assertEqual(defaults['custom']['stacking']['mode'], 'normal' if name == 'cross-chain' and panel_id in (26, 29, 34) else 'none')
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
                self.assertEqual('suffix: native units', panels[panel_id]['fieldConfig']['defaults']['unit'])
        gas = {p['id']: p for p in dashboard('gas')['panels']}
        for panel_id in (2, 3, 4, 5, 9, 10, 11, 16, 17):
            self.assertEqual(gas[panel_id]['type'], 'table')
            self.assertGreaterEqual(gas[panel_id]['gridPos']['y'], 20)
        self.assertIn('known_execution_paid_gwei', gas[2]['targets'][0]['rawSql'])
        self.assertIn('0.000000001', gas[2]['targets'][0]['rawSql'])
        blobs = {p['id']: p for p in dashboard('blobs')['panels']}
        self.assertIn('known_execution_paid_gwei', blobs[5]['targets'][0]['rawSql'])
        self.assertIn('0.000000001', blobs[5]['targets'][0]['rawSql'])
        utilization = blobs[10]['targets'][0]['rawSql']
        self.assertIn("sum(r.encoded_bytes) FILTER(WHERE r.measurement_state='measured')", utilization)
        self.assertIn("sum(r.purchased_blobs*r.usable_bytes_per_blob) FILTER(WHERE r.measurement_state='measured')", utilization)
        self.assertIn('NULLIF(', utilization)

    def test_fee_components_totals_and_balances_use_native_units_without_stacking_total(self):
        for name, ids in [('gas', (19, 20, 22)), ('blobs', (12,))]:
            panels = {p['id']: p for p in dashboard(name)['panels']}
            for panel_id in ids:
                p = panels[panel_id]
                query = p['targets'][0]['rawSql']
                self.assertEqual(p['fieldConfig']['defaults']['unit'], 'suffix: native units')
                for field in ('execution_native', 'blob_native', 'total_native'):
                    self.assertIn('AS ' + field, query)
                self.assertIn('sum(r.execution_paid+r.blob_paid)/1e18', query)
                self.assertNotIn('COALESCE', query)
                if p['type'] == 'timeseries':
                    self.assertEqual(p['fieldConfig']['defaults']['custom']['stacking']['mode'], 'none')
        gas = {p['id']: p for p in dashboard('gas')['panels']}
        self.assertIn("c.role='l1'", gas[22]['targets'][0]['rawSql'])
        for panel_id in (12, 21):
            self.assertEqual(gas[panel_id]['fieldConfig']['defaults']['unit'], 'suffix: native units')
            self.assertNotIn(' * 1e9', gas[panel_id]['targets'][0]['expr'])

    def test_queue_directions_partition_states_and_keep_snapshot_freshness(self):
        panels = {p['id']: p for p in dashboard('cross-chain')['panels']}
        states = ('queued_ready', 'queued_blocked', 'queued_unknown', 'in_flight')
        for panel_id, direction in ((26, 'inbound'), (29, 'outbound')):
            panel = panels[panel_id]
            self.assertLessEqual(panel['gridPos']['y'], 10)
            self.assertEqual(len(panel['targets']), 4)
            for target, state in zip(panel['targets'], states):
                expr = target['expr']
                self.assertIn('direction="' + direction + '",state="' + state + '"', expr)
                self.assertIn('eez_native_queue_sample_valid', expr)
                self.assertIn('<= 45', expr)
                self.assertIn('>= -5', expr)
                self.assertIn('up{', expr)
                self.assertNotIn('rate(', expr)
                self.assertNotIn('or vector(0)', expr)
        age = panels[30]
        self.assertEqual(age['fieldConfig']['defaults']['unit'], 's')
        self.assertIn('eez_native_queue_oldest_age_seconds', age['targets'][0]['expr'])
        self.assertNotIn('rate(', age['targets'][0]['expr'])

    def test_pressure_diagnostics_keep_rates_gauges_and_dispositions_separate(self):
        panels = {p['id']: p for p in dashboard('cross-chain')['panels']}
        flow = panels[24]['targets']
        self.assertEqual([t['refId'] for t in flow], ['A', 'B', 'C'])
        self.assertIn('admissions_total', flow[0]['expr'])
        self.assertIn('terminal_observations_total', flow[1]['expr'])
        self.assertNotIn('outcome=', flow[1]['expr'])
        self.assertIn('outcome="committed"', flow[2]['expr'])
        self.assertIn('outcome!="committed"', panels[35]['targets'][0]['expr'])
        bands = panels[34]['targets']
        for index, target in enumerate(bands):
            self.assertNotIn('rate(', target['expr'])
            self.assertNotIn('clamp_min', target['expr'])
            self.assertNotIn('queue_evaluation_valid', target['expr'])
            self.assertIn('queue_sample_valid', target['expr'])
            if index in (1, 2):
                self.assertIn(' - ignoring(le) ', target['expr'])
                self.assertIn('>= 0', target['expr'])
        self.assertIn('queue_age_unknown_transactions', bands[3]['expr'])

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
