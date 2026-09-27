"""Attribute retained preparation intervals without inventing missing CPU timings.

Usage: profile-preparation.py WAVE_DIRECTORY OUTPUT_DIRECTORY [PREPARED_JSON]
PREPARED_JSON supplies baseline KZG timing absent from old admission events.
"""
import collections
import csv
import json
import statistics
import sys
from pathlib import Path

wave, out = map(Path, sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
candidates = json.loads((wave / 'admission-candidates.json').read_text())
events = json.loads((wave / 'detail/events.private.json').read_text())
prepared = (json.loads(Path(sys.argv[3]).read_text()) if len(sys.argv) > 3 else [])
prepared = {row['candidate_id']: row for row in prepared}
by_candidate = collections.defaultdict(list)
for event in events:
    by_candidate[event['candidate_id']].append(event['data'])
rows = []
for candidate in candidates:
    history = sorted(by_candidate[candidate['candidate_id']], key=lambda e: e['sequence'])
    assert [e['sequence'] for e in history] == list(range(len(history)))
    stages = {e['kind']: e for e in history}
    sources = collections.defaultdict(dict)
    for event in history:
        source = event['detail'].get('source_hash')
        if source:
            sources[source][event['kind']] = event
    parts = dict.fromkeys(['accepted_checks_simulation_ms', 'accepted_lowering_hashing_append_ms',
        'rejected_checks_simulation_ms', 'rejection_after_estimate_ms',
        'cheap_checks_outcome_ms'], 0.0)
    for source in sources.values():
        start = source['source_selected']['elapsed_ms']
        if 'source_estimate' in source:
            estimate = source['source_estimate']['elapsed_ms']
            accepted = 'source_accepted' in source
            end = source.get('source_accepted', source.get('source_outcome'))['elapsed_ms']
            assert start <= estimate <= end
            parts['accepted_checks_simulation_ms' if accepted else 'rejected_checks_simulation_ms'] += estimate - start
            parts['accepted_lowering_hashing_append_ms' if accepted else 'rejection_after_estimate_ms'] += end - estimate
        else:
            parts['cheap_checks_outcome_ms'] += source['source_outcome']['elapsed_ms'] - start
    parts['other_admission_ms'] = stages['summary']['elapsed_ms'] - sum(parts.values())
    parts['final_build_encode_projection_ms'] = stages['full_projection']['elapsed_ms'] - stages['summary']['elapsed_ms']
    finishing = stages['proof_requested']['elapsed_ms'] - stages['full_projection']['elapsed_ms']
    request = stages['proof_requested']['detail']
    blob = request.get('blob_preparation_ms')
    if blob is None and candidate['candidate_id'] in prepared:
        record = prepared[candidate['candidate_id']]
        assert record['accepted'] == stages['summary']['detail']['accepted']
        assert abs(record['preparation_ms'] - request['preparation_ms']) <= 2
        blob = record['blob_preparation_ms']
    if blob is None:
        parts['kzg_witness_request_ms'] = finishing
    else:
        parts['blob_preparation_ms'] = blob
        parts['witness_and_remaining_request_ms'] = finishing - blob
    assert min(parts.values()) >= 0
    total = stages['proof_requested']['elapsed_ms']
    assert abs(sum(parts.values()) - total) < 1e-6
    rows.append({'candidate_id': candidate['candidate_id'],
        'l1_block': candidate['canonical_l1_block'],
        'accepted': stages['summary']['detail']['accepted'],
        'reason': candidate['admission_stop_reason'], **parts,
        'total_preparation_ms': total,
        'prefix_hash_ms': stages['summary']['detail'].get('prefix_hash_ms'),
        'prefix_hash_executed_tx_count': stages['summary']['detail'].get('prefix_hash_executed_tx_count'),
        'witness_preparation_ms': request.get('witness_preparation_ms')})
assert rows and sum(r['accepted'] for r in rows) == 870
keys = [k for k in rows[0] if k.endswith('_ms')]
summary = {'canonical_candidates': len(rows), 'canonical_sources': 870, 'intervals': {}}
for key in keys:
    values = [r[key] for r in rows if r[key] is not None]
    summary['intervals'][key] = {'samples': len(values),
        'mean': statistics.mean(values) if values else None,
        'median': statistics.median(values) if values else None,
        'max': max(values) if values else None,
        'sum': sum(values) if values else None}
summary['limits'] = [
    'Wall-clock event intervals include logging, checks and scheduling, not isolated CPU samples.',
    'Prefix hashing overlaps source intervals, including work later deferred by timing; never add it to the interval total.',
    'Witness timing includes blocking-pool wait; historical blob logs are integer milliseconds.',
    'Canonical candidates only; failed/excluded attempts are outside this breakdown.']
with (out / 'preparation-per-post.csv').open('w') as stream:
    writer = csv.DictWriter(stream, fieldnames=list(rows[0]), lineterminator='\n')
    writer.writeheader()
    writer.writerows(rows)
(out / 'preparation-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary, indent=2))
