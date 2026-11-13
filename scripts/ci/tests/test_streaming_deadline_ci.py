"""The optional measurement dispatch cannot replace normal release validation."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml


REPO = Path(__file__).resolve().parents[3]
MANUAL = "${{ github.event_name == 'workflow_dispatch' && inputs.measure_streaming_deadlines && needs.build-runtime.outputs.published == 'true' }}"


class StreamingDeadlineWorkflowTest(unittest.TestCase):
    def setUp(self):
        self.workflow = yaml.safe_load((REPO / ".github/workflows/ci.yml").read_text())
        self.jobs = self.workflow["jobs"]

    def test_default_events_keep_every_normal_validation_and_promotion_gate(self):
        for name in ("build-runtime", "lint", "debug-unit-tests"):
            self.assertNotIn("if", self.jobs[name])
        result = self.jobs["ci-result"]
        self.assertEqual(result["if"], "always()")
        self.assertEqual(set(result["needs"]), {
            "build-runtime", "lint", "debug-unit-tests", "embedded-e2e", "cross-chain-e2e", "kurtosis-e2e", "streaming-deadline-measurement"})
        promotion = next(s for s in result["steps"] if s["name"] == "Publish commit and latest image tags")
        self.assertEqual(promotion["if"], "${{ success() && github.ref == 'refs/heads/main' && needs.build-runtime.outputs.published == 'true' }}")
        for name in ("embedded-e2e", "cross-chain-e2e", "kurtosis-e2e"):
            self.assertEqual(self.jobs[name]["needs"], "build-runtime")
            self.assertEqual(self.jobs[name]["if"], "${{ needs.build-runtime.outputs.published == 'true' }}")
        trigger = self.workflow.get("on", self.workflow.get(True))
        option = trigger["workflow_dispatch"]["inputs"]["measure_streaming_deadlines"]
        self.assertEqual(option, {
            "description": "Also measure streaming deadlines after consuming this run's verified release artifacts",
            "type": "boolean", "required": False, "default": False})
        self.assertEqual(self.workflow["concurrency"]["cancel-in-progress"], "${{ github.event_name == 'pull_request' }}")
        self.assertIn("github.run_id", self.workflow["concurrency"]["group"])

    def test_manual_job_uses_existing_read_only_consumer_without_compilation(self):
        job = self.jobs["streaming-deadline-measurement"]
        self.assertEqual(job["if"], MANUAL)
        self.assertEqual(job["runs-on"], "ubuntu-24.04")
        self.assertEqual(job["permissions"], {"contents": "read", "packages": "read"})
        self.assertNotIn("continue-on-error", job)
        self.assertEqual(job["needs"], ["build-runtime", "cross-chain-e2e"])
        first = job["steps"][0]["run"]
        self.assertIn('case "$GITHUB_REF" in refs/heads/codex/*)', first)
        consumer = next(s for s in job["steps"] if s.get("uses") == "./.github/actions/native-test-artifact")
        self.assertEqual(consumer["with"]["payload-ref"], "${{ needs.build-runtime.outputs.tests-ref }}")
        for field, key in (("RUN_ID", "producer-run-id"), ("RUN_ATTEMPT", "producer-run-attempt")):
            self.assertEqual(job["env"]["EEZ_NATIVE_PRODUCER_" + field], "${{ needs.build-runtime.outputs." + key + " }}")
        commands = "\n".join(s.get("run", "") for s in job["steps"])
        self.assertIn('native-tests.py measure "$EEZ_NATIVE_TEST_DIR"', commands)
        self.assertNotIn("${{", commands, "untrusted input must travel through quoted environment/action inputs")
        for forbidden in ("GITHUB_SHA=", "cargo build", "cargo test", "docker build", "docker push", "swapoff", "init-cargo-target", "run-native-suite", "forge build"):
            self.assertNotIn(forbidden, commands)
        self.assertFalse(any("compiler-cache" in s.get("uses", "") for s in job["steps"]))
        producer = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())
        publishing = next(s for s in producer["jobs"]["build"]["steps"] if s.get("id") == "payloads")
        for key, variable in (("producer-run-id", "GITHUB_RUN_ID"), ("producer-run-attempt", "GITHUB_RUN_ATTEMPT")):
            self.assertIn(f'echo "{key}=${variable}"', publishing["run"])
            self.assertEqual(producer["jobs"]["build"]["outputs"][key], "${{ steps.payloads.outputs." + key + " }}")

    def test_diagnostics_are_retained_on_rejection_without_uploading_payload_bytes(self):
        job = self.jobs["streaming-deadline-measurement"]
        upload = next(s for s in job["steps"] if s.get("uses", "").startswith("actions/upload-artifact@"))
        self.assertEqual(upload["if"], "always()")
        self.assertEqual(upload["with"]["path"].split(), [
            "artifacts/streaming-deadlines", "artifacts/streaming-deadlines.artifact-provenance.json",
            "artifacts/streaming-deadlines.artifact-post.json", "artifacts/streaming-deadlines.log"])
        self.assertNotIn("target", upload["with"]["path"])
        self.assertIn("github.run_attempt", upload["with"]["name"])

    def test_requested_and_not_requested_outcomes_preserve_every_normal_lane(self):
        gate = self.jobs["ci-result"]["steps"][0]["run"]
        names = self.jobs["ci-result"]["needs"]
        measurement = "streaming-deadline-measurement"
        def evaluate(rows, requested):
            return subprocess.run(["bash", "-e", "-c", gate], text=True, capture_output=True,
                env=dict(os.environ, RESULTS=json.dumps(rows), MEASUREMENT_REQUESTED=requested)).returncode
        gate_step = self.jobs["ci-result"]["steps"][0]
        self.assertEqual(gate_step["env"]["MEASUREMENT_REQUESTED"], "${{ github.event_name == 'workflow_dispatch' && inputs.measure_streaming_deadlines }}")
        measure_step = next(s for s in self.jobs["streaming-deadline-measurement"]["steps"] if s.get("name", "").startswith("Measure the verified"))
        self.assertEqual(measure_step["timeout-minutes"], 225)
        for requested in ("false", "true"):
            rows = {name: {"result": ("skipped" if requested == "false" else "success") if name == measurement else "success",
                           "outputs": {"published": "true"} if name == "build-runtime" else {}} for name in names}
            self.assertEqual(evaluate(rows, requested), 0)
            for name in names:
                for status in ("failure", "cancelled", "skipped"):
                    if name == measurement and requested == "false" and status == "skipped":
                        continue
                    broken = json.loads(json.dumps(rows)); broken[name]["result"] = status
                    self.assertNotEqual(evaluate(broken, requested), 0, (requested, name, status))
            if requested == "false":
                rows[measurement]["result"] = "success"
                self.assertNotEqual(evaluate(rows, requested), 0)
        unpublished = {name: {"result": "skipped" if name in
                       ("embedded-e2e", "cross-chain-e2e", "kurtosis-e2e") else "success",
                       "outputs": {"published": "false"} if name == "build-runtime" else {}}
                       for name in names}
        # The success shape isolates the publication guard; skipped is reachable.
        for outcome in ("success", "skipped"):
            unpublished[measurement]["result"] = outcome
            self.assertNotEqual(evaluate(unpublished, "true"), 0)
        valid_default = {name: {"result": "skipped" if name == measurement else "success",
                               "outputs": {"published": "true"} if name == "build-runtime" else {}}
                         for name in names}
        self.assertEqual(evaluate(valid_default, "false"), 0)
        for invalid in ("", "yes", "invalid"):
            self.assertNotEqual(evaluate(valid_default, invalid), 0)

    def test_real_git_observer_preflight_rejects_old_staging_accepts_runner_temp(self):
        # Real Git and observer preflight; executables are stubs and resource
        # observation stops deliberately. This supplies no native/timing evidence.
        with tempfile.TemporaryDirectory(prefix="eez-measure-preflight.") as temporary:
            root = Path(temporary); workspace = root / "workspace"; workspace.mkdir()
            runner_temp = root / "runner-temp"; runner_temp.mkdir()
            script = workspace / "scripts/measure-streaming-deadlines.py"; script.parent.mkdir()
            shutil.copy2(REPO / "scripts/measure-streaming-deadlines.py", script)
            shutil.copy2(REPO / ".gitignore", workspace / ".gitignore")
            (workspace / "Cargo.lock").write_text("preflight fixture lock\n")
            clean_env = {key: value for key, value in os.environ.items()
                         if not key.startswith("GIT_")}
            env = dict(clean_env, GITHUB_WORKSPACE=str(workspace), RUNNER_TEMP=str(runner_temp),
                       GITHUB_ENV=str(root / "old-env"), GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=workspace, env=env, text=True).strip()
            git("init", "-q"); git("add", ".")
            git("-c", "user.name=Preflight Fixture", "-c", "user.email=fixture@example.invalid",
                "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "fixture")
            spec = importlib.util.spec_from_file_location("observer_preflight", script)
            observer = importlib.util.module_from_spec(spec); spec.loader.exec_module(observer)
            test = root / "release/deps/node_e2e"; test.parent.mkdir(parents=True)
            test.write_text("#!/bin/sh\nprintf '%s\\n' '" + observer.TEST + ": test'\n"); test.chmod(0o755)
            subprocess.run(["bash", str(REPO / "scripts/ci/init-cargo-target.sh")], cwd=workspace, env=env, check=True, capture_output=True)
            old_target = Path((root / "old-env").read_text().strip().split("=", 1)[1])
            old_signer = old_target / "release/eez-proof-signer"; old_signer.parent.mkdir()
            old_signer.write_text("signer bytes")
            self.assertIn(".cargo-target.", git("status", "--porcelain"))
            with patch.dict(os.environ, env, clear=True), \
                    patch.object(observer, "snapshot", side_effect=AssertionError("old staging must fail before observation")), \
                    patch.object(sys, "argv", [str(script), str(test), str(old_signer), str(root / "old-evidence")]):
                with self.assertRaisesRegex(RuntimeError, "commit the reviewed driver"):
                    observer.main()
            shutil.rmtree(old_target)
            env["GITHUB_ENV"] = str(root / "new-env")
            prepare = next(s for s in self.jobs["streaming-deadline-measurement"]["steps"] if s.get("name") == "Initialize owned measurement staging")
            subprocess.run(["bash", "-e", "-o", "pipefail", "-c", prepare["run"]], cwd=workspace, env=env, check=True, capture_output=True)
            new_target = Path((root / "new-env").read_text().strip().split("=", 1)[1])
            self.assertFalse(new_target.is_relative_to(workspace))
            new_signer = new_target / "release/eez-proof-signer"; new_signer.parent.mkdir()
            new_signer.write_text("signer bytes")
            (workspace / "artifacts").mkdir()
            (workspace / "artifacts/measurement-preflight.log").write_text("owned ignored diagnostic")
            self.assertEqual(git("status", "--porcelain"), "")
            evidence = root / "new-evidence"
            with patch.dict(os.environ, env, clear=True), \
                    patch.object(sys, "argv", [str(script), str(test), str(new_signer), str(evidence)]), \
                    patch.object(observer, "snapshot", side_effect=ValueError("controlled resource boundary reached")) as reached:
                self.assertEqual(observer.main(), 1)
            reached.assert_called_once()
            self.assertEqual(json.loads((evidence / "manifest.json").read_text())["changes"], [])
            summary = json.loads((evidence / "summary.json").read_text())
            self.assertFalse(summary["eligible_performance_evidence"])
            self.assertIn("controlled resource boundary reached", summary["status"])


if __name__ == "__main__":
    unittest.main()
