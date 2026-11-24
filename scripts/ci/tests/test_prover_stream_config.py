"""Render the Composer environment on each supported Compose network."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


@unittest.skipUnless(shutil.which("docker"), "Docker Compose CLI required")
class ProverStreamConfigTest(unittest.TestCase):
    def test_window_passthrough_and_shell_precedence(self):
        key = "EEZ_PROVER_STREAM_MAX_IN_FLIGHT"
        env = {k: v for k, v in os.environ.items() if not k.startswith(("EEZ_", "COMPOSE_"))}
        for network in ("chiado", "ethereum", "kurtosis"):
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                compose = f"docker-compose.{network}-node.yml"
                shutil.copy(REPO / compose, root / compose)
                (root / "deployments.env").touch()
                dotenv = ".env.kurtosis" if network == "kurtosis" else ".env"
                example = (REPO / f".env.{network}.example").read_text().replace(
                    "EEZ_L2_FEE_RECIPIENT=\n", "EEZ_L2_FEE_RECIPIENT=0x" + "00" * 20 + "\n"
                ).replace(":<full-commit>", ":" + "a" * 40)
                for file_value, shell_value in ((None, None), ("8", None), ("8", "32"),
                                                ("8", ""), (None, "invalid")):
                    with self.subTest(network=network, dotenv=file_value, shell=shell_value):
                        (root / dotenv).write_text(example + (
                            f"\n{key}={file_value}\n" if file_value is not None else ""))
                        services = json.loads(subprocess.check_output(
                            ["docker", "compose", "--env-file", dotenv, "-f", compose,
                             "config", "--format", "json"], cwd=root, text=True,
                            env=env if shell_value is None else {**env, key: shell_value},
                        ))["services"]
                        expected = file_value if shell_value is None else shell_value
                        self.assertEqual(services["eez-node"]["environment"].get(key), expected)
                        self.assertNotIn(key, services["eez-proof-signer"]["environment"])
