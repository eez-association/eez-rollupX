import copy
import unittest

from overhead_lock_analysis import SAMPLE, analyze

ORIGIN = 1_000_000_000


def event(sequence, start, end, *, line=10):
    acquired = start + 2
    return {'fields': {'event_name': SAMPLE, 'sequence': sequence, 'pool': 1,
        'origin_unix_before_ns': ORIGIN, 'origin_unix_after_ns': ORIGIN,
        'started_ns': start, 'acquired_ns': acquired, 'released_ns': end,
        'wait_ns': 2, 'hold_ns': end - acquired,
        'clock_before_ns': end + 1, 'clock_after_ns': end + 3,
        'observed_unix_ns': ORIGIN + end + 2,
        'callsite_file': 'crates/eez-composer/src/held_pool.rs',
        'callsite_line': line, 'callsite_column': 20}}


def fixture():
    return [event(1, 0, 10), event(2, 30, 40), event(3, 50, 60, line=30), event(4, 90, 100)]


def run(events, **kwargs):
    phases = [{'name': 'busy', 'start_unix_ns': ORIGIN + 20, 'end_unix_ns': ORIGIN + 80}]
    return analyze(events, phases, clock_slack_ns=1, boundary_guard_ns=1, **kwargs)


class LockAnalysis(unittest.TestCase):
    def test_actual_boundaries_and_operation_populations_remain_distinct(self):
        result = run(fixture())
        self.assertEqual(result['status'], 'passed')
        self.assertFalse(result['shipping_overhead_claim'])
        phase = result['phases'][0]
        self.assertEqual(phase['aggregate']['count'], 2)
        self.assertEqual(len(phase['by_callsite']), 2)
        self.assertEqual(phase['aggregate']['wait']['sum_ns'], 4)
        self.assertEqual(phase['aggregate']['hold']['sum_ns'], 16)

    def test_delivery_order_and_identical_duplicate_do_not_invent_locks(self):
        events = fixture()
        result = run([events[3], events[1], events[0], events[2], copy.deepcopy(events[1])])
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['samples'], 4)
        self.assertEqual(result['duplicate_records'], 1)
        self.assertEqual(result['phases'][0]['aggregate']['count'], 2)

    def test_conflicting_duplicates_and_logger_gaps_are_inconclusive(self):
        events = fixture()
        changed = copy.deepcopy(events[1])
        changed['fields']['hold_ns'] += 1
        for candidate in [events + [changed], events[1:], events[:2] + events[3:], []]:
            self.assertEqual(run(candidate)['status'], 'inconclusive')

    def test_missing_post_phase_bracket_cannot_hide_last_logger_drops(self):
        result = run(fixture()[:-1])
        self.assertEqual(result['status'], 'inconclusive')
        self.assertIn('logger coverage', result['issues'][0])

    def test_clock_step_mixed_origin_invalid_duration_and_limits_fail_explicitly(self):
        mutations = [('observed_unix_ns', ORIGIN + 500), ('origin_unix_after_ns', ORIGIN + 4),
                     ('wait_ns', 5), ('acquired_ns', 100), ('sequence', 65537),
                     ('callsite_file', '../../private')]
        for field, value in mutations:
            with self.subTest(field=field):
                events = fixture()
                events[1]['fields'][field] = value
                self.assertEqual(run(events)['status'], 'inconclusive')

    def test_explicit_overflow_or_clock_error_never_becomes_a_partial_pass(self):
        for name in ['eez.benchmark.held_pool_overflow', 'eez.benchmark.held_pool_clock_error']:
            self.assertEqual(run(fixture() + [{'fields': {'event_name': name}}])['status'], 'inconclusive')

    def test_crossing_guard_is_reported_separately_from_interior_population(self):
        events = [event(1, 0, 10), event(2, 15, 35), event(3, 50, 60), event(4, 90, 100)]
        result = run(events)
        self.assertEqual(result['status'], 'inconclusive')
        phase = result['phases'][0]
        self.assertEqual(phase['status'], 'inconclusive')
        self.assertEqual(phase['aggregate']['count'], 1)
        self.assertEqual(len(phase['boundary_crossings']), 1)
        self.assertEqual(phase['boundary_crossings'][0]['hold_ns'], 18)

    def test_long_crossing_guard_cannot_be_removed_to_improve_tail(self):
        for end in [70, 85]:
            events = [event(1, 0, 10), event(2, 15, end), event(3, 50, 60), event(4, 90, 100)]
            events[1]['fields']['pool'] = 2
            result = run(events)
            self.assertEqual(result['status'], 'inconclusive')
            self.assertEqual(result['phases'][0]['boundary_crossings'][0]['hold_ns'], end - 17)

    def test_margin_only_guard_has_explicit_population_without_false_crossing(self):
        events = [event(1, 0, 10), event(2, 20, 30), event(3, 50, 60), event(4, 90, 100)]
        result = run(events)
        self.assertEqual(result['status'], 'passed')
        phase = result['phases'][0]
        self.assertEqual(phase['boundary_crossings'], [])
        self.assertEqual(len(phase['boundary_margin_exclusions']), 1)

    def test_integer_strings_preserve_native_precision_and_malformed_types_fail(self):
        events = fixture()
        for item in events:
            for key, value in list(item['fields'].items()):
                if isinstance(value, int): item['fields'][key] = str(value)
        self.assertEqual(run(events)['status'], 'passed')
        for bad in [None, True, 1.5, '1e3']:
            events = fixture()
            events[1]['fields']['wait_ns'] = bad
            self.assertEqual(run(events)['status'], 'inconclusive')
        self.assertEqual(run(['bad'])['status'], 'inconclusive')

    def test_overlapping_and_empty_phase_populations_are_not_verified(self):
        for phases in [[{'name': 'empty', 'start_unix_ns': ORIGIN+70, 'end_unix_ns': ORIGIN+80}],
                       [{'name': 'one', 'start_unix_ns': ORIGIN+20, 'end_unix_ns': ORIGIN+80},
                        {'name': 'two', 'start_unix_ns': ORIGIN+30, 'end_unix_ns': ORIGIN+90}]]:
            result = analyze(fixture(), phases, clock_slack_ns=1, boundary_guard_ns=1)
            self.assertEqual(result['status'], 'inconclusive')


if __name__ == '__main__':
    unittest.main()
