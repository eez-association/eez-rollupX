"""Verify explicit network routing and bounded funding without RPC access."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[3]
LIB = REPO / "testing/kurtosis/scripts/lib.sh"
PORTS = REPO / "testing/kurtosis/ports.sh"


class PublicNetworkChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = {**os.environ, "REPO": str(self.root), "ENCLAVE": "fixture",
                    "EEZ_USE_LOCAL_DEPLOYMENTS": "1"}
        for name in ("L1_RPC", "L2_RPC", "L1_FRONT", "L2_FRONT"):
            self.env["EEZ_DEVNET_" + name] = "http://127.0.0.1:1234/" + name

    def run_shell(self, script, success=True):
        result = subprocess.run(["bash", "-c", "set -e\n" + script],
                                env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def test_explicit_endpoints_do_not_contact_kurtosis(self):
        self.run_shell(f'kurtosis() {{ exit 77; }}\nsource "{PORTS}"')

    def test_missing_endpoint_fails_without_enclave_fallback(self):
        self.env.pop("EEZ_DEVNET_L1_FRONT")
        result = self.run_shell(f'kurtosis() {{ exit 77; }}\nsource "{PORTS}"', False)
        self.assertIn("EEZ_DEVNET_L1_FRONT", result.stderr)

    def test_invalid_endpoint_fails(self):
        self.env["EEZ_DEVNET_L1_RPC"] = "file:///unexpected"
        self.run_shell(f'source "{PORTS}"', False)

    def test_explicit_deployments_override_checkout_bindings(self):
        (self.root / "deployments.env").write_text("NETWORK=wrong\n")
        expected = self.root / "chiado.env"
        expected.write_text("NETWORK=chiado\n")
        destination = self.root / "loaded"
        destination.mkdir()
        self.env["EEZ_DEPLOYMENTS_FILE"] = str(expected)
        self.run_shell(f'source "{LIB}"\nkurtosis() {{ exit 77; }}\n'
                       f'load_deployment_artifact "{destination}"')
        self.assertEqual((destination / "deployments.env").read_text(), "NETWORK=chiado\n")

    def test_missing_deployments_fail_without_enclave_fallback(self):
        self.run_shell(f'source "{LIB}"\nkurtosis() {{ exit 77; }}\n'
                       f'load_deployment_artifact "{self.root}"', False)

    def test_default_deployments_still_use_enclave(self):
        self.env.pop("EEZ_USE_LOCAL_DEPLOYMENTS")
        log = self.root / "called"
        self.run_shell(f'source "{LIB}"\nkurtosis() {{ printf "%s" "$*" > "{log}"; }}\n'
                       f'load_deployment_artifact "{self.root}"')
        self.assertEqual(log.read_text(), f"files download fixture eez-deployments {self.root}")

    def test_invalid_funding_is_rejected_before_rpc(self):
        for amount in ("0", "0.000", "-1", "1;false", "0.0000000000000000001"):
            with self.subTest(amount=amount):
                self.env["EEZ_TEST_FUND_AMOUNT_ETH"] = amount
                result = self.run_shell(f'source "{LIB}"\ncast() {{ exit 77; }}\n'
                                        'fund http://unused dummy 0x123', False)
                self.assertIn("positive decimal", result.stderr)

    def test_default_and_small_funding_amounts_reach_signing(self):
        log = self.root / "send"
        for amount in (None, "0.05"):
            with self.subTest(amount=amount):
                self.env.pop("EEZ_TEST_FUND_AMOUNT_ETH", None)
                if amount is not None:
                    self.env["EEZ_TEST_FUND_AMOUNT_ETH"] = amount
                self.run_shell(f'''source "{LIB}"
cast() {{
  case "$1" in
    wallet) echo 0x123 ;;
    nonce) echo 0 ;;
    send) printf '%s' "$*" > "{log}" ;;
    *) exit 77 ;;
  esac
}}
gas_price_for() {{ echo 1; }}
fund http://unused dummy 0x456
''')
                self.assertIn(f"--value {amount or '10'}ether", log.read_text())


    def test_raw_transaction_fallback_preserves_typed_and_legacy_envelopes(self):
        # The fallback takes the transaction at its exact raw-block index and
        # hashes its original envelope; it does not synthesize unsigned data.
        for envelope in ('"0x02c0"', '["0x01", "0x02"]'):
            with self.subTest(envelope=envelope):
                self.run_shell(f'''source "{LIB}"
real_cast=$(command -v cast)
expected_raw=$(printf '%s' '{envelope}' | jq -r 'if type == "string" then . else empty end')
if [[ -z "$expected_raw" ]]; then expected_raw=$(printf '%s' '{envelope}' | "$real_cast" to-rlp); fi
expected_hash=$(printf '%s' "$expected_raw" | "$real_cast" keccak)
block=$(printf '%s' '[[],["0x01c0",{envelope}],[]]' | "$real_cast" to-rlp)
cast() {{
  if [[ "$1" != rpc ]]; then "$real_cast" "$@"; return; fi
  case "$2" in
    eth_getRawTransactionByHash) echo null ;;
    eth_getTransactionByHash) echo '{{"blockHash":"0x{'a' * 64}","transactionIndex":"0x1"}}' ;;
    debug_getRawBlock) [[ "$3" == "0x{'a' * 64}" ]] || return 77; printf '"%s"' "$block" ;;
    *) return 77 ;;
  esac
}}
actual=$(raw_transaction_for http://unused "$expected_hash" "{self.root}")
[[ "$actual" == "$expected_raw" ]]
if raw_transaction_for http://unused "0x{'b' * 64}" "{self.root}"; then exit 77; fi
''')

    def test_raw_transaction_rpc_bytes_are_hash_checked(self):
        self.run_shell(f'''source "{LIB}"
real_cast=$(command -v cast)
expected_hash=$(printf '0x02c0' | "$real_cast" keccak)
cast() {{
  if [[ "$1" == rpc ]]; then
    [[ "$2" == eth_getRawTransactionByHash ]] || return 77
    echo '"0x02c0"'
  else "$real_cast" "$@"; fi
}}
[[ $(raw_transaction_for http://unused "$expected_hash" "{self.root}") == 0x02c0 ]]
if raw_transaction_for http://unused "0x{'b' * 64}" "{self.root}"; then exit 77; fi
''')

    def test_public_replay_uses_explicit_genesis_profile_and_timing(self):
        evidence = self.root / "evidence.json"
        evidence.write_text('{"enclave":"public","rollup_id":1}')
        genesis = self.root / "genesis.json"
        genesis.write_text("{}")
        deployment = self.root / "deployment.env"
        deployment.write_text("EEZ_REGISTRY_ADDRESS=fixture\n")
        observed = self.root / "observed"
        self.env.update(EEZ_DEPLOYMENTS_FILE=str(deployment),
                        EEZ_L2_GENESIS=str(genesis), EEZ_L1_CHAIN_CONFIG="chiado",
                        EEZ_L1_BEACON_URL="http://beacon.invalid", EEZ_L2_BLOCK_TIME_MS="1000",
                        EEZ_L2_SYSTEM_KEY="fixture", EEZ_PROOF_TIME_MS="1500",
                        EEZ_SUBMISSION_SLACK_MS="700")
        script = REPO / "testing/kurtosis/scripts/verify-policy-replay.sh"
        self.run_shell(f'''kurtosis() {{ exit 77; }}
cast() {{ [[ "$1" == chain-id ]] || exit 77; echo 10200; }}
curl() {{ echo '{{"data":{{"SECONDS_PER_SLOT":"5"}}}}'; }}
cargo() {{ printf '%s\\n' "$EEZ_KURTOSIS_L2_GENESIS" "$EEZ_L1_CHAIN_CONFIG" "$EEZ_L1_CHAIN_ID" "$EEZ_L1_BLOCK_TIME_MS" "$EEZ_L2_BLOCK_TIME_MS" "$EEZ_PROOF_TIME_MS" "$EEZ_SUBMISSION_SLACK_MS" > "{observed}"; }}
export -f kurtosis cast curl cargo
bash "{script}" "{evidence}"
''')
        self.assertEqual(observed.read_text().splitlines(),
                         [str(genesis), "chiado", "10200", "5000", "1000", "1500", "700"])

if __name__ == "__main__":
    unittest.main()
