"""Summarize observed admission, liveness and proof timings, without rates inferred from plans."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import statistics

parser = argparse.ArgumentParser()
parser.add_argument('--run', type=Path, required=True)
parser.add_argument('--evidence', type=Path, required=True)
parser.add_argument('--node-log', type=Path, required=True)
parser.add_argument('--signer-log', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()


def lines(path):
    rows = []
    for line in path.read_text().splitlines():
        try:
            rows.append(json.loads(line))
        except ValueError:
            pass
    return rows


def spread(values):
    if not values:
        return {'count': 0}
    values = sorted(values)
    return {'count': len(values), 'min': values[0], 'p50': statistics.median(values),
        'p95': values[min(len(values) - 1, int(.95 * len(values)))], 'max': values[-1],
        'mean': statistics.mean(values)}


def timestamp(value):
    return datetime.datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()


meta = json.loads((args.run / 'ordinary/metadata.json').read_text())
summary = json.loads((args.run / 'ordinary/summary.json').read_text())
assert summary['verified_receipts_and_accounting'] and summary['error'] is None
accepted = json.loads((args.run / 'ordinary/accepted.json').read_text())
receipts = json.loads((args.run / 'ordinary/receipts.json').read_text())
observations = lines(args.run / 'ordinary/observations.jsonl')
events = lines(args.run / 'ordinary/events.jsonl')
resources = [r for r in lines(args.run / 'resources.jsonl') if meta['started_at'] <= r['at'] <= meta['ended_at']]
evidence = json.loads(args.evidence.read_text())
assert evidence['verified'] and evidence['runtime_source'] == meta['runtime_source']
post_by_hash = {r['settlement_claim']['post_block_hash']: r for r in evidence['verified_blob_batches']}
canonical = {r['block']['number']: r['block']['hash']
    for r in json.loads((args.evidence.parent / 'all-measured-block-receipts.json').read_text())}
safe_events, prepared, signer = [], [], []
for row in lines(args.node_log):
    fields = row.get('fields', {})
    name = fields.get('event_name')
    at = timestamp(row['timestamp']) if 'timestamp' in row else None
    if name == 'eez.deriver.safe.advanced':
        post = post_by_hash.get(fields['new_safe_hash'])
        if post:
            assert fields['tx_hash'] == post['l1_transaction_hash']
            assert fields['l1_block_number'] == post['l1_block_number']
            safe_events.append((at, fields['to_block'], fields['new_safe_hash']))
    elif name == 'eez.composer.posting.v1':
        event = json.loads(fields['posting_event'])
        if event['stage'] == 'prepared' and event['posting_id'] in post_by_hash:
            detail = event['detail']
            if meta['started_at'] <= at <= meta['ended_at']:
                prepared.append({'at': at, 'attempt_id': event['attempt_id'], 'post_hash': event['posting_id'],
                    'proof_roundtrip_ms': detail['proof_received_ms'] - detail['proof_requested_ms'],
                    'preparation_to_proof_ms': detail['proof_received_ms'] - detail['preparation_started_ms']})
for row in lines(args.signer_log):
    fields = row.get('fields', {})
    if fields.get('event_name') == 'eez.proof_signer.window_signed':
        post = post_by_hash.get(fields['validated_window_post_block_hash'])
        at = timestamp(row['timestamp'])
        if post and meta['started_at'] <= at <= meta['ended_at']:
            assert fields['validated_window_pre_block_hash'] == post['settlement_claim']['pre_block_hash']
            signer.append({'at': at, 'request_id': row.get('span', {}).get('request_id'),
                'post_hash': fields['validated_window_post_block_hash'], 'elapsed_ms': fields.get('elapsed_ms')})

receipt_by_hash = {r['transactionHash']: r for r in receipts}
latencies, unavailable = [], []
for tx in accepted:
    receipt = receipt_by_hash[tx['hash']]
    number = int(receipt['blockNumber'], 16)
    candidates = [at for at, end, _ in safe_events if end >= number and at >= tx['submitted_at']]
    if candidates:
        latencies.append(min(candidates) - tx['submitted_at'])
    else:
        unavailable.append(tx['hash'])
queue = [r['queue'] for r in observations if 'queue' in r]
assert observations and resources and queue and prepared and signer
per_stage = []
for index, stage in enumerate(meta['stages']):
    count = sum(row['stage'] == index for row in accepted)
    per_stage.append({**stage, 'planned_offers': stage['rate'] * stage['seconds'], 'accepted': count,
        'accepted_per_second': count / stage['seconds'],
        'expired_schedule_slots': sum(row['kind'] == 'scheduler_missed_offer' and row['stage'] == index for row in events),
        'backpressure_slots': sum(row['kind'] == 'backpressure' and row['stage'] == index for row in events)})
role_ids = [r['ID'] for r in resources[0]['owned']]
result = {'runtime_source': meta['runtime_source'], 'enclave': meta['enclave'],
    'interval': {k: meta[k] for k in ['started_at', 'ended_at', 'elapsed_seconds']},
    'admission': {'planned': summary['planned'], 'offered': summary['offered'],
        'accepted': len(accepted), 'per_stage': per_stage, 'retries': 0,
        'rpc_response_ms': spread([1000 * (r['at'] - r['submitted_at']) for r in events if r['kind'] == 'accepted'])},
    'liveness': {'observed_head_age_seconds': spread([r['at'] - r['head_timestamp'] for r in observations]),
        'head_minus_safe_blocks': spread([r['head_number'] - r['safe_number'] for r in observations]),
        'pending_ordinary_max': max(r['pending'] for r in observations),
        'queue_max': {key: max(q[key] for q in queue) for key in ['queued', 'inFlight', 'blocked', 'unknown']},
        'observed_same_height_replacements': sum(r['kind'] == 'unsafe_replacement' for r in events),
        'canonical_header_cadence_seconds': 2,
        'scope': 'One-second sampling and final canonical ancestry; future block timestamps cover intentional intervals between commits.'},
    'settlement': {'submission_to_canonical_safe_seconds': spread(latencies),
        'latency_without_matching_log_count': len(unavailable), 'all_receipts_canonical_and_safe': True},
    'proving': {'canonical_window_signer_ms': spread([r['elapsed_ms'] for r in signer if r['elapsed_ms'] is not None]),
        'window_duration_unavailable_count': sum(r['elapsed_ms'] is None for r in signer),
        'request_context_unavailable_count': sum(r['request_id'] is None for r in signer),
        'canonical_attempt_roundtrip_ms': spread([r['proof_roundtrip_ms'] for r in prepared]),
        'preparation_through_proof_ms': spread([r['preparation_to_proof_ms'] for r in prepared]),
        'signed_windows': signer, 'prepared_attempts': prepared},
    'resources': {'host_available_min_bytes': min(r['mem_available_bytes'] for r in resources),
        'container_cpu_percent': {cid: spread([float(role['CPUPerc'].rstrip('%'))
            for sample in resources for role in sample['owned'] if role['ID'] == cid]) for cid in role_ids},
        'container_memory_samples': {cid: sorted(set(role['MemUsage'].split(' / ')[0]
            for sample in resources for role in sample['owned'] if role['ID'] == cid)) for cid in role_ids}},
    'evidence_sha256': hashlib.file_digest(args.evidence.open('rb'), 'sha256').hexdigest(),
    'limits': ['Shared host and independently bootstrapped chains; not a maximum-capacity benchmark.',
        'Offered rate and actual accepted throughput are separate.',
        'CPU samples use Docker percent of one logical CPU, not normalized whole-host utilization.',
        'Safe latency uses canonical hash-bound node events; first unsafe receipt observation is not canonical settlement.',
        'Cold follower result is recorded separately.',
        'Canonical signer windows are joined by exact pre/post hashes. Missing request context or duration is unavailable, never inferred from event proximity or per-block timings.']}
args.output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({k: result[k] for k in ['admission', 'liveness', 'settlement']}))
