#!/usr/bin/env python3
"""Wait for a wave's canonical receipts and destination state; never send/retry txs.

The JSON input comes from cross-chain-wave.sh. Reads use one L1 snapshot and an
L2 safe snapshot. Existing event, divergence and signer assertions still run
after this readiness barrier. Only Python's standard library is needed.
"""
import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request


class NotReady(Exception):
    pass


def quantity(value):
    if not isinstance(value, str) or not re.fullmatch(r"0x[0-9a-fA-F]+", value):
        raise NotReady("missing or invalid RPC quantity")
    return int(value, 16)


def block(value):
    if not isinstance(value, dict):
        raise NotReady("block unavailable")
    number = quantity(value.get("number"))
    digest = value.get("hash", "")
    if not re.fullmatch(r"0x[0-9a-fA-F]{64}", digest) or int(digest, 16) == 0:
        raise NotReady("block hash unavailable")
    return number, digest.lower()


class Rpc:
    def __init__(self, endpoints, deadline):
        self.endpoints = endpoints
        self.deadline = deadline

    def __call__(self, chain, method, params):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise NotReady("settlement deadline reached")
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        request = urllib.request.Request(self.endpoints[chain], body,
                                         {"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=min(5, remaining)) as response:
                result = json.load(response)
            if not isinstance(result, dict) or result.get("error") or result.get("id") != 1:
                raise NotReady(f"{chain} {method}: RPC result unavailable")
            return result.get("result")
        except (OSError, urllib.error.URLError, ValueError) as error:
            # An HTTP exception can include a credential-bearing URL/body.
            raise NotReady(f"{chain} {method}: RPC unavailable") from error


def settled_snapshot(config, rpc):
    heads = {chain: block(rpc(chain, "eth_getBlockByNumber", [tag, False]))
             for chain, tag in (("l1", "latest"), ("l2", "safe"))}
    commitment = rpc("l1", "eth_call", [
        {"to": config["registry"], "data": config["rollup_call"]}, hex(heads["l1"][0])])
    # rollups(uint64) returns (address, bytes32 blockHash, uint256).
    if not isinstance(commitment, str) or not re.fullmatch(r"0x[0-9a-fA-F]{192}", commitment):
        raise NotReady("L1 rollup commitment unavailable")
    if "0x" + commitment[66:130].lower() != heads["l2"][1]:
        raise NotReady("L1 commitment has not converged with the L2 safe block")

    blocks = {(chain, height): digest for chain, (height, digest) in heads.items()}
    for chain, hashes in config["receipts"].items():
        for tx_hash in hashes:
            receipt = rpc(chain, "eth_getTransactionReceipt", [tx_hash])
            if not isinstance(receipt, dict) or quantity(receipt.get("status")) != 1:
                raise NotReady(f"{chain} wave receipt missing or unsuccessful")
            if receipt.get("transactionHash", "").lower() != tx_hash.lower():
                raise NotReady(f"{chain} wave receipt identity mismatch")
            height = quantity(receipt.get("blockNumber"))
            if height > heads[chain][0]:
                raise NotReady(f"{chain} wave receipt is ahead of the settled snapshot")
            key = chain, height
            if key not in blocks:
                canonical_height, blocks[key] = block(rpc(chain, "eth_getBlockByNumber", [hex(height), False]))
                if canonical_height != height:
                    raise NotReady(f"{chain} canonical block height mismatch")
            if receipt.get("blockHash", "").lower() != blocks[key]:
                raise NotReady(f"{chain} wave receipt points to an orphaned block")

    for check in config["checks"]:
        chain = check["chain"]
        value = rpc(chain, check["method"], [check["argument"], hex(heads[chain][0])])
        if quantity(value) != int(check["expected"]):
            raise NotReady(f"{chain} destination state has not converged")

    # A reorg during the reads invalidates the whole snapshot. Advancing either
    # head is fine, provided the blocks just checked remain canonical.
    for chain, expected in heads.items():
        if block(rpc(chain, "eth_getBlockByNumber", [hex(expected[0]), False])) != expected:
            raise NotReady(f"{chain} snapshot changed during verification")
    return heads


def validate(config):
    for chain in ("l1", "l2"):
        if not isinstance(config["rpc"][chain], str) or not config["rpc"][chain].startswith(("http://", "https://")):
            raise ValueError("HTTP RPC endpoints are required")
        hashes = config["receipts"][chain]
        if not isinstance(hashes, list) or any(not re.fullmatch(r"0x[0-9a-fA-F]{64}", h) for h in hashes):
            raise ValueError("invalid wave transaction hashes")
    if set(config["receipts"]) != {"l1", "l2"} or not any(config["receipts"].values()):
        raise ValueError("wave receipts must not be empty")
    if not re.fullmatch(r"0x[0-9a-fA-F]{40}", config["registry"]):
        raise ValueError("invalid registry address")
    if not re.fullmatch(r"0x[0-9a-fA-F]{72}", config["rollup_call"]):
        raise ValueError("invalid rollups(uint64) calldata")
    if not config["checks"]:
        raise ValueError("destination state checks must not be empty")
    for check in config["checks"]:
        if check["chain"] not in ("l1", "l2") or check["method"] not in ("eth_call", "eth_getBalance"):
            raise ValueError("only read-only destination checks are permitted")
        if not re.fullmatch(r"[0-9]+", check["expected"]) or int(check["expected"]) >= 2**256:
            raise ValueError("invalid expected uint256 value")


def wait(config, timeout, interval=1):
    validate(config)
    deadline = time.monotonic() + timeout
    rpc = Rpc(config["rpc"], deadline)
    last_reason = "no snapshot observed"
    while time.monotonic() < deadline:
        try:
            heads = settled_snapshot(config, rpc)
            if time.monotonic() >= deadline:
                break
            print(f"Wave settled: L1 snapshot={heads['l1'][0]}, L2 safe={heads['l2'][0]}", flush=True)
            return
        except NotReady as error:
            reason = str(error)
            if reason != last_reason:
                print(f"Waiting for wave settlement: {reason}", flush=True)
            last_reason = reason
        time.sleep(max(0, min(interval, deadline - time.monotonic())))
    raise TimeoutError(f"Wave did not settle within {timeout:g}s: {last_reason}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=int, default=60)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    try:
        wait(json.load(sys.stdin), args.timeout)
    except (ValueError, KeyError, TypeError, TimeoutError) as error:
        # Configuration errors are fail-closed, not transient network retries.
        print(f"Wave settlement failed: {type(error).__name__}" if not isinstance(error, TimeoutError)
              else str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
