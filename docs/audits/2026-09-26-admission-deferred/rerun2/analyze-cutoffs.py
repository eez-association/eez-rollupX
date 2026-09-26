"""Read-only cohort report: join retained admission history to canonical sources.

Run after measure-wave.py and measure-admission.py, with their output directory.
Raw event snapshots stay in ignored artifacts; compact reports can be retained.
Attempt counts deliberately include retries; source counts use canonical joins.
"""
import collections
import csv
import datetime
import json
import re
import statistics
import subprocess
import sys
from pathlib import Path

out = Path(sys.argv[1])
report = out / 'detail'
report.mkdir(exist_ok=True)
run = Path(json.loads((out / 'protocol-run.json').read_text())['protocol_run'])
sent = list(csv.reader((run / 'sent.csv').open()))
assert len(sent) == 870 and len({r[2] for r in sent}) == 870
jobs = {r[2]: r[0] for r in sent}
canonical = list(csv.DictReader((out / 'canonical-sources.csv').open()))
settled = {(r['hash'], r['post_batch_hash']) for r in canonical}
source_by_hash = {r['hash']: r for r in canonical}
posts = list(csv.DictReader((out / 'canonical-postings.csv').open()))
post_hashes = {p['hash'] for p in posts}
phases = {r['name']: r for r in csv.DictReader((run / 'timings.csv').open())}


def sql(query):
    command = ['docker', 'exec', 'eez-monitoring-indexer-db-1', 'psql',
               '-U', 'eez_indexer', '-d', 'eez_indexer', '-At', '-v',
               'ON_ERROR_STOP=1', '-c',
               "SELECT coalesce(json_agg(row_to_json(t)), '[]'::json) FROM (" + query + ') t']
    return json.loads(subprocess.check_output(command, text=True))


def quoted(values, pattern):
    assert all(re.fullmatch(pattern, v) for v in values)
    return ','.join("'" + v + "'" for v in values)


def save(name, value):
    (report / name).write_text(json.dumps(value, indent=2) + '\n')


def table(name, rows):
    assert rows
    with (report / name).open('w', newline='') as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0]), lineterminator='\n')
        w.writeheader()
        for row in rows:
            w.writerow({k: json.dumps(v, sort_keys=True) if isinstance(v, (dict, list)) else v
                        for k, v in row.items()})


def median(values):
    values = [v for v in values if v is not None]
    return statistics.median(values) if values else None


hashes = quoted(list(jobs), r'0x[0-9a-f]{64}')
ids = [r['candidate_id'] for r in sql(
    "SELECT DISTINCT candidate_id FROM eez_indexer.admission_events "
    f"WHERE data->'detail'->>'source_hash' IN ({hashes})")]
assert ids
id_list = quoted(ids, r'[0-9:-]+')
candidates = sql(f'SELECT * FROM eez_indexer.admission_candidates WHERE candidate_id IN ({id_list}) ORDER BY time')
events = sql(f'SELECT candidate_id, sequence, data FROM eez_indexer.admission_events WHERE candidate_id IN ({id_list}) ORDER BY candidate_id, sequence')
sources = sql(f'SELECT * FROM eez_indexer.admission_sources WHERE candidate_id IN ({id_list}) ORDER BY time, position')
assert len(candidates) == len(ids) and all(c['history'] == 'Complete sequence' for c in candidates)
assert len({(s['candidate_id'], s['source_hash']) for s in sources}) == len(sources)
save('all-candidates.json', candidates)
save('events.private.json', events)
window_candidates = sql('SELECT * FROM eez_indexer.admission_candidates WHERE time >= '
    f"to_timestamp({int(phases['send']['start_ms'])}/1000.0) AND time <= "
    f"to_timestamp({max(int(p['timestamp']) for p in posts) + 1}) ORDER BY time")
save('window-candidates.json', window_candidates)
by_candidate = collections.defaultdict(list)
for e in events:
    by_candidate[e['candidate_id']].append(e['data'])
attempt_rows, rejection_rows = [], []
for c in candidates:
    ev = by_candidate[c['candidate_id']]
    detail = {e['kind']: e['detail'] for e in ev}
    summary = detail.get('summary', {})
    proof = detail.get('proof_requested', {})
    accepted = {e['detail']['source_hash'] for e in ev if e['kind'] == 'source_accepted'}
    estimates = [e['detail'] for e in ev if e['kind'] == 'source_estimate']
    charges = [e for e in estimates if e['source_hash'] in accepted]
    if charges:
        # No allocation of aggregate receipt gas to individual sources is made.
        assert sum(e['projected_after'] - e['projected_before'] for e in charges) + charges[0]['projected_before'] == c['admission_gas']
    row = {k: c[k] for k in ['candidate_id', 'target_block', 'target_time', 'transaction_hash',
                            'canonical_l1_block', 'queued', 'selected', 'selection_limit',
                            'selection_limit_reason', 'accepted', 'admission_stop_reason',
                            'reason', 'admission_gas', 'planning_cap', 'full_projection_gas',
                            'preflight_gas_used', 'preflight_gas_required', 'signed_gas_limit',
                            'canonical_receipt_gas', 'preparation_ms', 'nominal_preparation_ms',
                            'proof_ms', 'finalization_ms', 'dispatch_remaining_ms', 'delivery_ms',
                            'relay_acknowledged']}
    row.update(canonical=c['transaction_hash'] in post_hashes)
    post = next((p for p in posts if p['hash'] == c['transaction_hash']), None)
    row['canonical_companion_gas'] = int(post['companion_gas_used']) if post else None
    row['canonical_bundle_gas'] = int(post['gas_used']) + int(post['companion_gas_used']) if post else None
    row['canonical_block_gas_limit'] = int(post['block_gas_limit']) if post else None
    for k in ['examined_count', 'not_examined_count', 'selection_filtered_count',
              'companion_gas', 'whole_block_cap', 'cold_timing_probe']:
        row[k] = summary.get(k)
    for k in ['proof_estimate_ms', 'proof_estimate_basis', 'post_reserve_ms', 'remaining_ms']:
        row[k] = proof.get(k)
    for k in ['witnesses', 'blobs']:
        row[k] = proof.get(k)
    row['failed_proof_ms'] = detail.get('proof_failed', {}).get('proof_ms')
    row['initial_projection_gas'] = charges[0]['projected_before'] if charges else None
    row['initial_queue_cleanup_gas'] = charges[0]['initial_queue_cleanup'] if charges else None
    row['entry_reserve_gas'] = sum(e['entry_reserve_gas'] for e in charges)
    row['calldata_gas'] = sum(e['calldata_gas'] for e in charges)
    row['target_simulation_gas'] = sum(e['l1_target_sim_gas'] for e in charges)
    row['gas_estimates_rejected'] = sum(not e['gas_gate_accepted'] for e in estimates)
    attempt_rows.append(row)
    for i, e in enumerate(estimates):
        if e['gas_gate_accepted']:
            continue
        prior = [a for a in estimates[:i] if a['source_hash'] in accepted]
        # Revocation restores prior accepted discounts as well as this group's.
        delta = e['entry_pin_gas'] + e['calldata_gas'] + e['l1_target_sim_gas']
        delta += sum(a['entry_discount'] for a in prior) if e['batch_discount_revoked'] else -e['entry_discount']
        proposed = e['projected_before'] + delta
        used = required = 0
        for a in prior + [e]:
            required = max(required, used + a['companion_declared_gas'])
            used += a['companion_sim_gas']
        rejection_rows.append({
            'candidate_id': c['candidate_id'], 'canonical': c['transaction_hash'] in post_hashes,
            'source_hash': e['source_hash'], 'job': jobs.get(e['source_hash']),
            'constraint': e['constraint'], 'accepted_prefix_sources': len(prior),
            'prefix_gas': e['projected_before'], 'next_group_gas': delta,
            'proposed_postbatch_gas': proposed, 'planning_cap': e['postbatch_planning_cap'],
            'postbatch_over_cap': max(0, proposed - e['postbatch_planning_cap']),
            'proposed_companion_requirement': required,
            'drain_margin': e['fixed_drain_reserve_gas'],
            'proposed_bundle_requirement': proposed + required + e['fixed_drain_reserve_gas'],
            'whole_block_cap': e['whole_block_cap'],
            'bundle_over_cap': max(0, proposed + required + e['fixed_drain_reserve_gas'] - e['whole_block_cap']),
            'batch_discount_revoked': e['batch_discount_revoked']})
table('candidate-cutoffs.csv', attempt_rows)
if rejection_rows:
    table('gas-rejections.csv', rejection_rows)
save('gas-rejections.json', rejection_rows)

accepted_sources = []
for s in sources:
    if s['decision'] != 'Accepted' or (s['source_hash'], s['postbatch_hash']) not in settled:
        continue
    accepted_sources.append({'job': jobs[s['source_hash']], **s,
                             'canonical_source_execution_gas': int(source_by_hash[s['source_hash']]['gas_used']),
                             'canonical_source_layer': source_by_hash[s['source_hash']]['layer']})
assert len(accepted_sources) == len(settled)
table('source-estimates.csv', accepted_sources)
families = []
for family in sorted({j.rsplit('-', 1)[0] for j in jobs.values()}):
    rows = [s for s in accepted_sources if s['job'].rsplit('-', 1)[0] == family]
    if not rows:
        continue
    stats = {'family': family, 'sources': len(rows), 'direction': rows[0]['direction']}
    for k in ['entry_count', 'entry_reserve_gas', 'calldata_gas', 'l1_target_simulation_gas',
              'accepted_delta_gas', 'source_simulation_gas', 'source_preflight_gas',
              'source_required_gas', 'canonical_source_receipt_gas', 'canonical_source_execution_gas']:
        values = [s[k] for s in rows if s[k] is not None]
        stats[k + '_mean'] = sum(values) / len(values) if values else None
    stats['estimate_reasons'] = dict(sum((collections.Counter(s['estimate_reasons']) for s in rows), collections.Counter()))
    stats['batch_discount_revoked_sources'] = sum(s['batch_discount_revoked'] for s in rows)
    families.append(stats)
table('source-families.csv', families)
save('source-families.json', families)

cutoffs = []
for reason in sorted({c['admission_stop_reason'] for c in candidates}):
    all_rows = [r for r in attempt_rows if r['admission_stop_reason'] == reason]
    rows = [r for r in all_rows if r['canonical']]
    gas = {k: sum(r[k] or 0 for r in rows) for k in ['admission_gas', 'preflight_gas_required', 'canonical_receipt_gas']}
    cutoffs.append({'reason': reason, 'all_candidates': len(all_rows), 'canonical_candidates': len(rows),
                    'canonical_sources': sum(r['accepted'] for r in rows),
                    'accepted_min': min((r['accepted'] for r in rows), default=None),
                    'accepted_max': max((r['accepted'] for r in rows), default=None),
                    'preparation_ms_median': median(r['preparation_ms'] for r in rows),
                    'proof_prediction_ms_median': median(r['proof_estimate_ms'] for r in rows),
                    'proof_ms_median': median(r['proof_ms'] for r in rows),
                    'dispatch_remaining_ms_median': median(r['dispatch_remaining_ms'] for r in rows),
                    'planning_utilization_mean': statistics.mean(r['admission_gas'] / r['planning_cap'] for r in rows) if rows else None,
                    **gas, 'admission_receipt_ratio': gas['admission_gas'] / gas['canonical_receipt_gas'] if gas['canonical_receipt_gas'] else None})
save('cutoff-summary.json', cutoffs)
table('cutoff-summary.csv', cutoffs)
canonical_candidates = [c for c in candidates if c['transaction_hash'] in post_hashes]
gas = {k: sum(c[k] for c in canonical_candidates) for k in ['admission_gas', 'full_projection_gas', 'preflight_gas_used', 'preflight_gas_required', 'signed_gas_limit', 'canonical_receipt_gas']}
summary = {'checked_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
           'cohort_basis': 'candidates that examined at least one of the 870 fresh wave sources',
           'attempted_sources': len(sent), 'canonical_sources': len(canonical),
           'all_candidates': len(candidates), 'canonical_candidates': len(canonical_candidates),
           'candidates_in_send_to_last_settlement_window': len(window_candidates),
           'window_candidates_without_wave_source_events': [c['candidate_id'] for c in window_candidates if c['candidate_id'] not in ids],
           'all_sequences_complete': True, 'source_attempt_rows': len(sources),
           'unique_wave_sources_with_estimates': len({s['source_hash'] for s in sources if s['entry_count'] is not None and s['source_hash'] in jobs}),
           'source_decisions_across_attempts': dict(collections.Counter(s['decision'] for s in sources)),
           'estimate_reasons_canonical_entries': dict(sum((collections.Counter(s['estimate_reasons']) for s in accepted_sources), collections.Counter())),
           'batch_discount_revoked_canonical_sources': sum(s['batch_discount_revoked'] for s in accepted_sources),
           'cold_timing_probes': sum(r['cold_timing_probe'] for r in attempt_rows),
           'gas': gas, 'admission_receipt_ratio': gas['admission_gas'] / gas['canonical_receipt_gas'],
           'admission_required_ratio': gas['admission_gas'] / gas['preflight_gas_required'],
           'preflight_receipt_max_abs_difference': max(abs(c['preflight_gas_used'] - c['canonical_receipt_gas']) for c in canonical_candidates),
           'preflight_required_fits_signed_limits': all(c['preflight_gas_required'] <= c['signed_gas_limit'] for c in candidates if c['preflight_gas_required'] is not None),
           'nominal_preparation_overruns': sum(c['preparation_ms'] > c['nominal_preparation_ms'] for c in candidates if c['preparation_ms'] is not None)}
save('summary.json', summary)
print(json.dumps(summary, indent=2))
