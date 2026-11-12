"""Quiet-window and sample-accounting boundaries; no timing workload is executed."""
import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("measure_streaming", Path(__file__).resolve().parents[2]/"measure-streaming-deadlines.py")
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class QuietWindowTests(unittest.TestCase):
    def test_interrupted_record_is_retained_and_cannot_pass(self):
        for contaminated in (False, True):
            with self.subTest(contaminated=contaminated), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                test = root/"release/deps/node_e2e"
                signer = root/"release/eez-proof-signer"
                test.parent.mkdir(parents=True)
                test.touch()
                signer.touch()
                output = root/"evidence"
                child = mock.Mock(pid=123456, returncode=0)
                child.poll.return_value = 0
                def launch(*args, **kwargs):
                    (output/"attempts.jsonl").write_text('{"event": "attempt_result",')
                    return child
                def snapshot(*args):
                    row = self.sample()
                    row.update(utc="2026-11-12T00:00:00Z", monotonic=snapshot.tick)
                    row["cpu"][3] += snapshot.tick
                    if contaminated and snapshot.tick == 240:
                        row["heavy"] = [dict(pid=99, command="rustc")]
                    if snapshot.tick >= 360:
                        self.assertEqual(cleanup.call_count, 1, "cleanup must precede post-observation")
                    snapshot.tick += 120
                    return row
                snapshot.tick = 0
                check_reasons = measure.reasons
                def reasons(previous, current):
                    if current["monotonic"] == 360:
                        self.assertEqual(previous["monotonic"], 240, "post interval must start at the final during snapshot")
                    return check_reasons(previous, current)
                with mock.patch.object(sys, "argv", ["measure", str(test), str(signer), str(output)]), \
                        mock.patch.object(measure.subprocess, "check_output", side_effect=lambda cmd, **kw: measure.TEST+": test" if "--list" in cmd else ""), \
                        mock.patch.object(measure.subprocess, "Popen", side_effect=launch), \
                        mock.patch.object(measure, "snapshot", side_effect=snapshot), \
                        mock.patch.object(measure, "reasons", side_effect=reasons), \
                        mock.patch.object(measure, "wait_interval", return_value=[]), \
                        mock.patch.object(measure, "terminate_owned") as cleanup:
                    self.assertEqual(measure.main(), 1)
                    self.assertEqual(cleanup.call_count, 1)
                summary = measure.json.loads((output/"summary.json").read_text())
                self.assertFalse(summary["eligible_performance_evidence"])
                self.assertTrue(summary["contaminated"])
                self.assertIn("incomplete/unreadable", summary["status"])
                self.assertEqual((output/"attempts.jsonl").read_text(), '{"event": "attempt_result",')

    def test_driver_stays_in_the_published_native_suite_inventory(self):
        path = Path(__file__).resolve().parents[1]/"native-tests.py"
        native_spec = importlib.util.spec_from_file_location("native_tests", path)
        native = importlib.util.module_from_spec(native_spec)
        native_spec.loader.exec_module(native)
        self.assertIn(measure.TEST.split("::")[0], native.CROSS_CHAIN_MODULES)
        for binary in (*native.VECTOR_TARGETS, "verify_blob_evidence"):
            self.assertTrue(measure.HEAVY.search(binary[:15]), binary)

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

    def test_host_process_names_may_contain_spaces(self):
        with mock.patch.object(measure.subprocess, "check_output", return_value="10 1 lxc monitor 0\n11 1 rustc\n12 1 eez-composer\n"):
            count, heavy = measure.heavy_processes()
        self.assertEqual(count, 3)
        self.assertEqual(heavy, [dict(pid=11, command="rustc")])

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
