#!/usr/bin/env python3
"""Explain why a settlement bundle was not included in its target L1 block.

The Composer only reports `bundle dropped ... reason=excluded`. This read-only
tool classifies each excluded target from public chain and relay data:

  LOCAL                  the proposer built the block without MEV-Boost, so no
                         bundle could be included
  OTHER_BUILDER          a builder outside EEZ_L1_BUILDER_RPC_URL won the slot
  CONFIGURED_BUILDER     one of our builders won the slot without our bundle;
                         the only case that may need action (tip, bundle shape)
  TARGET_MISMATCH        the target height carries another timestamp (skipped
                         slot or reorg)
  UNKNOWN                no listed relay delivered it and the extraData is not
                         a known client string

Targets come from block numbers or from Composer JSON logs (`--logs`), which
also supply the expected target timestamp and the bundle hash each builder
returned. Builder bundle-stats endpoints are queried only when a hash is known;
their retention is short, so run this soon after the slot.

Usage:
  sudo docker logs --since 2h eez-node-ethereum 2>&1 | \\
    python3 scripts/why-excluded.py --logs - --poster 0x...
  python3 scripts/why-excluded.py 26131577 26131674 --rpc https://ethereum-rpc.publicnode.com
"""
import argparse
import json
import os
import re
import sys
import urllib.error
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

# Builder RPC hosts -> lowercase extraData markers of the blocks they build.
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

# Bundle-stats methods known to accept {"bundleHash": ...}; best effort only.
BUNDLE_STATS = {
    "titanbuilder.xyz": "titan_getBundleStats",
    "bombora.build": "bombora_getBundleStats",
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
    if "error" in reply:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def host_key(url):
    """Registrable host of a builder URL, e.g. rpc.titanbuilder.xyz -> titanbuilder.xyz."""
    host = (urllib.parse.urlsplit(url.strip()).hostname or "").lower()
    for key in BUILDER_MARKERS:
        if host == key or host.endswith("." + key):
            return key
    return ".".join(host.split(".")[-2:]) if host else ""


def configured_builders(urls):
    """Map each configured builder host to the extraData markers it signs blocks with."""
    builders = {}
    for url in filter(None, (u.strip() for u in urls.split(","))):
        key = host_key(url)
        builders[key] = BUILDER_MARKERS.get(key, (key.split(".")[0],))
    return builders


def decode_extra(hex_data):
    raw = bytes.fromhex(hex_data[2:] if hex_data.startswith("0x") else hex_data)
    text = raw.decode("utf-8", "replace")
    # Clients such as geth RLP-encode their version; keep only readable text.
    return re.sub(r"\s+", " ", "".join(
        ch if ch.isprintable() and ch != "�" else " " for ch in text)).strip()


def classify(extra, delivered, builders, expected_ts=None, block_ts=None):
    """Pure classification used by tests. Returns (category, matched builder host)."""
    if expected_ts is not None and block_ts is not None and expected_ts != block_ts:
        return "TARGET_MISMATCH", None
    lowered = extra.lower()
    for key, markers in builders.items():
        if any(marker in lowered for marker in markers):
            return "CONFIGURED_BUILDER", key
    if not delivered and (not extra or CLIENT_RE.search(extra)):
        return "LOCAL", None
    if delivered:
        return "OTHER_BUILDER", None
    return "UNKNOWN", None


def parse_logs(lines):
    """Collect excluded targets from Composer JSON logs.

    Returns {target_block: {"timestamp": int|None, "post_tx": str|None,
    "bundles": {builder_host: bundle_hash}}} for targets dropped as excluded.
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
        fields = event.get("fields", {})
        target = fields.get("target_block")
        if target is None:
            continue
        target = int(target)
        entry = seen.setdefault(target, {"timestamp": None, "post_tx": None, "bundles": {}})
        message = fields.get("message", "")
        if fields.get("target_timestamp") is not None:
            entry["timestamp"] = int(fields["target_timestamp"])
        if message.startswith("eth_sendBundle response received"):
            try:
                result = json.loads(fields.get("response", "{}")).get("result") or {}
            except ValueError:
                result = {}
            if result.get("bundleHash"):
                entry["bundles"][host_key("https://" + fields.get("builder_host", ""))] = \
                    result["bundleHash"]
        if message.startswith("bundle dropped") and fields.get("reason") == "excluded":
            excluded.add(target)
            entry["post_tx"] = fields.get("tx_hash")
    return {target: seen[target] for target in sorted(excluded)}


def relay_deliveries(block):
    delivered, unavailable = [], []
    for name, base in RELAYS.items():
        url = f"{base}/relay/v1/data/bidtraces/proposer_payload_delivered?block_number={block}"
        try:
            data = http_json(url)
        except (urllib.error.URLError, OSError, ValueError):
            unavailable.append(name)
            continue
        for trace in data or []:
            delivered.append({"relay": name, "builder_pubkey": trace.get("builder_pubkey"),
                              "value_eth": int(trace.get("value", 0)) / 1e18})
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
        "senders": {tx["from"].lower() for tx in block["transactions"]},
    }


def landed_after(rpc_url, poster, start, horizon):
    """First block at or after `start` with a transaction from the poster."""
    for number in range(start, start + horizon):
        try:
            summary = block_summary(rpc_url, number)
        except Exception:
            return None
        if summary is None:
            return None
        if poster in summary["senders"]:
            return {"block": number, "builder": summary["extra"]}
    return None


def bundle_stats(host, bundle_hash):
    method = BUNDLE_STATS.get(host)
    if not method:
        return None
    url = "https://rpc." + host
    try:
        reply = http_json(url, {"jsonrpc": "2.0", "id": 1, "method": method,
                                "params": [{"bundleHash": bundle_hash}]})
    except (urllib.error.URLError, OSError, ValueError) as error:
        return {"error": str(error)}
    return reply.get("result", reply.get("error"))


def analyze(target, info, args, builders):
    summary = block_summary(args.rpc, target)
    if summary is None:
        return {"target": target, "category": "NOT_YET_PRODUCED"}
    delivered, unavailable = relay_deliveries(target)
    category, matched = classify(summary["extra"], delivered, builders,
                                 info.get("timestamp"), summary["timestamp"])
    report = {
        "target": target, "category": category, "builder": summary["extra"] or "(empty)",
        "matched_builder": matched, "relays": sorted({d["relay"] for d in delivered}),
        "relays_unavailable": unavailable,
        "block": {key: summary[key] for key in (
            "hash", "timestamp", "fee_recipient", "gas_used_pct", "base_fee_gwei",
            "txs", "blob_txs", "blobs", "tip_p25_gwei", "tip_p50_gwei")},
        "expected_timestamp": info.get("timestamp"),
        "bundles_acknowledged": sorted(info.get("bundles", {})),
    }
    if args.poster:
        report["post_in_target"] = args.poster in summary["senders"]
        report["landed"] = landed_after(args.rpc, args.poster, target + 1, args.horizon)
    if category == "CONFIGURED_BUILDER" and matched in info.get("bundles", {}):
        report["builder_bundle_stats"] = bundle_stats(matched, info["bundles"][matched])
    return report


EXPLANATION = {
    "LOCAL": "proposer built locally (no MEV-Boost); no bundle could be included",
    "OTHER_BUILDER": "won by a builder outside EEZ_L1_BUILDER_RPC_URL",
    "CONFIGURED_BUILDER": "one of our builders won without our bundle (auction/blob choice)",
    "TARGET_MISMATCH": "target height has another timestamp (skipped slot or reorg)",
    "UNKNOWN": "no listed relay delivered it; extraData is not a known client",
    "NOT_YET_PRODUCED": "target block does not exist yet",
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
    parser.add_argument("--poster", help="poster address, to report where the post landed")
    parser.add_argument("--horizon", type=int, default=12,
                        help="blocks to scan after the target for the poster (default 12)")
    parser.add_argument("--json", action="store_true", help="print one JSON object per target")
    args = parser.parse_args(argv)
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
              "every winning builder will be reported as OTHER_BUILDER", file=sys.stderr)

    counts = {}
    for target in sorted(targets):
        report = analyze(target, targets[target], args, builders)
        counts[report["category"]] = counts.get(report["category"], 0) + 1
        if args.json:
            print(json.dumps(report))
            continue
        print(f"{target}  {report['category']:<18} {EXPLANATION[report['category']]}")
        if report["category"] == "NOT_YET_PRODUCED":
            continue
        block = report["block"]
        print(f"    builder='{report['builder']}' relays={report['relays'] or '-'}"
              f" gas={block['gas_used_pct']}% base={block['base_fee_gwei']} gwei"
              f" blobs={block['blobs']} tip p25/p50={block['tip_p25_gwei']}/{block['tip_p50_gwei']} gwei")
        if report["bundles_acknowledged"]:
            print(f"    our bundle acknowledged by: {', '.join(report['bundles_acknowledged'])}")
        if report.get("builder_bundle_stats") is not None:
            print(f"    {report['matched_builder']} bundle stats: "
                  f"{json.dumps(report['builder_bundle_stats'])[:300]}")
        if args.poster:
            landed = report.get("landed")
            where = (f"landed in {landed['block']} ({landed['builder']})" if landed
                     else f"not landed within {args.horizon} blocks")
            print(f"    post: {'in target' if report.get('post_in_target') else where}")
        if report["relays_unavailable"]:
            print(f"    (relays unavailable: {', '.join(report['relays_unavailable'])})")
    if not args.json:
        print("summary: " + ", ".join(f"{k}={v}" for k, v in sorted(counts.items())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
