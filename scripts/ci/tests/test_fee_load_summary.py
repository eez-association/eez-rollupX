import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[2] / 'summarize-chiado-fee-load.py'
SPEC = importlib.util.spec_from_file_location('fee_load_summary', SOURCE)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class FeeLoadSummaryTests(unittest.TestCase):
    def test_failures_skips_and_drain_have_separate_denominators(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            name = 'q1-tx1'
            stage = dict(name=name, quoteRate=1, txRate=1, durationSeconds=2,
                         begin=1000, end=3000, drained=5000)
            (folder / 'stages.json').write_text(json.dumps([stage]))
            records = {
                'quotes': [dict(stage=name, kind='nested', usage='load', code='ok', ms=20),
                           dict(stage=name, kind='nested', usage='load', code='quote_busy', ms=1)],
                'submissions': [dict(stage=name, code='ok', ms=8), dict(stage=name, code='quote_busy', ms=1)],
                'settlements': [dict(stage=name, kind='nested', observedAt=2500, latencyMs=1000),
                                dict(stage=name, kind='nested', observedAt=4500, latencyMs=2000)],
                'generator-skips': [dict(at=2000, label=name + ':actor')],
            }
            for filename, rows in records.items():
                (folder / (filename + '.jsonl')).write_text(''.join(json.dumps(r) + '\n' for r in rows))
            result = MODULE.summarize(folder)['stages'][0]
            self.assertEqual(result['quoteCodes'], {'ok': 1, 'quote_busy': 1})
            self.assertEqual(result['successfulQuotesPerSecond'], .5)
            self.assertEqual(result['successfulQuoteLatencyMs']['p95'], 20)
            self.assertEqual(result['settledInWindowPerSecond'], .5)
            self.assertAlmostEqual(result['cohortIncludingDrainPerSecond'], 2 / 3.5)
            self.assertEqual(result['generatorSkips'], {name + ':actor': 1})
            self.assertEqual(result['accepted'], 1)

    def test_empty_success_set_is_not_reported_as_zero_latency(self):
        self.assertEqual(MODULE.timings([]), dict(count=0, p50=None, p95=None, p99=None, maximum=None))

    def test_nearest_rank_percentile_retains_tail(self):
        self.assertEqual(MODULE.percentile([1] * 98 + [100, 1000], .99), 100)
        self.assertEqual(MODULE.percentile([1] * 98 + [100, 1000], 1), 1000)


if __name__ == '__main__':
    unittest.main()
