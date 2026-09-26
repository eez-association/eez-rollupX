"""Read-only canonical measurement for this audit's 870-source Chiado cohorts.

Run from the repository root with the protocol run directory and an existing
private output directory. This is intentionally bound to rollup 1 / L2 6292 and
the recorded registry. It rejects missing/reverted sources, changed canonical
blocks, ambiguous multiple postings in one L1 block and incomplete correlations.
It does not send transactions, prove semantic effects, or run a follower.

The staged runner's initial mined heights may precede L2 reconciliation. Retain
this small audit helper because canonical attribution must be repeated identically
for baseline and candidate; receipts alone do not establish source completion.
"""
import csv
import datetime
import json
import os
import statistics
import sys
import urllib.request
from pathlib import Path
os.umask(0o077)
run = Path(sys.argv[1])
out = Path(sys.argv[2])
registry = '0x5826242cf480856c36ad4afa464954d3571d1b5b'
ports = {'L1': 18645, 'L2': 18688}
chain_ids = {'L1': 10200, 'L2': 6292}

def batch(layer, queries):
    result = []
    for offset in range(0, len(queries), 25):
        chunk = queries[offset:offset + 25]
        payload = [{'jsonrpc': '2.0', 'id': i, 'method': m, 'params': p} for i, (m, p) in enumerate(chunk)]
        req = urllib.request.Request(f'http://127.0.0.1:{ports[layer]}', json.dumps(payload).encode(), {'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=60) as f:
            rows = json.load(f)
        index = {r['id']: r for r in rows}
        assert len(index) == len(chunk)
        for i in range(len(chunk)):
            assert 'error' not in index[i], index[i]
            result.append(index[i]['result'])
    return result

def rpc(layer, m, p):
    return batch(layer, [(m, p)])[0]
num = lambda x: int(x, 16)
sent = list(csv.reader((run / 'sent.csv').open()))
mined = list(csv.reader((run / 'mined.csv').open()))
assert len(sent) == len(mined) == 870 and len({r[2] for r in sent}) == 870
assert {r[2] for r in sent} == {r[2] for r in mined}
assert all((num(r[4]) == 1 for r in mined))
source_meta = {h: {'job': job, 'layer': layer} for job, layer, h in sent}
old_height = {r[2]: int(r[3]) for r in mined}
blocks = {}
receipts = {}
sources = {}
postings = []
correlations = []
for layer in ['L1', 'L2']:
    assert num(rpc(layer, 'eth_chainId', [])) == chain_ids[layer]
    wanted = {h for h, v in source_meta.items() if v['layer'] == layer}
    heights = [old_height[h] for h in wanted]
    lower = min(heights) - 5
    upper = min(max(heights) + (100 if layer == 'L2' else 5), num(rpc(layer, 'eth_blockNumber', [])))
    rows = batch(layer, [('eth_getBlockByNumber', [hex(n), True]) for n in range(lower, upper + 1)])
    assert all(rows)
    blocks[layer] = rows
    interesting = [b for b in rows if any((t['hash'] in wanted or (layer == 'L1' and (t.get('to') or '').lower() == registry and (num(t['type']) == 3)) for t in b['transactions']))]
    recs = batch(layer, [('eth_getBlockReceipts', [b['number']]) for b in interesting])
    receipts[layer] = {b['hash']: r for b, r in zip(interesting, recs)}
    for b, rs in zip(interesting, recs):
        by_hash = {r['transactionHash']: r for r in rs}
        for i, t in enumerate(b['transactions']):
            h = t['hash']
            r = by_hash.get(h)
            assert r and r['blockHash'] == b['hash'] and (r['blockNumber'] == b['number']) and (num(r['transactionIndex']) == i)
            if h in wanted:
                assert num(r['status']) == 1 and h not in sources
                sources[h] = {**source_meta[h], 'hash': h, 'block_number': num(b['number']), 'block_hash': b['hash'], 'timestamp': num(b['timestamp']), 'gas_used': num(r['gasUsed']), 'gas_limit': num(t['gas']), 'recorded_height': old_height[h], 'moved': old_height[h] != num(b['number'])}
            if layer == 'L1' and (t.get('to') or '').lower() == registry and (num(t['type']) == 3):
                assert num(r['status']) == 1
                postings.append({'hash': h, 'block_number': num(b['number']), 'block_hash': b['hash'], 'timestamp': num(b['timestamp']), 'gas_used': num(r['gasUsed']), 'gas_limit': num(t['gas']), 'block_gas_used': num(b['gasUsed']), 'block_gas_limit': num(b['gasLimit']), 'blob_count': len(t.get('blobVersionedHashes', []))})
    assert wanted <= sources.keys(), f'Missing canonical {layer} sources: {len(wanted - sources.keys())}'
    recheck = batch(layer, [('eth_getBlockByNumber', [b['number'], False]) for b in rows])
    assert all((a['hash'] == b['hash'] for a, b in zip(rows, recheck)))
assert len(sources) == 870
for post in postings:
    ranges = rpc('L2', 'eez_getSettledL2RangesByL1Block', [post['block_hash']])
    correlations.append({'l1_block_hash': post['block_hash'], 'ranges': ranges})
    matching = [r for r in ranges if r['l1TransactionHash'] == post['hash']]
    assert matching and all((r['canonicalL2'] for r in matching))
    l2hashes = {b['hash'] for r in matching for b in r['l2Blocks']}
    inbound = [s for s in sources.values() if s['layer'] == 'L1' and s['block_hash'] == post['block_hash']]
    outbound = [s for s in sources.values() if s['layer'] == 'L2' and s['block_hash'] in l2hashes]
    assert len([p for p in postings if p['block_hash'] == post['block_hash']]) == 1, 'Multiple posts require finer inbound attribution'
    post.update(inbound=len(inbound), outbound=len(outbound), sources=len(inbound) + len(outbound), companion_gas_used=sum((s['gas_used'] for s in inbound)), companion_signed_limits=sum((s['gas_limit'] for s in inbound)))
    for s in inbound + outbound:
        assert 'post_batch_hash' not in s
        s['post_batch_hash'] = post['hash']
assert all(('post_batch_hash' in s for s in sources.values()))
posts = [p for p in postings if p['sources']]
low = min((p['block_number'] for p in posts))
high = max((p['block_number'] for p in posts))
window = [b for b in blocks['L1'] if low <= num(b['number']) <= high]
skipped = sum(((num(b['timestamp']) - num(a['timestamp'])) // 5 - 1 for a, b in zip(window, window[1:])))
phases = list(csv.DictReader((run / 'timings.csv').open()))
phase = {r['name']: r for r in phases}
seconds = sum((int(phase[n]['duration_ms']) for n in ['send', 'monitor'])) / 1000
summary = {'checked_at': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'run': str(run), 'source_count': len(sources), 'canonical_by_layer': {layer: sum((s['layer'] == layer for s in sources.values())) for layer in ports}, 'moved_source_heights': sum((s['moved'] for s in sources.values())), 'source_window_seconds': seconds, 'sources_per_second': 870 / seconds, 'postings_with_sources': len(posts), 'sources_per_post': [p['sources'] for p in posts], 'first_l1_block': low, 'last_l1_block': high, 'skipped_slots_between_first_last': skipped, 'produced_blocks_between_first_last': len(window), 'post_batch_gas_total': sum((p['gas_used'] for p in posts)), 'queue': rpc('L2', 'eez_getCrossChainQueueStatus', []), 'phases': phases}
post_times = {p['hash']: p['timestamp'] * 1000 for p in posts}
latency = [post_times[s['post_batch_hash']] - int(phase['send']['start_ms']) for s in sources.values()]
assert all((value >= 0 for value in latency))
summary['bulk_send_to_canonical_settlement_ms'] = {'p50': statistics.median(latency), 'p95': statistics.quantiles(latency, n=100, method='inclusive')[94], 'max': max(latency), 'send_duration_ms': int(phase['send']['duration_ms']), 'basis': 'Canonical postBatch timestamp minus bulk send start; per-source send uncertainty is at most the bulk send duration.'}
(out / 'canonical-snapshot.private.json').write_text(json.dumps({'blocks': blocks, 'receipts': receipts, 'correlations': correlations}, indent=2) + '\n')
(out / 'canonical-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
for name, rows in [('canonical-sources.csv', list(sources.values())), ('canonical-postings.csv', posts)]:
    with (out / name).open('w') as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0]))
        w.writeheader()
        w.writerows(rows)
print(json.dumps({k: v for k, v in summary.items() if k not in ['queue', 'phases']}))
