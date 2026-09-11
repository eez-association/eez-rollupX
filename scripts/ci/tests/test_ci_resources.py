"""Runner resource setup tests; never delete installed SDKs or real build outputs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


class CIResourcesTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="eez-ci-resources.")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.commands = self.root / "commands.jsonl"
        self.commands.touch()
        self.tools = self.root / "bin"
        self.tools.mkdir()
        for tool in ("sudo", "df", "free", "ps", "find", "sccache", "forge", "python3"):
            path = self.tools / tool
            path.write_text(f"#!{sys.executable}\n" + '''
import json, os, sys
from pathlib import Path
tool = Path(sys.argv[0]).name
with open(os.environ["CI_TEST_COMMANDS"], "a") as stream:
    stream.write(json.dumps([tool] + sys.argv[1:]) + "\\n")
if tool in ("forge", "python3"):
    target = Path(os.environ["CARGO_TARGET_DIR"])
    (target / (tool + ".json")).write_text(json.dumps({
        "target": str(target), "incremental": os.environ.get("CARGO_INCREMENTAL"),
        "log_dir": os.environ.get("EEZ_TEST_LOG_DIR"),
        "args": sys.argv[1:]}))
    print(tool + " output")
    key = "CI_TEST_FORGE_EXIT" if tool == "forge" else "CI_TEST_NEXTEST_EXIT"
    sys.exit(int(os.environ.get(key, "0")))
''')
            path.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.tools) + os.pathsep + os.environ["PATH"],
                        CI_TEST_COMMANDS=str(self.commands), GITHUB_WORKSPACE=str(self.root),
                        GITHUB_ENV=str(self.root / "github-env"))
        for key in ("CARGO_TARGET_DIR", "CARGO_INCREMENTAL", "RUSTC_WRAPPER"):
            self.env.pop(key, None)

    def run_script(self, name):
        result = subprocess.run(["bash", str(REPO / "scripts/ci" / name)],
                                env=self.env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def calls(self):
        return [json.loads(line) for line in self.commands.read_text().splitlines()]

    def test_cleanup_is_a_noop_on_self_hosted_and_unknown_runners(self):
        for environment in ("self-hosted", ""):
            self.env["RUNNER_ENVIRONMENT"] = environment
            self.run_script("prepare-hosted-runner.sh")
            self.assertEqual(self.calls(), [])

    def test_hosted_cleanup_only_removes_allowlisted_sdks(self):
        self.env["RUNNER_ENVIRONMENT"] = "github-hosted"
        self.run_script("prepare-hosted-runner.sh")
        self.assertEqual([call for call in self.calls() if call[0] == "sudo"], [[
            "sudo", "rm", "-rf", "/usr/local/lib/android", "/usr/share/dotnet",
            "/opt/ghc", "/usr/local/.ghcup",
        ]])

    def test_each_target_is_new_and_existing_outputs_are_preserved(self):
        self.run_script("init-cargo-target.sh")
        first = Path((self.root / "github-env").read_text().strip().split("=", 1)[1])
        (first / "keep").write_text("active artifact")
        self.env["CARGO_TARGET_DIR"] = str(first)
        self.run_script("init-cargo-target.sh")
        second = Path((self.root / "github-env").read_text().splitlines()[-1].split("=", 1)[1])
        self.assertNotEqual(first, second)
        self.assertEqual(second.parent, self.root)
        self.assertEqual((first / "keep").read_text(), "active artifact")
        self.assertTrue(second.is_dir())

    def test_e2e_suites_publish_their_actual_directory_even_on_failure(self):
        workspace = self.root / "workspace with spaces"
        workspace.mkdir()
        self.env.update(GITHUB_WORKSPACE=str(workspace), RUNNER_TEMP=str(self.root))
        self.run_script("init-cargo-target.sh")
        unit_target = Path((self.root / "github-env").read_text().strip().split("=", 1)[1])
        (unit_target / "keep").write_text("unit archive extraction")
        self.env["CARGO_TARGET_DIR"] = str(unit_target)
        payload = self.root / "execution payload"
        for runner in ("github-hosted", "self-hosted"):
            for suite, forge_exit, test_exit in (("embedded", 0, 101), ("cross-chain", 0, 0),
                                                ("embedded", 12, 0)):
                with self.subTest(runner=runner, suite=suite, forge_exit=forge_exit, test_exit=test_exit):
                    self.env.update(RUNNER_ENVIRONMENT=runner, CI_TEST_FORGE_EXIT=str(forge_exit),
                                    CI_TEST_NEXTEST_EXIT=str(test_exit))
                    previous = set(workspace.glob(".cargo-target.*"))
                    result = subprocess.run(["bash", str(REPO / "scripts/ci/run-native-suite.sh"),
                                             suite, str(payload)], cwd=workspace, env=self.env,
                                            capture_output=True, text=True, timeout=30)
                    self.assertEqual(result.returncode, forge_exit or test_exit, result.stdout + result.stderr)
                    # Simulate the runner exporting GITHUB_ENV to the next step.
                    self.env.update(line.split("=", 1) for line in (self.root / "github-env").read_text().splitlines())
                    target = Path(self.env["CARGO_TARGET_DIR"])
                    self.assertEqual(set(workspace.glob(".cargo-target.*")) - previous, {target})
                    logs = workspace / f"artifacts/{suite}-e2e/logs"
                    expected = {"target": str(target), "incremental": "0", "log_dir": str(logs),
                                "args": ["build", "--root", "contracts"]}
                    self.assertEqual(json.loads((target / "forge.json").read_text()), expected)
                    self.assertTrue(logs.is_dir())
                    if forge_exit:
                        self.assertFalse((target / "python3.json").exists())
                    else:
                        expected["args"] = ["scripts/ci/native-tests.py", "run", str(payload), "--suite", suite]
                        self.assertEqual(json.loads((target / "python3.json").read_text()), expected)
                        log = workspace / f"artifacts/{suite}-e2e/nextest.log"
                        self.assertIn("python3 output", log.read_text())
                    diagnostics = self.run_script("cargo-failure-diagnostics.sh")
                    self.assertIn(f"CARGO_TARGET_DIR={target}\n", diagnostics.stdout)
                    self.assertNotIn("directory disappeared", diagnostics.stdout)
                    self.assertEqual((unit_target / "keep").read_text(), "unit archive extraction")

    def test_diagnostics_work_before_cargo_initialization(self):
        result = self.run_script("cargo-failure-diagnostics.sh")
        self.assertNotIn("::error::Cargo target", result.stdout)
        self.assertFalse(any(call[0] == "find" for call in self.calls()))
        self.assertIn(["sccache", "--show-stats"], self.calls())

    def test_diagnostics_identify_a_disappeared_target(self):
        self.env["CARGO_TARGET_DIR"] = str(self.root / "missing-target")
        result = self.run_script("cargo-failure-diagnostics.sh")
        self.assertIn("Cargo target directory disappeared", result.stdout)


if __name__ == "__main__":
    unittest.main()
