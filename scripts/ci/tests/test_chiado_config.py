"""Static Compose validation: no daemon, deployment, or credentials required."""
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
            "docker-compose.chiado-node.yml", ".env.chiado.example",
            "scripts/check-chiado-blob-config.sh", "scripts/chiado-up.sh",
        ):
            target = self.repo / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO / name, target)
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


if __name__ == "__main__":
    unittest.main()
