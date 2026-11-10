#!/usr/bin/env python3
"""Select scenario table transactions from canonical L2 blocks, not summary hints.

The upstream summary reports the first table load and all calls in a shared
block. Select by runtime entry hash (proxyEntryHash || rollingHash) instead.
This is discovery only: callers must revalidate receipts and pass the COMPLETE
selected calldata/receipts to the independent causal and blob verifiers.
"""
import argparse
from collections import Counter
import json
from pathlib import Path
import re
import subprocess
import sys


def cast(*args):
    return subprocess.check_output(["cast", *args], text=True, timeout=30).strip()


def hex_value(value, size):
    if not isinstance(value, str) or not re.fullmatch(r"0x[0-9a-fA-F]{%d}" % (size * 2), value):
        raise ValueError(f"expected {size}-byte hex value")
    return value.lower()


def quantity(value):
    if not ((type(value) is int) or (isinstance(value, str) and re.fullmatch(r"(?:0x[0-9a-fA-F]+|[0-9]+)", value))):
        raise ValueError("invalid block/index quantity")
    number = int(value, 16) if isinstance(value, str) and value.startswith("0x") else int(value)
    if number < 0:
        raise ValueError("negative block/index quantity")
    return number


def select_transactions(blocks, manager, expected, methods, run_cast=cast, users=None):
    manager = hex_value(manager, 20)
    if not isinstance(blocks, list) or not isinstance(expected, list):
        raise ValueError("expected arrays of canonical blocks and entry hashes")
    remaining = Counter(hex_value(value, 32) for value in expected)
    if users is None:
        users = []
    if not isinstance(users, list):
        raise ValueError("expected an array of user transaction hashes")
    users = [hex_value(value, 32) for value in users]
    static_only = not remaining
    if static_only and len(users) != 1:
        raise ValueError("static-only selection requires one user transaction")
    matched_users = set()
    signatures = {}
    for name in ("loadExecutionTable", "executeIncomingCrossChainCall"):
        matches = [(sig, selector) for sig, selector in methods.items() if sig.startswith(name + "(")]
        if len(matches) != 1:
            raise ValueError(f"expected one pinned ABI signature for {name}")
        sig, selector = matches[0]
        signatures[hex_value("0x" + selector, 4)] = sig
    selected, seen_blocks, seen_transactions = [], set(), set()
    loaded_keys, selected_proxies = [], set()
    for block in sorted(blocks, key=lambda b: quantity(b["number"])):
        height = quantity(block["number"])
        block_hash = hex_value(block["hash"], 32)
        if height in seen_blocks:
            raise ValueError("duplicate canonical block")
        seen_blocks.add(height)
        latest_table = None
        for index, tx in enumerate(block["transactions"]):
            tx_hash = hex_value(tx["hash"], 32)
            if tx_hash in seen_transactions:
                raise ValueError("duplicate transaction in canonical blocks")
            seen_transactions.add(tx_hash)
            if (hex_value(tx["blockHash"], 32) != block_hash or
                    quantity(tx["blockNumber"]) != height or quantity(tx["transactionIndex"]) != index):
                raise ValueError("transaction is inconsistent with its canonical block")
            if static_only and tx_hash in users:
                if latest_table is None:
                    raise ValueError("static user has no preceding same-block table")
                table, decoded, signature = latest_table
                if (not signature.startswith("loadExecutionTable(") or decoded[0] or not decoded[1]):
                    raise ValueError("static user does not follow a complete static-only load")
                selected.append(table)  # Preserve the full pool for independent verification.
                matched_users.add(tx_hash)
            if (tx.get("to") or "").lower() != manager:
                continue
            calldata = tx["input"]
            signature = signatures.get(calldata[:10].lower())
            if signature is None:
                continue
            decoded = json.loads(run_cast("decode-calldata", "--json", signature, calldata))
            if (not isinstance(decoded, list) or len(decoded) != 2
                    or not all(isinstance(entries, list) for entries in decoded)):
                raise ValueError("invalid decoded L2 table arrays")
            latest_table = (tx, decoded, signature)
            matched = []
            for entry in decoded[0]:
                # Pinned IEEZL2.ExecutionEntry: proxy, incoming, outgoing, rolling,
                # success, result. Rust independently decodes/validates the full ABI.
                preimage = hex_value(entry[0], 32) + hex_value(entry[3], 32)[2:]
                entry_hash = hex_value(run_cast("keccak", preimage), 32)
                proxy_hash = hex_value(entry[0], 32)
                loaded_keys.append((proxy_hash, entry_hash))
                if entry_hash in remaining:
                    matched.append(entry_hash)
                    selected_proxies.add(proxy_hash)
            if matched:
                for entry_hash in matched:
                    remaining[entry_hash] -= 1
                    if remaining[entry_hash] < 0:
                        raise ValueError("duplicate matching L2 entry occurrence")
                selected.append(tx)  # No filtering of entries or receipt events.
    if static_only and matched_users != set(users):
        raise ValueError("canonical L2 blocks are missing the static user transaction")
    if any(remaining.values()):
        raise ValueError("canonical L2 blocks are missing expected entry occurrences")
    if any(proxy in selected_proxies and key not in remaining for proxy, key in loaded_keys):
        raise ValueError("unexpected additional table for a selected scenario call")
    return selected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--abi", required=True, type=Path)
    parser.add_argument("--manager", required=True)
    parser.add_argument("--expected", required=True, help="JSON array of runtime entry hashes")
    parser.add_argument("--users", default="[]", help="Canonical user hashes for static-only discovery")
    args = parser.parse_args()
    methods = json.loads(args.abi.read_text())["methodIdentifiers"]
    selected = select_transactions(json.load(sys.stdin), args.manager, json.loads(args.expected),
                                   methods, users=json.loads(args.users))
    print(json.dumps(selected))


if __name__ == "__main__":
    main()
