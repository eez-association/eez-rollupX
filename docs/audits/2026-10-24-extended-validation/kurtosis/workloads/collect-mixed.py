"""Read-only accounting, DA and replay selection for the two isolated campaigns.

All full canonical settlement claims remain unchanged. Native receipts are
retained separately as well; only the baseline archive omits them from its receipt map.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request

config = json.loads(Path(os.environ['EEZ_LOAD_CONFIG']).read_text())
assert config['enclave'] == os.environ['KURTOSIS_ENCLAVE']
root = Path(os.environ['EEZ_MIXED_OUTPUT'])
out = Path(os.environ['EEZ_MIXED_EVIDENCE'])
out.mkdir(exist_ok=False)
verifier = Path(os.environ['EEZ_BLOB_VERIFIER_BIN'])
l1, l2, beacon = config['l1'], config['l2'], config['beacon']
registry = os.environ['EEZ_REGISTRY_ADDRESS']


def get(url):
    with urllib.request.urlopen(url, timeout=30) as response:
        return json.load(response)


def batch(url, calls):
    req = urllib.request.Request(url, json.dumps([
        {'jsonrpc': '2.0', 'id': i, 'method': method, 'params': params}
        for i, (method, params) in enumerate(calls)
    ]).encode(), {'Content-Type': 'application/json'})
    result = get(req)
    assert len(result) == len(calls)
    rows = {row['id']: row for row in result}
    assert len(rows) == len(calls)
    for i, call in enumerate(calls):
        assert 'error' not in rows[i], (call[0], rows[i].get('error'))
    return [rows[i]['result'] for i in range(len(calls))]


def rpc(url, method, *params):
    return batch(url, [(method, params)])[0]


def save(name, value):
    (out / name).write_text(json.dumps(value, indent=2) + '\n')


def checked_receipt(url, tx):
    row = rpc(url, 'eth_getTransactionReceipt', tx)
    assert row and row['transactionHash'] == tx and row['status'] == '0x1'
    assert rpc(url, 'eth_getBlockByNumber', row['blockNumber'], False)['hash'] == row['blockHash']
    return row


assert rpc(l2, 'eth_getBlockByNumber', '0x0', False)['hash'] == config['genesis']
ordinary = json.loads((root / 'ordinary/summary.json').read_text())
assert ordinary['verified_receipts_and_accounting'] and ordinary['error'] is None
assert ordinary['planned'] == ordinary['offered'] == 15000
meta = json.loads((root / 'ordinary/metadata.json').read_text())
original_receipts = json.loads((root / 'ordinary/receipts.json').read_text())
workloads = {'mixed_ordinary': ordinary}
if os.environ.get('EEZ_PIPELINE_OUTPUT'):
    pipeline_root = Path(os.environ['EEZ_PIPELINE_OUTPUT'])
    pipeline = json.loads((pipeline_root / 'summary.json').read_text())
    assert pipeline['verified_receipts_and_accounting'] and pipeline['error'] is None
    assert pipeline['planned'] == pipeline['offered'] == 5400
    assert pipeline['runtime_source'] == ordinary['runtime_source']
    original_receipts += json.loads((pipeline_root / 'receipts.json').read_text())
    workloads['pipeline_ordinary'] = pipeline
for name, variable in [
    ('recovery_ordinary', 'EEZ_RECOVERY_OUTPUT'),
    ('delayed_l1_ordinary', 'EEZ_DELAYED_L1_OUTPUT'),
    ('missed_l1_ordinary', 'EEZ_MISSED_L1_OUTPUT'),
]:
    if not os.environ.get(variable):
        continue
    recovery_root = Path(os.environ[variable])
    recovery = json.loads((recovery_root / 'summary.json').read_text())
    assert recovery['verified_receipts_and_accounting'] and recovery['error'] is None
    assert recovery['accepted'] == recovery['planned'] == recovery['offered'] == 24
    assert recovery['runtime_source'] == ordinary['runtime_source']
    recovery_receipts = json.loads((recovery_root / 'receipts.json').read_text())
    assert len(recovery_receipts) == recovery['accepted']
    original_receipts += recovery_receipts
    workloads[name] = recovery
assert len({r['transactionHash'] for r in original_receipts}) == len(original_receipts)
ordinary_terminal = max(int(row['terminal']['number'], 16) for row in workloads.values())
log = (root / 'waves.log').read_text()
assert 'WAVE TEST PASSED' in log and 'PROOF SIGNER TEST PASSED' in log
triggers = re.findall(r'tx side=(in|out) kind=(\w+) arg=(\d+) hash=(0x[0-9a-f]{64})', log)
assert len(triggers) == len(set(row[3] for row in triggers)) == 96
assert sum(side == 'in' for side, *_ in triggers) == 48

# Bind the whole retained history to one canonical L1 observation. A subsequent
# moving head cannot silently change the selection or its commitment.
l1_head = rpc(l1, 'eth_getBlockByNumber', 'latest', False)
safe = rpc(l2, 'eth_getBlockByNumber', 'safe', False)
call = subprocess.check_output(['cast', 'calldata', 'rollups(uint64)', '1'], text=True).strip()
commitment = rpc(l1, 'eth_call', {'to': registry, 'data': call}, l1_head['number'])
tracked = '0x' + commitment[66:130]
tracked_block = rpc(l2, 'eth_getBlockByHash', tracked, False)
assert tracked_block and rpc(l2, 'eth_getBlockByNumber', tracked_block['number'], False)['hash'] == tracked
assert int(safe['number'], 16) >= ordinary_terminal
assert int(tracked_block['number'], 16) >= ordinary_terminal
topic = subprocess.check_output(['cast', 'keccak', 'BatchPosted(bytes32,uint64[])'], text=True).strip()
events = rpc(l1, 'eth_getLogs', {'address': registry, 'fromBlock': '0x0',
    'toBlock': l1_head['number'], 'topics': [topic]})
assert events
save('post-events.json', events)
genesis_time = int(get(beacon + '/eth/v1/beacon/genesis')['data']['genesis_time'])
slot_seconds = int(get(beacon + '/eth/v1/config/spec')['data']['SECONDS_PER_SLOT'])
posts, excluded, range_by_post = {}, [], {}
headers = {}


def l2_header(number):
    if number not in headers:
        headers[number] = rpc(l2, 'eth_getBlockByNumber', hex(number), False)
        assert headers[number]
    return headers[number]


for tx in dict.fromkeys(event['transactionHash'] for event in events):
    transaction = rpc(l1, 'eth_getTransactionByHash', tx)
    receipt = checked_receipt(l1, tx)
    header = rpc(l1, 'eth_getBlockByNumber', transaction['blockNumber'], False)
    assert transaction['blockHash'] == receipt['blockHash'] == header['hash']
    assert int(transaction['type'], 16) == 3
    elapsed = int(header['timestamp'], 16) - genesis_time
    assert elapsed >= 0 and elapsed % slot_seconds == 0
    slot = elapsed // slot_seconds
    path = out / (tx + '-input.json')
    path.write_text(json.dumps({'rollup_id': 1, 'beacon_slot': slot,
        'l1_transaction': transaction, 'l1_receipt': receipt,
        'beacon_sidecars': get(beacon + '/eth/v1/beacon/blobs/' + str(slot))}) + '\n')
    with (out / (tx + '-batch.json')).open('x') as stdout, (out / (tx + '-verifier.log')).open('x') as stderr:
        subprocess.run([str(verifier), str(path)], stdout=stdout, stderr=stderr, check=True, timeout=60)
    evidence = json.loads((out / (tx + '-batch.json')).read_text())
    assert evidence['verified'] and evidence['l1_transaction_hash'] == tx
    assert evidence['rollup_id'] == 1
    claim = evidence['settlement_claim']
    before = rpc(l2, 'eth_getBlockByHash', claim['pre_block_hash'], False)
    after = rpc(l2, 'eth_getBlockByHash', claim['post_block_hash'], False)
    if not before or not after or any(l2_header(int(b['number'], 16))['hash'] != b['hash'] for b in (before, after)):
        excluded.append({'post': tx, 'reason': 'original full endpoint absent or noncanonical', 'claim': claim})
        continue
    first, last = int(before['number'], 16) + 1, int(after['number'], 16)
    assert last - first + 1 == evidence['payload']['block_count']
    assert 0 < evidence['payload']['block_count'] <= 24
    if last > int(safe['number'], 16):
        excluded.append({'post': tx, 'reason': 'after captured safe endpoint', 'claim': claim})
        continue
    posts[tx] = evidence
    range_by_post[tx] = (first, last)
save('excluded-original-claims.json', excluded)
assert posts
ordered = sorted(posts, key=lambda tx: range_by_post[tx])
coverage = {}
for tx in ordered:
    first, last = range_by_post[tx]
    for number in range(first, last + 1):
        assert number not in coverage, ('overlapping complete canonical claims', number)
        coverage[number] = tx
terminal = max(coverage)
assert terminal >= ordinary_terminal

wave_rows, selected, native = [], {}, {}
wrapped_topic = subprocess.check_output(['cast', 'keccak', 'Wrapped(uint256,bool,bool,uint256)'], text=True).strip()
wrapper_return_checks = []
for source_side, kind, argument, tx in triggers:
    front = config['l1front'] if source_side == 'in' else config['front']
    correlation = rpc(front, 'eez_getCrossChainTransaction', tx)
    assert correlation['ownership']['state'] == 'terminal'
    assert correlation['ownership']['reason'] == 'Committed'
    links = correlation['transactions']
    assert links['complete'] is True
    post = links['postBatch']['hash']
    assert post in posts
    source = checked_receipt(l1 if source_side == 'in' else l2, tx)
    if kind == 'wrap':
        source_tx = rpc(l1 if source_side == 'in' else l2, 'eth_getTransactionByHash', tx)
        wrapped = [row for row in source['logs'] if row['address'].lower() == source_tx['to'].lower()
            and row['topics'] == [wrapped_topic]]
        assert len(wrapped) == 1
        data = bytes.fromhex(wrapped[0]['data'][2:])
        assert len(data) == 128
        decoded = [int.from_bytes(data[i:i + 32], 'big') for i in range(0, 128, 32)]
        assert decoded == [int(argument), 1, 1, int(argument)]
        wrapper_return_checks.append({'source': tx, 'side': source_side,
            'event': wrapped[0], 'decoded_input_ok_changed_newValue': decoded})
    own = correlation['ownership']['canonical']
    assert rpc(l1, 'eth_getBlockByNumber', hex(own['number']), False)['hash'] == own['hash']
    assert posts[post]['l1_block_hash'] == own['hash'] and posts[post]['l1_block_number'] == own['number']
    for link in links['l2']:
        receipt = checked_receipt(l2, link['hash'])
        assert coverage[int(receipt['blockNumber'], 16)] == post
        receipt_type = int(receipt['type'], 16)
        value = {'post': post, 'receipt': receipt}
        baseline_parser = config['runtime_source'].startswith('a50c326d')
        if receipt_type in (0x45, 0x76):
            assert receipt_type == (0x76 if baseline_parser else 0x45)
            assert link['hash'] not in native or native[link['hash']] == value
            native[link['hash']] = value
        # The final archive decodes native-v1 receipts; only the baseline's
        # older receipt parser needs those records outside its explicit map.
        if receipt_type != 0x76 or not baseline_parser:
            assert link['hash'] not in selected or selected[link['hash']] == value
            selected[link['hash']] = value
    if source_side == 'in':
        assert source['blockHash'] == own['hash']
    else:
        assert tx in selected
    wave_rows.append({'direction': source_side, 'kind': kind, 'argument': argument,
        'transaction': tx, 'source_receipt': source, 'correlation': correlation})
assert len(wrapper_return_checks) == 24

for offset in range(0, len(original_receipts), 100):
    original = original_receipts[offset:offset + 100]
    current = batch(l2, [('eth_getTransactionReceipt', [r['transactionHash']]) for r in original])
    for before, receipt in zip(original, current):
        assert before == receipt, ('receipt changed since accounting', before['transactionHash'])
        number = int(receipt['blockNumber'], 16)
        assert receipt['blockHash'] == l2_header(number)['hash']
        selected[receipt['transactionHash']] = {'post': coverage[number], 'receipt': receipt}

# The aborted preparation is explicitly outside all measured workloads. Recheck
# its original receipts against this chain and retain it as a separate replay group.
preparation = None
if os.environ.get('EEZ_PREPARATORY_RECEIPTS'):
    preparation_path = Path(os.environ['EEZ_PREPARATORY_RECEIPTS'])
    preparation_input = json.loads(preparation_path.read_text())
    assert preparation_input['verified'] and preparation_input['first_failure_retained']
    assert preparation_input['cross_chain_triggers_submitted'] == 0
    preparation_rows = preparation_input['aborted_campaign_ordinary_receipts']
    assert len(preparation_rows) == 2
    preparation_hashes = []
    for original in preparation_rows:
        tx = original['transactionHash']
        assert tx not in selected, ('duplicate preparatory receipt', tx)
        receipt = checked_receipt(l2, tx)
        assert receipt == original, ('preparatory receipt changed', tx)
        assert receipt['from'].lower() == config['sender'].lower()
        assert receipt['to'].lower() == config['recipient'].lower()
        assert int(receipt['type'], 16) == 2 and int(receipt['gasUsed'], 16) == 21000
        number = int(receipt['blockNumber'], 16)
        assert number <= int(meta['before']['number'], 16)
        selected[tx] = {'post': coverage[number], 'receipt': receipt}
        preparation_hashes.append(tx)
    preparation_gas = sum(int(r['gasUsed'], 16) * int(r['effectiveGasPrice'], 16)
        for r in preparation_rows)
    assert preparation_gas == int(preparation_input['aborted_campaign_gas_cost_wei'])
    preparation = {'source_sha256': hashlib.file_digest(preparation_path.open('rb'), 'sha256').hexdigest(),
        'transaction_hashes': preparation_hashes, 'canonical_receipts_rechecked': True,
        'gas_cost_wei': str(preparation_gas), 'included_in_measured_statistics': False,
        'origin': 'mixed-r2 aborted after the separate bootstrap quote_limit rejection'}
    save('preparatory-replay-group.json', preparation)

# Account for every non-native transaction in the entire measured interval, not
# just the dedicated sender. Fresh random wave recipients never equal the two
# fixed collectors; any unexpected external balance movement fails explicitly.
first = int(meta['before']['number'], 16) + 1
last = ordinary_terminal
base_fees = tips = 0
miner = '0x4444444444444444444444444444444444444444'
vault = '0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078'
fee_rows = []
previous = l2_header(first - 1)
for offset in range(first, last + 1, 50):
    nums = list(range(offset, min(offset + 50, last + 1)))
    blocks = batch(l2, [('eth_getBlockByNumber', [hex(n), False]) for n in nums])
    receipts = batch(l2, [('eth_getBlockReceipts', [hex(n)]) for n in nums])
    for block, block_receipts in zip(blocks, receipts):
        assert block and block['miner'].lower() == miner
        assert int(block['number'], 16) == int(previous['number'], 16) + 1
        assert block['parentHash'] == previous['hash']
        assert int(block['timestamp'], 16) == int(previous['timestamp'], 16) + 2
        assert [r['transactionHash'] for r in block_receipts] == block['transactions']
        assert sum(int(r['gasUsed'], 16) for r in block_receipts) == int(block['gasUsed'], 16)
        for receipt in block_receipts:
            assert receipt['blockHash'] == block['hash']
            if int(receipt['type'], 16) in (0x45, 0x76):
                continue
            gas = int(receipt['gasUsed'], 16)
            price = int(receipt['effectiveGasPrice'], 16)
            base = int(block['baseFeePerGas'], 16)
            assert price >= base
            base_fees += gas * base
            tips += gas * (price - base)
        fee_rows.append({'block': block, 'receipts': block_receipts})
        previous = block
save('all-measured-block-receipts.json', fee_rows)
balances = {}
for name, address in [('beneficiary', miner), ('vault', vault)]:
    values = batch(l2, [('eth_getBalance', [address, hex(n)]) for n in (first - 1, last)])
    balances[name] = {'before': values[0], 'after': values[1], 'delta': int(values[1], 16) - int(values[0], 16)}
assert balances['beneficiary']['delta'] == tips
vault_code = rpc(l2, 'eth_getCode', vault, hex(last))
fee_mode = 'burn' if config['runtime_source'].startswith('a50c326d') else 'vault'
assert balances['vault']['delta'] == (0 if fee_mode == 'burn' else base_fees)
assert (vault_code == '0x') == (fee_mode == 'burn')
accounting = {'verified': True, 'first': first, 'last': last, 'base_fee_mode': fee_mode,
    'all_non_native_base_fees': str(base_fees), 'all_non_native_tips': str(tips),
    'balances': balances,
    'ordinary_gas_cost_wei': str(sum(int(row['gas_cost_wei']) for row in workloads.values())),
    'sender_and_recipient_exact': True}
save('fee-accounting.json', accounting)
assert rpc(l1, 'eth_getBlockByNumber', l1_head['number'], False)['hash'] == l1_head['hash']
summary = {'verified': True, 'runtime_source': config['runtime_source'], 'enclave': config['enclave'],
    'rollup_id': 1, 'workloads': workloads, 'preparatory_receipts': preparation,
    'verifier_sha256': hashlib.file_digest(verifier.open('rb'), 'sha256').hexdigest(),
    'settlement': {'safe_l2_block': terminal, 'safe_l2_block_hash': l2_header(terminal)['hash']},
    'verified_blob_batches': [posts[tx] for tx in ordered],
    'l2_transaction_to_l1_blob_batch': {tx: row['post'] for tx, row in selected.items()},
    'l2_transaction_status': {tx: row['receipt']['status'] == '0x1' for tx, row in selected.items()},
    'selected_receipts': {tx: row['receipt'] for tx, row in selected.items()},
    'native_receipts_separate': native, 'ranges': range_by_post,
    'wave_sources': wave_rows, 'wrapper_return_checks': wrapper_return_checks,
    'accounting': accounting, 'canonical_l1_observation': l1_head,
    'l1_commitment_at_observation': tracked, 'excluded_claims': excluded,
    'native_receipts_selected': not config['runtime_source'].startswith('a50c326d'),
    'scope': 'KZG/native payload authentication, complete canonical claims, all accepted ordinary receipts and 96 source effects; cold follower pending. Native-v1 receipts enter the final archive map; only baseline native receipts remain outside its older parser.'}
save('replay-input.json', summary)
print(json.dumps({'verified': True, 'batches': len(posts), 'ordinary_receipts': len(original_receipts),
    'selected_receipts': len(selected), 'native_receipts_separate': len(native),
    'wave_sources': len(wave_rows), 'terminal': terminal, 'fee_mode': fee_mode}))
