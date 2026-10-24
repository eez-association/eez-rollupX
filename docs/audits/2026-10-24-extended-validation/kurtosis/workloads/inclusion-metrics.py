"""Measure local inclusion separately from canonical L1 settlement.

Inputs are retained results of the exact workload generators in this directory.
Only commits matching the final receipt's block hash count. The generators and
the independent evidence collector own canonicality and accounting assertions;
this read-only analysis adds wall-clock timing, not another execution oracle.
"""

import argparse
import datetime
import hashlib
import json
from pathlib import Path
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--node-log', type=Path, required=True)
parser.add_argument('--phase', action='append', required=True, metavar='NAME=RUN_DIRECTORY')
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
assert not args.output.exists(), 'Preserve the first result; select a new output path.'

commits = {}
for line in args.node_log.read_text().splitlines():
    try:
        row = json.loads(line)
    except ValueError:
        continue
    fields = row.get('fields', {})
    if fields.get('message') == 'Canonical chain committed':
        at = datetime.datetime.fromisoformat(row['timestamp'].replace('Z', '+00:00')).timestamp()
        commits.setdefault(fields['hash'], []).append((at, fields['number']))

sources = set()
phases = {}
for phase in args.phase:
    name, path = phase.split('=', 1)
    assert name and name not in phases
    directory = Path(path) / 'ordinary'
    summary = json.loads((directory / 'summary.json').read_text())
    assert summary['verified_receipts_and_accounting'] and summary['error'] is None
    sources.add(json.loads((directory / 'metadata.json').read_text())['runtime_source'])
    accepted = json.loads((directory / 'accepted.json').read_text())
    receipts = {row['transactionHash']: row for row in json.loads((directory / 'receipts.json').read_text())}
    assert accepted and len(receipts) == len(accepted)
    values = []
    for transaction in accepted:
        receipt = receipts[transaction['hash']]
        events = commits.get(receipt['blockHash'], [])
        assert events, ('No exact commit event', transaction['hash'])
        assert all(number == int(receipt['blockNumber'], 16) for _, number in events)
        elapsed = min(at for at, _ in events) - transaction['submitted_at']
        assert elapsed >= 0, ('Commit precedes submission', transaction['hash'], elapsed)
        values.append(elapsed)
    values.sort()
    phases[name] = {
        'local_inclusion_seconds': {
            'count': len(values), 'min': values[0], 'p50': statistics.median(values),
            'p95': values[min(len(values) - 1, int(.95 * len(values)))],
            'max': values[-1], 'mean': statistics.mean(values),
        },
        'missing_count': 0, 'negative': [],
    }
assert len(sources) == 1, 'Do not combine different runtime revisions.'
result = {
    'runtime_source': sources.pop(),
    'node_log_sha256': hashlib.file_digest(args.node_log.open('rb'), 'sha256').hexdigest(),
    'scope': 'Elapsed wall-clock time from the sole RPC send to the first Canonical chain committed event with the exact final canonical receipt block hash. This is local inclusion observation, not L1 settlement.',
    'phases': phases,
}
args.output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
