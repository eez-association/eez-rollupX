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
            # Acknowledged but included: not reported.
            log("eth_sendBundle response received", builder_host="rpc.titanbuilder.xyz",
                target_block=26131578,
                response=json.dumps({"result": {"bundleHash": OTHER_HASH}})),
            # Dropped for another reason: not an exclusion.
            log("bundle dropped; composer will retry on next tick", target_block=26131579,
                reason="slot_skipped"),
        ]
        self.assertEqual(MODULE.parse_logs(lines), {
            26131577: {"timestamp": 1791268463, "post_tx": POST_HASH,
                       "bundles": {"titanbuilder.xyz": POST_HASH, "quasar.win": OTHER_HASH}},
        })

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
                self.assertEqual(parsed[100]["bundles"], {"titanbuilder.xyz": None})

    def test_bad_responses_do_not_crash_or_count_as_acknowledgements(self):
        for response in ('not json', 'null', '[]', 'true', '{}',
                         '{"error":{"code":-32601}}',
                         '{"result":true,"error":{"code":-1}}'):
            with self.subTest(response=response):
                events = [log("eth_sendBundle response received", target_block=100,
                              builder_host="rpc.titanbuilder.xyz", response=response),
                          log("bundle dropped", target_block=100, tx_hash=POST_HASH,
                              reason="excluded")]
                self.assertEqual(MODULE.parse_logs(events)[100]["bundles"], {})


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
            self.assertEqual(MODULE.bundle_stats("quasar.win", POST_HASH)["availability"],
                             "unsupported")
            self.assertEqual(MODULE.bundle_stats("titanbuilder.xyz", None)["availability"],
                             "missing_hash")
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
        self.info = {"timestamp": 1000, "post_tx": POST_HASH, "bundles": {}}

    def test_relay_outage_is_unknown_not_local(self):
        with patch.object(MODULE, "rpc", return_value=block()), \
                patch.object(MODULE, "http_json", side_effect=OSError("offline")):
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertEqual(report["category"], "UNKNOWN")
        self.assertEqual(set(report["relays_unavailable"]), set(MODULE.RELAYS))

    def test_stats_for_every_acknowledging_builder_even_when_another_wins(self):
        self.info["bundles"] = {"titanbuilder.xyz": POST_HASH, "bombora.build": OTHER_HASH,
                                "quasar.win": None}
        with patch.object(MODULE, "rpc", return_value=block(extra="Builder+ www.btcs.com")), \
                patch.object(MODULE, "relay_deliveries", return_value=(RELAYED, [])), \
                patch.object(MODULE, "bundle_stats", return_value={"availability": "fixture"}) as stats:
            report = MODULE.analyze(100, self.info, self.args, BUILDERS)
        self.assertEqual(report["category"], "OTHER_BUILDER")
        self.assertEqual(set(stats.call_args_list[i].args for i in range(stats.call_count)),
                         set(self.info["bundles"].items()))
        self.assertEqual(set(report["builder_bundle_stats"]), set(self.info["bundles"]))

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
    "titanbuilder.xyz": (base + "/stats/titan", "titan_getBundleStats"),
    "bombora.build": (base + "/stats/bombora", "bombora_getBundleStats"),
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
                if self.path == "/l1":
                    if request["method"] == "eth_chainId":
                        result = fixture.chain_id
                    elif request["method"] == "eth_getBlockByNumber":
                        if fixture.block_error:
                            self.reply({"jsonrpc": "2.0", "id": 1, "error": fixture.block_error})
                            return
                        number = int(request["params"][0], 16)
                        result = fixture.target if number == 100 else block(number, [transaction(OTHER_HASH)])
                    else:
                        self.reply({"error": {"code": -32601}}, 400)
                        return
                elif self.path == "/stats/titan":
                    result = {"status": "SimulationFail", "error": fixture.simulation_error}
                elif self.path == "/stats/bombora":
                    result = {"status": "SimulationPass", "error": ""}
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
        report = json.loads(result.stdout)
        self.assertEqual(report["category"], "OTHER_BUILDER")
        self.assertFalse(report["post_in_target"])
        self.assertEqual(report["landed"], {"status": "not_found", "through_block": 101})
        stats = report["builder_bundle_stats"]
        self.assertEqual(stats["titanbuilder.xyz"]["result"]["status"], "SimulationFail")
        self.assertEqual(stats["bombora.build"]["result"]["status"], "SimulationPass")
        self.assertEqual(stats["quasar.win"]["availability"], "unsupported")
        queries = {path: request for path, request in self.requests if path.startswith("/stats/")}
        self.assertEqual(set(queries), {"/stats/titan", "/stats/bombora"})
        self.assertEqual(queries["/stats/titan"]["params"], [{"bundleHash": POST_HASH}])
        self.assertEqual(queries["/stats/bombora"]["params"], [{"bundleHash": OTHER_HASH}])
        self.assertIn(("/relay/relay/v1/data/bidtraces/proposer_payload_delivered?block_hash=" + BLOCK_HASH, None),
                      self.requests)

    def test_cli_text_preserves_complete_simulation_error(self):
        result = self.run_cli(json_output=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(self.simulation_error, result.stdout)
        self.assertIn("bombora.build bundle stats:", result.stdout)

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
        report = json.loads(result.stdout)
        self.assertEqual(report["category"], "NOT_YET_PRODUCED")
        self.assertEqual(report["builder_bundle_stats"]["titanbuilder.xyz"]["result"]["status"],
                         "SimulationFail")

    def test_cli_retains_stats_on_target_rpc_failure_and_returns_nonzero(self):
        self.block_error = {"code": -32000, "message": "fixture read failed"}
        result = self.run_cli()
        self.assertEqual(result.returncode, 1, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["category"], "RPC_UNAVAILABLE")
        self.assertIn("fixture read failed", report["error"])
        self.assertEqual(report["builder_bundle_stats"]["bombora.build"]["result"]["status"],
                         "SimulationPass")

    def test_cli_relay_outage_remains_inconclusive(self):
        self.relay_error = True
        self.target = block(extra="geth")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["category"], "UNKNOWN")
        self.assertEqual(report["relays_unavailable"], ["fixture"])

    def test_cli_matches_exact_post_hash(self):
        self.target = block(transactions=[transaction(POST_HASH)])
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
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
