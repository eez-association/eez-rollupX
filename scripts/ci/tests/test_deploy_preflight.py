"""Deployment must reject unusable genesis inputs before broadcasting contracts."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


@unittest.skipUnless(shutil.which("jq"), "jq is required")
class DeployPreflightTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-deploy-preflight-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        (self.root / "contracts").mkdir()
        (self.root / "bin").mkdir()
        shutil.copy2(REPO / "scripts/deploy.sh", self.root / "scripts/deploy.sh")
        (self.root / ".env").write_text(
            "EEZ_L1_RPC_URL=http://example.invalid\nEEZ_L1_POSTER_KEY=poster\n"
            "EEZ_PROOF_SIGNER_KEY=attester\nEEZ_L2_SYSTEM_KEY=system\n"
        )
        self.tool("bin/cast", '''
[[ "$1 $2" == "wallet address" ]] || exit 91
case "${@: -1}" in
    attester) echo 0x1111111111111111111111111111111111111111 ;;
    system) echo 0x2222222222222222222222222222222222222222 ;;
    poster) echo 0x3333333333333333333333333333333333333333 ;;
    *) exit 92 ;;
esac
''')
        self.tool("scripts/update-eezl2-genesis.sh", '''
[[ "$FAIL_STAGE" != render ]] || { echo 'render failed' >&2; exit 60; }
while (( $# )); do
    case "$1" in
        --output) genesis="$2"; shift ;;
        --profile-output) profile="$2"; shift ;;
    esac
    shift
done
mkdir -p "$(dirname "$genesis")"
echo '{}' > "$genesis"
echo '{"genesisStateRoot":"0x1234","runtimeCodeHash":"0x5678"}' > "$profile"
''')
        self.tool("bin/eez-genesis-block-hash", '''
[[ -f "$1" ]] || exit 93
[[ "$FAIL_STAGE" != hash ]] || { echo 'hash failed' >&2; exit 61; }
echo 0x1234
''')
        self.tool("bin/forge", '''
printf '%s\n' "$*" >> "$BROADCAST_LOG"
exit 97
''')

    def tool(self, name, body):
        path = self.root / name
        path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body)
        path.chmod(0o755)

    def run_deploy(self, failure):
        environment = {k: v for k, v in os.environ.items() if not k.startswith("EEZ_")}
        return subprocess.run(
            ["bash", str(self.root / "scripts/deploy.sh")],
            env={**environment, "PATH": str(self.root / "bin") + os.pathsep + os.environ["PATH"],
                 "FAIL_STAGE": failure, "BROADCAST_LOG": str(self.root / "broadcast.log")},
            capture_output=True, text=True,
        )

    def test_failed_render_or_hash_prevents_broadcast(self):
        for stage in ("render", "hash"):
            with self.subTest(stage=stage):
                result = self.run_deploy(stage)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(stage + " failed", result.stderr)
                self.assertFalse((self.root / "broadcast.log").exists())
                self.assertFalse((self.root / "deployments.env").exists())

    def test_successful_preflight_reaches_first_broadcast(self):
        result = self.run_deploy("")
        self.assertNotEqual(result.returncode, 0)  # Forge stub stops before any transaction.
        command = (self.root / "broadcast.log").read_text()
        self.assertIn("DeployEEZ", command)
        self.assertIn("--broadcast", command)
        self.assertIn("DeployEEZ failed", result.stderr)


if __name__ == "__main__":
    unittest.main()
