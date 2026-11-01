"""Exact semantic/trace binding; RPC canonicality and KZG are separate checks."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("lookup_trace", ROOT / "scripts/ci/verify-protocol-lookup-trace.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
FIXTURES = Path(__file__).parent / "fixtures/protocol-lookup"
CASES = ("catchRemoteRevertAfterCallback", "staticReadCallsBackAndReadsAgain", "topLevelStaticReentrantMissingProxy")


def fixture(name):
    return json.loads((FIXTURES / (name + "-trace.json")).read_text())


def frame(document, path):
    node = document["trace"]
    for index in path:
        node = node["calls"][index]
    return node


class LookupTraceTests(unittest.TestCase):
    def test_captured_three_lookup_shapes(self):
        for name in CASES:
            with self.subTest(name=name):
                result = MODULE.verify(fixture(name))
                self.assertTrue(result["verified"])
                self.assertEqual(result["action_count"], 3 if name == CASES[1] else 2)
                self.assertEqual(len(result["missing_proxy_probes"]), int(name == CASES[2]))

    def test_exact_outcomes_and_identities_are_required(self):
        for name in CASES:
            original = fixture(name)
            for index in range(len(original["semantic_transaction"]["actions"])):
                for field, value in (("return_data", "0xdeadbeef"), ("data", "0xdeadbeef"),
                                     ("from_address", "0x" + "ff" * 20), ("to_address", "0x" + "ff" * 20),
                                     ("value", "1"), ("success", False)):
                    doc = copy.deepcopy(original)
                    action = doc["semantic_transaction"]["actions"][index]
                    if action[field] == value:
                        continue
                    action[field] = value
                    with self.subTest(name=name, index=index, field=field), self.assertRaises(ValueError):
                        MODULE.verify(doc)

    def test_unmade_or_omitted_callbacks_reject(self):
        for name in CASES:
            original = fixture(name)
            paths = MODULE.verify(original)["action_trace_paths"]
            doc = copy.deepcopy(original)
            doc["semantic_transaction"]["actions"].pop()
            with self.subTest(name=name, mutation="omitted action"), self.assertRaises(ValueError):
                MODULE.verify(doc)
            doc = copy.deepcopy(original)
            del frame(doc, paths[1][:-1])["calls"][paths[1][-1]]
            with self.subTest(name=name, mutation="missing target"), self.assertRaises(ValueError):
                MODULE.verify(doc)

    def test_registry_context_and_ancestry_are_required(self):
        original = fixture(CASES[1])
        paths = MODULE.verify(original)["action_trace_paths"]
        for change in ("registry", "forwarder", "parent", "dispatch", "trigger", "static", "extra callback"):
            doc = copy.deepcopy(original)
            if change == "registry":
                doc["registry"] = "0x" + "ff" * 20
            elif change == "forwarder":
                frame(doc, paths[1][:-1])["input"] = "0xdeadbeef"
            elif change == "parent":
                doc["semantic_transaction"]["actions"][2]["parent_index"] = None
            elif change == "dispatch":
                frame(doc, [0, 2])["output"] = "0xdeadbeef"
            elif change == "trigger":
                doc["trigger"]["input"] = "0xdeadbeef"
            elif change == "static":
                frame(doc, paths[0])["type"] = "CALL"
            else:
                wrapper = copy.deepcopy(frame(doc, paths[1][:-1]))
                frame(doc, paths[1][:-2])["calls"].append(wrapper)
            with self.subTest(change=change), self.assertRaises(ValueError):
                MODULE.verify(doc)

    def test_missing_proxy_requires_exact_error_creation_and_retry(self):
        original = fixture(CASES[2])
        for change in ("error", "proxy", "creation", "creator", "create input", "retry", "probe dispatch"):
            doc = copy.deepcopy(original)
            if change == "error":
                frame(doc, [1, 2])["output"] = "0xdeadbeef"
            elif change == "proxy":
                frame(doc, [1, 2])["output"] = "0x9c9f37f4" + "00" * 12 + "ff" * 20
            elif change == "creation":
                frame(doc, [2, 0])["calls"] = []
            elif change == "creator":
                frame(doc, [2, 0, 0])["from"] = "0x" + "ff" * 20
            elif change == "create input":
                frame(doc, [2])["input"] = "0xdeadbeef"
            elif change == "retry":
                del doc["trace"]["calls"][3]
            else:
                frame(doc, [1, 2])["from"] = "0x" + "ff" * 20
            with self.subTest(change=change), self.assertRaises(ValueError):
                MODULE.verify(doc)

    def test_every_deferred_lookup_key_and_root_is_selected(self):
        for name in CASES:
            doc = fixture(name)
            with self.subTest(name=name):
                result = MODULE.lookup_entries(doc)
                self.assertEqual(len(result["keys"]), 2 if name == CASES[1] else 1)
                self.assertEqual(len(result["roots"]), 1)
                self.assertEqual(result["entries"][0]["action_index"], 0)
                if name == CASES[1]:
                    self.assertEqual(result["entries"][1]["action_index"], 2)

    def test_wrong_ambiguous_immediate_or_state_changing_entries_reject(self):
        for name in CASES:
            original = fixture(name)
            static = name != CASES[0]
            slot = 2 if static else 1
            for mutation in ("key", "destination", "outcome", "duplicate", "immediate", "root rollup", "extra root", "missing"):
                doc = copy.deepcopy(original)
                batch = doc["decoded_settlement"][0]
                entry = batch[slot][0]
                if mutation == "key":
                    entry[1] = "0x" + "ff" * 32
                elif mutation == "destination":
                    entry[-3] = 2
                elif mutation == "outcome":
                    entry[-1] = "0xdeadbeef"
                elif mutation == "duplicate":
                    batch[slot].append(copy.deepcopy(entry))
                elif mutation == "immediate":
                    batch[4 if static else 3] = 1
                elif mutation == "root rollup":
                    entry[0][0][0] = 2
                elif mutation == "extra root":
                    entry[0].append(copy.deepcopy(entry[0][0]))
                else:
                    batch[slot] = []
                with self.subTest(name=name, mutation=mutation), self.assertRaises(ValueError):
                    MODULE.lookup_entries(doc)
        for field, value in ((1, 1), (3, "0x" + "ff" * 32)):
            doc = fixture(CASES[0])
            doc["decoded_settlement"][0][1][0][0][0][field] = value
            with self.subTest(update_field=field), self.assertRaisesRegex(ValueError, "root or balance"):
                MODULE.lookup_entries(doc)


if __name__ == "__main__":
    unittest.main()
