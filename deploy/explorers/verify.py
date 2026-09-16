#!/usr/bin/env python3
"""Compare local explorer indexes with canonical RPC and native beacon data."""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import hmac
import json
from pathlib import Path
import time
import urllib.parse
import urllib.request

from configuration import read_env


def http(url, payload=None, method=None, headers=None):
    req = urllib.request.Request(
        url, data=None if payload is None else json.dumps(payload).encode(),
        method=method, headers={"Content-Type": "application/json", **(headers or {})},
    )
    with urllib.request.urlopen(req, timeout=60) as response:
        data = response.read()
        return json.loads(data) if data else None


def rpc(url, method, params):
    data = http(url, dict(jsonrpc="2.0", id=1, method=method, params=params))
    if "error" in data:
        raise RuntimeError(data["error"])
    return data["result"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--complete-backfill", action="store_true")
    parser.add_argument("--env-file", type=Path, default=Path(".env.explorers"))
    parser.add_argument("--l2-rpc", default="http://127.0.0.1:18688")
    args = parser.parse_args()
    values = read_env(args.env_file)
    l1 = values.get("BLOBSCAN_L1_RPC_URL", "http://127.0.0.1:18645")
    l2 = args.l2_rpc
    api = "http://127.0.0.1:" + values.get("BLOBSCAN_API_PORT", "18101")
    explorer = "http://127.0.0.1:" + values.get("BLOCKSCOUT_WEB_PORT", "18110")
    beacon = values.get("BLOBSCAN_BEACON_URL", "http://127.0.0.1:6152")
    check_l2 = "blockscout" in values.get("COMPOSE_PROFILES", "").split(",")
    assert int(rpc(l1, "eth_chainId", []), 16) == 10200
    if check_l2:
        assert int(rpc(l2, "eth_chainId", []), 16) == int(values["L2_CHAIN_ID"])
    start = int(values["BLOBSCAN_START_BLOCK"])
    snapshot = rpc(l1, "eth_getBlockByNumber", ["finalized", False])
    stop = int(snapshot["number"], 16)
    assert stop >= start, "Starting block is not finalized yet"
    genesis = http(beacon + "/eth/v1/beacon/genesis")["data"]
    spec = http(beacon + "/eth/v1/config/spec")["data"]
    slot_seconds = int(spec["SECONDS_PER_SLOT"])
    assert slot_seconds == 5 and int(spec["SLOTS_PER_EPOCH"]) == 16
    initial = rpc(l1, "eth_getBlockByNumber", [hex(start), False])
    start_slot = (int(initial["timestamp"], 16) - int(genesis["genesis_time"])) // slot_seconds
    assert start_slot == int(values["BLOBSCAN_START_SLOT"])
    assert initial["hash"].lower() == values["BLOBSCAN_START_BLOCK_HASH"].lower()
    state = http(api + "/blockchain-sync-state")
    snapshot_slot = (int(snapshot["timestamp"], 16) - int(genesis["genesis_time"])) // slot_seconds
    assert state["lastUpperSyncedSlot"] >= snapshot_slot, "Blobscan is still catching up; retry after indexing finishes"

    # Scan every canonical L1 block, including other posters. Checking only
    # registry events cannot establish complete retained Chiado blob history.
    blob_blocks = []
    previous_hash = None
    for first in range(start, stop + 1, 50):
        heights = list(range(first, min(first + 50, stop + 1)))
        data = http(l1, [dict(jsonrpc="2.0", id=n, method="eth_getBlockByNumber", params=[hex(n), True]) for n in heights])
        by_id = {item["id"]: item for item in data}
        assert len(by_id) == len(heights)
        for n in heights:
            assert "error" not in by_id[n], by_id[n]
            block = by_id[n]["result"]
            assert int(block["number"], 16) == n
            if previous_hash is not None:
                assert block["parentHash"] == previous_hash
            previous_hash = block["hash"]
            txs = [tx for tx in block["transactions"] if int(tx["type"], 16) == 3]
            if txs:
                assert sum(len(tx["blobVersionedHashes"]) for tx in txs) <= 2
                blob_blocks.append((block, txs))
        if (first - start) % 1000 == 0:
            print(f"Scanned canonical L1 blocks through {heights[-1]}", flush=True)
    assert previous_hash == snapshot["hash"]

    def check_blob_block(item):
        block, txs = item
        slot = (int(block["timestamp"], 16) - int(genesis["genesis_time"])) // slot_seconds
        beacon_block = http(beacon + f"/eth/v2/beacon/blocks/{slot}")["data"]["message"]["body"]
        assert beacon_block["execution_payload"]["block_hash"] == block["hash"]
        sidecars = http(beacon + f"/eth/v1/beacon/blob_sidecars/{slot}")["data"]
        by_hash = {}
        for sidecar in sidecars:
            commitment = bytes.fromhex(sidecar["kzg_commitment"][2:])
            versioned = "0x01" + hashlib.sha256(commitment).hexdigest()[2:]
            assert beacon_block["blob_kzg_commitments"][int(sidecar["index"])] == sidecar["kzg_commitment"]
            by_hash[versioned] = sidecar
        assert set(by_hash) == {h for tx in txs for h in tx["blobVersionedHashes"]}
        rows = []
        for tx in txs:
            indexed = http(api + "/transactions/" + tx["hash"])
            receipt = rpc(l1, "eth_getTransactionReceipt", [tx["hash"]])
            assert indexed["hash"] == tx["hash"]
            assert indexed["blockHash"] == receipt["blockHash"] == block["hash"]
            assert indexed["blockNumber"] == int(block["number"], 16)
            assert indexed["index"] == int(tx["transactionIndex"], 16)
            assert indexed["from"].lower() == tx["from"].lower()
            assert indexed["to"].lower() == tx["to"].lower()
            assert int(indexed["blobGasUsed"]) == int(receipt["blobGasUsed"], 16)
            assert int(indexed["blobGasPrice"]) == int(receipt["blobGasPrice"], 16)
            assert int(indexed["blobGasBaseFee"]) == int(receipt["blobGasUsed"], 16) * int(receipt["blobGasPrice"], 16)
            assert [b["versionedHash"] for b in indexed["blobs"]] == tx["blobVersionedHashes"]
            blobs = []
            for versioned in tx["blobVersionedHashes"]:
                details = http(api + "/blobs/" + versioned)
                data = bytes.fromhex(http(api + "/blobs/" + versioned + "/data")[2:])
                sidecar = by_hash[versioned]
                assert details["commitment"].lower() == sidecar["kzg_commitment"].lower()
                assert len(data) == details["size"] == 131072
                assert data == bytes.fromhex(sidecar["blob"][2:])
                assert any(ref["storage"] == "postgres" for ref in details["dataStorageReferences"])
                assert any(t["txHash"] == tx["hash"] for t in details["transactions"])
                blobs.append({"versionedHash": versioned, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
            rows.append({"hash": tx["hash"], "l1Block": indexed["blockNumber"], "slot": slot, "from": tx["from"], "blobFeeWei": indexed["blobGasBaseFee"], "blobs": blobs})
        return rows

    rows = []
    with ThreadPoolExecutor(max_workers=3) as pool:
        for batch in pool.map(check_blob_block, blob_blocks):
            rows.extend(batch)
            if len(rows) % 25 < len(batch):
                print(f"Compared {len(rows)} blob transactions with canonical receipts and beacon bytes", flush=True)

    safe_height = None
    l2_blocks, l2_txs = [], []
    if check_l2:
        safe = rpc(l2, "eth_getBlockByNumber", ["safe", False])
        safe_height = int(safe["number"], 16)
        heights = sorted({0, min(1, safe_height), safe_height // 2, max(0, safe_height - 1), safe_height})
        l2_blocks, l2_txs = [], []
        sample_txs = set()
        for number in heights:
            actual = rpc(l2, "eth_getBlockByNumber", [hex(number), False])
            indexed = http(explorer + f"/api/v2/blocks/{number}")
            assert indexed["hash"] == actual["hash"]
            assert int(indexed["gas_used"]) == int(actual["gasUsed"], 16)
            assert indexed["transactions_count"] == len(actual["transactions"])
            sample_txs.update(actual["transactions"])
            l2_blocks.append({"number": number, "hash": actual["hash"]})
        for tx_hash in sorted(sample_txs):
            tx = rpc(l2, "eth_getTransactionByHash", [tx_hash])
            receipt = rpc(l2, "eth_getTransactionReceipt", [tx_hash])
            indexed = http(explorer + "/api/v2/transactions/" + tx_hash)
            assert indexed["hash"] == tx_hash
            assert indexed["block_number"] == int(receipt["blockNumber"], 16)
            assert int(indexed["gas_used"]) == int(receipt["gasUsed"], 16)
            assert indexed["status"] == ("ok" if int(receipt["status"], 16) else "error")
            assert int(indexed["fee"]["value"]) == int(receipt["gasUsed"], 16) * int(receipt["effectiveGasPrice"], 16)
            assert indexed["from"]["hash"].lower() == tx["from"].lower()
            l2_txs.append({"hash": tx_hash, "block": indexed["block_number"], "status": indexed["status"]})
    assert rpc(l1, "eth_getBlockByNumber", [hex(stop), False])["hash"] == snapshot["hash"]

    if args.complete_backfill:
        if not args.env_file:
            parser.error("--complete-backfill requires the local explorer --env-file")
        state = http(api + "/blockchain-sync-state")
        assert state["lastLowerSyncedSlot"] in (None, start_slot)
        b64 = lambda value: base64.urlsafe_b64encode(value).rstrip(b"=")
        body = b64(b'{"alg":"HS512","typ":"JWT"}') + b"." + b64(json.dumps({"exp": int(time.time()) + 60}).encode())
        token = body + b"." + b64(hmac.digest(values["BLOBSCAN_SECRET_KEY"].encode(), body, hashlib.sha512))
        http(api + "/blockchain-sync-state", {"lastLowerSyncedSlot": start_slot}, "PUT", {"Authorization": "Bearer " + token.decode()})

    result = {
        "verified": True, "checkedAt": datetime.now(timezone.utc).isoformat(),
        "l1From": start, "l1Through": stop, "l1ThroughHash": snapshot["hash"],
        "canonicalL1BlocksScanned": stop - start + 1,
        "blobTransactions": len(rows), "blobs": sum(len(row["blobs"]) for row in rows),
        "blobBytesAndCommitmentsMatchNativeBeacon": True,
        "independentKzgRecomputation": False,
        "l2SafeAtCheck": safe_height, "l2Blocks": l2_blocks, "l2Transactions": l2_txs,
        "backfillCompletionRecorded": args.complete_backfill,
        "transactions": rows,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k not in ("transactions", "l2Transactions", "l2Blocks")}), flush=True)


if __name__ == "__main__":
    main()
