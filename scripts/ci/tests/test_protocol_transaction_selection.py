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
from pathlib import Path
f = json.loads(open(os.environ["FIXTURE"]).read())
a = sys.argv[1:]
if a[0] == "block":
    chain = a[a.index("--rpc-url") + 1]
    block = f["blocks"][chain][str(int(a[1], 0))]
    if "--field" in a:
        print(f.get("recheck_hash", {}).get(chain, block["hash"]))
    else:
        print(json.dumps(block))
elif a[0] == "to-dec": print(int(a[1], 16))
elif a[0] == "decode-calldata": print(json.dumps(f["inputs"][a[-1]]))
elif a[0] == "keccak": print(a[1][:66])
elif a[:2] == ["rpc", "eth_getTransactionByHash"]: print(json.dumps(f["transactions"][a[2]]))
elif a[:2] == ["rpc", "eth_getTransactionReceipt"]:
    if f.get("single_receipt_read"):
        marker = Path(os.environ["FIXTURE"]).with_name(a[2] + ".read")
        if marker.exists(): raise SystemExit("receipt fetched a second time")
        marker.touch()
    print(json.dumps(f["receipts"][a[2]]))
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
expected_systems = f.get("wanted_txs", [f["wanted"]])
assert actual["l2_calldata"] == [tx["input"] for tx in expected_systems], actual
expected_receipts = [f["receipts"][tx["hash"]] for tx in expected_systems]
if f.get("outbound"): expected_receipts.insert(0, f["receipts"][f["trigger_hash"]])
assert actual["l2_receipts"] == expected_receipts, actual
post_hashes = f.get("post_hashes", [f["post_hash"]])
expected_l1 = [f["receipts"][h] for h in post_hashes]
if not f.get("outbound"):
    expected_l1 = [f["receipts"][h] for h in f.get("trigger_hashes", [f["trigger_hash"]])] + expected_l1
assert actual["l1_receipts"] == expected_l1, actual
inputs = [f["transactions"][h]["input"] for h in post_hashes]
assert actual["l1_calldata"] == (inputs[0] if len(inputs) == 1 else inputs), actual
if f.get("causal_failure"): raise SystemExit(1)
open(sys.argv[2], "w").write('{"verified":true}')
''')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            (scripts / "verify-semantic-settlement-blob.sh").write_text('''python3 - "$@" <<'PY'
import json, os, sys
f = json.loads(open(os.environ["FIXTURE"]).read())
post, chain, output, sidecars, *triggers = sys.argv[1:]
if f.get("blob_failure") or post == f.get("blob_failure_post"): raise SystemExit(1)
if "post_triggers" in f: assert triggers == f["post_triggers"][post], (post, triggers)
evidence = dict(verified=True, settlement_hash=post, triggers=triggers, sidecars=sidecars)
if f.get("blob_padding"): evidence["padding"] = "0" * f["blob_padding"]
with open(output, "w") as stream:
    json.dump(evidence, stream)
PY
''')
            scenario = root / "E2ECounter.s.sol"
            scenario.write_text("contract ExecuteNetwork {}\n")
            other = self.add_tx([9], "22222222")
            wanted = self.add_tx([1], "22222222")
            trigger = dict(wanted, hash=word(201), to="0x" + "ab" * 20,
                           blockHash=word(116), transactionIndex="0x1")
            post = dict(wanted, hash=word(202), to="0x" + "cd" * 20,
                        blockHash=word(116), transactionIndex="0x0")
            l1_block = dict(number="0x10", hash=word(116), transactions=[post, trigger])
            txs = {tx["hash"]: tx for tx in (other, wanted, trigger, post)}
            receipts = {key: dict(transactionHash=key, status="0x1", blockNumber="0x10",
                                  blockHash=tx["blockHash"], transactionIndex=tx["transactionIndex"],
                                  logs=[dict(marker="keep the entire receipt")]) for key, tx in txs.items()}
            fixture = dict(blocks={"l1": {"16": l1_block}, "l2": {"16": self.block}},
                           inputs=self.inputs, transactions=txs, receipts=receipts,
                           wanted=wanted, trigger_hash=trigger["hash"], post_hash=post["hash"])
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
            successes = {"pass", "outbound-without-call-search", "outbound-replayed",
                         "settlement-summary-hint", "outbound-receipt-snapshot", "split",
                         "oversize-evidence"}
            failures = {
                "orphan": "differs from the canonical block",
                "causal_failure": "independent trigger/table/event causal verification failed",
                "blob_failure": "settlement blob semantic verification failed",
                "l1-trigger-moved": "moved from reported block 10 to 16",
                "outbound-orphan-trigger": "is absent from its canonical block",
                "outbound-missing-trigger": "is absent from its canonical block",
                "outbound-wrong-index": "is absent from its canonical block",
                "outbound-wrong-input": "differs from the canonical block",
                "outbound-reverted": "reverted",
                "outbound-missing-table": "cannot select exact scenario table transactions",
                "outbound-reorg-during-collection": "L2 changed during evidence collection",
                "settlement-orphan": "is absent from its canonical block",
                "settlement-reorg-during-collection": "L1 changed during evidence collection",
                "split-missing-batch": "independent trigger/table/event causal verification failed",
                "split-orphan": "is absent from its canonical block",
                "split-wrong-registry": "postBatch transaction target differs",
                "split-blob-failure": "settlement blob semantic verification failed",
                "split-uncovered-trigger": "settlements do not uniquely cover trigger blocks",
            }
            for case in (*sorted(successes), *failures):
                with self.subTest(case=case):
                    current = copy.deepcopy(fixture)
                    scenario.write_text("contract ExecuteNetwork {}\n")
                    log.write_text(original_log)
                    if case.startswith("split"):
                        # Two triggers settle at 16 and the third at 17. Scan
                        # order is reversed and includes a repeated match line.
                        wanted2 = dict(wanted, hash=word(301), input="0x22222222aa", transactionIndex="0x2")
                        wanted3 = dict(wanted, hash=word(302), input="0x22222222bb",
                                       blockNumber="0x11", blockHash=word(17), transactionIndex="0x0")
                        trigger2 = dict(trigger, hash=word(303), transactionIndex="0x2")
                        trigger3 = dict(trigger, hash=word(304), blockNumber="0x11", blockHash=word(117))
                        post2 = dict(post, hash=word(305), input="0x11111111cc",
                                     blockNumber="0x11", blockHash=word(117))
                        current["blocks"]["l2"]["16"]["transactions"].append(wanted2)
                        current["blocks"]["l1"]["16"]["transactions"].append(trigger2)
                        current["blocks"]["l2"]["17"] = dict(number="0x11", hash=word(17), transactions=[wanted3])
                        current["blocks"]["l1"]["17"] = dict(number="0x11", hash=word(117), transactions=[post2, trigger3])
                        for tx in (wanted2, wanted3, trigger2, trigger3, post2):
                            current["transactions"][tx["hash"]] = tx
                            current["receipts"][tx["hash"]] = dict(
                                transactionHash=tx["hash"], status="0x1", blockNumber=tx["blockNumber"],
                                blockHash=tx["blockHash"], transactionIndex=tx["transactionIndex"],
                                logs=[dict(marker="keep the entire receipt")])
                        for tx, key in ((wanted2, 2), (wanted3, 3)):
                            current["inputs"][tx["input"]] = [[[word(key), [], [], word(0), True, "0x"]], []]
                        current["wanted_txs"] = [wanted, wanted2, wanted3]
                        current["post_hashes"] = [post["hash"], post2["hash"]]
                        current["trigger_hashes"] = [trigger["hash"], trigger2["hash"], trigger3["hash"]]
                        current["post_triggers"] = {post["hash"]: [trigger["hash"], trigger2["hash"]],
                                                    post2["hash"]: [trigger3["hash"]]}
                        matches = [post2["hash"], post["hash"], post2["hash"]]
                        if case == "split-missing-batch": matches = [post["hash"]]
                        split_log = original_log.replace(f'L2 table expected: [{word(1)}]',
                                                         f'L2 table expected: [{word(1)}, {word(2)}, {word(3)}]')
                        split_log = split_log.replace('L2 blocks to verify: [16]', 'L2 blocks to verify: [16, 17]')
                        split_log += f'User tx[2]: {trigger2["hash"]} (block 16)\nUser tx[3]: {trigger3["hash"]} (block 17)\n'
                        split_log += ''.join(f'Matched settlement tx {h}\n' for h in matches)
                        log.write_text(split_log)
                        if case == "split-orphan": current["receipts"][post2["hash"]]["blockHash"] = word(999)
                        if case == "split-wrong-registry": post2["to"] = "0x" + "88" * 20
                        if case == "split-blob-failure": current["blob_failure_post"] = post2["hash"]
                        if case == "split-uncovered-trigger":
                            # A matched table posted after its trigger cannot
                            # have supplied the entry consumed by that trigger.
                            post2["transactionIndex"], trigger3["transactionIndex"] = "0x1", "0x0"
                            current["blocks"]["l1"]["17"]["transactions"] = [trigger3, post2]
                            for tx in (post2, trigger3):
                                current["receipts"][tx["hash"]]["transactionIndex"] = tx["transactionIndex"]
                    if case.startswith("outbound-"):
                        scenario.write_text("contract ExecuteNetworkL2 {}\n")
                        current["outbound"] = True
                        block = current["blocks"]["l2"]["16"]
                        user = current["transactions"][trigger["hash"]]
                        user.update(blockHash=block["hash"], transactionIndex="0x2")
                        block["transactions"].append(copy.deepcopy(user))
                        current["receipts"][trigger["hash"]].update(
                            blockHash=block["hash"], transactionIndex="0x2")
                    if case == "outbound-without-call-search":
                        log.write_text(original_log.replace("L2 blocks to verify: [16]\n", ""))
                    if case == "outbound-receipt-snapshot":
                        current["single_receipt_read"] = True
                    if case in ("outbound-replayed", "l1-trigger-moved"):
                        # The summary retains the first optimistic receipt and
                        # first block hint; the current table and user are at 16.
                        log.write_text(original_log.replace("(block 16)", "(block 10)")
                                       .replace("L2 blocks to verify: [16]", "L2 blocks to verify: [10]"))
                    if case == "settlement-summary-hint":
                        log.write_text(original_log.replace(
                            f'L1 postBatch: {post["hash"]} (block 16)',
                            f'L1 postBatch: {post["hash"]} (block 10)'))
                    if case == "orphan":
                        current["receipts"][wanted["hash"]]["blockHash"] = word(999)
                    if case in ("causal_failure", "blob_failure"):
                        current[case] = True
                    if case == "outbound-orphan-trigger":
                        current["receipts"][trigger["hash"]]["blockHash"] = word(999)
                    if case == "outbound-missing-trigger":
                        current["blocks"]["l2"]["16"]["transactions"].pop()
                    if case == "outbound-wrong-index":
                        current["receipts"][trigger["hash"]]["transactionIndex"] = "0x1"
                    if case == "outbound-wrong-input":
                        current["transactions"][trigger["hash"]]["input"] = "0xdeadbeef"
                    if case == "outbound-reverted":
                        current["receipts"][trigger["hash"]]["status"] = "0x0"
                    if case == "outbound-missing-table":
                        current["inputs"][wanted["input"]][0][0][0] = word(8)
                    if case == "outbound-reorg-during-collection":
                        current["recheck_hash"] = {"l2": word(999)}
                    if case == "oversize-evidence":
                        # Larger than Linux's 128 KiB per-argument limit: the
                        # collector must pass evidence to jq through files.
                        current["blob_padding"] = 300_000
                    if case == "settlement-orphan":
                        current["receipts"][post["hash"]]["blockHash"] = word(999)
                    if case == "settlement-reorg-during-collection":
                        current["recheck_hash"] = {"l1": word(999)}
                    fixture_path.write_text(json.dumps(current))
                    output = root / (case + ".json")
                    result = subprocess.run(["bash", str(collector), "counter", str(scenario), str(log), str(output)],
                                            env=env, capture_output=True, text=True, timeout=30)
                    if case in successes:
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                        evidence = json.loads(output.read_text())
                        self.assertEqual([t["hash"] for t in evidence["system_transactions"]],
                                         [t["hash"] for t in current.get("wanted_txs", [wanted])])
                        self.assertEqual(evidence["source_log"]["upstream_first_table_hash"], other["hash"])
                        if case == "split":
                            self.assertEqual([t["hash"] for t in evidence["settlement_transactions"]], current["post_hashes"])
                            blobs = evidence["blob_da"]["settlements"]
                            self.assertTrue(evidence["blob_da"]["verified"])
                            self.assertEqual([b["triggers"] for b in blobs], list(current["post_triggers"].values()))
                            self.assertNotEqual(blobs[0]["sidecars"], blobs[1]["sidecars"])
                        if case == "oversize-evidence":
                            self.assertEqual(len(evidence["blob_da"]["padding"]), 300_000)
                        if case == "outbound-replayed":
                            user = evidence["trigger_transactions"][0]
                            self.assertEqual(user["reported_block"], 10)
                            self.assertEqual(user["block_number"], 16)
                            self.assertFalse(user["reported_block_matches"])
                            self.assertFalse(evidence["claims"]["upstream_summary_blocks_match_receipts"])
                    else:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn(failures[case], result.stderr)
                        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
