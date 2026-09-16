"""Shared explorer configuration and canonical Chiado history lookup."""
import json
import shlex
import urllib.request


def read_env(path):
    values = {}
    for line in path.read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = " ".join(shlex.split(value, comments=True))
    return values


def request(url, payload=None, method=None, headers=None):
    req = urllib.request.Request(
        url, data=None if payload is None else json.dumps(payload).encode(),
        method=method, headers={"Content-Type": "application/json", **(headers or {})},
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        body = response.read()
        return json.loads(body) if body else None


def rpc(url, method, params):
    result = request(url, dict(jsonrpc="2.0", id=1, method=method, params=params))
    if "error" in result:
        raise ValueError(result["error"])
    return result["result"]


def resolve_origin(start_block, l1_rpc, beacon):
    if start_block < 1:
        raise ValueError("The Blobscan starting block must be positive")
    if int(rpc(l1_rpc, "eth_chainId", []), 16) != 10200:
        raise ValueError("Blobscan requires Chiado L1 chain 10200")
    block = rpc(l1_rpc, "eth_getBlockByNumber", [hex(start_block), False])
    if not block:
        raise ValueError("The L1 endpoint does not have the configured starting block")
    genesis = request(beacon + "/eth/v1/beacon/genesis")["data"]
    spec = request(beacon + "/eth/v1/config/spec")["data"]
    slot, remainder = divmod(
        int(block["timestamp"], 16) - int(genesis["genesis_time"]),
        int(spec["SECONDS_PER_SLOT"]),
    )
    deneb_slot = int(spec["DENEB_FORK_EPOCH"]) * int(spec["SLOTS_PER_EPOCH"])
    if remainder or slot < max(1, deneb_slot):
        raise ValueError("Starting block must correspond to a beacon slot at or after Deneb")
    beacon_block = request(beacon + f"/eth/v2/beacon/blocks/{slot}")
    payload = beacon_block["data"]["message"]["body"]["execution_payload"]
    if payload["block_hash"].lower() != block["hash"].lower():
        raise ValueError("Execution and consensus endpoints disagree on the starting block")
    return dict(chainId=10200, startBlock=start_block, startSlot=slot,
                startBlockHash=block["hash"].lower())
