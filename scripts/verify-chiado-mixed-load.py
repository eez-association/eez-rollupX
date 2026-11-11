"""Independent, read-only checks supplementing the unchanged wave assertions."""
import argparse
import os
import json
import re
import subprocess
import time
import urllib.request
from pathlib import Path
from settlement_evidence import check_runtime, check_snapshot

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--evidence", type=Path, required=True)
parser.add_argument("--waves", type=int, required=True)
args = parser.parse_args()
assert 1 <= args.waves <= 30
root = out = args.evidence
waves = args.waves


def rpc(url, method, params):
    req = urllib.request.Request(url, json.dumps(dict(jsonrpc='2.0', id=1,
        method=method, params=params)).encode(), {'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=15) as response:
        value = json.load(response)
    assert 'error' not in value, (method, value.get('error'))
    return value['result']


def available(url, method, params):
    deadline = time.monotonic()+120
    while True:
        value = rpc(url, method, params)
        if value is not None:
            return value
        assert time.monotonic() < deadline, ('data unavailable', method)
        time.sleep(2)


# Never turn an incomplete or failed workload into a full-suite pass.
assert (root/'live-mixed.exit').read_text().strip() == '0'
wave = (root/'live-waves.log').read_text()
entries = re.findall(r'tx side=(in|out) kind=(\w+) arg=(\d+) hash=(0x[0-9a-fA-F]{64})', wave)
assert len(entries) == waves*8 and len({e[3] for e in entries}) == waves*8
for side in ('in','out'):
    assert sum(e[0]==side for e in entries)==waves*4
    expected_kinds=('set','noret','dep','wrap') if side=='in' else ('set','noret','wd','wrap')
    assert all(sum(e[0]==side and e[1]==kind for e in entries)==waves for kind in expected_kinds)
l1, l2, follower = ['http://127.0.0.1:'+str(p) for p in (18645, 18688, 19288)]
external = subprocess.check_output(['bash', '-c',
    'set +x; source .env; printf "%s" "$EEZ_L1_RPC_URL"'],
    cwd=os.environ['STRESS_DEPLOYMENT_ROOT'], text=True)
assert rpc(external, 'eth_chainId', []) == hex(10200)
safe = rpc(l2, 'eth_getBlockByNumber', ['safe', False])
deadline = time.monotonic()+360
while int(rpc(follower, 'eth_getBlockByNumber', ['safe', False])['number'], 16) < int(safe['number'], 16):
    assert time.monotonic() < deadline, 'Independent follower behind settlement endpoint'
    time.sleep(3)
assert rpc(follower, 'eth_getBlockByNumber', [safe['number'], False])['hash'] == safe['hash']
snapshots = [(l2, safe), (follower, safe)]
receipts, checked_blocks = [], set()
for side, kind, arg, txhash in entries:
    origin, independent = (l1, external) if side == 'in' else (l2, follower)
    receipt = available(origin, 'eth_getTransactionReceipt', [txhash])
    other = available(independent, 'eth_getTransactionReceipt', [txhash])
    assert receipt['status'] == '0x1'
    assert all(receipt[k] == other[k] for k in (
        'transactionHash', 'blockHash', 'blockNumber', 'transactionIndex',
        'status', 'gasUsed', 'effectiveGasPrice'))
    # RPC vendors may add optional metadata; bind every log's bytes, order,
    # transaction and block identity instead of requiring identical schemas.
    fields = ('address', 'topics', 'data', 'blockHash', 'blockNumber',
              'transactionHash', 'transactionIndex', 'logIndex', 'removed')
    assert [[log[k] for k in fields] for log in receipt['logs']] == [
        [log[k] for k in fields] for log in other['logs']]
    block = available(origin, 'eth_getBlockByNumber', [receipt['blockNumber'], False])
    assert block['hash'] == receipt['blockHash'] and txhash in block['transactions']
    if side == 'out':
        assert int(receipt['blockNumber'], 16) <= int(safe['number'], 16)
    if (side, block['number']) not in checked_blocks:
        other_block = available(independent, 'eth_getBlockByNumber', [block['number'], False])
        assert all(block[k] == other_block[k] for k in ('hash', 'stateRoot', 'receiptsRoot', 'transactionsRoot'))
        checked_blocks.add((side, block['number']))
        snapshots.extend(((origin, block), (independent, block)))
    receipts.append(dict(side=side, kind=kind, argument=arg, receipt=receipt))
call = subprocess.check_output(['cast', 'calldata', 'value()'], text=True).strip()
state_checks = []
for layer, origin, independent, expected in [
        ('L2', l2, follower, (300+waves, 200+waves)), ('L1', l1, external, (600+waves, 500+waves))]:
    match = re.search(layer+r' Value=(0x[0-9a-fA-F]{40})\s+ValueNoRet=(0x[0-9a-fA-F]{40})', wave)
    assert match
    block = safe if layer == 'L2' else rpc(l1, 'eth_getBlockByNumber', ['latest', False])
    assert available(independent, 'eth_getBlockByNumber', [block['number'], False])['hash'] == block['hash']
    snapshots.extend(((origin, block), (independent, block)))
    for address, value in zip(match.groups(), expected):
        args = [dict(to=address, data=call), block['number']]
        assert int(rpc(origin, 'eth_call', args), 16) == value
        assert int(rpc(independent, 'eth_call', args), 16) == value
        state_checks.append(dict(layer=layer, address=address, expected=value, block=block['number']))
# Bind value transfers to the proxy's registered recipient, including its
# pre-run balance; receipt success alone does not prove cross-chain delivery.
for side, kind, origin, origin_independent, manager, destination, independent in [
    ('in','dep',l1,external,os.environ['EEZ_REGISTRY_ADDRESS'],l2,follower),
    ('out','wd',l2,follower,'0x4B2760F84B83d6b3829E30A7573B0172B6d6402C',l1,external)]:
    selected=[e for e in entries if e[0]==side and e[1]==kind]
    first=rpc(origin,'eth_getTransactionByHash',[selected[0][3]])
    getter=subprocess.check_output(['cast','calldata','authorizedProxies(address)',first['to']],text=True).strip()
    mapping_block = rpc(origin, 'eth_getBlockByNumber', ['latest' if side == 'in' else 'safe', False])
    assert available(origin_independent, 'eth_getBlockByNumber', [mapping_block['number'], False])['hash'] == mapping_block['hash']
    snapshots.extend(((origin, mapping_block), (origin_independent, mapping_block)))
    args=[dict(to=manager,data=getter),mapping_block['number']]
    mapping=rpc(origin,'eth_call',args)
    assert mapping==rpc(origin_independent,'eth_call',args) and len(mapping)==194
    assert int(mapping[2:66],16)==1
    recipient='0x'+mapping[66:130][-40:]
    assert int(mapping[130:194],16)==(int(os.environ['EEZ_ROLLUP_ID']) if side=='in' else 0)
    block=safe if side=='in' else rpc(l1,'eth_getBlockByNumber',['latest',False])
    snapshots.extend(((destination, block), (independent, block)))
    # On L1 use the block before the first withdrawal's settlement; on L2 use
    # the driver's captured start before wave setup or any delivery.
    if side=='in':
        before_number=json.loads((root/'initial-blocks.json').read_text())['l2']['number']
    else:
        rcpt=rpc(l2,'eth_getTransactionReceipt',[selected[0][3]])
        corr=rpc('http://127.0.0.1:18998','eez_getSettlementByL2Block',[rcpt['blockHash']])
        before_number=hex(int(corr['l1BlockNumber'],16)-1)
    baseline=int(rpc(destination,'eth_getBalance',[recipient,before_number]),16)
    assert baseline==int(rpc(independent,'eth_getBalance',[recipient,before_number]),16)==0
    amount=sum(int(e[2]) for e in selected)
    assert int(rpc(destination,'eth_getBalance',[recipient,block['number']]),16)==amount
    assert int(rpc(independent,'eth_getBalance',[recipient,block['number']]),16)==amount
    state_checks.append(dict(side=side,kind=kind,recipient=recipient,before=baseline,amount=amount,block=block['number']))
queue = rpc('http://127.0.0.1:18998', 'eez_getCrossChainQueueStatus', [])
assert all(queue[k] == 0 for k in ('queued', 'inFlight', 'blocked', 'unknown'))
# Re-read pinned blocks after the evidence reads rather than assuming an
# earlier canonical observation remained canonical throughout verification.
ordinary = json.loads((root/'live-mixed-ordinary/summary.json').read_text())
assert ordinary['result'] == 'pass' and ordinary['accepted'] == 4500
for settlement in json.loads((root/'live-mixed-ordinary/settlements.json').read_text()):
    receipt, span = settlement['receipt'], settlement['correlation']['l2Range']
    l1_snapshot = dict(number=receipt['blockNumber'], hash=receipt['blockHash'])
    l2_snapshot = dict(number=span['lastBlockNumber'], hash=span['lastBlockHash'])
    snapshots.extend(((l1, l1_snapshot), (external, l1_snapshot),
                      (l2, l2_snapshot), (follower, l2_snapshot)))
for endpoint, snapshot in snapshots:
    check_snapshot(rpc, endpoint, snapshot)
identity = json.loads((root/'identity.json').read_text())
containers = json.loads(subprocess.check_output(['docker','inspect',*[entry['name'] for entry in identity]]))
check_runtime(identity, containers)
history = [json.loads(line) for line in (root/'cross-chain-receipt-history.jsonl').read_text().splitlines()]
movements = [r for r in history if r.get('previous') is not None and
    (r.get('receipt') is None or r['receipt']['blockHash'] != r['previous'][0])]
summary = dict(result='pass', plannedWaves=waves, checkedCompleteWaves=waves,
    originalRunnerResult='pass',
    checkedInboundReceipts=waves*4, checkedOutboundReceipts=waves*4,
    independentlyCheckedBlocks=len(checked_blocks), stateChecks=state_checks,
    safeL2Number=safe['number'], safeL2Hash=safe['hash'],
    observedReceiptMovements=movements,
    observedUniqueHashes=len({row['hash'] for row in history if 'hash' in row}),
    observationErrors=sum('observationError' in row for row in history),
    observationIsSampled=True, finalizedL1=False,
    queue={k:queue[k] for k in ('queued', 'inFlight', 'blocked', 'unknown')})
(out/'independent-cross-chain-receipts.json').write_text(json.dumps(receipts, indent=2))
(out/'independent-cross-chain-summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary, indent=2))
