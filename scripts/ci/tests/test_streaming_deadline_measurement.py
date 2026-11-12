"""Quiet-window and sample-accounting boundaries; no timing workload is executed."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location("measure_streaming", Path(__file__).resolve().parents[2]/"measure-streaming-deadlines.py")
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class QuietWindowTests(unittest.TestCase):
    def sample(self):
        return dict(process_count=100, heavy=[], free_bytes={"/": 6*2**30},
                    mem_available_kib=13*2**20, load=[2, 2, 2], affinity_cpus=8,
                    pressure={"memory": "some avg10=0.00 avg60=0.00 total=0"},
                    cpu=[100, 0, 0, 900, 0, 0, 0, 0], swapin=0, swapout=0)

    def test_stable_and_exact_boundaries(self):
        before, after = self.sample(), self.sample()
        after["cpu"] = [130, 0, 0, 970, 0, 0, 0, 0]
        self.assertEqual(measure.reasons(before, after), [])
        after["cpu"][3] -= 1
        self.assertIn("CPU idle below 70%", measure.reasons(before, after))

    def test_compilers_disk_swap_visibility_and_pressure_reject(self):
        for field, value, expected in [
            ("heavy", [{"pid": 1, "command": "clippy-driver"}], "unrelated compiler/test/IO work"),
            ("free_bytes", {"/": 100}, "less than 5 GiB free"),
            ("swapout", 1, "swap traffic"),
            ("process_count", 4, "host process visibility unavailable"),
            ("pressure", {"memory": "some avg10=1.00 total=0"}, "memory pressure"),
        ]:
            with self.subTest(field=field):
                before, after = self.sample(), self.sample()
                after["cpu"][3] += 100
                after[field] = value
                self.assertIn(expected, measure.reasons(before, after))
        self.assertTrue(measure.HEAVY.search("clippy-driver"))
        self.assertTrue(measure.HEAVY.search("solc-0.8.34"))
        self.assertTrue(measure.HEAVY.search("node_e2e-a83b"))
        self.assertFalse(measure.HEAVY.search("eez-composer"))

    def test_owned_descendants_do_not_include_other_tasks(self):
        self.assertEqual(measure.descendants([(10, 1, "test"), (11, 10, "signer"), (12, 11, "child"), (13, 1, "rustc")], 10), {10, 11, 12})

    def test_cleanup_terminates_only_the_owned_process_group(self):
        own = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)
        other = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)
        try:
            measure.terminate_owned(own)
            self.assertIsNotNone(own.poll())
            self.assertIsNone(other.poll())
        finally:
            measure.terminate_owned(own)
            measure.terminate_owned(other)

    def test_percentile_ranks_and_failures_are_not_discarded(self):
        rows = [dict(event="attempt_result", warmup=False, success=True,
                     result=dict(fixture="a", scenario="b", prove_ms=n)) for n in range(1, 21)]
        rows += [dict(event="attempt_result", warmup=False, success=False,
                      result=dict(fixture="a", scenario="b", failed_after_ms=999))]
        rows += [dict(event="attempt_result", warmup=True, success=True,
                      result=dict(fixture="a", scenario="b", prove_ms=1000))]
        summary = measure.distributions(rows)["a/b"]
        self.assertEqual((summary["successes"], summary["failures"]), (20, 1))
        self.assertEqual(summary["metrics"]["prove_ms"]["p95"], 19)
        self.assertEqual(summary["metrics"]["prove_ms"]["maximum"], 20)
        self.assertEqual(summary["metrics"]["prove_ms"]["median"], 10.5)

    def test_operation_correlation_uses_transport_and_request_together(self):
        row = dict(event="attempt_result", warmup=False, success=True,
            result=dict(fixture="a", scenario="b", events=[
                dict(direction="client", transport=1, request_id=1, kind="Block", elapsed_ms=1),
                dict(direction="client", transport=2, request_id=1, kind="Block", elapsed_ms=2),
                dict(direction="server", transport=2, request_id=1, kind="Validated", elapsed_ms=4),
                dict(direction="server", transport=1, request_id=1, kind="Validated", elapsed_ms=8),
            ]))
        intervals = measure.distributions([row])["a/b"]["operation_intervals_ms"]["Block/Validated"]
        self.assertEqual((intervals["n"], intervals["minimum"], intervals["maximum"]), (2, 2, 7))


if __name__ == "__main__":
    unittest.main()
