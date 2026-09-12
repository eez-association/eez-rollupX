#!/usr/bin/env python3
"""Private-network canary: replace a source nonce after its bundle is proved.

Requires the partial-consumption Kurtosis profile and a deployed Value proxy.
Uses fresh funded users and records public evidence; never saves their keys.
The actual pinned builder must omit the stale allowed user and include the
dependent source revert. There is no request mutation or fake receipt source.
"""

import argparse
import json
import re
import secrets
import subprocess
import time
import urllib.request
from pathlib import Path

FUNDER = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"


def command(*args):
    try:
        result = subprocess.run(args, text=True, capture_output=True, timeout=45, check=False)
    except subprocess.TimeoutExpired:
        # TimeoutExpired includes argv, which may contain a fresh user's key.
        raise RuntimeError(f"{args[0]} timed out") from None
    if result.returncode:
        error = result.stderr
        if "--private-key" in args:
            error = error.replace(args[args.index("--private-key") + 1], "[private key]")
        raise RuntimeError(f"{args[0]} failed: {error}")
    return result.stdout.strip()


def rpc(url, method, *params):
    request = urllib.request.Request(url, data=json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
    }).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=10) as response:
        result = json.load(response)
    if "error" in result:
        raise RuntimeError(f"{method}: {result['error']}")
    return result["result"]


def wait_for(predicate, label, seconds=180, interval=0.1):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    raise RuntimeError(f"timed out: {label}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--enclave", required=True)
    parser.add_argument("--proxy", required=True, help="L1 proxy to an L2 Value target")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"0x[0-9a-fA-F]{40}", args.proxy):
        parser.error("--proxy must be an address")

    def port(service, name):
        value = command("kurtosis", "port", "print", args.enclave, service, name)
        return value if value.startswith("http") else "http://" + value

    l1 = port("el-1-reth-lighthouse", "rpc")
    l2 = port("eez-node", "l2-rpc")
    front = port("eez-node", "l1-xchain")
    assert int(rpc(l1, "eth_chainId"), 16) == 7331, "private validation chain required"
    service = command("kurtosis", "service", "inspect", args.enclave, "eez-node")
    uuid = re.search(r"^UUID: (\w+)$", service, re.MULTILINE).group(1)
    containers = command("docker", "ps", "--filter", f"name=eez-node--{uuid}", "--format", "{{.Names}}").splitlines()
    assert len(containers) == 1, "ambiguous node container"
    container = containers[0]
    assert command("docker", "exec", container, "printenv", "EEZ_BUNDLE_POLICY") == "allow-inbound-reverts"
    builder = command("kurtosis", "service", "inspect", args.enclave, "el-2-reth-builder-lighthouse")
    digest = "sha256:916ba84da2690e3e368f57da5b0c73bfd919b452de8b95f64895846e0f3a8308"
    assert digest in builder, "unexpected builder image"

    def journal():
        return json.loads(command("docker", "exec", container, "cat", "/data/l2/settlement-journal.json"))["state"]

    def signed(key, to, nonce, *, value="0", calldata="0x", priority="1000000000", gas="600000"):
        return command("cast", "mktx", to, calldata, "--private-key", key,
                       "--chain", "7331", "--nonce", str(nonce), "--gas-limit", gas,
                       "--gas-price", "1000000000000", "--priority-gas-price", priority,
                       "--value", value, "--rpc-url", l1)

    def receipt(hash_):
        return wait_for(lambda: rpc(l1, "eth_getTransactionReceipt", hash_), f"receipt {hash_}")

    keys = ["0x" + secrets.token_hex(32) for _ in range(3)]
    senders = [command("cast", "wallet", "address", "--private-key", key) for key in keys]
    funder = command("cast", "wallet", "address", "--private-key", FUNDER)
    for sender in senders:
        nonce = int(rpc(l1, "eth_getTransactionCount", funder, "latest"), 16)
        funded = rpc(l1, "eth_sendRawTransaction", signed(FUNDER, sender, nonce, value="1ether", gas="21000"))
        assert receipt(funded)["status"] == "0x1"

    users = [signed(key, args.proxy, 0, calldata=command("cast", "calldata", "setValue(uint256)", str(11 + index)))
             for index, key in enumerate(keys)]
    hashes = [command("cast", "keccak", raw).lower() for raw in users]
    replacement = signed(keys[1], senders[1], 0, gas="21000", priority="500000000000")
    replacement_hash = command("cast", "keccak", replacement).lower()

    # A dispatched empty attempt is a barrier after the current pool drain.
    generation = journal()["next_generation"]

    def empty_dispatch():
        state = journal()
        return any(int(key) >= generation and not attempt["txs"]
                   and attempt["dispatched_target"] is not None
                   for key, attempt in state["attempts"].items())

    wait_for(empty_dispatch, "empty dispatch before wave")
    for raw, hash_ in zip(users, hashes):
        assert rpc(front, "eth_sendRawTransaction", raw).lower() == hash_

    def proved_attempt():
        for attempt in journal()["attempts"].values():
            if [tx["hash"].lower() for tx in attempt["txs"]] == hashes:
                return attempt
        return None

    attempt = wait_for(proved_attempt, "durably prepared three-user proof")
    # begin() happens only after successful proof/sidecar construction. The
    # much higher replacement fee places it ahead of this bundle for both
    # pinned ordering algorithms; all canonical ordering is checked below.
    assert rpc(l1, "eth_sendRawTransaction", replacement).lower() == replacement_hash
    post_hash = attempt["id"]["post_batch_hash"]
    post = receipt(post_hash)
    replaced = receipt(replacement_hash)
    first = receipt(hashes[0])
    last = receipt(hashes[2])
    assert post["status"] == first["status"] == replaced["status"] == "0x1"
    assert last["status"] == "0x0"
    assert len({item["blockHash"] for item in [post, replaced, first, last]}) == 1
    assert int(replaced["transactionIndex"], 16) < int(post["transactionIndex"], 16) < int(first["transactionIndex"], 16) < int(last["transactionIndex"], 16)
    assert rpc(l1, "eth_getTransactionReceipt", hashes[1]) is None

    def completed():
        state = journal()
        if all("Terminal" in state["users"][hash_]["location"] for hash_ in hashes):
            return state
        return None

    state = wait_for(completed, "canonical per-user dispositions")
    reasons = [state["users"][hash_]["location"]["Terminal"]["reason"] for hash_ in hashes]
    assert reasons == ["Committed", "SourceNonceSpent", "SourceReverted"], reasons
    assert all(state["users"][hash_]["tx"]["attempts"] == 0 for hash_ in hashes)
    height = attempt["sync_height"]
    prefix = attempt["manifest"]["effects"][0]["block_hash"]
    full = attempt["manifest"]["sync_hash"]
    assert prefix != full
    assert rpc(l2, "eth_getBlockByNumber", hex(height), False)["hash"] == prefix

    healthy = signed(keys[1], args.proxy, 1, calldata=command("cast", "calldata", "setValue(uint256)", "99"))
    healthy_hash = rpc(front, "eth_sendRawTransaction", healthy)
    assert receipt(healthy_hash)["status"] == "0x1"
    safe = wait_for(lambda: (block if int((block := rpc(l2, "eth_getBlockByNumber", "safe", False))["number"], 16) > height else None), "continued safe-head progress")
    assert rpc(l2, "eth_getBlockByNumber", hex(height), False)["hash"] == prefix
    evidence = {"builder_image": digest, "post": post_hash, "l1_block": post["blockHash"],
                "l1_number": int(post["blockNumber"], 16), "sync_height": height,
                "partial_hash": prefix, "proposed_full_hash": full, "users": hashes,
                "replacement": replacement_hash, "dispositions": reasons,
                "attempt_counts": [0, 0, 0], "healthy": healthy_hash,
                "safe": {"number": int(safe["number"], 16), "hash": safe["hash"]}}
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
