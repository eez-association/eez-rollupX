"""Bounded live ordinary load plus canonical settlement audit, under wallet locks.

Uses only the explicitly selected workload account. Preserves signed intents before send;
never retries an ambiguous submission. No deployments, funding, or lifecycle calls.
"""
import collections
import json
import math
import os
from pathlib import Path
import subprocess
import time
import urllib.request

os.umask(0o077)
from settlement_evidence import check_queue, check_completion, cast_with_key

ROOT = Path(os.environ['STRESS_DEPLOYMENT_ROOT'])
OUT = Path(os.environ['STRESS_OUTPUT_DIR'])
OUT.mkdir(exist_ok=False)
RATE = 5
DURATION = int(os.environ.get('STRESS_DURATION_SECONDS', '900'))
assert DURATION in (180, 900), 'Supported validation durations are 180 and 900 seconds'
COUNT = RATE * DURATION
MAX_FEE = int(os.environ.get('STRESS_MAX_FEE_WEI', '100000000'))
TIP, GAS = 10_000_000, 21000
assert TIP <= MAX_FEE <= 100_000_000
EXPECTED_NODE = os.environ['STRESS_NODE_IMAGE_ID']
EXPECTED_SOURCE = os.environ['STRESS_SOURCE_REVISION']
EXPECTED_DELAY = int(os.environ.get('STRESS_DELAY_MS', '0'))
FOLLOWER_PORT = 19288
FOLLOWER_NAME = os.environ['STRESS_FOLLOWER_CONTAINER']
FOLLOWER_IMAGE = os.environ['STRESS_FOLLOWER_IMAGE']
EXPECTED_ADDRESS = os.environ['STRESS_SENDER_ADDRESS'].lower()
L2_CHAIN_ID = int(os.environ['STRESS_L2_CHAIN_ID'])


def request(url, payload=None):
    req = urllib.request.Request(url, None if payload is None else json.dumps(payload).encode(),
                                 {'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            return json.load(response)
    except Exception as exc:
        raise RuntimeError('Transport '+type(exc).__name__) from None


def batch(port, calls):
    url = f'http://127.0.0.1:{port}' if isinstance(port, int) else port
    response = request(url, [dict(jsonrpc='2.0', id=i, method=m, params=p)
                            for i, (m, p) in enumerate(calls)])
    assert isinstance(response, list) and len(response) == len(calls)
    by_id = {r['id']: r for r in response}
    assert set(by_id) == set(range(len(calls)))
    for i, r in by_id.items():
        if 'error' in r:
            raise RuntimeError(f'RPC {calls[i][0]} rejected: '+str(r['error']))
    return [by_id[i]['result'] for i in range(len(calls))]


def rpc(port, method, params=None):
    return batch(port, [(method, params or [])])[0]


def wait_rpc(port, method, params):
    # An independent provider may trail the local canonical observer. Missing
    # data is not disagreement; bound the wait, then retain exact comparisons.
    deadline = time.monotonic() + 120
    while True:
        result = rpc(port, method, params)
        if result is not None and result != []:
            return result
        assert time.monotonic() < deadline, 'Independent L1 data remained unavailable'
        time.sleep(2)


def dump(name, value):
    (OUT/name).write_text(json.dumps(value, indent=2))


def container():
    r = json.loads(subprocess.check_output(['docker', 'inspect', 'eez-node-chiado']))[0]
    assert r['State']['Running'] and r['Image'] == EXPECTED_NODE
    assert r['Config']['Labels']['org.opencontainers.image.revision'] == EXPECTED_SOURCE
    return r


node = container()
follower = json.loads(subprocess.check_output(['docker','inspect',FOLLOWER_NAME]))[0]
assert follower['State']['Running']
assert follower['Config']['Image'] == FOLLOWER_IMAGE
assert follower['Config']['Entrypoint'] == ['/usr/local/bin/eez-follower']
assert not any(v.startswith(('EEZ_SEQUENCER_RPC=', 'EEZ_L1_POSTER_KEY=', 'EEZ_PROOF_SIGNER_KEY=')) for v in follower['Config']['Env'])
node_env = dict((v.split('=', 1)+[None])[:2] for v in node['Config']['Env'])
assert node_env.get('EEZ_SETTLEMENT_START_AFTER_BLOCK') == '2'
assert int(node_env.get('EEZ_SETTLEMENT_START_DELAY_MS') or '0') == EXPECTED_DELAY
assert node_env['EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS'] == '150'
key = os.environ.pop('SOURCE_PK')
# The wave runner exports aliases of this same workload key. Do not pass them
# to unrelated child processes. cast still uses its documented key CLI option;
# this procedure requires a trusted operator host, as does the wave runner.
for name in ('EEZ_FUND_FROM_KEY', 'EEZ_L1_SETUP_KEY', 'EEZ_L2_SETUP_KEY'):
    os.environ.pop(name, None)
address = cast_with_key(['wallet', 'address'], key)
poster = os.environ['STRESS_POSTER_ADDRESS']
assert address.lower() == EXPECTED_ADDRESS and address.lower() != poster.lower()
assert rpc(18688, 'eth_chainId') == hex(L2_CHAIN_ID)
assert rpc(FOLLOWER_PORT, 'eth_chainId') == hex(L2_CHAIN_ID)
assert rpc(FOLLOWER_PORT,'eth_getBlockByNumber',['0x0',False])['hash'] == rpc(18688,'eth_getBlockByNumber',['0x0',False])['hash']
assert rpc(18645, 'eth_chainId') == hex(10200) and rpc(18645, 'eth_syncing') is False
latest = rpc(18688, 'eth_getBlockByNumber', ['latest', False])
assert abs(time.time()-int(latest['timestamp'], 16)) < 15
assert int(latest['baseFeePerGas'], 16) < MAX_FEE-TIP
nonce = int(rpc(18688, 'eth_getTransactionCount', [address, 'latest']), 16)
assert int(rpc(18688, 'eth_getTransactionCount', [address, 'pending']), 16) == nonce
balance = int(rpc(18688, 'eth_getBalance', [address, 'latest']), 16)
assert balance > COUNT*GAS*MAX_FEE
queue = rpc(18998, 'eez_getCrossChainQueueStatus')
# The driver checked the empty queue before starting the mixed workload.
# Cross-chain actors are now intentionally active alongside this sender.
check_queue(queue, idle=False)
dump('initial-mixed-queue.json', queue)
genesis = int(rpc(18688, 'eth_getBlockByNumber', ['0x0', False])['timestamp'], 16)
external = subprocess.check_output(['bash', '-c', 'set +x; source .env; printf "%s" "$EEZ_L1_RPC_URL"'], cwd=ROOT, text=True)
assert rpc(external, 'eth_chainId') == hex(10200)
start, monotonic_start = time.time(), time.monotonic()
metadata = dict(source=node['Config']['Labels']['org.opencontainers.image.revision'],
    protocol=node['Config']['Labels']['io.eez.protocol.revision'], image=node['Image'],
    containerId=node['Id'], startedAt=start, durationSeconds=DURATION, targetRate=RATE,
    maxTransactions=COUNT, maxTotalGasCostWei=COUNT*GAS*MAX_FEE, firstNonce=nonce,
    balanceBefore=balance, startBlock=latest['number'], P=2, delayMs=EXPECTED_DELAY,
    gas=GAS, maxFeePerGas=MAX_FEE, priorityFeePerGas=TIP)
dump('metadata.json', metadata)
events = (OUT/'events.jsonl').open('x', buffering=1)


def event(kind, **fields):
    events.write(json.dumps(dict(at=time.time(), kind=kind, **fields))+'\n')


sent, first_seen, health, observed = [], {}, [], {}
skipped, send_error, highest_seen = 0, None, int(latest['number'], 16)
last_health, last_progress = -100, -100
current_nonce = nonce
try:
    for tick in range(COUNT):
        scheduled = monotonic_start+tick/RATE
        time.sleep(max(0, scheduled-time.monotonic()))
        elapsed = time.monotonic()-monotonic_start
        if elapsed >= DURATION:
            skipped += COUNT-tick
            break
        if tick % RATE == 0:
            current_nonce = int(rpc(18688, 'eth_getTransactionCount', [address, 'latest']), 16)
            block = rpc(18688, 'eth_getBlockByNumber', ['latest', False])
            height = int(block['number'], 16)
            assert abs(time.time()-int(block['timestamp'], 16)) < 30, 'L2 head stale; stopping load'
            blocks = [block]
            if height > highest_seen+1:
                blocks = batch(18688, [('eth_getBlockByNumber', [hex(h), False])
                                      for h in range(highest_seen+1, height)])+blocks
            for b in blocks:
                observed[b['hash']] = b
                for txhash in b['transactions']:
                    first_seen.setdefault(txhash, time.time())
            highest_seen = max(height, highest_seen)
        if elapsed-last_health >= 10:
            queue = rpc(18998, 'eez_getCrossChainQueueStatus')
            safe = rpc(18688, 'eth_getBlockByNumber', ['safe', False])
            sample = dict(at=time.time(), elapsed=elapsed, sent=len(sent), latestNonce=current_nonce,
                          safeNumber=safe['number'], safeHash=safe['hash'], queue={k:queue[k] for k in ['queued','inFlight','blocked','unknown']})
            health.append(sample); event('health', **{k:v for k,v in sample.items() if k!='at'})
            last_health = elapsed
        if elapsed-last_progress >= 30:
            print(json.dumps(dict(elapsedSeconds=round(elapsed), accepted=len(sent),
                outstanding=max(0, nonce+len(sent)-current_nonce), skippedTicks=skipped)), flush=True)
            last_progress = elapsed
        if nonce+len(sent)-current_nonce >= 48:
            skipped += 1
            event('backpressure', outstanding=nonce+len(sent)-current_nonce)
            continue
        raw = cast_with_key(['mktx',address,'--rpc-url','http://127.0.0.1:18688',
            '--chain-id',str(L2_CHAIN_ID),'--nonce',str(nonce+len(sent)),
            '--gas-limit',str(GAS),'--gas-price',str(MAX_FEE),'--priority-gas-price',str(TIP),
            '--value','0'], key)
        txhash = subprocess.check_output(['cast','keccak',raw], text=True).strip()
        tx = dict(hash=txhash, nonce=nonce+len(sent), submittedAt=time.time())
        event('intent', **tx, raw=raw)
        assert rpc(18688, 'eth_sendRawTransaction', [raw]) == txhash
        sent.append(tx); event('accepted', **tx)
    time.sleep(max(0, monotonic_start+DURATION-time.monotonic()))
except BaseException as exc:
    send_error = type(exc).__name__+': '+str(exc)
    event('load_stopped', error=send_error)
    print('Load stopped; preserving accepted hashes:',send_error,flush=True)
    if not isinstance(exc, Exception):
        raise
finally:
    del key
    metadata.update(loadEndedAt=time.time(), loadElapsedSeconds=time.monotonic()-monotonic_start,
                    accepted=len(sent), skippedTicks=skipped, loadError=send_error)
    dump('metadata.json', metadata); dump('accepted.json', sent)
    dump('observed-blocks.json', list(observed.values())); dump('first-seen.json', first_seen); dump('health.json', health)

assert sent, 'No transactions accepted'
print('Load complete; checking every receipt and waiting for canonical settlement',flush=True)
deadline = time.monotonic()+360
while True:
    receipts = []
    for offset in range(0,len(sent),100):
        receipts += batch(18688, [('eth_getTransactionReceipt',[t['hash']]) for t in sent[offset:offset+100]])
    present = [r for r in receipts if r]
    assert all(r['status']=='0x1' for r in present), 'Reverted ordinary transaction'
    if len(present)==len(sent):
        last = max(present,key=lambda r:int(r['blockNumber'],16))
        correlation = rpc(18998,'eez_getSettlementByL2Block',[last['blockHash']])
        if correlation and correlation['canonicalL2'] and correlation['l1TransactionHash']:
            break
    if time.monotonic()>deadline:
        dump('receipts-at-timeout.json',receipts)
        raise RuntimeError('Canonical settlement timeout; no automatic resend')
    print('Drain:',len(present),'/',len(sent),'receipts; awaiting final canonical range',flush=True)
    time.sleep(10)
dump('receipts.json',receipts)
end = rpc(18688,'eth_getBlockByNumber',['latest',False])
blocks=[]
for offset in range(int(latest['number'],16),int(end['number'],16)+1,50):
    blocks += batch(18688,[('eth_getBlockByNumber',[hex(h),False]) for h in range(offset,min(offset+50,int(end['number'],16)+1))])
dump('canonical-blocks.json',blocks)
by_height={b['number']:b for b in blocks}; tx_by_hash={t['hash']:t for t in sent}
positions=collections.Counter(); fees=0
for tx,r in zip(sent,receipts):
    b=by_height[r['blockNumber']]
    assert r['transactionHash']==tx['hash'] and b['hash']==r['blockHash'] and tx['hash'] in b['transactions']
    positions[(int(b['timestamp'],16)-genesis)%5]+=1
    fees+=int(r['gasUsed'],16)*int(r['effectiveGasPrice'],16)
assert int(rpc(18688,'eth_getTransactionCount',[address,'latest']),16)==nonce+len(sent)
after_balance=int(rpc(18688,'eth_getBalance',[address,'latest']),16)
assert balance-after_balance==fees, 'Unexpected account balance movement beyond receipt gas'

settlements={}
for height in sorted({r['blockNumber'] for r in receipts},key=lambda h:int(h,16)):
    if any(int(s['l2Range']['firstBlockNumber'],16)<=int(height,16)<=int(s['l2Range']['lastBlockNumber'],16) for s in settlements.values()):
        continue
    c=rpc(18998,'eez_getSettlementByL2Block',[by_height[height]['hash']])
    assert c and c['canonicalL2'] and c['l1TransactionHash']
    settlements[c['l1TransactionHash']]={k:v for k,v in c.items() if k!='l2Blocks'}
beacon_genesis=int(request('http://127.0.0.1:6152/eth/v1/beacon/genesis')['data']['genesis_time'])
slot_seconds=int(request('http://127.0.0.1:6152/eth/v1/config/spec')['data']['SECONDS_PER_SLOT'])
verified=[]
transaction_batches={}
execution_topic=subprocess.check_output(['cast','keccak','L2ExecutionPerformed(uint64,bytes32,uint256)'],text=True).strip()
for txhash,c in settlements.items():
    tx,r= batch(18645,[('eth_getTransactionByHash',[txhash]),('eth_getTransactionReceipt',[txhash])])
    assert r['status']=='0x1' and int(tx['type'],16)==3 and tx['from'].lower()==poster.lower()
    remote_r=wait_rpc(external,'eth_getTransactionReceipt',[txhash])
    assert all(remote_r[k]==r[k] for k in ['blockHash','blockNumber','status','gasUsed'])
    b=rpc(18645,'eth_getBlockByNumber',[r['blockNumber'],False])
    assert b['hash']==r['blockHash']==wait_rpc(external,'eth_getBlockByNumber',[r['blockNumber'],False])['hash']
    slot=(int(b['timestamp'],16)-beacon_genesis)//slot_seconds
    assert tx['blobVersionedHashes']
    full_blobs=[]
    for h in tx['blobVersionedHashes']:
        blobs=request(f'http://127.0.0.1:6152/eth/v1/beacon/blobs/{slot}?versioned_hashes={h}')['data']
        assert len(blobs)==1 and len(blobs[0])==262146
        full_blobs.extend(blobs)
    evidence_name=f'blob-{txhash}.json'
    dump(evidence_name,dict(rollup_id=int(node_env['EEZ_ROLLUP_ID']),l2_chain_id=L2_CHAIN_ID,
        beacon_slot=slot,l1_transaction=tx,l1_receipt=r,beacon_sidecars=dict(data=full_blobs)))
    # Receipt success can include partial consumption. Authenticate the actual
    # registry event in the independent L1 receipt, not only a calldata claim.
    # Mixed batches may finish in deferred inbound companion transactions,
    # after postAndVerifyBatch. Authenticate the complete same-block event
    # stream independently; the posting receipt alone is only a prefix.
    query=dict(blockHash=r['blockHash'], address=node_env['EEZ_REGISTRY_ADDRESS'],
        topics=[execution_topic, '0x'+format(int(node_env['EEZ_ROLLUP_ID']), '064x')])
    updates=wait_rpc(external,'eth_getLogs',[query])
    local_updates=rpc(18645,'eth_getLogs',[query])
    completion=wait_rpc(external,'eth_getTransactionReceipt',[updates[-1]['transactionHash']]) if updates else None
    check_completion(c, r, updates, local_updates, completion,
                     node_env['EEZ_REGISTRY_ADDRESS'], query['topics'])
    dump(f'execution-events-{txhash}.json',dict(correlation=c,events=updates,completionReceipt=completion))
    terminal=rpc(18688,'eth_getBlockByNumber',[c['l2Range']['lastBlockNumber'],False])
    assert terminal['hash']==c['l2Range']['lastBlockHash']
    contained={t['hash'] for t,receipt in zip(sent,receipts)
        if int(c['l2Range']['firstBlockNumber'],16)<=int(receipt['blockNumber'],16)<=int(c['l2Range']['lastBlockNumber'],16)}
    transaction_batches.update({h:txhash for h in contained})
    verified.append(dict(transactionHash=txhash,receipt=r,correlation=c,blobCount=len(tx['blobVersionedHashes'])))
dump('settlements.json',verified)
assert set(transaction_batches)==set(tx_by_hash)
terminal=max(verified,key=lambda s:int(s['correlation']['l2Range']['lastBlockNumber'],16))['correlation']['l2Range']
# The runbook requires an independent L1/Beacon-only follower. Its blob source
# recomputes KZG versioned hashes before decoding and executing the payloads.
# Compare every workload receipt and every containing block, not just its tip.
deadline=time.monotonic()+360
while int(rpc(FOLLOWER_PORT,'eth_getBlockByNumber',['safe',False])['number'],16)<int(terminal['lastBlockNumber'],16):
    assert time.monotonic()<deadline, 'Independent follower did not reach the settled endpoint'
    time.sleep(5)
replay_receipts=[]
for offset in range(0,len(sent),100):
    replay_receipts+=batch(FOLLOWER_PORT,[('eth_getTransactionReceipt',[t['hash']]) for t in sent[offset:offset+100]])
for actual,expected in zip(replay_receipts,receipts):
    assert actual and all(actual[k]==expected[k] for k in ['transactionHash','blockHash','blockNumber','transactionIndex','status','gasUsed','effectiveGasPrice'])
heights=sorted({r['blockNumber'] for r in receipts}|{s['correlation']['l2Range']['lastBlockNumber'] for s in verified},key=lambda h:int(h,16))
for offset in range(0,len(heights),100):
    for b in batch(FOLLOWER_PORT,[('eth_getBlockByNumber',[h,False]) for h in heights[offset:offset+100]]):
        assert b and b['hash']==by_height[b['number']]['hash'] and b['stateRoot']==by_height[b['number']]['stateRoot']
assert int(rpc(FOLLOWER_PORT,'eth_getTransactionCount',[address,terminal['lastBlockNumber']]),16)==nonce+len(sent)
assert int(rpc(FOLLOWER_PORT,'eth_getBalance',[address,terminal['lastBlockNumber']]),16)==after_balance
follower_after=json.loads(subprocess.check_output(['docker','inspect',FOLLOWER_NAME]))[0]
assert follower_after['Id']==follower['Id'] and follower_after['State']['Running'] and follower_after['RestartCount']==follower['RestartCount']
dump('independent-replay.json',dict(verified=True,source='canonical_l1_and_beacon_only',
    followerSource=follower['Config']['Labels']['org.opencontainers.image.revision'],
    followerImage=follower['Config']['Image'],followerContainer=follower['Id'],
    checkedReceipts=len(replay_receipts),checkedBlocks=len(heights),
    safeL2Block=terminal['lastBlockNumber'],safeL2Hash=terminal['lastBlockHash'],
    kzgVerification='independent-follower-blob-reconstruction',receipts=replay_receipts))
interior=[b for b in blocks if start+2 <= int(b['timestamp'],16) <= start+DURATION-2]
occupancy=[]
for b in interior:
    position=(int(b['timestamp'],16)-genesis)%5
    occupancy.append(dict(number=int(b['number'],16),timestamp=int(b['timestamp'],16),position=position,
        testTransactions=sum(h in tx_by_hash for h in b['transactions']),totalTransactions=len(b['transactions'])))
dump('occupancy.json',occupancy)
ordinary=[b for b in occupancy if b['position']!=0]; sync=[b for b in occupancy if b['position']==0]
latencies=sorted(first_seen[t['hash']]-t['submittedAt'] for t in sent if t['hash'] in first_seen)
def percentile(q): return latencies[min(len(latencies)-1, math.ceil(q*len(latencies))-1)] if latencies else None
node_after=container();assert node_after['Id']==node['Id'] and node_after['RestartCount']==node['RestartCount']
queue=rpc(18998,'eez_getCrossChainQueueStatus')
summary=dict(result='pass' if not send_error and len(sent)==COUNT and metadata['loadElapsedSeconds']>=DURATION else 'incomplete-load',
    accepted=len(sent),canonicalSuccessfulReceipts=len(receipts),durationSeconds=metadata['loadElapsedSeconds'],
    skippedTicks=skipped,positions=dict(positions),ordinaryBlocks=len(ordinary),
    ordinaryWithMultipleTestTransactions=sum(b['testTransactions']>=2 for b in ordinary),
    emptyOrdinaryBlocks=sum(b['testTransactions']==0 for b in ordinary),
    oneTestTransactionOrdinaryBlocks=sum(b['testTransactions']==1 for b in ordinary),
    syncBlocks=len(sync),syncWithTestTransactions=sum(b['testTransactions']>0 for b in sync),
    settlements=len(verified),fullBlobsRetrieved=sum(s['blobCount'] for s in verified),
    exactNonceAdvance=True,gasCostWei=fees,balanceMatchesGas=True,
    firstInclusionObservationSeconds=dict(samples=len(latencies),p50=percentile(.5),p95=percentile(.95),max=max(latencies) if latencies else None),
    queue={k:queue[k] for k in ['queued','inFlight','blocked','unknown']},
    independentFollowerReplay=True,independentKzgVerification=True,
    kzgVerificationMethod='independent-follower-blob-reconstruction',loadError=send_error)
dump('summary.json',summary)
event('complete',summary=summary);events.close()
print(json.dumps(summary,indent=2),flush=True)
assert summary['result'] == 'pass', 'Incomplete load is not a passing stress test'
