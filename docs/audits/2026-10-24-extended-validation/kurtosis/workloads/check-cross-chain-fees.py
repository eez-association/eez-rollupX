"""Read-only fee checks over retained wave quotes and canonical receipts.

Uses the largest measured L1 payer and net-refund arithmetic of the existing
verify-l1-fees.mjs oracle. No quote requests, broadcasts, or profitability claim.
"""
import hashlib
import json
import os
from pathlib import Path
import urllib.request
import yaml

root = Path.cwd()
config = json.loads(Path(os.environ['EEZ_LOAD_CONFIG']).read_text())
assert config['enclave'] == os.environ['KURTOSIS_ENCLAVE']
evidence_path = Path(os.environ['EEZ_REPLAY_EVIDENCE'])
evidence = json.loads(evidence_path.read_text())
assert evidence['verified'] and evidence['runtime_source'] == config['runtime_source']
out = Path(os.environ['EEZ_CROSS_FEE_OUTPUT'])
assert not out.exists()
args = yaml.safe_load(Path(os.environ['KURTOSIS_ARGS_FILE']).read_text())
terms = args['eez']['settlement_env']
assert terms['EEZ_FEES_L1_TO_L2_ENABLED'] == terms['EEZ_FEES_L2_TO_L1_ENABLED'] == 'true'
percent = int(terms['EEZ_FEES_L1_REFUND_PERCENT'])
assert 0 < percent < 100
recipient = terms['EEZ_FEES_L1_REFUND_RECIPIENT'].lower()
registry = os.environ['EEZ_REGISTRY_ADDRESS'].lower()
quote_dir = Path(os.environ['EEZ_MIXED_OUTPUT']) / 'wave/fee-quotes'
quotes = {}
for path in quote_dir.glob('*.json'):
    quote = json.loads(path.read_text())
    assert quote['signedHash'] not in quotes
    quotes[quote['signedHash']] = (path, quote)
assert len(quotes) == len(evidence['wave_sources']) == 96
record = {'verified': False, 'runtime_source': config['runtime_source'],
    'enclave': config['enclave'], 'evidence_sha256': hashlib.sha256(evidence_path.read_bytes()).hexdigest(),
    'reference': {'path': 'testing/kurtosis/scripts/verify-l1-fees.mjs',
        'sha256': hashlib.sha256((root / 'testing/kurtosis/scripts/verify-l1-fees.mjs').read_bytes()).hexdigest(),
        'formula': 'floor(largest(gasUsed * (effectiveGasPrice - baseFee)) * refundPercent / 100) - payoutGasUsed * payoutEffectiveGasPrice'},
    'refund_percent': percent, 'refund_recipient': recipient, 'quotes': [], 'l1_posts': [],
    'broadcasts': 0, 'limits': ['Quotes are advisory; gas-used equality is measured, not assumed.',
        'Per-batch cost recovery/profit is not required by the subsidized batching policy.',
        'The maximum-tip source is inferred from receipts; this does not authenticate submitted refundTxHashes or distinguish tied/rounded payer identities.',
        'Quote-to-transaction equality proves applied fields, effective price and limits, not the economic correctness of the quotation algorithm by itself.',
        'L2 beneficiary/FeeVault totals and exact ordinary balances are checked by collect-mixed.py.']}


def save():
    out.write_text(json.dumps(record, indent=2) + '\n')


def rpc(url, method, *params):
    request = urllib.request.Request(url, json.dumps({'jsonrpc': '2.0', 'id': 1,
        'method': method, 'params': params}).encode(), {'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=15) as response:
        value = json.load(response)
    assert 'error' not in value, value
    return value['result']


def receipt(url, tx):
    value = rpc(url, 'eth_getTransactionReceipt', tx)
    assert value and value['status'] == '0x1' and value['transactionHash'] == tx
    header = rpc(url, 'eth_getBlockByNumber', value['blockNumber'], False)
    assert header['hash'] == value['blockHash']
    return value, header


save()
try:
    assert rpc(config['l2'], 'eth_getBlockByNumber', '0x0', False)['hash'] == config['genesis']
    assert rpc(config['l1'], 'eth_getBlockByNumber', '0x0', False)['hash'] == config['l1_genesis']
    grouped = {}
    for source in evidence['wave_sources']:
        tx_hash = source['transaction']
        path, plan = quotes[tx_hash]
        url = config['l1'] if source['direction'] == 'in' else config['l2']
        tx = rpc(url, 'eth_getTransactionByHash', tx_hash)
        actual, header = receipt(url, tx_hash)
        assert actual == source['source_receipt']
        assert tx['hash'] == tx_hash and tx['blockHash'] == header['hash']
        intent = plan['intent']; option = plan['selectedOption']
        assert tx['from'].lower() == intent['from'].lower()
        assert tx['to'].lower() == intent['to'].lower()
        assert tx['input'].lower() == intent['data'].lower()
        for field in ['nonce', 'value']:
            assert int(tx[field], 16) == int(intent[field], 16)
        assert int(tx['chainId'], 16) == int(plan['chain'])
        assert tx['type'] == option['type'] == '0x2'
        for field, planned in [('gas', 'gas'), ('maxFeePerGas', 'cap'), ('maxPriorityFeePerGas', 'tip')]:
            assert int(tx[field], 16) == int(plan[planned]) == int(option[field], 16)
        gas = int(actual['gasUsed'], 16); limit = int(tx['gas'], 16)
        price = int(actual['effectiveGasPrice'], 16); base = int(header['baseFeePerGas'], 16)
        assert 0 < gas <= limit
        assert price == min(int(tx['maxFeePerGas'], 16), base + int(tx['maxPriorityFeePerGas'], 16))
        item = {'source': tx_hash, 'direction': source['direction'], 'quote_file': path.name,
            'quote_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'receipt': actual, 'base_fee_per_gas': str(base), 'gas_limit': limit,
            'quote_gas_equals_used': gas == limit, 'charged_gas_wei': str(gas * price),
            'gross_tip_wei': str(gas * (price - base))}
        record['quotes'].append(item)
        if source['direction'] == 'in':
            post = source['correlation']['transactions']['postBatch']['hash']
            grouped.setdefault(post, []).append(item)
        save()
    assert len(record['quotes']) == 96 and sum(row['direction'] == 'in' for row in record['quotes']) == 48
    for post_hash, sources in grouped.items():
        post, header = receipt(config['l1'], post_hash)
        block = rpc(config['l1'], 'eth_getBlockByHash', header['hash'], True)
        assert block['hash'] == header['hash']
        posts = [tx for tx in block['transactions'] if (tx.get('to') or '').lower() == registry and tx['type'] == '0x3']
        assert len(posts) == 1 and posts[0]['hash'] == post_hash
        assert all(source['receipt']['blockHash'] == header['hash'] for source in sources)
        assert int(post['transactionIndex'], 16) < min(int(row['receipt']['transactionIndex'], 16) for row in sources)
        payer = max(sorted(sources, key=lambda row: int(row['receipt']['transactionIndex'], 16)), key=lambda row: int(row['gross_tip_wei']))
        payments = [tx for tx in block['transactions'] if tx['from'].lower() == block['miner'].lower()
            and (tx.get('to') or '').lower() == recipient and tx['input'] == '0x' and int(tx['value'], 16) > 0]
        assert len(payments) == 1, ('ambiguous/missing actual refund', post_hash, len(payments))
        payment = payments[0]; payout, _ = receipt(config['l1'], payment['hash'])
        assert int(payout['transactionIndex'], 16) > int(payer['receipt']['transactionIndex'], 16)
        assert int(payout['gasUsed'], 16) == 21000
        payout_cost = int(payout['gasUsed'], 16) * int(payout['effectiveGasPrice'], 16)
        expected = int(payer['gross_tip_wei']) * percent // 100 - payout_cost
        income = int(payment['value'], 16)
        assert income == expected and expected > 0
        before = rpc(config['l1'], 'eth_getBalance', recipient, hex(int(header['number'], 16) - 1))
        after = rpc(config['l1'], 'eth_getBalance', recipient, header['number'])
        assert int(after, 16) - int(before, 16) == income
        cost = int(post['gasUsed'], 16) * int(post['effectiveGasPrice'], 16) + int(post['blobGasUsed'], 16) * int(post['blobGasPrice'], 16)
        record['l1_posts'].append({'post': post_hash, 'block_hash': header['hash'], 'sources': [s['source'] for s in sources],
            'measured_max_tip_source': payer['source'], 'gross_tip_wei': payer['gross_tip_wei'],
            'post_receipt': post, 'payout_transaction': payment, 'payout_receipt': payout,
            'actual_income_wei': str(income), 'actual_post_cost_wei': str(cost),
            'shortfall_wei': str(max(0, cost - income)), 'surplus_wei': str(max(0, income - cost)),
            'recipient_balance_before': before, 'recipient_balance_after': after})
        save()
    record['verified'] = True
except BaseException as error:
    record['error'] = str(error)
    raise
finally:
    save()
print(json.dumps({'verified': True, 'quotes': len(record['quotes']), 'l1_posts': len(record['l1_posts']),
    'shortfall_posts': sum(int(row['shortfall_wei']) > 0 for row in record['l1_posts'])}))
