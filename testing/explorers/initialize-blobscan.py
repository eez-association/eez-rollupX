#!/usr/bin/env python3
"""Set the initial local indexing floor; never rewind an existing cursor."""
import argparse
import base64
import hashlib
import hmac
import json
from pathlib import Path
import time
import urllib.request


def request(url, payload=None, method=None, headers=None):
    req = urllib.request.Request(
        url,
        data=None if payload is None else json.dumps(payload).encode(),
        method=method,
        headers={"Content-Type": "application/json", **(headers or {})},
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        body = response.read()
        return json.loads(body) if body else None


def rpc(url, method, params):
    result = request(url, dict(jsonrpc="2.0", id=1, method=method, params=params))
    if "error" in result:
        raise RuntimeError(result["error"])
    return result["result"]


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file", type=Path, required=True)
    parser.add_argument("--start-block", type=int, default=23043686)
    parser.add_argument("--l1-rpc", default="http://127.0.0.1:18645")
    parser.add_argument("--beacon", default="http://127.0.0.1:6152")
    parser.add_argument("--api", default="http://127.0.0.1:18101")
    args = parser.parse_args()
    if int(rpc(args.l1_rpc, "eth_chainId", []), 16) != 10200:
        parser.error("This explorer profile requires Chiado chain 10200")
    state = request(args.api + "/blockchain-sync-state")
    if state["lastUpperSyncedSlot"] is not None:
        print("Keeping existing Blobscan cursor:", state["lastUpperSyncedSlot"])
        return
    if any(state.values()):
        parser.error("Partially initialized database; refusing to replace its cursor")
    block = rpc(args.l1_rpc, "eth_getBlockByNumber", [hex(args.start_block), False])
    genesis = request(args.beacon + "/eth/v1/beacon/genesis")["data"]
    spec = request(args.beacon + "/eth/v1/config/spec")["data"]
    elapsed = int(block["timestamp"], 16) - int(genesis["genesis_time"])
    slot, remainder = divmod(elapsed, int(spec["SECONDS_PER_SLOT"]))
    if remainder or slot < 1:
        parser.error("Deployment block does not correspond to a valid beacon slot")
    beacon_block = request(args.beacon + f"/eth/v2/beacon/blocks/{slot}")
    payload = beacon_block["data"]["message"]["body"]["execution_payload"]
    if payload["block_hash"].lower() != block["hash"].lower():
        parser.error("Execution and consensus endpoints disagree on the deployment block")
    values = dict(
        line.split("=", 1)
        for line in args.env_file.read_text().splitlines()
        if line and not line.startswith("#")
    )
    token_body = b".".join(
        [b64(b'{"alg":"HS512","typ":"JWT"}'),
         b64(json.dumps({"exp": int(time.time()) + 60}).encode())]
    )
    token = token_body + b"." + b64(hmac.digest(
        values["BLOBSCAN_SECRET_KEY"].encode(), token_body, hashlib.sha512
    ))
    # A traversal cursor immediately before the retained range. Leave the
    # lower-history checkpoint unset: earlier Chiado history is not indexed.
    request(
        args.api + "/blockchain-sync-state",
        {"lastUpperSyncedSlot": slot - 1},
        method="PUT",
        headers={"Authorization": "Bearer " + token.decode()},
    )
    result = {
        "chainId": 10200,
        "startBlock": args.start_block,
        "startBlockHash": block["hash"],
        "startSlot": slot,
        "initialCursor": slot - 1,
        "coverage": "All Chiado blobs from the EEZ deployment onward",
    }
    (args.env_file.parent / "indexing-origin.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
