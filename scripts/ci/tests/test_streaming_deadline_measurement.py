"""Quiet-window and sample-accounting boundaries; no timing workload is executed."""
import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("measure_streaming", Path(__file__).resolve().parents[2]/"measure-streaming-deadlines.py")
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class QuietWindowTests(unittest.TestCase):
    def test_driver_stays_in_the_published_native_suite_inventory(self):
        path = Path(__file__).resolve().parents[1]/"native-tests.py"
        native_spec = importlib.util.spec_from_file_location("native_tests", path)
        native = importlib.util.module_from_spec(native_spec)
        native_spec.loader.exec_module(native)
        self.assertIn(measure.TEST.split("::")[0], native.CROSS_CHAIN_MODULES)

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
        self.assertTrue(measure.HEAVY.search("eez_proof_signe"))  # Linux truncates comm to 15 bytes.
        self.assertTrue(measure.HEAVY.search("eez_control_rpc"))
        self.assertFalse(measure.HEAVY.search("eez-composer"))

    def test_owned_descendants_do_not_include_other_tasks(self):
        self.assertEqual(measure.descendants([(10, 1, "test"), (11, 10, "signer"), (12, 11, "child"), (13, 1, "rustc")], 10), {10, 11, 12})

    def test_short_build_between_full_snapshots_is_retained(self):
        states = [(100, []) for _ in range(measure.INTERVAL)]
        states[3] = (100, [dict(pid=99, command="rustc")])
        with mock.patch.object(measure, "heavy_processes", side_effect=states), mock.patch.object(measure.time, "sleep"):
            self.assertEqual(measure.wait_interval(), [dict(pid=99, command="rustc")])

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

    def test_cleanup_stops_descendants_after_the_group_leader_exits(self):
        child = "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print('ready',flush=True); time.sleep(60)"
        code = f"import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c',{child!r}],stdout=subprocess.PIPE,text=True); p.stdout.readline(); print(p.pid,flush=True); time.sleep(60)"
        own = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE, text=True, start_new_session=True)
        descendant = int(own.stdout.readline())
        try:
            measure.terminate_owned(own)
            for _ in range(100):
                try:
                    state = Path(f"/proc/{descendant}/stat").read_text().split(")", 1)[1].split()[0]
                except FileNotFoundError:
                    break
                if state == "Z":
                    break
                time.sleep(.01)
            else:
                self.fail("owned descendant remained running after cleanup")
        finally:
            try:
                os.killpg(own.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            own.wait()
            own.stdout.close()

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
