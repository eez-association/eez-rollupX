"""Run the shared submission helper against isolated HTTP/transport fixtures."""
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = Path(__file__).resolve().parents[3]
KEY = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"


@unittest.skipUnless(all(shutil.which(cmd) for cmd in ("curl", "jq", "cast")),
                     "curl, jq and cast required")
class FrontSubmissionTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw = subprocess.check_output([
            "cast", "mktx", "0x1111111111111111111111111111111111111111",
            "--private-key", KEY, "--chain", "7331", "--nonce", "7",
            "--gas-limit", "600000", "--gas-price", "100", "--priority-gas-price", "1",
        ], text=True).strip()
        cls.hash = subprocess.check_output(["cast", "keccak", cls.raw], text=True).strip()

    def run_front(self, sends, observations=(), refused=False, tick=60, repeat=1,
                  curl_config=None, parser_tick=0, real_clock=False, expire_before_curl=False):
        requests = []
        accepted = []
        sends, observations = list(sends), list(observations)
        expected = self.hash

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                requests.append(request)
                if request["method"] == "eth_sendRawTransaction":
                    action = sends.pop(0) if len(sends) > 1 else sends[0]
                    if action in ("accept_drop", "accept_reset", "accept_empty", "accept_partial",
                                  "accept_stall", "success"):
                        if request["params"][0] not in accepted:
                            accepted.append(request["params"][0])
                    if action in ("drop", "accept_drop", "reset", "accept_reset"):
                        if action.endswith("reset"):
                            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER,
                                                       struct.pack("ii", 1, 0))
                        self.close_connection = True
                        return
                    if action == "accept_stall":
                        time.sleep(2)
                        return
                    if action in ("empty", "accept_empty"):
                        self.send_response(200)
                        self.send_header("Content-Length", "0")
                        self.end_headers()
                        return
                    if action == "accept_partial":
                        self.send_response(200)
                        self.send_header("Content-Length", "100")
                        self.end_headers()
                        self.wfile.write(b"{")
                        self.close_connection = True
                        return
                    response = {"result": expected} if action == "success" else action
                else:
                    action = observations.pop(0) if len(observations) > 1 else (
                        observations[0] if observations else None)
                    if action == "owned":
                        action = {"hash": expected, "lifecycle": "queued", "ownership": None}
                    if action == "stall":
                        time.sleep(2)
                        return
                    response = action if isinstance(action, dict) and "error" in action else {"result": action}
                body = action if isinstance(action, bytes) else json.dumps(
                    {"jsonrpc": "2.0", "id": 1, **response}).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        with tempfile.TemporaryDirectory(prefix="eez-front-submission.") as tmp:
            root = Path(tmp)
            if curl_config is not None:
                (root / ".curlrc").write_text(curl_config)
            server = ThreadingHTTPServer(("127.0.0.1", 0), Handler, bind_and_activate=False)
            server.server_bind()  # Reserve an OS-assigned port even during refusal.
            if not refused:
                server.server_activate()
                (root / "listening").touch()
            stopped = threading.Event()

            def serve():
                if refused:
                    while not (root / "retry").exists():
                        if stopped.wait(0.01):
                            return
                    server.server_activate()
                    (root / "listening").touch()
                server.serve_forever(poll_interval=0.01)

            thread = threading.Thread(target=serve, daemon=True)
            thread.start()
            clock_setup = "SECONDS=0" if real_clock else "unset SECONDS\nSECONDS=0"
            script = '''set -euo pipefail
CLOCK_SETUP
# Advance only the retry clock; use a handshake for the refused listener.
sleep() {
    SECONDS=$((SECONDS + TICK))
    touch "$TRACE/retry"
    while [[ ! -e "$TRACE/listening" ]]; do command sleep 0.01; done
}
curl() { printf '%s\\n' "$*" >> "$TRACE/curl"; command curl "$@"; }
jq() {
    local rc=0
    command jq "$@" || rc=$?
    SECONDS=$((SECONDS + PARSER_TICK))
    return "$rc"
}
source "$REPO/testing/kurtosis/scripts/lib.sh"
if (( EXPIRE_BEFORE_CURL )); then
    set -T
    trap 'if [[ $BASH_COMMAND == left=* ]]; then SECONDS=$deadline; fi' DEBUG
fi
for ((i=0; i<REPEAT; i++)); do send_front "$FRONT" "$RAW" "$HASH"; done
echo ADMITTED
'''.replace("CLOCK_SETUP", clock_setup)
            try:
                result = subprocess.run(["bash", "-c", script], env={
                    **os.environ, "REPO": str(REPO), "RAW": self.raw, "HASH": expected,
                    "TRACE": tmp, "TICK": str(tick),
                    "REPEAT": str(repeat), "CURL_HOME": tmp, "PARSER_TICK": str(parser_tick),
                    "EXPIRE_BEFORE_CURL": str(int(expire_before_curl)),
                    "FRONT": f"http://127.0.0.1:{server.server_address[1]}",
                }, text=True, capture_output=True, timeout=15)
                trace = (root / "curl").read_text() if (root / "curl").exists() else ""
            finally:
                stopped.set()
                if not refused or (root / "retry").exists():
                    server.shutdown()
                thread.join(timeout=3)
                server.server_close()
        self.assertTrue(all(request["params"] == [self.raw] if
                            request["method"] == "eth_sendRawTransaction" else
                            request["method"] == "eez_getCrossChainTransaction" and
                            request["params"] == [expected] for request in requests))
        return result, requests, accepted, trace

    def assert_admitted(self, result, success=True):
        self.assertEqual(result.returncode == 0, success, result.stderr)
        self.assertEqual("ADMITTED" in result.stdout, success)

    def test_refused_connection_retries_same_envelope(self):
        result, requests, accepted, _ = self.run_front(["success"], refused=True)
        self.assert_admitted(result)
        self.assertEqual(len(requests), 1)
        self.assertEqual(accepted, [self.raw])

    def test_lost_response_reconciles_without_duplicate_submission(self):
        for action in ("accept_drop", "accept_reset", "accept_empty", "accept_partial"):
            with self.subTest(action=action):
                result, requests, accepted, _ = self.run_front([action], ["owned"])
                self.assert_admitted(result)
                self.assertEqual([r["method"] for r in requests],
                                 ["eth_sendRawTransaction", "eez_getCrossChainTransaction"])
                self.assertEqual(accepted, [self.raw])

    def test_absence_does_not_authorize_resubmission_or_success(self):
        for action in ("drop", "reset", "empty"):
            with self.subTest(action=action):
                result, requests, accepted, _ = self.run_front([action])
                self.assert_admitted(result, False)
                self.assertIn("unresolved", result.stderr)
                self.assertEqual(sum(r["method"] == "eth_sendRawTransaction" for r in requests), 1)
                self.assertFalse(accepted)

    def test_delayed_ownership_is_reconciled(self):
        result, requests, accepted, _ = self.run_front(["accept_drop"], [None, "owned"])
        self.assert_admitted(result)
        self.assertEqual(len(requests), 3)
        self.assertEqual(accepted, [self.raw])

    def test_recognized_startup_and_temporary_rpc_refusals_retry(self):
        for error in (
            {"code": -32000, "message": "eez node is starting up; cross-chain submissions are not accepted yet"},
            {"code": -32003, "data": {"code": "quote_busy"}},
        ):
            with self.subTest(error=error):
                result, requests, accepted, _ = self.run_front([{"error": error}, "success"])
                self.assert_admitted(result)
                self.assertEqual(len(requests), 2)
                self.assertEqual(accepted, [self.raw])

    def test_terminal_failure_wrong_hash_and_invalid_status_are_not_success(self):
        for status in (
            *({"hash": self.hash, "lifecycle": "terminal", "ownership": {"reason": reason}}
              for reason in ("SimulationFailed", "PoolDiscarded", "SourceReverted",
                             "SourceCompletedWithMissingEffects", "SourceNonceSpent",
                             "AttemptsExhausted", "NonceGap")),
            {"hash": "0x" + "ff" * 32, "lifecycle": "queued"},
            {"hash": self.hash, "lifecycle": "unknown"},
            {"error": {"code": -32601, "message": "method not found"}},
        ):
            with self.subTest(status=status):
                result, requests, _, _ = self.run_front(["accept_drop"], [status])
                self.assert_admitted(result, False)
                self.assertEqual(sum(r["method"] == "eth_sendRawTransaction" for r in requests), 1)
                if status.get("lifecycle") == "terminal":
                    self.assertEqual(len(requests), 2)

    def test_active_and_committed_ownership_confirm_admission(self):
        for lifecycle in ("queued", "in_flight", "composing", "awaiting_evidence", "terminal"):
            with self.subTest(lifecycle=lifecycle):
                ownership = {"state": "terminal", "reason": "Committed"} if lifecycle == "terminal" else None
                status = {"hash": self.hash.upper(), "lifecycle": lifecycle, "ownership": ownership}
                result, _, _, _ = self.run_front(["accept_reset"], [status])
                self.assert_admitted(result)

    def test_temporary_refusal_and_reconciliation_share_deadline(self):
        busy = {"error": {"code": -32003, "data": {"code": "quote_busy"}}}
        result, requests, _, trace = self.run_front([busy, "accept_drop"], [None])
        self.assert_admitted(result, False)
        self.assertEqual(len(requests), 3)
        self.assertIn("unresolved", result.stderr)
        self.assertIn("--max-time 10", trace)

    def test_remaining_deadline_caps_blocked_observation(self):
        busy = {"error": {"code": -32003, "data": {"code": "quote_busy"}}}
        started = time.monotonic()
        result, requests, _, trace = self.run_front([busy, "accept_drop"], ["stall"], tick=119)
        self.assert_admitted(result, False)
        self.assertIn("--max-time 1", trace)
        self.assertLess(time.monotonic() - started, 5)
        self.assertEqual(len(requests), 3)

    def test_timed_out_send_can_reconcile_accepted_identity(self):
        # Frozen time isolates rc=28 handling; the real-clock test covers expiry.
        busy = {"error": {"code": -32003, "data": {"code": "quote_busy"}}}
        result, requests, accepted, trace = self.run_front(
            [busy, "accept_stall"], ["owned"], tick=119)
        self.assert_admitted(result)
        self.assertIn("curl rc=28", result.stderr)
        self.assertIn("--max-time 1", trace)
        self.assertEqual(len(requests), 3)
        self.assertEqual(accepted, [self.raw])

    def test_duplicate_hash_acknowledgement_is_success_but_error_is_not(self):
        result, requests, accepted, _ = self.run_front(["success"], repeat=2)
        self.assert_admitted(result)
        self.assertEqual(len(requests), 2)
        self.assertEqual(accepted, [self.raw])
        result, requests, _, _ = self.run_front([
            {"error": {"code": -32000, "message": "already known"}}])
        self.assert_admitted(result, False)
        self.assertEqual(len(requests), 1)

    def test_unrecognized_startup_text_is_terminal(self):
        result, requests, _, _ = self.run_front([
            {"error": {"code": -32602, "message": "invalid field: starting up"}}])
        self.assert_admitted(result, False)
        self.assertEqual(len(requests), 1)

    def test_source_completed_confirms_admission_without_resubmitting(self):
        status = {"hash": self.hash, "lifecycle": "terminal",
                  "ownership": {"state": "terminal", "reason": "SourceCompleted"}}
        result, requests, accepted, _ = self.run_front(["accept_drop"], [status])
        self.assert_admitted(result)
        self.assertEqual(len(requests), 2)
        self.assertEqual(accepted, [self.raw])

    def test_real_clock_final_send_timeout_reports_unresolved_acceptance(self):
        busy = {"error": {"code": -32003, "data": {"code": "quote_busy"}}}
        result, requests, accepted, _ = self.run_front(
            [busy, "accept_stall"], ["owned"], tick=119, real_clock=True)
        self.assert_admitted(result, False)
        self.assertIn("acceptance unresolved", result.stderr)
        self.assertEqual([r["method"] for r in requests], ["eth_sendRawTransaction"] * 2)
        self.assertEqual(accepted, [self.raw])

    def test_deadline_crossing_before_timeout_calculation_makes_no_request(self):
        result, requests, _, trace = self.run_front(["success"], expire_before_curl=True)
        self.assert_admitted(result, False)
        self.assertIn("deadline", result.stderr)
        self.assertEqual(requests, [])
        self.assertEqual(trace, "")

    def test_curl_config_cannot_resubmit_after_lost_reply(self):
        result, requests, accepted, _ = self.run_front(
            ["accept_drop", "success"], ["owned"],
            curl_config="retry = 1\nretry-all-errors\nretry-delay = 1\n")
        self.assert_admitted(result)
        self.assertEqual([r["method"] for r in requests],
                         ["eth_sendRawTransaction", "eez_getCrossChainTransaction"])
        self.assertEqual(accepted, [self.raw])

    def test_submission_requires_complete_exact_hash_reply(self):
        valid = json.dumps({"result": self.hash})
        for body in (valid + " BROKEN", json.dumps({"result": self.hash + "\n"}),
                     valid + "\n" + valid, valid + "\n" + json.dumps({"error": {
                         "code": -32003, "data": {"code": "quote_busy"}}}),
                     json.dumps({"result": self.hash, "error": {
                         "code": -32003, "data": {"code": "quote_busy"}}})):
            with self.subTest(body=body):
                result, requests, _, _ = self.run_front([body.encode(), "success"])
                self.assert_admitted(result, False)
                self.assertEqual(len(requests), 1)

    def test_reconciliation_requires_one_complete_status_object(self):
        valid = json.dumps({"result": {"hash": self.hash, "lifecycle": "queued"}})
        for body in (valid + "\n" + valid, "null\n" + valid, valid + " BROKEN"):
            with self.subTest(body=body):
                result, requests, _, _ = self.run_front(["accept_drop"], [body.encode()])
                self.assert_admitted(result, False)
                self.assertIn("unresolved", result.stderr)
                self.assertEqual(sum(r["method"] == "eth_sendRawTransaction" for r in requests), 1)

    def test_parsing_cannot_confirm_admission_after_deadline(self):
        for send, observations in (("success", []), ("accept_drop", ["owned"])):
            with self.subTest(send=send):
                result, _, _, _ = self.run_front([send], observations, parser_tick=120)
                self.assert_admitted(result, False)
                self.assertIn("deadline", result.stderr)


if __name__ == "__main__":
    unittest.main()
