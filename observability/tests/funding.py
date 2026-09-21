"""Actual optional funding worker on the shared private database/RPC fixture."""
import copy
import json
import time


def exercise(run, scalar, index_config, control, name, rpc_name, report):
    config = copy.deepcopy(index_config)
    config['deployment'] = 'funding'
    config['exporter'].update(listen='0.0.0.0:19123', allow_non_loopback=True)
    config['funding'] = {'burn_window_seconds': 3600, 'accounts': [
        {'role': 'poster', 'chain': 'l1', 'address': '0x' + 'aa' * 20},
        {'role': 'system', 'chain': 'l2', 'address': '0x' + 'bb' * 20}]}
    control(phase='healthy', finalize=False, post_count=1, post_success=False, protocol_logs='missing',
            multi_rollup=False, source_unselected=False, source_receipt_missing=False, source_cost_missing=False,
            source_hash_mismatch=False, source_receipt_failure=False, source_receipt_delay_ms=0,
            blob_mode='disabled', correlation='none', funding_base_time=int(time.time()),
            funding_balance=591216, funding_balance_error=False)
    def start():
        run(['docker','exec','-i',name,'sh','-c','umask 077; cat > /tmp/funding-indexer.json'], data=json.dumps(config))
        run(['docker','exec','-d',name,'sh','-c','echo $$ > /tmp/funding-indexer.pid; exec /observer indexer --config /tmp/funding-indexer.json > /tmp/funding-indexer.log 2>&1'])
    def stop():
        run(['docker','exec',name,'sh','-c','kill -TERM "$(cat /tmp/funding-indexer.pid)"'],check=False)
        deadline = time.monotonic() + 15
        while run(['docker','exec',name,'sh','-c','kill -0 "$(cat /tmp/funding-indexer.pid)"'],check=False).returncode == 0:
            if time.monotonic() >= deadline:
                raise AssertionError('funding fixture observer did not stop')
            time.sleep(0.2)
    start()
    poster = 'account="poster",chain="l1"'
    system = 'account="system",chain="l2"'
    def metrics():
        text = run(['docker','exec',rpc_name,'python3','-c',
            'import urllib.request; print(urllib.request.urlopen(' + repr('http://' + name + ':19123/metrics') + ',timeout=5).read().decode())']).stdout
        return {key: float(value) for line in text.splitlines() if line and not line.startswith('#')
                for key, value in [line.rsplit(' ',1)]}
    def metric(values, suffix, labels=poster):
        return values.get('eez_observer_funding_' + suffix + '{' + labels + '}')
    def until(label, predicate):
        deadline = time.monotonic() + 90
        last = None
        while time.monotonic() < deadline:
            try:
                last = metrics()
                if predicate(last):
                    print('PASS: funding ' + label, flush=True)
                    return last
            except (RuntimeError, ValueError):
                pass
            time.sleep(1)
        # The log has only bounded reasons; config/credentials never enter it.
        diagnostic = run(['docker','exec',name,'tail','-25','/tmp/funding-indexer.log'],check=False).stdout
        raise AssertionError(label + ': funding deadline; metrics=' + repr(last) + '; log=' + diagnostic)
    try:
        values = until('fresh covered positive burn and independent zero-burn account', lambda values:
            metric(values,'source_valid') == 1 and metric(values,'burn_valid') == 1
            and metric(values,'runway_below_day') == 1 and metric(values,'burn_valid',system) == 0)
        assert abs(metric(values,'runway_seconds') - 3600) < 0.001
        assert metric(values,'source_valid',system) == 1
        assert metric(values,'runway_seconds',system) is None
        assert 'eez_observer_funding_burn_unknown{account="system",chain="l2",reason="zero_burn"}' in values
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='funding' AND role='l1';") == '738216'
        cursor_sql = "SELECT b.number::text FROM eez.scan_cursors s JOIN eez.chain_instances c ON c.id=s.chain_instance_id JOIN eez.deployments d ON d.id=c.deployment_id JOIN eez.blocks b ON b.chain_instance_id=c.id AND b.hash=s.block_hash WHERE d.name='funding' AND c.role='l1';"
        previous_height = int(scalar(cursor_sql))
        until('ordinary cursor append preserves the verified historical burn window', lambda values:
            metric(values,'burn_valid') == 1 and int(scalar(cursor_sql)) > previous_height)
        assert scalar("SELECT canonical_revision FROM eez.chain_instances c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='funding' AND c.role='l1';") == '0'
        report['checks'].append('funding_historical_window_stays_valid_across_actual_append_only_cursor_progress')
        control(funding_balance=591216 * 24)
        until('exact24h boundary is not below threshold', lambda values: metric(values,'runway_below_day') == 0 and abs(metric(values,'runway_seconds') - 86400) < 0.001)
        control(phase='fork')
        until('canonical reorg recomputes burn and retracts previous runway', lambda values: metric(values,'runway_below_day') == 1 and metric(values,'burn_valid') == 1)
        assert scalar("SELECT known_total_paid FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='funding' AND role='l1';") == '804216'
        control(funding_balance_error=True)
        until('RPC failure removes balance/runway instead of zero fill', lambda values: metric(values,'source_valid') == 0 and metric(values,'balance_native_units') is None and metric(values,'runway_seconds') is None)
        control(funding_balance_error=False, funding_balance=0)
        until('recovery accepts actual zero balance and low runway', lambda values: metric(values,'source_valid') == 1 and metric(values,'balance_native_units') == 0 and metric(values,'runway_below_day') == 1)
        # Populate another namespace from actually incomplete RPC costs. Never
        # mutate previously complete immutable receipt facts to manufacture loss.
        stop()
        config['deployment'] = 'funding-incomplete'
        control(source_cost_missing=True)
        start()
        until('incomplete costs make burn unknown while balance remains known', lambda values: metric(values,'source_valid') == 1 and metric(values,'burn_valid') == 0 and metric(values,'runway_seconds') is None
              and 'eez_observer_funding_burn_unknown{account="poster",chain="l1",reason="incomplete_history"}' in values)
        assert scalar("SELECT incomplete_receipts FROM eez.canonical_cost_totals c JOIN eez.deployments d ON d.id=c.deployment_id WHERE d.name='funding-incomplete' AND role='l1';") == '1'
        report['checks'].append('funding_explicit_accounts_hash_pinned_balances_covered_positive_burn_exact_day_boundary_reorg_failure_zero_balance_and_cost_gap_unknown')
    finally:
        stop()
        control(funding_base_time=None, funding_balance_error=False, source_cost_missing=False)
