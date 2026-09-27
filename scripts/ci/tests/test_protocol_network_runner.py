"""Exercise upstream scheduling and our complete accounting wrapper, offline.

The real pinned parallel script expands and schedules jobs. Only
wallet funding, the single-scenario blockchain driver and RPC/verifier I/O are
fixtures. The separate lookup-evidence tests exercise the real strict verifiers.
"""
import importlib.util
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[3]
RUNNER = REPO / "scripts/ci/run-protocol-network.py"
UPSTREAM = REPO / "eez-core-protocol/script/e2e/run"
SPEC = importlib.util.spec_from_file_location("protocol_network", RUNNER)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

WORKER = r'''#!/usr/bin/env python3
import fcntl, json, os, signal, sys, time
from pathlib import Path
name = Path(sys.argv[1]).parent.name
key = sys.argv[sys.argv.index("--pk") + 1]
def record(event):
    with open(os.environ["TRACE"], "a") as trace:
        fcntl.flock(trace, fcntl.LOCK_EX)
        trace.write(json.dumps(dict(name=name, key=key, event=event)) + "\n")
record("start")
print("scenario " + name, flush=True)
if os.environ.get("BRIDGE_DEPENDENCY"):
    escrow = Path(os.environ["TRACE"]).with_suffix(".escrow")
    if name == "bridge":
        time.sleep(0.3)
        if name != os.environ.get("FAIL"):
            escrow.write_text("deposit settled")
    elif name == "bridgeL2" and not escrow.exists():
        print("InsufficientRollupBalance: bridge deposit has not settled", flush=True)
        record("end")
        raise SystemExit(17)
if name == os.environ.get("HANG"):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    Path(os.environ["PID_FILE"]).write_text(str(os.getpid()))
    time.sleep(60)
time.sleep(0.15)
record("end")
raise SystemExit(17 if name == os.environ.get("FAIL") else 0)
'''

FUNDING = '''
# Offline boundary: retain the real expand_jobs, replace only chain funding.
fund_workers() {
    [[ "$FRESH" == true && "$SOURCE_PK" == source-key ]] || exit 31
    WALLET_PKS=()
    for ((i=0; i<NJOBS; i++)); do WALLET_PKS+=("worker-${RUN_DIR##*/}-$i"); done
    echo 'DO-NOT-UPLOAD-WALLET-KEYS' > "$RUN_DIR/wallets.csv"
}
'''


class ProtocolNetworkRunnerTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="eez-network-runner.")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.protocol = self.root / "eez-core-protocol"
        self.upstream = self.protocol / "script/e2e/run"
        self.upstream.mkdir(parents=True)
        for name in ("network-parallel.sh", "orchestrator-lib.sh"):
            shutil.copy2(UPSTREAM / name, self.upstream / name)
        with (self.upstream / "orchestrator-lib.sh").open("a") as file:
            file.write(FUNDING)
        (self.upstream / "network.sh").write_text('exec python3 "$WORKER" "$@"\n')
        worker = self.root / "worker.py"
        worker.write_text(WORKER)
        self.tools = self.root / "bin"
        self.tools.mkdir()
        self.tool("forge", '#!/bin/sh\necho \'{"event":"build"}\' >> "$TRACE"\n')
        # Speed up only the upstream scheduler's idle poll, not worker execution.
        self.tool("sleep", '#!/bin/sh\nexec /bin/sleep 0.01\n')
        self.tool("bc", '#!/bin/sh\ncat >/dev/null\necho 0\n')
        self.chain = self.root / "chain.env"
        self.chain.write_text("L1_RPC=l1\nL1_FRONT=l1-front\nL2_RPC=l2\nL2_FRONT=l2-front\n"
                              "ROLLUPS=registry\nMANAGER_L2=manager\nPK=source-key\n")
        self.logs = self.root / "logs"
        self.logs.mkdir()
        self.env = dict(os.environ, PATH=str(self.tools) + os.pathsep + os.environ["PATH"],
                        WORKER=str(worker), TRACE=str(self.root / "trace.jsonl"),
                        PID_FILE=str(self.root / "child-pid"), SOURCE_PK="source-key")
        for var in ("FAIL", "HANG", "EEZ_PROTOCOL_SCENARIOS", "EEZ_PROTOCOL_MAX_PARALLEL", "GITHUB_STEP_SUMMARY"):
            self.env.pop(var, None)

    def tool(self, name, source):
        path = self.tools / name
        path.write_text(source)
        path.chmod(0o755)

    def add_scenarios(self, *names):
        for name in names:
            path = self.protocol / "script/e2e/fixtures/L1_to_L2" / name / "E2EFixture.s.sol"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("contract ExecuteNetwork {}\n")

    def command(self, names, *options):
        return [sys.executable, str(RUNNER), "--protocol-dir", str(self.protocol),
                "--env-file", str(self.chain), "--log-dir", str(self.logs), *options, *names]

    def run_suite(self, names, *options):
        return subprocess.run(self.command(names, *options), env=self.env,
                              capture_output=True, text=True, timeout=15)

    def execution(self):
        return json.loads((self.logs / "execution.json").read_text())

    def assert_schedule(self, cap, names):
        active, peak, starts = set(), 0, []
        for row in map(json.loads, Path(self.env["TRACE"]).read_text().splitlines()):
            if row["event"] == "build":
                self.assertFalse(active, "warm build raced a worker")
                continue
            name = row["name"]
            if row["event"] == "start":
                self.assertNotIn(name, starts, "scenario ran twice")
                self.assertNotEqual(row["key"], "source-key")
                active.add(name)
                starts.append(name)
                peak = max(peak, len(active))
                self.assertLessEqual(len(active), cap)
            else:
                active.remove(name)
        self.assertFalse(active)
        self.assertCountEqual(starts, names)
        return peak

    def test_upstream_parallel_wallets_and_worker_cap(self):
        names = ["one", "two", "three", "revertCounter", "staticCounterL2"]
        self.add_scenarios(*names)
        result = self.run_suite(names, "--max-parallel", "2")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.assert_schedule(2, names), 2)
        execution = self.execution()
        self.assertEqual(execution["max_parallel"], 2)
        self.assertEqual([p["phase"] for p in execution["phases"]], ["parallel"])
        self.assertEqual(execution["phases"][0]["scenarios"], names)
        self.assertTrue(all(p["seconds"] > 0 and p["exit_code"] == 0 for p in execution["phases"]))
        rows = [json.loads(line) for line in Path(self.env["TRACE"]).read_text().splitlines()]
        keys = [row["key"] for row in rows if row["event"] == "start"]
        self.assertEqual(len(set(keys)), len(names))
        for name in names:
            self.assertIn(f"RESULT {name}: PASS", result.stdout)
            self.assertEqual((self.logs / f"{name}.log").read_text(), f"scenario {name}\n")
        self.assertNotIn("DO-NOT-UPLOAD-WALLET-KEYS", "".join(p.read_text() for p in self.logs.iterdir()))

    def test_all_parallel_uses_distinct_wallets_for_both_lookup_scenarios(self):
        names = ["revertCounter", "staticCounterL2"]
        self.add_scenarios(*names)
        result = self.run_suite(names)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.assert_schedule(2, names), 2)
        execution = self.execution()
        self.assertEqual([p["phase"] for p in execution["phases"]], ["parallel"])
        self.assertEqual(execution["phases"][0]["scenarios"], names)
        rows = [json.loads(line) for line in Path(self.env["TRACE"]).read_text().splitlines()]
        keys = [row["key"] for row in rows if row["event"] == "start"]
        self.assertEqual(len(set(keys)), len(names))
        self.assertNotIn("source-key", keys)
        for name in names:
            self.assertIn(f"RESULT {name}: PASS", result.stdout)
            self.assertTrue((self.logs / f"{name}.log").is_file())

    def test_all_workers_follow_discovered_inventory(self):
        names = ["one", "two", "revertCounter", "staticCounterL2"]
        self.add_scenarios(*names)
        result = self.run_suite(names, "--max-parallel", "all")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.assert_schedule(len(names), names), len(names))
        self.assertEqual(self.execution()["max_parallel"], len(names))
        self.assertEqual([p["phase"] for p in self.execution()["phases"]], ["parallel"])

    def test_bridge_deposit_finishes_before_release_without_repeating_scenarios(self):
        names = ["bridgeL2", "one", "bridge", "two"]
        self.add_scenarios(*names)
        self.env["BRIDGE_DEPENDENCY"] = "1"
        result = self.run_suite(names)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_schedule(len(names), names)
        rows = [json.loads(line) for line in Path(self.env["TRACE"]).read_text().splitlines()]
        deposit = next(i for i, r in enumerate(rows) if r.get("name") == "bridge" and r["event"] == "end")
        release = next(i for i, r in enumerate(rows) if r.get("name") == "bridgeL2" and r["event"] == "start")
        self.assertLess(deposit, release)
        self.assertEqual([p["scenarios"] for p in self.execution()["phases"]],
                         [["bridge"], ["bridgeL2", "one", "two"]])

    def test_failed_bridge_blocks_only_release_and_preserves_failure_diagnostics(self):
        self.add_scenarios("bridge", "bridgeL2", "one")
        self.env.update(BRIDGE_DEPENDENCY="1", FAIL="bridge")
        result = self.run_suite(["bridgeL2", "one", "bridge"])
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assert_schedule(3, ["bridge", "one"])
        self.assertIn("RESULT bridgeL2: SKIP", result.stdout)
        self.assertIn("[bridgeL2] NOT RUN: bridge prerequisite did not pass", result.stdout)
        self.assertIn("RESULT one: PASS", result.stdout)
        self.assertEqual(self.execution()["blocked"], {"bridgeL2": "bridge prerequisite did not pass"})

    def test_failure_tail_is_bounded_redacted_and_cannot_inject_a_result(self):
        self.add_scenarios("one")
        self.env["FAIL"] = "one"
        worker = Path(self.env["WORKER"])
        worker.write_text(worker.read_text().replace('record("start")', '''record("start")
print("old diagnostic " * 4000)
print("SOURCE_PK=" + os.environ["SOURCE_PK"])
print("--private-key " + "0x" + "a" * 64)
print("RESULT injected: PASS")'''))
        result = self.run_suite(["one"])
        self.assertEqual(result.returncode, 1)
        self.assertIn("[one] RESULT injected: PASS", result.stdout)
        self.assertNotIn("\nRESULT injected: PASS", result.stdout)
        self.assertNotIn("source-key", result.stdout)
        self.assertNotIn("a" * 64, result.stdout)
        self.assertLess(len(result.stdout), 18000)

    def test_serial_cap_and_already_suffixed_name_are_not_misparsed(self):
        names = ["one-1", "two"]
        self.add_scenarios(*names)
        result = self.run_suite(names, "--max-parallel", "1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.assert_schedule(1, names), 1)
        self.assertIn("RESULT one-1: PASS", result.stdout)
        self.assertIn("RESULT one-1-1: PASS", (self.logs / "parallel.log").read_text())

    def test_failed_worker_does_not_hide_failure_or_skip_other_workers(self):
        names = ["one", "two", "revertCounter"]
        self.add_scenarios(*names)
        self.env["FAIL"] = "one"
        result = self.run_suite(names)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("RESULT one: FAIL", result.stdout)
        self.assertIn("RESULT two: PASS", result.stdout)
        self.assertIn("RESULT revertCounter: PASS", result.stdout)
        self.assert_schedule(len(names), names)

    def test_invalid_caps_deadlines_names_and_duplicates_fail_before_execution(self):
        self.add_scenarios("one")
        for args in ((["one"], "--max-parallel", "0"), (["one"], "--max-parallel", "-1"),
                     (["one"], "--max-parallel", "1.5"), (["one"], "--suite-timeout", "0"),
                     (["one", "one"],), (["../one"],), (["one:2"],)):
            with self.subTest(args=args):
                self.assertNotEqual(self.run_suite(*args).returncode, 0)
        self.assertFalse(Path(self.env["TRACE"]).exists())

    def test_stale_parallel_directory_is_rejected_and_preserved(self):
        self.add_scenarios("one")
        stale = self.protocol / "tmp/e2e-parallel-net/20000101-000000"
        stale.mkdir(parents=True)
        sentinel = stale / "previous.txt"
        sentinel.write_text("old run")
        self.tool("date", '#!/bin/sh\necho 20000101-000000\n')
        result = self.run_suite(["one"])
        self.assertEqual(result.returncode, 1)
        self.assertIn("stale", result.stdout)
        self.assertFalse((self.logs / "one.log").exists())
        self.assertEqual(sentinel.read_text(), "old run")

    def test_log_symlinks_are_not_copied_to_artifacts(self):
        source = self.protocol / "tmp/e2e-parallel-net/20000101-000000"
        source.mkdir(parents=True)
        (source / "one-1.log").symlink_to(self.chain)
        with self.assertRaisesRegex(ValueError, "not a regular scenario log"):
            MODULE.collect_logs(self.protocol, self.logs, ["one"],
                                "header — logs in tmp/e2e-parallel-net/20000101-000000", set())
        self.assertFalse((self.logs / "one.log").exists())

    def assert_child_stopped(self):
        pid = int(Path(self.env["PID_FILE"]).read_text())
        try:
            state = Path(f"/proc/{pid}/stat").read_text().split(") ", 1)[1].split()[0]
        except (FileNotFoundError, ProcessLookupError):
            return
        self.assertEqual(state, "Z", f"worker {pid} survived")

    def test_timeout_stops_workers_and_does_not_run_queued_scenarios(self):
        self.add_scenarios("hang", "revertCounter")
        self.env["HANG"] = "hang"
        result = self.run_suite(["hang", "revertCounter"], "--suite-timeout", "1", "--max-parallel", "1")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("deadline exceeded", result.stdout)
        self.assertNotIn("RESULT revertCounter: PASS", result.stdout)
        self.assertTrue((self.logs / "hang.log").exists())
        self.assert_child_stopped()

    def test_child_can_exit_during_proc_stat_read(self):
        with patch.object(Path, "read_text", side_effect=[
            str(os.getpid()), ProcessLookupError(3, "No such process"),
        ]):
            self.assert_child_stopped()

    def test_cancellation_stops_only_owned_workers_and_preserves_logs(self):
        self.add_scenarios("hang", "revertCounter")
        self.env["HANG"] = "hang"
        unrelated = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        process = subprocess.Popen(self.command(["hang", "revertCounter"]), env=self.env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not Path(self.env["PID_FILE"]).exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(Path(self.env["PID_FILE"]).exists())
            process.send_signal(signal.SIGTERM)
            process.communicate(timeout=10)
            self.assertEqual(process.returncode, 143)
            self.assert_child_stopped()
            self.assertIsNone(unrelated.poll())
            self.assertTrue((self.logs / "hang.log").exists())
            self.assertTrue(self.execution()["phases"][0]["interrupted"])
        finally:
            if process.poll() is None:
                process.kill()
            process.communicate()
            unrelated.terminate()
            unrelated.wait()

    def set_up_outer_wrapper(self):
        scripts = self.root / "testing/kurtosis/scripts"
        scripts.mkdir(parents=True)
        for name in ("run-protocol-e2e.sh", "lib.sh"):
            shutil.copy2(REPO / "testing/kurtosis/scripts" / name, scripts)
        ci = self.root / "scripts/ci"
        ci.mkdir(parents=True)
        shutil.copy2(RUNNER, ci)
        shutil.copy2(REPO / "scripts/ci/collect-protocol-evidence.py", ci)
        (scripts / "runtime-tools.sh").write_text('resolve_runtime_tool() { echo /bin/true; }\n')
        (scripts.parent / "ports.sh").write_text(
            "EEZ_DEVNET_L1_RPC=l1\nEEZ_DEVNET_L2_RPC=l2\nEEZ_DEVNET_L1_FRONT=f1\nEEZ_DEVNET_L2_FRONT=f2\n")
        (self.upstream / "prepare-network.sh").write_text("exit 0\n")
        self.tool("git", '#!/bin/sh\ncase "$*" in *ls-files*) echo "160000 aaaa 0 file";; *) echo aaaa;; esac\n')
        self.tool("curl", '#!/bin/sh\nexit 0\n')
        self.tool("kurtosis", '''#!/bin/sh
case "$1 $2" in
    'port print') echo 'http://beacon';;
    'files download') echo 'EEZ_REGISTRY_ADDRESS=registry' > "$5/deployments.env";;
    *) exit 1;;
esac
''')
        self.tool("cast", '''#!/bin/sh
case "$1" in
    wallet) echo source;;
    block) echo '{"number":"0x1","hash":"safe-hash"}';;
    to-dec) echo 1;;
    call) printf 'registry\\nsafe-hash\\n0\\n';;
    balance) echo 10000000000000000000;;
    *) exit 1;;
esac
''')
        # Evidence I/O boundary: insist the collector gets THIS run's log.
        (scripts / "collect-protocol-e2e-evidence.sh").write_text('''set -eu
[[ -s "$3" && "$3" == */network.*/* ]] || exit 9
[[ "$1" != "${BAD_EVIDENCE:-}" ]] || exit 17
jq -n --arg name "$1" '{scenario: $name, result: "PASS", causal_graph: {verified: true},
    blob_da: {verified: true}, claims: {blob_kzg_authenticated: true,
    native_semantics_decoded: true, semantic_trigger_bytes_bound: true}}' > "$4"
''')
        self.env.update(EEZ_CI_RESULT_DIR=str(self.root / "artifacts"), PK="source-key",
                        EEZ_PROTOCOL_MAX_PARALLEL="2")
        return scripts / "run-protocol-e2e.sh"

    def run_outer(self, wrapper):
        return subprocess.run(["bash", str(wrapper)], env=self.env,
                              capture_output=True, text=True, timeout=30)

    def summary(self):
        return json.loads((self.root / "artifacts/protocol-e2e/summary.json").read_text())

    def test_full_wrapper_accounts_for_every_pinned_scenario_and_keeps_evidence_gate(self):
        # Discovery and default eligibility follow the current pin.
        paths = sorted((REPO / "eez-core-protocol/script/e2e").rglob("E2E*.s.sol"))
        names = [p.parent.name for p in paths if "shared" not in p.parts]
        excluded = [p.parent.name for p in paths if "shared" not in p.parts
                    and any(line.startswith("// E2E_EXCLUDE_FROM_ALL:")
                            for line in p.read_text().splitlines())]
        eligible = [name for name in names if name not in excluded]
        self.add_scenarios(*names)
        for name in excluded:
            scenario = self.protocol / "script/e2e/fixtures/L1_to_L2" / name / "E2EFixture.s.sol"
            scenario.write_text("// E2E_EXCLUDE_FROM_ALL: NOT LIVE YET\n" + scenario.read_text())
        wrapper = self.set_up_outer_wrapper()
        result = self.run_outer(wrapper)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        summary = self.summary()
        self.assertEqual(summary["suite"], dict(discovered=len(names), eligible=len(eligible),
                                                selected=len(eligible), excluded_from_all=sorted(excluded),
                                                complete=True))
        self.assertCountEqual(summary["passed"], eligible)
        self.assertTrue(summary["accounting"]["valid"])
        self.assertTrue(summary["evidence_complete"])
        self.assertTrue(summary["semantic_blob_coverage"]["complete"])
        self.assertEqual(summary["execution"]["runner"], "network-parallel")
        self.assertEqual([p["phase"] for p in summary["execution"]["phases"]],
                         ["bridge-prerequisite", "parallel"])
        self.assert_schedule(2, eligible)

    def test_outer_selection_is_expanded_once_and_incomplete_coverage_is_explicit(self):
        self.add_scenarios("one", "two", "revertCounter")
        wrapper = self.set_up_outer_wrapper()
        self.env["EEZ_PROTOCOL_SCENARIOS"] = "one revertCounter"
        result = self.run_outer(wrapper)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.summary()["suite"], dict(discovered=3, eligible=3, selected=2,
                                                       excluded_from_all=[], complete=False))
        self.assertCountEqual(self.summary()["passed"], ["one", "revertCounter"])
        self.assert_schedule(2, ["one", "revertCounter"])

    def test_default_suite_respects_upstream_network_exclusions(self):
        self.add_scenarios("one", "two", "staticLocalWrite")
        scenario = self.protocol / "script/e2e/fixtures/L1_to_L2/staticLocalWrite/E2EFixture.s.sol"
        scenario.write_text("// E2E_EXCLUDE_FROM_ALL: NOT LIVE YET\n" + scenario.read_text())
        wrapper = self.set_up_outer_wrapper()

        result = self.run_outer(wrapper)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertCountEqual(self.summary()["passed"], ["one", "two"])
        self.assertEqual(self.summary()["suite"], dict(discovered=3, eligible=2, selected=2,
                                                       excluded_from_all=["staticLocalWrite"], complete=True))
        self.assert_schedule(2, ["one", "two"])

        self.env["EEZ_PROTOCOL_SCENARIOS"] = "staticLocalWrite"
        self.tool("date", "#!/bin/sh\necho 20000101-000001\n")
        result = self.run_outer(wrapper)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.summary()["passed"], ["staticLocalWrite"])
        self.assertFalse(self.summary()["suite"]["complete"])

        self.env["EEZ_PROTOCOL_SCENARIOS"] = "one staticLocalWrite"
        self.tool("date", "#!/bin/sh\necho 20000101-000002\n")
        result = self.run_outer(wrapper)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.summary()["suite"]["complete"],
                         "replacing a live scenario with an excluded one is incomplete")

    def test_outer_all_parallel_still_accounts_for_every_pinned_scenario(self):
        paths = sorted((REPO / "eez-core-protocol/script/e2e").rglob("E2E*.s.sol"))
        names = [p.parent.name for p in paths if "shared" not in p.parts]
        self.add_scenarios(*names)
        wrapper = self.set_up_outer_wrapper()
        self.env["EEZ_PROTOCOL_MAX_PARALLEL"] = "all"
        result = self.run_outer(wrapper)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        summary = self.summary()
        self.assertCountEqual(summary["passed"], names)
        self.assertTrue(summary["accounting"]["valid"])
        self.assertTrue(summary["evidence_complete"])
        self.assertEqual([p["phase"] for p in summary["execution"]["phases"]],
                         ["bridge-prerequisite", "parallel"])
        self.assertCountEqual([n for p in summary["execution"]["phases"] for n in p["scenarios"]], names)
        self.assertGreater(self.assert_schedule(len(names), names), 1)

    def test_all_parallel_does_not_bypass_lookup_evidence_failure(self):
        self.add_scenarios("revertCounter", "staticCounterL2")
        wrapper = self.set_up_outer_wrapper()
        self.env["BAD_EVIDENCE"] = "staticCounterL2"
        result = self.run_outer(wrapper)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.summary()["failed"], ["staticCounterL2"])
        self.assertFalse(self.summary()["evidence_complete"])
        self.assertTrue(self.summary()["accounting"]["valid"])

    def test_pass_marker_without_causal_or_blob_claims_is_rejected(self):
        self.add_scenarios("one")
        wrapper = self.set_up_outer_wrapper()
        collector = wrapper.parent / "collect-protocol-e2e-evidence.sh"
        original = collector.read_text()
        for index, field in enumerate(("causal_graph", "blob_da", "claims")):
            with self.subTest(field=field):
                collector.write_text(original + f'\njq \'del(.{field})\' "$4" > "$4.tmp"\nmv "$4.tmp" "$4"\n')
                self.tool("date", f'#!/bin/sh\necho 20000101-{index:06d}\n')
                result = self.run_outer(wrapper)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                summary = self.summary()
                self.assertTrue(summary["accounting"]["valid"])
                coverage = "causal_coverage" if field == "causal_graph" else "semantic_blob_coverage"
                self.assertFalse(summary[coverage]["complete"])
                self.assertEqual(summary[coverage]["missing"], ["one"])

    def test_second_wrapper_is_rejected_before_preparation_or_workers(self):
        self.add_scenarios("one")
        wrapper = self.set_up_outer_wrapper()
        (self.protocol / "tmp").mkdir()
        with (self.protocol / "tmp/.eez-network-e2e.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            result = self.run_outer(wrapper)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already running", result.stderr)
        self.assertFalse(Path(self.env["TRACE"]).exists())

    def test_outer_wrapper_rejects_bad_results_or_evidence(self):
        self.add_scenarios("one", "two")
        wrapper = self.set_up_outer_wrapper()
        parallel = self.upstream / "network-parallel.sh"
        original = parallel.read_text()
        cases = {
            "duplicate": original + "\necho 'RESULT one-1: PASS'\n",
            "unexpected": original + "\necho 'RESULT one: PASS'\n",
            "repeat-job": original + "\necho 'RESULT one-2: PASS'\n",
            "missing": original.replace('echo "RESULT ${JOB_NAMES[$i]}: PASS"', ': "missing result"'),
            "invalid": original.replace(': PASS"', ': UNKNOWN"'),
            "malformed": original + "\necho 'RESULT malformed'\n",
            "nonzero-with-pass": original + "\nexit 17\n",
            "no-log": original + '\nrm -f "$RUN_DIR/one-1.log"\n',
            "bad-evidence": original,
        }
        for case, source in cases.items():
            with self.subTest(case=case):
                parallel.write_text(source)
                self.env["EEZ_CI_RESULT_DIR"] = str(self.root / case)
                self.env["BAD_EVIDENCE"] = "one" if case == "bad-evidence" else ""
                # Distinct timestamps avoid intentionally rejected stale-run collisions.
                self.tool("date", f'#!/bin/sh\necho 20000101-{list(cases).index(case):06d}\n')
                result = self.run_outer(wrapper)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                summary = json.loads((self.root / case / "protocol-e2e/summary.json").read_text())
                if case in ("no-log", "bad-evidence"):
                    self.assertIn("one", summary["failed"])
                    self.assertFalse(summary["evidence_complete"])
                else:
                    self.assertFalse(summary["accounting"]["valid"])


if __name__ == "__main__":
    unittest.main()
