"""Lookup-only evidence: real shell/jq, mocked RPC/KZG/Forge boundaries, no network."""
import copy
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
SCRIPTS = REPO / "testing/kurtosis/scripts"
CAPTURED_LOGS = Path(__file__).resolve().parent / "fixtures/protocol-lookup"
ZERO = "0x" + "0" * 64
KEY, ROOT, L1_HASH, USER, POST = ["0x" + char * 64 for char in "12345"]
SOURCE, DESTINATION, REGISTRY, MANAGER = ["0x" + char * 40 for char in "abcd"]
PROXY = "0x" + "e" * 40
LOAD = "0x" + "7" * 64
PARENT = "0x" + "8" * 64
RETURN = "0x" + "0" * 63 + "1"
REVERT = "0x08c379a0"  # Fixture data; the real collector computes ABI Error(string).
ABI_RETURN = "0x" + f"{32:064x}{32:064x}{1:064x}"
# topLevelStaticReentrantCounter's callback: CALLBACK_L2 reads COUNTER_L1
# through the registry's proxy for CALLBACK_L2.
CALLBACK_SOURCE, CALLBACK_TARGET, CALLBACK_PROXY = ["0x" + char * 40 for char in "f95"]
CALLBACK_FOLD = "0x" + "6" * 64  # The mocked `cast keccak` result.
PROXY_QUERY = "0xeb20c0aa" + "0" * 24 + "f" * 40 + "0" * 63 + "1"
PROXY_QUERY_RESULT = "0x" + "0" * 24 + "5" * 40
STATIC_LOOKUPS = ("topLevelStaticCounter", "topLevelStaticReentrantCounter")


def fixture(scenario):
    static = scenario in STATIC_LOOKUPS
    callback = scenario == "topLevelStaticReentrantCounter"
    result = RETURN if static else REVERT
    mutable = [[[1, 0, ROOT, ROOT]], KEY, [], [], "0x" + "6" * 64, 1, False, result]
    callbacks = [[0, True, 0, CALLBACK_SOURCE, 1, CALLBACK_TARGET, 0, "0x61bc221a"]] if callback else []
    static_entry = [[[1, ROOT]], KEY, callbacks, CALLBACK_FOLD if callback else ZERO, 1, True, result]
    callback_action = {
        "parent_index": 0, "kind": "StaticCall", "source_chain": 1, "target_chain": 0,
        "from_address": CALLBACK_SOURCE, "to_address": CALLBACK_TARGET, "value": "0",
        "data": "0x61bc221a", "success": True, "return_data": result, "revert_span": None,
    }
    # callTracer shape: the registry STATICCALLs the callback's source proxy,
    # which forwards one CALL (read-only under the STATICCALL) to the producer.
    callback_frames = [{"type": "STATICCALL", "from": REGISTRY, "to": CALLBACK_PROXY, "calls": [
        {"type": "CALL", "from": CALLBACK_PROXY, "to": CALLBACK_TARGET, "value": "0x0",
         "input": "0x61bc221a", "output": result}]}] if callback else []
    post = dict(hash=POST, to=REGISTRY, input="0xpost", blockHash=L1_HASH,
                blockNumber="0x10", transactionIndex="0x0")
    user = dict(hash=USER, to=SOURCE, input="0xd09de08a" if static else "0x11111111",
                blockHash=L1_HASH, blockNumber="0x10", transactionIndex="0x1")
    receipt = lambda tx: dict(transactionHash=tx["hash"], blockHash=L1_HASH,
                              blockNumber="0x10", transactionIndex=tx["transactionIndex"], status="0x1")
    return {
        "static": static, "transactions": {POST: post, USER: user},
        "receipts": {POST: receipt(post), USER: receipt(user)},
        "block": dict(hash=L1_HASH, number="0x10", transactions=[copy.deepcopy(post), copy.deepcopy(user)]),
        "decoded": [[[], [] if static else [mutable], [static_entry] if static else [], 0, 0]],
        "blob": {
            "verified": True,
            "l1_block_hash": L1_HASH,
            "settlement_claim": dict(pre_block_hash=PARENT, post_block_hash=ROOT),
            "payload": {
                "block_count": 1,
                "count_runs": [dict(blocks=1, transactions_per_block=0)],
                # L1-origin brackets publish no transaction bytes.
                "semantic_transactions": [{"origin_chain": 0, "tx_data": "0x", "actions": [{
                    "parent_index": None, "kind": "StaticCall" if static else "Call",
                    "source_chain": 0, "target_chain": 1, "from_address": SOURCE,
                    "to_address": DESTINATION, "value": "0", "data": "0x61bc221a" if static else "0xd09de08a",
                    "success": static, "return_data": result, "revert_span": None,
                }] + ([callback_action] if callback else [])}],
            },
            "semantic_evidence": {"raw_trigger_transactions": [{"raw_transaction": "0xraw"}],
                                  "trigger_binding": "settlement_table",
                                  "semantics_bound_to_settlement": True},
        },
        "trace": {"type":"CALL", "to":SOURCE, "input":user["input"], "calls":([
            # StaticRoundTripReader asks the registry for the callback proxy first.
            {"type":"STATICCALL", "from":SOURCE, "to":REGISTRY, "input":PROXY_QUERY,
             "output":PROXY_QUERY_RESULT}] if callback else []) + [
            {"type":"STATICCALL" if static else "CALL", "from":SOURCE, "to":PROXY, "calls":[
                {"type":"STATICCALL", "from":PROXY, "to":REGISTRY,
                 "input":"0xb7ab4572", "output":"0x" + f"{10000:064x}"},
                {"type":"CALL", "from":PROXY, "to":PROXY, "input":"0x9f149e1b",
                 **({"error":"write protection"} if static else {})},
                {"type":"STATICCALL" if static else "CALL", "from":PROXY, "to":REGISTRY,
                 "input":"0xstaticlookup" if static else "0xmutablelookup", "value":"0x0",
                 "output":ABI_RETURN if static else REVERT, "calls": callback_frames,
                 **({} if static else {"error":"execution reverted"})}]}]},
        "l2_hash": ROOT, "l2_head": 100,
    }


def local_write_fixture():
    data = fixture("topLevelStaticCounter")
    data["local_write"] = True
    returns = ["0x" + f"{n:064x}" for n in (1, 2, 1)]
    callbacks = [[0, True, 0, DESTINATION, 1, SOURCE, 0, "0x2c4e722e"]]
    data["decoded"][0][2] = [
        [[[1, ROOT]], KEY, copy.deepcopy(callbacks), "0x" + ("6" if n == 1 else "7") * 64,
         1, True, result] for n, result in zip((1, 2, 1), returns)]
    data["transactions"][USER]["input"] = data["block"]["transactions"][1]["input"] = "0xc0406226"
    data["trace"] = {"type": "CALL", "to": SOURCE, "input": "0xc0406226", "calls": []}
    actions = []
    for i, result in enumerate(returns):
        read = {"type": "CALL", "from": PROXY, "to": SOURCE,
                "input": "0x2c4e722e", "output": result}
        callback = {"type": "STATICCALL", "from": REGISTRY, "to": PROXY,
                    "input": "0xcallback", "output": result, "calls": [read]}
        data["trace"]["calls"].append({
            "type": "STATICCALL", "from": SOURCE, "to": PROXY, "input": "0x999b93af", "output": result,
            "calls": [
                {"type": "STATICCALL", "from": PROXY, "to": REGISTRY,
                 "input": "0xb7ab4572", "output": "0x" + f"{1000:064x}"},
                {"type": "CALL", "from": PROXY, "to": PROXY,
                 "input": "0x9f149e1b", "error": "StateChangeDuringStaticCall"},
                {"type": "STATICCALL", "from": PROXY, "to": REGISTRY,
                 "input": "0xstaticlookup", "output": "0x" + f"{32:064x}{32:064x}" + result[2:],
                 "calls": [copy.deepcopy(callback) for _ in range(2 if i == 1 else 1)]}]})
        for callback_action in (False, True):
            actions.append({"index": len(actions), "parent_index": 2 * i if callback_action else None,
                            "kind": "StaticCall", "source_chain": 1 if callback_action else 0,
                            "target_chain": 0 if callback_action else 1,
                            "from_address": DESTINATION if callback_action else SOURCE,
                            "to_address": SOURCE if callback_action else DESTINATION, "value": "0",
                            "data": "0x2c4e722e" if callback_action else "0x999b93af",
                            "success": True, "result_kind": "ReturnSuccess",
                            "return_data": result, "revert_span": None})
    data["blob"]["payload"]["semantic_transactions"][0]["actions"] = actions
    return data


MOCK_TOOL = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
f = json.loads(pathlib.Path(os.environ["LOOKUP_FIXTURE"]).read_text())
args = sys.argv[1:]
with open(os.environ["LOOKUP_CALLS"], "a") as out:
    out.write(json.dumps([pathlib.Path(sys.argv[0]).name] + args) + "\n")
if pathlib.Path(sys.argv[0]).name == "forge":
    assert "Verify.s.sol:VerifyL2Absent" in args[1], args
    first = "100"
    end = str(f["l2_head"])
    assert "--broadcast" not in args and args[-4:] == [first, end, "0x" + "d"*40, "[0x" + "1"*64 + "]"], args
    sys.exit(1 if f.get("unexpected_delivery") else 0)
if pathlib.Path(sys.argv[0]).name == "causal":
    request = json.loads(pathlib.Path(args[0]).read_text())
    assert request["select_static"] and request["direction"] == "L2_to_L1", request
    assert request["expected_l2_entry_hashes"] == [], request
    assert request["expected_l2_static_key"] == "0x" + "1"*64, request
    if f.get("causal_failure"): sys.exit(1)
    pathlib.Path(args[1]).write_text(json.dumps({"verified": True, "pairs": ["fixture-pair"]}))
    sys.exit(0)
if pathlib.Path(sys.argv[0]).name == "blob":
    assert args[0] == "--prefixes", args
    if f.get("prefix_failure"): sys.exit(1)
    request = json.loads(pathlib.Path(args[1]).read_text())
    assert request["genesis"] == {"config": {}, "timestamp": "0x1234"}, request
    assert request["genesis_hash"] == "0x" + "8"*64, request
    assert request["chain_config"] == {}, request
    assert request["l1_chain_config"] == f.get("l1_chain_config", {"config": {"chainId": 7331}, "timestamp": "0x1230"}), request
    assert request["protocol_config_hash"] == "0x" + "9"*64, request
    assert request["l1_headers_rlp"] == f.get("expected_l1_headers", ["0xc001"]), request
    print(json.dumps({"verified":True, "block_hash":request["block_hash"], "prefixes":f.get("prefixes", [])}))
    sys.exit(0)
command = args[0]
if command == "sig":
    value = {"counter()": "0x61bc221a", "increment()": "0xd09de08a", "incrementProxy()": "0x11111111",
             "postAndVerifyBatch(bytes)": "0xpost", "STATIC_CHECK_GAS()": "0xb7ab4572",
             "run()": "0xc0406226", "quote()": "0x999b93af", "rate()": "0x2c4e722e"}[args[1]]
elif command == "calldata":
    if args[1] == "Error(string)":
        assert args[1:] == ["Error(string)", "always reverts"], args
        value = "0x08c379a0"
    elif args[1] == "executeOnBehalf(address,uint64,bytes)":
        assert args[2:] == ["0x" + "a"*40, "0", "0x2c4e722e"], args
        value = "0xcallback"
    elif args[1] == "computeCrossChainProxyAddress(address,uint64)":
        value = "0xeb20c0aa" + "0" * 24 + args[2][2:] + f"{int(args[3]):064x}"
    else:
        assert args[1] in ("staticCrossChainCall(address,bytes)", "executeCrossChainCall(address,bytes)"), args
        value = "0xstaticlookup" if args[1].startswith("static") else "0xmutablelookup"
elif command == "abi-encode":
    if args[1] == "f(bytes)": value = "0x" + f"{32:064x}{32:064x}" + args[2][2:]
    elif args[1] == "f(address)": value = "0x" + "0" * 24 + args[2][2:]
    elif args[1] == "f(uint256)": value = "0x" + f"{int(args[2]):064x}"
    elif args[1] == "f(bool,address,uint64,address,uint64,uint256,uint64,bytes)":
        assert args[2:] == ["true", "0x" + "a"*40, "0", "0x" + "b"*40, "1", "0", "0", "0x999b93af"], args
        value = "0xidentity"
    else: value = "0x00"
elif command == "keccak":
    if args[1] == "0xidentity": value = "0x" + "1"*64
    elif f.get("local_write") and args[1] == "0x" + "0"*64 + "01" + f"{2:064x}":
        value = "0x" + "7"*64
    else: value = "0x" + "6"*64
elif command == "call":
    sig = args[2]
    if sig.startswith("computeCrossChainCallHash("):
        value = "0x" + "1"*64
    elif sig.startswith("computeCrossChainProxyAddress("):
        value = "0x" + "5"*40 if "0x" + "f"*40 in args else "0x" + "e"*40
    elif sig in ("rate()(uint256)", "quoteAtRate1()(uint256)", "quoteAtRate2()(uint256)",
                  "quoteAfterResetToRate1()(uint256)", "runs()(uint256)"):
        value = f.get("reader_state", {}).get(sig, "2" if sig == "quoteAtRate2()(uint256)" else "1")
    elif sig == "lastCallFailed()(bool)":
        value = f.get("last_failed", "true")
    elif sig == "targetCounter()(uint256)":
        value = f.get("target_counter", "0")
    elif sig == "lastRead()(uint256)":
        value = f.get("last_read", "1")
    elif sig == "rollups(uint64)(address,bytes32,uint256)":
        assert args[args.index("--block") + 1] == "0x" + "3"*64, args
        value = "0x" + "c"*40 + "\n" + "0x" + "8"*64 + "\n0"
    elif sig == "protocolConfigHash()(bytes32)":
        assert args[1] == "0x" + "c"*40, args
        assert args[args.index("--block") + 1] == "0x" + "3"*64, args
        value = "0x" + "9"*64
    else:
        assert sig == "counter()(uint256)", args
        value = f.get("counter", "1")
elif command == "rpc":
    rpc = args[1]
    if rpc == "eth_getTransactionByHash": value = f["transactions"][args[2]]
    elif rpc == "eth_getTransactionReceipt": value = f["receipts"][args[2]]
    elif rpc == "eth_getBlockByNumber":
        value = dict(hash="0x" + "8"*64, number="0x0") if args[2] == "0x0" else f["block"]
    elif rpc == "eth_getBlockByHash":
        value = (dict(hash="0x" + "8"*64, number="0x63") if args[2] == "0x" + "8"*64
                 else dict(hash=f["l2_hash"], number=hex(f["l2_head"]), parentHash="0x" + "8"*64))
    elif rpc == "eth_getStorageAt":
        assert args[2:4] == ["0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077", "0x2"], args
        value = f.get("history_first" if args[4] == "0x" + "8"*64 else "history_last", "0x" + f"{1:064x}")
    elif rpc == "debug_getRawHeader":
        value = f.get("raw_l1_headers", {}).get(args[2], "0xc0" + f"{int(args[2], 16):02x}")
    elif rpc == "debug_traceTransaction":
        assert args[2] == "0x" + "4"*64 and json.loads(args[3])["tracer"] == "callTracer", args
        if f.get("trace_unavailable"): sys.exit(1)
        value = f["trace"]
    elif rpc == "debug_getRawBlock": value = "0x00"
    elif rpc == "debug_getRawReceipts": value = []
    elif rpc in ("eez_executionWitnessByBlockHash", "debug_chainConfig"): value = {}
    else: raise AssertionError(args)
elif command == "to-dec": value = str(int(args[1], 0))
elif command == "decode-calldata": value = f["load_decoded"] if args[2].startswith("loadExecutionTable(") else f["decoded"]
elif command == "block-number": value = str(f["l2_head"])
elif command == "block":
    if "--full" in args: value = f["l2_block"]
    elif args[-1] == "http://l2.invalid": value = "0x" + "8"*64 if args[1] == "99" else f["l2_hash"]
    else: value = f.get("final_l1_hash", f["block"]["hash"])
else: raise AssertionError(args)  # Includes any accidental transaction broadcast.
print(json.dumps(value) if command == "rpc" or isinstance(value, (dict, list)) else value)
'''


@unittest.skipUnless(shutil.which("jq"), "jq required by CI evidence scripts")
class ProtocolLookupEvidenceTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-lookup-test.")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.scripts = self.root / "testing/kurtosis/scripts"
        self.scripts.mkdir(parents=True)
        for name in ("verify-l1-lookup.sh", "lookup-entry.jq", "lookup-trace.jq",
                     "verify-lookup-root.sh", "verify-static-counter-l2.sh"):
            shutil.copy2(SCRIPTS / name, self.scripts / name)
        artifact = self.root / "eez-core-protocol/out/EEZ.sol/EEZ.json"
        artifact.parent.mkdir(parents=True)
        artifact.write_text(json.dumps({"methodIdentifiers": {"postAndVerifyBatch(bytes)": "00000000"}}))
        tools = self.root / "bin"
        tools.mkdir()
        for name in ("cast", "forge", "causal", "blob"):
            tool = tools / name
            tool.write_text(MOCK_TOOL)
            tool.chmod(0o755)
        # Existing KZG/blob binding helper is tested separately. This stub
        # models its authenticated output; these tests exercise its consumer.
        (self.scripts / "verify-semantic-settlement-blob.sh").write_text('''#!/bin/bash
set -eu
[[ "${LOOKUP_BLOB_FAIL:-0}" == 0 ]] || exit 1
jq '.blob' "$LOOKUP_FIXTURE" >"$3"
''')
        self.log = self.root / "scenario.log"
        self.data = self.root / "fixture.json"
        self.output = self.root / "evidence.json"
        self.calls = self.root / "calls.jsonl"
        self.genesis = self.root / "genesis.json"
        self.genesis.write_text(json.dumps({"config": {}, "timestamp": "0x1234"}))
        self.l1_genesis = self.root / "l1-genesis.json"
        self.l1_genesis.write_text(json.dumps({"config": {"chainId": 7331}, "timestamp": "0x1230"}))
        self.env = {**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"],
                    "LOOKUP_FIXTURE": str(self.data), "LOOKUP_CALLS": str(self.calls),
                    "L1_RPC": "http://l1.invalid", "L2_RPC": "http://l2.invalid",
                    "L2_FRONT": "http://composer.invalid", "ROLLUPS": REGISTRY,
                    "MANAGER_L2": MANAGER, "ROLLUP_ID": "1", "PROTOCOL_COMMIT": "fixture-pin",
                    "CAUSAL_VERIFIER": str(tools / "causal"),
                    "BEACON_URL": "http://beacon.invalid", "BLOB_VERIFIER": str(tools / "blob"),
                    "EEZ_L2_GENESIS": str(self.genesis), "EEZ_L1_CHAIN_CONFIG": str(self.l1_genesis)}

    def run_fixture(self, scenario, data=None, complete=True, log_transform=None):
        self.data.write_text(json.dumps(data or (local_write_fixture() if scenario == "staticLocalWrite" else fixture(scenario))))
        self.output.unlink(missing_ok=True)
        # Preserve real upstream text. Substitute only identities handled by
        # mocked RPCs, so invented PASS markers cannot hide collector drift.
        log = (CAPTURED_LOGS / f"{scenario}.txt").read_text()
        for field, replacement in (("READER", SOURCE), ("QUOTE", DESTINATION),
                                   ("READER_L1", SOURCE), ("COUNTER_L2", DESTINATION),
                                   ("SAFE_CALLER", SOURCE), ("REV_COUNTER_L2", DESTINATION),
                                   ("CALLBACK_L2", CALLBACK_SOURCE), ("COUNTER_L1", CALLBACK_TARGET),
                                   ("CALLBACK_PROXY", CALLBACK_PROXY)):
            if match := re.search(rf"^{field}=(0x[0-9a-fA-F]+)$", log, re.MULTILINE):
                log = log.replace(match[1], replacement)
        for field, replacement in (("User tx", USER), ("L1 postBatch", POST)):
            original = re.search(rf"^{field}:\s*(0x[0-9a-fA-F]+)", log, re.MULTILINE)[1]
            log = log.replace(original, replacement)
        if not complete:
            log = log.replace("====== Done ======", "ERROR: failed upstream")
        self.log.write_text(log_transform(log) if log_transform else log)
        return subprocess.run(["bash", str(self.scripts / "verify-l1-lookup.sh"), scenario,
                               "scenario.sol", str(self.log), str(self.output)],
                              env=self.env, capture_output=True, text=True, timeout=15)

    def test_static_local_write_dispatches_to_independent_lookup_verification(self):
        scenario = "staticLocalWrite"
        setup = self.run_fixture(scenario)
        self.assertEqual(setup.returncode, 0, setup.stdout + setup.stderr)
        shutil.copy2(SCRIPTS / "collect-protocol-e2e-evidence.sh", self.scripts)
        (self.root / "scenario.sol").write_text("contract ExecuteNetwork {}\n")
        self.output.unlink()
        result = subprocess.run(
            ["bash", str(self.scripts / "collect-protocol-e2e-evidence.sh"), scenario,
             str(self.root / "scenario.sol"), str(self.log), str(self.output)],
            env=self.env, capture_output=True, text=True, timeout=15, cwd=self.root)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads(self.output.read_text())
        self.assertTrue(evidence["lookup_trace"]["first_candidate_reused"])
        self.assertEqual(evidence["system_transactions"], [])
        self.assertEqual([read["callback_attempts"] for read in evidence["lookup_trace"]["reads"]], [1, 2, 1])

    def test_static_local_write_preserves_repeated_rows_and_allows_complete_deduplication(self):
        for count in (2, 3):
            with self.subTest(serialized_rows=count):
                data = local_write_fixture()
                data["decoded"][0][2] = data["decoded"][0][2][:count]
                result = self.run_fixture("staticLocalWrite", data)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                evidence = json.loads(self.output.read_text())
                self.assertEqual(len(evidence["lookup_entry"]["candidates"]), count)
                self.assertEqual(evidence["lookup_entry"]["selected_candidate_indices"], [0, 1, 0])
                self.assertTrue(evidence["claims"]["l2_absence_verified"])

    def test_static_local_write_counts_duplicate_candidates_before_the_match(self):
        data = local_write_fixture()
        rows = data["decoded"][0][2]
        rows.insert(1, copy.deepcopy(rows[0]))  # A/A/B/A: second read tries A twice, then B.
        callbacks = data["trace"]["calls"][1]["calls"][2]["calls"]
        callbacks.insert(0, copy.deepcopy(callbacks[0]))
        result = self.run_fixture("staticLocalWrite", data)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads(self.output.read_text())
        self.assertEqual(evidence["lookup_entry"]["selected_candidate_indices"], [0, 2, 0])
        self.assertEqual([read["callback_attempts"] for read in evidence["lookup_trace"]["reads"]], [1, 3, 1])
        callbacks.pop()
        result = self.run_fixture("staticLocalWrite", data)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("actual registry lookup returned the wrong", result.stderr)
        self.assertFalse(self.output.exists())

    def test_static_local_write_rejects_wrong_candidate_fields(self):
        cases = {
            "key": lambda d: d["decoded"][0][2][1].__setitem__(1, ZERO),
            "result": lambda d: d["decoded"][0][2][1].__setitem__(6, RETURN),
            "fold": lambda d: d["decoded"][0][2][1].__setitem__(3, CALLBACK_FOLD),
            "root": lambda d: d["decoded"][0][2][1][0][0].__setitem__(1, ZERO),
            "source": lambda d: d["decoded"][0][2][1][2][0].__setitem__(3, SOURCE),
            "target": lambda d: d["decoded"][0][2][1][2][0].__setitem__(5, DESTINATION),
            "callback selector": lambda d: d["decoded"][0][2][1][2][0].__setitem__(7, "0x999b93af"),
            "mutable callback": lambda d: d["decoded"][0][2][1][2][0].__setitem__(1, False),
            "missing callback": lambda d: d["decoded"][0][2][1].__setitem__(2, []),
            "extra callback": lambda d: d["decoded"][0][2][1][2].append(copy.deepcopy(d["decoded"][0][2][1][2][0])),
            "gas": lambda d: d["decoded"][0][2][1][2][0].__setitem__(6, 1),
            "value": lambda d: d["decoded"][0][2][1][2][0].__setitem__(2, 1),
            "rollback": lambda d: d["decoded"][0][2][1][2][0].__setitem__(0, 1),
            "destination rollup": lambda d: d["decoded"][0][2][1].__setitem__(4, 2),
            "reverting candidate": lambda d: d["decoded"][0][2][1].__setitem__(5, False),
            "transient-only candidate": lambda d: d["decoded"][0].__setitem__(4, 2),
            "ambiguous repeated row": lambda d: d["decoded"][0][2][2].__setitem__(6, ZERO),
        }
        self.assert_local_write_rejections(cases, "posted lookup has the wrong")

    def assert_local_write_rejections(self, cases, message):
        for name, mutate in cases.items():
            with self.subTest(mutation=name):
                data = local_write_fixture()
                mutate(data)
                result = self.run_fixture("staticLocalWrite", data)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(message, result.stderr)
                self.assertFalse(self.output.exists())

    def test_static_local_write_rejects_missing_retry_or_reuse_and_wrong_trace(self):
        def callbacks(data, read=1):
            return data["trace"]["calls"][read]["calls"][2]["calls"]
        cases = {
            "missing failed candidate": lambda d: callbacks(d).pop(0),
            "extra candidate": lambda d: callbacks(d).append(copy.deepcopy(callbacks(d)[0])),
            "no callback": lambda d: callbacks(d, 0).clear(),
            "failed candidate result": lambda d: callbacks(d)[0]["calls"][0].update(output=RETURN),
            "selected candidate result": lambda d: callbacks(d)[1].update(output=RETURN),
            "callback target": lambda d: callbacks(d)[0]["calls"][0].update(to=DESTINATION),
            "callback selector": lambda d: callbacks(d)[0]["calls"][0].update(input="0x999b93af"),
            "callback forwarding": lambda d: callbacks(d)[0].update(input="0xwrong"),
            "callback caller": lambda d: callbacks(d)[0].update(**{"from": SOURCE}),
            "mutable callback": lambda d: callbacks(d)[0].update(type="CALL"),
            "callback reverted": lambda d: callbacks(d)[0]["calls"][0].update(error="execution reverted"),
            "third read not reused": lambda d: callbacks(d, 2).append(copy.deepcopy(callbacks(d, 2)[0])),
            "second returned first result": lambda d: d["trace"]["calls"][1].update(output=RETURN),
            "lookup calldata": lambda d: d["trace"]["calls"][1]["calls"][2].update(input="0xwrong"),
            "lookup returned first result": lambda d: d["trace"]["calls"][1]["calls"][2].update(output=ABI_RETURN),
            "extra direct call": lambda d: d["trace"]["calls"].append(copy.deepcopy(d["trace"]["calls"][0])),
        }
        self.assert_local_write_rejections(cases, "actual registry lookup returned the wrong")

    def test_static_local_write_binds_exactly_three_logical_reads_and_callbacks(self):
        def actions(data):
            return data["blob"]["payload"]["semantic_transactions"][0]["actions"]
        cases = {
            "result": lambda d: actions(d)[2].update(return_data=RETURN),
            "callback result": lambda d: actions(d)[3].update(return_data=RETURN),
            "callback parent": lambda d: actions(d)[3].update(parent_index=0),
            "quote parent": lambda d: actions(d)[2].update(parent_index=0),
            "index": lambda d: actions(d)[4].update(index=0),
            "identity": lambda d: actions(d)[2].update(from_address=DESTINATION),
            "callback identity": lambda d: actions(d)[3].update(from_address=SOURCE),
            "callback data": lambda d: actions(d)[3].update(data="0x999b93af"),
            "candidate retry became logical action": lambda d: actions(d).insert(3, copy.deepcopy(actions(d)[3])),
            "missing logical read": lambda d: actions(d).pop(),
            "unbound": lambda d: d["blob"]["semantic_evidence"].update(semantics_bound_to_settlement=False),
            "duplicate bracket": lambda d: d["blob"]["payload"]["semantic_transactions"].append(
                copy.deepcopy(d["blob"]["payload"]["semantic_transactions"][0])),
        }
        self.assert_local_write_rejections(cases, "lookup is not bound to the authenticated semantic action")

    def test_captured_static_local_write_trace_verifies_candidate_retries(self):
        trace = json.loads((CAPTURED_LOGS / "staticLocalWrite-trace.json").read_text())
        source = "0xa9906ad40c824ecf61db7c07ae728e19cfb5f440"
        registry = "0xe7f1725e7734ce288f8367e1bb143e90bb3f0512"
        proxy = "0x86f01af18e9e94d64e7cebf2d490beb7011c6d8f"
        word = lambda value: f"{value:064x}"
        # ABI encodings from the fixed scenario, independently of trace fields.
        lookup = "0x31344ade" + source[2:].zfill(64) + word(64) + word(4) + "999b93af".ljust(64, "0")
        forward = "0x8205f3e1" + source[2:].zfill(64) + word(0) + word(96) + word(4) + "2c4e722e".ljust(64, "0")
        arguments = {
            "scenario": "staticLocalWrite", "source": source, "registry": registry, "proxy": proxy,
            "trigger_input": "0xc0406226", "lookup_input": lookup, "call_input": "0x999b93af",
            "static_check_gas_input": "0xb7ab4572", "callback_input": "0x2c4e722e",
            "callback_forward_input": forward, "callback_output": RETURN, "result_two": "0x" + word(2),
            "output": ABI_RETURN,
        }
        command = ["jq", "-ce", "--argjson", "static", "true",
                   "--argjson", "selected_candidates", "[0,1,0]"]
        for key, value in arguments.items():
            command += ["--arg", key, value]
        command += ["-f", str(SCRIPTS / "lookup-trace.jq")]
        for mutation in (None, "delegate_output", "delegate_sender", "extra_callback"):
            with self.subTest(mutation=mutation):
                candidate = copy.deepcopy(trace)
                forwarded = candidate["calls"][1]["calls"][2]["calls"][0]
                if mutation == "delegate_output": forwarded["output"] = ABI_RETURN
                if mutation == "delegate_sender": forwarded["from"] = source
                if mutation == "extra_callback": forwarded["calls"].append(copy.deepcopy(forwarded["calls"][0]))
                result = subprocess.run(command, input=json.dumps(candidate), text=True, capture_output=True)
                if mutation is None:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual([read["callback_attempts"] for read in json.loads(result.stdout)["reads"]], [1, 2, 1])
                else:
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_static_local_write_requires_root_state_and_absence(self):
        self.assert_local_write_rejections({
            "unbound root": lambda d: [row[0][0].__setitem__(1, ZERO) for row in d["decoded"][0][2]],
        }, "lookup root is not bound to the authenticated L2 execution")
        self.assert_local_write_rejections({
            "wrong persisted quote": lambda d: d.update(reader_state={"quoteAtRate2()(uint256)": "1"}),
        }, "reader persisted the wrong local-write state")
        self.assert_local_write_rejections({
            "unexpected delivery": lambda d: d.update(unexpected_delivery=True),
        }, "lookup appeared on L2")

    def test_both_lookup_modes_accept_complete_canonical_evidence(self):
        for scenario in (*STATIC_LOOKUPS, "revertCounter"):
            with self.subTest(scenario=scenario):
                result = self.run_fixture(scenario)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                evidence = json.loads(self.output.read_text())
                self.assertTrue(evidence["causal_graph"]["verified"])
                self.assertTrue(evidence["claims"]["l2_absence_verified"])
                self.assertEqual(evidence["system_transactions"], [])
                self.assertNotIn("--broadcast", self.calls.read_text())

    def test_v0_window_is_bound_to_canonical_endpoint_heights(self):
        for scenario in ("topLevelStaticCounter", "revertCounter"):
            with self.subTest(scenario=scenario):
                data = fixture(scenario)
                data["blob"]["payload"].update(
                    block_count=2, count_runs=[dict(blocks=2, transactions_per_block=0)])
                data["l2_head"] = 101
                result = self.run_fixture(scenario, data)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                evidence = json.loads(self.output.read_text())
                self.assertEqual(evidence["causal_graph"]["l2_absence_range"],
                                 {"from": 100, "to": 101})

    def test_rejects_malformed_or_unbound_v0_windows(self):
        mutations = {
            "height mismatch": lambda f: f.update(l2_head=101),
            "missing endpoint": lambda f: f["blob"]["settlement_claim"].pop("post_block_hash"),
            "wrong parent": lambda f: f["blob"]["settlement_claim"].update(pre_block_hash=ZERO),
            "oversized count": lambda f: f["blob"]["payload"].update(block_count=4294967296),
            "empty runs": lambda f: f["blob"]["payload"].update(count_runs=[]),
            "run coverage": lambda f: f["blob"]["payload"]["count_runs"][0].update(blocks=2),
            "invalid pure count": lambda f: f["blob"]["payload"]["count_runs"][0].update(transactions_per_block=16384),
        }
        for name, mutate in mutations.items():
            with self.subTest(mutation=name):
                data = fixture("revertCounter")
                mutate(data)
                result = self.run_fixture("revertCounter", data)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertFalse(self.output.exists())

    def test_never_overrides_upstream_failure(self):
        for scenario in ("topLevelStaticCounter", "revertCounter"):
            with self.subTest(scenario=scenario):
                result = self.run_fixture(scenario, complete=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.output.exists())
        self.assertFalse(self.calls.exists(), "failure must be rejected before any RPC")

    def test_requires_the_scenarios_own_table_verification(self):
        mutable = "PASS: posted batch calldata matches expected table"
        static = "PASS: posted batch static entries match expected static table"
        for scenario, marker, wrong_marker in (("topLevelStaticCounter", static, mutable),
                                                ("revertCounter", mutable, static)):
            for replacement in ("NOTE: not verified", wrong_marker):
                with self.subTest(scenario=scenario, replacement=replacement):
                    result = self.run_fixture(scenario, log_transform=lambda log: log.replace(marker, replacement))
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(self.output.exists())
        self.assertFalse(self.calls.exists(), "missing table evidence must fail before RPC")

    def test_requires_absence_check_and_rejects_errors_even_with_done(self):
        for transform in (
            lambda log: log.replace("PASS: none of 1 absent key(s) appears on L2", "NOTE: absence unchecked"),
            lambda log: log + "ERROR: later verification failed\n",
        ):
            for scenario in ("topLevelStaticCounter", "revertCounter"):
                with self.subTest(scenario=scenario):
                    result = self.run_fixture(scenario, log_transform=transform)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(self.output.exists())
        self.assertFalse(self.calls.exists())

    def test_rejects_mutated_or_incomplete_evidence(self):
        mutations = {
            "orphaned receipt": lambda f: f["receipts"][USER].update(blockHash=ZERO),
            "wrong tx input": lambda f: f["transactions"][USER].update(input="0xdeadbeef"),
            "reverted post": lambda f: f["receipts"][POST].update(status="0x0"),
            "missing canonical tx": lambda f: f["block"]["transactions"].pop(),
            "wrong caller state": lambda f: f.update(counter="0"),
            "wrong result": lambda f: f["decoded"][0][1][0].__setitem__(7, "0xdead"),
            "unexpected root change": lambda f: f["decoded"][0][1][0][0][0].__setitem__(3, ZERO),
            "unexpected ether delta": lambda f: f["decoded"][0][1][0][0][0].__setitem__(1, 1),
            "wrong rolling hash": lambda f: f["decoded"][0][1][0].__setitem__(4, ZERO),
            "nested callback": lambda f: f["decoded"][0][1][0][2].append([1]),
            "duplicate entry": lambda f: f["decoded"][0][1].append(copy.deepcopy(f["decoded"][0][1][0])),
            "lookup in immediate prefix": lambda f: f["decoded"][0].__setitem__(3, 1),
            "wrong semantic origin": lambda f: f["blob"]["payload"]["semantic_transactions"][0].update(origin_chain=1),
            "wrong semantic result": lambda f: f["blob"]["payload"]["semantic_transactions"][0]["actions"][0].update(success=True),
            "extra semantic action": lambda f: f["blob"]["payload"]["semantic_transactions"][0]["actions"].append({}),
            "semantic trigger missing": lambda f: f["blob"]["payload"]["semantic_transactions"][0].update(tx_data="0xother"),
            "wrong blob root": lambda f: f["blob"]["settlement_claim"].update(post_block_hash=ZERO),
            "orphaned L2 block": lambda f: f.update(l2_hash=ZERO),
            "unexpected L2 delivery": lambda f: f.update(unexpected_delivery=True),
            "L1 reorg during collection": lambda f: f.update(final_l1_hash=ZERO),
            "revert not caught": lambda f: f.update(last_failed="false"),
        }
        for name, mutate in mutations.items():
            with self.subTest(mutation=name):
                data = fixture("revertCounter")
                mutate(data)
                result = self.run_fixture("revertCounter", data)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertFalse(self.output.exists())

    def test_static_lookup_does_not_requery_the_expired_pool(self):
        result = self.run_fixture("topLevelStaticCounter")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
        self.assertFalse(any(call[:2] == ["cast", "call"] and call[3] == "increment()(uint256)" for call in calls))

    def test_blob_failure_cannot_become_a_pass(self):
        self.env["LOOKUP_BLOB_FAIL"] = "1"
        result = self.run_fixture("revertCounter")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())

    def test_static_entry_shape_is_checked_field_by_field(self):
        for index, replacement in ((0, []), (1, ZERO), (2, [[1]]), (3, KEY),
                                   (4, 2), (5, False), (6, "0xdeadbeef")):
            with self.subTest(index=index):
                data = fixture("topLevelStaticCounter")
                data["decoded"][0][2][0][index] = replacement
                result = self.run_fixture("topLevelStaticCounter", data)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.output.exists())

    def test_intervening_batch_cannot_produce_a_false_revert_pass(self):
        data = fixture("revertCounter")
        data["transactions"][USER]["transactionIndex"] = "0x2"
        data["receipts"][USER]["transactionIndex"] = "0x2"
        data["block"]["transactions"][1]["transactionIndex"] = "0x2"
        later = {**data["transactions"][POST], "hash": LOAD, "transactionIndex": "0x1"}
        data["block"]["transactions"].insert(1, later)
        result = self.run_fixture("revertCounter", data)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("another batch replaced", result.stderr)
        self.assertFalse(self.output.exists())

    def test_revert_case_accepts_other_lookups_only_with_exact_executed_revert(self):
        data = fixture("revertCounter")
        other = copy.deepcopy(data["decoded"][0][1][0])
        other[1] = ZERO  # A different scenario's lookup on the same rollup.
        data["decoded"][0][1].append(other)
        result = self.run_fixture("revertCounter", data)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        data["trace"]["calls"][0]["calls"][-1]["output"] = "0xdeadbeef"  # e.g. ExecutionNotFound
        result = self.run_fixture("revertCounter", data)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("actual registry lookup", result.stderr)
        self.assertFalse(self.output.exists())

    def test_lookup_bracket_binding_requires_settlement_tables_and_one_empty_origin(self):
        # L1-origin brackets carry no bytes: the lookup is bound through the
        # settlement tables and must match exactly one empty-origin bracket.
        for scenario in ("topLevelStaticCounter", "revertCounter"):
            for case in ("unbound", "duplicate", "origin_bytes"):
                with self.subTest(scenario=scenario, case=case):
                    data = fixture(scenario)
                    blob = data["blob"]
                    if case == "unbound":
                        blob["semantic_evidence"]["semantics_bound_to_settlement"] = False
                    elif case == "duplicate":
                        transactions = blob["payload"]["semantic_transactions"]
                        transactions.append(copy.deepcopy(transactions[0]))
                    else:
                        blob["payload"]["semantic_transactions"][0]["tx_data"] = "0xraw"
                    result = self.run_fixture(scenario, data)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn("lookup is not bound to the authenticated semantic action", result.stderr)
                    self.assertFalse(self.output.exists())

    def test_canonical_events_proxy_traces_include_the_gas_cap_read(self):
        for scenario in ("topLevelStaticCounter", "revertCounter"):
            with self.subTest(scenario=scenario):
                captured = json.loads((CAPTURED_LOGS / f"{scenario}-events-trace.json").read_text())
                trace = captured.pop("trace")
                args = []
                for key, value in captured.items():
                    args += ["--argjson" if isinstance(value, bool) else "--arg", key,
                             json.dumps(value) if isinstance(value, bool) else value]
                result = subprocess.run(["jq", "-ce", *args, "-f", str(SCRIPTS / "lookup-trace.jq")],
                                        input=json.dumps(trace), text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(json.loads(result.stdout)["verified"])

    def test_real_static_callback_trace_admits_only_the_readers_proxy_query(self):
        """Captured callTracer output of the rc.2 L1-origin callback scenario."""
        trace = json.loads((CAPTURED_LOGS / "topLevelStaticReentrantCounter-trace.json").read_text())
        query, read_proxy = trace["calls"][0], trace["calls"][1]
        lookup = read_proxy["calls"][2]
        callback_proxy = lookup["calls"][0]
        read = callback_proxy["calls"][0]
        named = {
            "source": trace["to"], "registry": query["to"], "proxy": read_proxy["to"],
            "trigger_input": trace["input"], "lookup_input": lookup["input"],
            "output": lookup["output"], "static_check_gas_input": "0xb7ab4572",
            "callback_proxy": callback_proxy["to"], "callback_target": read["to"],
            "callback_input": read["input"], "callback_output": read["output"],
            "proxy_query_input": query["input"], "proxy_query_output": query["output"],
        }

        def accepted(candidate, arguments=named):
            args = ["jq", "-ce", "--argjson", "static", "true"]
            for key, value in arguments.items():
                args += ["--arg", key, value.lower()]
            run = subprocess.run([*args, "-f", str(SCRIPTS / "lookup-trace.jq")],
                                 input=json.dumps(candidate), capture_output=True, text=True)
            return run.returncode == 0

        self.assertTrue(accepted(trace))
        mutations = {
            "wrong query arguments": lambda t: t["calls"][0].__setitem__(
                "input", t["calls"][0]["input"][:-1] + "2"),
            "wrong query result": lambda t: t["calls"][0].__setitem__("output", ZERO),
            "failed query": lambda t: t["calls"][0].__setitem__("error", "execution reverted"),
            "missing query": lambda t: t["calls"].pop(0),
            "extra registry call": lambda t: t["calls"].insert(1, copy.deepcopy(t["calls"][0])),
            "wrong callback result": lambda t: t["calls"][1]["calls"][2]["calls"][0]["calls"][0]
                .__setitem__("output", ZERO),
        }
        for name, mutate in mutations.items():
            with self.subTest(case=name):
                candidate = copy.deepcopy(trace)
                mutate(candidate)
                self.assertFalse(accepted(candidate))
        # Callback-free lookups stay strict: the extra query is not admitted.
        strict = {**named, "callback_proxy": "", "proxy_query_input": "", "proxy_query_output": ""}
        self.assertFalse(accepted(trace, strict))

    def test_lookup_trace_accepts_only_the_registry_proxy_forwarding_frame(self):
        def forward(f):
            read = f["trace"]["calls"][0]["calls"][0]
            read["calls"] = [{"type": "DELEGATECALL", "from": read["to"], "to": "0x" + "ab" * 20,
                              "input": read["input"], "output": read["output"]}]
            return read["calls"][0]

        for scenario in ("revertCounter", "topLevelStaticCounter"):
            with self.subTest(scenario=scenario, case="proxied"):
                data = fixture(scenario)
                forward(data)
                result = self.run_fixture(scenario, data)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            for case, mutate in {
                "plain call": lambda c: c.update(type="CALL"),
                "other input": lambda c: c.update(input="0xwrong"),
                "failed": lambda c: c.update(error="execution reverted"),
                "nested call": lambda c: c.update(calls=[copy.deepcopy(c)]),
            }.items():
                with self.subTest(scenario=scenario, case=case):
                    data = fixture(scenario)
                    mutate(forward(data))
                    result = self.run_fixture(scenario, data)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertFalse(self.output.exists())

    def test_lookup_trace_rejects_wrong_or_ambiguous_execution(self):
        mutations = {
            "missing trace": lambda f: f.update(trace_unavailable=True),
            "outer reverted": lambda f: f["trace"].update(error="execution reverted"),
            "wrong caller": lambda f: f["trace"]["calls"][0].update(**{"from": DESTINATION}),
            "wrong proxy": lambda f: f["trace"]["calls"][0]["calls"][-1].update(**{"from": DESTINATION}),
            "wrong call": lambda f: f["trace"]["calls"][0]["calls"][-1].update(input="0xwrong"),
            "wrong result": lambda f: f["trace"]["calls"][0]["calls"][-1].update(output="0xwrong"),
            "gas failure": lambda f: f["trace"]["calls"][0]["calls"][-1].update(error="out of gas"),
            "wrong gas-cap selector": lambda f: f["trace"]["calls"][0]["calls"][0].update(input="0xwrong"),
            "failed gas-cap read": lambda f: f["trace"]["calls"][0]["calls"][0].update(error="execution reverted"),
            "short gas-cap result": lambda f: f["trace"]["calls"][0]["calls"][0].update(output="0x01"),
            "mutable gas-cap read": lambda f: f["trace"]["calls"][0]["calls"][0].update(type="CALL"),
            "missing gas-cap read": lambda f: f["trace"]["calls"][0]["calls"].pop(0),
            "extra registry read": lambda f: f["trace"]["calls"][0]["calls"].insert(0, copy.deepcopy(f["trace"]["calls"][0]["calls"][0])),
            "reordered registry calls": lambda f: f["trace"]["calls"][0]["calls"].reverse(),
            "duplicate lookup": lambda f: f["trace"]["calls"][0]["calls"].append(copy.deepcopy(f["trace"]["calls"][0]["calls"][-1])),
        }
        for scenario in ("revertCounter", "topLevelStaticCounter"):
            for name, mutate in mutations.items():
                with self.subTest(scenario=scenario, mutation=name):
                    data = fixture(scenario)
                    mutate(data)
                    result = self.run_fixture(scenario, data)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertFalse(self.output.exists())

    def test_lookup_accepts_reconstructed_prefix_but_not_an_unbound_pin(self):
        for scenario in ("revertCounter", "topLevelStaticCounter"):
            for failure in (None, "empty_prefix", "missing", "duplicate", "replay_failure", "missing_genesis", "missing_l1_genesis"):
                with self.subTest(scenario=scenario, failure=failure):
                    data = fixture(scenario)
                    data["blob"]["settlement_claim"]["post_block_hash"] = ZERO
                    data["l2_hash"] = ZERO
                    data["prefixes"] = [dict(hash=ROOT, transaction_index=3, transaction_count=4)]
                    if failure == "empty_prefix":
                        data["prefixes"] = [dict(hash=ROOT, transaction_index=None, transaction_count=0)]
                    if failure == "missing": data["prefixes"] = []
                    if failure == "duplicate": data["prefixes"] *= 2
                    if failure == "replay_failure": data["prefix_failure"] = True
                    self.env["EEZ_L2_GENESIS"] = str(
                        self.root / "absent-genesis.json" if failure == "missing_genesis" else self.genesis)
                    self.env["EEZ_L1_CHAIN_CONFIG"] = str(
                        self.root / "absent-l1-genesis.json" if failure == "missing_l1_genesis" else self.l1_genesis)
                    result = self.run_fixture(scenario, data)
                    if failure not in (None, "empty_prefix"):
                        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertFalse(self.output.exists())
                    else:
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertEqual(json.loads(self.output.read_text())["lookup_root"]["kind"], "execution_prefix")
                        calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
                        witnesses = [call for call in calls if "eez_executionWitnessByBlockHash" in call]
                        self.assertTrue(witnesses)
                        for call in witnesses:
                            self.assertEqual(call[call.index("--rpc-url") + 1], self.env["L2_FRONT"])
                        for call in calls:
                            if any(method in call for method in ("debug_getRawBlock", "debug_getRawReceipts", "debug_chainConfig")):
                                self.assertEqual(call[call.index("--rpc-url") + 1], self.env["L2_RPC"])
                            if any(method in call for method in ("debug_getRawHeader", "protocolConfigHash()(bytes32)")):
                                self.assertEqual(call[call.index("--rpc-url") + 1], self.env["L1_RPC"])

    def test_prefix_history_collection_is_bounded_and_preserves_replay_inputs(self):
        for first, last, expected in ((1, 3, True), (1, 65, True), (1, 66, False),
                                      (2, 1, False), (2**64, 2**64, False), ("0x1", 1, False)):
            with self.subTest(first=first, last=last):
                self.calls.unlink(missing_ok=True)
                data = fixture("revertCounter")
                data["blob"]["settlement_claim"]["post_block_hash"] = ZERO
                data["l2_hash"] = ZERO
                data["prefixes"] = [dict(hash=ROOT, transaction_index=3, transaction_count=4)]
                data["history_first"] = f"0x{first:064x}" if isinstance(first, int) else first
                data["history_last"] = f"0x{last:064x}"
                if expected:
                    data["expected_l1_headers"] = [f"0xc0{n:02x}" for n in range(first, last + 1)]
                result = self.run_fixture("revertCounter", data)
                self.assertEqual(result.returncode == 0, expected, result.stdout + result.stderr)
                calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
                fetched = [call[2] for call in calls if call[1] == "rpc" and call[2] == "debug_getRawHeader"]
                self.assertEqual(len(fetched), last - first + 1 if expected else 0)
                if not expected:
                    self.assertFalse(any(call[0] == "blob" for call in calls))

        # A rejected replay must retain the complete, unmodified evidence input.
        data = fixture("revertCounter")
        data["blob"]["settlement_claim"]["post_block_hash"] = ZERO
        data["l2_hash"] = ZERO
        data["raw_l1_headers"] = {"0x1": "0x00"}
        data["prefix_failure"] = True
        result = self.run_fixture("revertCounter", data)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        retained = json.loads(next(self.root.glob("*.lookup-root.input.json")).read_text())
        self.assertEqual(retained["l1_headers_rlp"], ["0x00"])
        self.assertEqual(retained["protocol_config_hash"], "0x" + "9" * 64)
        self.assertEqual(retained["l1_chain_config"], json.loads(self.l1_genesis.read_text()))

    def test_prefix_collector_preserves_public_l1_presets(self):
        for preset in ("mainnet", "sepolia", "hoodi", "chiado"):
            with self.subTest(preset=preset):
                data = fixture("revertCounter")
                data["blob"]["settlement_claim"]["post_block_hash"] = ZERO
                data["l2_hash"] = ZERO
                data["prefixes"] = [dict(hash=ROOT, transaction_index=3, transaction_count=4)]
                data["l1_chain_config"] = preset
                self.env["EEZ_L1_CHAIN_CONFIG"] = preset
                result = self.run_fixture("revertCounter", data)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_static_callback_is_bound_in_table_trace_blob_and_fold(self):
        scenario = "topLevelStaticReentrantCounter"
        result = self.run_fixture(scenario)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads(self.output.read_text())
        self.assertEqual(evidence["lookup_entry"]["callbacks"][0][3], CALLBACK_SOURCE)
        self.assertTrue(evidence["lookup_trace"]["callback_traced"])
        # The fold preimage is bytes32(0) || 0x01 || returnData.
        calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
        self.assertIn(["cast", "keccak", "0x" + "0" * 64 + "01" + RETURN[2:]], calls)

        def wrong(mutate):
            data = fixture(scenario)
            mutate(data)
            return data

        def frames(data):
            return data["trace"]["calls"][1]["calls"][2]["calls"]

        cases = {
            "callback source": lambda d: d["decoded"][0][2][0][2][0].__setitem__(3, "0x" + "1" * 40),
            "callback target": lambda d: d["decoded"][0][2][0][2][0].__setitem__(5, "0x" + "1" * 40),
            "callback data": lambda d: d["decoded"][0][2][0][2][0].__setitem__(7, "0xdeadbeef"),
            "callback missing": lambda d: d["decoded"][0][2][0].__setitem__(2, []),
            "extra callback": lambda d: d["decoded"][0][2][0][2].append(d["decoded"][0][2][0][2][0]),
            "callback fold": lambda d: d["decoded"][0][2][0].__setitem__(3, ZERO),
            "untraced callback": lambda d: frames(d).clear(),
            "callback output": lambda d: frames(d)[0]["calls"][0].__setitem__("output", "0xdead"),
            "callback reverted": lambda d: frames(d)[0].__setitem__("error", "execution reverted"),
            "duplicate callback": lambda d: frames(d).append(copy.deepcopy(frames(d)[0])),
            "blob callback": lambda d: d["blob"]["payload"]["semantic_transactions"][0]["actions"].pop(),
            "blob callback result": lambda d: d["blob"]["payload"]["semantic_transactions"][0]
                ["actions"][1].__setitem__("return_data", "0xdead"),
            "producer changed": lambda d: d.__setitem__("counter", "2"),
            "proxy query missing": lambda d: d["trace"]["calls"].pop(0),
            "proxy query address": lambda d: d["trace"]["calls"][0].__setitem__(
                "output", "0x" + "0" * 24 + "1" * 40),
            "extra registry call": lambda d: d["trace"]["calls"].insert(
                0, copy.deepcopy(d["trace"]["calls"][0])),
        }
        for name, mutate in cases.items():
            with self.subTest(case=name):
                result = self.run_fixture(scenario, data=wrong(mutate))
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertFalse(self.output.exists())
        with self.subTest(case="callback proxy"):
            result = self.run_fixture(scenario, log_transform=lambda log: log.replace(
                f"CALLBACK_PROXY={CALLBACK_PROXY}", f"CALLBACK_PROXY={PROXY}"))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("CALLBACK_PROXY is not the registry's proxy", result.stderr)

    def test_collector_dispatches_the_l1_callback_scenario_to_the_lookup_verifier(self):
        scenario = "topLevelStaticReentrantCounter"
        self.assertEqual(self.run_fixture(scenario).returncode, 0)  # Writes the log and data.
        shutil.copy2(SCRIPTS / "collect-protocol-e2e-evidence.sh", self.scripts)
        (self.root / "scenario.sol").write_text("contract ExecuteNetwork {}\n")
        self.output.unlink()
        result = subprocess.run(
            ["bash", str(self.scripts / "collect-protocol-e2e-evidence.sh"), scenario,
             str(self.root / "scenario.sol"), str(self.log), str(self.output)],
            env=self.env, capture_output=True, text=True, timeout=15, cwd=self.root)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads(self.output.read_text())
        self.assertEqual(evidence["verification_mode"], "upstream-lookup-only")
        self.assertTrue(evidence["lookup_trace"]["callback_traced"])

    def test_static_l2_success_uses_the_table_not_old_failure_diagnostics(self):
        data = fixture("topLevelStaticCounter")
        data["block"]["transactions"] = [data["transactions"][POST]]
        data["transactions"][USER]["blockHash"] = ROOT
        data["receipts"][USER]["blockHash"] = ROOT
        data["transactions"][LOAD] = dict(hash=LOAD, to=MANAGER, input="0xabcdef01", blockHash=ROOT,
                                        blockNumber="0x10", transactionIndex="0x0")
        data["receipts"][LOAD] = dict(transactionHash=LOAD, blockHash=ROOT, blockNumber="0x10",
                                    transactionIndex="0x0", status="0x1")
        data["l2_block"] = dict(hash=ROOT, number="0x10",
                                transactions=[data["transactions"][LOAD], data["transactions"][USER]])
        data["load_decoded"] = [[], [[0, KEY, [], ZERO, True, RETURN]]]
        artifact = self.root / "eez-core-protocol/out/EEZL2.sol/EEZL2.json"
        artifact.parent.mkdir(parents=True)
        artifact.write_text(json.dumps({"methodIdentifiers": {"loadExecutionTable(bytes)": "abcdef01",
                                                             "executeIncomingCrossChainCall(bytes)": "abcd0002"}}))
        # New upstream logs contain no TOP-LEVEL STATIC or returnData dump.
        self.log.write_text(f"""READER_L2={SOURCE}
COUNTER_L1={DESTINATION}
tx: {USER}
block: 0xa (status: 0x1)
Matched settlement tx {POST}
PASS: posted batch calldata matches expected table
PASS: posted rolling hashes reproduced from posted roots
VERIFY_PASS reader.lastRead=1 reader.counter=1
====== Done ======
""")
        command = ["bash", str(self.scripts / "verify-static-counter-l2.sh"), str(self.log), str(self.output)]
        for failure in (None, "causal_failure", "orphaned_load", "wrong_return",
                        "unrelated_static_entry", "unrelated_mutable_entry", "duplicate_static_key",
                        "overwritten_table", "wrong_load_transaction", "wrong_position", "l1_reorg", "l2_reorg"):
            with self.subTest(failure=failure):
                mutated = copy.deepcopy(data)
                if failure == "orphaned_load": mutated["receipts"][LOAD]["blockHash"] = ZERO
                elif failure == "wrong_return": mutated["load_decoded"][1][0][5] = "0xdead"
                elif failure == "wrong_position": mutated["load_decoded"][1][0][0] = 1
                elif failure == "unrelated_static_entry":
                    mutated["load_decoded"][1].append([0, ZERO, [], ZERO, True, RETURN])
                elif failure == "unrelated_mutable_entry":
                    mutated["load_decoded"][0].append([ZERO, [], [], ZERO, True, RETURN])
                elif failure == "duplicate_static_key":
                    mutated["load_decoded"][1].append([0, KEY, [], ZERO, True, "0xdead"])
                elif failure == "overwritten_table":
                    mutated["transactions"][USER]["transactionIndex"] = "0x2"
                    mutated["receipts"][USER]["transactionIndex"] = "0x2"
                    mutated["l2_block"]["transactions"].insert(1, dict(
                        hash=ZERO, to=MANAGER, input="0xabcd0002", transactionIndex="0x1"))
                elif failure == "wrong_load_transaction":
                    mutated["transactions"][LOAD] = {**mutated["transactions"][LOAD], "input": "0xwrong"}
                elif failure == "l1_reorg": mutated["final_l1_hash"] = ZERO
                elif failure == "l2_reorg": mutated["l2_hash"] = ZERO
                elif failure: mutated[failure] = True
                self.data.write_text(json.dumps(mutated))
                self.output.unlink(missing_ok=True)
                result = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=15)
                if failure not in (None, "unrelated_static_entry", "unrelated_mutable_entry"):
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertFalse(self.output.exists())
                else:
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(json.loads(self.output.read_text())["verification_mode"], "upstream-static-pool")

    def test_static_l2_callback_scenario_binds_the_loaded_callback(self):
        """topLevelStaticReentrantCounterL2 through the real collector dispatch."""
        producer, callback = "0x" + "9" * 40, "0x" + "f" * 40
        data = fixture("topLevelStaticCounter")
        data["block"]["transactions"] = [data["transactions"][POST]]
        data["transactions"][USER]["blockHash"] = ROOT
        data["receipts"][USER]["blockHash"] = ROOT
        data["transactions"][LOAD] = dict(hash=LOAD, to=MANAGER, input="0xabcdef01", blockHash=ROOT,
                                        blockNumber="0x10", transactionIndex="0x0")
        data["receipts"][LOAD] = dict(transactionHash=LOAD, blockHash=ROOT, blockNumber="0x10",
                                    transactionIndex="0x0", status="0x1")
        data["l2_block"] = dict(hash=ROOT, number="0x10",
                                transactions=[data["transactions"][LOAD], data["transactions"][USER]])
        incoming = [0, True, 0, callback, 0, producer, 0, "0x61bc221a"]
        data["load_decoded"] = [[], [[0, KEY, [incoming], CALLBACK_FOLD, True, RETURN]]]
        artifact = self.root / "eez-core-protocol/out/EEZL2.sol/EEZL2.json"
        artifact.parent.mkdir(parents=True)
        artifact.write_text(json.dumps({"methodIdentifiers": {"loadExecutionTable(bytes)": "abcdef01",
                                                             "executeIncomingCrossChainCall(bytes)": "abcd0002"}}))
        shutil.copy2(SCRIPTS / "collect-protocol-e2e-evidence.sh", self.scripts)
        # Real upstream text; only identities the mocked RPCs answer are replaced.
        log = (CAPTURED_LOGS / "topLevelStaticReentrantCounterL2.txt").read_text()
        for field, replacement in (("READER_L2", SOURCE), ("COUNTER_L1", DESTINATION),
                                   ("CALLBACK_L1", callback), ("PRODUCER_L2", producer)):
            original = re.search(rf"^{field}=(0x[0-9a-fA-F]+)$", log, re.MULTILINE)[1]
            log = log.replace(original, replacement)
        for pattern, replacement in ((r"^tx: (0x[0-9a-fA-F]+)", USER),
                                     (r"^Matched settlement tx (0x[0-9a-fA-F]+)", POST)):
            log = log.replace(re.search(pattern, log, re.MULTILINE)[1], replacement)
        log = re.sub(r"^block: 0x[0-9a-fA-F]+ ", "block: 0x10 ", log, flags=re.MULTILINE)
        self.log.write_text(log)
        command = ["bash", str(self.scripts / "collect-protocol-e2e-evidence.sh"),
                   "topLevelStaticReentrantCounterL2", "scenario.sol", str(self.log), str(self.output)]
        (self.root / "scenario.sol").write_text("contract ExecuteNetworkL2 {}\n")

        def run(mutated):
            self.data.write_text(json.dumps(mutated))
            self.output.unlink(missing_ok=True)
            return subprocess.run(command, env=self.env, capture_output=True, text=True,
                                  timeout=15, cwd=self.root)

        result = run(data)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads(self.output.read_text())
        self.assertEqual(evidence["scenario"], "topLevelStaticReentrantCounterL2")
        self.assertEqual(evidence["static_entry"]["incoming_calls"], 1)
        self.assertEqual(evidence["static_entry"]["rolling_hash"], CALLBACK_FOLD)
        calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
        self.assertIn(["cast", "keccak", "0x" + "0" * 64 + "01" + RETURN[2:]], calls)

        entry = lambda d: d["load_decoded"][1][0]
        cases = {
            "callback source": lambda d: entry(d)[2][0].__setitem__(3, "0x" + "1" * 40),
            "callback target": lambda d: entry(d)[2][0].__setitem__(5, "0x" + "1" * 40),
            "callback source chain": lambda d: entry(d)[2][0].__setitem__(4, 1),
            "callback not static": lambda d: entry(d)[2][0].__setitem__(1, False),
            "callback data": lambda d: entry(d)[2][0].__setitem__(7, "0xdeadbeef"),
            "callback missing": lambda d: entry(d).__setitem__(2, []),
            "extra callback": lambda d: entry(d)[2].append(list(incoming)),
            "callback fold": lambda d: entry(d).__setitem__(3, ZERO),
            "producer changed": lambda d: d.__setitem__("counter", "2"),
        }
        for name, mutate in cases.items():
            with self.subTest(case=name):
                mutated = copy.deepcopy(data)
                mutate(mutated)
                result = run(mutated)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
