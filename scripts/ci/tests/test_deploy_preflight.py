"""Deployment must reject unusable genesis inputs before broadcasting contracts."""
import os
import json
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
            "EEZ_PROOF_SIGNER_KEY=attester\n"
            "EEZ_L1_BLOCK_TIME_MS=5000\nEEZ_L2_BLOCK_TIME_MS=1000\n"
        )
        self.tool("bin/cast", '''
if [[ "$1" == chain-id ]]; then echo "${CHAIN_ID:-10200}"; exit; fi
if [[ "$1 $2" == "block finalized" ]]; then
    if [[ "$FAIL_STAGE" == reference ]]; then echo null; exit; fi
    echo '{"number":"0x2a","timestamp":"0x3e8","hash":"0x1111111111111111111111111111111111111111111111111111111111111111","mixHash":"0x2222222222222222222222222222222222222222222222222222222222222222"}'
    exit
fi
if [[ "$1 $2" == "block 0" ]]; then
    echo '{"number":"0x0","timestamp":"0x64","hash":"0x4444444444444444444444444444444444444444444444444444444444444444","mixHash":"0x5555555555555555555555555555555555555555555555555555555555555555"}'
    exit
fi
[[ "$1 $2" == "wallet address" ]] || exit 91
case "${@: -1}" in
    attester) echo 0x1111111111111111111111111111111111111111 ;;
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
echo '{"config":{}}' > "$genesis"
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

    def run_deploy(self, failure, **extra):
        environment = {k: v for k, v in os.environ.items() if not k.startswith("EEZ_")}
        return subprocess.run(
            ["bash", str(self.root / "scripts/deploy.sh")],
            env={**environment, "PATH": str(self.root / "bin") + os.pathsep + os.environ["PATH"],
                 "FAIL_STAGE": failure, "BROADCAST_LOG": str(self.root / "broadcast.log"),
                 **extra},
            capture_output=True, text=True,
        )

    def test_private_devnet_can_pin_l1_genesis_without_waiting_for_finality(self):
        result = self.run_deploy("", CHAIN_ID="31337", EEZ_L1_REFERENCE="genesis")
        self.assertIn("DeployEEZ failed", result.stderr)
        genesis = json.loads((self.root / "datadir/genesis.json").read_text())
        self.assertEqual(genesis["timestamp"], "0x64")
        self.assertEqual(genesis["mixHash"], "0x" + "55" * 32)
        self.assertEqual(genesis["config"]["rollupX"]["l1Reference"], {
            "chainId": 31337, "number": 0, "timestamp": 100,
            "hash": "0x" + "44" * 32, "prevRandao": "0x" + "55" * 32})

    def test_genesis_reference_is_rejected_on_public_or_ambiguous_selection(self):
        for chain, value, message in (
            ("10200", "genesis", "only for private devnets"),
            ("1", "genesis", "only for private devnets"),
            ("31337", "", "must be 'finalized' or 'genesis'"),
            ("31337", "latest", "must be 'finalized' or 'genesis'"),
        ):
            with self.subTest(chain=chain, value=value):
                result = self.run_deploy("", CHAIN_ID=chain, EEZ_L1_REFERENCE=value)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertFalse((self.root / "broadcast.log").exists())

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
        genesis = json.loads((self.root / "datadir/genesis.json").read_text())
        self.assertEqual(genesis["timestamp"], "0x3e8")
        self.assertEqual(genesis["mixHash"], "0x" + "22" * 32)
        self.assertEqual(genesis["config"]["rollupX"], {
            "blockIntervalSeconds": 1, "syncIntervalBlocks": 5,
            "l1Reference": {"chainId": 10200, "number": 42, "timestamp": 1000,
                            "hash": "0x" + "11" * 32, "prevRandao": "0x" + "22" * 32}})

    def test_unavailable_finalized_reference_prevents_broadcast(self):
        result = self.run_deploy("reference")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("finalized reference is unavailable", result.stderr)
        self.assertFalse((self.root / "broadcast.log").exists())

    def test_cadence_cannot_override_the_chiado_genesis_profile(self):
        with (self.root / ".env").open("a") as config:
            config.write("EEZ_L1_BLOCK_TIME_MS=10000\n")
        result = self.run_deploy("")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cadence conflicts", result.stderr)
        self.assertFalse((self.root / "broadcast.log").exists())

    def test_absent_public_cadence_uses_fixed_profile_but_explicit_empty_is_rejected(self):
        path = self.root / ".env"
        original = path.read_text()
        path.write_text("\n".join(line for line in original.splitlines()
                                 if "BLOCK_TIME_MS" not in line) + "\n")
        result = self.run_deploy("")
        self.assertIn("DeployEEZ failed", result.stderr)
        (self.root / "broadcast.log").unlink()
        with path.open("a") as config:
            config.write("EEZ_L1_BLOCK_TIME_MS=''\n")
        result = self.run_deploy("")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "broadcast.log").exists())


if __name__ == "__main__":
    unittest.main()
