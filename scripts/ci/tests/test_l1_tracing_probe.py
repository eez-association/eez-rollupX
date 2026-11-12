"""Optional tracing diagnostic: real HTTP fixtures plus an owned Anvil backend."""
import copy
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import re
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import Mock, patch
import urllib.request

REPO = Path(__file__).resolve().parents[3]
PROBE = REPO / "testing/kurtosis/scripts/probe-l1-tracing.py"
TX, OTHER, BLOCK, TOPIC = ["0x" + c * 64 for c in "1234"]
ADDRESS = "0x" + "ab" * 20
OPTIONS = {"tracer": "callTracer", "tracerConfig": {"onlyTopCall": False, "withLog": True},
           "timeout": "4s"}


def fixture():
    log = dict(address=ADDRESS, topics=[TOPIC], data="0x002a", blockHash=BLOCK,
               transactionHash=TX, removed=False)
    frame_log = {key: log[key] for key in ("address", "topics", "data")}
    return {
        "eth_getTransactionReceipt": dict(status="0x1", transactionHash=TX, blockHash=BLOCK,
                                           blockNumber="0x1", transactionIndex="0x1", logs=[log]),
        "eth_getBlockByHash": dict(hash=BLOCK, number="0x1", transactions=[OTHER, TX]),
        "eth_getBlockByNumber": dict(hash=BLOCK, number="0x1", transactions=[OTHER, TX]),
        "debug_traceBlockByHash": [
            dict(txHash=OTHER, result=dict(type="CALL")),
            dict(txHash=TX, result=dict(type="CALL", calls=[dict(type="CALL", logs=[frame_log])])),
        ],
    }


class ProbeTests(unittest.TestCase):
    def run_probe(self, document, error_method=None, bad_http=False, oversized=False):
        calls = []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                method, params = request["method"], request["params"]
                calls.append((method, params))
                if bad_http and method == "debug_traceBlockByHash":
                    self.wfile.write(b"private endpoint response\r\n\r\n")
                    return
                reply = {"jsonrpc": "2.0", "id": request["id"]}
                if method == error_method:
                    reply["error"] = dict(code=-32601, message="private endpoint must not leak")
                else:
                    reply["result"] = document[method]
                data = (b" " * (32 * 1024 * 1024 + 1)
                        if oversized and method == "debug_traceBlockByHash"
                        else json.dumps(reply).encode())
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever)
            thread.start()
            try:
                result = subprocess.run([sys.executable, str(PROBE),
                                         f"http://127.0.0.1:{server.server_port}", TX],
                                        capture_output=True, text=True, timeout=15)
            finally:
                server.shutdown()
                thread.join()
        return result, calls

    def test_nonempty_nested_trace_matches_receipt_and_pins_hash(self):
        result, calls = self.run_probe(fixture())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["logs"], 1)
        self.assertEqual(calls, [
            ("eth_getTransactionReceipt", [TX]), ("eth_getBlockByHash", [BLOCK, False]),
            ("debug_traceBlockByHash", [BLOCK, OPTIONS]),
            ("eth_getBlockByNumber", ["0x1", False]),
        ])

    def test_log_content_multiplicity_and_rollback_are_checked(self):
        for change in ("missing", "duplicate", "data", "topic", "address", "reverted", "root revert"):
            with self.subTest(change=change):
                document = fixture()
                root = document["debug_traceBlockByHash"][1]["result"]
                child = root["calls"][0]
                if change == "missing":
                    child.pop("logs")
                elif change == "duplicate":
                    child["logs"] *= 2
                elif change in ("data", "topic", "address"):
                    child["logs"][0][{"topic": "topics"}.get(change, change)] = {
                        "data": "0x002b", "topic": ["0x" + "55" * 32], "address": "0x" + "cd" * 20,
                    }[change]
                else:
                    (root if change == "root revert" else child)["error"] = "execution reverted"
                result, calls = self.run_probe(document)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("logs differ from receipt", result.stderr)
                self.assertIn(("debug_traceBlockByHash", [BLOCK, OPTIONS]), calls)

    def test_reverted_sibling_logs_are_not_committed(self):
        document = fixture()
        root = document["debug_traceBlockByHash"][1]["result"]
        sibling = copy.deepcopy(root["calls"][0])
        sibling["error"] = "execution reverted"
        root["calls"].append(sibling)
        result, _ = self.run_probe(document)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_trace_coverage_identity_and_errors_reject(self):
        for change in ("empty", "missing tx", "wrong tx", "null hash", "trace error", "wrong tracer"):
            with self.subTest(change=change):
                document = fixture()
                traces = document["debug_traceBlockByHash"]
                if change == "empty":
                    traces.clear()
                elif change == "missing tx":
                    traces.pop()
                elif change == "wrong tx":
                    traces[1]["txHash"] = OTHER
                elif change == "null hash":
                    traces[1]["txHash"] = None
                elif change == "trace error":
                    traces[1]["error"] = "private timeout details"
                else:
                    traces[1]["result"] = {"structLogs": []}
                result, calls = self.run_probe(document)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(("debug_traceBlockByHash", [BLOCK, OPTIONS]), calls)
                self.assertNotIn("private", result.stderr)

    def test_missing_method_and_http_failure_do_not_leak(self):
        for failure in ({"error_method": "debug_traceBlockByHash"}, {"bad_http": True}):
            with self.subTest(failure=failure):
                result, calls = self.run_probe(fixture(), **failure)
                self.assertEqual(result.returncode, 1)
                self.assertIn("debug_traceBlockByHash RPC failed", result.stderr)
                self.assertNotIn("private", result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertEqual(calls[-1], ("debug_traceBlockByHash", [BLOCK, OPTIONS]))

    def test_oversized_trace_reports_probe_limit(self):
        result, calls = self.run_probe(fixture(), oversized=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("RPC response exceeds probe limit", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertEqual(calls[-1], ("debug_traceBlockByHash", [BLOCK, OPTIONS]))

    def test_reorg_after_trace_rejects(self):
        document = fixture()
        document["eth_getBlockByNumber"]["hash"] = OTHER
        result, calls = self.run_probe(document)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no longer canonical", result.stderr)
        self.assertEqual(calls[-1][0], "eth_getBlockByNumber")

    def test_empty_or_failed_receipts_are_not_trace_coverage(self):
        for change in ("logs", "status", "transactionHash", "transactionIndex", "blockHash"):
            with self.subTest(change=change):
                document = fixture()
                document["eth_getTransactionReceipt"][change] = {
                    "logs": [], "status": "0x0", "transactionHash": OTHER,
                    "transactionIndex": "0x0", "blockHash": OTHER,
                }[change]
                result, calls = self.run_probe(document)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("debug_traceBlockByHash", [method for method, _ in calls])

    def test_malformed_response_is_a_controlled_failure(self):
        document = fixture()
        document["debug_traceBlockByHash"][1]["result"]["calls"] = "invalid"
        result, _ = self.run_probe(document)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("Traceback", result.stderr)


@unittest.skipUnless(shutil.which("anvil"), "requires Anvil on PATH; CI pins 1.7.1")
class AnvilProbeTests(unittest.TestCase):
    def test_owned_backend_with_real_event_and_no_log_negative_control(self):
        # This process and all of its transactions belong only to this test.
        output = REPO / "artifacts/issue147"
        output.mkdir(parents=True, exist_ok=True)
        output = Path(tempfile.mkdtemp(prefix="anvil-", dir=output))
        with (output / "anvil.log").open("w") as log:
            node = subprocess.Popen(["anvil", "--host", "127.0.0.1", "--port", "0"],
                                    stdout=log, stderr=log)
            rpc_url = None

            def rpc(method, params):
                request = urllib.request.Request(rpc_url, data=json.dumps({
                    "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
                }).encode(), headers={"Content-Type": "application/json"})
                with urllib.request.urlopen(request, timeout=5) as response:
                    payload = json.load(response)
                self.assertNotIn("error", payload, payload)
                return payload["result"]

            def receipt(tx):
                deadline = time.monotonic() + 5
                while True:
                    result = rpc("eth_getTransactionReceipt", [tx])
                    if result is not None:
                        return result
                    self.assertLess(time.monotonic(), deadline, "owned Anvil did not mine fixture")
                    time.sleep(0.01)

            try:
                deadline = time.monotonic() + 10
                while True:
                    self.assertIsNone(node.poll(), f"owned Anvil failed to bind; see {output / 'anvil.log'}")
                    listener = re.search(r"Listening on (127\.0\.0\.1:[0-9]+)",
                                         (output / "anvil.log").read_text())
                    try:
                        if listener is None:
                            raise OSError("owned Anvil has not announced its listener")
                        rpc_url = "http://" + listener[1]
                        accounts = rpc("eth_accounts", [])
                        break
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(0.05)
                # Runtime emits LOG1 with topic=1 and a 32-byte value=42.
                runtime = "602a6000527f" + "00" * 31 + "01" + "60206000a100"
                size = len(bytes.fromhex(runtime))
                init = f"60{size:02x}600c60003960{size:02x}6000f3" + runtime
                deploy = rpc("eth_sendTransaction", [{"from": accounts[0], "data": "0x" + init}])
                contract = receipt(deploy)["contractAddress"]
                tx = rpc("eth_sendTransaction", [{"from": accounts[0], "to": contract}])
                receipt(tx)
                result = subprocess.run([sys.executable, str(PROBE), rpc_url, tx],
                                        capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["logs"], 1)
                (output / "probe.json").write_text(result.stdout)
                no_log = rpc("eth_sendTransaction", [{"from": accounts[0], "to": accounts[1]}])
                receipt(no_log)
                negative = subprocess.run([sys.executable, str(PROBE), rpc_url, no_log],
                                          capture_output=True, text=True, timeout=20)
                self.assertEqual(negative.returncode, 1)
                self.assertIn("nonempty receipt logs", negative.stderr)
            finally:
                node.terminate()
                try:
                    node.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    node.kill()
                    node.wait()


class AnvilLifecycleTests(unittest.TestCase):
    def test_concurrent_anvil_runs_use_distinct_logs(self):
        launch = threading.Barrier(2)
        logs = []

        def failed_launch(command, *, stdout, stderr):
            self.assertIs(stdout, stderr)
            logs.append(Path(stdout.name))
            launch.wait(timeout=5)
            raise OSError("intentional launch failure")

        def run():
            with self.assertRaisesRegex(OSError, "intentional launch failure"):
                AnvilProbeTests().test_owned_backend_with_real_event_and_no_log_negative_control()

        with patch.object(subprocess, "Popen", side_effect=failed_launch), \
                ThreadPoolExecutor(max_workers=2) as workers:
            futures = [workers.submit(run) for _ in range(2)]
            for future in futures:
                future.result(timeout=10)
        self.assertEqual(len(set(logs)), 2, "concurrent runs must not share a listener log")

    def test_slow_shutdown_kills_owned_process_and_preserves_failure(self):
        node = Mock()
        node.poll.return_value = None
        node.wait.side_effect = [subprocess.TimeoutExpired("owned-anvil", 10), None]
        with patch.object(subprocess, "Popen", return_value=node), \
                patch.object(Path, "read_text", side_effect=AssertionError("intentional fixture failure")), \
                self.assertRaisesRegex(AssertionError, "intentional fixture failure"):
            AnvilProbeTests().test_owned_backend_with_real_event_and_no_log_negative_control()
        node.terminate.assert_called_once()
        node.kill.assert_called_once()
        self.assertEqual(node.wait.call_count, 2, "forced shutdown must reap the owned process")
