"""Classify attempted targets from a retained canonical wave snapshot.

Usage: classify-targets.py WAVE_DIRECTORY OUTPUT_JSON
This distinguishes skipped slots from produced blocks lacking the candidate.
It cannot establish why a builder omitted an acknowledged bundle.
"""
import collections
import datetime
import json
import sys
from pathlib import Path

wave = Path(sys.argv[1])
candidates = json.loads((wave / 'detail/all-candidates.json').read_text())
blocks = json.loads((wave / 'canonical-snapshot.private.json').read_text())['blocks']['L1']
by_time = {int(b['timestamp'], 16): b for b in blocks}
assert len(by_time) == len(blocks)
rows = []
for candidate in candidates:
    timestamp = int(datetime.datetime.fromisoformat(candidate['target_time']).timestamp())
    assert min(by_time) < timestamp < max(by_time), 'Snapshot must bracket every target'
    block = by_time.get(timestamp)
    if candidate['canonical_l1_block'] is not None:
        assert block and int(block['number'], 16) == candidate['canonical_l1_block']
        assert any(t['hash'] == candidate['transaction_hash'] for t in block['transactions'])
        outcome = 'canonical_candidate'
    elif block is None:
        outcome = 'skipped_slot'
    else:
        assert all(t['hash'] != candidate['transaction_hash'] for t in block['transactions'])
        outcome = ('produced_without_candidate' if int(block['number'], 16) == candidate['target_block']
                   else 'different_canonical_height')
    rows.append({'candidate_id': candidate['candidate_id'], 'target_block': candidate['target_block'],
                 'target_time': candidate['target_time'], 'outcome': outcome,
                 'accepted': candidate['accepted'], 'relay_acknowledged': candidate['relay_acknowledged'],
                 'dispatch_remaining_ms': candidate['dispatch_remaining_ms'],
                 'delivery_ms': candidate['delivery_ms']})
summary = {'outcomes': dict(collections.Counter(r['outcome'] for r in rows)),
           'accepted_sources_in_noncanonical_attempts': sum(r['accepted'] for r in rows if r['outcome'] != 'canonical_candidate'),
           'limit': 'Canonical observations distinguish skipped targets and omitted candidates; relay acknowledgment does not establish builder inclusion or its omission reason.',
           'attempts': rows}
Path(sys.argv[2]).write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps({k: v for k, v in summary.items() if k != 'attempts'}, indent=2))
