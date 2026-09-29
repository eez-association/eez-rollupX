"""Reconcile per-source capacity deferrals and companion estimates for this wave."""
import collections
import csv
import json
import sys
from pathlib import Path

out = Path(sys.argv[1])
detail = out / 'detail'
candidates = json.loads((detail / 'all-candidates.json').read_text())
events = json.loads((detail / 'events.private.json').read_text())
sources = list(csv.DictReader((detail / 'source-estimates.csv').open()))
posts = list(csv.DictReader((out / 'canonical-postings.csv').open()))
canonical = {p['hash'] for p in posts}
by_candidate = collections.defaultdict(list)
for event in events:
    by_candidate[event['candidate_id']].append(event['data'])
assert all(c['policy_version'] == 'packing-minimum-check-3' for c in candidates)
assert all(c['estimator_version'] == 'companion-storage-6' for c in candidates)
assert all(c['selection_limit_reason'] != 'fifo_fairness_limit' for c in candidates)
assert all(c['admission_stop_reason'] != 'fifo_fairness_limit' for c in candidates)

capacity_reasons = {'l1_tx_gas_budget', 'l1_bundle_gas_budget',
                    'l2_block_capacity', 'composition_blob_limit'}
deferrals = collections.Counter()
canonical_deferrals = collections.Counter()
packing_rows = []
for candidate in candidates:
    history = by_candidate[candidate['candidate_id']]
    declared = candidate['capacity_deferrals']
    if declared is None:
        assert not any(e['kind'] == 'summary' for e in history)
        continue
    outcomes = [e for e in history if e['kind'] == 'source_outcome'
                and e['detail']['reason'] in capacity_reasons]
    observed = collections.Counter(e['detail']['reason'] for e in outcomes)
    assert dict(observed) == declared, (candidate['candidate_id'], observed, declared)
    deferrals.update(observed)
    if candidate['transaction_hash'] in canonical:
        canonical_deferrals.update(observed)
    first_miss = next((i for i, e in enumerate(history) if e in outcomes), None)
    if first_miss is not None:
        later = [e for e in history[first_miss + 1:] if e['kind'] == 'source_accepted']
        packing_rows.append({'candidate_id': candidate['candidate_id'],
            'canonical': candidate['transaction_hash'] in canonical,
            'postbatch_hash': candidate['transaction_hash'],
            'accepted_after_first_capacity_deferral': len(later),
            'capacity_deferrals': declared, 'fifo_bypassed': candidate['fifo_bypassed'],
            'final_stop_reason': candidate['admission_stop_reason']})

inbound = [s for s in sources if s['direction'] == 'inbound']
assert len(inbound) == 520 and len(sources) == 870
groups = collections.defaultdict(list)
for source in inbound:
    assert source['source_receipt_coverage'] == 'available'
    estimate = int(source['companion_estimate_gas'])
    limit = int(source['companion_declared_gas'])
    basis = source['companion_estimate_basis']
    if basis == 'declared_limit':
        assert estimate == limit
    else:
        assert basis in {'bounded_protocol', 'bounded_capped'}
        predicted = sum(int(source[k]) for k in ['companion_simulation_gas',
                        'companion_target_simulation_gas', 'companion_protocol_reserve_gas'])
        assert estimate == min(limit, predicted)
    groups[('basis', basis)].append(source)
    groups[('family', source['job'].rsplit('-', 1)[0])].append(source)

companion_rows = []
for (kind, name), rows in sorted(groups.items()):
    estimate = sum(int(s['companion_estimate_gas']) for s in rows)
    actual = sum(int(s['canonical_source_receipt_gas']) for s in rows)
    companion_rows.append({'group': kind, 'name': name, 'sources': len(rows),
        'estimate_gas': estimate, 'receipt_gas': actual, 'estimate_receipt_ratio': estimate / actual,
        'source_simulation_gas': sum(int(s['companion_simulation_gas']) for s in rows),
        'callback_simulation_gas': sum(int(s['companion_target_simulation_gas']) for s in rows),
        'protocol_reserve_gas': sum(int(s['companion_protocol_reserve_gas'] or 0) for s in rows),
        'basis_counts': dict(collections.Counter(s['companion_estimate_basis'] for s in rows))})

summary = {'canonical_postbatches': len(posts), 'canonical_sources': len(sources),
    'sources_per_postbatch_mean': len(sources) / len(posts),
    'capacity_deferrals_all_candidates': dict(deferrals),
    'capacity_deferrals_canonical_candidates': dict(canonical_deferrals),
    'candidates_accepting_work_after_capacity_deferral': sum(r['accepted_after_first_capacity_deferral'] > 0 for r in packing_rows),
    'canonical_candidates_accepting_work_after_capacity_deferral': sum(r['canonical'] and r['accepted_after_first_capacity_deferral'] > 0 for r in packing_rows),
    'canonical_sources_accepted_after_capacity_deferral': sum(r['accepted_after_first_capacity_deferral'] for r in packing_rows if r['canonical']),
    'fifo_bypassed_candidates': sum(c['fifo_bypassed'] is True for c in candidates),
    'companion_estimate_gas': sum(int(s['companion_estimate_gas']) for s in inbound),
    'companion_receipt_gas': sum(int(s['canonical_source_receipt_gas']) for s in inbound),
    'all_companion_components_reconciled': True, 'all_capacity_deferrals_reconciled': True}
for name, value in [('packing-summary.json', summary), ('packing-candidates.json', packing_rows),
                    ('companion-breakdown.json', companion_rows)]:
    (detail / name).write_text(json.dumps(value, indent=2) + '\n')
for name, rows in [('packing-candidates.csv', packing_rows), ('companion-breakdown.csv', companion_rows)]:
    if rows:
        with (detail / name).open('w', newline='') as stream:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0]), lineterminator='\n')
            writer.writeheader()
            writer.writerows({key: json.dumps(value, sort_keys=True) if isinstance(value, dict) else value
                              for key, value in row.items()} for row in rows)
print(json.dumps(summary, indent=2))
