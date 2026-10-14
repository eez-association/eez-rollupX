#!/usr/bin/env python3
"""Minimal eth_sendBundle stub for tests / devnet.

Real Flashbots-style relays accept bundles and try to include them in a
specific L1 block. They don't consume the poster's nonce on miss.

By default this stub fakes a relay: it accepts single-transaction
`eth_sendBundle` requests and forwards the signed tx via
`eth_sendRawTransaction` to a backing Anvil. Bundles containing zero or
multiple transactions are rejected explicitly because forwarding them
individually would not preserve bundle atomicity. The nonce IS consumed on
miss (Anvil doesn't replay-protect), but the Composer's cursor-race guard +
pending-nonce read keeps it correct.

With `--passthrough-bundles`, the upstream must implement `eth_sendBundle`;
the request is forwarded with the upstream's execution semantics. That native
test endpoint can accept a prefix before returning an error. Pooled blob
envelopes are therefore captured before forwarding; the Beacon fixture exposes
only captures with canonical receipts. Suitable for tests; not for production.

Usage:
    builder-stub.py --listen 127.0.0.1:9001 --upstream http://127.0.0.1:8545
"""
import argparse
import hashlib
import http.server
import json
import os
import socketserver
import tempfile
import threading
import urllib.request
import time


class Handler(http.server.BaseHTTPRequestHandler):
    upstream = ""
    capture_dir = None
    passthrough_bundles = False
    test_control_dir = None
    control_lock = threading.Lock()

    def log_message(self, *_):
        pass

    def _wait_for_block(self, target):
        if target <= 0:
            return
        import time
        for _ in range(60):
            try:
                resp = urllib.request.urlopen(
                    urllib.request.Request(
                        self.upstream,
                        data=json.dumps({
                            "jsonrpc": "2.0",
                            "id": 0,
                            "method": "eth_blockNumber",
                            "params": [],
                        }).encode(),
                        headers={"Content-Type": "application/json"},
                    ),
                    timeout=2,
                ).read()
                bn = int(json.loads(resp).get("result", "0x0"), 16)
                if bn >= target:
                    return
            except Exception:
                pass
            time.sleep(0.1)

    def _capture_blob(self, raw, tx_hash=None):
        if self.capture_dir is None or not raw.startswith("0x03"):
            return
        raw_bytes = bytes.fromhex(raw[2:])
        capture_key = hashlib.sha256(raw_bytes).hexdigest()
        destination = os.path.join(self.capture_dir, f"blob-{capture_key}.json")
        if os.path.exists(destination):
            return
        capture = {"raw": raw}
        if tx_hash is not None:
            capture["transactionHash"] = tx_hash
        fd, temporary = tempfile.mkstemp(
            dir=self.capture_dir,
            prefix="blob-",
            suffix=".tmp",
        )
        try:
            with os.fdopen(fd, "w") as output:
                json.dump(capture, output)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, destination)
        except Exception:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass
            raise

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        except Exception:
            self.send_error(400)
            return

        errors = (os.path.join(self.test_control_dir, "rpc-errors.json")
                  if self.test_control_dir else None)
        failed_methods = []
        pruned_before = 0
        if errors:
            try:
                with open(errors) as source:
                    failed_methods = json.load(source)
            except FileNotFoundError:
                pass
            try:
                with open(os.path.join(self.test_control_dir, "pruned-before.json")) as source:
                    pruned_before = int(json.load(source))
            except FileNotFoundError:
                pass
        # Model Reth's historical bodies/receipts boundary: full-block reads
        # and even empty address-filtered log queries fail below it, while
        # debug_getRawHeader still serves the retained canonical header.
        pruned_history = False
        if pruned_before:
            start = "latest"
            if body.get("method") == "eth_getLogs":
                start = body["params"][0].get("fromBlock", "latest")
            elif body.get("method") in ("eth_getBlockByNumber", "eth_getHeaderByNumber"):
                start = body["params"][0]
            pruned_history = start == "earliest" or (
                start.startswith("0x") and int(start, 16) < pruned_before)
        if body.get("method") in failed_methods:
            resp = json.dumps({
                "jsonrpc": "2.0", "id": body.get("id"),
                "error": {"code": -32000, "message": "injected temporary RPC failure"},
            }).encode()
        elif pruned_history:
            resp = json.dumps({
                "jsonrpc": "2.0", "id": body.get("id"),
                "error": {"code": 4444, "message": "pruned history unavailable"},
            }).encode()
        elif body.get("method") == "eth_sendBundle":
            params = body["params"][0]
            txs = params.get("txs", [])
            if self.passthrough_bundles:
                # A restart barrier must also stop later recompositions after
                # the first held request's transport deadline has expired.
                pause_path = (os.path.join(self.test_control_dir, "pause-after.json")
                              if self.test_control_dir else None)
                paused = False
                if pause_path:
                    with self.control_lock:
                        try:
                            with open(pause_path) as source:
                                skip = json.load(source)["skip"]
                            if not isinstance(skip, int) or skip < 0:
                                raise ValueError("pause skip must be a non-negative integer")
                            if skip:
                                with open(pause_path + ".new", "w") as output:
                                    json.dump({"skip": skip - 1}, output)
                                os.replace(pause_path + ".new", pause_path)
                            else:
                                paused = True
                                captured = os.path.join(self.test_control_dir, "paused-bundle.json")
                                if not os.path.exists(captured):
                                    with open(captured + ".new", "w") as output:
                                        json.dump(body, output)
                                    os.replace(captured + ".new", captured)
                        except FileNotFoundError:
                            pass
                if paused:
                    resp = json.dumps({
                        "jsonrpc": "2.0", "id": body.get("id"),
                        "error": {"code": -32000, "message": "test relay paused"},
                    }).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(resp)))
                    self.end_headers()
                    self.wfile.write(resp)
                    return
                drop_path = (os.path.join(self.test_control_dir, "drop-next.json")
                             if self.test_control_dir else None)
                drop_next = False
                if drop_path and os.path.exists(drop_path):
                    try:
                        os.replace(drop_path, drop_path + ".used")
                        with open(drop_path + ".used") as source:
                            remaining = json.load(source).get("remaining", 1)
                        if remaining > 1:
                            with open(drop_path + ".new", "w") as output:
                                json.dump({"remaining": remaining - 1}, output)
                            os.replace(drop_path + ".new", drop_path)
                        drop_next = True
                    except FileNotFoundError:
                        pass
                if drop_next:
                    # A relay acknowledgment is not proof of L1 inclusion.
                    captured = os.path.join(self.test_control_dir, "dropped-bundle.json")
                    with open(captured + ".new", "w") as output:
                        json.dump(body, output)
                    os.replace(captured + ".new", captured)
                    resp = json.dumps({
                        "jsonrpc": "2.0", "id": body.get("id"),
                        "result": {"bundleHash": "0x" + "00" * 32},
                    }).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(resp)))
                    self.end_headers()
                    self.wfile.write(resp)
                    return
                hold_path = (os.path.join(self.test_control_dir, "hold-next.json")
                             if self.test_control_dir else None)
                hold_next = False
                if hold_path:
                    try:
                        with open(hold_path) as source:
                            hold = json.load(source)
                        if hold.get("transaction") is None or hold["transaction"] in txs:
                            # `skip` lets that many matching bundles through
                            # first, so a test can arm the hold before the
                            # bundle it targets is composed.
                            skip = hold.get("skip", 0)
                            if not isinstance(skip, int) or skip < 0:
                                raise ValueError("hold skip must be a non-negative integer")
                            if skip:
                                with open(hold_path + ".new", "w") as output:
                                    json.dump(dict(hold, skip=skip - 1), output)
                                os.replace(hold_path + ".new", hold_path)
                            else:
                                os.replace(hold_path, hold_path + ".used")
                                hold_next = True
                    except FileNotFoundError:
                        # Another request may have consumed this one-shot control.
                        pass
                if hold_next:
                    # A bounded, test-only barrier after proving. The test
                    # supplies valid signed L1 transactions around the exact
                    # original postBatch; this never alters its proof or DA.
                    captured = os.path.join(self.test_control_dir, "held-bundle.json")
                    with open(captured + ".new", "w") as output:
                        json.dump(body, output)
                    os.replace(captured + ".new", captured)
                    released = os.path.join(self.test_control_dir, "released-bundle.json")
                    # Leave time for restart tests to persist the already
                    # settled L1 prefix while the next proposal stays unposted.
                    deadline = time.monotonic() + 60
                    while not os.path.exists(released):
                        if time.monotonic() >= deadline:
                            raise TimeoutError("test did not release held bundle")
                        time.sleep(0.01)
                    with open(released) as source:
                        body = json.load(source)
                    params = body["params"][0]
                    txs = params["txs"]
                fault_path = (os.path.join(self.test_control_dir, "omit-user.json")
                              if self.test_control_dir else None)
                # Native integration fault, after composition/proving. This
                # deliberately controls L1 inclusion, not builder semantics.
                # Production request generation is tested against rbuilder.
                if fault_path and os.path.exists(fault_path) and len(txs) == 4:
                    with open(fault_path) as fault_file:
                        fault = json.load(fault_file)
                    omitted = fault["userIndex"]
                    if not isinstance(omitted, int) or not 0 <= omitted < 3:
                        raise ValueError("test omission index must be 0, 1, or 2")
                    os.replace(fault_path, fault_path + ".used")
                    original = json.loads(json.dumps(body))
                    if fault.get("holdRetry", False):
                        # Arm before forwarding the omission. Maintenance-only
                        # posts must not consume the restart test's retry barrier.
                        with open(hold_path + ".new", "w") as output:
                            json.dump({"transaction": txs[omitted + 1]}, output)
                        os.replace(hold_path + ".new", hold_path)
                    params["txs"] = [raw for index, raw in enumerate(txs)
                                     if index != omitted + 1]
                    # The embedded dev endpoint does not interpret allowlists.
                    # Retain the original request for exact membership checks.
                    record = {"original": original, "forwarded": body,
                              "omittedUser": omitted}
                    destination = os.path.join(self.test_control_dir, "omission.json")
                    with open(destination + ".new", "w") as output:
                        json.dump(record, output)
                        output.flush()
                        os.fsync(output.fileno())
                    os.replace(destination + ".new", destination)
                # The native dev endpoint is intentionally non-atomic: a
                # prefix may enter its pool even when a later companion fails
                # or the HTTP response is lost. Retain blob carriers before
                # forwarding; the Beacon fixture exposes canonical receipts only.
                for raw in txs:
                    self._capture_blob(raw)
                single = (self.test_control_dir and os.path.exists(
                    os.path.join(self.test_control_dir, "single-transaction.json")))
                # The controlled-consensus native fixture uses a standard EL
                # Engine API. Keep the same proof/hold/Beacon barriers, but use
                # the existing single-transaction relay semantics on that EL.
                if single and len(txs) != 1:
                    resp = json.dumps({"jsonrpc": "2.0", "id": body.get("id"),
                        "error": {"code": -32602, "message": "controlled native relay requires exactly one transaction"}}).encode()
                else:
                    forwarded = {"jsonrpc": "2.0", "id": body.get("id"),
                        "method": "eth_sendRawTransaction", "params": txs} if single else body
                    resp = urllib.request.urlopen(
                        urllib.request.Request(
                            self.upstream,
                            data=json.dumps(forwarded).encode(),
                            headers={"Content-Type": "application/json"},
                        ),
                        timeout=10,
                    ).read()
            elif len(txs) != 1:
                resp = json.dumps({
                    "jsonrpc": "2.0",
                    "id": body.get("id"),
                    "error": {
                        "code": -32602,
                        "message": "builder stub accepts exactly one transaction per bundle",
                        "data": {"transactionCount": len(txs)},
                    },
                }).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(resp)))
                self.end_headers()
                self.wfile.write(resp)
                return
            else:
                raw = txs[0]
                target_hex = params.get("blockNumber", "0x0")
                target = int(target_hex, 16) if isinstance(target_hex, str) else int(target_hex)

                # Wait until anvil's tip is one short of `target` so the
                # forwarded tx lands *in* `target` (anvil mines a block as
                # soon as a tx arrives at >= block_time cadence).
                self._wait_for_block(target - 1)

                forwarded = {
                    "jsonrpc": "2.0",
                    "id": body.get("id"),
                    "method": "eth_sendRawTransaction",
                    "params": [raw],
                }
                resp = urllib.request.urlopen(
                    urllib.request.Request(
                        self.upstream,
                        data=json.dumps(forwarded).encode(),
                        headers={"Content-Type": "application/json"},
                    ),
                    timeout=10,
                ).read()
                forwarded_result = json.loads(resp)
                tx_hash = forwarded_result.get("result")
                if isinstance(tx_hash, str):
                    self._capture_blob(raw, tx_hash)
        else:
            resp = urllib.request.urlopen(
                urllib.request.Request(
                    self.upstream,
                    data=json.dumps(body).encode(),
                    headers={"Content-Type": "application/json"},
                ),
                timeout=10,
            ).read()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(resp)))
        self.end_headers()
        self.wfile.write(resp)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--listen", required=True, help="host:port")
    p.add_argument("--upstream", required=True, help="anvil RPC url")
    p.add_argument(
        "--capture-dir",
        help="atomically capture accepted pooled blob transactions for test fixtures",
    )
    p.add_argument(
        "--passthrough-bundles",
        action="store_true",
        help="forward eth_sendBundle unchanged instead of adapting it for Anvil",
    )
    p.add_argument("--test-control-dir", help="native integration fault controls (test-only)")
    args = p.parse_args()
    host, port = args.listen.split(":")
    Handler.upstream = args.upstream
    Handler.capture_dir = args.capture_dir
    Handler.passthrough_bundles = args.passthrough_bundles
    Handler.test_control_dir = args.test_control_dir
    if Handler.capture_dir is not None:
        os.makedirs(Handler.capture_dir, exist_ok=True)

    class Reusable(socketserver.ThreadingTCPServer):
        allow_reuse_address = True

    with Reusable((host, int(port)), Handler) as srv:
        srv.serve_forever()


if __name__ == "__main__":
    main()
