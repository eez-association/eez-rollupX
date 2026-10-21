#!/usr/bin/env python3
"""Investigate excluded Ethereum mainnet settlement bundles (read-only).

The Composer only reports `bundle dropped ... reason=excluded`. This read-only
tool classifies each excluded target from public chain and relay data:

  POSSIBLY_LOCAL         client-like extraData with no observed relay delivery;
                         this is a hint, not proof of local construction
  OTHER_BUILDER          relay delivery confirmed; extraData does not match
                         the configured builders
  CONFIGURED_BUILDER     relay delivery confirmed; extraData matches a
                         configured builder (identity is inferred)
  TARGET_MISMATCH        the target height carries another timestamp (skipped
                         slot or reorg)
  UNKNOWN                insufficient evidence, including relay outages

Targets come from block numbers or from Composer JSON logs (`--logs`), which
also supply the expected target timestamp and the bundle hash each builder
returned. Stats are queried for every acknowledging builder with a supported
API and a valid hash, regardless of who won. Unsupported APIs, missing hashes,
and unavailable stats are explicit. Titan traces take about five minutes to
appear; Bombora stats take four blocks. Pending/missing stats are inconclusive.

Usage:
  sudo docker logs --since 2h eez-node-ethereum 2>&1 | \\
    python3 scripts/why-excluded.py --logs - --builders "$EEZ_L1_BUILDER_RPC_URL"
  python3 scripts/why-excluded.py 26131577 26131674 --rpc https://ethereum-rpc.publicnode.com
"""
import argparse
import json
import os
import re
import sys
import urllib.parse
import urllib.request

UA = {"Content-Type": "application/json", "Accept": "application/json",
      "User-Agent": "eez-why-excluded/1"}

# Public MEV-Boost relay data APIs (read-only).
RELAYS = {
    "ultrasound": "https://relay.ultrasound.money",
    "flashbots": "https://boost-relay.flashbots.net",
    "titan": "https://titanrelay.xyz",
    "bloxroute-regulated": "https://bloxroute.regulated.blxrbdn.com",
    "aestus": "https://aestus.live",
    "agnostic": "https://agnostic-relay.net",
}

# Builder RPC hosts -> extraData identity hints (not authenticated identities).
BUILDER_MARKERS = {
    "titanbuilder.xyz": ("titan",),
    "buildernet.org": ("buildernet",),
    "flashbots.net": ("buildernet", "flashbots"),
    "quasar.win": ("quasar",),
    "eurekabuilder.xyz": ("eureka",),
    "bombora.build": ("bombora",),
    "beaverbuild.org": ("beaver",),
    "rsync-builder.xyz": ("rsync",),
}

# Stats use dedicated hosts, not the submission RPC. Both accept {"bundleHash": ...}.
BUNDLE_STATS = {
    "titanbuilder.xyz": ("https://stats.titanbuilder.xyz", "titan_getBundleStats"),
    "bombora.build": ("https://stats.bombora.build", "bombora_getBundleStats"),
}

CLIENT_RE = re.compile(
    r"geth|nethermind|besu|erigon|reth|nimbus|lighthouse|teku|lodestar|prysm|grandine",
    re.IGNORECASE)


def http_json(url, payload=None, timeout=15):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(url, data=data, headers=UA)
    with urllib.request.urlopen(req, timeout=timeout) as response:
        return json.load(response)


def rpc(url, method, params):
    reply = http_json(url, {"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    if not isinstance(reply, dict) or "result" not in reply and "error" not in reply:
        raise ValueError(f"{method}: malformed RPC response")
    if reply.get("error") is not None:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def host_key(url):
    """Known builder domain, or the last two labels as an unknown-host fallback."""
    host = (urllib.parse.urlsplit(url.strip()).hostname or "").lower()
    for key in BUILDER_MARKERS:
        if host == key or host.endswith("." + key):
            return key
    return ".".join(host.split(".")[-2:]) if host else ""


def configured_builders(urls):
    """Map configured builder domains to their expected extraData hints."""
    builders = {}
    for url in filter(None, (u.strip() for u in urls.split(","))):
        key = host_key(url)
        if key:
            builders[key] = BUILDER_MARKERS.get(key, (key.split(".")[0],))
    return builders


def decode_extra(hex_data):
    raw = bytes.fromhex(hex_data[2:] if hex_data.startswith("0x") else hex_data)
    text = raw.decode("utf-8", "replace")
    # Clients such as geth RLP-encode their version; keep only readable text.
    return re.sub(r"\s+", " ", "".join(
        ch if ch.isprintable() and ch != "�" else " " for ch in text)).strip()


def classify(extra, delivered, builders, expected_ts=None, block_ts=None, unavailable=()):
    """Return an evidence category and optional extraData-based builder hint."""
    if expected_ts is not None and block_ts is not None and expected_ts != block_ts:
        return "TARGET_MISMATCH", None
    if not delivered:
        if not unavailable and extra and CLIENT_RE.search(extra):
            return "POSSIBLY_LOCAL", None
        return "UNKNOWN", None
    if not builders or not extra or CLIENT_RE.search(extra):
        return "UNKNOWN", None
    lowered = extra.lower()
    for key, markers in builders.items():
        if any(marker in lowered for marker in markers):
            return "CONFIGURED_BUILDER", key
    return "OTHER_BUILDER", None


def correlation_hash(value):
    """Only retain exact 32-byte correlation identifiers from logs."""
    if isinstance(value, str) and re.fullmatch(r"0x[0-9a-fA-F]{64}", value):
        return value.lower()
    return None


def parse_logs(lines):
    """Collect excluded targets from Composer JSON logs.

    Returns {target_block: {"timestamp": int|None, "post_tx": str|None,
    "bundles": {builder_host: bundle_hash|None}}} for targets dropped as excluded.
    A successful acknowledgement need not contain a usable correlation hash.
    """
    seen, excluded = {}, set()
    for line in lines:
        start = line.find("{")
        if start < 0:
            continue
        try:
            event = json.loads(line[start:])
        except ValueError:
            continue
        if not isinstance(event, dict) or not isinstance(event.get("fields"), dict):
            continue
        fields = event["fields"]
        try:
            target = int(fields.get("target_block"))
        except (TypeError, ValueError):
            continue
        if target < 0:
            continue
        entry = seen.setdefault(target, {"timestamp": None, "post_tx": None, "bundles": {}})
        message = fields.get("message", "")
        if not isinstance(message, str):
            continue
        if fields.get("target_timestamp") is not None:
            try:
                entry["timestamp"] = int(fields["target_timestamp"])
            except (TypeError, ValueError):
                pass
        if message.startswith("eth_sendBundle response received"):
            try:
                response = json.loads(fields.get("response", "{}"))
            except (TypeError, ValueError):
                response = None
            # Match the submitter's acknowledgement semantics: an optional hash
            # is metadata, and a non-object result is still a success response.
            if isinstance(response, dict) and "result" in response and "error" not in response:
                result = response["result"]
                bundle_hash = correlation_hash(result.get("bundleHash")) if isinstance(result, dict) else None
                host = fields.get("builder_host")
                if isinstance(host, str) and host:
                    entry["bundles"][host_key("https://" + host)] = bundle_hash
        if message.startswith("bundle dropped") and fields.get("reason") == "excluded":
            excluded.add(target)
            entry["post_tx"] = correlation_hash(fields.get("tx_hash"))
    return {target: seen[target] for target in sorted(excluded)}


def relay_deliveries(block_hash):
    """Keep only deliveries of the canonical execution payload, not its siblings."""
    delivered, unavailable = [], []
    for name, base in RELAYS.items():
        url = f"{base}/relay/v1/data/bidtraces/proposer_payload_delivered?block_hash={block_hash}"
        try:
            data = http_json(url)
            if not isinstance(data, list) or any(not isinstance(trace, dict) for trace in data):
                raise ValueError("malformed relay response")
            matching = []
            for trace in data:
                if not isinstance(trace.get("block_hash"), str):
                    raise ValueError("relay delivery has no block hash")
                if trace["block_hash"].lower() != block_hash.lower():
                    continue
                matching.append({"relay": name, "builder_pubkey": trace.get("builder_pubkey"),
                                 "value_eth": int(trace.get("value", 0)) / 1e18})
        except (OSError, ValueError, TypeError):
            unavailable.append(name)
            continue
        delivered.extend(matching)
    return delivered, unavailable


def block_summary(rpc_url, number):
    block = rpc(rpc_url, "eth_getBlockByNumber", [hex(number), True])
    if block is None:
        return None
    base = int(block["baseFeePerGas"], 16)
    tips, blob_txs, blobs = [], 0, 0
    for tx in block["transactions"]:
        max_fee = int(tx.get("maxFeePerGas", tx["gasPrice"]), 16)
        priority = int(tx.get("maxPriorityFeePerGas", tx["gasPrice"]), 16)
        tips.append(min(priority, max_fee - base))
        if tx.get("type") == "0x3":
            blob_txs += 1
            blobs += len(tx.get("blobVersionedHashes", []))
    tips.sort()

    def pct(p):
        return round(tips[min(len(tips) - 1, int(len(tips) * p))] / 1e9, 6) if tips else None

    return {
        "timestamp": int(block["timestamp"], 16),
        "hash": block["hash"],
        "extra": decode_extra(block["extraData"]),
        "fee_recipient": block["miner"],
        "gas_used_pct": round(100 * int(block["gasUsed"], 16) / int(block["gasLimit"], 16), 1),
        "base_fee_gwei": round(base / 1e9, 6),
        "txs": len(tips), "blob_txs": blob_txs, "blobs": blobs,
        "tip_p25_gwei": pct(0.25), "tip_p50_gwei": pct(0.5),
        "transactions": {tx["hash"].lower(): tx["from"].lower() for tx in block["transactions"]},
    }


def landed_after(rpc_url, post_tx, start, horizon):
    """Locate the exact post; partial or failed scans do not prove absence."""
    for number in range(start, start + horizon):
        try:
            summary = block_summary(rpc_url, number)
        except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
            return {"status": "unavailable", "through_block": number - 1, "error": str(error)}
        if summary is None:
            return {"status": "pending", "through_block": number - 1}
        if post_tx in summary["transactions"]:
            return {"status": "included", "block": number, "block_hash": summary["hash"]}
    return {"status": "not_found", "through_block": start + horizon - 1}


def bundle_stats(host, bundle_hash):
    endpoint = BUNDLE_STATS.get(host)
    if not endpoint:
        return {"availability": "unsupported"}
    if not bundle_hash:
        return {"availability": "missing_hash"}
    url, method = endpoint
    try:
        reply = http_json(url, {"jsonrpc": "2.0", "id": 1, "method": method,
                                "params": [{"bundleHash": bundle_hash}]})
    except (OSError, ValueError) as error:
        return {"availability": "unavailable", "error": str(error)}
    if not isinstance(reply, dict):
        return {"availability": "unavailable", "error": "malformed stats response"}
    if reply.get("error") is not None:
        return {"availability": "unavailable", "error": reply["error"]}
    if not isinstance(reply.get("result"), dict):
        return {"availability": "unavailable", "error": "missing stats result"}
    return {"availability": "available", "result": reply["result"]}


def analyze(target, info, args, builders):
    # A losing builder can still explain a rejection. Collect its stats even
    # when the target is unavailable, mismatched, or won by another builder.
    report = {
        "target": target, "expected_timestamp": info.get("timestamp"),
        "bundles_acknowledged": sorted(info.get("bundles", {})),
        "builder_bundle_stats": {
            host: {"bundle_hash": bundle_hash, **bundle_stats(host, bundle_hash)}
            for host, bundle_hash in sorted(info.get("bundles", {}).items())
        },
    }
    try:
        summary = block_summary(args.rpc, target)
    except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
        return {**report, "category": "RPC_UNAVAILABLE", "error": str(error)}
    if summary is None:
        return {**report, "category": "NOT_YET_PRODUCED"}
    delivered, unavailable = relay_deliveries(summary["hash"])
    category, matched = classify(summary["extra"], delivered, builders,
                                 info.get("timestamp"), summary["timestamp"], unavailable)
    report.update({
        "category": category, "builder": summary["extra"] or "(empty)",
        "matched_builder": matched, "relays": sorted({d["relay"] for d in delivered}),
        "relays_unavailable": unavailable,
        "block": {key: summary[key] for key in (
            "hash", "timestamp", "fee_recipient", "gas_used_pct", "base_fee_gwei",
            "txs", "blob_txs", "blobs", "tip_p25_gwei", "tip_p50_gwei")},
    })
    post_tx = correlation_hash(info.get("post_tx"))
    report["post_tx"] = post_tx
    report["post_in_target"] = post_tx in summary["transactions"] if post_tx else None
    if report["post_in_target"]:
        report["landed"] = {"status": "included", "block": target, "block_hash": summary["hash"]}
    elif post_tx:
        report["landed"] = landed_after(args.rpc, post_tx, target + 1, args.horizon)
    else:
        report["landed"] = {"status": "unknown", "reason": "no exact post hash in supplied logs"}
    if args.poster:
        report["poster_transactions_in_target"] = [
            tx_hash for tx_hash, sender in summary["transactions"].items() if sender == args.poster]
    return report


EXPLANATION = {
    "POSSIBLY_LOCAL": "client-like extraData; no observed relay delivery (local construction unproven)",
    "OTHER_BUILDER": "relay delivery confirmed; extraData does not match configured builders",
    "CONFIGURED_BUILDER": "relay delivery confirmed; extraData matches a configured builder",
    "TARGET_MISMATCH": "target height has another timestamp (skipped slot or reorg)",
    "UNKNOWN": "insufficient evidence to identify the block producer",
    "NOT_YET_PRODUCED": "target block does not exist yet",
    "RPC_UNAVAILABLE": "target block could not be read",
}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("blocks", nargs="*", type=int, help="target L1 block numbers")
    parser.add_argument("--logs", help="Composer JSON log file, or - for stdin")
    parser.add_argument("--rpc", default=os.environ.get("WHY_EXCLUDED_RPC", "http://127.0.0.1:18645"),
                        help="L1 execution RPC (default: embedded L1 on this host)")
    parser.add_argument("--builders", default=os.environ.get("EEZ_L1_BUILDER_RPC_URL", ""),
                        help="comma-separated builder RPC URLs (default: EEZ_L1_BUILDER_RPC_URL)")
    parser.add_argument("--poster", help="show transactions from this address in the target (activity only)")
    parser.add_argument("--horizon", type=int, default=12,
                        help="blocks to scan after the target for the exact logged post hash (default 12)")
    parser.add_argument("--json", action="store_true", help="print one JSON object per target")
    args = parser.parse_args(argv)
    if args.horizon < 0 or any(number < 0 for number in args.blocks):
        parser.error("block numbers and --horizon must be nonnegative")
    if args.poster:
        args.poster = args.poster.lower()

    targets = {}
    if args.logs:
        stream = sys.stdin if args.logs == "-" else open(args.logs, encoding="utf-8")
        with stream:
            targets.update(parse_logs(stream))
    for block in args.blocks:
        targets.setdefault(block, {"timestamp": None, "post_tx": None, "bundles": {}})
    if not targets:
        parser.error("no targets: pass block numbers or --logs with excluded drops")
    builders = configured_builders(args.builders)
    if not builders:
        print("warning: no configured builders (set --builders or EEZ_L1_BUILDER_RPC_URL); "
              "builder membership is unknown", file=sys.stderr)

    try:
        chain_id = int(rpc(args.rpc, "eth_chainId", []), 16)
    except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
        parser.error(f"cannot verify the L1 network: {error}")
    if chain_id != 1:
        parser.error("the configured relay/stats APIs are Ethereum mainnet only (chain ID 1)")

    counts = {}
    for target in sorted(targets):
        report = analyze(target, targets[target], args, builders)
        counts[report["category"]] = counts.get(report["category"], 0) + 1
        if args.json:
            print(json.dumps(report))
            continue
        print(f"{target}  {report['category']:<18} {EXPLANATION[report['category']]}")
        if "block" in report:
            block = report["block"]
            print(f"    extraData={json.dumps(report['builder'])} relays={report['relays'] or '-'}"
                  f" gas={block['gas_used_pct']}% base={block['base_fee_gwei']} gwei"
                  f" blobs={block['blobs']} tip p25/p50={block['tip_p25_gwei']}/{block['tip_p50_gwei']} gwei")
        if report["bundles_acknowledged"]:
            print(f"    our bundle acknowledged by: {', '.join(report['bundles_acknowledged'])}")
        for host, stats in report["builder_bundle_stats"].items():
            print(f"    {host} bundle stats: {json.dumps(stats)}")
        if "landed" in report:
            print(f"    exact post {report['post_tx'] or '(unknown)'}: {json.dumps(report['landed'])}")
        if "poster_transactions_in_target" in report:
            print(f"    poster activity (not settlement evidence): {report['poster_transactions_in_target']}")
        if report.get("relays_unavailable"):
            print(f"    (relays unavailable: {', '.join(report['relays_unavailable'])})")
        if "error" in report:
            print(f"    error: {json.dumps(report['error'])}")
    if not args.json:
        print("summary: " + ", ".join(f"{k}={v}" for k, v in sorted(counts.items())))
    return 1 if counts.get("RPC_UNAVAILABLE") else 0


if __name__ == "__main__":
    sys.exit(main())
