"""Offline checks for scripts/why-excluded.py classification and log parsing."""
import importlib.util
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[2] / "why-excluded.py"
SPEC = importlib.util.spec_from_file_location("why_excluded", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

BUILDERS = MODULE.configured_builders(
    "https://rpc.titanbuilder.xyz,https://rpc.buildernet.org,https://rpc.quasar.win,"
    "https://rpc.eurekabuilder.xyz,https://rpc.bombora.build")
RELAYED = [{"relay": "ultrasound", "builder_pubkey": "0xb7", "value_eth": 0.01}]
BLOCK_HASH = "0x" + "aa" * 32
POST_HASH = "0x" + "bb" * 32
OTHER_HASH = "0x" + "cc" * 32
POSTER = "0x" + "11" * 20


def block(number=100, transactions=(), extra="geth"):
    return {"hash": BLOCK_HASH, "timestamp": hex(1000 + 12 * (number - 100)),
            "extraData": "0x" + extra.encode().hex(), "miner": POSTER,
            "baseFeePerGas": "0x1", "gasUsed": "0x5208", "gasLimit": "0x1c9c380",
            "transactions": list(transactions)}


def transaction(tx_hash):
    return {"hash": tx_hash, "from": POSTER, "gasPrice": "0x2", "type": "0x0"}


def log(message, **fields):
    return "2026-10-06T06:34:19Z INFO " + json.dumps(
        {"timestamp": "2026-10-06T06:34:19Z", "level": "INFO",
         "fields": {"message": message, **fields}})


def dispatch(post=POST_HASH, target=100, timestamp=1000):
    return log("dispatching bundle to builder", target_block=target,
               target_timestamp=timestamp, tx_hash=post)


def delivery(host, outcome="acknowledged", bundle_hash=POST_HASH, index=0):
    return {"index": index, "host": host, "outcome": outcome,
            "relay_bundle_hash": bundle_hash, "elapsed_ms": 12.5}


def fanout(*builders, post=POST_HASH, target=100):
    return log("eth_sendBundle fan-out finished", target_block=target, tx_hash=post,
               outcomes=json.dumps(list(builders)))


class SubmissionLogsTest(unittest.TestCase):
    def test_successful_only_logs_select_an_attempt_and_keep_dispatch_hash(self):
        attempts = MODULE.parse_logs([
            dispatch(),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz", response='{"result":true}'),
            fanout(delivery("rpc.titanbuilder.xyz", bundle_hash=None)),
        ])
        self.assertEqual(len(attempts), 1)
        attempt = attempts[(100, POST_HASH)]
        self.assertEqual(attempt["timestamp"], 1000)
        self.assertEqual(attempt["submissions"]["titanbuilder.xyz"]["outcomes"], ["acknowledged"])

    def test_rejected_only_logs_are_selected_without_a_drop(self):
        attempts = MODULE.parse_logs([
            dispatch(), fanout(delivery("rpc.titanbuilder.xyz", "rejected", None)),
        ])
        self.assertEqual(attempts[(100, POST_HASH)]["submissions"]["titanbuilder.xyz"]["outcomes"],
                         ["rejected"])

    def test_duplicate_events_do_not_duplicate_attempts_or_hashes(self):
        events = [dispatch(), fanout(delivery("rpc.titanbuilder.xyz"))]
        attempts = MODULE.parse_logs(events + events)
        self.assertEqual(len(attempts), 1)
        self.assertEqual(attempts[(100, POST_HASH)]["submissions"]["titanbuilder.xyz"]["bundle_hashes"],
                         [POST_HASH])

    def test_two_posts_at_same_height_remain_separate(self):
        attempts = MODULE.parse_logs([
            dispatch(), fanout(delivery("rpc.titanbuilder.xyz")),
            dispatch(post=OTHER_HASH),
            fanout(delivery("rpc.titanbuilder.xyz", bundle_hash=OTHER_HASH), post=OTHER_HASH),
            log("bundle dropped", target_block=100, tx_hash=POST_HASH, reason="excluded"),
        ])
        self.assertEqual(set(attempts), {(100, POST_HASH), (100, OTHER_HASH)})
        self.assertEqual(attempts[(100, POST_HASH)]["drop_reasons"], ["excluded"])
        self.assertEqual(attempts[(100, OTHER_HASH)]["drop_reasons"], [])

    def test_fanout_overrides_malformed_response_breadcrumb(self):
        attempts = MODULE.parse_logs([
            dispatch(),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz", response='{}'),
            fanout(delivery("rpc.titanbuilder.xyz", bundle_hash=None)),
        ])
        self.assertEqual(attempts[(100, POST_HASH)]["submissions"]["titanbuilder.xyz"]["outcomes"],
                         ["acknowledged"])

    def test_ambiguous_responses_are_not_attached_to_the_wrong_post(self):
        diagnostics = {}
        attempts = MODULE.parse_logs([
            dispatch(), dispatch(post=OTHER_HASH),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz", response='{"result":true}'),
        ], diagnostics)
        self.assertEqual(diagnostics["uncorrelated_responses"], 1)
        self.assertEqual(attempts[(100, POST_HASH)]["submissions"], {})
        self.assertEqual(attempts[(100, OTHER_HASH)]["submissions"], {})
        self.assertEqual(attempts[(100, None)]["submissions"]["titanbuilder.xyz"]["outcomes"],
                         ["acknowledged"])

    def test_unmatched_ambiguous_hash_remains_available_for_stats(self):
        attempts = MODULE.parse_logs([dispatch(), dispatch(post=OTHER_HASH),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz",
                response=json.dumps({"result": {"bundleHash": BLOCK_HASH}})),
        ])
        self.assertEqual(attempts[(100, None)]["submissions"]["titanbuilder.xyz"]["bundle_hashes"],
                         [BLOCK_HASH])

    def test_ambiguous_responses_do_not_recount_typed_fanout_observations(self):
        attempts = MODULE.parse_logs([
            dispatch(), fanout(delivery("rpc.titanbuilder.xyz")),
            dispatch(post=OTHER_HASH), fanout(delivery("rpc.titanbuilder.xyz", bundle_hash=None), post=OTHER_HASH),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz", response='{"result":null}'),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz", response=json.dumps({"result": {"bundleHash": POST_HASH}})),
        ])
        self.assertEqual(set(attempts), {(100, POST_HASH), (100, OTHER_HASH)})

    def test_rpc_rejection_and_unsupported_are_distinct(self):
        attempts = MODULE.parse_logs([
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz",
                response=json.dumps({"error": {"code": -32000, "message": "fixture rejection"}})),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.bombora.build", response='{"error":{"code":-32601}}'),
        ])
        submissions = attempts[(100, None)]["submissions"]
        self.assertEqual(submissions["titanbuilder.xyz"]["outcomes"], ["rejected"])
        self.assertEqual(submissions["titanbuilder.xyz"]["errors"][0]["message"], "fixture rejection")
        self.assertEqual(submissions["bombora.build"]["outcomes"], ["unsupported"])

    def test_all_fanout_failure_variants_remain_explicit(self):
        outcomes = ["rejected", "unsupported", "transport_error", "deadline_exceeded",
                    "abandoned_after_acknowledgement"]
        attempts = MODULE.parse_logs([dispatch(), fanout(*[
            delivery(f"builder{i}.example", outcome, None, i) for i, outcome in enumerate(outcomes)
        ])])
        self.assertEqual({row["outcomes"][0] for row in attempts[(100, POST_HASH)]["submissions"].values()},
                         set(outcomes))

    def test_malformed_fanout_is_reported_without_inventing_acceptance(self):
        diagnostics = {}
        for payload in ('null', '{}', 'not json', '[null]', '[{"host":3}]'):
            with self.subTest(payload=payload):
                attempts = MODULE.parse_logs([dispatch(), log("eth_sendBundle fan-out finished",
                    target_block=100, tx_hash=POST_HASH, outcomes=payload)], diagnostics)
                self.assertEqual(attempts[(100, POST_HASH)]["submissions"], {})
        self.assertGreater(diagnostics["malformed_fanouts"], 0)

    def test_non_integer_targets_are_ignored(self):
        for target in (True, 1.5, "bad", -1, None):
            with self.subTest(target=target):
                self.assertEqual(MODULE.parse_logs([dispatch(target=target)]), {})

    def test_conflicting_timestamps_remain_unknown(self):
        diagnostics = {}
        attempts = MODULE.parse_logs([dispatch(), dispatch(timestamp=1012)], diagnostics)
        self.assertIsNone(attempts[(100, POST_HASH)]["timestamp"])
        self.assertEqual(diagnostics["conflicting_timestamps"], 1)

    def test_pre_dispatch_window_is_not_a_submission(self):
        self.assertEqual(MODULE.parse_logs([log("bundle submission window open",
            target_block=100, tx_hash=POST_HASH, target_timestamp=1000)]), {})

    def test_rejected_response_reason_survives_duplicate_fanout(self):
        attempts = MODULE.parse_logs([dispatch(),
            log("eth_sendBundle response received", target_block=100,
                builder_host="rpc.titanbuilder.xyz", response='{"error":{"code":-32000,"message":"bad nonce"}}'),
            fanout(delivery("rpc.titanbuilder.xyz", "rejected", None)),
        ])
        row = attempts[(100, POST_HASH)]["submissions"]["titanbuilder.xyz"]
        self.assertEqual(row["outcomes"], ["rejected"])
        self.assertEqual(row["errors"], [{"code": -32000, "message": "bad nonce"}])


class ClassifyTest(unittest.TestCase):
    def test_configured_builder_is_matched_by_its_extra_data(self):
        self.assertEqual(MODULE.classify("Titan (titanbuilder.xyz)", RELAYED, BUILDERS),
                         ("CONFIGURED_BUILDER", "titanbuilder.xyz"))
        self.assertEqual(MODULE.classify("✨ Quasar (quasar.win) ✨", RELAYED, BUILDERS)[1],
                         "quasar.win")
        self.assertEqual(MODULE.classify("Eureka (eurekabuilder.xyz)", RELAYED, BUILDERS)[1],
                         "eurekabuilder.xyz")

    def test_unrelayed_client_is_only_possibly_local(self):
        for extra in ("besu 26.8.1", "Nethermind v1.38.1", "geth go1.25.1 linux"):
            self.assertEqual(MODULE.classify(extra, [], BUILDERS),
                             ("POSSIBLY_LOCAL", None), extra)

    def test_empty_extra_data_does_not_identify_a_local_proposer(self):
        self.assertEqual(MODULE.classify("", [], BUILDERS), ("UNKNOWN", None))

    def test_missing_configuration_does_not_imply_another_builder(self):
        self.assertEqual(MODULE.classify("Titan", RELAYED, {}), ("UNKNOWN", None))

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
                response=json.dumps({"result": {"bundleHash": POST_HASH}})),
            log("eth_sendBundle response received", builder_host="rpc.quasar.win",
                target_block=26131577,
                response=json.dumps({"result": {"bundleHash": OTHER_HASH}})),
            log("bundle dropped; composer will retry on next tick", target_block=26131577,
                tx_hash=POST_HASH, reason="excluded"),
            # Acknowledgement without a drop still selects a target.
            log("eth_sendBundle response received", builder_host="rpc.titanbuilder.xyz",
                target_block=26131578,
                response=json.dumps({"result": {"bundleHash": OTHER_HASH}})),
            # Other drop reasons are retained without asserting exclusion.
            log("bundle dropped; composer will retry on next tick", target_block=26131579,
                reason="slot_skipped"),
        ]
        attempts = MODULE.parse_logs(lines)
        self.assertEqual(set(attempts), {(26131577, POST_HASH), (26131578, None), (26131579, None)})
        attempt = attempts[(26131577, POST_HASH)]
        self.assertEqual(attempt["timestamp"], 1791268463)
        self.assertEqual(attempt["submissions"]["titanbuilder.xyz"]["bundle_hashes"], [POST_HASH])
        self.assertEqual(attempt["submissions"]["quasar.win"]["bundle_hashes"], [OTHER_HASH])
        self.assertEqual(attempts[(26131579, None)]["drop_reasons"], ["slot_skipped"])

    def test_acknowledgements_without_valid_hash_are_retained(self):
        for result in (True, False, None, "0xhash", [], {}, {"bundleHash": 3},
                       {"bundleHash": "0xshort"}):
            with self.subTest(result=result):
                events = [log("eth_sendBundle response received", target_block=100,
                              builder_host="rpc.titanbuilder.xyz",
                              response=json.dumps({"result": result})),
                          log("bundle dropped", target_block=100, tx_hash=POST_HASH,
                              reason="excluded")]
                parsed = MODULE.parse_logs(events)
                row = parsed[(100, POST_HASH)]["submissions"]["titanbuilder.xyz"]
                self.assertEqual(row["outcomes"], ["acknowledged"])
                self.assertEqual(row["bundle_hashes"], [])

    def test_bad_responses_do_not_crash_or_count_as_acknowledgements(self):
        for response in ('not json', 'null', '[]', 'true', '{}',
                         '{"error":{"code":-32601}}',
                         '{"result":true,"error":{"code":-1}}'):
            with self.subTest(response=response):
                events = [log("eth_sendBundle response received", target_block=100,
                              builder_host="rpc.titanbuilder.xyz", response=response),
                          log("bundle dropped", target_block=100, tx_hash=POST_HASH,
                              reason="excluded")]
                row = MODULE.parse_logs(events)[(100, POST_HASH)]["submissions"]["titanbuilder.xyz"]
                self.assertNotIn("acknowledged", row["outcomes"])


class RelayTest(unittest.TestCase):
    def test_deliveries_are_filtered_by_canonical_hash(self):
        traces = [{"block_hash": OTHER_HASH, "builder_pubkey": "wrong", "value": "1"},
                  {"block_hash": BLOCK_HASH.upper(), "builder_pubkey": "right", "value": "2"}]
        with patch.dict(MODULE.RELAYS, {"fixture": "https://relay.example"}, clear=True), \
                patch.object(MODULE, "http_json", return_value=traces) as request:
            delivered, unavailable = MODULE.relay_deliveries(BLOCK_HASH)
        self.assertEqual([trace["builder_pubkey"] for trace in delivered], ["right"])
        self.assertEqual(unavailable, [])
        self.assertIn("block_hash=" + BLOCK_HASH, request.call_args.args[0])

    def test_malformed_relay_responses_are_unavailable(self):
        for response in (None, {}, {"error": "rate limited"}, [None]):
            with self.subTest(response=response), \
                    patch.dict(MODULE.RELAYS, {"fixture": "https://relay.example"}, clear=True), \
                    patch.object(MODULE, "http_json", return_value=response):
                self.assertEqual(MODULE.relay_deliveries(BLOCK_HASH), ([], ["fixture"]))


class StatsTest(unittest.TestCase):
    def test_quasar_uses_snake_case_hash_parameter(self):
        reply = {"result": {"status": "submitted", "targetBlock": 100, "error": None}}
        with patch.object(MODULE, "http_json", return_value=reply) as request:
            stats = MODULE.bundle_stats("quasar.win", POST_HASH)
        self.assertEqual(stats, {"availability": "available", "result": reply["result"]})
        self.assertEqual(request.call_args.args[0], "https://stats.quasar.win")
        self.assertEqual(request.call_args.args[1]["method"], "quasar_getBundleStats")
        self.assertEqual(request.call_args.args[1]["params"], [{"bundle_hash": POST_HASH}])

    def test_eureka_uses_submission_rpc_and_preserves_lifecycle_details(self):
        reply = {"result": {"bundleHash": POST_HASH, "status": "Submitted",
                            "error": "HeadAdvanced", "targetBlock": 100,
                            "timeline": [{"status": "SimulationPass"}]}}
        with patch.object(MODULE, "http_json", return_value=reply) as request:
            stats = MODULE.bundle_stats("eurekabuilder.xyz", POST_HASH)
        self.assertEqual(stats, {"availability": "available", "result": reply["result"]})
        self.assertEqual(request.call_args.args[0], "https://rpc.eurekabuilder.xyz")
        self.assertEqual(request.call_args.args[1]["method"], "eureka_getBundleStats")
        self.assertEqual(request.call_args.args[1]["params"], [{"bundleHash": POST_HASH}])

    def test_buildernet_unsupported_explains_api_limit_without_requesting(self):
        with patch.object(MODULE, "http_json") as request:
            stats = MODULE.bundle_stats("buildernet.org", POST_HASH)
        self.assertEqual(stats["availability"], "unsupported")
        self.assertIn("deprecated", stats["reason"])
        request.assert_not_called()

    def test_quasar_failure_status_variants_are_counted_without_changing_result(self):
        for status, expected in (("invalid", True), ("simulation_fail", True),
                                 ("SimulationFail", True), ("submitted", False),
                                 ("landed", False), ("excluded_from_block", False),
                                 ("not_found", False), (None, False), ({}, False)):
            with self.subTest(status=status):
                record = {"availability": "available", "result": {"status": status}}
                self.assertEqual(MODULE.stats_simulation_failed(record), expected)
                self.assertEqual(record["result"]["status"], status)

    def test_stats_for_another_target_or_hash_are_unavailable(self):
        for result in ({"status": "invalid", "targetBlock": 101},
                       {"status": "SimulationFail", "bundleHash": OTHER_HASH},
                       {"status": "invalid", "targetBlock": True}):
            with self.subTest(result=result):
                with patch.object(MODULE, "http_json", return_value={"result": result}):
                    stats = MODULE.bundle_stats("quasar.win", POST_HASH, target=100)
                self.assertEqual(stats["availability"], "unavailable")
                self.assertEqual(stats["result"], result)
                self.assertFalse(MODULE.stats_simulation_failed(stats))

    def test_uses_documented_stats_hosts(self):
        for host, method in (("titanbuilder.xyz", "titan_getBundleStats"),
                             ("bombora.build", "bombora_getBundleStats")):
            with self.subTest(host=host), patch.object(
                    MODULE, "http_json", return_value={"result": {"status": "Pending"}}) as request:
                result = MODULE.bundle_stats(host, POST_HASH)
                self.assertEqual(request.call_args.args[0], "https://stats." + host)
                self.assertEqual(request.call_args.args[1]["method"], method)
                self.assertEqual(request.call_args.args[1]["params"], [{"bundleHash": POST_HASH}])
                self.assertEqual(result, {"availability": "available", "result": {"status": "Pending"}})

    def test_unsupported_and_missing_hash_do_not_make_requests(self):
        with patch.object(MODULE, "http_json") as request:
            self.assertEqual(MODULE.bundle_stats("buildernet.org", POST_HASH)["availability"],
                             "unsupported")
            for host in MODULE.BUNDLE_STATS:
                for bundle_hash in (None, "0xshort", 3):
                    self.assertEqual(MODULE.bundle_stats(host, bundle_hash)["availability"], "missing_hash")
            request.assert_not_called()

    def test_transport_rpc_and_malformed_responses_are_unavailable(self):
        for response in ({"error": {"code": -32601}}, [], {"result": None}, {}):
            with self.subTest(response=response), patch.object(MODULE, "http_json", return_value=response):
                self.assertEqual(MODULE.bundle_stats("titanbuilder.xyz", POST_HASH)["availability"],
                                 "unavailable")
        with patch.object(MODULE, "http_json", side_effect=OSError("offline")):
            self.assertEqual(MODULE.bundle_stats("titanbuilder.xyz", POST_HASH)["availability"],
                             "unavailable")


class AnalyzeTest(unittest.TestCase):
    def setUp(self):
        self.args = SimpleNamespace(rpc="https://execution.example", poster=POSTER, horizon=2)
        self.info = {"timestamp": 1000, "post_tx": POST_HASH, "submissions": {}}

    def test_relay_outage_is_unknown_not_local(self):
        with patch.object(MODULE, "rpc", return_value=block()), \
                patch.object(MODULE, "http_json", side_effect=OSError("offline")):
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertEqual(report["category"], "UNKNOWN")
        self.assertEqual(set(report["relays_unavailable"]), set(MODULE.RELAYS))

    def test_stats_for_every_acknowledging_builder_even_when_another_wins(self):
        submissions = {"titanbuilder.xyz": POST_HASH, "bombora.build": OTHER_HASH, "quasar.win": None}
        for host, bundle_hash in submissions.items():
            MODULE.add_submission(self.info, host, "acknowledged", bundle_hash)
        with patch.object(MODULE, "rpc", return_value=block(extra="Builder+ www.btcs.com")), \
                patch.object(MODULE, "relay_deliveries", return_value=(RELAYED, [])), \
                patch.object(MODULE, "bundle_stats", return_value={"availability": "fixture"}) as stats:
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertEqual(report["category"], "OTHER_BUILDER")
        self.assertEqual(set(stats.call_args_list[i].args for i in range(stats.call_count)),
                         set(submissions.items()))
        self.assertEqual(set(report["builder_bundle_stats"]), set(submissions))

    def test_unrelated_poster_transaction_is_not_post_inclusion(self):
        with patch.object(MODULE, "rpc", return_value=block(transactions=[transaction(OTHER_HASH)])), \
                patch.object(MODULE, "relay_deliveries", return_value=([], [])):
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertFalse(report["post_in_target"])
        self.assertEqual(report["landed"]["status"], "not_found")
        self.assertEqual(report["poster_transactions_in_target"], [OTHER_HASH])

    def test_exact_post_in_target_does_not_scan_later_blocks(self):
        with patch.object(MODULE, "rpc", return_value=block(transactions=[transaction(POST_HASH)])) as rpc, \
                patch.object(MODULE, "relay_deliveries", return_value=([], [])):
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertTrue(report["post_in_target"])
        self.assertEqual(report["landed"]["block"], 100)
        self.assertEqual(rpc.call_count, 1)

    def test_later_exact_hash_is_reported_without_poster_option(self):
        self.args.poster = None
        with patch.object(MODULE, "rpc", side_effect=[block(), block(101, [transaction(POST_HASH)])]), \
                patch.object(MODULE, "relay_deliveries", return_value=([], [])):
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertFalse(report["post_in_target"])
        self.assertEqual(report["landed"]["status"], "included")
        self.assertEqual(report["landed"]["block"], 101)

    def test_poster_without_post_hash_only_reports_activity(self):
        self.info["post_tx"] = None
        with patch.object(MODULE, "rpc", return_value=block(transactions=[transaction(OTHER_HASH)])) as rpc, \
                patch.object(MODULE, "relay_deliveries", return_value=([], [])):
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertIsNone(report["post_in_target"])
        self.assertEqual(report["landed"]["status"], "unknown")
        self.assertEqual(rpc.call_count, 1)

    def test_unproduced_blocks_and_rpc_errors_are_not_negative_inclusion_evidence(self):
        for outcome, expected in ((None, "pending"), (OSError("offline"), "unavailable")):
            with self.subTest(expected=expected), patch.object(
                    MODULE, "rpc", side_effect=[block(), outcome]), patch.object(
                    MODULE, "relay_deliveries", return_value=([], [])):
                report = MODULE.analyze(100, self.info, self.args, BUILDERS)
            self.assertEqual(report["landed"]["status"], expected)

    def test_zero_horizon_only_checks_target(self):
        self.args.horizon = 0
        with patch.object(MODULE, "rpc", return_value=block()) as rpc, \
                patch.object(MODULE, "relay_deliveries", return_value=([], [])):
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertEqual(report["landed"], {"status": "not_found", "through_block": 100})
        self.assertEqual(rpc.call_count, 1)


class SummaryTest(unittest.TestCase):
    def analyze(self, transactions=(), timestamp=1000, drop_reason=None, target_exists=True):
        info = MODULE.new_attempt(100, POST_HASH)
        info["timestamp"] = timestamp
        if drop_reason:
            info["drop_reasons"].append(drop_reason)
        MODULE.add_submission(info, "rpc.titanbuilder.xyz", "acknowledged", POST_HASH)
        MODULE.add_submission(info, "rpc.bombora.build", "acknowledged", OTHER_HASH)
        MODULE.add_submission(info, "rpc.quasar.win", "rejected", error={"code": -32000})
        args = SimpleNamespace(rpc="http://fixture", poster=None, horizon=0)
        with patch.object(MODULE, "block_summary", return_value=block_summary_fixture(transactions)
                          if target_exists else None), \
                patch.object(MODULE, "relay_deliveries", return_value=(RELAYED, [])), \
                patch.object(MODULE, "bundle_stats", return_value={"availability": "unsupported"}):
            report = MODULE.analyze(100, info, args, BUILDERS)
        return report, MODULE.summarize([report], BUILDERS, {})["builders"]

    def test_target_inclusion_applies_to_accepted_posts_without_awarding_builder_wins(self):
        report, counts = self.analyze([transaction(POST_HASH)])
        self.assertEqual(report["matched_builder"], "titanbuilder.xyz")
        for host in ("titanbuilder.xyz", "bombora.build"):
            self.assertEqual(counts[host]["accepted"], 1)
            self.assertEqual(counts[host]["included"], 1)
            self.assertEqual(counts[host]["excluded"], 0)
        self.assertEqual(counts["quasar.win"]["accepted"], 0)
        self.assertEqual(counts["quasar.win"]["included"], 0)
        self.assertEqual(counts["quasar.win"]["rejected"], 1)
        self.assertEqual(counts["buildernet.org"]["observed"], 0)

    def test_absent_exact_post_is_excluded_even_without_a_drop_log(self):
        report, counts = self.analyze([transaction(OTHER_HASH)])
        self.assertEqual(report["target_outcome"], "excluded")
        self.assertEqual(counts["titanbuilder.xyz"]["excluded"], 1)
        self.assertEqual(counts["titanbuilder.xyz"]["unresolved"], 0)
        self.assertEqual(counts["quasar.win"]["excluded"], 0)

    def test_missing_or_mismatched_timestamp_cannot_prove_exclusion(self):
        for timestamp in (None, 1012):
            with self.subTest(timestamp=timestamp):
                report, counts = self.analyze(timestamp=timestamp, drop_reason="excluded")
                self.assertEqual(report["target_outcome"], "unresolved")
                self.assertEqual(counts["titanbuilder.xyz"]["unresolved"], 1)
                self.assertEqual(counts["titanbuilder.xyz"]["excluded"], 0)

    def test_current_canonical_inclusion_overrides_historical_exclusion(self):
        report, counts = self.analyze([transaction(POST_HASH)], drop_reason="excluded")
        self.assertEqual(report["drop_reasons"], ["excluded"])
        self.assertEqual(counts["titanbuilder.xyz"]["included"], 1)
        self.assertEqual(counts["titanbuilder.xyz"]["excluded"], 0)

    def test_pending_target_is_unresolved(self):
        report, counts = self.analyze(target_exists=False)
        self.assertEqual(report["category"], "NOT_YET_PRODUCED")
        self.assertEqual(counts["titanbuilder.xyz"]["unresolved"], 1)

    def test_multiple_endpoints_of_one_builder_count_once_and_keep_all_hashes(self):
        info = MODULE.parse_logs([dispatch(), fanout(
            delivery("rpc.titanbuilder.xyz", bundle_hash=POST_HASH),
            delivery("other.titanbuilder.xyz", bundle_hash=OTHER_HASH, index=1),
            delivery("third.titanbuilder.xyz", "rejected", None, index=2),
        )])[(100, POST_HASH)]
        with patch.object(MODULE, "block_summary", return_value=None), \
                patch.object(MODULE, "bundle_stats", return_value={"availability": "available",
                    "result": {"status": "SimulationFail"}}) as stats:
            report = MODULE.analyze(100, info, SimpleNamespace(rpc="http://fixture"), BUILDERS)
        self.assertEqual({call.args for call in stats.call_args_list},
                         {("titanbuilder.xyz", POST_HASH), ("titanbuilder.xyz", OTHER_HASH)})
        counts = MODULE.summarize([report], BUILDERS, {})["builders"]["titanbuilder.xyz"]
        self.assertEqual(counts["observed"], 1)
        self.assertEqual(counts["accepted"], 1)
        self.assertEqual(counts["rejected"], 0)
        self.assertEqual(counts["simulation_failed"], 1)


def block_summary_fixture(transactions):
    with patch.object(MODULE, "rpc", return_value=block(transactions=transactions, extra="Titan")):
        return MODULE.block_summary("http://fixture", 100)


# Only the service locations are replaced. The child executes the real CLI,
# log parser, HTTP/RPC clients, classifiers and JSON/text renderers.
CLI_RUNNER = '''
import importlib.util, sys
script, base, *args = sys.argv[1:]
spec = importlib.util.spec_from_file_location("why_excluded", script)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.RELAYS = {"fixture": base + "/relay"}
module.BUNDLE_STATS = {
    host: (base + "/stats/" + name, *module.BUNDLE_STATS[host][1:])
    for host, name in (("titanbuilder.xyz", "titan"), ("bombora.build", "bombora"),
                       ("quasar.win", "quasar"), ("eurekabuilder.xyz", "eureka"))
}
sys.exit(module.main(args))
'''


class CliTest(unittest.TestCase):
    def setUp(self):
        self.requests = []
        self.chain_id = "0x1"
        self.target = block(extra="Builder+ www.btcs.com")
        self.block_error = None
        self.relay_error = False
        self.later_blocks = {}
        self.stats_status = {"titan": "SimulationFail", "bombora": "SimulationPass"}
        self.quasar_result = {"status": "submitted", "targetBlock": 100, "error": None,
                              "receivedAt": 988, "submittedAt": 990, "landedAt": None}
        self.eureka_result = {"bundleHash": OTHER_HASH, "status": "Submitted", "targetBlock": 100,
                              "error": "HeadAdvanced", "timeline": []}
        self.stats_error = {}
        self.simulation_error = "BundleRevert: " + "fixture-detail " * 30 + POST_HASH
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def reply(self, data, status=200):
                body = json.dumps(data).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                fixture.requests.append((self.path, None))
                if fixture.relay_error:
                    self.reply({"error": "fixture outage"}, 503)
                else:
                    self.reply([{"block_hash": BLOCK_HASH, "builder_pubkey": "0xfixture", "value": "1"}])

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                fixture.requests.append((self.path, request))
                if self.path in fixture.stats_error:
                    self.reply({"jsonrpc": "2.0", "id": 1, "error": fixture.stats_error[self.path]})
                    return
                if self.path == "/l1":
                    if request["method"] == "eth_chainId":
                        result = fixture.chain_id
                    elif request["method"] == "eth_getBlockByNumber":
                        if fixture.block_error:
                            self.reply({"jsonrpc": "2.0", "id": 1, "error": fixture.block_error})
                            return
                        number = int(request["params"][0], 16)
                        result = fixture.target if number == 100 else fixture.later_blocks.get(
                            number, block(number, [transaction(OTHER_HASH)]))
                    else:
                        self.reply({"error": {"code": -32601}}, 400)
                        return
                elif self.path == "/stats/titan":
                    result = {"status": fixture.stats_status["titan"], "error": fixture.simulation_error}
                elif self.path == "/stats/bombora":
                    result = {"status": fixture.stats_status["bombora"], "error": ""}
                elif self.path == "/stats/quasar":
                    if request["method"] != "quasar_getBundleStats" or request["params"] != [{"bundle_hash": POST_HASH}]:
                        self.reply({"error": {"code": -32602, "message": "expected bundle_hash"}})
                        return
                    result = fixture.quasar_result
                elif self.path == "/stats/eureka":
                    if request["method"] != "eureka_getBundleStats" or request["params"] != [{"bundleHash": OTHER_HASH}]:
                        self.reply({"error": {"code": -32602, "message": "expected bundleHash"}})
                        return
                    result = fixture.eureka_result
                else:
                    self.reply({"error": "unexpected endpoint"}, 404)
                    return
                self.reply({"jsonrpc": "2.0", "id": 1, "result": result})

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.addCleanup(server.server_close)
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
        thread.start()
        self.addCleanup(thread.join)
        self.addCleanup(server.shutdown)
        self.base = f"http://127.0.0.1:{server.server_port}"
        self.logs = "\n".join([
            log("bundle submission window open", target_block=100, target_timestamp=1000),
            log("eth_sendBundle response received", target_block=100, builder_host="rpc.titanbuilder.xyz",
                response=json.dumps({"result": {"bundleHash": POST_HASH}})),
            log("eth_sendBundle response received", target_block=100, builder_host="rpc.bombora.build",
                response=json.dumps({"result": {"bundleHash": OTHER_HASH}})),
            log("eth_sendBundle response received", target_block=100, builder_host="rpc.quasar.win",
                response=json.dumps({"result": True})),
            log("bundle dropped", target_block=100, tx_hash=POST_HASH, reason="excluded"),
        ])

    def run_cli(self, *extra, json_output=True):
        args = [sys.executable, "-c", CLI_RUNNER, str(SCRIPT), self.base,
                "--rpc", self.base + "/l1", "--logs", "-", "--horizon", "1", "--poster", POSTER,
                "--builders", "https://rpc.titanbuilder.xyz,https://rpc.bombora.build"]
        if json_output:
            args.append("--json")
        return subprocess.run(args + list(extra), input=self.logs, capture_output=True, text=True, timeout=10)

    def test_cli_reports_all_builders_and_exact_post_absence(self):
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual(report["category"], "OTHER_BUILDER")
        self.assertFalse(report["post_in_target"])
        self.assertEqual(report["landed"], {"status": "not_found", "through_block": 101})
        stats = report["builder_bundle_stats"]
        self.assertEqual(stats["titanbuilder.xyz"][0]["result"]["status"], "SimulationFail")
        self.assertEqual(stats["bombora.build"][0]["result"]["status"], "SimulationPass")
        self.assertEqual(stats["quasar.win"][0]["availability"], "missing_hash")
        queries = {path: request for path, request in self.requests if path.startswith("/stats/")}
        self.assertEqual(set(queries), {"/stats/titan", "/stats/bombora"})
        self.assertEqual(queries["/stats/titan"]["params"], [{"bundleHash": POST_HASH}])
        self.assertEqual(queries["/stats/bombora"]["params"], [{"bundleHash": OTHER_HASH}])
        self.assertIn(("/relay/relay/v1/data/bidtraces/proposer_payload_delivered?block_hash=" + BLOCK_HASH, None),
                      self.requests)
        self.assertEqual(summary["type"], "summary")
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["accepted"], 1)
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["excluded"], 1)
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["simulation_failed"], 1)

    def test_cli_text_preserves_complete_simulation_error(self):
        result = self.run_cli(json_output=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(self.simulation_error, result.stdout)
        self.assertIn("bombora.build bundle stats:", result.stdout)
        self.assertIn("per-builder summary", result.stdout)
        self.assertIn("accepted=1 rejected=0 included=0 excluded=1", result.stdout)

    def use_new_builder_logs(self):
        self.logs = "\n".join([dispatch(), fanout(
            delivery("rpc.quasar.win", bundle_hash=POST_HASH),
            delivery("rpc.eurekabuilder.xyz", bundle_hash=OTHER_HASH),
            delivery("rpc.buildernet.org", bundle_hash=BLOCK_HASH),
        )])

    def test_cli_queries_quasar_and_eureka_with_their_actual_parameter_names(self):
        self.use_new_builder_logs()
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        stats = report["builder_bundle_stats"]
        self.assertEqual(stats["quasar.win"][0]["result"], self.quasar_result)
        self.assertEqual(stats["eurekabuilder.xyz"][0]["result"], self.eureka_result)
        self.assertEqual(stats["buildernet.org"][0]["availability"], "unsupported")
        self.assertIn("deprecated", stats["buildernet.org"][0]["reason"])
        self.assertEqual({path for path, _ in self.requests if path.startswith("/stats/")},
                         {"/stats/quasar", "/stats/eureka"})
        self.assertEqual(summary["builders"]["eurekabuilder.xyz"]["simulation_failed"], 0)
        self.assertEqual(summary["builders"]["quasar.win"]["accepted"], 1)

    def test_cli_quasar_lowercase_failures_are_counted_and_raw_status_is_preserved(self):
        self.use_new_builder_logs()
        for status in ("invalid", "simulation_fail"):
            with self.subTest(status=status):
                self.quasar_result["status"] = status
                result = self.run_cli()
                self.assertEqual(result.returncode, 0, result.stderr)
                report, summary = map(json.loads, result.stdout.splitlines())
                self.assertEqual(report["builder_bundle_stats"]["quasar.win"][0]["result"]["status"], status)
                counts = summary["builders"]["quasar.win"]
                self.assertEqual(counts["simulation_failed"], 1)
                self.assertEqual(counts["accepted"], 1)
                self.assertEqual(counts["rejected"], 0)

    def test_cli_quasar_landed_claim_does_not_override_canonical_evidence(self):
        self.use_new_builder_logs()
        self.quasar_result["status"] = "landed"
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual(report["target_outcome"], "excluded")
        for host in ("quasar.win", "eurekabuilder.xyz"):
            counts = summary["builders"][host]
            self.assertEqual(counts["included"], 0)
            self.assertEqual(counts["simulation_failed"], 0)
            self.assertEqual(counts["excluded"], 1)

    def test_cli_missing_stats_and_rate_limits_are_not_bundle_rejections(self):
        self.use_new_builder_logs()
        self.stats_error = {"/stats/quasar": {"code": -32005, "message": "rate limit exceeded"},
                            "/stats/eureka": {"code": -32001, "message": "bundle not found"}}
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        for host, path in (("quasar.win", "/stats/quasar"), ("eurekabuilder.xyz", "/stats/eureka")):
            stats = report["builder_bundle_stats"][host][0]
            self.assertEqual(stats["availability"], "unavailable")
            self.assertEqual(stats["error"], self.stats_error[path])
            self.assertEqual(summary["builders"][host]["rejected"], 0)
            self.assertEqual(summary["builders"][host]["simulation_failed"], 0)

    def test_cli_mismatched_stats_metadata_is_retained_but_not_counted(self):
        self.use_new_builder_logs()
        self.quasar_result.update(status="invalid", targetBlock=101)
        self.eureka_result.update(status="SimulationFail", bundleHash=BLOCK_HASH)
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        for host, expected in (("quasar.win", self.quasar_result), ("eurekabuilder.xyz", self.eureka_result)):
            stats = report["builder_bundle_stats"][host][0]
            self.assertEqual(stats["availability"], "unavailable")
            self.assertEqual(stats["result"], expected)
            self.assertIn("does not match", stats["error"])
            self.assertEqual(summary["builders"][host]["simulation_failed"], 0)

    def test_cli_text_includes_quasar_timings_and_eureka_detail(self):
        self.use_new_builder_logs()
        result = self.run_cli(json_output=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("quasar.win bundle stats:", result.stdout)
        self.assertIn('"receivedAt": 988', result.stdout)
        self.assertIn('"error": "HeadAdvanced"', result.stdout)
        self.assertIn("No verified public BuilderNet bundle-stats API", result.stdout)

    def test_cli_successful_only_logs_report_inclusion_and_summary(self):
        self.target = block(transactions=[transaction(POST_HASH)], extra="Titan")
        self.logs = "\n".join([dispatch(), fanout(delivery("rpc.titanbuilder.xyz"),
                                                delivery("rpc.bombora.build", bundle_hash=OTHER_HASH))])
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual(report["target_outcome"], "included")
        self.assertEqual(summary["attempts"], 1)
        for host in ("titanbuilder.xyz", "bombora.build"):
            self.assertEqual(summary["builders"][host]["accepted"], 1)
            self.assertEqual(summary["builders"][host]["included"], 1)
        self.assertEqual(len([path for path, _ in self.requests if path.startswith("/stats/")]), 2)

    def test_cli_rejected_only_logs_report_rejection_without_stats_queries(self):
        self.logs = "\n".join([dispatch(), fanout(delivery("rpc.titanbuilder.xyz", "rejected", None))])
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        counts = summary["builders"]["titanbuilder.xyz"]
        self.assertEqual(counts["rejected"], 1)
        self.assertEqual(counts["accepted"], 0)
        self.assertEqual(counts["excluded"], 0)
        self.assertEqual(report["builder_bundle_stats"], {})
        self.assertFalse(any(path.startswith("/stats/") for path, _ in self.requests))

    def test_cli_multiple_posts_and_duplicate_logs_count_distinct_attempts(self):
        self.target = block(transactions=[transaction(OTHER_HASH)], extra="Titan")
        events = [dispatch(), fanout(delivery("rpc.titanbuilder.xyz")),
                  dispatch(post=OTHER_HASH), fanout(delivery("rpc.titanbuilder.xyz", bundle_hash=OTHER_HASH),
                                                  post=OTHER_HASH)]
        self.logs = "\n".join(events + events)
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        first, second, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual((first["post_tx"], second["post_tx"]), (POST_HASH, OTHER_HASH))
        counts = summary["builders"]["titanbuilder.xyz"]
        self.assertEqual(counts["observed"], 2)
        self.assertEqual(counts["accepted"], 2)
        self.assertEqual(counts["included"], 1)
        self.assertEqual(counts["excluded"], 1)

    def test_cli_later_inclusion_does_not_rewrite_target_exclusion(self):
        self.later_blocks[101] = block(101, [transaction(POST_HASH)])
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual(report["landed"]["block"], 101)
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["included"], 0)
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["excluded"], 1)

    def test_cli_transport_failures_do_not_become_bundle_rejections(self):
        self.logs = "\n".join([dispatch(), fanout(
            delivery("rpc.titanbuilder.xyz", "transport_error", None),
            delivery("rpc.bombora.build", "deadline_exceeded", None),
            delivery("rpc.quasar.win", "abandoned_after_acknowledgement", None),
        )])
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        for host, failure in (("titanbuilder.xyz", "transport_error"), ("bombora.build", "deadline_exceeded"),
                              ("quasar.win", "abandoned_after_acknowledgement")):
            self.assertEqual(summary["builders"][host][failure], 1)
            self.assertEqual(summary["builders"][host]["rejected"], 0)
            self.assertEqual(summary["builders"][host]["accepted"], 0)

    def test_cli_stats_candidate_inclusion_does_not_prove_canonical_inclusion(self):
        self.stats_status = {"titan": "IncludedInBlock", "bombora": "Submitted"}
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        for host in ("titanbuilder.xyz", "bombora.build"):
            self.assertEqual(summary["builders"][host]["included"], 0)
            self.assertEqual(summary["builders"][host]["excluded"], 1)
            self.assertEqual(summary["builders"][host]["simulation_failed"], 0)

    def test_cli_raw_acknowledgement_without_post_hash_remains_unresolved(self):
        self.logs = log("eth_sendBundle response received", target_block=100,
                        builder_host="rpc.titanbuilder.xyz", response='{"result":true}')
        self.target = block(transactions=[transaction(POST_HASH)])
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        counts = summary["builders"]["titanbuilder.xyz"]
        self.assertEqual(counts["accepted"], 1)
        self.assertEqual(counts["unresolved"], 1)
        self.assertEqual(counts["uncorrelated"], 1)
        self.assertEqual(counts["included"], 0)

    def test_cli_empty_or_plain_text_logs_explain_missing_targets(self):
        for logs, count in (("", 0), ("WARN bundle dropped reason=excluded target_block=100", 1)):
            with self.subTest(logs=logs):
                self.logs = logs
                result = self.run_cli()
                self.assertEqual(result.returncode, 2)
                self.assertIn(f"{count} lines, 0 JSON events", result.stderr)
                self.assertEqual(self.requests, [])

    def test_cli_rejects_other_network_before_querying_public_services(self):
        self.chain_id = hex(560048)
        result = self.run_cli()
        self.assertEqual(result.returncode, 2)
        self.assertIn("mainnet only", result.stderr)
        self.assertEqual(len(self.requests), 1)
        self.assertEqual(self.requests[0][1]["method"], "eth_chainId")

    def test_cli_retains_stats_when_target_has_not_been_produced(self):
        self.target = None
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual(report["category"], "NOT_YET_PRODUCED")
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["unresolved"], 1)
        self.assertEqual(report["builder_bundle_stats"]["titanbuilder.xyz"][0]["result"]["status"],
                         "SimulationFail")

    def test_cli_retains_stats_on_target_rpc_failure_and_returns_nonzero(self):
        self.block_error = {"code": -32000, "message": "fixture read failed"}
        result = self.run_cli()
        self.assertEqual(result.returncode, 1, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual(report["category"], "RPC_UNAVAILABLE")
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["unresolved"], 1)
        self.assertEqual(summary["builders"]["titanbuilder.xyz"]["excluded"], 0)
        self.assertIn("fixture read failed", report["error"])
        self.assertEqual(report["builder_bundle_stats"]["bombora.build"][0]["result"]["status"],
                         "SimulationPass")

    def test_cli_relay_outage_remains_inconclusive(self):
        self.relay_error = True
        self.target = block(extra="geth")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertEqual(report["category"], "UNKNOWN")
        self.assertEqual(report["relays_unavailable"], ["fixture"])

    def test_cli_matches_exact_post_hash(self):
        self.target = block(transactions=[transaction(POST_HASH)])
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report, summary = map(json.loads, result.stdout.splitlines())
        self.assertTrue(report["post_in_target"])
        self.assertEqual(report["landed"], {"status": "included", "block": 100, "block_hash": BLOCK_HASH})
        reads = [request for path, request in self.requests
                 if path == "/l1" and request["method"] == "eth_getBlockByNumber"]
        self.assertEqual(len(reads), 1)

    def test_cli_rejects_negative_scan_horizon_before_requests(self):
        result = self.run_cli("--horizon", "-1")
        self.assertEqual(result.returncode, 2)
        self.assertIn("nonnegative", result.stderr)
        self.assertEqual(self.requests, [])


class RunbookTest(unittest.TestCase):
    def test_documented_commands_pass_sourced_builders_to_child(self):
        runbook = (SCRIPT.parents[1] / "docs/runbooks/ethereum-node.md").read_text()
        commands = runbook.split("### Why a post was excluded", 1)[1].split("```bash\n", 1)[1].split("```", 1)[0]
        commands = commands.replace("<POSTER_ADDRESS>", POSTER)
        wrapper = '''
sudo() { :; }
python3() {
    "$REVIEW_PYTHON" -c 'import json, os, sys; print(json.dumps({"args":sys.argv[1:], "builders":os.environ.get("EEZ_L1_BUILDER_RPC_URL")}))' "$@"
}
'''
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, ".env").write_text("EEZ_L1_BUILDER_RPC_URL=https://rpc.titanbuilder.xyz\n")
            env = {key: value for key, value in os.environ.items() if key != "EEZ_L1_BUILDER_RPC_URL"}
            env["REVIEW_PYTHON"] = sys.executable
            result = subprocess.run(["bash", "-eu", "-c", wrapper + commands], cwd=directory,
                                    env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        reports = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertGreater(len(reports), 0)
        for report in reports:
            args = report["args"]
            builders = args[args.index("--builders") + 1] if "--builders" in args else report["builders"]
            self.assertEqual(builders, "https://rpc.titanbuilder.xyz")


if __name__ == "__main__":
    unittest.main()
