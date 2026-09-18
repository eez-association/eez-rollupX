#!/usr/bin/env python3
import importlib.util
import json
import pathlib
import tempfile
import unittest
from urllib.parse import parse_qs, urlsplit

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

    def test_native_currency_labels_are_explicit_per_chain_and_do_not_change_amounts(self):
        config = {**self.config, 'database_enabled': True}
        runtime = prepare.prepare(config, self.root / 'currencies')
        def panel(name, panel_id):
            d = json.loads((runtime / f'grafana/dashboards/{name}.json').read_text())
            return next(p for p in d['panels'] if p['id'] == panel_id)
        original = panel('gas', 21)
        self.assertEqual(original['fieldConfig']['defaults']['unit'], 'suffix: native units')
        prepare.prepare({**config, 'native_currencies': {'l1': 'xDAI', 'l2': 'ETH'}}, runtime)
        self.assertEqual(panel('gas', 21)['fieldConfig']['defaults']['unit'], 'suffix: xDAI')
        self.assertEqual(panel('gas', 21)['targets'], original['targets'])
        self.assertEqual(panel('gas', 22)['fieldConfig']['defaults']['unit'], 'suffix: xDAI')
        self.assertEqual(panel('gas', 23)['fieldConfig']['defaults']['unit'], 'suffix: xDAI')
        self.assertEqual(panel('blobs', 12)['fieldConfig']['defaults']['unit'], 'suffix: xDAI')
        self.assertIn('xDAI', panel('gas', 21)['title'])
        for panel_id in (12, 18, 20):
            overrides = panel('gas', panel_id)['fieldConfig']['overrides']
            currency_units = {prop['value'] for o in overrides for prop in o['properties'] if prop['id'] == 'unit'}
            self.assertTrue({'suffix: xDAI', 'suffix: ETH'} <= currency_units)
            for ref_id, role, symbol in (('A', 'l1', 'xDAI'), ('B', 'l2', 'ETH')):
                current = panel('gas', panel_id)
                target = next(t for t in current['targets'] if t['refId'] == ref_id)
                if panel_id == 12:
                    self.assertIn('chain="' + role + '"', target['expr'])
                else:
                    self.assertIn("AND c.role='" + role + "' GROUP BY", target['rawSql'])
                matched = [prop['value'] for override in overrides
                           if override['matcher'] == {'id': 'byFrameRefID', 'options': ref_id}
                           for prop in override['properties'] if prop['id'] == 'unit']
                self.assertEqual(matched, ['suffix: ' + symbol])
        self.assertIn('effective_gas_price_gwei', panel('gas', 5)['targets'][0]['rawSql'])
        prepare.prepare({**config, 'native_currencies': {}}, runtime)
        self.assertEqual(panel('gas', 21)['fieldConfig']['defaults']['unit'], 'suffix: native units')
        self.assertFalse(any(o['matcher']['id'] == 'byFrameRefID' for o in panel('gas', 12)['fieldConfig']['overrides']))
        self.assertEqual(len(panel('gas', 12)['targets']), 1)
        for invalid in (None, [], {'l3': 'ETH'}, {'l1': ''}, {'l1': 123}, {'l1': '$ETH'}, {'l1': 'a' * 13}):
            with self.subTest(invalid=invalid), self.assertRaisesRegex(ValueError, 'native_currencies'):
                prepare.prepare({**config, 'native_currencies': invalid}, runtime)

    def test_network_block_activity_requires_database_profile(self):
        runtime = prepare.prepare(self.config, self.root / 'activity')
        def panels():
            return json.loads((runtime / 'grafana/dashboards/network.json').read_text())['panels']
        self.assertFalse(any(p.get('datasource', {}).get('uid') == 'eez-postgres' for p in panels()))
        prepare.prepare({**self.config, 'database_enabled': True}, runtime)
        self.assertTrue(set(range(27, 34)) <= {p['id'] for p in panels()})
        prepare.prepare(self.config, runtime)
        self.assertFalse(any(p.get('datasource', {}).get('uid') == 'eez-postgres' for p in panels()))

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

    def test_notification_origin_and_local_runbooks_are_wired_without_secret_urls(self):
        runtime = prepare.prepare(self.config, self.root / 'links')
        self.assertIn('GRAFANA_PUBLIC_URL=http://localhost:19300', (runtime / 'compose.env').read_text())
        rules = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())['groups'][0]['rules']
        self.assertTrue(all(rule['annotations']['runbook_url'] == 'http://localhost:19300/d/eez-runbooks?viewPanel=1' for rule in rules))
        dashboard = json.loads((runtime / 'grafana/dashboards/runbooks.json').read_text())
        self.assertEqual(dashboard['uid'], 'eez-runbooks')
        self.assertTrue(all(panel['type']=='text' and not panel.get('targets') for panel in dashboard['panels']))
        self.assertIn('Do not delete node data or reset the chain to recover monitoring.', dashboard['panels'][0]['options']['content'])
        explicit = {**self.config, 'grafana_public_url':'https://monitor.example.test:9443/', 'database_enabled':True,
                    'protocol_alerts':{'owner':'ops'}}
        prepare.prepare(explicit, runtime)
        rules = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())['groups'][0]['rules']
        self.assertEqual(next(rule for rule in rules if rule['uid']=='eez-progress-critical')['annotations']['runbook_url'],
                         'https://monitor.example.test:9443/d/eez-runbooks?viewPanel=2')
        self.assertIn('GF_SERVER_ROOT_URL: ${GRAFANA_PUBLIC_URL:', (ROOT / 'compose.yaml').read_text())
        prepare.prepare({**self.config,'grafana_port':0},runtime)
        self.assertIsNone(json.loads((runtime/'manifest.json').read_text())['grafana_public_url'])
        rules=json.loads((runtime/'grafana/provisioning/alerting/eez.yaml').read_text())['groups'][0]['rules']
        self.assertTrue(all('runbook_url' not in rule['annotations'] for rule in rules))
        for value in [None, '', 1, 'javascript:alert(1)', 'https://user:secret@monitor.test',
                      'http://@monitor.test', 'http://:@monitor.test', 'http://monitor.test?', 'http://monitor.test#', 'https://monitor.test?token=secret', 'https://monitor.test/#secret',
                      'http://monitor.test:0', 'http://monitor.test:bad',
                      'http://bad host', 'http://host/$secret', 'http://host\n', 'http://host\x00']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                prepare.validate({**self.config,'grafana_public_url':value})

    def test_grafana_subpaths_propagate_to_root_flags_and_notification_links(self):
        for url, base, subpath, secure in [
            ('https://eez.asuscomm.com/grafana/', 'https://eez.asuscomm.com/grafana', True, True),
            ('http://monitor.test/metrics/grafana', 'http://monitor.test/metrics/grafana', True, False),
            ('https://monitor.test/', 'https://monitor.test', False, True),
            ('http://monitor.test', 'http://monitor.test', False, False),
        ]:
            config = {**self.config, 'grafana_public_url': url, 'database_enabled': True,
                      'protocol_alerts': {'owner': 'ops'}}
            runtime = prepare.prepare(config, self.root / 'subpath')
            environment = (runtime / 'compose.env').read_text().splitlines()
            self.assertIn(f'GRAFANA_PUBLIC_URL={base}', environment)
            self.assertIn(f'GRAFANA_SERVE_FROM_SUB_PATH={str(subpath).lower()}', environment)
            self.assertIn(f'GRAFANA_COOKIE_SECURE={str(secure).lower()}', environment)
            prefix = urlsplit(base).path
            self.assertIn(f'GRAFANA_SUB_PATH={prefix}', environment)
            prometheus = json.loads((runtime / 'prometheus/prometheus.yaml').read_text())
            scrape = next(job for job in prometheus['scrape_configs'] if job['job_name'] == 'grafana')
            self.assertEqual(scrape.get('metrics_path', '/metrics'), prefix + '/metrics')
            self.assertEqual(scrape['static_configs'][0]['targets'], ['grafana:3000'])
            rules = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())['groups'][0]['rules']
            self.assertTrue(all(rule['annotations']['runbook_url'].startswith(base + '/d/eez-runbooks?') for rule in rules))
            self.assertEqual(json.loads((runtime / 'manifest.json').read_text())['grafana_public_url'], base)
        for config in [self.config, {**self.config, 'grafana_port': 0}]:
            runtime = prepare.prepare(config, self.root / 'subpath-default')
            environment = (runtime / 'compose.env').read_text().splitlines()
            self.assertIn('GRAFANA_SERVE_FROM_SUB_PATH=false', environment)
            self.assertIn('GRAFANA_COOKIE_SECURE=false', environment)
        compose = (ROOT / 'compose.yaml').read_text()
        self.assertIn('GF_SERVER_SERVE_FROM_SUB_PATH: ${GRAFANA_SERVE_FROM_SUB_PATH:-false}', compose)
        self.assertIn('GF_SECURITY_COOKIE_SECURE: ${GRAFANA_COOKIE_SECURE:-false}', compose)
        self.assertIn('http://127.0.0.1:3000${GRAFANA_SUB_PATH:-}/api/health', compose)

    def test_grafana_subpaths_reject_traversal_encoding_and_ambiguous_segments(self):
        for path in ['//grafana', '/grafana//', '/one//two', '/./grafana', '/grafana/../other',
                     '/..', '/grafana/.', '/%2e%2e/grafana', '/grafana%2fother', '/grafana%252fother',
                     '/grafana?x=1', '/grafana#fragment', '/grafana;other', '/grafana\\other',
                     '/grafana/$var', '/grafana/"quoted"', '/grafana/`command`', '/grafana/a b',
                     '/grafana/é', '/' + 'a' * 256]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                prepare.prepare({**self.config, 'grafana_public_url': 'https://monitor.test' + path},
                                self.root / 'invalid-subpath')
            self.assertFalse((self.root / 'invalid-subpath').exists())

    def test_subpath_dashboard_navigation_preserves_external_urls_and_template_links(self):
        config = {**self.config, 'database_enabled': True, 'traces_enabled': True,
                  'explorer_urls': {'l1': 'https://explorer.example'},
                  'grafana_public_url': 'https://monitor.test/monitoring/grafana/'}
        runtime = prepare.prepare(config, self.root / 'navigation')
        def urls(value):
            if isinstance(value, dict):
                for key, child in value.items():
                    if key == 'url':
                        yield child
                    else:
                        yield from urls(child)
            elif isinstance(value, list):
                for child in value:
                    yield from urls(child)
        rendered = [json.loads(path.read_text()) for path in (runtime / 'grafana/dashboards').glob('*.json')]
        links = [url for dashboard in rendered for url in urls(dashboard)]
        self.assertTrue(any(url.startswith('/monitoring/grafana/d/eez-settlement?') for url in links))
        self.assertTrue(any(url.startswith('/monitoring/grafana/explore?') for url in links))
        self.assertFalse(any(url.startswith(('/d/', '/explore?')) for url in links))
        self.assertIn('${__value.raw}', links)
        gas = json.loads((runtime / 'grafana/dashboards/gas.json').read_text())
        self.assertIn('https://explorer.example/tx/0x', json.dumps(gas))
        runbooks = json.loads((runtime / 'grafana/dashboards/runbooks.json').read_text())
        content = '\n'.join(panel['options']['content'] for panel in runbooks['panels'])
        self.assertIn('](/monitoring/grafana/d/eez-runbooks?', content)
        self.assertNotIn('](/d/eez-runbooks?', content)
        # Only app-route URL fields are rewritten; unrelated URLs/text remain literal.
        external = {'url': 'https://example.test/d/foo', 'nested': [{'url': '/explorer'},
                    {'url': '${__value.raw}'}], 'rawSql': "SELECT '/d/foo'"}
        expected = json.loads(json.dumps(external))
        prepare.grafana_subpath_links(external, '/grafana')
        self.assertEqual(external, expected)
        prepare.prepare({**config, 'grafana_public_url': 'http://localhost:19300'}, runtime)
        dashboard = json.loads((runtime / 'grafana/dashboards/cross-chain.json').read_text())
        self.assertTrue(any(url.startswith('/d/eez-settlement?') for url in urls(dashboard)))
        self.assertFalse(any(url.startswith('/monitoring/grafana/') for url in urls(dashboard)))

    def test_protocol_alerts_opt_in_thresholds_owner_and_no_unknown_zero(self):
        self.assertFalse(any(rule['uid'] == 'eez-progress-critical' for rule in prepare.alert_rules(self.config)[0]['rules']))
        config = {**self.config, 'database_enabled': True, 'protocol_alerts': {'owner': 'operations',
            'index_lag_blocks': 12, 'gap_threshold': 3, 'progress_warning_seconds': 150,
            'progress_critical_seconds': 420, 'blob_collection': True,
            'lifecycle_sources': [{'service': 'composer', 'instance': 'node-1'}]}}
        runtime = prepare.prepare(config, self.root / 'protocol')
        rules = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())['groups'][0]['rules']
        by_id = {rule['uid']: rule for rule in rules}
        self.assertEqual(len(by_id), len(rules))
        self.assertTrue(all(len(rule['uid']) <= 40 for rule in rules))
        self.assertTrue(all(rule['labels']['owner'] == 'operations' for rule in rules))
        for uid, value in [('eez-index-lag-l1', 12), ('eez-index-gap-l2', 3), ('eez-progress-critical', 420)]:
            self.assertIn(str(value), by_id[uid]['data'][0]['model']['expr'])
        self.assertEqual(by_id['eez-progress-critical']['for'], '0s')
        self.assertEqual(by_id['eez-progress-critical']['noDataState'], 'OK')
        self.assertEqual(by_id['eez-progress-critical']['execErrState'], 'Alerting')
        self.assertIn('unless', by_id['eez-progress-unknown']['data'][0]['model']['expr'])
        self.assertNotIn('or vector(0)', json.dumps(rules))
        self.assertEqual(by_id['eez-blob-coverage']['labels']['severity'], 'warning')
        prepare.prepare(self.config, runtime)
        rules = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())['groups'][0]['rules']
        self.assertFalse(any(rule['uid'].startswith('eez-progress-') for rule in rules))

    def test_invalid_protocol_alert_configuration_fails_before_writing(self):
        base = {**self.config, 'database_enabled': True}
        invalid = [None, {}, {'owner': 'REPLACE_ME'}, {'owner': 'bad"owner'},
            {'owner': 'ops', 'extra': True}, {'owner': 'ops', 'index_lag_blocks': True},
            {'owner': 'ops', 'gap_threshold': 0}, {'owner': 'ops', 'progress_warning_seconds': 300},
            {'owner': 'ops', 'blob_collection': 1}, {'owner': 'ops', 'lifecycle_sources': ['composer']},
            {'owner': 'ops', 'follower_comparison': 'yes'}, {'owner': 'ops', 'pure_l2_inclusion_grace_seconds': 60},
            {'owner': 'ops', 'policy_critical_delay_seconds': 300}, {'owner':'ops','signer_authorized_failure_threshold':2}, {'owner': 'ops', 'signer_rejection_threshold': 2},
            {'owner': 'ops', 'poster_funding_estimates': True}, {'owner': 'ops', 'funding_accounts': [{'account':'poster','chain':'l2'}]},
            {'owner': 'ops', 'funding_accounts': [{'account':'poster','chain':'l1'}]*2},
            {'owner': 'ops', 'required_blob_derivation': 1}, {'owner': 'ops', 'required_blob_retry_threshold': 5},
            {'owner': 'ops', 'required_blob_derivation': True, 'required_blob_retry_threshold': 1},
            {'owner': 'ops', 'lifecycle_sources': [{'service': 'composer', 'instance': 'x'}] * 2}]
        for value in invalid:
            with self.assertRaises(ValueError):
                prepare.prepare({**base, 'protocol_alerts': value}, self.root / 'invalid-protocol')
            self.assertFalse((self.root / 'invalid-protocol').exists())
        with self.assertRaises(ValueError):
            prepare.prepare({**self.config, 'protocol_alerts': {'owner': 'ops'}}, self.root / 'no-database')
        self.assertFalse((self.root / 'no-database').exists())

    def test_native_policy_alert_thresholds_reach_rules_and_capabilities_are_explicit(self):
        config = {**self.config, 'database_enabled': True,
            'native_processes': [{'service': service, 'instance': 'node', 'metrics_target': target,
                'metrics_listen': '127.0.0.1:19101'} for service, target in [('eez-composer', 'composer:19101'), ('eez-proof-signer', 'signer:19101')]],
            'protocol_alerts': {'owner': 'ops', 'pure_l2_inclusion_grace_seconds': 45,
                'policy_critical_delay_seconds': 90, 'signer_rejection_threshold': 7, 'signer_authorized_failure_threshold':9, 'follower_comparison': True,
                'poster_funding_estimates': True, 'funding_accounts': [{'account':'poster','chain':'l1'}],
                'required_blob_derivation': True, 'required_blob_retry_threshold': 5}}
        runtime = prepare.prepare(config, self.root / 'native-alerts')
        rules = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())['groups'][0]['rules']
        policy = next(rule for rule in rules if rule['title'] == 'EEZPureL2PolicyOverdueCritical_node')
        self.assertEqual(policy['for'], '135s')
        self.assertIn('reth_eez_settlement_due', policy['data'][0]['model']['expr'])
        self.assertTrue(all(len(rule['uid']) <= 40 for rule in rules))
        authorized = next(rule for rule in rules if rule['title'] == 'EEZSignerAuthorizedAttestationFailures_node')
        self.assertIn('>= bool 9',authorized['data'][0]['model']['expr'])
        for invalid in (0, 86401, True, '3'):
            with self.assertRaises(ValueError):
                prepare.protocol_alert_config({**config, 'protocol_alerts': {**config['protocol_alerts'], 'signer_authorized_failure_threshold': invalid}})
        signer = next(rule for rule in rules if rule['title'] == 'EEZSignerRepeatedRejections_node')
        self.assertIn('>= bool 7', signer['data'][0]['model']['expr'])
        follower = next(rule for rule in rules if rule['uid'] == 'eez-follower-mismatch')
        self.assertEqual(follower['for'], '30s')
        self.assertEqual(follower['labels']['severity'], 'critical')
        blocked = next(rule for rule in rules if rule['uid'] == 'eez-blob-required-blocked')
        self.assertIn('>= 5', blocked['data'][0]['model']['expr'])
        self.assertEqual(blocked['for'], '30s')
        poster = next(rule for rule in rules if rule['title'] == 'EEZActualPosterCannotFundCandidate_node')
        self.assertIn('reth_eez_poster_can_fund_candidate', poster['data'][0]['model']['expr'])
        self.assertNotIn('eez_observer_funding', poster['data'][0]['model']['expr'])
        for invalid in (-1, True, 86401, 1.5):
            with self.assertRaises(ValueError):
                prepare.prepare({**config, 'protocol_alerts': {**config['protocol_alerts'], 'pure_l2_inclusion_grace_seconds': invalid}}, self.root / 'bad-grace')
            self.assertFalse((self.root / 'bad-grace').exists())

    def test_selected_native_queue_alert_never_uses_rpc_readiness(self):
        config = {**self.config, 'queue_source': 'native', 'native_processes': [{'service': 'eez-composer', 'instance': 'node',
            'metrics_target': 'composer:19101', 'metrics_listen': '127.0.0.1:19101'}]}
        rule = next(rule for rule in prepare.alert_rules(config)[0]['rules'] if rule['uid'] == 'eez-queue-stale')
        expression = rule['data'][0]['model']['expr']
        self.assertIn('eez:queue_evaluation_valid:current', expression)
        self.assertIn('absent(', expression)
        self.assertIn('job="eez-composer-node"', expression)
        self.assertNotIn('eez_observer_queue_evaluation_valid', expression)

    def test_configured_client_and_node_agent_missing_scrapes_alert_explicitly(self):
        config = {**self.config, 'client_targets':[{'service':'lighthouse','instance':'beacon','metrics_target':'beacon:5054'}],
            'node_agent':{'instance':'node-agent','metrics_target':'node:12346','loki_url':'http://private-loki:3100/loki/api/v1/push','read_gid':1234,
                'sources':[{'service':'eez-composer','instance':'node','kind':'reth_json','directory':'/dedicated/logs'}]}}
        rules = [rule for rule in prepare.alert_rules(config)[0]['rules'] if rule['uid'].startswith('eez-client-')]
        self.assertEqual(len(rules),2)
        for rule in rules:
            self.assertEqual(rule['noDataState'],'Alerting')
            self.assertEqual(rule['execErrState'],'Alerting')
            self.assertIn('absent(up{',rule['data'][0]['model']['expr'])
            self.assertLessEqual(len(rule['uid']),40)

    def test_required_receivers_and_placeholder_targets_fail_before_writing(self):
        for key, value in [('observer_target', 'REPLACE_WITH_PRIVATE_HOST:19100'),
                           ('deployment', 'bad"label'), ('alert_webhook_url', 'ftp://receiver'),
                           ('grafana_port', True), ('alert_webhook_url', None),
                           ('observer_target', None), ('public_rpc_url', 'http://rpc/?api_key=secret')]:
            config = {**self.config, key: value}
            with self.assertRaises(ValueError):
                prepare.prepare(config, self.root / 'invalid')
            self.assertFalse((self.root / 'invalid').exists())

    def test_alerting_defaults_enabled_and_requires_both_receivers(self):
        for enabled in [None, True]:
            config = self.config.copy()
            if enabled is not None:
                config['alerting_enabled'] = enabled
            runtime = prepare.prepare(config, self.root / str(enabled))
            alerts = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())
            self.assertTrue(all(not rule['isPaused'] for group in alerts['groups'] for rule in group['rules']))
            self.assertEqual(len(alerts['contactPoints']), 2)
            self.assertEqual(alerts['policies'][0]['receiver'], 'eez-operator')
            self.assertTrue(json.loads((runtime / 'manifest.json').read_text())['alerting_enabled'])
            for receiver in ['alert_webhook_url', 'heartbeat_webhook_url']:
                for missing in [True, False]:
                    invalid = config.copy()
                    if missing:
                        invalid.pop(receiver)
                    else:
                        invalid[receiver] = None
                    with self.subTest(enabled=enabled, receiver=receiver, missing=missing), self.assertRaises(ValueError):
                        prepare.prepare(invalid, self.root / 'invalid-alerts')
                    self.assertFalse((self.root / 'invalid-alerts').exists())

    def test_disabled_alerting_is_paused_without_fabricated_receivers_or_policy_reset(self):
        for nulls in [False, True]:
            config = {key: value for key, value in self.config.items()
                      if key not in ('alert_webhook_url', 'heartbeat_webhook_url')}
            config['alerting_enabled'] = False
            if nulls:
                config.update(alert_webhook_url=None, heartbeat_webhook_url=None)
            runtime = prepare.prepare(config, self.root / f'disabled-{nulls}')
            alerts = json.loads((runtime / 'grafana/provisioning/alerting/eez.yaml').read_text())
            self.assertEqual(set(alerts), {'apiVersion', 'groups'})
            self.assertTrue(alerts['groups'][0]['rules'])
            self.assertTrue(all(rule['isPaused'] for group in alerts['groups'] for rule in group['rules']))
            self.assertFalse(json.loads((runtime / 'manifest.json').read_text())['alerting_enabled'])
            self.assertTrue((runtime / 'grafana/dashboards/network.json').exists())
            self.assertTrue((runtime / 'prometheus/recording-rules.yaml').exists())

    def test_disabled_alerting_rejects_dead_configuration_before_writes(self):
        base = {**self.config, 'alerting_enabled': False, 'alert_webhook_url': None, 'heartbeat_webhook_url': None}
        for field, value in [('alerting_enabled', None), ('alerting_enabled', 0),
                             ('alerting_enabled', 'false'), ('alert_webhook_url', 'http://receiver'),
                             ('heartbeat_webhook_url', 'http://receiver'), ('alert_webhook_url', ''),
                             ('protocol_alerts', {'owner': 'ops'}), ('protocol_alerts', None)]:
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                prepare.prepare({**base, field: value}, self.root / 'invalid-disabled')
            self.assertFalse((self.root / 'invalid-disabled').exists())

    def test_disabling_alerts_pauses_previously_generated_optional_rules_and_reenable_restores_current(self):
        enabled = {**self.config, 'database_enabled': True, 'protocol_alerts': {'owner': 'ops'},
                   'node_exporter_target': 'host.docker.internal:19101'}
        runtime = prepare.prepare(enabled, self.root / 'transition')
        path = runtime / 'grafana/provisioning/alerting/eez.yaml'
        old = json.loads(path.read_text())
        previous_uids = {rule['uid'] for group in old['groups'] for rule in group['rules']}
        self.assertIn('eez-progress-critical', previous_uids)
        # This generator owns only its EEZ group, never unrelated Grafana rules.
        old['groups'].append({'orgId': 1, 'folder': 'Other', 'name': 'other',
                              'rules': [{'uid': 'eez-unrelated', 'isPaused': False}]})
        path.write_text(json.dumps(old))
        disabled = {**self.config, 'alerting_enabled': False, 'alert_webhook_url': None,
                    'heartbeat_webhook_url': None}
        for _ in range(2):
            prepare.prepare(disabled, runtime)
            paused = json.loads(path.read_text())
            self.assertEqual(set(paused), {'apiVersion', 'groups'})
            rules = paused['groups'][0]['rules']
            self.assertEqual({rule['uid'] for rule in rules}, previous_uids)
            self.assertEqual(len(rules), len(previous_uids))
            self.assertTrue(all(rule['isPaused'] for rule in rules))
        prepare.prepare(enabled, runtime)
        restored = json.loads(path.read_text())
        self.assertEqual({rule['uid'] for rule in restored['groups'][0]['rules']}, previous_uids)
        self.assertTrue(all(not rule['isPaused'] for rule in restored['groups'][0]['rules']))
        self.assertIn('contactPoints', restored)
        self.assertIn('policies', restored)

    def test_disabled_alerting_refuses_corrupt_previous_rule_inventory_before_writes(self):
        runtime = prepare.prepare(self.config, self.root / 'corrupt-alerts')
        manifest = (runtime / 'manifest.json').read_bytes()
        path = runtime / 'grafana/provisioning/alerting/eez.yaml'
        for invalid in ['not json', '[]', '{"groups":{}}', '{"groups":[null]}',
                        '{"groups":[{"orgId":1,"folder":"EEZ","name":"eez-operational","rules":[null]}]}']:
            path.write_text(invalid)
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                prepare.prepare({**self.config, 'alerting_enabled': False,
                                 'alert_webhook_url': None, 'heartbeat_webhook_url': None}, runtime)
            self.assertEqual(path.read_text(), invalid)
            self.assertEqual((runtime / 'manifest.json').read_bytes(), manifest)

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
        self.assertEqual(postgres['jsonData']['maxOpenConns'], 1)
        self.assertEqual(postgres['jsonData']['maxIdleConns'], 1)
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

    def test_database_container_credentials_share_secrets_without_changing_host_files(self):
        for port in [19432, 23456]:
            config = {**self.config, 'database_enabled': True, 'postgres_port': port}
            runtime = prepare.prepare(config, self.root / f'database-container-{port}')
            for role, user in [('admin', 'postgres'), ('writer', 'eez_observer_writer')]:
                password = (runtime / f'secrets/postgres-{role}-password').read_text()
                host_path = runtime / f'database/{role}.json'
                container_path = runtime / f'database/{role}-container.json'
                host = json.loads(host_path.read_text())
                container = json.loads(container_path.read_text())
                self.assertEqual(host, {'connection_string':
                    f'host=127.0.0.1 port={port} user={user} dbname=eez_observer password={password} sslmode=disable',
                    'allow_private_plaintext': True})
                self.assertEqual(container, {'connection_string':
                    f'host=postgres port=5432 user={user} dbname=eez_observer password={password} sslmode=disable',
                    'allow_private_plaintext': True})
                self.assertEqual(host_path.stat().st_mode & 0o777, 0o600)
                self.assertEqual(container_path.stat().st_mode & 0o777, 0o600)
                saved = (host_path.read_bytes(), container_path.read_bytes())
                prepare.prepare(config, runtime)
                self.assertEqual((host_path.read_bytes(), container_path.read_bytes()), saved)

    def test_protocol_dashboards_follow_database_profile_and_optional_links(self):
        runtime = prepare.prepare(self.config, self.root / 'runtime')
        dashboards = runtime / 'grafana/dashboards'
        self.assertEqual(len(list(dashboards.glob('*.json'))), 3)
        self.config['database_enabled'] = True
        prepare.prepare(self.config, runtime)
        self.assertEqual(len(list(dashboards.glob('*.json'))), 7)
        gas = json.loads((dashboards / 'gas.json').read_text())
        detail = next(p for p in gas['panels'] if p['title'].startswith('Receipt detail'))
        self.assertNotIn('__EEZ_', detail['targets'][0]['rawSql'])
        self.assertIn('NULL::text', detail['targets'][0]['rawSql'])
        string_override = {'matcher': {'id': 'byType', 'options': 'string'}, 'properties': [{'id': 'unit', 'value': 'string'}]}
        original = json.loads((ROOT / 'grafana/dashboards/gas.json').read_text())
        original_overrides = next(p for p in original['panels'] if p['id'] == detail['id'])['fieldConfig']['overrides']
        self.assertEqual(detail['fieldConfig']['overrides'], original_overrides + [string_override])
        self.config.update(explorer_urls={'l1': "https://explorer.test/it's"}, traces_enabled=True)
        prepare.prepare(self.config, runtime)
        gas = json.loads((dashboards / 'gas.json').read_text())
        detail = next(p for p in gas['panels'] if p['title'].startswith('Receipt detail'))
        self.assertIn("https://explorer.test/it''s/tx/0x", detail['targets'][0]['rawSql'])
        self.assertEqual(len(detail['fieldConfig']['overrides']), len(original_overrides) + 2)
        self.assertEqual(detail['fieldConfig']['overrides'][:len(original_overrides)], original_overrides)
        self.assertIn(string_override, detail['fieldConfig']['overrides'])
        cross = json.loads((dashboards / 'cross-chain.json').read_text())
        trace = next(p for p in cross['panels'] if p['title'].startswith('Retained trace references'))
        self.assertIn('eez-tempo', json.dumps(trace['fieldConfig']['overrides']))
        trace_link = trace['fieldConfig']['overrides'][0]['properties'][0]['value'][0]['url']
        panes = json.loads(parse_qs(urlsplit(trace_link).query)['panes'][0])
        self.assertEqual(panes['trace']['queries'][0], {
            'refId': 'A', 'queryType': 'traceql', 'query': '${__value.raw}'})
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
        self.assertEqual(next(p for p in cross['panels'] if p['title'].startswith('Retained trace references'))['fieldConfig']['overrides'], [string_override])
        (dashboards / 'operator-owned.json').write_text('{}')
        self.config['database_enabled'] = False
        prepare.prepare(self.config, runtime)
        self.assertEqual({p.name for p in dashboards.glob('*.json')}, {'network.json', 'infrastructure.json', 'runbooks.json', 'operator-owned.json'})

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
