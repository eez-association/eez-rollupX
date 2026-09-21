#!/usr/bin/env python3
"""Deterministic tests of the exact provisioned PromQL, using pinned promtool."""
import argparse
import importlib.util
import json
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'prepare.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


def fixture_config():
    return {'environment': 'devnet', 'deployment': 'alerts', 'observer_instance': 'observer',
            'database_enabled': True, 'protocol_alerts': {'owner': 'fixture-operator', 'blob_collection': True,
                'follower_comparison': True, 'pure_l2_inclusion_grace_seconds': 60,
                'poster_funding_estimates': True, 'funding_accounts': [{'account': 'poster', 'chain': 'l1'}],
                'required_blob_derivation': True,
                'lifecycle_sources': [{'service': 'composer', 'instance': 'node'}]},
            'native_processes': [{'service': service, 'instance': 'node', 'metrics_target': target,
                'metrics_listen': '127.0.0.1:19101'} for service, target in [('eez-composer', 'composer:19101'), ('eez-proof-signer', 'signer:19101')]]}


def cases(config):
    scope = 'environment="devnet",deployment="alerts",job="eez-observer",instance="observer"'
    lifecycle = 'source_service="composer",source_instance="node"'
    def name(metric, labels=''):
        return metric + '{' + scope + (',' + labels if labels else '') + '}'
    def constant(value):
        return str(value) + '+0x40'
    def native_name(metric, service='eez-composer', labels=''):
        return metric + '{environment="devnet",deployment="alerts",job="' + service + '-node",instance="node"' + (',' + labels if labels else '') + '}'
    base = {name('up'): constant(1), name('eez_observer_canonical_outcome_evaluation_valid'): constant(1),
            name('eez_observer_last_canonical_terminal_timestamp_seconds', 'direction="inbound"'): '0+15x40',
            name('eez:queue_transactions:current', 'state="queued_ready"'): constant(2),
            name('eez:chain_head_number:current', 'source="l1"'): '100+1x40',
            name('eez_observer_lifecycle_collection_success', lifecycle): constant(1),
            name('eez_observer_blob_collection_success'): constant(1),
            name('eez_observer_blob_posts', 'state="measured"'): constant(2),
            name('eez_observer_blob_posts', 'state="unavailable"'): constant(0)}
    base.update({name('eez_observer_follower_comparison_known'): constant(1), name('eez_observer_follower_state_root_agreement'): constant(1),
        name('eez_observer_collection_success', 'source="l1"'): constant(1), name('eez_observer_collection_success', 'source="l2"'): constant(1),
        native_name('up'): constant(1), native_name('up', 'eez-proof-signer'): constant(1),
        native_name('reth_eez_settlement_policy_enabled'): constant(1),
        native_name('reth_eez_settlement_policy_setting', labels='setting="pure_l2_interval_seconds"'): constant(120),
        native_name('reth_eez_settlement_policy_evaluation_valid', labels='rollup="1"'): constant(1),
        native_name('reth_eez_settlement_policy_evaluation_timestamp_seconds', labels='rollup="1"'): '0+15x40',
        native_name('reth_eez_settlement_anchor_age_seconds', labels='rollup="1"'): constant(60),
        native_name('reth_eez_settlement_pending_pure_l2_transactions', labels='rollup="1"'): constant(2)})
    base[native_name('reth_eez_settlement_due', labels='rollup="1"')] = constant(1)
    for outcome in ('success', 'invalid', 'rejected'):
        base[native_name('eez_signer_requests_total', 'eez-proof-signer', f'outcome="{outcome}"')] = constant(0)
    for outcome in ('success','internal','deadline','panicked','cancelled'):
        base[native_name('eez_signer_authorized_requests_total', 'eez-proof-signer', f'outcome="{outcome}"')] = constant(0)
    fund = 'account="poster",chain="l1"'
    for metric, value in [('source_valid',1),('burn_valid',1),('runway_below_day',0)]:
        base[name('eez_observer_funding_' + metric, fund)] = constant(value)
    for metric in ('estimate_valid','balance_valid'):
        base[native_name('reth_eez_poster_funding_' + metric)] = constant(1)
    base[native_name('reth_eez_poster_funding_estimate_timestamp_seconds')] = '0+15x40'
    base[native_name('reth_eez_poster_can_fund_candidate')] = constant(1)
    base[name('eez_observer_required_blob_evaluation_valid')] = constant(1)
    base[name('eez_observer_required_blob_blocked')] = constant(0)
    base[name('eez_observer_required_blob_retries')] = constant(0)
    base[name('eez_observer_required_blob_last_attempt_timestamp_seconds')] = '0+15x40'
    for chain in ('l1', 'l2'):
        for metric, value in [('eez_observer_index_collection_success', 1), ('eez_observer_lag_blocks', 0), ('eez_observer_index_open_gaps', 0)]:
            base[name(metric, f'chain="{chain}"')] = constant(value)
        base[name('eez_observer_source_receipt_collection_success', f'chain="{chain}"')] = constant(1)
        for category in ('missing_receipts', 'accounting_gaps', 'lookup_errors', 'not_observed', 'outside_interval', 'unavailable_transactions', 'unindexed_blocks'):
            base[name('eez_observer_source_receipt_' + category, f'chain="{chain}"')] = constant(0)
    for category in ('gap_intervals', 'quarantined_records', 'lost_segments', 'stale_partial_segments', 'unreadable_segments', 'unknown_trailing_epochs'):
        base[name('eez_observer_lifecycle_' + category, lifecycle)] = constant(0)
    rules = [rule for rule in prepare.alert_rules(config)[0]['rules'] if rule['annotations']['runbook'].startswith('observability/runbooks/protocol-alerts.md')]
    native_queue = next(rule for rule in prepare.alert_rules({**config, 'queue_source':'native'})[0]['rules'] if rule['uid'] == 'eez-queue-stale')
    rules.append(native_queue)
    base[native_name('eez:queue_evaluation_valid:current')] = constant(1)
    tests = []
    def case(title, updates=None, removed=(), firing=(), evaluation='10m'):
        data = {**base, **(updates or {})}
        for metric in removed:
            data.pop(metric, None)
        tests.append({'name': title, 'interval': '15s',
            'input_series': [{'series': metric, 'values': values} for metric, values in data.items()],
            'promql_expr_test': [{'expr': 'count((' + rule['data'][0]['model']['expr'] + ') > 0)',
                'eval_time': evaluation, 'exp_samples': [{'labels': '{}', 'value': 1}] if rule['uid'] in firing else []} for rule in rules]})
    progress = name('eez_observer_last_canonical_terminal_timestamp_seconds', 'direction="inbound"')
    stall = {progress: constant(100)}
    case('healthy_no_protocol_alerts')
    case('canonical_progress_warning_and_critical', stall, firing=['eez-progress-warning', 'eez-progress-critical'])
    case('canonical_progress_warning_only', {progress: constant(350)}, firing=['eez-progress-warning'])
    case('quiet_workload_does_not_stall', {**stall, name('eez:queue_transactions:current', 'state="queued_ready"'): constant(0)})
    case('stationary_l1_suppresses_dependent_progress', {**stall, name('eez:chain_head_number:current', 'source="l1"'): constant(100)})
    case('missing_progress_is_unknown_not_stalled', removed=[progress], firing=['eez-progress-unknown'])
    case('invalid_canonical_evidence_suppresses_stall', {**stall, name('eez_observer_canonical_outcome_evaluation_valid'): constant(0)}, firing=['eez-outcomes-unavailable'])
    case('failed_index_suppresses_outcome_and_progress', {**stall, name('eez_observer_index_collection_success', 'chain="l1"'): constant(0)}, firing=['eez-index-l1'])
    case('missing_index_signal_is_unknown', removed=[name('eez_observer_index_collection_success', 'chain="l2"')], firing=['eez-index-l2'])
    case('observer_outage_suppresses_all_dependent_protocol_alerts', {**stall, name('up'): constant(0), name('eez_observer_blob_collection_success'): constant(0)})
    case('configured_index_lag_and_gap_thresholds', {name('eez_observer_lag_blocks', 'chain="l1"'): constant(65), name('eez_observer_index_open_gaps', 'chain="l2"'): constant(1)}, firing=['eez-index-lag-l1', 'eez-index-gap-l2'])
    case('index_lag_boundary_does_not_warn', {name('eez_observer_lag_blocks', 'chain="l1"'): constant(64)})
    source_uid = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZLifecycleCollectionUnavailable'))
    case('missing_lifecycle_source_alerts', removed=[name('eez_observer_lifecycle_collection_success', lifecycle)], firing=[source_uid])
    gap_uid = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZLifecycleCoverage_gap_intervals'))
    case('lifecycle_gap_is_visible', {name('eez_observer_lifecycle_gap_intervals', lifecycle): constant(1)}, firing=[gap_uid])
    case('failed_lifecycle_suppresses_retained_coverage', {name('eez_observer_lifecycle_collection_success', lifecycle): constant(0), name('eez_observer_lifecycle_gap_intervals', lifecycle): constant(1)}, firing=[source_uid])
    case('historical_blob_coverage_warns_only', {name('eez_observer_blob_posts', 'state="unavailable"'): constant(1)}, firing=['eez-blob-coverage'])
    case('missing_blob_worker_alerts', removed=[name('eez_observer_blob_collection_success')], firing=['eez-blob-collection'])
    case('failed_blob_worker_suppresses_old_coverage', {name('eez_observer_blob_collection_success'): constant(0), name('eez_observer_blob_posts', 'state="unavailable"'): constant(1)}, firing=['eez-blob-collection'])
    case('progress_retraction_uses_recomputed_gauge', {progress: '0+15x20 100+0x20'}, firing=['eez-progress-warning', 'eez-progress-critical'])
    case('successful_progress_recovers', {progress: '100+0x20 315+15x20'})
    case('fresh_exact_follower_mismatch_pages', {name('eez_observer_follower_state_root_agreement'): constant(0)}, firing=['eez-follower-mismatch'])
    case('stale_follower_is_unknown_not_agreement', {name('eez_observer_follower_comparison_known'): constant(0)}, removed=[name('eez_observer_follower_state_root_agreement')], firing=['eez-follower-unknown'])
    case('missing_follower_capability_is_unknown', removed=[name('eez_observer_follower_comparison_known'), name('eez_observer_follower_state_root_agreement')], firing=['eez-follower-unknown'])
    signer_uid = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZSignerRepeatedRejections'))
    rejection = native_name('eez_signer_requests_total', 'eez-proof-signer', 'outcome="invalid"')
    case('signer_repeated_invalid_proposals_warn', {rejection: '0+1x40'}, firing=[signer_uid])
    case('signer_rejection_symptom_suppressed_by_l1_failure', {rejection: '0+1x40', name('eez_observer_collection_success', 'source="l1"'): constant(0)})
    case('signer_rejection_symptom_suppressed_by_missing_l2', {rejection: '0+1x40'}, removed=[name('eez_observer_collection_success', 'source="l2"')])
    authorized_uid = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZSignerAuthorizedAttestationFailures'))
    authorized_unknown = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZSignerAuthorizedOutcomesUnknown'))
    authorized = lambda outcome: native_name('eez_signer_authorized_requests_total','eez-proof-signer',f'outcome="{outcome}"')
    case('fully_authorized_internal_failures_page', {authorized('internal'):'0+1x40'}, firing=[authorized_uid])
    case('fully_authorized_deadline_failures_page', {authorized('deadline'):'0+1x40'}, firing=[authorized_uid])
    case('fully_authorized_panics_page', {authorized('panicked'):'0+1x40'}, firing=[authorized_uid])
    case('caller_cancelled_authorized_work_never_pages', {authorized('cancelled'):'0+1x40'})
    case('authorized_success_in_window_recovers_failure_page', {authorized('internal'):'0+1x40',authorized('success'):'0+0x30 1+0x9'})
    case('historical_success_outside_window_does_not_mask_new_failure', {authorized('internal'):'0+1x40',authorized('success'):'0 1+0x39'}, firing=[authorized_uid])
    case('historical_failures_expire_without_counter_reset', {authorized('internal'):'0+1x15 15+0x25'})
    case('authorized_failures_suppressed_by_signer_down', {authorized('internal'):'0+1x40',native_name('up','eez-proof-signer'):constant(0)})
    case('authorized_failures_suppressed_by_upstream_loss', {authorized('internal'):'0+1x40',name('eez_observer_collection_success','source="l1"'):constant(0)})
    case('unknown_authorized_success_never_manufactures_zero', {authorized('internal'):'0+1x40'}, removed=[authorized('success')], firing=[authorized_unknown])
    policy_warning = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZPureL2PolicyOverdueWarning'))
    policy_critical = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZPureL2PolicyOverdueCritical'))
    policy_unknown = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZPureL2PolicyEvaluationUnknown'))
    age = native_name('reth_eez_settlement_anchor_age_seconds', labels='rollup="1"')
    stamp = native_name('reth_eez_settlement_policy_evaluation_timestamp_seconds', labels='rollup="1"')
    case('policy_due_condition_above_interval', {age: constant(181)}, firing=[policy_warning,policy_critical])
    case('policy_interval_boundary_not_due', {age: constant(120)})
    case('policy_due_condition_independent_of_long_anchor_age', {age: constant(481)}, firing=[policy_warning, policy_critical])
    case('inactive_policy_does_not_page', {age: constant(1000), native_name('reth_eez_settlement_policy_enabled'): constant(0)})
    case('no_pending_pure_l2_work_does_not_page', {age: constant(1000), native_name('reth_eez_settlement_pending_pure_l2_transactions', labels='rollup="1"'): constant(0)})
    case('native_policy_not_due_suppresses_age_condition', {age: constant(1000), native_name('reth_eez_settlement_due', labels='rollup="1"'): constant(0)})
    case('stale_policy_evaluation_unknown_not_overdue', {age: constant(1000), stamp: constant(100)}, firing=[policy_unknown])
    case('missing_policy_timestamp_unknown_not_overdue', {age: constant(1000)}, removed=[stamp], firing=[policy_unknown])
    case('source_receipt_gaps_outside_wallet_selection_warn', {name('eez_observer_source_receipt_missing_receipts', 'chain="l1"'): constant(1)}, firing=['eez-source-receipt-gaps-l1'])
    case('source_receipt_collection_error_suppresses_old_coverage', {name('eez_observer_source_receipt_collection_success', 'chain="l1"'): constant(0), name('eez_observer_source_receipt_accounting_gaps', 'chain="l1"'): constant(2)}, firing=['eez-source-receipts-l1'])
    case('covered_positive_burn_below_day_warns', {name('eez_observer_funding_runway_below_day', fund): constant(1)}, firing=['eez-funding-runway-poster'])
    case('unknown_or_zero_burn_not_low_runway', {name('eez_observer_funding_burn_valid', fund): constant(0), name('eez_observer_funding_burn_unknown', fund + ',reason="zero_burn"'): constant(1)}, removed=[name('eez_observer_funding_runway_below_day', fund)])
    case('incomplete_burn_coverage_warns_unknown', {name('eez_observer_funding_burn_valid', fund): constant(0), name('eez_observer_funding_burn_unknown', fund + ',reason="incomplete_history"'): constant(1)}, removed=[name('eez_observer_funding_runway_below_day', fund)], firing=['eez-funding-coverage-poster'])
    case('missing_balance_source_is_not_zero', removed=[name('eez_observer_funding_source_valid', fund)], firing=['eez-funding-source-poster'])
    actual_poster = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZActualPosterCannotFundCandidate'))
    unknown_estimate = next(rule['uid'] for rule in rules if rule['title'].startswith('EEZPosterCandidateFundingUnknown'))
    affordability = native_name('reth_eez_poster_can_fund_candidate')
    case('actual_poster_exact_insufficient_balance_pages', {affordability:constant(0)}, firing=[actual_poster])
    case('expired_candidate_does_not_page', {affordability:constant(0),native_name('reth_eez_poster_funding_estimate_timestamp_seconds'):constant(100)}, firing=[unknown_estimate])
    case('missing_balance_with_candidate_is_unknown', {affordability:constant(0),native_name('reth_eez_poster_funding_balance_valid'):constant(0)}, firing=[unknown_estimate])
    case('missing_comparison_with_claimed_validity_is_unknown', removed=[affordability], firing=[unknown_estimate])
    case('nan_comparison_is_unknown_not_affordable', {affordability:'NaN+0x40'}, firing=[unknown_estimate])
    case('quiet_expired_candidate_is_unexercised', {native_name('reth_eez_poster_funding_estimate_timestamp_seconds'):constant(100),name('eez:queue_transactions:current','state="queued_ready"'):constant(0)})
    case('poster_affordability_is_not_inferred_from_observer_account', {name('eez_observer_funding_source_valid',fund):constant(0) }, firing=['eez-funding-source-poster'])
    blocked = {name('eez_observer_required_blob_blocked'):constant(1),name('eez_observer_required_blob_retries'):constant(3)}
    case('confirmed_required_blob_after_retries_pages', blocked, firing=['eez-blob-required-blocked'])
    case('required_blob_retry_threshold_boundary', {**blocked,name('eez_observer_required_blob_retries'):constant(2)})
    case('old_current_blob_failure_is_not_a_page', {**blocked,name('eez_observer_required_blob_last_attempt_timestamp_seconds'):constant(100)}, firing=['eez-blob-required-unknown'])
    case('missing_current_blob_capability_unknown_not_historical_substitution', removed=[name('eez_observer_required_blob_evaluation_valid'),name('eez_observer_required_blob_blocked')], firing=['eez-blob-required-unknown'])
    case('reorg_retracts_current_blob_failure', {**blocked,name('eez_observer_required_blob_evaluation_valid'):constant(0)}, removed=[name('eez_observer_required_blob_blocked')], firing=['eez-blob-required-unknown'])
    case('native_queue_invalid_does_not_fall_back_to_healthy_rpc', {native_name('eez:queue_evaluation_valid:current'):constant(0),name('eez_observer_queue_evaluation_valid'):constant(1)}, firing=['eez-queue-stale'])
    case('native_queue_missing_sample_is_unknown', removed=[native_name('eez:queue_evaluation_valid:current')], firing=['eez-queue-stale'])
    case('native_queue_target_down_suppresses_evaluation_symptom', {native_name('up'):constant(0)}, removed=[native_name('eez:queue_evaluation_valid:current')])
    # Temporal acceptance uses the actual provisioned Grafana hold durations,
    # not the much older canonical anchor as an age for newly arrived work.
    policy_rules = [rule for rule in rules if rule['uid'] in (policy_warning,policy_critical)]
    def temporal(title, pending, times, invalid=None):
        data = {key: value.replace('x40','x80') for key,value in base.items()}
        data[age] = '36000+0x80'
        data[native_name('reth_eez_settlement_pending_pure_l2_transactions',labels='rollup="1"')] = pending
        if invalid is not None:
            data[native_name('reth_eez_settlement_policy_evaluation_valid',labels='rollup="1"')] = invalid
        tests.append({'name':title,'interval':'15s','input_series':[{'series':key,'values':value} for key,value in data.items()],
            'alert_rule_test':[{'eval_time':at,'alertname':rule['title'],
                'exp_alerts':[{'exp_labels':{**rule['labels'],'job':'eez-composer-node','instance':'node','rollup':'1'}}] if rule['labels']['severity'] in firing else []}
                for at,firing in times for rule in policy_rules]})
    temporal('long_idle_new_work_settles_within_grace_never_alerts','0+0x19 2+0x3 0+0x56', [('5m',()),('5m45s',()),('6m',()),('7m',())])
    temporal('sustained_due_work_honors_warning_and_critical_holds','0+0x19 2+0x25 0+0x35',
             [('5m45s',()),('6m',('warning',)),('10m45s',('warning',)),('11m',('warning','critical')),('11m30s',())])
    temporal('evaluation_loss_resets_policy_grace','0+0x19 2+0x61', [('5m15s',()),('7m',()),('7m30s',('warning',))],
             invalid='1+0x21 0+0x3 1+0x55')
    return rules, tests


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    rules, tests = cases(fixture_config())
    images = json.loads((ROOT / 'images.lock.json').read_text())
    image = next(name + '@' + digest for name, digest in images.items() if name.startswith('prom/prometheus:'))
    with tempfile.TemporaryDirectory(prefix='eez-protocol-rules-') as temporary:
        directory = pathlib.Path(temporary)
        directory.chmod(0o755)
        generated = [{'record': 'fixture:' + rule['uid'].replace('-', '_'), 'expr': rule['data'][0]['model']['expr']} for rule in rules]
        generated += [{'alert':rule['title'],'expr':'(' + rule['data'][0]['model']['expr'] + ') > 0','for':rule['for'],'labels':rule['labels']}
                      for rule in rules if rule['title'].startswith('EEZPureL2PolicyOverdue')]
        (directory / 'rules.json').write_text(json.dumps({'groups': [{'name': 'actual-provisioned-expressions', 'rules': generated}]}))
        (directory / 'tests.json').write_text(json.dumps({'rule_files': ['rules.json'], 'evaluation_interval': '15s', 'tests': tests}))
        subprocess.run(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', '/bin/promtool',
                        '-v', str(directory) + ':/work:ro', '-w', '/work', image, 'test', 'rules', 'tests.json'], check=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps({'status': 'passed', 'image': image, 'cases': [test['name'] for test in tests],
        'expression_assertions': sum(len(test.get('promql_expr_test',[])) for test in tests),
        'alert_state_assertions':sum(len(test.get('alert_rule_test',[])) for test in tests),
        'scope': 'exact provisioned PromQL and policy hold timers; synthetic deterministic series'}, indent=2) + '\n')


if __name__ == '__main__':
    main()
