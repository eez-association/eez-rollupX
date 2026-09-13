"""Relay fault controls must hold the intended retry, not a maintenance post."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[2] / "builder-stub.py"
SPEC = importlib.util.spec_from_file_location("builder_stub", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def bundle(*transactions):
    return {"jsonrpc": "2.0", "id": 1, "method": "eth_sendBundle",
            "params": [{"txs": list(transactions), "blockNumber": "0x10",
                        "minTimestamp": 100, "maxTimestamp": 100}]}


class BuilderControlTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="eez-builder-control.")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.forwarded = []

    def forward(self, request, **_):
        self.forwarded.append(json.loads(request.data))
        return io.BytesIO(b'{"jsonrpc":"2.0","id":1,"result":{"bundleHash":"0x01"}}')

    def send(self, body, upstream=None):
        # Exercise the real request handler with only HTTP transport replaced.
        handler = object.__new__(MODULE.Handler)
        raw = json.dumps(body).encode()
        handler.headers = {"Content-Length": str(len(raw))}
        handler.rfile = io.BytesIO(raw)
        handler.wfile = io.BytesIO()
        handler.upstream = "http://fixture.invalid"
        handler.passthrough_bundles = True
        handler.test_control_dir = str(self.directory)
        handler.send_response = lambda *_: None
        handler.send_header = lambda *_: None
        handler.end_headers = lambda: None
        with patch.object(MODULE.urllib.request, "urlopen", side_effect=upstream or self.forward):
            handler.do_POST()
        return json.loads(handler.wfile.getvalue())

    def release(self, expected, replacement=None):
        def on_sleep(_):
            held = json.loads((self.directory / "held-bundle.json").read_text())
            self.assertEqual(held, expected)
            (self.directory / "released-bundle.json").write_text(
                json.dumps(replacement or expected))
        return on_sleep

    def test_targeted_hold_skips_maintenance_and_unrelated_users(self):
        retry = bundle("0x0304", "0x0202")
        hold = self.directory / "hold-next.json"
        hold.write_text(json.dumps({"transaction": "0x0202"}))
        maintenance = bundle("0x0301")
        unrelated = bundle("0x0302", "0x0201")
        with patch.object(MODULE.time, "sleep", side_effect=self.release(retry)):
            self.send(maintenance)
            self.send(unrelated)
            self.assertEqual(self.forwarded, [maintenance, unrelated])
            self.assertTrue(hold.exists())
            self.assertFalse((self.directory / "held-bundle.json").exists())
            self.send(retry)
        self.assertFalse(hold.exists())
        self.assertEqual(self.forwarded, [maintenance, unrelated, retry])
        # The control is one-shot; a later request must not be held again.
        (self.directory / "released-bundle.json").unlink()
        with patch.object(MODULE.time, "sleep", side_effect=AssertionError("held twice")):
            self.send(maintenance)

    def test_omission_arms_exact_retry_before_forwarding(self):
        for omitted in range(3):
            with self.subTest(omitted=omitted):
                original = bundle("0x0300", "0x0201", "0x0202", "0x0203")
                raw = original["params"][0]["txs"][omitted + 1]
                (self.directory / "omit-user.json").write_text(
                    json.dumps({"userIndex": omitted, "holdRetry": True}))

                def forward(request, **kwargs):
                    control = json.loads((self.directory / "hold-next.json").read_text())
                    self.assertEqual(control, {"transaction": raw})
                    return self.forward(request, **kwargs)

                self.send(original, upstream=forward)
                expected = copy.deepcopy(original)
                del expected["params"][0]["txs"][omitted + 1]
                self.assertEqual(self.forwarded[-1], expected)
                record = json.loads((self.directory / "omission.json").read_text())
                self.assertEqual(record, {"original": original, "forwarded": expected,
                                          "omittedUser": omitted})
                (self.directory / "hold-next.json").unlink()

    def test_ordinary_omission_does_not_arm_a_hold(self):
        (self.directory / "omit-user.json").write_text(json.dumps({"userIndex": 1}))
        original = bundle("0x0300", "0x0201", "0x0202", "0x0203")
        self.send(original)
        self.assertFalse((self.directory / "hold-next.json").exists())
        self.assertEqual(self.forwarded[0]["params"][0]["txs"],
                         ["0x0300", "0x0201", "0x0203"])

    def test_unfiltered_hold_preserves_explicit_release_sequence(self):
        (self.directory / "hold-next.json").write_text("{}")
        original = bundle("0x0300")
        replacement = bundle("0x0201", "0x0300", "0x0202")
        with patch.object(MODULE.time, "sleep", side_effect=self.release(original, replacement)):
            self.send(original)
        self.assertEqual(self.forwarded, [replacement])

    def test_concurrent_control_claim_does_not_fail_another_request(self):
        hold = self.directory / "hold-next.json"
        hold.write_text("{}")
        original = bundle("0x0300")

        def already_claimed(*_):
            hold.unlink()
            raise FileNotFoundError("another request claimed the hold")

        with patch.object(MODULE.os, "replace", side_effect=already_claimed):
            self.send(original)
        self.assertEqual(self.forwarded, [original])
        self.assertFalse((self.directory / "held-bundle.json").exists())

    def test_unreleased_matching_retry_times_out_without_forwarding(self):
        (self.directory / "hold-next.json").write_text(json.dumps({"transaction": "0x0202"}))
        retry = bundle("0x0300", "0x0202")
        with patch.object(MODULE.time, "monotonic", side_effect=[0, 61]):
            with self.assertRaisesRegex(TimeoutError, "test did not release held bundle"):
                self.send(retry)
        self.assertEqual(self.forwarded, [])
        self.assertEqual(json.loads((self.directory / "held-bundle.json").read_text()), retry)


if __name__ == "__main__":
    unittest.main()
