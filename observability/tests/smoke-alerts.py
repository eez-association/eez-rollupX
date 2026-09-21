#!/usr/bin/env python3
"""Actual Grafana notification/suppression drill on synthetic metric inputs."""
import argparse
import importlib.util
import json
import pathlib
import tempfile
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('trace_helpers', ROOT / 'tests/smoke-traces.py')
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)
run, request, until, prepare = helpers.run, helpers.request, helpers.until, helpers.prepare


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    project = 'eez-alert-test-' + uuid.uuid4().hex[:10]
    report = {'project': project, 'scope': 'synthetic metrics with actual pinned Prometheus/Grafana and webhook delivery; no native fault injection', 'checks': [], 'status': 'failed'}
    images = json.loads((ROOT / 'tests/images.lock.json').read_text())
    fixture_image = next(name + '@' + digest for name, digest in images.items())
    with tempfile.TemporaryDirectory(prefix=project + '-') as temporary:
        directory = pathlib.Path(temporary)
        logs = directory / 'logs'
        logs.mkdir()
        config = {'environment': 'devnet', 'deployment': project, 'observer_instance': 'fixture', 'observer_target': 'fixture:9000',
            'log_directory': str(logs), 'alert_webhook_url': 'http://fixture:9000/alerts', 'heartbeat_webhook_url': 'http://fixture:9000/heartbeat',
            'grafana_port': 0, 'prometheus_port': 0, 'loki_port': 0, 'database_enabled': True,
            'protocol_alerts': {'owner': 'fixture-operator', 'blob_collection': True,
                'follower_comparison': True, 'pure_l2_inclusion_grace_seconds': 60,
                'poster_funding_estimates': True, 'funding_accounts': [{'account': 'poster', 'chain': 'l1'}],
                'required_blob_derivation': True,
                'lifecycle_sources': [{'service': 'composer', 'instance': 'node'}]},
            'native_processes': [{'service': 'eez-composer', 'instance': 'node', 'metrics_target': 'fixture:9000',
                'metrics_listen': '127.0.0.1:19101', 'lifecycle_directory': '/fixture-spool',
                'traces': {'receiver': '127.0.0.1:14318', 'sample_ratio': 1.0}},
                {'service': 'eez-proof-signer', 'instance': 'signer', 'metrics_target': 'signer-fixture:9000', 'metrics_listen': '127.0.0.1:19102'}]}
        runtime = prepare.prepare(config, directory / 'runtime', fast_test=True)
        override = directory / 'fixture.json'
        override.write_text(json.dumps({'services': {'fixture': {'image': fixture_image, 'command': ['python3', '/fixture.py'],
            'ports': ['127.0.0.1:0:9000'], 'volumes': [str(ROOT / 'tests/fixture-server.py') + ':/fixture.py:ro'],
            'mem_limit': '128m', 'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true'],
            'networks': {'default': {'aliases': ['signer-fixture']}}}}}))
        compose = ['docker', 'compose', '--project-name', project, '--env-file', str(runtime / 'compose.env'),
            '-f', str(ROOT / 'compose.yaml'), '-f', str(override)]
        try:
            run(compose + ['up', '-d', '--wait', '--wait-timeout', '150', 'fixture', 'prometheus', 'grafana'])
            base = lambda service, port: 'http://' + run(compose + ['port', service, str(port)]).strip()
            fixture, prom, grafana = base('fixture', 9000), base('prometheus', 9090), base('grafana', 3000)
            password = (runtime / 'secrets/grafana-admin-password').read_text()
            def query(expression):
                return request(prom + '/api/v1/query?' + helpers.urllib.parse.urlencode({'query': expression}))['data']['result']
            baseline = {
                'eez_observer_collection_success{source="l1"}': '1', 'eez_observer_collection_success{source="l2"}': '1',
                'eez_observer_collection_success{source="queue"}': '1', 'eez_observer_queue_evaluation_valid': '1',
                'eez_observer_chain_head_number{source="l1"}': '$HEAD', 'eez_observer_chain_head_timestamp_seconds{source="l1"}': '$NOW',
                'eez_observer_queue_transactions{state="queued_ready"}': '2', 'eez_observer_queue_transactions{state="in_flight"}': '0',
                'eez_observer_canonical_outcome_evaluation_valid': '1',
                'eez_observer_last_canonical_terminal_timestamp_seconds{direction="inbound"}': '$NOW',
                'eez_observer_blob_collection_success': '1', 'eez_observer_blob_posts{state="measured"}': '2',
                'eez_observer_blob_posts{state="unavailable"}': '0',
                'eez_observer_lifecycle_collection_success{source_service="composer",source_instance="node"}': '1',
                'reth_eez_lifecycle_writer_enabled': '1',
                'reth_eez_telemetry_dropped_events_total{signal="trace",reason="queue_full"}': '0',
                'reth_eez_telemetry_dropped_events_total{signal="lifecycle",reason="queue_full"}': '0',
                'reth_eez_telemetry_writer_errors_total{reason="io"}': '0'}
            baseline.update({'eez_observer_follower_comparison_known': '1', 'eez_observer_follower_state_root_agreement': '1',
                'reth_eez_settlement_policy_enabled': '1', 'reth_eez_settlement_policy_setting{setting="pure_l2_interval_seconds"}': '120',
                'reth_eez_settlement_policy_evaluation_valid{rollup="1"}': '1', 'reth_eez_settlement_policy_evaluation_timestamp_seconds{rollup="1"}': '$NOW',
                'reth_eez_settlement_anchor_age_seconds{rollup="1"}': '60', 'reth_eez_settlement_pending_pure_l2_transactions{rollup="1"}': '2',
                'reth_eez_settlement_due{rollup="1"}': '1'})
            for outcome in ('success', 'invalid', 'rejected'):
                baseline[f'eez_signer_requests_total{{outcome="{outcome}"}}'] = '0'
            for outcome in ('success','internal','deadline','panicked','cancelled'):
                baseline[f'eez_signer_authorized_requests_total{{outcome="{outcome}"}}'] = '0'
            baseline.update({'eez_observer_funding_source_valid{account="poster",chain="l1"}': '1',
                'eez_observer_funding_burn_valid{account="poster",chain="l1"}': '1',
                'eez_observer_funding_runway_below_day{account="poster",chain="l1"}': '0',
                'reth_eez_poster_funding_estimate_valid': '1', 'reth_eez_poster_funding_balance_valid': '1',
                'reth_eez_poster_funding_estimate_timestamp_seconds': '$NOW', 'reth_eez_poster_can_fund_candidate': '1'})
            baseline.update({'eez_observer_required_blob_evaluation_valid':'1', 'eez_observer_required_blob_blocked':'0',
                'eez_observer_required_blob_retries':'0', 'eez_observer_required_blob_last_attempt_timestamp_seconds':'$NOW'})
            for chain in ('l1', 'l2'):
                for metric, value in [('index_collection_success', '1'), ('lag_blocks', '0'), ('index_open_gaps', '0')]:
                    baseline[f'eez_observer_{metric}{{chain="{chain}"}}'] = value
                baseline[f'eez_observer_source_receipt_collection_success{{chain="{chain}"}}'] = '1'
                for category in ('missing_receipts', 'accounting_gaps', 'lookup_errors', 'not_observed', 'outside_interval', 'unavailable_transactions', 'unindexed_blocks'):
                    baseline[f'eez_observer_source_receipt_{category}{{chain="{chain}"}}'] = '0'
            for category in ('gap_intervals', 'quarantined_records', 'lost_segments', 'stale_partial_segments', 'unreadable_segments', 'unknown_trailing_epochs'):
                baseline[f'eez_observer_lifecycle_{category}{{source_service="composer",source_instance="node"}}'] = '0'
            def set_metrics(updates=None, remove=(), mode='healthy'):
                values = {**baseline, **(updates or {})}
                for key in remove:
                    values.pop(key, None)
                request(fixture + '/control', {'mode': mode, 'metric_lines': [key + ' ' + value for key, value in values.items()]})
            def events():
                return request(fixture + '/events')
            def delivered(title, status, start):
                return any(event['path'] == '/alerts' and any(alert.get('labels', {}).get('alertname') == title
                    and alert.get('status') == status and alert['labels'].get('owner') == 'fixture-operator'
                    for alert in event['data'].get('alerts', [])) for event in events()[start:])
            def states():
                return request(grafana + '/api/prometheus/grafana/api/v1/alerts', password=password)['data']['alerts']
            def firing(title):
                return any(alert['labels'].get('alertname') == title and alert['state'] == 'firing' for alert in states())
            set_metrics()
            until('canonical source samples ingested', lambda: query('eez_observer_index_collection_success{chain="l1"}')[0]['value'][1] == '1')
            until('fresh queue rule and advancing L1 available', lambda: query('delta(eez:chain_head_number:current{source="l1"}[2m]) > 0'))
            rules = request(grafana + '/api/v1/provisioning/alert-rules', password=password)
            assert any(rule['uid'] == 'eez-progress-critical' for rule in rules)
            report['checks'].append('actual_generated_rules_loaded_with_explicit_owner_and_error_policy')

            start = len(events())
            set_metrics({'eez_observer_last_canonical_terminal_timestamp_seconds{direction="inbound"}': str(time.time() - 600)})
            until('canonical progress warning actually delivers', lambda: delivered('EEZCanonicalProgressStalledWarning', 'firing', start))
            until('canonical progress page actually delivers', lambda: delivered('EEZCanonicalProgressStalledCritical', 'firing', start))
            set_metrics()
            until('canonical progress recovery actually delivers', lambda: delivered('EEZCanonicalProgressStalledCritical', 'resolved', start))
            report['checks'].append('canonical_progress_warning_page_and_recovery_delivery')

            start = len(events())
            set_metrics(remove=['eez_observer_last_canonical_terminal_timestamp_seconds{direction="inbound"}'])
            until('missing progress history is explicit unknown', lambda: delivered('EEZCanonicalProgressUnknown', 'firing', start))
            assert not firing('EEZCanonicalProgressStalledCritical')
            set_metrics({'eez_observer_index_collection_success{chain="l1"}': '0', 'eez_observer_canonical_outcome_evaluation_valid': '0',
                'eez_observer_last_canonical_terminal_timestamp_seconds{direction="inbound"}': str(time.time() - 600)})
            until('failed index parent alerts', lambda: delivered('EEZCanonicalIndexUnavailable_l1', 'firing', start))
            assert not firing('EEZCanonicalProgressStalledCritical')
            assert not firing('EEZCanonicalOutcomeEvidenceUnavailable')
            set_metrics()
            until('index parent recovers', lambda: delivered('EEZCanonicalIndexUnavailable_l1', 'resolved', start))
            report['checks'].append('missing_history_unknown_and_confirmed_upstream_failure_suppression')

            start = len(events())
            set_metrics({'eez_observer_lag_blocks{chain="l1"}': '65', 'eez_observer_index_open_gaps{chain="l2"}': '1',
                'eez_observer_blob_posts{state="unavailable"}': '1',
                'eez_observer_lifecycle_gap_intervals{source_service="composer",source_instance="node"}': '1'})
            titles = ['EEZCanonicalIndexLag_l1', 'EEZCanonicalIndexGaps_l2', 'EEZBlobMeasurementCoverageGap', 'EEZLifecycleCoverage_gap_intervals_composer_node']
            for title in titles:
                until(title + ' delivery', lambda title=title: delivered(title, 'firing', start))
            set_metrics()
            for title in titles:
                until(title + ' recovery', lambda title=title: delivered(title, 'resolved', start))
            report['checks'].append('configured_lag_receipt_lifecycle_and_historical_blob_coverage_notifications_recover')

            start = len(events())
            for value in ('10', '20', '30'):
                set_metrics({'eez_observer_follower_state_root_agreement': '0', 'reth_eez_settlement_anchor_age_seconds{rollup="1"}': '600',
                    'eez_observer_source_receipt_missing_receipts{chain="l1"}': '1', 'eez_signer_requests_total{outcome="invalid"}': value})
                time.sleep(10)
            titles = ['EEZFollowerStateRootMismatch', 'EEZPureL2PolicyOverdueWarning_node', 'EEZPureL2PolicyOverdueCritical_node',
                      'EEZSignerRepeatedRejections_signer', 'EEZSourceReceiptCoverageGap_l1']
            for title in titles:
                until(title + ' delivery', lambda title=title: delivered(title, 'firing', start))
            baseline['eez_signer_requests_total{outcome="invalid"}'] = '30'
            set_metrics()
            for title in titles:
                until(title + ' recovery', lambda title=title: delivered(title, 'resolved', start))
            report['checks'].append('follower_exact_mismatch_policy_age_signer_rejections_and_source_receipt_gaps_delivery_recovery')

            start = len(events())
            for value in ('10','20','30'):
                set_metrics({'eez_signer_authorized_requests_total{outcome="internal"}':value})
                time.sleep(10)
            authorized_title = 'EEZSignerAuthorizedAttestationFailures_signer'
            until('fully authorized attestation failure page delivered', lambda: delivered(authorized_title,'firing',start))
            baseline['eez_signer_authorized_requests_total{outcome="internal"}'] = '30'
            baseline['eez_signer_authorized_requests_total{outcome="success"}'] = '1'
            set_metrics()
            until('successful authorized response resolves page', lambda: delivered(authorized_title,'resolved',start))
            report['checks'].append('fully_authorized_attestation_failures_page_and_success_recovers_without_counter_reset')

            start = len(events())
            set_metrics({'eez_observer_funding_runway_below_day{account="poster",chain="l1"}': '1', 'reth_eez_poster_can_fund_candidate': '0'})
            titles = ['EEZFundingRunwayBelowDay_poster', 'EEZActualPosterCannotFundCandidate_node']
            for title in titles:
                until(title + ' delivery', lambda title=title: delivered(title, 'firing', start))
            set_metrics({'eez_observer_funding_burn_valid{account="poster",chain="l1"}': '0',
                'eez_observer_funding_burn_unknown{account="poster",chain="l1",reason="zero_burn"}': '1',
                'reth_eez_poster_funding_balance_valid': '0'}, remove=['eez_observer_funding_runway_below_day{account="poster",chain="l1"}'])
            for title in titles:
                until(title + ' withdraws on unknown evidence', lambda title=title: delivered(title, 'resolved', start))
            until('unknown candidate comparison actually warns', lambda: delivered('EEZPosterCandidateFundingUnknown_node', 'firing', start))
            assert not firing('EEZFundingRunwayBelowDay_poster')
            set_metrics()
            until('candidate comparison recovery delivers', lambda: delivered('EEZPosterCandidateFundingUnknown_node', 'resolved', start))
            report['checks'].append('separate_observer_runway_and_actual_poster_pages_withdraw_on_unknown_and_recover')

            start = len(events())
            set_metrics({'eez_observer_required_blob_blocked':'1','eez_observer_required_blob_retries':'3'})
            until('confirmed current required blob actually pages', lambda: delivered('EEZRequiredBlobBlocksDerivation','firing',start))
            set_metrics({'eez_observer_required_blob_evaluation_valid':'0'}, remove=['eez_observer_required_blob_blocked'])
            until('retracted dependency resolves required-blob page', lambda: delivered('EEZRequiredBlobBlocksDerivation','resolved',start))
            until('unknown required-blob evidence actually warns', lambda: delivered('EEZRequiredBlobEvidenceUnknown','firing',start))
            set_metrics()
            until('required-blob evidence recovery delivers', lambda: delivered('EEZRequiredBlobEvidenceUnknown','resolved',start))
            report['checks'].append('typed_required_current_blob_retry_page_retracts_with_unknown_and_recovers')

            start = len(events())
            # Two successive increments keep the shortened increase window
            # populated through Grafana's hold/evaluation scheduling.
            for value in ('10', '20', '30'):
                set_metrics({'reth_eez_telemetry_dropped_events_total{signal="trace",reason="queue_full"}': value,
                    'reth_eez_telemetry_dropped_events_total{signal="lifecycle",reason="queue_full"}': value})
                time.sleep(10)
            for title in ['EEZNativeTraceLoss_eez-composer-node', 'EEZLifecycleLoss_eez-composer-node']:
                until(title + ' delivery', lambda title=title: delivered(title, 'firing', start))
            # Preserve cumulative counters on recovery, rather than clearing
            # loss history to make the alert green.
            set_metrics({'reth_eez_telemetry_dropped_events_total{signal="trace",reason="queue_full"}': '30',
                'reth_eez_telemetry_dropped_events_total{signal="lifecycle",reason="queue_full"}': '30'})
            for title in ['EEZNativeTraceLoss_eez-composer-node', 'EEZLifecycleLoss_eez-composer-node']:
                until(title + ' recovery', lambda title=title: delivered(title, 'resolved', start))
            report['checks'].append('trace_and_lifecycle_loss_notifications_recover_without_resetting_counters')
            report['status'] = 'passed'
        except Exception:
            args.report.with_suffix('.logs.txt').write_text(run(compose + ['logs', '--no-color', '--tail', '60'], check=False))
            raise
        finally:
            run(compose + ['down', '--volumes', '--remove-orphans'], check=False)
            assert not run(['docker', 'ps', '-aq', '--filter', 'label=com.docker.compose.project=' + project]).strip()
            assert not run(['docker', 'volume', 'ls', '-q', '--filter', 'label=com.docker.compose.project=' + project]).strip()
            report['owned_resource_cleanup'] = 'verified'
            args.report.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
