"""Quality checks for the explicitly non-shipping HeldPool guard probe.

The probe timestamps hold end immediately before unlocking and logs afterwards.
These are acquisition/guard-boundary measurements, not kernel-instruction costs.
"""
from collections import defaultdict

from overhead_analysis import quantile

LIMIT = 65_536
SAMPLE = 'eez.benchmark.held_pool_sample'
ERRORS = {'eez.benchmark.held_pool_overflow', 'eez.benchmark.held_pool_clock_error'}
FIELDS = ('sequence', 'pool', 'origin_unix_before_ns', 'origin_unix_after_ns',
          'started_ns', 'acquired_ns', 'released_ns', 'wait_ns', 'hold_ns',
          'clock_before_ns', 'observed_unix_ns', 'clock_after_ns',
          'callsite_line', 'callsite_column')


def integer(value):
    if isinstance(value, str) and value.isascii() and value.isdecimal():
        value = int(value)
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value < 2**64:
        raise ValueError('probe values must be exact unsigned integers')
    return value


def distribution(samples):
    def summary(key):
        values = [sample[key] for sample in samples]
        return {'p50_ns': quantile(values, .5), 'p95_ns': quantile(values, .95),
                'maximum_ns': max(values), 'sum_ns': sum(values)}
    return {'count': len(samples), 'wait': summary('wait_ns'), 'hold': summary('hold_ns')}


def analyze(events, phases, *, clock_slack_ns=1_000_000, boundary_guard_ns=1_000_000):
    """Require complete logs and bracketed phase populations; never impute gaps.

    `events` contains parsed probe JSON events from one complete process log.
    `phases` contains unique name/start_unix_ns/end_unix_ns markers from the
    workload harness. Caller records the frozen probe binary/config identity.
    """
    report = {'status': 'inconclusive', 'sample_scope': 'benchmark_probe',
              'shipping_overhead_claim': False, 'issues': [], 'phases': []}
    issues = report['issues']
    try:
        clock_slack_ns, boundary_guard_ns = integer(clock_slack_ns), integer(boundary_guard_ns)
        if clock_slack_ns > 1_000_000_000 or boundary_guard_ns > 1_000_000_000:
            raise ValueError('clock/boundary allowance exceeds one second')
        if not isinstance(events, list) or len(events) > 2 * LIMIT:
            raise ValueError('probe input exceeds bounded event count')
        if not isinstance(phases, list) or not 1 <= len(phases) <= 256:
            raise ValueError('expected bounded explicit phase markers')
        unique = {}
        duplicates = 0
        for event in events:
            if not isinstance(event, dict):
                raise ValueError('probe event must be an object')
            fields = event.get('fields', event)
            if not isinstance(fields, dict):
                raise ValueError('probe fields must be an object')
            name = fields.get('event_name')
            if name in ERRORS:
                raise ValueError(f'probe reported {name}')
            if name != SAMPLE:
                raise ValueError('input must contain only reviewed probe events')
            sample = {key: integer(fields[key]) for key in FIELDS}
            sample['callsite_file'] = fields['callsite_file']
            if (not isinstance(sample['callsite_file'], str)
                    or not sample['callsite_file'].startswith('crates/eez-composer/src/')
                    or '..' in sample['callsite_file'].split('/')
                    or len(sample['callsite_file']) > 256):
                raise ValueError('invalid compiler callsite')
            sequence = sample['sequence']
            if not 1 <= sequence <= LIMIT or not sample['pool'] or not sample['callsite_line']:
                raise ValueError('invalid sequence/pool/callsite identity')
            if sequence in unique:
                if unique[sequence] != sample:
                    raise ValueError('conflicting duplicate probe sequence')
                duplicates += 1
            unique[sequence] = sample
        samples = [unique[key] for key in sorted(unique)]
        if not samples or list(sorted(unique)) != list(range(1, len(samples) + 1)):
            raise ValueError('missing startup/sequence coverage or logger drop')
        origin = (samples[0]['origin_unix_before_ns'], samples[0]['origin_unix_after_ns'])
        if origin[1] < origin[0] or origin[1] - origin[0] > clock_slack_ns:
            raise ValueError('origin clock bracket is invalid or too uncertain')
        for sample in samples:
            if (sample['origin_unix_before_ns'], sample['origin_unix_after_ns']) != origin:
                raise ValueError('mixed process clock origins')
            start, acquired, released = (sample[key] for key in ('started_ns','acquired_ns','released_ns'))
            before, wall, after = (sample[key] for key in ('clock_before_ns','observed_unix_ns','clock_after_ns'))
            if not start <= acquired <= released <= before <= after:
                raise ValueError('invalid monotonic lock/record ordering')
            if sample['wait_ns'] != acquired - start or sample['hold_ns'] != released - acquired:
                raise ValueError('reported duration differs from actual guard boundaries')
            if not origin[0] + before - clock_slack_ns <= wall <= origin[1] + after + clock_slack_ns:
                raise ValueError('wall/monotonic clock drift exceeds frozen allowance')
        names = set()
        previous_end = None
        for phase in phases:
            name = phase['name']
            start, end = integer(phase['start_unix_ns']), integer(phase['end_unix_ns'])
            if not isinstance(name, str) or not name or len(name) > 64 or name in names:
                raise ValueError('phase names must be explicit and unique')
            if end - start <= 2 * boundary_guard_ns or (previous_end is not None and start < previous_end):
                raise ValueError('invalid/overlapping phase window')
            names.add(name)
            previous_end = end
            if (not any(origin[1] + sample['released_ns'] < start - boundary_guard_ns for sample in samples)
                    or not any(origin[0] + sample['started_ns'] > end + boundary_guard_ns for sample in samples)):
                raise ValueError(f'{name}: missing before/after logger coverage')
            included, crossing, margin = [], [], []
            for sample in samples:
                earliest = origin[0] + sample['started_ns']
                latest = origin[1] + sample['released_ns']
                if earliest >= start + boundary_guard_ns and latest <= end - boundary_guard_ns:
                    included.append(sample)
                elif latest >= start and earliest <= end:
                    if earliest < start < latest or earliest < end < latest:
                        crossing.append(sample)
                    else:
                        margin.append(sample)
            if crossing:
                issues.append(f'{name}: guard crosses phase boundary or clock bracket makes membership ambiguous')
            if not included:
                raise ValueError(f'{name}: no complete guards inside measured population')
            grouped = defaultdict(list)
            for sample in included:
                grouped[(sample['pool'], sample['callsite_file'], sample['callsite_line'], sample['callsite_column'])].append(sample)
            report['phases'].append({'name': name, 'population': 'guards wholly inside marker window plus boundary guard',
                'start_unix_ns': start, 'end_unix_ns': end, 'aggregate': distribution(included),
                'by_callsite': [{'pool': key[0], 'file': key[1], 'line': key[2], 'column': key[3],
                                 **distribution(values)} for key, values in sorted(grouped.items())],
                'status': 'inconclusive' if crossing else 'passed',
                'boundary_crossings': [dict(sample) for sample in crossing],
                'boundary_margin_exclusions': [dict(sample) for sample in margin]})
        report.update(status='inconclusive' if issues else 'passed', samples=len(samples), duplicate_records=duplicates,
                      clock_slack_ns=clock_slack_ns, boundary_guard_ns=boundary_guard_ns,
                      origin_unix_before_ns=origin[0], origin_unix_after_ns=origin[1])
    except (KeyError, TypeError, ValueError, OverflowError) as error:
        issues.append(str(error))
    return report
