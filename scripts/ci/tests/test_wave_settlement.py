"""Offline regression tests for wave setup and settlement; no live network."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import urllib.error


REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("wave_settlement", REPO / "scripts/ci/wait-wave-settlement.py")
wave = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wave)


def digest(value):
    return "0x" + f"{value:064x}"


class Snapshot:
    def __init__(self):
        self.config = {
            "rpc": {"l1": "http://l1.invalid", "l2": "http://l2.invalid"},
            "registry": "0x" + "1" * 40, "rollup_call": "0x" + "2" * 72,
            "receipts": {"l1": [digest(1)], "l2": [digest(2)]},
            "checks": [
                {"chain": chain, "method": method, "argument": argument, "expected": str(expected)}
                for chain in ("l1", "l2")
                for method, argument, expected in (
                    ("eth_call", {"to": "0x" + "3" * 40, "data": "0x3fa4f245"}, 12),
                    ("eth_call", {"to": "0x" + "4" * 40, "data": "0x3fa4f245"}, 13),
                    ("eth_getBalance", "0x" + "5" * 40, 10**18 + 1))],
        }
        self.heads = {chain: {"number": hex(number), "hash": digest(number)}
                      for chain, number in (("l1", 20), ("l2", 40))}
        self.receipts = {chain: {"status": "0x1", "blockNumber": head["number"], "blockHash": head["hash"],
                                 "transactionHash": self.config["receipts"][chain][0]}
                         for chain, head in self.heads.items()}
        self.commitment = "0x" + "0" * 64 + self.heads["l2"]["hash"][2:] + "0" * 64
        self.state = {chain: True for chain in self.heads}
        self.reorg = None
        self.calls = []

    def __call__(self, chain, method, params):
        self.calls.append((chain, method, params))
        if method == "eth_getBlockByNumber":
            head = dict(self.heads[chain])
            if params[0].startswith("0x"):
                head["number"] = params[0]
                head["hash"] = digest(int(params[0], 16))
                if self.reorg == chain:
                    head["hash"] = digest(99)
            return head
        if method == "eth_getTransactionReceipt":
            return self.receipts[chain]
        if method == "eth_call" and params[0]["to"] == self.config["registry"]:
            return self.commitment
        for check in self.config["checks"]:
            if check["chain"] == chain and check["method"] == method and check["argument"] == params[0]:
                if params[1] != self.heads[chain]["number"]:
                    raise AssertionError("destination reads must use pinned block numbers")
                return hex(int(check["expected"]) if self.state[chain] else 0)
        raise AssertionError((chain, method, params))


class WaveSettlementTest(unittest.TestCase):
    def setUp(self):
        self.snapshot = Snapshot()

    def test_valid_snapshot_is_immediate_and_read_only(self):
        with patch.object(wave, "Rpc", return_value=self.snapshot), patch.object(wave.time, "sleep") as sleep:
            with patch("sys.stdout", new=io.StringIO()):
                wave.wait(self.snapshot.config, 1)
            sleep.assert_not_called()
        self.assertTrue(all(method.startswith("eth_get") or method == "eth_call"
                            for _, method, _ in self.snapshot.calls))

    def test_json_rpc_transport_encodes_reads_and_decodes_results(self):
        def response(request, timeout):
            self.assertGreater(timeout, 0)
            body = json.loads(request.data)
            self.assertEqual((body["id"], body["jsonrpc"]), (1, "2.0"))
            chain = "l1" if request.full_url == self.snapshot.config["rpc"]["l1"] else "l2"
            result = self.snapshot(chain, body["method"], body["params"])
            return io.BytesIO(json.dumps({"jsonrpc": "2.0", "id": 1, "result": result}).encode())

        with patch.object(wave.urllib.request, "urlopen", side_effect=response), patch("sys.stdout", new=io.StringIO()):
            wave.wait(self.snapshot.config, 1)

    def test_shell_builds_complete_checks_without_rounding_balances(self):
        script = (REPO / "testing/kurtosis/scripts/cross-chain-wave.sh").read_text()
        start = script.index("    local checks='[]'")
        end = script.index("\n    refresh_node_log", start)
        with tempfile.TemporaryDirectory(prefix="eez-wave-json.") as temporary:
            root = Path(temporary)
            # Capture the actual shell-generated stdin without starting a node.
            (root / "python3").write_text("#!/bin/sh\nexec cat\n")
            (root / "cast").write_text("""#!/bin/sh
case "$2" in
    'value()') echo 0x3fa4f245 ;;
    'rollups(uint64)') printf '0x%072d\\n' 1 ;;
    *) exit 1 ;;
esac
""")
            for path in root.iterdir():
                path.chmod(0o755)
            setup = '''
REPO=/unused
L1=http://l1.invalid L2=http://l2.invalid
EEZ_REGISTRY_ADDRESS=0x1111111111111111111111111111111111111111 EEZ_ROLLUP_ID=1
L2_VALUE=0x3333333333333333333333333333333333333333
L2_VALUE_NORET=0x4444444444444444444444444444444444444444
L2_DEP_RECIPIENT=0x5555555555555555555555555555555555555555
L1_VALUE=$L2_VALUE L1_VALUE_NORET=$L2_VALUE_NORET L1_WD_RECIPIENT=$L2_DEP_RECIPIENT
IN_LAST_VALUE=12 IN_LAST_NORET=13 OUT_LAST_VALUE=12 OUT_LAST_NORET=13
DEP_BEFORE=1000000000000000000 WD_BEFORE=$DEP_BEFORE IN_DEP_SUM=1 OUT_WD_SUM=1
IN_HASHES=() OUT_HASHES=()
if (( do_in )); then IN_HASHES+=("0x0000000000000000000000000000000000000000000000000000000000000001"); fi
if (( do_out )); then OUT_HASHES+=("0x0000000000000000000000000000000000000000000000000000000000000002"); fi
'''
            for incoming, outgoing in ((1, 0), (0, 1), (1, 1)):
                command = f"do_in={incoming}; do_out={outgoing}\n" + setup + "\ncheck() {\n" + script[start:end] + "\n}\ncheck\n"
                result = subprocess.run(["bash", "-euo", "pipefail", "-c", command], text=True, capture_output=True,
                                        env={**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"]}, timeout=10)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                config = json.loads(result.stdout)
                wave.validate(config)
                self.assertEqual(len(config["checks"]), 3 * (incoming + outgoing))
                self.assertEqual(config["receipts"]["l1"], [digest(1)] if incoming else [])
                self.assertEqual(config["receipts"]["l2"], [digest(2)] if outgoing else [])
                for check in config["checks"]:
                    if check["method"] == "eth_getBalance":
                        self.assertEqual(check["expected"], "1000000000000000001")

    def test_inbound_outbound_and_mixed_modes(self):
        for mode in ("inbound", "outbound", "mixed", "mixed-pure"):
            with self.subTest(mode=mode):
                snapshot = Snapshot()
                if mode == "inbound":
                    snapshot.config["receipts"]["l2"] = []
                    snapshot.config["checks"] = [c for c in snapshot.config["checks"] if c["chain"] == "l2"]
                elif mode == "outbound":
                    snapshot.config["receipts"]["l1"] = []
                    snapshot.config["checks"] = [c for c in snapshot.config["checks"] if c["chain"] == "l1"]
                wave.validate(snapshot.config)
                wave.settled_snapshot(snapshot.config, snapshot)

    def test_receipts_must_be_successful_canonical_and_safe(self):
        for chain in ("l1", "l2"):
            for field, value in (("status", "0x0"), ("blockHash", digest(99)),
                                 ("blockNumber", "0x100"), ("transactionHash", digest(99))):
                with self.subTest(chain=chain, field=field):
                    snapshot = Snapshot()
                    snapshot.receipts[chain][field] = value
                    with self.assertRaises(wave.NotReady):
                        wave.settled_snapshot(snapshot.config, snapshot)
            snapshot = Snapshot()
            snapshot.receipts[chain] = None
            with self.assertRaises(wave.NotReady):
                wave.settled_snapshot(snapshot.config, snapshot)

    def test_older_canonical_receipts_are_accepted(self):
        for chain in ("l1", "l2"):
            self.snapshot.receipts[chain].update(blockNumber="0x10", blockHash=digest(16))
        wave.settled_snapshot(self.snapshot.config, self.snapshot)

    def test_reorg_during_state_reads_invalidates_the_snapshot(self):
        for chain in ("l1", "l2"):
            self.snapshot.reorg = chain
            with self.subTest(chain=chain), self.assertRaisesRegex(wave.NotReady, "snapshot changed"):
                wave.settled_snapshot(self.snapshot.config, self.snapshot)

    def test_destination_state_and_commitment_are_required(self):
        for chain in ("l1", "l2"):
            snapshot = Snapshot()
            snapshot.state[chain] = False
            with self.subTest(chain=chain), self.assertRaisesRegex(wave.NotReady, "destination state"):
                wave.settled_snapshot(snapshot.config, snapshot)
        self.snapshot.commitment = "0x" + "0" * 192
        with self.assertRaisesRegex(wave.NotReady, "commitment"):
            wave.settled_snapshot(self.snapshot.config, self.snapshot)

    def test_unavailable_safe_head_does_not_fall_back_to_latest(self):
        def rpc(chain, method, params):
            if chain == "l2" and method == "eth_getBlockByNumber" and params[0] == "safe":
                return None
            return self.snapshot(chain, method, params)

        with self.assertRaises(wave.NotReady):
            wave.settled_snapshot(self.snapshot.config, rpc)

    def test_transient_rpc_failure_is_retried_without_resubmitting_transactions(self):
        calls = 0

        def rpc(*args):
            nonlocal calls
            calls += 1
            if calls == 1:
                raise wave.NotReady("fixture RPC unavailable")
            return self.snapshot(*args)

        with patch.object(wave, "Rpc", return_value=rpc), patch("sys.stdout", new=io.StringIO()):
            wave.wait(self.snapshot.config, 1, interval=0)
        self.assertGreater(calls, 1)

    def test_missing_settlement_fails_at_deadline(self):
        self.snapshot.state["l2"] = False
        with patch.object(wave, "Rpc", return_value=self.snapshot), patch("sys.stdout", new=io.StringIO()):
            with self.assertRaisesRegex(TimeoutError, "destination state"):
                wave.wait(self.snapshot.config, 0.01, interval=0)

    def test_rpc_deadline_and_errors_do_not_leak_credentials(self):
        rpc = wave.Rpc({"l1": "http://user:password@cache.invalid"}, wave.time.monotonic() + 1)
        with patch.object(wave.urllib.request, "urlopen", side_effect=urllib.error.URLError("password")) as request:
            with self.assertRaises(wave.NotReady) as error:
                rpc("l1", "eth_getBlockByNumber", ["latest", False])
            self.assertNotIn("password", str(error.exception))
            self.assertLessEqual(request.call_args.kwargs["timeout"], 1)
        rpc.deadline = 0
        with patch.object(wave.urllib.request, "urlopen") as request:
            with self.assertRaises(wave.NotReady):
                rpc("l1", "eth_getBlockByNumber", ["latest", False])
            request.assert_not_called()

    def test_empty_or_mutating_checks_fail_closed(self):
        for field, value in (("receipts", {"l1": [], "l2": []}), ("checks", [])):
            config = copy.deepcopy(self.snapshot.config)
            config[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                wave.validate(config)
        self.snapshot.config["checks"][0]["method"] = "eth_sendRawTransaction"
        with self.assertRaises(ValueError):
            wave.validate(self.snapshot.config)

    def test_wave_runner_preserves_coverage_and_has_no_trailing_gap(self):
        script = (REPO / "testing/kurtosis/scripts/cross-chain-wave.sh").read_text()
        self.assertIn('if (( w < WAVES )); then\n            sleep "$WAVE_GAP_SECS"', script)
        self.assertNotIn('sleep 15', script)
        self.assertIn('wait-wave-settlement.py', script)
        self.assertNotIn('"$(receipt_status "$mh" "$L1")" == "1" ]] || continue', script)
        for assertion in ("PB_COUNT >= WAVES", "L2ExecutionPerformed", "diverged from L1-confirmed batch",
                          "window validated and signed", "ok_all && signer_ok"):
            self.assertIn(assertion, script)


class WaveProxySetupTest(unittest.TestCase):
    """Execute the shell helper, including its command-substitution call site."""

    def run_setup(self, scenario):
        script = (REPO / "testing/kurtosis/scripts/cross-chain-wave.sh").read_text()
        start = script.index("create_l2_proxy() {")
        end = script.index('\necho "==> creating cross-chain proxies', start)
        setup = r'''
L2=http://l2.invalid L2_CHAIN_ID=6290
HH_KEY_2=fixture-key HH_KEY_2_ADDR=0x2222222222222222222222222222222222222222
EEZL2_ADDRESS=0x4200000000000000000000000000000000000007 MAINNET_RID=0
PRIORITY_GAS_PRICE=1
gas_price_for() { echo 15; }
sleep() { echo sleep >> "$CALLS"; }
cast() {
    echo "$*" >> "$CALLS"
    case "$1" in
        call)
            [[ "$SCENARIO" != address-error ]] || return 7
            echo 0x1111111111111111111111111111111111111111 ;;
        nonce) echo 8 ;;
        code)
            [[ "$SCENARIO" != code-error ]] || return 7
            if [[ "$SCENARIO" == existing || ( -f "$SENT" && "$SCENARIO" != no-code ) ]]; then
                echo 0x6000
            else
                echo 0x
            fi ;;
        mktx) echo 0xfeed ;;
        send)
            touch "$SENT"
            case "$SCENARIO" in
                rejected|timeout) echo 'proxy send failed' >&2; return 9 ;;
                reverted) echo '{"status":"0x0"}' ;;
                malformed) echo '{}' ;;
                *) echo '{"status":"0x1","blockNumber":"0x20"}' ;;
            esac ;;
        *) echo "unexpected cast command: $*" >&2; return 8 ;;
    esac
}
curl() {
    # The old implementation ignored this response and claimed the proxy
    # existed. Keep this path so the regression test fails on that code.
    echo '{"error":{"message":"transaction rejected"}}'
}
'''
        with tempfile.TemporaryDirectory(prefix="eez-wave-proxy.") as temporary:
            root = Path(temporary)
            command = setup + script[start:end] + '''
proxy=$(create_l2_proxy 0x3333333333333333333333333333333333333333)
printf 'READY=%s\n' "$proxy"
'''
            result = subprocess.run(["bash", "-euo", "pipefail", "-c", command],
                                    text=True, capture_output=True, timeout=10,
                                    env={**os.environ, "SCENARIO": scenario,
                                         "CALLS": str(root / "calls"), "SENT": str(root / "sent")})
            return result, (root / "calls").read_text()

    def test_existing_proxy_needs_no_transaction_or_sleep(self):
        result, calls = self.run_setup("existing")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "READY=0x" + "1" * 40)
        self.assertNotIn("send ", calls)
        self.assertNotIn("sleep", calls)

    def test_new_proxy_uses_explicit_tip_and_checks_inclusion(self):
        result, calls = self.run_setup("created")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "READY=0x" + "1" * 40)
        self.assertIn("--priority-gas-price 1", calls)
        self.assertIn("--json", calls)
        self.assertIn("--timeout 30", calls)
        self.assertNotIn("sleep", calls)

    def test_setup_cannot_succeed_without_a_deployed_proxy(self):
        for scenario in ("rejected", "timeout", "reverted", "malformed", "no-code", "address-error", "code-error"):
            with self.subTest(scenario=scenario):
                result, _ = self.run_setup(scenario)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertNotIn("READY=", result.stdout)


if __name__ == "__main__":
    unittest.main()
