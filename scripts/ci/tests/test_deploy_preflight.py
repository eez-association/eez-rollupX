"""Deployment must reject unusable genesis inputs before broadcasting contracts."""
import os
import json
from pathlib import Path
import shutil
import re
import shlex
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
        (self.root / "genesis.json").write_text('{"config":{}}')
        shutil.copy2(REPO / "scripts/deploy.sh", self.root / "scripts/deploy.sh")
        (self.root / ".env").write_text(
            "EEZ_L1_RPC_URL=http://example.invalid\nEEZ_L1_POSTER_KEY=poster\n"
            "EEZ_PROOF_SIGNER_KEY=attester\n"
            "EEZ_L2_TREASURY_RECIPIENT=0x1111111111111111111111111111111111111111\n"
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
        --base) base="$2"; shift ;;
        --treasury-owner) owner="$2"; shift ;;
        --treasury-recipient) recipient="$2"; shift ;;
    esac
    shift
done
mkdir -p "$(dirname "$genesis")"
jq '.config.rollupX.feeCollector = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078"' "$base" > "$genesis"
stat -c '%a' "$base" > "$(dirname "$genesis")/render-base-mode"
chmod --reference="$base" "$genesis"
echo '{"treasuryOwner":"0x3333333333333333333333333333333333333333","treasuryRecipient":"0x1111111111111111111111111111111111111111","feeCollector":"0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078","genesisStateRoot":"0x1234","runtimeCodeHash":"0x5678","eezL2Address":"0x4b2760f84b83d6b3829e30a7573b0172b6d6402c","eezL2ImplementationAddress":"0x4c3d971b1079e58843d0e1af155a2e54b9e54dba","eezL2ProxyAdminAddress":"0x57bf29a07746a25b87c613e01d5da49f6119e96c"}' > "$profile"
jq --arg owner "${owner,,}" --arg recipient "${recipient,,}" '.treasuryOwner = $owner | .treasuryRecipient = $recipient' "$profile" > "$profile.tmp"
mv "$profile.tmp" "$profile"
if [[ "$FAIL_STAGE" == treasury ]]; then
    jq '.treasuryRecipient = "0x2222222222222222222222222222222222222222"' "$profile" > "$profile.tmp"
    mv "$profile.tmp" "$profile"
fi
''')
        self.tool("bin/eez-genesis-block-hash", '''
[[ -f "$1" ]] || exit 93
[[ "$FAIL_STAGE" != hash ]] || { echo 'hash failed' >&2; exit 61; }
[[ "$FAIL_STAGE" != profile || "${2-}" != --network-profile ]] || { echo 'profile failed' >&2; exit 62; }
if [[ "${2-}" == --network-profile ]]; then printf '0x%064x\n' 1; else echo 0x1234; fi
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
        result = self.run_deploy("", CHAIN_ID="31337", EEZ_L1_REFERENCE="genesis",
                                 EEZ_L1_CHAIN_CONFIG="private-genesis.json")
        self.assertIn("DeployEEZ failed", result.stderr)
        genesis = json.loads((self.root / "datadir/genesis.json").read_text())
        self.assertEqual(genesis["timestamp"], "0x69")
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
        for stage in ("render", "hash", "profile"):
            with self.subTest(stage=stage):
                shutil.rmtree(self.root / "datadir", ignore_errors=True)
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
        self.assertEqual(genesis["timestamp"], "0x3ed")
        self.assertEqual(genesis["mixHash"], "0x" + "22" * 32)
        self.assertEqual(genesis["config"]["rollupX"], {
            "blockIntervalSeconds": 1, "syncIntervalBlocks": 5,
            "historyUpdateOffset": 2,
            "feeCollector": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078",
            "l1Reference": {"chainId": 10200, "number": 42, "timestamp": 1000,
                            "hash": "0x" + "11" * 32, "prevRandao": "0x" + "22" * 32}})

    def test_network_key_reaches_manager_and_runtime_bindings(self):
        cast = self.root / "bin/cast"
        cast.write_text(cast.read_text().replace('if [[ "$1" == chain-id ]]', '''
if [[ "$1" == block-number ]]; then echo 42; exit; fi
if [[ "$1" == call ]]; then [[ "$3" == 'rollupCounter()(uint256)' ]]; echo 0; exit; fi
if [[ "$1" == keccak ]]; then
    case "$2" in
        EEZ_ROLLUPX_VERIFICATION_KEY_V1) printf '0x%064x\\n' 2 ;;
        0xabcdef|'') printf '0x%064x\\n' 3 ;;
        *) exit 94 ;;
    esac
    exit
fi
if [[ "$1" == abi-encode ]]; then
    [[ "$FAIL_STAGE" != key ]] || { echo 'key encoding failed' >&2; exit 63; }
    [[ "$2" == 'f(bytes32,bytes32,address,address,address)' ]]
    [[ "$3" == "$(printf '0x%064x' 2)" && "$4" == "$(printf '0x%064x' 1)" ]]
    [[ "$5" == 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ]]
    [[ "$6" == 0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ]]
    [[ "$7" == 0x1111111111111111111111111111111111111111 ]]
    echo 0xabcdef; exit
fi
if [[ "$1" == chain-id ]]'''))
        self.tool("bin/eez-genesis-state-root", "echo 0x1234\n")
        self.tool("scripts/verify-blockscout.sh", "exit 0\n")
        self.tool("bin/forge", '''
printf '%s\\n' "$*" >> "$BROADCAST_LOG"
case "$2" in
    script/DeployEEZ.s.sol:DeployEEZ)
        for key in EEZ EEZ_IMPLEMENTATION EEZ_PROXY_ADMIN; do
            echo "$key=0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        done ;;
    script/DeployECDSAProofSystem.s.sol:DeployECDSAProofSystem)
        echo ECDSA_PS=0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ;;
    script/DeployRollup.s.sol:DeployRollup)
        [[ "$4" == 'run(address,address,bytes32,address,address)' ]]
        [[ "$7" == "$(printf '0x%064x' 3)" ]]
        for key in ROLLUP_CONTRACT ROLLUP_IMPLEMENTATION ROLLUP_PROXY_ADMIN; do
            echo "$key=0xcccccccccccccccccccccccccccccccccccccccc"
        done ;;
    script/RegisterRollup.s.sol:RegisterRollup) echo L2_ROLLUP_ID=1 ;;
    *) exit 95 ;;
esac
''')
        result = self.run_deploy("")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        bindings = (self.root / "deployments.env").read_text()
        self.assertIn(f"EEZ_VKEY=0x{3:064x}\n", bindings)
        self.assertIn("EEZ_ATTESTER_ADDRESS=0x" + "11" * 20 + "\n", bindings)
        shutil.rmtree(self.root / "datadir")
        (self.root / "deployments.env").unlink()
        (self.root / "broadcast.log").unlink()
        result = self.run_deploy("key")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("key encoding failed", result.stderr)
        self.assertNotIn("DeployRollup", (self.root / "broadcast.log").read_text())
        self.assertFalse((self.root / "deployments.env").exists())
        shutil.rmtree(self.root / "datadir")
        (self.root / "broadcast.log").unlink()
        helper = self.root / "bin/eez-genesis-block-hash"
        helper.write_text(helper.read_text().replace("set -euo pipefail", '''set -euo pipefail
if [[ "${2-}" == --network-profile && -f "$BROADCAST_LOG" ]]; then
    printf '0x%064x\\n' 4; exit
fi'''))
        result = self.run_deploy("")
        self.assertIn("network profile changed", result.stderr)
        self.assertNotIn("RegisterRollup", (self.root / "broadcast.log").read_text())
        self.assertFalse((self.root / "deployments.env").exists())

    def test_timestamped_base_preserves_genesis_read_permissions_for_renderer(self):
        for mode in (0o644, 0o664, 0o640, 0o600):
            with self.subTest(mode=oct(mode)):
                (self.root / "genesis.json").chmod(mode)
                shutil.rmtree(self.root / "datadir", ignore_errors=True)
                result = self.run_deploy("")
                self.assertIn("DeployEEZ failed", result.stderr)
                consumed = self.root / "datadir/render-base-mode"
                self.assertEqual(int(consumed.read_text().strip(), 8), mode)
                rendered = self.root / "datadir/genesis.json"
                self.assertEqual(rendered.stat().st_mode & 0o777, mode)

    def test_treasury_recipient_is_required_and_profile_mismatch_prevents_broadcast(self):
        config = self.root / ".env"
        original = config.read_text()
        for recipient in (None, ""):
            with self.subTest(recipient=recipient):
                config.write_text("\n".join(line for line in original.splitlines()
                    if not line.startswith("EEZ_L2_TREASURY_RECIPIENT="))
                    + ("\nEEZ_L2_TREASURY_RECIPIENT=\n" if recipient == "" else "\n"))
                result = self.run_deploy("")
                self.assertIn("EEZ_L2_TREASURY_RECIPIENT", result.stderr)
                self.assertFalse((self.root / "broadcast.log").exists())
        # A renderer/profile mismatch must stop deployment before broadcasting.
        config.write_text(original)
        result = self.run_deploy("treasury")
        self.assertIn("rendered treasury profile differs", result.stderr)
        self.assertFalse((self.root / "broadcast.log").exists())

    def test_treasury_owner_and_recipient_reach_the_renderer(self):
        config = self.root / ".env"
        original = config.read_text()
        for dotenv_owner, shell_owner, expected in (
            (None, None, "0x" + "33" * 20),
            (None, "0x" + "44" * 20, "0x" + "44" * 20),
            ("0x" + "55" * 20, None, "0x" + "55" * 20),
            ("0x" + "55" * 20, "0x" + "44" * 20, "0x" + "55" * 20),
        ):
            with self.subTest(dotenv=dotenv_owner, shell=shell_owner):
                config.write_text(original + (f"EEZ_L2_TREASURY_OWNER={dotenv_owner}\n"
                                             if dotenv_owner is not None else ""))
                shutil.rmtree(self.root / "datadir", ignore_errors=True)
                overrides = {"EEZ_L2_TREASURY_RECIPIENT": "0x" + "22" * 20}
                if shell_owner is not None:
                    overrides["EEZ_L2_TREASURY_OWNER"] = shell_owner
                result = self.run_deploy("", **overrides)
                self.assertIn("DeployEEZ failed", result.stderr)
                profile = json.loads((self.root / "datadir/genesis.profile.json").read_text())
                self.assertEqual(profile["treasuryOwner"], expected)
                # deploy.sh sources .env, so an explicit assignment there wins.
                self.assertEqual(profile["treasuryRecipient"], "0x" + "11" * 20)
                self.assertEqual(profile["feeCollector"], "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078")

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

    def test_history_delay_is_fixed_at_genesis_and_rejects_invalid_inputs(self):
        for value in ("", "-1", "5", "invalid"):
            with self.subTest(value=value):
                result = self.run_deploy("", EEZ_L2_HISTORY_UPDATE_OFFSET=value)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((self.root / "broadcast.log").exists())
        result = self.run_deploy("", EEZ_L2_HISTORY_UPDATE_OFFSET="4")
        self.assertIn("DeployEEZ failed", result.stderr)
        genesis = json.loads((self.root / "datadir/genesis.json").read_text())
        self.assertEqual(genesis["config"]["rollupX"]["historyUpdateOffset"], 4)

    def test_existing_genesis_is_preserved_without_broadcast(self):
        directory = self.root / "datadir"
        directory.mkdir()
        genesis = directory / "genesis.json"
        genesis.write_text("existing network state")
        result = self.run_deploy("")
        self.assertIn("output files already exist", result.stderr)
        self.assertEqual(genesis.read_text(), "existing network state")
        self.assertFalse((self.root / "broadcast.log").exists())

    def test_private_network_requires_a_local_l1_profile(self):
        result = self.run_deploy("", CHAIN_ID="31337", EEZ_L1_REFERENCE="genesis")
        self.assertIn("private L1 requires EEZ_L1_CHAIN_CONFIG", result.stderr)
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



class DeployImageAssetsTest(unittest.TestCase):
    def test_render_history_identities_exist_at_their_consumed_image_paths(self):
        renderer = (REPO / "scripts/update-eezl2-genesis.sh").read_text()
        assets = re.findall(r"root / '([^'\n]+(?:\.bin|\.sol))'", renderer)
        self.assertEqual(len(assets), 3, "history proxy, implementation and admin must be packaged")
        dockerfile = (REPO / "testing/kurtosis/Dockerfile.deploy").read_text()
        for asset in assets:
            with self.subTest(asset=asset):
                source_asset = REPO / asset
                self.assertTrue(source_asset.is_file())
                destinations = []
                workdir = Path("/")
                for line in dockerfile.splitlines():
                    if not line.startswith(("WORKDIR ", "COPY ")):
                        continue
                    words = shlex.split(line, comments=True)
                    if not words:
                        continue
                    if words[0] == "WORKDIR":
                        workdir = Path(words[1])
                    if words[0] != "COPY" or words[1].startswith("--from="):
                        continue
                    target = workdir / words[-1]
                    for source in words[1:-1]:
                        source_path = REPO / source
                        if source_path.is_dir() and source_asset.is_relative_to(source_path):
                            destinations.append(target / source_asset.relative_to(source_path))
                        elif source_path == source_asset:
                            destinations.append(target / source_path.name
                                                if words[-1].endswith("/") or len(words) > 3
                                                else target)
                self.assertIn(Path("/repo") / asset, destinations,
                              "deploy image must retain the identity checked by --render")


@unittest.skipUnless(shutil.which("cast") and shutil.which("jq"), "requires Foundry cast and jq")
class TreasuryRenderPreflight(unittest.TestCase):
    def test_invalid_treasury_inputs_are_rejected_before_writing_output(self):
        for field, other in (("owner", "recipient"), ("recipient", "owner")):
            flag = "--treasury-" + field
            cases = [
                ([], "usage:"),
                ([flag, ""], "usage:"),
                ([flag, "0x1234"], "exactly 20 hexadecimal bytes"),
                ([flag, "0x" + "gg" * 20], "exactly 20 hexadecimal bytes"),
                ([flag, "11" * 20], "exactly 20 hexadecimal bytes"),
                ([flag, "0x" + "00" * 20], "nonzero and differ"),
                ([flag, "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078"], "nonzero and differ"),
                ([flag, "0xfffffffffffffffffffffffffffffffffffffffe"], "nonzero and differ"),
                ([flag, "0x4B2760F84B83d6b3829E30A7573B0172B6d6402C"], "nonzero and differ"),
                ([flag, "0x" + "11" * 20, flag, "0x" + "22" * 20], "must occur exactly once"),
            ]
            for arguments, message in cases:
                with self.subTest(field=field, arguments=arguments), tempfile.TemporaryDirectory() as directory:
                    output = Path(directory) / "genesis.json"
                    result = subprocess.run([
                        "bash", str(REPO / "scripts/update-eezl2-genesis.sh"), "--render",
                        "--rollup-id", "1", "--system-address", "0xfffffffffffffffffffffffffffffffffffffffe",
                        "--recovery-address", "0x" + "11" * 20,
                        "--upgrade-owner", "0x" + "22" * 20,
                        "--treasury-" + other, "0x" + "33" * 20,
                        "--output", str(output), *arguments,
                    ], env={**os.environ, "GITHUB_ACTIONS": "false"}, capture_output=True, text=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(message, result.stderr)
                    self.assertFalse(output.exists())

if __name__ == "__main__":
    unittest.main()
