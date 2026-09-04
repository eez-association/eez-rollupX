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
the request is forwarded unchanged so its atomic semantics are preserved.
Either mode can capture accepted pooled blob envelopes for the Beacon test
fixture. Suitable for tests; not for production.

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
import urllib.request


class Handler(http.server.BaseHTTPRequestHandler):
    upstream = ""
    capture_dir = None
    passthrough_bundles = False

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

        if body.get("method") == "eth_sendBundle":
            params = body["params"][0]
            txs = params.get("txs", [])
            if self.passthrough_bundles:
                resp = urllib.request.urlopen(
                    urllib.request.Request(
                        self.upstream,
                        data=json.dumps(body).encode(),
                        headers={"Content-Type": "application/json"},
                    ),
                    timeout=10,
                ).read()
                forwarded_result = json.loads(resp)
                if "error" not in forwarded_result:
                    for raw in txs:
                        self._capture_blob(raw)
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
    args = p.parse_args()
    host, port = args.listen.split(":")
    Handler.upstream = args.upstream
    Handler.capture_dir = args.capture_dir
    Handler.passthrough_bundles = args.passthrough_bundles
    if Handler.capture_dir is not None:
        os.makedirs(Handler.capture_dir, exist_ok=True)

    class Reusable(socketserver.ThreadingTCPServer):
        allow_reuse_address = True

    with Reusable((host, int(port)), Handler) as srv:
        srv.serve_forever()


if __name__ == "__main__":
    main()
