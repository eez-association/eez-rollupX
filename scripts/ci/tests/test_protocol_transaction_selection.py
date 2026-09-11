"""Shared-block evidence selection must preserve whole transactions and counts."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
SCRIPT = REPO / "scripts/ci/select-protocol-l2-transactions.py"
SPEC = importlib.util.spec_from_file_location("protocol_selection", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def word(n):
    return "0x" + f"{n:064x}"


MANAGER = "0x" + "42" * 20
METHODS = {"loadExecutionTable(tuple[])": "11111111",
           "executeIncomingCrossChainCall(tuple[])": "22222222"}


class SelectionFixture:
    def setUp(self):
        self.inputs = {}
        self.block = dict(number="0x10", hash=word(16), transactions=[])

    def add_tx(self, keys, selector="11111111", to=MANAGER, block=None):
        block = self.block if block is None else block
        index = len(block["transactions"])
        calldata = "0x" + selector + f"{len(self.inputs):064x}"
        self.inputs[calldata] = [[[word(key), [], [], word(0), True, "0x"] for key in keys], []]
        tx = dict(hash=word(100 + len(self.inputs)), input=calldata, to=to,
                  transactionIndex=hex(index), blockNumber=block["number"], blockHash=block["hash"])
        block["transactions"].append(tx)
        return tx

    def cast(self, *args):
        if args[0] == "decode-calldata":
            return json.dumps(self.inputs[args[-1]])
        self.assertEqual(args[0], "keccak")
        # Deterministic fixture hash of both fields; real ABI/keccak is exercised
        # by the network test. Most fixture rolling hashes are zero.
        return word(int(args[1][:66], 16) + int(args[1][66:], 16))

    def select(self, expected, blocks=None):
        return MODULE.select_transactions([self.block] if blocks is None else blocks,
                                          MANAGER, list(map(word, expected)), METHODS, self.cast)


class SelectionTest(SelectionFixture, unittest.TestCase):
    def test_first_load_and_other_scenario_calls_are_not_selected(self):
        self.add_tx([9])
        wanted = self.add_tx([1], "22222222")
        self.add_tx([8], "22222222")
        self.add_tx([1], to="0x" + "99" * 20)
        self.assertEqual(self.select([1]), [wanted])

    def test_outbound_uses_matching_load_not_first_load_or_user_tx(self):
        self.add_tx([9])
        wanted = self.add_tx([1, 1])
        self.add_tx([], "aaaaaaaa")
        self.assertEqual(self.select([1, 1]), [wanted])

    def test_multiple_blocks_keep_canonical_order_and_whole_transactions(self):
        first = self.add_tx([1, 9], "22222222")
        second_block = dict(number="0x11", hash=word(17), transactions=[])
        second = self.add_tx([2], "22222222", block=second_block)
        self.assertEqual(self.select([1, 2], [second_block, self.block]), [first, second])
        # Discovery never removes the unrelated entry or edits selected calldata.
        self.assertEqual(len(self.inputs[first["input"]][0]), 2)

    def test_missing_wrong_or_duplicate_occurrences_fail(self):
        self.add_tx([1])
        for expected in ([], [2], [1, 1]):
            with self.subTest(expected=expected), self.assertRaises(ValueError):
                self.select(expected)
        self.add_tx([1])
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.select([1])
        self.assertEqual(len(self.select([1, 1])), 2)

    def test_wrong_emitter_selector_or_abi_cannot_satisfy_entries(self):
        self.add_tx([1], to="0x" + "99" * 20)
        self.add_tx([1], "aaaaaaaa")
        with self.assertRaisesRegex(ValueError, "missing"):
            self.select([1])
        with self.assertRaisesRegex(ValueError, "ABI signature"):
            MODULE.select_transactions([self.block], MANAGER, [word(1)], {}, self.cast)

    def test_valid_table_cannot_hide_extra_table_for_the_same_call(self):
        self.add_tx([1])
        extra = self.add_tx([1])
        self.inputs[extra["input"]][0][0][3] = word(1)
        with self.assertRaisesRegex(ValueError, "unexpected additional table"):
            self.select([1])

    def test_canonical_block_transaction_identity_and_uniqueness_required(self):
        self.add_tx([1])
        for field, bad in (("blockHash", word(999)), ("blockNumber", "0x11"),
                           ("transactionIndex", "0x1")):
            with self.subTest(field=field):
                block = copy.deepcopy(self.block)
                block["transactions"][0][field] = bad
                with self.assertRaisesRegex(ValueError, "inconsistent"):
                    self.select([1], [block])
        with self.assertRaisesRegex(ValueError, "duplicate canonical block"):
            self.select([1], [self.block, self.block])
        duplicate = copy.deepcopy(self.block)
        duplicate["transactions"].append(duplicate["transactions"][0])
        with self.assertRaisesRegex(ValueError, "duplicate transaction"):
            self.select([1], [duplicate])

    def test_malformed_hashes_quantities_and_collection_types_fail(self):
        self.add_tx([1])
        for expected in ({word(1): 1}, ["0x1"], [False]):
            with self.subTest(expected=expected), self.assertRaises(ValueError):
                MODULE.select_transactions([self.block], MANAGER, expected, METHODS, self.cast)
        for value in (True, 1.5, -1, "1.5"):
            with self.subTest(quantity=value), self.assertRaises(ValueError):
                MODULE.quantity(value)


FAKE_CAST = '''#!/usr/bin/env python3
import json, os, sys
f = json.loads(open(os.environ["FIXTURE"]).read())
a = sys.argv[1:]
if a[0] == "block": print(json.dumps(f["block"]))
elif a[0] == "to-dec": print(int(a[1], 16))
elif a[0] == "decode-calldata": print(json.dumps(f["inputs"][a[-1]]))
elif a[0] == "keccak": print(a[1][:66])
elif a[:2] == ["rpc", "eth_getTransactionByHash"]: print(json.dumps(f["transactions"][a[2]]))
elif a[:2] == ["rpc", "eth_getTransactionReceipt"]: print(json.dumps(f["receipts"][a[2]]))
else: raise SystemExit("unexpected cast call: " + repr(a))
'''


class CollectorTest(SelectionFixture, unittest.TestCase):
    def test_real_collector_passes_only_selected_complete_receipts_and_rejects_orphans(self):
        with tempfile.TemporaryDirectory(prefix="eez-selected-evidence.") as temporary:
            root = Path(temporary)
            scripts = root / "testing/kurtosis/scripts"
            scripts.mkdir(parents=True)
            collector = scripts / "collect-protocol-e2e-evidence.sh"
            shutil.copy2(REPO / "testing/kurtosis/scripts" / collector.name, collector)
            ci = root / "scripts/ci"
            ci.mkdir(parents=True)
            shutil.copy2(SCRIPT, ci)
            abi = root / "eez-core-protocol/out/EEZL2.sol/EEZL2.json"
            abi.parent.mkdir(parents=True)
            abi.write_text(json.dumps(dict(methodIdentifiers=METHODS)))
            tools = root / "bin"
            tools.mkdir()
            (tools / "cast").write_text(FAKE_CAST)
            # Boundary: assert the Rust verifier sees the full selected calldata
            # and receipts, not filtered logs, then explicitly test its rejection.
            (tools / "causal").write_text('''#!/usr/bin/env python3
import json, os, sys
f = json.loads(open(os.environ["FIXTURE"]).read())
actual = json.loads(open(sys.argv[1]).read())
assert actual["l2_calldata"] == [f["wanted"]["input"]], actual
expected_receipts = [f["receipts"][f["wanted"]["hash"]]]
if f.get("outbound"): expected_receipts.insert(0, f["receipts"][f["trigger_hash"]])
assert actual["l2_receipts"] == expected_receipts, actual
if f.get("causal_failure"): raise SystemExit(1)
open(sys.argv[2], "w").write('{"verified":true}')
''')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            (scripts / "verify-semantic-settlement-blob.sh").write_text("echo '{\"verified\":true}' > \"$3\"\n")
            scenario = root / "E2ECounter.s.sol"
            scenario.write_text("contract ExecuteNetwork {}\n")
            other = self.add_tx([9], "22222222")
            wanted = self.add_tx([1], "22222222")
            trigger = dict(wanted, hash=word(201), to="0x" + "ab" * 20)
            post = dict(wanted, hash=word(202), to="0x" + "cd" * 20)
            txs = {tx["hash"]: tx for tx in (other, wanted, trigger, post)}
            receipts = {key: dict(transactionHash=key, status="0x1", blockNumber="0x10",
                                  blockHash=word(16), transactionIndex=tx["transactionIndex"],
                                  logs=[dict(marker="keep the entire receipt")]) for key, tx in txs.items()}
            fixture = dict(block=self.block, inputs=self.inputs, transactions=txs, receipts=receipts,
                           wanted=wanted, trigger_hash=trigger["hash"])
            fixture_path = root / "fixture.json"
            fixture_path.write_text(json.dumps(fixture))
            log = root / "scenario.log"
            log.write_text(f'''target: {trigger["to"]}
PASS: posted batch calldata matches expected table
PASS: L2 field checks on 1 entry
PASS: L1 field checks on 1 entry
L2 table expected: [{word(1)}]
L2 blocks to verify: [16]
User tx: {trigger["hash"]} (block 16)
L1 postBatch: {post["hash"]} (block 16)
L2 loadTable: {other["hash"]} (block 16)
L2 call exec: {other["hash"]}
{wanted["hash"]} (block 16)
====== Done ======
''')
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"],
                       FIXTURE=str(fixture_path), L1_RPC="l1", L2_RPC="l2", ROLLUPS=post["to"],
                       MANAGER_L2=MANAGER, PROTOCOL_COMMIT="a" * 40, ROLLUP_ID="1",
                       CAUSAL_VERIFIER=str(tools / "causal"), BLOB_VERIFIER="unused", BEACON_URL="unused")
            original_log = log.read_text()
            for case in ("pass", "outbound-without-call-search", "orphan", "causal_failure"):
                with self.subTest(case=case):
                    current = copy.deepcopy(fixture)
                    scenario.write_text("contract ExecuteNetwork {}\n")
                    log.write_text(original_log)
                    if case == "outbound-without-call-search":
                        scenario.write_text("contract ExecuteNetworkL2 {}\n")
                        log.write_text(original_log.replace("L2 blocks to verify: [16]\n", ""))
                        current["outbound"] = True
                    if case == "orphan":
                        current["receipts"][wanted["hash"]]["blockHash"] = word(999)
                    if case == "causal_failure":
                        current["causal_failure"] = True
                    fixture_path.write_text(json.dumps(current))
                    output = root / (case + ".json")
                    result = subprocess.run(["bash", str(collector), "counter", str(scenario), str(log), str(output)],
                                            env=env, capture_output=True, text=True, timeout=30)
                    if case in ("pass", "outbound-without-call-search"):
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                        evidence = json.loads(output.read_text())
                        self.assertEqual([t["hash"] for t in evidence["system_transactions"]], [wanted["hash"]])
                        self.assertEqual(evidence["source_log"]["upstream_first_table_hash"], other["hash"])
                    else:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
