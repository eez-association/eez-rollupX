"""Real worker processes: bounded checks, exact results and cancellation."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("evidence", REPO / "scripts/ci/collect-protocol-evidence.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ParallelEvidenceTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="eez-evidence-workers.")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.collector = self.root / "collector.sh"
        self.collector.write_text('''exec python3 - "$@" <<'PY'
import fcntl, json, os, subprocess, sys, time
from pathlib import Path
name, source, log, output = sys.argv[1:]
def event(kind):
    with open(os.environ['TRACE'], 'a') as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        stream.write(json.dumps(dict(name=name, event=kind))+'\\n')
event('start')
if name == 'hang':
    child = subprocess.Popen(['sleep', '60'])
    Path(os.environ['CHILD_PID']).write_text(str(child.pid))
    time.sleep(60)
time.sleep(0.1)
if name == 'fail':
    print('wrong canonical block')
    raise SystemExit(17)
if name != 'missing':
    Path(output).write_text(json.dumps(dict(scenario='other' if name == 'wrong' else name, result='PASS')))
event('end')
PY
''')
        self.output = self.root / "outputs"
        self.output.mkdir()
        self.trace = self.root / "trace"
        self.child = self.root / "child"

    def collect(self, names, workers=3, timeout=5):
        with patch.object(MODULE, "COLLECTOR", self.collector), \
                patch.dict(os.environ, TRACE=str(self.trace), CHILD_PID=str(self.child)):
            return MODULE.collect(names, dict.fromkeys(names, self.root / "source.sol"),
                                  self.root, self.output, workers, timeout)

    def child_is_stopped(self):
        status = Path(f"/proc/{self.child.read_text()}/stat")
        try:
            return status.read_text().split()[2] == "Z"
        except (FileNotFoundError, ProcessLookupError):
            return True

    def test_child_can_exit_during_proc_stat_read(self):
        with patch.object(Path, "read_text", side_effect=[
            str(os.getpid()), ProcessLookupError(3, "No such process"),
        ]):
            self.assertTrue(self.child_is_stopped())

    def test_running_child_is_not_reported_stopped(self):
        self.child.write_text(str(os.getpid()))
        self.assertFalse(self.child_is_stopped())

    def test_parallel_limit_and_one_result_per_scenario(self):
        names = [f"case-{i}" for i in range(8)]
        records = self.collect(names)
        self.assertEqual([r["scenario"] for r in records], names)
        self.assertTrue(all(r["exit_code"] == 0 for r in records))
        active = peak = 0
        for row in map(json.loads, self.trace.read_text().splitlines()):
            active += 1 if row["event"] == "start" else -1
            peak = max(peak, active)
        self.assertEqual(active, 0)
        self.assertEqual(peak, 3)

    def test_worker_failure_missing_or_wrong_result_stay_failures(self):
        records = self.collect(["ok", "fail", "wrong", "missing"])
        self.assertEqual([r["exit_code"] for r in records], [0, 17, 1, 1])
        self.assertIn("wrong canonical block", (self.output / "fail.collect.log").read_text())

    def test_timeout_stops_descendants_and_does_not_kill_unrelated_process(self):
        unrelated = subprocess.Popen(["sleep", "60"])
        try:
            records = self.collect(["hang", "ok"], timeout=1)
            self.assertEqual([r["exit_code"] for r in records], [124, 0])
            self.assertIsNone(unrelated.poll())
            for _ in range(50):
                if self.child_is_stopped():
                    break
                time.sleep(0.02)
            else:
                self.fail("collector descendant survived timeout")
        finally:
            unrelated.terminate()
            unrelated.wait()

    def test_existing_logs_are_never_overwritten(self):
        sentinel = self.output / "ok.collect.log"
        sentinel.write_text("previous invocation")
        with self.assertRaises(FileExistsError):
            self.collect(["ok"])
        self.assertEqual(sentinel.read_text(), "previous invocation")

    def test_cancellation_stops_active_groups_and_never_starts_queued_work(self):
        launcher = self.root / "launch.py"
        launcher.write_text(f'''import importlib.util
from pathlib import Path
spec = importlib.util.spec_from_file_location("evidence", {str(REPO / 'scripts/ci/collect-protocol-evidence.py')!r})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.COLLECTOR = Path({str(self.collector)!r})
module.collect(["hang", "queued"], dict.fromkeys(["hang", "queued"], Path("unused")),
               Path({str(self.root)!r}), Path({str(self.output)!r}), 1, 60)
''')
        process = subprocess.Popen([sys.executable, str(launcher)],
                                   env=dict(os.environ, TRACE=str(self.trace), CHILD_PID=str(self.child)))
        try:
            deadline = time.monotonic() + 5
            while not self.child.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(self.child.exists())
            process.terminate()
            self.assertEqual(process.wait(timeout=5), 143)
            self.assertNotIn('"name": "queued"', self.trace.read_text())
            self.assertTrue(self.child_is_stopped())
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


if __name__ == "__main__":
    unittest.main()
