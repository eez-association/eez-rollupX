#!/usr/bin/env python3
"""Summarize first-attempt arrival, response and canonical observation records.

Success latency is conditional on success; failures and client omissions are
reported separately. Settled-in-window rate and offered-cohort drain rate have
different denominators. Neither is a saturation proof or a baseline comparison.
"""
import collections
import json
import math
from pathlib import Path
import statistics
import sys


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)] if values else None


def timings(rows, field='ms'):
    values = [r[field] for r in rows]
    return dict(count=len(values), p50=statistics.median(values) if values else None,
                p95=percentile(values, .95), p99=percentile(values, .99),
                maximum=max(values) if values else None)


def summarize(folder):
    def rows(name):
        p = folder / (name + '.jsonl')
        return [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []
    quotes, submissions, settlements = map(rows, ['quotes', 'submissions', 'settlements'])
    skips, observations = map(rows, ['generator-skips', 'observations'])
    result = dict(stages=[], byKind={}, note='Live candidate-only bounded ramp; loopback RPC; independent L1 checks, not independent L2 replay.')
    for stage in json.loads((folder / 'stages.json').read_text()):
        name = stage['name']
        duration = (stage['end'] - stage['begin']) / 1000
        q = [r for r in quotes if r['stage'] == name and r['usage'] == 'load']
        sq = [r for r in quotes if r['stage'] == name and r['usage'] == 'submission']
        s = [r for r in submissions if r['stage'] == name]
        settled = [r for r in settlements if r['stage'] == name]
        window = [r for r in settlements if stage['begin'] <= r['observedAt'] < stage['end']]
        omitted = [r for r in skips if r['label'].startswith(name + ':')]
        queue = [r for r in observations if stage['begin'] <= r['at'] < stage['end']]
        cohort_end = max([stage['end'], *[r['observedAt'] for r in settled]])
        result['stages'].append(dict(**stage, actualWindowSeconds=duration,
            quoteResponses=len(q), quoteCodes=dict(collections.Counter(r['code'] for r in q)),
            successfulQuotesPerSecond=sum(r['code'] == 'ok' for r in q)/duration,
            successfulQuoteLatencyMs=timings([r for r in q if r['code'] == 'ok']),
            submissionQuoteCodes=dict(collections.Counter(r['code'] for r in sq)),
            admissionCodes=dict(collections.Counter(r['code'] for r in s)),
            successfulAdmissionLatencyMs=timings([r for r in s if r['code'] == 'ok']),
            accepted=sum(r['code'] == 'ok' for r in s), settled=len(settled),
            settledDuringArrivalWindow=len(window), settledInWindowPerSecond=len(window)/duration,
            cohortIncludingDrainPerSecond=len(settled)/((cohort_end-stage['begin'])/1000),
            settlementObservationLatencyMs=timings(settled, 'latencyMs'),
            generatorSkips=dict(collections.Counter(r['label'] for r in omitted)),
            maxPendingTestSources=max([0, *[r['pendingTestSources'] for r in queue]]),
            maxQueue=max([0, *[r['queue']['queued']+r['queue']['inFlight'] for r in queue]])))
    for kind in ['nested', 'safe_create', 'safe_nested', 'safe_transfer']:
        q = [r for r in quotes if r['kind'] == kind and r['usage'] == 'load']
        s = [r for r in settlements if r['kind'] == kind and r['stage'] != 'canary']
        result['byKind'][kind] = dict(quoteCodes=dict(collections.Counter(r['code'] for r in q)),
            successfulQuoteLatencyMs=timings([r for r in q if r['code'] == 'ok']),
            settled=len(s), settlementObservationLatencyMs=timings(s, 'latencyMs'))
    if (folder / 'verification.json').exists():
        result['verification'] = json.loads((folder / 'verification.json').read_text())
    if (folder / 'run-end.json').exists():
        result['run'] = json.loads((folder / 'run-end.json').read_text())
    (folder / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


if __name__ == '__main__':
    result = summarize(Path(sys.argv[1]))
    for stage in result['stages']:
        print(json.dumps({k: stage[k] for k in ['name', 'quoteCodes', 'successfulQuoteLatencyMs',
            'admissionCodes', 'accepted', 'settled', 'cohortIncludingDrainPerSecond', 'generatorSkips', 'maxQueue']}))
