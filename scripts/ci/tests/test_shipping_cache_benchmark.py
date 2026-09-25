"""Boundaries of the opt-in real shipping-cache sizing experiment."""
import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml

REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("shipping_benchmark", REPO / "scripts/ci/rollupx-cache-benchmark.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class ShippingCacheBenchmarkTest(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("EEZ_TEST_SHIPPING_CACHE_IMAGE"), "opt-in bounded Docker cache orchestration fixture")
    def test_real_seed_archive_restore_orchestration_with_tiny_fixture(self):
        # Never builds RollupX: only exercise the new orchestration against
        # seven tiny executables, including the real archive round trip.
        (REPO / "artifacts").mkdir(exist_ok=True)
        root = Path(tempfile.mkdtemp(prefix="shipping-cache-fixture-", dir=REPO / "artifacts"))
        print(f"Preserving fixture evidence: {root}", flush=True)
        fixture = root / "fixture"
        (fixture / "src/bin").mkdir(parents=True)
        (fixture / "examples").mkdir()
        (fixture / "Cargo.toml").write_text('[package]\nname="shipping-cache-fixture"\nversion="0.1.0"\nedition="2021"\n')
        (fixture / "Cargo.lock").write_text('version = 4\n[[package]]\nname="shipping-cache-fixture"\nversion="0.1.0"\n')
        for name in bench.BINARIES:
            relative = name if name.startswith("examples/") else "src/bin/" + name
            (fixture / (relative + ".rs")).write_text('''fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--invalid-cli-fixture") {
        eprintln!("error: unexpected argument '--invalid-cli-fixture'");
        std::process::exit(2);
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Reth Version: 1.0.0");
    } else {
        println!("Usage: fixture");
    }
}\n''')
        def stage(destination):
            shutil.copytree(fixture, destination)
            return "controlled-fixture"
        with patch.object(bench, "stage_source", side_effect=stage), patch.object(bench, "BUILD",
                ("cargo", "build", "--release", "--locked", "--bins", "--examples")):
            bench.benchmark("seed", root / "seed", os.environ["EEZ_TEST_SHIPPING_CACHE_IMAGE"])
            (root / "restore").mkdir()
            shutil.copy2(root / "seed/state.tar.gz", root / "restore/state.tar.gz")
            bench.benchmark("restore", root / "restore", os.environ["EEZ_TEST_SHIPPING_CACHE_IMAGE"])
        restored = json.loads((root / "restore/result.json").read_text())
        self.assertTrue(restored["reuse"]["all_fresh"])
        self.assertEqual(len(restored["hashes"]), 7)
        self.assertEqual(restored["status"], "complete")

    def test_failures_and_timeouts_preserve_metrics_and_print_safe_bounded_logs(self):
        for timed_out in (False, True):
            with self.subTest(timed_out=timed_out), tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                def execute(args, **kwargs):
                    if args[1] == "rm":
                        return subprocess.CompletedProcess(args, 0)
                    kwargs["stdout"].write("old line\n" * 10000 + "::error::diagnostic\n")
                    kwargs["stderr"].write("root cause\nEEZ_BENCH_MEMORY_PEAK_BYTES=123\n")
                    if timed_out:
                        raise subprocess.TimeoutExpired(args, kwargs["timeout"])
                    return subprocess.CompletedProcess(args, 1)
                console = io.StringIO()
                with patch.object(bench.subprocess, "run", side_effect=execute) as invoked, \
                        contextlib.redirect_stdout(console):
                    with self.assertRaisesRegex(RuntimeError, "build failed"):
                        bench.run(work, "image", "build", ["cargo", "build"])
                metric = json.loads((work / "build.json").read_text())
                self.assertEqual(metric["timed_out"], timed_out)
                self.assertEqual(metric["container_memory_peak_bytes"], 123)
                self.assertIn('build stderr: "root cause"', console.getvalue())
                self.assertNotIn("\n::error::", console.getvalue())
                self.assertLess(len(console.getvalue()), 3000)
                self.assertEqual(invoked.call_args.args[0][1:3], ["rm", "--force"])

    def test_cli_checks_require_expected_exit_code_and_output(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            def execute(work, image, label, command, **kwargs):
                invalid = kwargs["expected_code"] == 2
                text = "unexpected argument '--invalid-cli-fixture'" if invalid else (
                    "Reth Version: 1.0.0" if "version" in label else "Usage: fixture")
                (work / f"{label}.stdout.log").write_text("" if invalid else text)
                (work / f"{label}.stderr.log").write_text(text if invalid else "")
            with patch.object(bench, "run", side_effect=execute) as invoked:
                bench.smoke_cli(work, "image")
            self.assertEqual(invoked.call_count, 14)
            for call in invoked.call_args_list:
                self.assertEqual(call.kwargs["timeout"], 60)
            with patch.object(bench, "run"):
                (work / "eez-composer-help.stdout.log").write_text("wrong output")
                with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(ValueError, "missing expected"):
                    bench.smoke_cli(work, "image")

    def test_expected_invalid_argument_exit_does_not_accept_crashes(self):
        for code in (0, 1, 2, 137):
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory:
                with patch.object(bench.subprocess, "run", return_value=subprocess.CompletedProcess([], code)), \
                        contextlib.redirect_stdout(io.StringIO()):
                    if code == 2:
                        bench.run(Path(directory), "image", "invalid", ["binary"], expected_code=2)
                    else:
                        with self.assertRaises(RuntimeError):
                            bench.run(Path(directory), "image", "invalid", ["binary"], expected_code=2)

    def test_smoke_failure_keeps_build_measurements_but_never_archives(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            def stage(destination):
                destination.mkdir()
                return "fixture"
            def run(work, image, label, command, **kwargs):
                (work / f"{label}.stdout.log").write_text("fixture environment")
                return {"seconds": 123, "exit_code": 0}
            with patch.object(bench, "stage_source", side_effect=stage), \
                    patch.object(bench, "output", side_effect=lambda *args: (
                        "160000 commit protocol" if args[1] == "ls-tree" else "456")), \
                    patch.object(bench.shutil, "disk_usage", return_value=shutil._ntuple_diskusage(0, 0, 40 * 1024**3)), \
                    patch.object(bench, "run", side_effect=run), \
                    patch.object(bench, "freshness", return_value={"all_fresh": False}), \
                    patch.object(bench, "hashes", return_value={"binary": "digest"}), \
                    patch.object(bench, "smoke_cli", side_effect=RuntimeError("CLI failed")), \
                    patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(work / "summary")}), \
                    contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(RuntimeError, "CLI failed"):
                    bench.benchmark("seed", work, "image")
            report = json.loads((work / "result.json").read_text())
            self.assertEqual(report["build"]["seconds"], 123)
            self.assertEqual(report["target_bytes"], 456)
            self.assertNotEqual(report["status"], "complete")
            self.assertIn("build measured", (work / "summary").read_text())
            self.assertFalse((work / "state.tar.gz").exists())
            self.assertFalse((work / "state/seed.json").exists())

    def test_command_matches_production_shipping_selection(self):
        dockerfile = (REPO / "Dockerfile.runtime").read_text().replace("\\\n", " ")
        production = re.search(r"runtime (cargo build .*?)\s+&& install", dockerfile).group(1)
        self.assertEqual(shlex.split(production), list(bench.BUILD))

    def test_native_packages_match_production(self):
        packages = []
        for name in ("Dockerfile.runtime", "scripts/ci/Dockerfile.cache-benchmark"):
            content = (REPO / name).read_text().replace("\\\n", " ")
            packages.append(shlex.split(re.search(r"install -y --no-install-recommends\s+(.*?)&&", content).group(1)))
        self.assertEqual(*packages)

    def test_container_caps_and_private_state(self):
        args = bench.container(Path("/owned/experiment"), "image", ["cargo", "build"])
        for option in ("--cpus=4", "--memory=20g", "--memory-swap=20g", "--network=none",
                       "--read-only", "--env=CARGO_INCREMENTAL=0", "--env=CARGO_BUILD_JOBS=4",
                       "--env=RUSTFLAGS=-C link-arg=-fuse-ld=mold"):
            self.assertIn(option, args)
        self.assertEqual(args.count("--mount"), 1)
        self.assertNotIn("docker.sock", " ".join(args))
        self.assertNotIn("sccache", " ".join(args))
        self.assertIn("--network=bridge", bench.container(Path("/owned"), "image", ["cargo", "fetch"], True))

    def test_freshness_requires_artifacts_and_counts_recompilation(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "cargo.log"
            log.write_text('{"reason":"build-finished","success":true}\n')
            with self.assertRaisesRegex(ValueError, "no compilation evidence"):
                bench.freshness(log)
            log.write_text("\n".join(json.dumps({"reason": "compiler-artifact", "fresh": f}) for f in (True, False)))
            self.assertEqual(bench.freshness(log), {"artifacts": 2, "fresh": 1, "all_fresh": False})

    def test_archive_size_is_a_hard_upload_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "state.tar.gz"
            archive.write_bytes(b"abc")
            self.assertEqual(bench.check_archive_size(archive), 3)
            with patch.object(bench, "MAX_ARCHIVE", 2):
                with self.assertRaisesRegex(ValueError, "do not upload"):
                    bench.check_archive_size(archive)

    def test_staging_excludes_untracked_files_and_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / "Cargo.toml").write_text("tracked")
            (repo / ".env").write_text("not for the cache")
            with patch.object(bench, "REPO", repo), patch.object(bench, "output", return_value="Cargo.toml\0"), \
                    patch.object(bench.subprocess, "run"):
                bench.stage_source(repo / "copy")
                self.assertEqual([p.name for p in (repo / "copy").iterdir()], ["Cargo.toml"])
                (repo / "Cargo.toml").unlink()
                (repo / "Cargo.toml").symlink_to(repo / ".env")
                with self.assertRaisesRegex(ValueError, "regular tracked file"):
                    bench.stage_source(repo / "other")

    def test_only_execution_branch_triggers_and_no_publication(self):
        workflow = yaml.safe_load((REPO / ".github/workflows/rollupx-cache-benchmark.yml").read_text())
        self.assertEqual(workflow.get("on", workflow.get(True)),
                         {"push": {"branches": ["test/rollupx-cache-benchmark"]}})
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        self.assertEqual(set(workflow["jobs"]), {"correctness"})
        self.assertEqual(workflow["env"]["TEST_SEED_KEY"], "rollupx-test-state-probe-v1-36188091462-1")
        caches = []
        for job in workflow["jobs"].values():
            self.assertEqual(job["runs-on"], "homelab-rust")
            for step in job["steps"]:
                self.assertNotIn("docker push", step.get("run", ""))
                if step.get("uses", "").startswith("actions/cache/"):
                    self.assertTrue(step["uses"].startswith("actions/cache/restore@"))
                    caches.append(step["with"])
        self.assertEqual(len(caches), 1)
        self.assertEqual(caches[0]["path"], "artifacts/shipping-cache/test-state.tar.gz")
        self.assertTrue(caches[0]["fail-on-cache-miss"])
        self.assertNotIn("restore-keys", caches[0])
        steps = workflow["jobs"]["correctness"]["steps"]
        commands = "\n".join(step.get("run", "") for step in steps)
        for required in ('prepare-runtime-artifacts.sh', '--suite unit', 'export-e2e',
                         'run-native-suite.sh embedded', 'run-native-suite.sh cross-chain',
                         'testing/kurtosis/run-ci.sh'):
            self.assertIn(required, commands)
        self.assertNotIn('--partition', commands)
        self.assertNotIn('build-runtime-artifacts.sh', commands)
        for step in steps:
            if 'run-native-suite.sh' in step.get('run', '') or 'run-ci.sh' in step.get('run', ''):
                self.assertFalse(step.get('continue-on-error', False))
            if 'run-ci.sh' in step.get('run', ''):
                self.assertIn('github.run_id', step['env']['KURTOSIS_ENCLAVE'])
                self.assertIn('github.run_attempt', step['env']['KURTOSIS_ENCLAVE'])
