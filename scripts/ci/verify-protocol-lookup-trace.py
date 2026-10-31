#!/usr/bin/env python3
"""Bind an authenticated L1 semantic bracket to its canonical callTracer tree.

The caller owns receipt/canonicality, KZG and settlement-table verification and
supplies proxy addresses derived from the pinned registry at the trigger block.
This check cannot replace those checks or independent execution replay.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess


STATIC_LOOKUP = "31344ade"
MUTABLE_LOOKUP = "9af53259"
FORWARD = "8205f3e1"
CREATE_PROXY = "a7587c62"
MISSING_PROXY = "9c9f37f4"


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def hex_bytes(value, size=None):
    require(isinstance(value, str) and re.fullmatch(r"0x(?:[0-9a-fA-F]{2})*", value),
            "invalid hex bytes")
    raw = bytes.fromhex(value[2:])
    require(size is None or len(raw) == size, "invalid hex width")
    return raw


def address(value):
    return hex_bytes(value, 20).hex()


def word(value):
    return value.to_bytes(32, "big").hex()


def abi_bytes(data):
    raw = hex_bytes(data)
    return word(len(raw)) + raw.hex() + "00" * (-len(raw) % 32)


def lookup_input(actor, data, static):
    return "0x" + (STATIC_LOOKUP if static else MUTABLE_LOOKUP) + "00" * 12 + address(actor) + word(64) + abi_bytes(data)


def forward_input(target, data):
    return "0x" + FORWARD + "00" * 12 + address(target) + word(0) + word(96) + abi_bytes(data)


def returned_bytes(data):
    return "0x" + word(32) + abi_bytes(data)


def verify(document):
    trace, trigger, bracket = (document[key] for key in ("trace", "trigger", "semantic_transaction"))
    registry = "0x" + address(document["registry"])
    rollup = document["rollup_id"]
    require(type(rollup) is int and 0 < rollup < 2**64, "invalid rollup")
    proxies = {"0x" + address(actor): "0x" + address(proxy)
               for actor, proxy in document["proxies"].items()}
    require(len(set(proxies.values())) == len(proxies), "ambiguous proxy identities")
    require(trace.get("type") == "CALL" and not trace.get("error"), "trigger trace failed")
    for field in ("from", "to", "input"):
        require(trace[field].lower() == trigger[field].lower(), "trace differs from canonical trigger")
    require(int(trace.get("value", "0x0"), 16) == int(trigger.get("value", "0x0"), 16),
            "trace value differs from canonical trigger")
    require(bracket["origin_chain"] == 0 and bracket["tx_data"] == "0x", "not an L1 semantic bracket")
    actions = bracket["actions"]
    require(actions, "empty semantic bracket")
    frames = []

    def flatten(node, path=(), static=False):
        static = static or node.get("type") == "STATICCALL"
        frames.append((path, node, static))
        for index, child in enumerate(node.get("calls", [])):
            flatten(child, path + (index,), static)

    flatten(trace)
    by_path = {path: (node, static) for path, node, static in frames}
    selected, dispatches, forwards = {}, set(), set()
    previous_path = None
    for index, action in enumerate(actions):
        require(action["index"] == index, "nonsequential action index")
        parent = action["parent_index"]
        require(parent is None or type(parent) is int and 0 <= parent < index, "invalid action parent")
        require(action["kind"] in ("Call", "StaticCall"), "unsupported lookup action kind")
        static = action["kind"] == "StaticCall"
        source, target = action["source_chain"], action["target_chain"]
        require((source, target) in ((0, rollup), (rollup, 0)), "invalid action direction")
        require(int(action["value"]) == 0, "lookup transfers value")
        require(type(action["success"]) is bool and action["result_kind"] ==
                ("ReturnSuccess" if action["success"] else "ReturnFail"), "inconsistent action result")
        if parent is None:
            require(source == 0 and action["from_address"].lower() == trigger["to"].lower(),
                    "lookup root differs from trigger target")
        else:
            require(source == actions[parent]["target_chain"], "action direction differs from parent")
        remote = action["to_address"] if source == 0 else action["from_address"]
        proxy = proxies.get(remote.lower())
        require(proxy is not None, "missing registry-derived proxy")
        sender = action["from_address"].lower() if source == 0 else proxy
        recipient = proxy if source == 0 else action["to_address"].lower()
        matches = []
        for path, node, effective_static in frames:
            if node.get("type") not in ("CALL", "STATICCALL") or effective_static != static:
                continue
            if (node.get("from", "").lower(), node.get("to", "").lower(), node.get("input", "").lower()) != (
                    sender, recipient, action["data"].lower()):
                continue
            if int(node.get("value", "0x0"), 16) != 0 or bool(node.get("error")) == action["success"]:
                continue
            if node.get("output", "0x").lower() != action["return_data"].lower():
                continue
            if parent is not None and not (len(path) > len(selected[parent]) and
                                           path[:len(selected[parent])] == selected[parent]):
                continue
            if previous_path is not None and path <= previous_path:
                continue
            matches.append(path)
        require(len(matches) == 1, f"action {index} lacks one exact ordered trace match")
        path = selected[index] = matches[0]
        previous_path = path
        node = by_path[path][0]
        if source == 0:
            expected_input = lookup_input(action["from_address"], action["data"], static)
            matching = [(path + (i,), child) for i, child in enumerate(node.get("calls", []))
                        if child.get("type") in ("CALL", "STATICCALL")
                        and child.get("from", "").lower() == proxy
                        and child.get("to", "").lower() == registry
                        and child.get("input", "").lower() == expected_input]
            require(len(matching) == 1, "lookup did not dispatch to the registry")
            dispatch_path, dispatch = matching[0]
            require(by_path[dispatch_path][1] == static and int(dispatch.get("value", "0x0"), 16) == 0,
                    "registry dispatch mode differs")
            expected_output = returned_bytes(action["return_data"]) if action["success"] else action["return_data"].lower()
            require(bool(dispatch.get("error")) != action["success"] and
                    dispatch.get("output", "0x").lower() == expected_output, "registry outcome differs")
            dispatches.add(dispatch_path)
        else:
            require(path, "callback has no proxy wrapper")
            wrapper = by_path[path[:-1]][0]
            require(wrapper.get("type") in ("CALL", "STATICCALL") and
                    wrapper.get("from", "").lower() == registry and wrapper.get("to", "").lower() == proxy and
                    wrapper.get("input", "").lower() == forward_input(action["to_address"], action["data"]),
                    "callback lacks the registry's exact forwarding frame")
            require(int(wrapper.get("value", "0x0"), 16) == 0 and
                    bool(wrapper.get("error")) != action["success"] and
                    wrapper.get("output", "0x").lower() == action["return_data"].lower(),
                    "callback forwarding outcome differs")
            require(parent is not None and any(path[:len(p)] == p for p in dispatches),
                    "callback is outside an authenticated registry dispatch")
            forwards.add(path[:-1])

    for index, path in selected.items():
        ancestors = [(len(p), i) for i, p in selected.items() if len(p) < len(path) and path[:len(p)] == p]
        actual_parent = max(ancestors)[1] if ancestors else None
        require(actual_parent == actions[index]["parent_index"], "semantic parent differs from trace ancestry")

    # A missing-proxy probe precedes deployment and the actual successful read.
    # It is not a semantic action; accept only the exact registry error for a
    # required callback proxy, followed by that proxy's creation and the retry.
    probes = []
    for path, node, static in frames:
        if node.get("type") not in ("CALL", "STATICCALL"):
            continue
        if node.get("from", "").lower() == registry and node.get("to", "").lower() in proxies.values() \
                and node.get("input", "")[2:10].lower() == FORWARD:
            require(path in forwards, "unmatched registry callback")
        if node.get("to", "").lower() != registry or node.get("input", "")[2:10].lower() not in (STATIC_LOOKUP, MUTABLE_LOOKUP):
            continue
        if path in dispatches:
            continue
        require(static and node.get("error") and path, "unmatched registry lookup")
        output = node.get("output", "0x").lower()
        missing = [proxy for proxy in proxies.values() if output == "0x" + MISSING_PROXY + "00" * 12 + address(proxy)]
        require(len(missing) == 1, "unmatched lookup is not an exact missing-proxy probe")
        proxy = missing[0]
        outer = by_path[path[:-1]][0]
        require(node.get("from", "").lower() == outer.get("to", "").lower() and
                node.get("input", "").lower() == lookup_input(outer["from"], outer["input"], True),
                "missing-proxy probe lacks exact registry dispatch")
        retry = [(i, p) for i, p in selected.items() if actions[i]["source_chain"] == 0 and p > path
                 and by_path[p][0].get("input", "").lower() == outer.get("input", "").lower()
                 and by_path[p][0].get("to", "").lower() == outer.get("to", "").lower()
                 and by_path[p][0].get("from", "").lower() == outer.get("from", "").lower()]
        require(len(retry) == 1 and outer.get("error") and outer.get("output", "0x").lower() == output,
                "missing-proxy probe lacks its exact retry")
        _, retry_path = retry[0]
        creators = [(p, n) for p, n, _ in frames if path < p < retry_path and n.get("type") == "CREATE2"
                    and n.get("from", "").lower() == registry and n.get("to", "").lower() == proxy
                    and not n.get("error") and n.get("output", "0x") != "0x"]
        require(len(creators) == 1, "missing callback proxy was not created before retry")
        actor = next(actor for actor, derived in proxies.items() if derived == proxy)
        creation_input = "0x" + CREATE_PROXY + "00" * 12 + address(actor) + word(rollup)
        creator_path = creators[0][0]
        require(any(p < creator_path and creator_path[:len(p)] == p and n.get("to", "").lower() == registry
                    and n.get("type") == "CALL" and not n.get("error")
                    and n.get("from", "").lower() == outer.get("from", "").lower()
                    and n.get("input", "").lower() == creation_input
                    and n.get("output", "").lower() == "0x" + "00" * 12 + address(proxy)
                    for p, n, _ in frames), "proxy creation lacks its registry call")
        require(any(actions[i]["source_chain"] == rollup and
                    proxies[actions[i]["from_address"].lower()] == proxy and p[:len(retry_path)] == retry_path
                    for i, p in selected.items()), "created proxy is not used by the retried lookup")
        probes.append({"path": list(path), "created_proxy": proxy})
    return {"verified": True, "action_count": len(actions),
            "action_trace_paths": [list(selected[i]) for i in range(len(actions))], "missing_proxy_probes": probes}


def lookup_entries(document):
    """Select the posted deferred entries whose roots need canonical binding.

    Full callback/table reconstruction is the native blob verifier's job. These
    keys bind that authenticated table to the trace and enumerate every required
    root; the shell caller independently proves each root from L2 execution.
    """
    batch = document["decoded_settlement"][0]
    actions = document["semantic_transaction"]["actions"]
    rollup = document["rollup_id"]
    selected = []
    for action in actions:
        if action["source_chain"] != 0:
            continue
        parent = action["parent_index"]
        while parent is not None:
            require(actions[parent]["kind"] == "StaticCall", "nested mutable host requires row verification")
            parent = actions[parent]["parent_index"]
        static = action["kind"] == "StaticCall"
        encoded = subprocess.check_output([
            "cast", "abi-encode", "f(bool,address,uint64,address,uint64,uint256,uint64,bytes)",
            str(static).lower(), action["from_address"], "0", action["to_address"], str(rollup),
            action["value"], "0", action["data"],
        ], text=True).strip()
        key = subprocess.check_output(["cast", "keccak", encoded], text=True).strip().lower()
        entries, immediate = (batch[2], int(batch[4])) if static else (batch[1], int(batch[3]))
        require(0 <= immediate <= len(entries), "invalid immediate entry count")
        matches = [(index, entry) for index, entry in enumerate(entries) if index >= immediate
                   and entry[1].lower() == key and int(entry[-3]) == rollup
                   and entry[-2] == action["success"] and entry[-1].lower() == action["return_data"].lower()]
        require(len(matches) == 1, "lookup does not select one deferred posted entry")
        index, entry = matches[0]
        require(len(entry[0]) == 1 and int(entry[0][0][0]) == rollup, "lookup has unexpected root pins")
        if static:
            root = entry[0][0][1]
        else:
            update = entry[0][0]
            require(int(update[1]) == 0 and update[2].lower() == update[3].lower(),
                    "lookup changes the destination root or balance")
            root = update[2]
        hex_bytes(root, 32)
        selected.append({"action_index": action["index"], "key": key, "root": root.lower(),
                         "static": static, "entry_index": index})
    require(selected, "no deferred lookup entries")
    return {"key": selected[0]["key"], "root": selected[0]["root"], "entries": selected,
            "keys": list(dict.fromkeys(entry["key"] for entry in selected)),
            "roots": list(dict.fromkeys(entry["root"] for entry in selected))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    args = parser.parse_args()
    try:
        document = json.loads(args.input.read_text())
        result = verify(document)
        if "decoded_settlement" in document:
            result = {"lookup_trace": result, "lookup_entry": lookup_entries(document)}
    except (KeyError, TypeError, ValueError) as error:
        parser.exit(1, f"lookup trace verification failed: {error}\n")
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
