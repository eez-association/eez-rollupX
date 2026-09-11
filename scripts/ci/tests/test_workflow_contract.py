"""Validate both execution modes and failure-only diagnostics, including the gate."""
import copy
import json
import os
from pathlib import Path
import subprocess
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

    def test_three_initial_lanes_keep_all_consolidated_checks(self):
        self.assertEqual({name for name, job in self.jobs.items() if not job.get("needs")},
                         {"build-runtime", "lint", "contract-tests"})
        lint = "\n".join(s.get("run", "") for s in self.jobs["lint"]["steps"])
        for command in ("cargo fmt", "cargo deny check", "cargo audit", "cargo clippy",
                        "unittest discover"):
            self.assertIn(command, lint)
        steps = self.jobs["contract-tests"]["steps"]
        self.assertEqual(sum(s.get("run", "").startswith("forge test") for s in steps), 2)
        self.assertFalse(any("update-eezl2-genesis" in s.get("run", "") for s in steps))
        network = (REPO / "testing/kurtosis/run-ci.sh").read_text()
        self.assertIn('bash "$REPO/scripts/update-eezl2-genesis.sh" --check', network)
        self.assertLess(network.index("update-eezl2-genesis.sh"), network.index('bash "$HERE/start.sh"'))
        self.assertTrue(any(s.get("run") == "bash scripts/ci/run-protocol-anvil.sh" for s in steps))
        self.assertTrue(any(s.get("run") == "forge build" and s.get("working-directory") == "eez-core-protocol"
                            for s in steps))

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

    def test_kurtosis_consumer_requires_publication_and_unconditional_login(self):
        caller = self.jobs["kurtosis-e2e"]
        self.assertEqual(caller["if"], "${{ needs.build-runtime.outputs.published == 'true' }}")
        self.assertEqual(set(caller["with"]), {"runner", "runtime-ref"})
        workflow = yaml.safe_load((REPO / ".github/workflows/kurtosis-e2e.yml").read_text())
        trigger = workflow.get("on", workflow.get(True))  # PyYAML's YAML 1.1 parses 'on' as true.
        self.assertEqual(set(trigger["workflow_call"]["inputs"]), {"runner", "runtime-ref"})
        steps = workflow["jobs"]["kurtosis-e2e"]["steps"]
        self.assertFalse(any("init-cargo-target.sh" in s.get("run", "") for s in steps))
        diagnostics = next(s for s in steps if s.get("run") == "bash scripts/ci/cargo-failure-diagnostics.sh")
        self.assertEqual(diagnostics["if"], "failure()")
        login = next(s for s in steps if s.get("id") == "ghcr-login")
        self.assertNotIn("if", login)
        prepare = next(s for s in steps if "prepare-runtime-artifacts.sh" in s.get("run", ""))
        self.assertEqual(prepare["env"]["EEZ_EXPECT_PUBLISHED"], "true")
        self.assertLess(steps.index(login), steps.index(prepare))

    def test_optional_auxiliary_pool_never_overrides_a_manual_runner_choice(self):
        for name, job in self.jobs.items():
            if name == "ci-result":
                continue
            expression = job["with"]["runner"] if "uses" in job else job["runs-on"]
            # Assert the exact supported grammar before evaluating its fallbacks.
            fields = expression.removeprefix("${{ ").removesuffix(" }}").split(" || ")
            expected = ["inputs.runner"]
            if name in ("lint", "contract-tests"):
                expected.append("vars.CI_AUX_RUNNER")
            expected += ["vars.CI_RUNNER", "'homelab-rust'"]
            self.assertEqual(fields, expected)

            def selected(manual="", primary="", auxiliary=""):
                values = {"inputs.runner": manual, "vars.CI_RUNNER": primary, "vars.CI_AUX_RUNNER": auxiliary,
                          "'homelab-rust'": "homelab-rust"}
                return next(values[field] for field in fields if values[field])

            with self.subTest(job=name):
                self.assertEqual(selected(manual="ubuntu-24.04", primary="homelab-rust", auxiliary="homelab-rust"),
                                 "ubuntu-24.04")
                self.assertEqual(selected(manual="homelab-rust", auxiliary="ubuntu-24.04"), "homelab-rust")
                self.assertEqual(selected(primary="ubuntu-24.04"), "ubuntu-24.04")
                self.assertEqual(selected(primary="homelab-rust", auxiliary="ubuntu-24.04"),
                                 "ubuntu-24.04" if name in ("lint", "contract-tests") else "homelab-rust")
                self.assertEqual(selected(primary="homelab-rust", auxiliary="homelab-small"),
                                 "homelab-small" if name in ("lint", "contract-tests") else "homelab-rust")

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


if __name__ == "__main__":
    unittest.main()
