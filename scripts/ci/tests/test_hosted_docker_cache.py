"""Hosted Docker setup must never change ARC or switch daemons during a build."""
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
SCRIPT = REPO / "scripts/ci/hosted-docker-cache.sh"


class HostedDockerCacheTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="eez-hosted-docker-test.")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.log = self.root / "commands"
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("EEZ_", "DOCKER_", "RUNNER_"))}
        self.env.update(PATH=str(self.root) + os.pathsep + os.environ["PATH"],
                        GITHUB_OUTPUT=str(self.root / "output"), GITHUB_ENV=str(self.root / "env"),
                        RUNNER_ENVIRONMENT="github-hosted", RUNNER_OS="Linux",
                        EEZ_PUBLISH_IMAGES="true", TEST_DOCKER_CALLS=str(self.log))
        mock = self.root / "docker"
        mock.write_text('''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
host = os.environ.get("DOCKER_HOST", "unix:///var/run/docker.sock")
with open(os.environ["TEST_DOCKER_CALLS"], "a") as stream:
    stream.write(json.dumps({"args": args, "host": host, "context": os.environ.get("DOCKER_CONTEXT")}) + "\\n")
if args[:2] == ["context", "inspect"]:
    print("unix:///var/run/docker.sock")
elif args[:1] == ["version"]:
    print("29.7.2")
elif args[:1] == ["info"]:
    if os.environ.get("TEST_ORIGINAL_DOWN") and host.endswith("/var/run/docker.sock"):
        sys.exit(1)
    store = os.environ.get("TEST_ORIGINAL_CONTAINERD") or (host.endswith("/cache.sock") and not os.environ.get("TEST_CACHE_DOWN"))
    print(json.dumps([["driver-type", "io.containerd.snapshotter.v1"]] if store else []))
else:
    raise SystemExit("unexpected Docker operation")
''')
        mock.chmod(0o755)

    def run_script(self, mode):
        return subprocess.run(["bash", str(SCRIPT), mode], env=self.env, text=True,
                              capture_output=True, timeout=10)

    def values(self, path):
        return dict(line.split("=", 1) for line in (self.root / path).read_text().splitlines())

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_self_hosted_unknown_and_non_linux_are_noops(self):
        for environment, system in (("self-hosted", "Linux"), ("", "Linux"), ("github-hosted", "Windows")):
            self.env.update(RUNNER_ENVIRONMENT=environment, RUNNER_OS=system)
            for mode in ("prepare", "select"):
                self.assertEqual(self.run_script(mode).returncode, 0)
        self.assertEqual(self.calls(), [])
        self.assertFalse((self.root / "env").exists())

    def test_classic_store_prepares_same_version_and_preserves_original_endpoint(self):
        result = self.run_script("prepare")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.values("output"), {"configure": "true", "version": "v29.7.2",
                                                "original-host": "unix:///var/run/docker.sock"})

    def test_no_publication_opt_out_and_existing_containerd_skip_setup(self):
        for option in ({"EEZ_PUBLISH_IMAGES": "false"}, {"EEZ_RUNTIME_CACHE": "false"},
                       {"EEZ_HOSTED_CONTAINERD": "false"}, {"TEST_ORIGINAL_CONTAINERD": "1"}):
            with self.subTest(option=option):
                previous = self.env.copy()
                self.env.update(option)
                self.assertEqual(self.run_script("prepare").returncode, 0)
                self.assertEqual(self.values("output"), {"configure": "false"})
                self.env = previous

    def test_remote_docker_is_not_replaced(self):
        self.env["DOCKER_HOST"] = "tcp://remote.example:2376"
        self.assertEqual(self.run_script("prepare").returncode, 0)
        self.assertEqual(self.values("output"), {"configure": "false"})

    def test_private_socket_is_selected_and_context_override_is_cleared(self):
        self.env.update(EEZ_DOCKER_SETUP_OUTCOME="success", EEZ_CACHE_DOCKER_HOST="unix:///tmp/cache.sock",
                        EEZ_ORIGINAL_DOCKER_HOST="unix:///var/run/docker.sock", DOCKER_CONTEXT="setup-docker-action")
        result = self.run_script("select")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.values("env"), {"DOCKER_HOST": "unix:///tmp/cache.sock", "DOCKER_CONTEXT": ""})
        self.assertEqual(self.calls()[0]["context"], "")

    def test_failed_setup_or_unusable_store_falls_back_before_compilation(self):
        for outcome, down in (("failure", ""), ("success", "1")):
            self.env.update(EEZ_DOCKER_SETUP_OUTCOME=outcome, TEST_CACHE_DOWN=down,
                            EEZ_CACHE_DOCKER_HOST="unix:///tmp/cache.sock",
                            EEZ_ORIGINAL_DOCKER_HOST="unix:///var/run/docker.sock")
            result = self.run_script("select")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("keeping the original", result.stdout)
            self.assertEqual(self.values("env")["DOCKER_HOST"], "unix:///var/run/docker.sock")

    def test_no_working_daemon_fails_closed(self):
        self.env.update(EEZ_DOCKER_SETUP_OUTCOME="failure", TEST_ORIGINAL_DOWN="1",
                        EEZ_ORIGINAL_DOCKER_HOST="unix:///var/run/docker.sock")
        self.assertNotEqual(self.run_script("select").returncode, 0)
        self.assertFalse((self.root / "env").exists())

    @unittest.skipIf(yaml is None, "PyYAML not installed")
    def test_action_is_hosted_only_and_daemon_is_fixed_before_build_or_login(self):
        steps = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())["jobs"]["build"]["steps"]
        action = next(step for step in steps if step.get("id") == "cache-docker")
        self.assertIn("runner.environment == 'github-hosted'", action["if"])
        self.assertIn("outputs.configure == 'true'", action["if"])
        self.assertEqual(action["timeout-minutes"], 3)
        self.assertTrue(action["continue-on-error"])
        self.assertTrue(action["with"]["set-host"])
        self.assertEqual(json.loads(action["with"]["daemon-config"]), {"features": {"containerd-snapshotter": True}})
        select = next(i for i, s in enumerate(steps) if s.get("run") == "bash scripts/ci/hosted-docker-cache.sh select")
        login = next(i for i, s in enumerate(steps) if s.get("id") == "ghcr-login")
        build = next(i for i, s in enumerate(steps) if s.get("run") == "bash scripts/ci/build-runtime-artifacts.sh")
        self.assertLess(steps.index(action), select)
        self.assertLess(select, login)
        self.assertLess(login, build)
        self.assertFalse(steps[select].get("continue-on-error", False))


if __name__ == "__main__":
    unittest.main()
