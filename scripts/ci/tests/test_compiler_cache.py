"""Compiler cache selection, isolation, metrics and failure semantics; offline."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


class CompilerCacheTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="eez-compiler-cache-test.")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.log = self.root / "commands.jsonl"
        self.env = {k: v for k, v in os.environ.items()
                    if not k.startswith(("SCCACHE_", "EEZ_", "ACTIONS_", "RUSTC_WRAPPER"))}
        self.env.update(PATH=str(self.tools) + os.pathsep + os.environ["PATH"],
                        GITHUB_ENV=str(self.root / "github-env"), RUNNER_TEMP=str(self.root),
                        TMPDIR=str(self.root), TEST_CACHE_COMMANDS=str(self.log),
                        EEZ_BUILD_METRICS_DIR=str(self.root / "metrics"))
        for tool in ("sccache", "cargo"):
            path = self.tools / tool
            path.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
tool = Path(sys.argv[0]).name
with open(os.environ["TEST_CACHE_COMMANDS"], "a") as f:
    f.write(json.dumps({"tool": tool, "args": sys.argv[1:],
                       "wrapper": os.environ.get("RUSTC_WRAPPER"),
                       "socket": os.environ.get("SCCACHE_SERVER_UDS"),
                       "gha_v2": os.environ.get("ACTIONS_CACHE_SERVICE_V2"),
                       "redis_options": {key: value for key, value in os.environ.items()
                                         if key.startswith("SCCACHE_REDIS_")}}) + "\\n")
if tool == "cargo":
    sys.exit(int(os.environ.get("TEST_CARGO_EXIT", "0")))
# sccache 0.17 checks presence, not whether these variables are nonempty.
# This validation also runs for non-Redis backends and stats/stop commands.
if all(key in os.environ for key in ("SCCACHE_REDIS_TTL", "SCCACHE_REDIS_EXPIRATION")):
    print("You mustn't set both SCCACHE_REDIS_EXPIRATION and SCCACHE_REDIS_TTL", file=sys.stderr)
    sys.exit(2)
if "--start-server" in sys.argv and os.environ.get("TEST_CACHE_DOWN"):
    print("private://password@cache", file=sys.stderr)
    sys.exit(1)
if "--show-stats" in sys.argv:
    print(json.dumps({"stats": {"compile_requests": 12, "cache_hits": {"counts": {"Rust": 10}},
                               "cache_misses": {"counts": {"Rust": 2}}},
                      "cache_location": "private://password@cache"}))
''')
            path.chmod(0o755)

    def setup_cache(self, **values):
        self.env.update(values)
        result = subprocess.run(["bash", str(REPO / "scripts/ci/setup-compiler-cache.sh")],
                                cwd=REPO, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return dict(line.split("=", 1) for line in (self.root / "github-env").read_text().splitlines())

    def measured_build(self, name="fixture"):
        return subprocess.run(["bash", str(REPO / "scripts/ci/with-compiler-cache.sh"), name,
                               "cargo", "build", "--locked"], cwd=REPO, env=self.env,
                              capture_output=True, text=True, timeout=30)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_arc_preserves_available_wrapper_and_redis(self):
        values = self.setup_cache(RUNNER_ENVIRONMENT="self-hosted", SCCACHE_REDIS_ENDPOINT="redis://cache:6379",
                                  RUSTC_WRAPPER=str(self.tools / "sccache"))
        self.assertEqual(values["EEZ_COMPILER_CACHE"], "redis")
        self.assertEqual(values["EEZ_SCCACHE_BIN"], str(self.tools / "sccache"))
        self.assertNotIn("SCCACHE_REDIS_KEY_PREFIX", values)

    def test_hosted_docker_uses_gha_without_arc_services(self):
        values = self.setup_cache(RUNNER_ENVIRONMENT="github-hosted", EEZ_COMPILER_CACHE_MODE="docker",
                                  ACTIONS_RESULTS_URL="https://cache.invalid", ACTIONS_RUNTIME_TOKEN="private-token",
                                  SCCACHE_REDIS_ENDPOINT="redis://irrelevant:6379")
        self.assertEqual(values["EEZ_COMPILER_CACHE"], "gha")
        self.assertNotIn("private-token", (self.root / "github-env").read_text())

    def test_no_credentials_or_opt_out_builds_plain(self):
        for values in ({"RUNNER_ENVIRONMENT": "github-hosted"},
                       {"RUNNER_ENVIRONMENT": "self-hosted", "EEZ_COMPILER_CACHE_ENABLED": "false"}):
            result = self.setup_cache(**values)
            self.assertEqual(result["EEZ_COMPILER_CACHE"], "none")

    def test_hosted_backend_explicitly_selects_v2_for_native_and_docker(self):
        self.env.update(EEZ_COMPILER_CACHE="gha", ACTIONS_RESULTS_URL="https://cache.invalid/",
                        ACTIONS_RUNTIME_TOKEN="private-token")
        for supplied_flag in (None, "", "true"):
            with self.subTest(supplied_flag=supplied_flag):
                if supplied_flag is None:
                    self.env.pop("ACTIONS_CACHE_SERVICE_V2", None)
                else:
                    self.env["ACTIONS_CACHE_SERVICE_V2"] = supplied_flag
                offset = len(self.calls()) if self.log.exists() else 0
                result = self.measured_build()
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(all(call["gha_v2"] == "true" for call in self.calls()[offset:]))
                self.assertNotIn("private-token", result.stdout + result.stderr)

    def test_missing_arc_wrapper_does_not_break_native_jobs(self):
        values = self.setup_cache(RUNNER_ENVIRONMENT="self-hosted", RUSTC_WRAPPER="/missing/sccache",
                                  SCCACHE_REDIS_ENDPOINT="redis://cache:6379")
        self.assertEqual(values["EEZ_COMPILER_CACHE"], "none")

    def test_build_failure_is_not_retried_or_masked_and_metrics_are_redacted(self):
        self.env.update(EEZ_COMPILER_CACHE="redis", SCCACHE_REDIS_ENDPOINT="redis://cache:6379",
                        EEZ_SCCACHE_BIN=str(self.tools / "sccache"), TEST_CARGO_EXIT="101")
        result = self.measured_build()
        self.assertEqual(result.returncode, 101)
        self.assertEqual(len([c for c in self.calls() if c["tool"] == "cargo"]), 1)
        self.assertIn(["--stop-server"], [c["args"] for c in self.calls()])
        counters = (self.root / "metrics/fixture.sccache.json").read_text()
        self.assertIn('"compile_requests": 12', counters)
        self.assertNotIn("password", counters + result.stdout + result.stderr)
        self.assertEqual(json.loads((self.root / "metrics/fixture.json").read_text())["exit_code"], 101)

    def test_cache_outage_falls_back_before_cargo_runs(self):
        self.env.update(EEZ_COMPILER_CACHE="redis", SCCACHE_REDIS_ENDPOINT="redis://cache:6379", TEST_CACHE_DOWN="1")
        result = self.measured_build()
        self.assertEqual(result.returncode, 0, result.stderr)
        builds = [c for c in self.calls() if c["tool"] == "cargo"]
        self.assertEqual(len(builds), 1)
        self.assertEqual(builds[0]["wrapper"], "")
        self.assertNotIn("password", result.stdout + result.stderr)
        self.assertFalse(json.loads((self.root / "metrics/fixture.json").read_text())["cache_active"])

    def test_empty_buildkit_expiration_secrets_do_not_disable_any_backend(self):
        for backend in ("redis", "gha", "inherited"):
            with self.subTest(backend=backend):
                self.env.update(EEZ_COMPILER_CACHE=backend,
                                SCCACHE_REDIS_TTL="", SCCACHE_REDIS_EXPIRATION="")
                if backend == "redis":
                    self.env["SCCACHE_REDIS_ENDPOINT"] = "redis://cache:6379"
                else:
                    self.env.pop("SCCACHE_REDIS_ENDPOINT", None)
                if backend == "gha":
                    self.env.update(ACTIONS_RESULTS_URL="https://cache.invalid",
                                    ACTIONS_RUNTIME_TOKEN="private-token")
                result = self.measured_build(backend)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(json.loads((self.root / f"metrics/{backend}.json").read_text())["cache_active"],
                                result.stdout + result.stderr)
                self.assertNotIn("Compiler cache unavailable", result.stdout)
        calls = self.calls()
        self.assertEqual(len([c for c in calls if c["tool"] == "cargo"]), 3)
        self.assertTrue(all(c["wrapper"] == "sccache" for c in calls if c["tool"] == "cargo"))
        for call in calls:
            self.assertNotIn("SCCACHE_REDIS_TTL", call["redis_options"])
            self.assertNotIn("SCCACHE_REDIS_EXPIRATION", call["redis_options"])

    def test_explicit_expiration_and_other_redis_options_are_preserved(self):
        other_options = dict(SCCACHE_REDIS_ENDPOINT="redis://cache:6379", SCCACHE_REDIS_DB="0",
                             SCCACHE_REDIS_KEY_PREFIX="unchanged/", SCCACHE_REDIS_USERNAME="ci-user",
                             SCCACHE_REDIS_PASSWORD="fixture-password", SCCACHE_REDIS_RW_MODE="READ_ONLY")
        self.env.update(EEZ_COMPILER_CACHE="redis", **other_options)
        for key, empty_key in (("SCCACHE_REDIS_TTL", "SCCACHE_REDIS_EXPIRATION"),
                               ("SCCACHE_REDIS_EXPIRATION", "SCCACHE_REDIS_TTL")):
            for value in ("0", "600"):
                with self.subTest(key=key, value=value):
                    self.env.update({key: value, empty_key: ""})
                    offset = len(self.calls()) if self.log.exists() else 0
                    result = self.measured_build()
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertTrue(json.loads((self.root / "metrics/fixture.json").read_text())["cache_active"],
                                    result.stdout + result.stderr)
                    for call in self.calls()[offset:]:
                        self.assertEqual(call["redis_options"], {**other_options, key: value})

    def test_two_nonempty_expiration_settings_remain_a_configuration_error(self):
        self.env.update(EEZ_COMPILER_CACHE="redis", SCCACHE_REDIS_ENDPOINT="redis://cache:6379",
                        SCCACHE_REDIS_TTL="0", SCCACHE_REDIS_EXPIRATION="600")
        result = self.measured_build()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(json.loads((self.root / "metrics/fixture.json").read_text())["cache_active"])
        self.assertIn("Compiler cache unavailable", result.stdout)
        builds = [c for c in self.calls() if c["tool"] == "cargo"]
        self.assertEqual(len(builds), 1)
        self.assertEqual(builds[0]["wrapper"], "")
        for call in self.calls():
            self.assertEqual(call["redis_options"]["SCCACHE_REDIS_TTL"], "0")
            self.assertEqual(call["redis_options"]["SCCACHE_REDIS_EXPIRATION"], "600")

    def test_missing_remote_credentials_never_silently_creates_a_disk_cache(self):
        for backend in ("redis", "gha"):
            self.env["EEZ_COMPILER_CACHE"] = backend
            self.assertEqual(self.measured_build(backend).returncode, 0)
        self.assertTrue(all(c["tool"] == "cargo" for c in self.calls()))

    def test_each_operation_owns_a_distinct_server(self):
        self.env["EEZ_COMPILER_CACHE"] = "inherited"
        for name in ("first", "second"):
            self.assertEqual(self.measured_build(name).returncode, 0)
        starts = [c["socket"] for c in self.calls() if c["args"] == ["--start-server"]]
        self.assertEqual(len(set(starts)), 2)
        self.assertFalse(any(c["args"] == ["--zero-stats"] for c in self.calls()))

    def test_unavailable_hosted_install_is_optional(self):
        # Use a fixture copy to fail the installer without a real download.
        (self.root / "scripts/ci").mkdir(parents=True)
        shutil.copy2(REPO / "scripts/ci/setup-compiler-cache.sh", self.root / "scripts/ci")
        (self.root / "scripts/ci/install-sccache.sh").write_text("exit 1\n")
        self.env.update(RUNNER_ENVIRONMENT="github-hosted", ACTIONS_RESULTS_URL="https://cache.invalid",
                        ACTIONS_RUNTIME_TOKEN="token")
        result = subprocess.run(["bash", "scripts/ci/setup-compiler-cache.sh"], cwd=self.root,
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("EEZ_COMPILER_CACHE=none", (self.root / "github-env").read_text())


if __name__ == "__main__":
    unittest.main()
