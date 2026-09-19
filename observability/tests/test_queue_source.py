#!/usr/bin/env python3
"""Validate source switching, including actual generated rules with pinned promtool."""
import argparse
import importlib.util
import json
import pathlib
import re
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


def config():
    return {'environment': 'test', 'deployment': 'switch', 'native_processes': [
        {'service': 'eez-composer', 'instance': 'node', 'metrics_target': 'node:19101',
         'metrics_listen': '127.0.0.1:19101'}]}


class SourceSelection(unittest.TestCase):
    def test_default_and_explicit_rpc_preserve_original_rules(self):
        original = json.loads((ROOT / 'prometheus/recording-rules.yaml').read_text())
        self.assertEqual(prepare.recording_rules(config()), original)
        self.assertEqual(prepare.recording_rules({**config(), 'queue_source': 'rpc'}), original)

    def test_native_requires_unique_configured_composer_and_has_no_fallback(self):
        native = {**config(), 'queue_source': 'native'}
        rules = prepare.recording_rules(native)
        queue = [rule for group in rules['groups'] for rule in group['rules'] if rule['record'].startswith('eez:queue_')]
        self.assertEqual(len(queue), 3)
        for rule in queue:
            self.assertIn('reth_eez_native_queue_', rule['expr'])
            self.assertNotIn('eez_observer_queue_', rule['expr'])
            if rule['record'] != 'eez:queue_transactions:current':
                self.assertNotIn(' or ', rule['expr'])
            self.assertIn('<= 45', rule['expr'])
            self.assertIn('>= -5', rule['expr'])
        for invalid in ['auto', None, True, [], {}]:
            with self.assertRaises(ValueError):
                prepare.recording_rules({**config(), 'queue_source': invalid})
        for processes in [[], [{**config()['native_processes'][0], 'service': 'eez-follower'}],
                          config()['native_processes'] + [{**config()['native_processes'][0], 'instance': 'other', 'metrics_target': 'other:19101'}]]:
            with self.assertRaises(ValueError):
                prepare.recording_rules({**native, 'native_processes': processes})

    def test_network_all_catalog_contains_observer_and_native_composer_instances(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / 'logs').mkdir()
            base = {**config(), 'observer_instance': 'observer-6291',
                    'observer_target': 'observer:19100', 'log_directory': str(root / 'logs'),
                    'alert_webhook_url': 'http://alerts/', 'heartbeat_webhook_url': 'http://heartbeat/'}
            for source in ('rpc', 'native'):
                runtime = prepare.prepare({**base, 'queue_source': source}, root / source)
                dashboard = json.loads((runtime / 'grafana/dashboards/network.json').read_text())
                variable = next(v for v in dashboard['templating']['list'] if v['name'] == 'instance')
                query = variable['query']['query']
                self.assertEqual(variable['definition'], query)
                self.assertIn('environment=~"$environment"', query)
                self.assertIn('deployment=~"$deployment"', query)
                self.assertTrue(variable['includeAll'])
                self.assertEqual(variable['current']['value'], '$__all')
                self.assertNotIn('allValue', variable)
                job_pattern = re.search(r'job=~"([^"]+)"', query).group(1)
                jobs = json.loads((runtime / 'prometheus/prometheus.yaml').read_text())['scrape_configs']
                # Model label_values over the actual generated scrape labels;
                # the Grafana datasource/browser gate tests real interpolation.
                instances = {target['labels']['instance'] for job in jobs
                             if re.fullmatch(job_pattern, job['job_name'])
                             for target in job['static_configs']}
                self.assertEqual(instances, {'observer-6291', 'node'})
                for unrelated in ('eez-proof-signer-node', 'eez-follower-node', 'prometheus'):
                    self.assertIsNone(re.fullmatch(job_pattern, unrelated))
                rules = json.loads((runtime / 'prometheus/recording-rules.yaml').read_text())
                queue = next(rule for group in rules['groups'] for rule in group['rules']
                             if rule['record'] == 'eez:queue_transactions:current')
                self.assertIn('reth_eez_native_queue_' if source == 'native' else 'eez_observer_queue_', queue['expr'])

    def test_reconfiguration_switches_all_current_rules_and_can_return_to_rpc(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / 'logs').mkdir()
            value = {**config(), 'observer_instance': 'observer', 'observer_target': 'observer:19100',
                'log_directory': str(root / 'logs'), 'alert_webhook_url': 'http://alerts/', 'heartbeat_webhook_url': 'http://heartbeat/'}
            runtime = prepare.prepare(value, root / 'runtime')
            path = runtime / 'prometheus/recording-rules.yaml'
            baseline = path.read_text()
            prepare.prepare({**value, 'queue_source': 'native'}, runtime)
            self.assertNotEqual(path.read_text(), baseline)
            self.assertEqual(json.loads(path.read_text()), prepare.recording_rules({**value, 'queue_source': 'native'}))
            prepare.prepare(value, runtime)
            self.assertEqual(path.read_text(), baseline)


def promtool():
    scope = 'environment="test",deployment="switch",job="eez-composer-node",instance="node"'
    observer = 'environment="test",deployment="switch",job="eez-observer",instance="observer"'
    def name(metric, labels=''):
        return metric + '{' + scope + (',' + labels if labels else '') + '}'
    base = {name('up'): '1', name('reth_eez_native_queue_sample_valid'): '1',
            name('reth_eez_native_queue_evaluation_valid'): '1',
            name('reth_eez_native_queue_sample_timestamp_seconds'): '0',
            name('reth_eez_native_queue_transactions', 'direction="inbound",state="queued_ready"'): '2',
            name('reth_eez_native_queue_oldest_age_seconds', 'direction="inbound",kind="pending"'): '60',
            'up{' + observer + '}': '1',
            'eez_observer_collection_success{' + observer + ',source="queue"}': '1',
            'eez_observer_queue_evaluation_valid{' + observer + '}': '1',
            'eez_observer_queue_transactions{' + observer + ',state="queued_ready"}': '99'}
    tests = []
    def case(title, changes=None, remove=(), at='0s', expected=None):
        data = {**base, **(changes or {})}
        for key in remove:
            data.pop(key)
        tests.append({'name': title, 'interval': '15s', 'input_series': [{'series': key, 'values': value + '+0x8'} for key, value in data.items()],
            'promql_expr_test': [{'expr': 'sum(eez:queue_transactions:current)', 'eval_time': at,
                'exp_samples': [] if expected is None else [{'labels': '{}', 'value': expected}]}]})
    case('native only despite live RPC with different count', expected=2)
    case('failed native scrape never falls back', {name('up'): '0'})
    case('missing native sample never falls back', remove=(name('reth_eez_native_queue_sample_valid'),))
    case('invalid native sample never falls back', {name('reth_eez_native_queue_sample_valid'): '0'})
    case('stale readiness is unknown', {name('reth_eez_native_queue_evaluation_valid'): '0'})
    case('freshness exact boundary accepted', at='45s', expected=2)
    case('expired collection is unknown even while exporter responds', at='60s')
    case('future clock outside tolerance is unknown', {name('reth_eez_native_queue_sample_timestamp_seconds'): '6'})
    case('future clock tolerance boundary accepted', {name('reth_eez_native_queue_sample_timestamp_seconds'): '5'}, expected=2)
    idle = {name('reth_eez_native_queue_transactions', f'direction="{direction}",state="{state}"'): '0'
            for direction in ('inbound', 'outbound')
            for state in ('queued_ready', 'queued_blocked', 'queued_unknown', 'in_flight')}
    idle[name('reth_eez_native_queue_evaluation_valid')] = '0'
    case('complete idle population is known zero without readiness', idle, expected=0)
    # Population completeness is enough only for counts, never readiness or ages.
    tests[-1]['promql_expr_test'].extend([
        {'expr':'sum(eez:queue_evaluation_valid:current)', 'eval_time':'0s', 'exp_samples':[{'labels':'{}','value':0}]},
        {'expr':'eez:queue_oldest_age_seconds:current', 'eval_time':'0s', 'exp_samples':[]}])
    case('complete idle population with missing readiness remains known zero', idle,
         remove=(name('reth_eez_native_queue_evaluation_valid'),), expected=0)
    for missing in list(idle)[:-1]:
        case('missing idle member is unknown: ' + missing, idle, remove=(missing,))
    member = name('reth_eez_native_queue_transactions', 'direction="outbound",state="queued_unknown"')
    for value in ('1', '-1', 'NaN', '+Inf', '-Inf'):
        case('idle exception rejects nonzero or nonfinite member: ' + value, {**idle, member:value})
    case('extra unknown state is not a complete empty population',
         {**idle,name('reth_eez_native_queue_transactions','direction="inbound",state="new_state"'):'0'})
    case('duplicated pair cannot stand in for a missing pair',
         {**idle,name('reth_eez_native_queue_transactions','direction="inbound",state="queued_ready",extra="duplicate"'):'0'},
         remove=(member,))
    for field in ('up','reth_eez_native_queue_sample_valid'):
        case('idle still requires ' + field,{**idle,name(field):'0'})
        case('idle missing ' + field,idle,remove=(name(field),))
    case('idle still expires at normal sample deadline',idle,at='60s')
    case('idle freshness boundary',idle,at='45s',expected=0)
    case('idle future sample rejected',{**idle,name('reth_eez_native_queue_sample_timestamp_seconds'):'6'})
    case('idle future tolerance boundary',{**idle,name('reth_eez_native_queue_sample_timestamp_seconds'):'5'},expected=0)
    images = json.loads((ROOT / 'images.lock.json').read_text())
    image = next(name + '@' + digest for name, digest in images.items() if name.startswith('prom/prometheus:'))
    with tempfile.TemporaryDirectory(prefix='eez-queue-rules-') as directory:
        directory = pathlib.Path(directory)
        directory.chmod(0o755)
        (directory / 'rules.json').write_text(json.dumps(prepare.recording_rules({**config(), 'queue_source': 'native'})))
        (directory / 'tests.json').write_text(json.dumps({'rule_files': ['rules.json'], 'evaluation_interval': '15s', 'tests': tests}))
        subprocess.run(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', '/bin/promtool', '-v', str(directory) + ':/work:ro', '-w', '/work', image, 'test', 'rules', 'tests.json'], check=True)
    print(f'PASS: {len(tests)} exact generated native recording-rule cases')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--promtool', action='store_true')
    args = parser.parse_args()
    if args.promtool:
        promtool()
    else:
        unittest.main(argv=[__file__])
