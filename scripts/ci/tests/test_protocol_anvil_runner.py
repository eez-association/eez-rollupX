"""Deadline, exit-status and process-isolation tests without running a blockchain."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


RUNNER = Path(__file__).resolve().parents[1] / "run-protocol-anvil.sh"
SCENARIO = r'''#!/usr/bin/env bash
set -eu
name="$(basename "$(dirname "$1")")"
echo "started $name"
echo "$L1_PORT $L2_PORT $L1_CHAIN_ID $L2_CHAIN_ID $ETH_TIMEOUT $ETH_RPC_TIMEOUT"
if [[ "$name" == fail ]]; then exit 17; fi
if [[ "$name" == hang ]]; then
    python3 -c 'import os, signal, time; from pathlib import Path; signal.signal(signal.SIGTERM, signal.SIG_IGN); Path(os.environ["PID_FILE"]).write_text(str(os.getpid())); time.sleep(60)' &
    wait
fi
'''


class ProtocolAnvilRunnerTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="eez-anvil-runner.")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        scenario = self.root / "script/e2e/lib/local-scenario.sh"
        scenario.parent.mkdir(parents=True)
        scenario.write_text(SCENARIO)
        self.pid_file = self.root / "child-pid"
        self.env = dict(os.environ, PID_FILE=str(self.pid_file), BASE_PORT="29545", BASE_CHAIN_ID="61337")
        for variable in ("ETH_TIMEOUT", "ETH_RPC_TIMEOUT", "EEZ_ANVIL_SCENARIO_TIMEOUT", "EEZ_ANVIL_SUITE_TIMEOUT"):
            self.env.pop(variable, None)

    def add_scenario(self, name, category="nested"):
        sol = self.root / "script/e2e/scenarios" / category / "L2_to_L1" / name / "E2EFixture.s.sol"
        sol.parent.mkdir(parents=True)
        sol.touch()

    def command(self):
        return ["bash", str(RUNNER), str(self.root)]

    def run_suite(self, **settings):
        return subprocess.run(self.command(), env=dict(self.env, **settings),
                              capture_output=True, text=True, timeout=20)

    def results(self):
        rows = next((self.root / "tmp").glob("e2e-ci-*/results.tsv")).read_text().splitlines()
        return [dict(scenario=name, result=result, exit_code=int(code), seconds=int(seconds))
                for name, result, code, seconds in (row.split("\t") for row in rows)]

    def assert_child_stopped(self):
        pid = int(self.pid_file.read_text())
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                state = Path(f"/proc/{pid}/stat").read_text().split(") ", 1)[1].split()[0]
                if state == "Z":  # Reaping belongs to PID 1, not the scenario wrapper.
                    return
            except (FileNotFoundError, ProcessLookupError):
                # Linux may report ESRCH if the child exits after stat opens.
                return
            time.sleep(0.05)
        self.fail(f"scenario child {pid} is still running")

    def test_all_scenarios_pass_with_distinct_ports_and_deadlines(self):
        self.add_scenario("one")
        self.add_scenario("two")
        self.add_scenario("ignored", category="shared")
        result = self.run_suite()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual([r["result"] for r in self.results()], ["PASS", "PASS"])
        logs = next((self.root / "tmp").glob("e2e-ci-*"))
        self.assertIn("29545 29546 61337 61338 120 30", (logs / "one.log").read_text())
        self.assertIn("29547 29548 61339 61340 120 30", (logs / "two.log").read_text())

    def test_child_can_exit_during_proc_stat_read(self):
        with patch.object(Path, "read_text", side_effect=[
            str(os.getpid()), ProcessLookupError(3, "No such process"),
        ]):
            self.assert_child_stopped()

    def test_failure_does_not_skip_remaining_scenarios_or_pass_the_suite(self):
        self.add_scenario("fail")
        self.add_scenario("pass")
        result = self.run_suite()
        self.assertEqual(result.returncode, 1)
        self.assertEqual([r["result"] for r in self.results()], ["FAIL", "PASS"])
        self.assertEqual(self.results()[0]["exit_code"], 17)

    def test_hang_is_bounded_and_descendants_are_stopped(self):
        self.add_scenario("hang")
        self.add_scenario("pass")
        result = self.run_suite(EEZ_ANVIL_SCENARIO_TIMEOUT="1")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual([r["result"] for r in self.results()], ["TIMEOUT", "PASS"])
        self.assertIn("deadline exceeded", result.stdout)
        self.assert_child_stopped()

    def test_suite_timeout_is_not_a_successful_partial_run(self):
        self.add_scenario("hang")
        self.add_scenario("pass")
        # The suite clock starts before spawning the first scenario. One second
        # can expire during process startup on a busy runner, testing NOT_RUN
        # instead of the intended running-child timeout and remaining-work gate.
        result = self.run_suite(EEZ_ANVIL_SUITE_TIMEOUT="5")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual([r["result"] for r in self.results()], ["TIMEOUT", "NOT_RUN"])
        self.assert_child_stopped()

    def test_cancellation_stops_only_the_scenario_process_group(self):
        self.add_scenario("hang")
        # Unrelated process simulates another workload on the same host.
        unrelated = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        process = subprocess.Popen(self.command(), env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not self.pid_file.exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertTrue(self.pid_file.exists())
            process.send_signal(signal.SIGTERM)
            process.communicate(timeout=10)
            self.assertEqual(process.returncode, 143)
            self.assert_child_stopped()
            self.assertIsNone(unrelated.poll())
        finally:
            if process.poll() is None:
                process.kill()
            process.communicate()
            unrelated.terminate()
            unrelated.wait()

    def test_empty_suite_and_invalid_deadline_are_errors(self):
        self.assertNotEqual(self.run_suite().returncode, 0)
        self.add_scenario("pass")
        for timeout in ("0", "-1", "inf", "nan"):
            with self.subTest(timeout=timeout):
                self.assertNotEqual(self.run_suite(EEZ_ANVIL_SCENARIO_TIMEOUT=timeout).returncode, 0)

    def test_duplicate_names_fail_before_execution(self):
        self.add_scenario("one", category="a")
        self.add_scenario("one", category="b")
        result = self.run_suite()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Duplicate scenario name", result.stderr)
        self.assertFalse((self.root / "tmp").exists())


if __name__ == "__main__":
    unittest.main()
