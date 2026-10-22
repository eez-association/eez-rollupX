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
    def test_reviewed_values_are_scoped_to_exact_values_and_paths(self):
        audit_path = "docs/audits/2026-10-15-simplified-fees/result.json"
        digest = json.loads((REPO / audit_path).read_text())["evidence"]["sha256"]["public-api-check.log"]
        cases = [
            ("scripts/ci/tests/test_chiado_config.py", "key = " + json.dumps("EEZ_L1_FEE_REFUND_POLICY") + "\n"),
            (audit_path, json.dumps({"public-api-check.log": digest}) + "\n"),
        ]
        builder_keys = [
            ("BUILDERNET_KEY",
             "0x850b00e055388c1b56f137e0852bc02d8b05534e9f96dd1d11cff8cf0481b800ecc2e708cdc158cd95b7cfdaa6840ee7"),
            ("BUILDERNET_RETIRED_KEY",
             "0x851b00b12f73770337fbd219b439aba7bd8e770dbdea16a3b0df030d971bc0d824d470d7868e5d93355f501bfe892947"),
        ]
        cases.extend(("scripts/ci/tests/test_why_excluded.py", f'{name} = "{value}"\n')
                     for name, value in builder_keys)
        for relative, allowed in cases:
            with self.subTest(path=relative), tempfile.TemporaryDirectory(prefix="eez-secret-scan.") as directory:
                root = Path(directory)
                source = root / "source"
                fixture = source / relative
                fixture.parent.mkdir(parents=True)
                report = root / "report.json"
                subprocess.run(["git", "init", "-q", str(source)], check=True, capture_output=True)
                command = ["gitleaks", "git", ".", "--log-opts=-1 -m", "--config", str(REPO / ".gitleaks.toml"),
                           "--redact=100", "--ignore-gitleaks-allow", "--gitleaks-ignore-path", "/dev/null",
                           "--no-banner", "--report-format", "json", "--report-path", str(report)]

                def scan_commit():
                    subprocess.run(["git", "add", "."], cwd=source, check=True, capture_output=True)
                    subprocess.run(["git", "-c", "user.name=Scanner fixture", "-c", "user.email=fixture@example.invalid",
                                    "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                                    "commit", "-qm", "Scanner fixture"],
                                   cwd=source, check=True, capture_output=True)
                    return subprocess.run(command, cwd=source, text=True, capture_output=True, timeout=30)

                fixture.write_text(allowed)
                result = scan_commit()
                self.assertEqual(result.returncode, 0, result.stderr)

                # The exception cannot hide a different credential in the same file.
                synthetic = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff81"
                fixture.write_text(allowed + f'private_key: "0x{synthetic}" # gitleaks:allow\n')
                result = scan_commit()
                self.assertEqual(result.returncode, 1, result.stderr)
                findings = json.loads(report.read_text())
                self.assertEqual(len(findings), 1)
                self.assertEqual(findings[0]["RuleID"], "generic-api-key")

                # Even the reviewed value remains detectable outside its evidence path.
                fixture.write_text(allowed)
                (source / "unreviewed.txt").write_text(allowed)
                result = scan_commit()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(len(json.loads(report.read_text())), 1)

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
