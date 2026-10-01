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


def fixture(scenario):
    static = scenario == "topLevelStaticCounter"
    result = RETURN if static else REVERT
    mutable = [[[1, 0, ROOT, ROOT]], KEY, [], [], "0x" + "6" * 64, 1, False, result]
    static_entry = [[[1, ROOT]], KEY, [], ZERO, 1, True, result]
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
                }]}],
            },
            "semantic_evidence": {"raw_trigger_transactions": [{"raw_transaction": "0xraw"}],
                                  "trigger_binding": "settlement_table",
                                  "semantics_bound_to_settlement": True},
        },
        "trace": {"type":"CALL", "to":SOURCE, "input":user["input"], "calls":[
            {"type":"STATICCALL" if static else "CALL", "from":SOURCE, "to":PROXY, "calls":[
                {"type":"STATICCALL", "from":PROXY, "to":REGISTRY,
                 "input":"0xb7ab4572", "output":"0x" + f"{10000:064x}"},
                {"type":"CALL", "from":PROXY, "to":PROXY, "input":"0x9f149e1b",
                 **({"error":"write protection"} if static else {})},
                {"type":"STATICCALL" if static else "CALL", "from":PROXY, "to":REGISTRY,
                 "input":"0xstaticlookup" if static else "0xmutablelookup", "value":"0x0",
                 "output":ABI_RETURN if static else REVERT,
                 **({} if static else {"error":"execution reverted"})}]}]},
        "l2_hash": ROOT, "l2_head": 100,
    }


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
    print(json.dumps({"verified":True, "block_hash":request["block_hash"], "prefixes":f.get("prefixes", [])}))
    sys.exit(0)
command = args[0]
if command == "sig":
    value = {"counter()": "0x61bc221a", "increment()": "0xd09de08a", "incrementProxy()": "0x11111111",
             "postAndVerifyBatch(bytes)": "0xpost", "STATIC_CHECK_GAS()": "0xb7ab4572"}[args[1]]
elif command == "calldata":
    if args[1] == "Error(string)":
        assert args[1:] == ["Error(string)", "always reverts"], args
        value = "0x08c379a0"
    else:
        assert args[1] in ("staticCrossChainCall(address,bytes)", "executeCrossChainCall(address,bytes)"), args
        value = "0xstaticlookup" if args[1].startswith("static") else "0xmutablelookup"
elif command == "abi-encode":
    if args[1] == "f(bytes)": value = "0x" + f"{32:064x}{32:064x}{1:064x}"
    else: value = "0x" + "0"*63 + "1" if args[1] == "f(uint256)" else "0x00"
elif command == "keccak":
    value = "0x" + "6"*64
elif command == "call":
    sig = args[2]
    if sig.startswith("computeCrossChainCallHash("):
        value = "0x" + "1"*64
    elif sig.startswith("computeCrossChainProxyAddress("):
        value = "0x" + "e"*40
    elif sig == "lastCallFailed()(bool)":
        value = f.get("last_failed", "true")
    elif sig == "targetCounter()(uint256)":
        value = f.get("target_counter", "0")
    elif sig == "lastRead()(uint256)":
        value = f.get("last_read", "1")
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
                 else dict(hash=f["l2_hash"], number=hex(f["l2_head"])))
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
        self.env = {**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"],
                    "LOOKUP_FIXTURE": str(self.data), "LOOKUP_CALLS": str(self.calls),
                    "L1_RPC": "http://l1.invalid", "L2_RPC": "http://l2.invalid", "ROLLUPS": REGISTRY,
                    "MANAGER_L2": MANAGER, "ROLLUP_ID": "1", "PROTOCOL_COMMIT": "fixture-pin",
                    "CAUSAL_VERIFIER": str(tools / "causal"),
                    "BEACON_URL": "http://beacon.invalid", "BLOB_VERIFIER": str(tools / "blob"),
                    "EEZ_L2_GENESIS": str(self.genesis)}

    def run_fixture(self, scenario, data=None, complete=True, log_transform=None):
        self.data.write_text(json.dumps(data or fixture(scenario)))
        self.output.unlink(missing_ok=True)
        # Preserve real upstream text. Substitute only identities handled by
        # mocked RPCs, so invented PASS markers cannot hide collector drift.
        log = (CAPTURED_LOGS / f"{scenario}.txt").read_text()
        for field, replacement in (("READER_L1", SOURCE), ("COUNTER_L2", DESTINATION),
                                   ("SAFE_CALLER", SOURCE), ("REV_COUNTER_L2", DESTINATION)):
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

    def test_both_lookup_modes_accept_complete_canonical_evidence(self):
        for scenario in ("topLevelStaticCounter", "revertCounter"):
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
            for failure in (None, "empty_prefix", "missing", "duplicate", "replay_failure", "missing_genesis"):
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
                    result = self.run_fixture(scenario, data)
                    if failure not in (None, "empty_prefix"):
                        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertFalse(self.output.exists())
                    else:
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertEqual(json.loads(self.output.read_text())["lookup_root"]["kind"], "execution_prefix")

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
block: 0x10 (status: 0x1)
Matched settlement tx {POST}
PASS: 1 settlement tx(s) with BatchPosted in range
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


if __name__ == "__main__":
    unittest.main()
