#!/usr/bin/env python3
"""Read-only, opt-in block callTracer/log check against a known mined transaction.

Use an owned test backend and a successful transaction with retained logs/state.
This checks RPC evidence, not settlement semantics or independent execution.
"""
import argparse
from collections import Counter
import http.client
import json
import re
import sys
import urllib.request


class ProbeError(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise ProbeError(reason)


def hex_bytes(value, size=None):
    require(isinstance(value, str) and re.fullmatch(r"0x(?:[0-9a-fA-F]{2})*", value),
            "invalid hex bytes")
    require(size is None or len(value) == 2 + 2 * size, "invalid hex width")
    return value.lower()


def log_key(log):
    return (hex_bytes(log["address"], 20),
            tuple(hex_bytes(topic, 32) for topic in log["topics"]),
            hex_bytes(log["data"]))


def committed_logs(frame):
    require(isinstance(frame, dict) and isinstance(frame.get("type"), str),
            "not a callTracer frame")
    if frame.get("error") or frame.get("revertReason"):
        return Counter()
    logs, calls = frame.get("logs", []), frame.get("calls", [])
    require(isinstance(logs, list) and isinstance(calls, list), "invalid callTracer tree")
    result = Counter(log_key(log) for log in logs)
    for call in calls:
        result.update(committed_logs(call))
    return result


def probe(rpc, tx_hash):
    tx_hash = hex_bytes(tx_hash, 32)
    receipt = rpc("eth_getTransactionReceipt", [tx_hash])
    require(isinstance(receipt, dict) and receipt.get("status") == "0x1",
            "fixture needs a successful mined receipt")
    require(hex_bytes(receipt["transactionHash"], 32) == tx_hash, "receipt transaction mismatch")
    logs = receipt["logs"]
    require(isinstance(logs, list) and logs, "fixture needs nonempty receipt logs")
    block_hash = hex_bytes(receipt["blockHash"], 32)
    number = receipt["blockNumber"]
    block = rpc("eth_getBlockByHash", [block_hash, False])
    require(block["hash"].lower() == block_hash and block["number"] == number,
            "receipt block mismatch")
    transactions = [hex_bytes(tx, 32) for tx in block["transactions"]]
    index = int(receipt["transactionIndex"], 16)
    require(0 <= index < len(transactions) and transactions[index] == tx_hash,
            "receipt position mismatch")
    for log in logs:
        require(log.get("removed") is not True and log["blockHash"].lower() == block_hash
                and log["transactionHash"].lower() == tx_hash, "receipt log identity mismatch")
    traces = rpc("debug_traceBlockByHash", [block_hash, {
        "tracer": "callTracer", "tracerConfig": {"onlyTopCall": False, "withLog": True},
        "timeout": "4s",
    }])
    require(isinstance(traces, list) and len(traces) == len(transactions),
            "incomplete block traces")
    for tx, trace in zip(transactions, traces):
        require(isinstance(trace, dict) and trace.get("txHash", "").lower() == tx
                and trace.get("error") is None and "result" in trace,
                "inconsistent block trace identity or failure")
    require(committed_logs(traces[index]["result"]) == Counter(log_key(log) for log in logs),
            "committed callTracer logs differ from receipt")
    canonical = rpc("eth_getBlockByNumber", [number, False])
    require(isinstance(canonical, dict) and canonical["hash"].lower() == block_hash
            and canonical["number"] == number,
            "fixture block is no longer canonical")
    return {"verified": True, "transaction_hash": tx_hash, "block_hash": block_hash,
            "block_number": number, "transactions": len(transactions), "logs": len(logs)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rpc_url", help="regular node HTTP RPC, not a Composer front")
    parser.add_argument("transaction_hash", help="known successful transaction with nonempty logs")
    args = parser.parse_args()

    def rpc(method, params):
        request = urllib.request.Request(args.rpc_url, data=json.dumps({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
        }).encode(), headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                raw = response.read(32 * 1024 * 1024 + 1)
                require(len(raw) <= 32 * 1024 * 1024, "RPC response exceeds probe limit")
                payload = json.loads(raw)
            require(payload.get("error") is None and "result" in payload, f"{method} RPC failed")
            return payload["result"]
        except ProbeError:
            raise
        except (OSError, ValueError, http.client.HTTPException) as error:
            raise ProbeError(f"{method} RPC failed") from error

    try:
        print(json.dumps(probe(rpc, args.transaction_hash), sort_keys=True))
    except (ValueError, KeyError, TypeError, AttributeError, RecursionError) as error:
        # Never echo remote error text or the endpoint, which may contain credentials.
        print(f"L1 tracing probe failed: {error if isinstance(error, ProbeError) else 'malformed RPC evidence'}",
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
