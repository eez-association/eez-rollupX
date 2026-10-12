"""Keep fixture exceptions narrow enough to detect a new credential beside them."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


@unittest.skipUnless(shutil.which("gitleaks"), "requires Gitleaks (installed by CI)")
class SecretScanTest(unittest.TestCase):
    def test_public_fixture_is_allowed_but_new_keys_and_inline_suppressions_fail(self):
        public_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        with tempfile.TemporaryDirectory(prefix="eez-secret-scan.") as directory:
            root = Path(directory)
            source = root / "source"
            fixture = source / "testing/kurtosis/ci-args.yaml"
            fixture.parent.mkdir(parents=True)
            report = root / "report.json"
            command = ["gitleaks", "dir", str(source), "--config", str(REPO / ".gitleaks.toml"),
                       "--redact=100", "--ignore-gitleaks-allow", "--gitleaks-ignore-path", "/dev/null",
                       "--no-banner", "--report-format", "json", "--report-path", str(report)]
            fixture.write_text(f'private_key: "0x{public_key}"\n')
            result = subprocess.run(command, text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            # A one-character change must not match the exact public-key exception.
            fixture.write_text(f'private_key: "0x{public_key[:-1]}1" # gitleaks:allow\n')
            result = subprocess.run(command, text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 1, result.stderr)
            findings = json.loads(report.read_text())
            self.assertEqual(len(findings), 1)
            self.assertEqual(findings[0]["RuleID"], "generic-api-key")
            self.assertTrue(findings[0]["File"].endswith("testing/kurtosis/ci-args.yaml"))


if __name__ == "__main__":
    unittest.main()
