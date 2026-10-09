"""Offline checks for scripts/why-excluded.py classification and log parsing."""
import importlib.util
import json
from pathlib import Path
import unittest


SCRIPT = Path(__file__).resolve().parents[2] / "why-excluded.py"
SPEC = importlib.util.spec_from_file_location("why_excluded", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

BUILDERS = MODULE.configured_builders(
    "https://rpc.titanbuilder.xyz,https://rpc.buildernet.org,https://rpc.quasar.win,"
    "https://rpc.eurekabuilder.xyz,https://rpc.bombora.build")
RELAYED = [{"relay": "ultrasound", "builder_pubkey": "0xb7", "value_eth": 0.01}]


def log(message, **fields):
    return "2026-10-06T06:34:19Z INFO " + json.dumps(
        {"timestamp": "2026-10-06T06:34:19Z", "level": "INFO",
         "fields": {"message": message, **fields}})


class ClassifyTest(unittest.TestCase):
    def test_configured_builder_is_matched_by_its_extra_data(self):
        self.assertEqual(MODULE.classify("Titan (titanbuilder.xyz)", RELAYED, BUILDERS),
                         ("CONFIGURED_BUILDER", "titanbuilder.xyz"))
        self.assertEqual(MODULE.classify("✨ Quasar (quasar.win) ✨", RELAYED, BUILDERS)[1],
                         "quasar.win")
        self.assertEqual(MODULE.classify("Eureka (eurekabuilder.xyz)", RELAYED, BUILDERS)[1],
                         "eurekabuilder.xyz")

    def test_unrelayed_client_or_empty_extra_data_is_local(self):
        for extra in ("besu 26.8.1", "Nethermind v1.38.1", "geth go1.25.1 linux", ""):
            self.assertEqual(MODULE.classify(extra, [], BUILDERS), ("LOCAL", None), extra)

    def test_relayed_unknown_builder_is_other_builder(self):
        self.assertEqual(MODULE.classify("Builder+ www.btcs.com/builder", RELAYED, BUILDERS),
                         ("OTHER_BUILDER", None))

    def test_unrelayed_non_client_extra_data_is_unknown(self):
        self.assertEqual(MODULE.classify("mystery", [], BUILDERS), ("UNKNOWN", None))

    def test_timestamp_mismatch_takes_precedence(self):
        self.assertEqual(MODULE.classify("Titan (titanbuilder.xyz)", RELAYED, BUILDERS,
                                         expected_ts=100, block_ts=112),
                         ("TARGET_MISMATCH", None))

    def test_unlisted_builder_host_uses_its_label_as_marker(self):
        builders = MODULE.configured_builders("https://rpc.newbuilder.io")
        self.assertEqual(builders, {"newbuilder.io": ("newbuilder",)})

    def test_geth_rlp_extra_data_is_readable(self):
        raw = "0x" + bytes([0xd8, 0x83, 1, 16, 7, 0x84]).hex() + b"geth".hex() \
            + bytes([0x88]).hex() + b"go1.25.1".hex() + bytes([0x85]).hex() + b"linux".hex()
        self.assertEqual(MODULE.decode_extra(raw), "geth go1.25.1 linux")


class ParseLogsTest(unittest.TestCase):
    def test_collects_excluded_targets_with_timestamp_and_bundle_hashes(self):
        lines = [
            "not json at all",
            log("bundle submission window open", target_block=26131577,
                target_timestamp=1791268463),
            log("eth_sendBundle response received", builder_host="rpc.titanbuilder.xyz",
                target_block=26131577,
                response=json.dumps({"result": {"bundleHash": "0xaa"}})),
            log("eth_sendBundle response received", builder_host="rpc.quasar.win",
                target_block=26131577,
                response=json.dumps({"result": {"bundleHash": "0xbb"}})),
            log("bundle dropped; composer will retry on next tick", target_block=26131577,
                tx_hash="0xpost", reason="excluded"),
            # Acknowledged but included: not reported.
            log("eth_sendBundle response received", builder_host="rpc.titanbuilder.xyz",
                target_block=26131578,
                response=json.dumps({"result": {"bundleHash": "0xcc"}})),
            # Dropped for another reason: not an exclusion.
            log("bundle dropped; composer will retry on next tick", target_block=26131579,
                reason="slot_skipped"),
        ]
        self.assertEqual(MODULE.parse_logs(lines), {
            26131577: {"timestamp": 1791268463, "post_tx": "0xpost",
                       "bundles": {"titanbuilder.xyz": "0xaa", "quasar.win": "0xbb"}},
        })


if __name__ == "__main__":
    unittest.main()
