"""Validate both execution modes and failure-only diagnostics, including the gate."""
import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

try:
    import yaml
except ImportError:
    yaml = None


REPO = Path(__file__).resolve().parents[3]


@unittest.skipIf(yaml is None, "PyYAML not installed; validate workflows with actionlint")
class WorkflowContractTest(unittest.TestCase):
    def setUp(self):
        self.jobs = yaml.safe_load((REPO / ".github/workflows/ci.yml").read_text())["jobs"]
        self.gate = self.jobs["ci-result"]["steps"][0]["run"]

    def result(self, published):
        return {name: {"result": "skipped" if published == "false" and name in
                       ("embedded-e2e", "cross-chain-e2e", "kurtosis-e2e") else "success",
                       "outputs": {"published": published} if name == "build-runtime" else {}}
                for name in self.jobs["ci-result"]["needs"]}

    def run_gate(self, results):
        return subprocess.run(["bash", "-e", "-c", self.gate], text=True, capture_output=True,
                              env=dict(os.environ, RESULTS=json.dumps(results), PYTHONOPTIMIZE="1"))

    def test_both_execution_modes_pass_and_every_required_lane_fails_closed(self):
        for mode in ("true", "false"):
            results = self.result(mode)
            self.assertEqual(self.run_gate(results).returncode, 0)
            for name in results:
                broken = copy.deepcopy(results)
                broken[name]["result"] = "failure"
                with self.subTest(mode=mode, job=name):
                    self.assertNotEqual(self.run_gate(broken).returncode, 0)
        for mode in (None, "", "invalid"):
            self.assertNotEqual(self.run_gate(self.result(mode)).returncode, 0)

    def test_all_jobs_participate_in_gate(self):
        self.assertEqual(set(self.jobs)-{"ci-result"}, set(self.jobs["ci-result"]["needs"]))
        results = self.result("true")
        results["kurtosis-e2e"]["result"] = "skipped"
        self.assertNotEqual(self.run_gate(results).returncode, 0)

    def test_debug_unit_lane_is_required_in_both_execution_modes(self):
        # It does not depend on publication, so it must never be skipped.
        self.assertIn("debug-unit-tests", self.jobs["ci-result"]["needs"])
        for mode in ("true", "false"):
            for state in ("failure", "cancelled", "skipped"):
                with self.subTest(mode=mode, state=state):
                    results = self.result(mode)
                    results["debug-unit-tests"]["result"] = state
                    self.assertNotEqual(self.run_gate(results).returncode, 0)

    def test_native_matrices_cover_every_partition_and_block_promotion_on_failure(self):
        for name in ("embedded-e2e", "cross-chain-e2e"):
            job = self.jobs[name]
            partitions = job["strategy"]["matrix"]["partition"]
            self.assertEqual(partitions, [f"slice:{i}/4" for i in range(1, 5)])
            self.assertFalse(job["strategy"]["fail-fast"])
            self.assertNotIn("continue-on-error", job)
            execution = next(s for s in job["steps"] if "run-native-suite.sh" in s.get("run", ""))
            self.assertEqual(execution["env"]["NEXTEST_PARTITION"], "${{ matrix.partition }}")
            self.assertIn('"$NEXTEST_PARTITION"', execution["run"])
            self.assertNotIn("continue-on-error", execution)
            upload = next(s for s in job["steps"] if s.get("uses", "").startswith("actions/upload-artifact@"))
            self.assertIn("${{ strategy.job-index }}", upload["with"]["name"])
            for state in ("failure", "cancelled", "skipped"):
                results = self.result("true")
                results[name]["result"] = state
                self.assertNotEqual(self.run_gate(results).returncode, 0)

    def test_native_submodule_retries_precede_payload_consumption_and_fail_closed(self):
        for name in ("embedded-e2e", "cross-chain-e2e"):
            steps = self.jobs[name]["steps"]
            checkout = next(step for step in steps if step.get("uses", "").startswith("actions/checkout@"))
            self.assertFalse(checkout["with"]["persist-credentials"])
            self.assertNotIn("submodules", checkout["with"])
            initialize = next(step for step in steps if step.get("run") == "bash scripts/ci/init-submodules.sh")
            payload = next(step for step in steps if step.get("uses") == "./.github/actions/native-test-artifact")
            self.assertEqual(initialize["timeout-minutes"], 6)
            self.assertNotIn("continue-on-error", initialize)
            self.assertLess(steps.index(checkout), steps.index(initialize))
            self.assertLess(steps.index(initialize), steps.index(payload))

    def test_promotion_is_after_the_gate_and_only_on_main(self):
        job = self.jobs["ci-result"]
        promotion = job["steps"][1]
        self.assertEqual(promotion["if"],
                         "${{ success() && github.ref == 'refs/heads/main' && needs.build-runtime.outputs.published == 'true' }}")
        self.assertFalse(job["concurrency"]["cancel-in-progress"])
        self.assertIn("github.ref", job["concurrency"]["group"])
        self.assertEqual(job["permissions"]["packages"], "write")
        self.assertEqual(promotion["env"]["IMAGES"], "${{ needs.build-runtime.outputs.images }}")
        producer = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())
        trigger = producer.get("on", producer.get(True))
        self.assertEqual(trigger["workflow_call"]["outputs"]["images"]["value"],
                         "${{ jobs.build.outputs.images }}")
        self.assertEqual(producer["jobs"]["build"]["outputs"]["images"], "${{ steps.payloads.outputs.images }}")

    def promote(self, *, current=True, fail_api=False, fail_publish=False, images=None):
        with tempfile.TemporaryDirectory(prefix="eez-image-promotion.") as directory:
            root = Path(directory)
            shim = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
tool = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["PROMOTION_LOG"], "a") as log:
    log.write(json.dumps([tool, *args]) + "\n")
if tool == "gh":
    if os.environ["FAIL_API"] == "true": sys.exit(1)
    print(os.environ["MAIN_HEAD"])
elif args[0] == "login":
    assert sys.stdin.read() == "fixture-token"
elif args[:3] == ["buildx", "imagetools", "create"]:
    if os.environ["FAIL_PUBLISH"] == "true": sys.exit(1)
elif args[0] != "logout":
    raise SystemExit("Unexpected Docker operation")
'''
            for name in ("docker", "gh"):
                path = root / name
                path.write_text(shim)
                path.chmod(0o755)
            references = {role: {"reference": f"ghcr.io/eduadiez/rollupx-{suffix}@sha256:" + digit * 64}
                          for role, suffix, digit in (("node", "node", "1"),
                                                      ("signer", "proof-signer", "2"),
                                                      ("deploy", "deploy", "3"))}
            result = subprocess.run(
                ["bash", "-e", "-o", "pipefail", "-c", self.jobs["ci-result"]["steps"][1]["run"]],
                text=True, capture_output=True, env={**os.environ,
                    "PATH": str(root) + os.pathsep + os.environ["PATH"],
                    "GH_TOKEN": "fixture-token", "GITHUB_ACTOR": "operator",
                    "GITHUB_REPOSITORY": "eduadiez/rollupX", "GITHUB_SHA": "a" * 40,
                    "GITHUB_STEP_SUMMARY": str(root / "summary"),
                    "IMAGES": json.dumps(references if images is None else images),
                    "MAIN_HEAD": ("a" if current else "b") * 40,
                    "FAIL_API": str(fail_api).lower(), "FAIL_PUBLISH": str(fail_publish).lower(),
                    "PROMOTION_LOG": str(root / "commands"),
                },
            )
            commands = [json.loads(line) for line in (root / "commands").read_text().splitlines()]
            copies = [command for command in commands if command[1:4] == ["buildx", "imagetools", "create"]]
            return result, copies, references

    def test_promotion_tags_exact_tested_digests_with_bare_commit_and_latest(self):
        result, copies, images = self.promote()
        self.assertEqual(result.returncode, 0, result.stderr)
        expected = {(image["reference"].split("@")[0] + ":" + tag, image["reference"])
                    for image in images.values() for tag in ("a" * 40, "latest")}
        self.assertEqual({(c[c.index("--tag") + 1], c[-1]) for c in copies}, expected)
        self.assertEqual(len(copies), 6)
        self.assertTrue(all("--prefer-index=false" in c for c in copies))

    def test_older_run_publishes_commit_tags_without_moving_latest(self):
        result, copies, _ = self.promote(current=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(copies), 3)
        self.assertTrue(all(c[c.index("--tag") + 1].endswith(":" + "a" * 40) for c in copies))
        self.assertIn("latest unchanged", result.stdout)

    def test_failed_lookup_or_publication_fails_ci_without_claiming_release(self):
        result, copies, _ = self.promote(fail_api=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(copies), 3)
        self.assertFalse(any(c[c.index("--tag") + 1].endswith(":latest") for c in copies))
        result, copies, _ = self.promote(fail_publish=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(copies), 1)

    def test_missing_producer_images_fail_before_tagging(self):
        for images in ({}, {"deploy": {"reference": "ghcr.io/eduadiez/rollupx-deploy@sha256:" + "3" * 64}},
                       {role: {"reference": "ghcr.io/eduadiez/rollupx-node:latest"}
                        for role in ("node", "signer", "deploy")}):
            with self.subTest(images=images):
                result, copies, _ = self.promote(images=images)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(copies, [])

    def test_initial_lanes_keep_lint_unit_and_runtime_checks(self):
        self.assertEqual({name for name, job in self.jobs.items() if not job.get("needs")},
                         {"build-runtime", "lint", "debug-unit-tests", "quote-trace-viewer"})
        lint = "\n".join(s.get("run", "") for s in self.jobs["lint"]["steps"])
        for command in ("cargo fmt", "cargo deny check", "cargo audit", "cargo clippy",
                        "unittest discover"):
            self.assertIn(command, lint)
        self.assertNotIn("contract-tests", self.jobs)
        network = (REPO / "testing/kurtosis/run-ci.sh").read_text()
        self.assertIn('bash "$REPO/scripts/update-eezl2-genesis.sh" --check', network)
        self.assertLess(network.index("update-eezl2-genesis.sh"), network.index('bash "$HERE/start.sh"'))

    def test_viewer_import_and_browser_tests_gate_release(self):
        steps = self.jobs["quote-trace-viewer"]["steps"]
        commands = [step.get("run", "") for step in steps]
        install = next(i for i, command in enumerate(commands) if "npm ci" in command)
        browser = next(i for i, command in enumerate(commands)
                       if "playwright install --with-deps chromium" in command)
        tests = next(i for i, command in enumerate(commands) if "npm test" in command)
        self.assertLess(install, browser)
        self.assertLess(browser, tests)
        self.assertIn("npm run test:browser", commands[tests])
        self.assertIn("quote-trace-viewer", self.jobs["ci-result"]["needs"])
        self.assertFalse(self.jobs["quote-trace-viewer"].get("continue-on-error", False))

    def test_lint_initializes_one_target_and_preserves_optional_hosted_caching(self):
        steps = self.jobs["lint"]["steps"]
        commands = [step.get("run", "") for step in steps]
        init = commands.index("bash scripts/ci/init-cargo-target.sh")
        self.assertEqual(commands.count(commands[init]), 1)
        self.assertLess(init, next(i for i, cmd in enumerate(commands) if "cargo fmt" in cmd))
        self.assertLess(init, next(i for i, cmd in enumerate(commands) if "cargo clippy" in cmd))
        self.assertEqual(self.jobs["lint"]["env"]["CARGO_INCREMENTAL"], "0")
        cache = next(s for s in steps if s.get("uses", "").startswith("Swatinem/rust-cache@"))
        self.assertEqual(cache["if"], "${{ runner.environment == 'github-hosted' }}")
        for key in ("cache-targets", "cache-on-failure", "cache-bin"):
            self.assertEqual(cache["with"][key], "false")
        compiler = next(s for s in steps if s.get("uses") == "./.github/actions/compiler-cache")
        self.assertEqual(compiler["with"]["enabled"], "${{ vars.CI_COMPILER_CACHE != 'false' }}")

    def test_lint_installs_cast_before_tests_that_decode_real_transactions(self):
        steps = self.jobs["lint"]["steps"]
        install = next(i for i, step in enumerate(steps)
                       if step.get("uses", "").startswith("foundry-rs/foundry-toolchain@"))
        tests = next(i for i, step in enumerate(steps)
                     if "unittest discover" in step.get("run", ""))
        self.assertLess(install, tests)
        self.assertEqual(steps[install]["with"], {"version": "v1.7.1"})

    def test_kurtosis_consumer_requires_publication_and_unconditional_login(self):
        caller = self.jobs["kurtosis-e2e"]
        self.assertEqual(caller["if"], "${{ needs.build-runtime.outputs.published == 'true' }}")
        self.assertEqual(set(caller["with"]), {"runtime-ref"})
        workflow = yaml.safe_load((REPO / ".github/workflows/kurtosis-e2e.yml").read_text())
        trigger = workflow.get("on", workflow.get(True))  # PyYAML's YAML 1.1 parses 'on' as true.
        self.assertEqual(set(trigger["workflow_call"]["inputs"]), {"runtime-ref"})
        steps = workflow["jobs"]["kurtosis-e2e"]["steps"]
        self.assertFalse(any("init-cargo-target.sh" in s.get("run", "") for s in steps))
        diagnostics = next(s for s in steps if s.get("run") == "bash scripts/ci/cargo-failure-diagnostics.sh")
        self.assertEqual(diagnostics["if"], "failure()")
        login = next(s for s in steps if s.get("id") == "ghcr-login")
        self.assertNotIn("if", login)
        prepare = next(s for s in steps if "prepare-runtime-artifacts.sh" in s.get("run", ""))
        self.assertEqual(prepare["env"]["EEZ_EXPECT_PUBLISHED"], "true")
        self.assertLess(steps.index(login), steps.index(prepare))

    def test_every_job_uses_hosted_runners_without_selection_overrides(self):
        # A default is insufficient: stale repo variables or reusable-workflow
        # inputs must not route public PR code back onto a private runner.
        for path in (REPO / ".github/workflows").glob("*.yml"):
            workflow = yaml.safe_load(path.read_text())
            trigger = workflow.get("on", workflow.get(True))
            for event in ("workflow_dispatch", "workflow_call"):
                inputs = (trigger.get(event) or {}).get("inputs", {})
                with self.subTest(path=path.name, event=event):
                    self.assertFalse({"runner", "cargo-state-cache"} & set(inputs))
            for name, job in workflow["jobs"].items():
                with self.subTest(path=path.name, job=name):
                    if "uses" in job:
                        self.assertFalse({"runner", "cargo-state-cache"} & set(job.get("with", {})))
                    else:
                        self.assertEqual(job["runs-on"], "ubuntu-24.04")

    def test_success_never_uploads_diagnostics_and_quota_never_hides_test_failure(self):
        for path in (REPO / ".github/workflows").glob("*.yml"):
            for job in yaml.safe_load(path.read_text())["jobs"].values():
                for step in job.get("steps", []):
                    if step.get("uses", "").startswith("actions/upload-artifact@"):
                        with self.subTest(path=path, step=step["name"]):
                            self.assertIn("failure()", step["if"])
                            self.assertTrue(step["continue-on-error"])
                            self.assertLessEqual(step["with"]["retention-days"], 3)

    def test_no_publication_kurtosis_requires_verified_candidate(self):
        steps = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())["jobs"]["build"]["steps"]
        by_id = {s["id"]: s for s in steps if "id" in s}
        self.assertIn("steps.local-runtime.outcome == 'success'", by_id["local-kurtosis"]["if"])
        self.assertIn("prepare-runtime-artifacts.sh", by_id["local-runtime"]["run"])
        self.assertEqual(by_id["local-runtime"]["env"]["EEZ_EXPECT_PUBLISHED"], "false")
        self.assertTrue(all(not s.get("continue-on-error", False) for s in steps if "run" in s))

    def test_hosted_protocol_concurrency_is_bounded_in_both_publication_paths(self):
        workflow = yaml.safe_load((REPO / ".github/workflows/kurtosis-e2e.yml").read_text())
        self.assertEqual(workflow["jobs"]["kurtosis-e2e"]["env"]["EEZ_PROTOCOL_MAX_PARALLEL"], "2")
        producer = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())
        local = next(s for s in producer["jobs"]["build"]["steps"]
                     if s.get("run") == "bash testing/kurtosis/run-ci.sh")
        self.assertEqual(local["env"]["EEZ_PROTOCOL_MAX_PARALLEL"], "2")

    def test_compile_cap_is_not_repository_configuration(self):
        # The producer sizes the cap from its Docker daemon. Overrides belong to
        # the runner environment (the DinD sidecar's limits), not to workflows.
        job = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())["jobs"]["build"]
        self.assertFalse({"EEZ_CI_BUILD_CPUS", "EEZ_CI_BUILD_MEMORY"} & set(job["env"]))
        for path in (REPO / ".github").rglob("*.yml"):
            with self.subTest(path=path.name):
                text = path.read_text()
                for name in ("CI_BUILD_CPUS", "CI_BUILD_MEMORY"):
                    self.assertNotIn(name, text)

    def test_hosted_build_cannot_enable_or_restore_homelab_cargo_state(self):
        workflow = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())
        job = workflow["jobs"]["build"]
        steps = job["steps"]
        by_id = {s["id"]: s for s in steps if "id" in s}
        self.assertEqual(job["env"]["EEZ_CARGO_STATE_CACHE"], "false")
        self.assertFalse({"cargo-state", "cargo-restore", "cargo-export"} & set(by_id))
        self.assertFalse(any(s.get("uses", "").startswith("actions/cache/") for s in steps))
        unit = next(s for s in steps if '--suite producer' in s.get("run", ""))
        build = next(s for s in steps if s.get("run") == 'bash scripts/ci/build-runtime-artifacts.sh')
        self.assertLess(steps.index(build), steps.index(unit))
        self.assertNotIn("continue-on-error", unit)
        self.assertNotIn("continue-on-error", build)


if __name__ == "__main__":
    unittest.main()
