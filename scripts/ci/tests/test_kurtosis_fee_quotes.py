"""Exercise the wave's quote/signing path with real envelopes and mocked RPCs."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

REPO = Path(__file__).resolve().parents[3]
CAST = shutil.which("cast")
KEY = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
TARGET = "0x1111111111111111111111111111111111111111"


@unittest.skipUnless(CAST and shutil.which("jq") and shutil.which("bc"), "cast, jq and bc required")
class WaveQuoteTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shim = self.root / "cast"
        shim.write_text('''#!/usr/bin/env python3
import json, os, pathlib, subprocess, sys
root = pathlib.Path(os.environ["FIXTURE"])
args = sys.argv[1:]
if args[0] == "chain-id":
    print(7331)
elif args[0] == "nonce":
    p = root / "nonces.json"
    values = json.loads(p.read_text())
    print(values[0])
    p.write_text(json.dumps(values[1:] or values))
elif args[0] == "balance":
    print(10 ** 19)
elif args[0] == "rpc":
    if "eez_getCrossChainTransaction" in args:
        print((root / "queue.json").read_text())
        sys.exit()
    request = json.loads(args[args.index("eez_quoteFee") + 1])
    (root / "request.json").write_text(json.dumps(request))
    if (root / "error.txt").exists():
        print((root / "error.txt").read_text(), file=sys.stderr)
        sys.exit(1)
    print((root / "quote.json").read_text())
else:
    if args[0] == "mktx":
        (root / "signed").touch()
    sys.exit(subprocess.run([os.environ["REAL_CAST"], *args]).returncode)
''')
        shim.chmod(0o755)
        curl = self.root / "curl"
        curl.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ["FIXTURE"])
flag = "--data" if "--data" in sys.argv else "-d"
request = json.loads(sys.argv[sys.argv.index(flag) + 1])
with (root / "submissions.jsonl").open("a") as out:
    out.write(json.dumps(request) + "\\n")
if (root / "transport-failure").exists():
    sys.exit(7)
responses = root / "submission-responses.json"
if responses.exists():
    values = json.loads(responses.read_text())
    print(json.dumps(values[0]))
    responses.write_text(json.dumps(values[1:] or values))
    sys.exit()
name = "receipt.json" if request["method"] == "eth_getTransactionReceipt" else "rejection.json"
print((root / name).read_text())
''')
        curl.chmod(0o755)
        self.env = {**os.environ, "PATH": str(self.root) + os.pathsep + os.environ["PATH"],
                    "FIXTURE": str(self.root), "REAL_CAST": CAST, "REPO": str(REPO),
                    "FIXTURE_KEY": KEY}
        (self.root / "nonces.json").write_text("[7]")

    def run_quote(self, kind="0x2", option_error=False, disabled=False, l2=False):
        args = [CAST, "mktx", TARGET, "0x12345678", "--private-key", KEY, "--chain", "7331",
                "--nonce", "7", "--gas-limit", "600000", "--gas-price", "100", "--value", "123wei"]
        args += ["--legacy"] if kind == "0x0" else ["--priority-gas-price", "1"]
        raw = subprocess.check_output(args, text=True).strip()
        option = {"type": kind, "gas": "0x30d40", "estimatedFee": "0x30d4000",
                  "requestHash": "0x" + "ab" * 32}
        option.update({"gasPrice": "0x100"} if kind == "0x0" else
                      {"maxFeePerGas": "0x100", "maxPriorityFeePerGas": "0x80"})
        if option_error:
            option = {"type": kind, "error": {"code": "unsupported_transaction", "message": "fixture revert"}}
        quote = {"version": 1, "chainId": "0x1ca3", "nonce": "0x7",
                 "expiresAt": hex(int(time.time()) + 30), "options": [option]}
        (self.root / "quote.json").write_text(json.dumps(quote))
        if disabled:
            (self.root / "error.txt").write_text('server returned an error response: error code -32004: quotes_disabled')
        env = dict(self.env, PREPARED=raw, L2="http://read" if l2 else "http://l2")
        result = subprocess.run(["bash", "-c", '''set -euo pipefail
source "$REPO/testing/kurtosis/scripts/lib.sh"
sign_fee_quoted http://front http://read "$FIXTURE_KEY" "$PREPARED" "$FIXTURE/evidence.json"
'''], env=env, capture_output=True, text=True, timeout=15)
        return raw, result

    def test_preserves_each_envelope_and_uses_quoted_gas_and_fees(self):
        for kind in ("0x0", "0x2"):
            with self.subTest(kind=kind):
                original, result = self.run_quote(kind)
                self.assertEqual(result.returncode, 0, result.stderr)
                def decode(raw):
                    decoded = json.loads(subprocess.check_output([CAST, "decode-transaction", raw], text=True))
                    return json.loads(decoded) if isinstance(decoded, str) else decoded
                before, after = decode(original), decode(result.stdout.strip())
                for key in ("signer", "type", "chainId", "nonce", "to", "value", "input"):
                    self.assertEqual(before[key], after[key], key)
                self.assertEqual(int(after["gas"], 16), 200000)
                fee = "gasPrice" if kind == "0x0" else "maxFeePerGas"
                self.assertEqual(int(after[fee], 16), 256)
                request = json.loads((self.root / "request.json").read_text())
                self.assertEqual(request["nonce"], "0x7")
                self.assertEqual(request["data"], "0x12345678")
                evidence = json.loads((self.root / "evidence.json").read_text())
                self.assertEqual(evidence["feeSource"], "composer")
                self.assertNotIn("raw", evidence)
                self.assertTrue((self.root / "evidence.responses.jsonl").is_file())

    def test_option_rejection_never_signs_or_uses_node_prices(self):
        _, result = self.run_quote(option_error=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported_transaction", result.stderr)
        self.assertFalse((self.root / "signed").exists())

    def test_disabled_quotes_are_terminal(self):
        _, result = self.run_quote(disabled=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("quotes_disabled", result.stderr)
        self.assertFalse((self.root / "signed").exists())

    def test_l2_waits_for_predecessor_settlement_before_quoting(self):
        (self.root / "nonces.json").write_text("[6, 7]")
        _, result = self.run_quote(l2=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / "request.json").is_file())

    def test_consumed_nonce_fails_before_quote_or_signing(self):
        (self.root / "nonces.json").write_text("[8]")
        _, result = self.run_quote(l2=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected=7 current=8", result.stderr)
        self.assertFalse((self.root / "request.json").exists())
        self.assertFalse((self.root / "signed").exists())

    def test_invalid_calls_require_execution_rejection_and_no_ownership_or_receipt(self):
        cases = (("unsupported_transaction", None, None, 0),
                 ("l2_tip_underfunded", None, None, 1),
                 ("quote_unavailable", None, None, 1),
                 ("unsupported_transaction", {"status": "queued"}, None, 1),
                 ("unsupported_transaction", None, {"status": "0x1"}, 1))
        for code, queue, receipt, expected in cases:
            with self.subTest(code=code, queue=queue, receipt=receipt):
                (self.root / "rejection.json").write_text(json.dumps({"error": {
                    "code": -32003, "data": {"code": code}}}))
                (self.root / "queue.json").write_text(json.dumps(queue))
                (self.root / "receipt.json").write_text(json.dumps({"result": receipt}))
                result = subprocess.run(["bash", "-c", '''set -euo pipefail
source "$REPO/testing/kurtosis/scripts/lib.sh"
expect_fee_admission_rejection http://front http://read 0x00 0x1234 "$FIXTURE/rejection-evidence.json"
'''], env=self.env, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, expected, result.stderr)

    def test_workload_expectations_follow_profile_and_explicit_disabled_override(self):
        profile = self.root / "args.yaml"
        profile.write_text('eez:\n  settlement_env:\n    EEZ_FEES_L1_TO_L2_ENABLED: "true"\n'
                           '    EEZ_FEES_L2_TO_L1_ENABLED: "false"\n')
        for side, override, expected in (("in", {}, 0), ("out", {}, 1),
                                         ("in", {"EEZ_FEES_L1_TO_L2_ENABLED": "false"}, 1)):
            with self.subTest(side=side, override=override):
                env = {key: value for key, value in self.env.items() if not key.startswith("EEZ_FEES_")}
                env.update(override, KURTOSIS_ARGS_FILE=str(profile), SIDE=side)
                result = subprocess.run(["bash", "-c", '''set -euo pipefail
source "$REPO/testing/kurtosis/scripts/lib.sh"
fee_collection_enabled "$SIDE"
'''], env=env, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, expected, result.stderr)

    def test_workload_expectations_accept_all_simple_scalar_quote_styles(self):
        profile = self.root / "args.yaml"
        for quote in ("", "'", '"'):
            for enabled in ("true", "false"):
                for side in ("in", "out"):
                    with self.subTest(quote=quote, enabled=enabled, side=side):
                        profile.write_text(
                            "eez:\n  settlement_env:\n"
                            f"    EEZ_FEES_L1_TO_L2_ENABLED: {quote}{enabled}{quote}  # profile\n"
                            f"    EEZ_FEES_L2_TO_L1_ENABLED: {quote}{enabled}{quote}\n"
                            f"    EEZ_SETTLEMENT_START_AFTER_BLOCK: {quote}3{quote}  \n")
                        env = {key: value for key, value in self.env.items()
                               if not key.startswith("EEZ_FEES_")}
                        env.update(KURTOSIS_ARGS_FILE=str(profile), SIDE=side)
                        result = subprocess.run(["bash", "-c", '''set -euo pipefail
source "$REPO/testing/kurtosis/scripts/lib.sh"
[[ $(yaml_value EEZ_SETTLEMENT_START_AFTER_BLOCK) == 3 ]]
fee_collection_enabled "$SIDE"
'''], env=env, capture_output=True, text=True, timeout=5)
                        self.assertEqual(result.returncode, int(enabled != "true"), result.stderr)

    def test_workload_expectations_do_not_enable_absent_or_mismatched_scalars(self):
        profile = self.root / "args.yaml"
        for scalar in (None, "", "# comment", "true#suffix", "'true", 'true"', "\"true'"):
            with self.subTest(scalar=scalar):
                profile.write_text("eez:\n  settlement_env:\n" + (
                    f"    EEZ_FEES_L1_TO_L2_ENABLED: {scalar}\n" if scalar is not None else ""))
                env = {key: value for key, value in self.env.items()
                       if not key.startswith("EEZ_FEES_")}
                env.update(KURTOSIS_ARGS_FILE=str(profile))
                result = subprocess.run(["bash", "-c", '''set -euo pipefail
source "$REPO/testing/kurtosis/scripts/lib.sh"
fee_collection_enabled in
'''], env=env, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 1, result.stderr)

    def run_submission(self, responses, transport_failure=False):
        (self.root / "submission-responses.json").write_text(json.dumps(responses))
        (self.root / "submissions.jsonl").unlink(missing_ok=True)
        marker = self.root / "transport-failure"
        marker.unlink(missing_ok=True)
        if transport_failure:
            marker.touch()
        result = subprocess.run(["bash", "-c", '''set -euo pipefail
# Make the bounded retry clock deterministic; Bash's special SECONDS also ticks.
unset SECONDS
SECONDS=0
sleep() { SECONDS=$((SECONDS + $1)); }
source "$REPO/testing/kurtosis/scripts/lib.sh"
send_front http://front 0xabcdef 0x1234
'''], env=self.env, capture_output=True, text=True, timeout=15)
        requests = [json.loads(line) for line in (self.root / "submissions.jsonl").read_text().splitlines()]
        self.assertTrue(all(request["method"] == "eth_sendRawTransaction" and
                            request["params"] == ["0xabcdef"] for request in requests))
        return result, requests

    def test_submission_retries_only_temporary_admission_refusals_with_identical_bytes(self):
        success = {"jsonrpc": "2.0", "id": 1, "result": "0x1234"}
        for code, message in (("quote_unavailable", "quote unavailable: queued predecessor is entering composition"),
                              ("quote_unavailable", "quote unavailable: settlement reconciliation pending"),
                              ("quote_unavailable", "quote unavailable: unsettled native history requires reconciliation"),
                              ("quote_busy", "quote request capacity is full"),
                              ("stale_context", "quote context expired or changed")):
            with self.subTest(code=code, message=message):
                error = {"error": {"code": -32003, "data": {"code": code, "message": message}}}
                result, requests = self.run_submission([error, success])
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(requests), 2)

    def test_submission_does_not_retry_tariff_profile_transport_or_hash_failures(self):
        responses = [
            {"error": {"code": -32003, "data": {"code": "l1_tip_underfunded"}}},
            {"error": {"code": -32003, "data": {"code": "unsupported_transaction"}}},
            {"error": {"code": -32003, "data": {"code": "quote_unavailable", "message": "unknown outage"}}},
            {"error": {"code": -32602, "data": {"code": "quote_busy"}}},
            {"result": "0x9876"}, {},
        ]
        for response in responses:
            with self.subTest(response=response):
                result, requests = self.run_submission([response, {"result": "0x1234"}])
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(requests), 1)
        result, requests = self.run_submission([{"result": "0x1234"}], transport_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(requests), 1)

    def test_temporary_submission_refusals_exhaust_the_bounded_deadline(self):
        error = {"error": {"code": -32003, "data": {"code": "quote_busy"}}}
        result, requests = self.run_submission([error])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("admission retry deadline", result.stderr)
        self.assertEqual(len(requests), 120)

    def test_outbound_only_wave_does_not_read_an_inbound_nonce(self):
        source = (REPO / "testing/kurtosis/scripts/cross-chain-wave.sh").read_text()
        start = source.index("    mk_and_send() {")
        end = source.index("\n    submit_pure_filler()", start)
        (self.root / "wave-function.sh").write_text(source[start:end])
        result = subprocess.run(["bash", "-c", '''set -euo pipefail
L1=http://l1 L2=http://l2 L1F=http://l1-front L2F=http://l2-front
MODE=outbound L2_CHAIN_ID=696990 OUT_NONCE=7
HH_KEY_OUT=fixture-key OUT_VALUE_PROXY=0x1111111111111111111111111111111111111111
PRIORITY_GAS_PRICE=1 EEZ_CI_RESULT_DIR="$FIXTURE"
OUT_HASHES=() TX_META=()
unset IN_NONCE HH_KEY_IN
gas_price_for() { echo 100; }
cast() { case "$1" in mktx) echo 0xab ;; keccak) echo 0x1234 ;; *) return 1 ;; esac; }
fee_collection_enabled() { return 0; }
sign_fee_quoted() {
    [[ "$1" == "$L2F" && "$2" == "$L2" && "$3" == "$HH_KEY_OUT" ]]
    [[ "$5" == */wave-outbound-out-7.json ]]
    echo 0xcd
}
send_front() { [[ "$1" == "$L2F" && "$2" == 0xcd && "$3" == 0x1234 ]]; }
source "$FIXTURE/wave-function.sh"
mk_and_send out set 401
[[ "$OUT_NONCE" == 8 && "${#OUT_HASHES[@]}" == 1 && "${#TX_META[@]}" == 1 ]]
'''], env=self.env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
