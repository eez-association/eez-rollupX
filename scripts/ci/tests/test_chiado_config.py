"""Static Compose validation: no daemon, deployment, or credentials required."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


@unittest.skipUnless(shutil.which("docker") and shutil.which("jq"),
                     "Docker Compose CLI and jq are required (no daemon needed)")
class ChiadoConfigTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-chiado-config-test.")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        for name in (
            "docker-compose.chiado-node.yml", "docker-compose.chiado.yml",
            ".env.chiado.example",
            "scripts/check-chiado-blob-config.sh", "scripts/chiado-up.sh",
            "configs/chiado/bootnodes.yaml", "configs/chiado/bootstrap_nodes.yaml",
        ):
            target = self.repo / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO / name, target, follow_symlinks=False)
        self.scratch = self.root / "scratch"
        self.scratch.mkdir()

    def check(self, success=True):
        result = subprocess.run(
            ["bash", str(self.repo / "scripts/check-chiado-blob-config.sh")],
            cwd=self.root, env={**os.environ, "TMPDIR": str(self.scratch)},
            text=True, capture_output=True,
        )
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        self.assertEqual(list(self.scratch.iterdir()), [], "Fixture must be cleaned up")
        return result

    def test_clean_checkout_without_deployments(self):
        self.assertIn("PASS", self.check().stdout)
        self.assertFalse((self.repo / "deployments.env").exists())

    def test_existing_deployment_file_is_not_read_or_modified(self):
        deployment = self.repo / "deployments.env"
        contents = "MUST_NOT_RESOLVE=${NONEXISTENT:?must not read deployment bindings}\n"
        deployment.write_text(contents)
        self.check()
        self.assertEqual(deployment.read_text(), contents)

    def test_wrong_chain_id_still_fails(self):
        compose = self.repo / "docker-compose.chiado-node.yml"
        compose.write_text(compose.read_text().replace('EEZ_L1_CHAIN_ID: "10200"',
                                                     'EEZ_L1_CHAIN_ID: "1"'))
        self.check(success=False)

    def test_legacy_bootnode_filename_alone_is_not_sufficient(self):
        (self.repo / "configs/chiado/bootstrap_nodes.yaml").unlink()
        result = self.check(success=False)
        self.assertIn("bootstrap_nodes.yaml is missing or empty", result.stderr)

    def test_empty_bootnodes_fail(self):
        (self.repo / "configs/chiado/bootnodes.yaml").write_text("")
        self.check(success=False)

    def test_bootstrap_seeds_match_the_canonical_list(self):
        bootstrap = self.repo / "configs/chiado/bootstrap_nodes.yaml"
        bootstrap.unlink()
        bootstrap.write_text("[]\n")
        self.check(success=False)

    def test_optional_enr_arguments_reach_both_beacons(self):
        # Exercise Compose's actual argv rendering. Empty optional arguments
        # must disappear, while explicit public endpoints must reach Lighthouse.
        environment = {k: v for k, v in os.environ.items()
                       if k not in ("LH_ENR_ADDRESS", "LH_ENR_UDP_PORT")}
        environment.update({
            "EEZ_CHIADO_LIGHTHOUSE_DATA": str(self.root / "beacon"),
            "EEZ_L1_JWT_SECRET": str(self.root / "jwt.hex"),
            "EEZ_CHIADO_CONFIG_DIR": str(self.repo / "configs/chiado"),
        })
        (self.root / "deployments.env").touch()
        for profile in ("docker-compose.chiado-node.yml", "docker-compose.chiado.yml"):
            baseline = None
            for overrides in ({}, {"LH_ENR_ADDRESS": "", "LH_ENR_UDP_PORT": ""},
                              {"LH_ENR_ADDRESS": "198.51.100.10", "LH_ENR_UDP_PORT": "19005"},
                              {"LH_ENR_ADDRESS": "beacon.example.org", "LH_ENR_UDP_PORT": "9405"}):
                with self.subTest(profile=profile, overrides=overrides):
                    result = subprocess.run(
                        ["docker", "compose", "--project-directory", str(self.root),
                         "--env-file", str(self.repo / ".env.chiado.example"),
                         "-f", str(self.repo / profile), "config",
                         "--no-env-resolution", "--format", "json"],
                        env={**environment, **overrides}, text=True, capture_output=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    command = json.loads(result.stdout)["services"]["lighthouse"]["command"]
                    self.assertNotIn("", command)
                    if baseline is None:
                        baseline = command
                        self.assertFalse(any(arg.startswith("--enr-") for arg in command))
                    if overrides.get("LH_ENR_ADDRESS"):
                        self.assertIn("--enr-address=" + overrides["LH_ENR_ADDRESS"], command)
                        self.assertIn("--enr-udp-port=" + overrides["LH_ENR_UDP_PORT"], command)
                        self.assertEqual([arg for arg in command if not arg.startswith("--enr-")],
                                         baseline)
                    else:
                        self.assertEqual(command, baseline)

    def test_optional_preparation_reaches_node_without_changing_legacy_defaults(self):
        environment = {k: v for k, v in os.environ.items() if k != "EEZ_PREPARATION_TIME_MS"}
        (self.root / "deployments.env").touch()
        for value in (None, "500", ""):
            with self.subTest(value=value):
                overrides = {} if value is None else {"EEZ_PREPARATION_TIME_MS": value}
                result = subprocess.run(
                    ["docker", "compose", "--project-directory", str(self.root),
                     "--env-file", str(self.repo / ".env.chiado.example"),
                     "-f", str(self.repo / "docker-compose.chiado-node.yml"), "config",
                     "--no-env-resolution", "--format", "json"],
                    env={**environment, **overrides}, text=True, capture_output=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                actual = json.loads(result.stdout)["services"]["eez-node"]["environment"]
                self.assertEqual(actual.get("EEZ_PREPARATION_TIME_MS"), value)
                # Explicit empty reaches Rust's startup validation; Compose must
                # not silently replace it with a default or an absent setting.
                self.assertEqual(actual["EEZ_PROOF_TIME_MS"], environment.get("EEZ_PROOF_TIME_MS", "1500"))
                self.assertEqual(actual["EEZ_SUBMISSION_SLACK_MS"], environment.get("EEZ_SUBMISSION_SLACK_MS", "700"))


if __name__ == "__main__":
    unittest.main()
