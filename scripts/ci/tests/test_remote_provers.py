"""Render each packaged Composer without a local signer or operator secrets."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


@unittest.skipUnless(shutil.which("docker"), "Docker Compose CLI is required")
class RemoteProversTest(unittest.TestCase):
    def test_remote_only_stacks(self):
        for network in ("chiado", "ethereum", "kurtosis"):
            with self.subTest(network=network), tempfile.TemporaryDirectory() as work:
                base = f"docker-compose.{network}-node.yml"
                overlay = "docker-compose.remote-provers.yml"
                for name in (base, overlay):
                    shutil.copy(REPO / name, work)
                example = (REPO / f".env.{network}.example").read_text()
                env_file = Path(work, ".env")
                env_file.write_text("\n".join(line for line in example.splitlines()
                    if not line.startswith(("EEZ_PROOF_SIGNER_KEY=", "EEZ_PROOF_SIGNER_IMAGE="))))
                Path(work, "deployments.env").touch()
                shutil.copy(env_file, Path(work, ".env.kurtosis"))
                roster = ",".join(f"https://p{i}.example:50061=0x{str(i)*40}=0x{str(i+3)*40}"
                                  for i in range(1, 4))
                env = {k: os.environ[k] for k in ("PATH", "HOME") if k in os.environ}
                env.update(EEZ_PROVERS=roster, EEZ_PROVER_TLS_DIR="/fixture/tls",
                           EEZ_L2_FEE_RECIPIENT="0x" + "11"*20)
                cmd = ["docker", "compose", "-f", base, "-f", overlay,
                       "config", "--no-env-resolution", "--format", "json"]
                result = subprocess.run(cmd, cwd=work, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                self.assertNotIn("eez-proof-signer", services)
                node = services["eez-node"]
                self.assertNotIn("eez-proof-signer", node.get("depends_on", {}))
                self.assertEqual(node["environment"]["EEZ_PROVERS"], roster)
                self.assertEqual(node["environment"].get("EEZ_PROOF_SIGNER_KEY"), "")
                mount = next(v for v in node["volumes"]
                             if v["target"] == node["environment"]["EEZ_PROVER_TLS_DIR"])
                self.assertEqual(mount["source"], "/fixture/tls")
                self.assertTrue(mount["read_only"])
                self.assertFalse(mount["bind"].get("create_host_path", False))
                for missing in ("EEZ_PROVERS", "EEZ_PROVER_TLS_DIR"):
                    bad = subprocess.run(cmd, cwd=work, env={**env, missing: ""},
                                         capture_output=True, text=True)
                    self.assertNotEqual(bad.returncode, 0)
                    self.assertIn(missing, bad.stderr)
