"""Bound transport retries while keeping Git's exact pinned checkout semantics."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


class SubmoduleCheckoutTest(unittest.TestCase):
    def checkout(self, failures):
        with tempfile.TemporaryDirectory(prefix="eez-submodule-retry.") as temporary:
            root = Path(temporary)
            log = root / "calls.jsonl"
            state = root / "attempt"
            for tool in ("git", "sleep"):
                path = root / tool
                path.write_text(f"#!{sys.executable}\n" + '''
import json, os, pathlib, sys
tool = pathlib.Path(sys.argv[0]).name
with open(os.environ['CALL_LOG'], 'a') as output:
    output.write(json.dumps([tool, *sys.argv[1:]]) + '\\n')
if tool == 'git':
    state = pathlib.Path(os.environ['STATE'])
    attempt = int(state.read_text()) + 1 if state.exists() else 1
    state.write_text(str(attempt))
    sys.exit(128 if attempt <= int(os.environ['FAILURES']) else 0)
''')
                path.chmod(0o755)
            result = subprocess.run(
                ["bash", str(REPO / "scripts/ci/init-submodules.sh")],
                env=dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                         CALL_LOG=str(log), STATE=str(state), FAILURES=str(failures)),
                text=True, capture_output=True, timeout=10,
            )
            return result, [json.loads(line) for line in log.read_text().splitlines()]

    def test_success_is_not_repeated(self):
        result, calls = self.checkout(0)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [["git", "-c", "protocol.version=2", "submodule", "update",
                                 "--init", "--recursive", "--depth=1", "--checkout"]])

    def test_transient_failures_retry_same_checkout_with_bounded_backoff(self):
        result, calls = self.checkout(2)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[0], calls[2])
        self.assertEqual(calls[0], calls[4])
        self.assertEqual(calls[1::2], [["sleep", "5"], ["sleep", "10"]])

    def test_persistent_failure_never_passes_or_retries_indefinitely(self):
        result, calls = self.checkout(99)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(sum(c[0] == "git" for c in calls), 3)
        self.assertIn("failed after three attempts", result.stderr)


if __name__ == "__main__":
    unittest.main()
