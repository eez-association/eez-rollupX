"""Fail-closed assertions shared by the live Chiado validation harness.

These checks do not send transactions, interpret queue state as settlement, or
replace independent replay. Callers authenticate the L1 block on both providers.
"""

from decimal import Decimal
import subprocess

if not __debug__:
    raise RuntimeError('Validation requires Python assertions; do not use optimization')


def cast_with_key(arguments, key):
    """Use Foundry's key interface without persisting argv/stderr on failure."""
    try:
        return subprocess.check_output(['cast', *arguments, '--private-key', key],
                                       text=True, stderr=subprocess.PIPE).strip()
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f'cast {arguments[0]} failed with exit {error.returncode}') from None
    except OSError:
        raise RuntimeError(f'cast {arguments[0]} could not start') from None

LOG_FIELDS = (
    'address', 'topics', 'data', 'blockHash', 'blockNumber',
    'transactionHash', 'transactionIndex', 'logIndex',
)


def check_funding(amount, waves=30):
    """Reject actor funding that cannot cover even the increasing deposits.

    This is a value-transfer floor, not a gas-price estimate. Operators still
    need a fee margin and sufficient source funds for setup and ordinary load.
    """
    value = Decimal(amount)
    assert value.is_finite() and value > 0, 'Invalid actor funding'
    wei = value * 10**18
    assert wei == wei.to_integral_value(), 'Funding exceeds wei precision'
    required = waves * (waves + 1) // 2 * 10**13
    assert wei > required, 'Actor funding does not cover wave deposits plus any gas'
    return int(wei)


def check_runtime(before, after):
    """Require the recorded container instances to stay running and unchanged."""
    assert len(before) == len(after) and before, 'Missing runtime identity'
    for expected, actual in zip(before, after):
        assert actual['State']['Running'], 'Runtime stopped'
        assert expected['name'] == actual['Name'], 'Wrong runtime container'
        assert expected['id'] == actual['Id'] and expected['image'] == actual['Image'], 'Runtime replaced'
        assert expected['started'] == actual['State']['StartedAt'], 'Runtime restarted'
        assert expected['restarts'] == actual['RestartCount'], 'Runtime restart count changed'


def check_snapshot(rpc, url, snapshot):
    """Detect reorgs during evidence reads; this is not a finality assertion."""
    current = rpc(url, 'eth_getBlockByNumber', [snapshot['number'], False])
    assert current and current['hash'] == snapshot['hash'], 'Canonical snapshot changed'


def check_queue(queue, *, idle):
    """Require an empty queue only before starting traffic or after draining."""
    counts = [queue[key] for key in ('queued', 'inFlight', 'blocked', 'unknown')]
    assert all(type(value) is int and value >= 0 for value in counts), 'Invalid queue counters'
    if idle:
        assert not any(counts), 'Queue is not idle'


def check_completion(correlation, posting, independent, local, completion, registry, topics):
    """Bind the exact correlated endpoint to the full canonical L1 block's logs.

    postAndVerifyBatch may execute only a prefix; deferred inbound companion
    transactions finish in the same block. A successful posting receipt or an
    earlier matching event cannot prove the final correlation endpoint.
    """
    assert correlation['canonicalL2'], 'Noncanonical correlation'
    assert correlation['l1TransactionHash'] == posting['transactionHash'], 'Posting identity mismatch'
    assert posting['status'] == '0x1', 'Unsuccessful posting'
    assert independent, 'Missing execution events'
    project = lambda logs: [[entry[key] for key in LOG_FIELDS] for entry in logs]
    assert project(independent) == project(local), 'Independent event disagreement'
    assert all(not entry.get('removed', False) for entry in local), 'Removed local execution event'
    order = []
    for entry in independent:
        assert not entry.get('removed', False), 'Removed execution event'
        assert entry['address'].lower() == registry.lower() and entry['topics'] == topics, 'Wrong event scope'
        assert entry['blockHash'] == posting['blockHash'] and entry['blockNumber'] == posting['blockNumber'], 'Wrong event block'
        assert len(entry['data']) == 130 and int(entry['data'][2:], 16) >= 0, 'Malformed execution event'
        order.append((int(entry['transactionIndex'], 16), int(entry['logIndex'], 16)))
    assert order == sorted(set(order)), 'Unordered or duplicate execution events'
    final = independent[-1]
    assert final['data'][:66] == correlation['l2Range']['lastBlockHash'], 'Incomplete or wrong commitment'
    assert int(final['transactionIndex'], 16) >= int(posting['transactionIndex'], 16), 'Completion precedes posting'
    assert completion and completion['status'] == '0x1', 'Unsuccessful completion'
    assert all(completion[key] == final[key] for key in (
        'transactionHash', 'transactionIndex', 'blockHash', 'blockNumber')), 'Completion identity mismatch'
    assert any(all(entry[key] == final[key] for key in LOG_FIELDS)
               and not entry.get('removed', False) for entry in completion['logs']), 'Completion event missing'
