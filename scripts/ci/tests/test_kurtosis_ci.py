"""CI resource overrides and failure artifacts; offline unless explicitly opted in."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
import uuid

try:
    import yaml
except ImportError:
    yaml = None


REPO = Path(__file__).resolve().parents[3]
RESOURCES = REPO / "testing/kurtosis/resources.star"
RELAY_IMAGE = "example.invalid/relay@sha256:" + "a" * 64
ETH_ARGS = {"mev_type": "flashbots", "mev_params": {"mev_relay_image": RELAY_IMAGE}}
SERVICES = ("mev-relay-redis", "mev-relay-api", "mev-relay-housekeeper", "mev-relay-website")


def fail(message):
    raise ValueError(message)


@unittest.skipUnless(shutil.which("jq"), "jq is required")
class GenesisReferenceReadinessTest(unittest.TestCase):
    def run_until_deployment(self, responses):
        class DeploymentReached(Exception):
            pass

        class FinalityUnavailable(Exception):
            pass

        events = []
        test = self

        def wait(**kwargs):
            events.append("wait")
            test.assertEqual(kwargs["service_name"], "el-1-reth-lighthouse")
            test.assertEqual(kwargs["timeout"], "300s")
            test.assertEqual(kwargs["assertion"], "==")
            test.assertIs(kwargs["target_value"], True)
            recipe = kwargs["recipe"]
            test.assertEqual(recipe["port_id"], "rpc")
            request = json.loads(recipe["body"])
            test.assertEqual(request["method"], "eth_getBlockByNumber")
            # L1 block zero is the devnet's genesis reference; no finality wait.
            test.assertEqual(request["params"], ["0x0", False])
            expression = recipe["extract"][kwargs["field"].removeprefix("extract.")]
            for response in responses:
                value = json.loads(subprocess.check_output(
                    ["jq", "-c", expression], input=json.dumps(response), text=True))
                events.append(value)
                if value is True:
                    return
            raise FinalityUnavailable

        def run_sh(**kwargs):
            if kwargs["description"].startswith("deploy EEZ contracts"):
                test.assertEqual(kwargs["env_vars"]["EEZ_L1_REFERENCE"], "genesis")
                events.append("deploy")
                raise DeploymentReached

        participant = SimpleNamespace(
            el_context=SimpleNamespace(service_name="el-1-reth-lighthouse", rpc_http_url="http://l1:8545"),
            cl_context=SimpleNamespace(enr="enr", multiaddr="peer", peer_id="id"))
        ethereum = SimpleNamespace(run=lambda *_: SimpleNamespace(all_participants=[participant]))
        resources = SimpleNamespace(configure_mev_relay_cpu=lambda *_: None,
                                    relay_startup_plan=lambda plan: plan)
        namespace = {
            "import_module": lambda name: ethereum if "ethereum-package" in name else resources,
            "StoreSpec": dict, "PostHttpRequestRecipe": dict, "fail": fail,
        }
        source = REPO / "testing/kurtosis/main.star"
        exec(compile(source.read_text(), str(source), "exec"), namespace)
        args = {"ethereum_package": {}, "eez": {
            "poster_key": "fixture", "proof_signer_key": "fixture",
            "builder_rpc_url": "http://builder:8645", "blob_sidecar_encoding": "eip7594",
        }}
        try:
            namespace["run"](SimpleNamespace(wait=wait, run_sh=run_sh), args)
        except DeploymentReached:
            return "deploy", events
        except FinalityUnavailable:
            return "unavailable", events
        self.fail("startup did not reach the finality/deployment boundary")

    def test_deployment_waits_for_the_l1_genesis_header(self):
        header = {"hash": "0x" + "11" * 32, "number": "0x0"}
        outcome, events = self.run_until_deployment([
            {"error": {"code": -32000, "message": "connection refused"}},
            {"result": None}, {"result": header},
        ])
        self.assertEqual(outcome, "deploy")
        self.assertEqual(events, ["wait", False, False, True, "deploy"])

    def test_missing_or_errored_genesis_header_never_reaches_deployment(self):
        for response in ({"result": None}, {"result": []},
                         {"error": {"code": -32000}, "result": {"hash": "0x1234"}}):
            with self.subTest(response=response):
                outcome, events = self.run_until_deployment([response])
                self.assertEqual(outcome, "unavailable")
                self.assertEqual(events, ["wait", False])


@unittest.skipIf(yaml is None, "PyYAML not installed")
class CiTimingTest(unittest.TestCase):
    def test_parallel_suite_has_preparation_time_without_moving_slot_deadlines(self):
        args = yaml.safe_load((REPO / "testing/kurtosis/ci-args.yaml").read_text())
        timing = args["eez"]
        l1 = timing["l1_block_time_ms"]
        l2 = timing["l2_block_time_ms"]
        proof = timing["proof_time_ms"]
        slack = timing["submission_slack_ms"]
        self.assertEqual((l1, l2, slack), (12000, 2000, 500))
        self.assertEqual(args["ethereum_package"]["network_params"]["slot_duration_ms"], l1)
        self.assertEqual(timing["bundle_submission_lead_ms"], 7500)

        # The explicit position leaves the same target/deadline while reserving
        # preparation time independently of the configured proof estimate.
        position = int(timing["settlement_env"]["EEZ_SETTLEMENT_START_AFTER_BLOCK"])
        delay = int(timing["settlement_env"].get("EEZ_SETTLEMENT_START_DELAY_MS", "0"))
        trigger_lead = l1 - position * l2 - delay
        self.assertEqual((position, trigger_lead), (3, 6000))
        self.assertGreaterEqual(proof, 4000)
        self.assertGreaterEqual(trigger_lead - proof - slack - 150, 1350)


class RelayReadinessTest(unittest.TestCase):
    def test_housekeeper_completion_gates_the_next_relay_service(self):
        namespace = {"ExecRecipe": dict, "struct": lambda **kwargs: SimpleNamespace(**kwargs)}
        exec(compile(RESOURCES.read_text(), str(RESOURCES), "exec"), namespace)
        for ready in (False, True):
            with self.subTest(ready=ready):
                calls = []
                config = {"image": RELAY_IMAGE, "env_vars": {"GENESIS_FORK_VERSION": "fixture"}}
                service = object()

                def add_service(name, config, **kwargs):
                    calls.append(name)
                    self.assertIs(config, original)
                    self.assertEqual(kwargs, {"force_update": True})
                    return service

                def wait(**kwargs):
                    calls.append("database-ready")
                    self.assertEqual(kwargs["service_name"], "mev-relay-housekeeper")
                    self.assertEqual((kwargs["field"], kwargs["assertion"], kwargs["target_value"]),
                                     ("code", "==", 0))
                    self.assertEqual(kwargs["timeout"], "120s")
                    if not ready:
                        raise RuntimeError("database startup failed")

                original = config
                plan = SimpleNamespace(add_service=add_service, wait=wait, run_sh=object())
                wrapped = namespace["relay_startup_plan"](plan)
                self.assertIs(wrapped.run_sh, plan.run_sh)
                try:
                    self.assertIs(wrapped.add_service("redis", config, force_update=True), service)
                    self.assertIs(wrapped.add_service("mev-relay-housekeeper", config, force_update=True), service)
                    wrapped.add_service("mev-relay-api", config, force_update=True)
                except RuntimeError:
                    self.assertFalse(ready)
                self.assertEqual(calls, ["redis", "mev-relay-housekeeper", "database-ready"]
                                 + (["mev-relay-api"] if ready else []))


class RelayResourcesTest(unittest.TestCase):
    def setUp(self):
        # This helper uses the Python-compatible subset of Starlark. Emulate
        # Starlark's type() for fast offline tests; the opt-in test below uses
        # the real interpreter, admission checks and Docker limits.
        namespace = {"ServiceConfig": dict, "fail": fail,
                     "type": lambda value: type(value).__name__}
        exec(compile(RESOURCES.read_text(), str(RESOURCES), "exec"), namespace)
        self.configure = namespace["configure_mev_relay_cpu"]
        self.calls = []
        self.plan = SimpleNamespace(set_service=lambda **kwargs: self.calls.append(kwargs))

    def test_other_profiles_keep_upstream_defaults(self):
        self.configure(self.plan, {}, None)
        self.assertEqual(self.calls, [])

    def test_all_four_limits_fit_hosted_without_changing_other_settings(self):
        self.configure(self.plan, ETH_ARGS, {"min": 100, "max": 2000})
        self.assertEqual({call["name"] for call in self.calls}, set(SERVICES))
        for call in self.calls:
            self.assertEqual(call["config"], {
                "image": "redis:alpine" if call["name"] == "mev-relay-redis" else RELAY_IMAGE,
                "min_cpu": 100, "max_cpu": 2000,
            })

    def test_invalid_limits_fail_before_any_override(self):
        for limits in (False, [], {}, {"min": 100}, {"max": 2000},
                       {"min": 0, "max": 2000}, {"min": 100, "max": 99},
                       {"min": "100", "max": 2000}, {"min": True, "max": 2000},
                       {"min": 100, "max": 2000, "typo": 1}):
            with self.subTest(limits=limits), self.assertRaises(ValueError):
                self.configure(self.plan, ETH_ARGS, limits)
            self.assertEqual(self.calls, [])


    def test_requires_flashbots_and_an_explicit_relay_image(self):
        for args in ({}, {"mev_type": "flashbots"},
                     {"mev_type": "none", "mev_params": ETH_ARGS["mev_params"]}):
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.configure(self.plan, args, {"min": 100, "max": 2000})
            self.assertEqual(self.calls, [])


class SettlementEnvironmentTest(unittest.TestCase):
    REQUIRED = {
        "EEZ_SETTLEMENT_START_AFTER_BLOCK",
        "EEZ_SETTLEMENT_PURE_L2_MODE",
        "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS",
        "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS",
        "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
        "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE",
    }

    def test_transports_complete_mandatory_policy_as_strings(self):
        source = REPO / "testing/kurtosis/main.star"
        namespace = {"import_module": lambda _: None, "fail": fail,
                     "type": lambda value: "string" if isinstance(value, str) else type(value).__name__}
        exec(compile(source.read_text(), str(source), "exec"), namespace)
        configure = namespace["settlement_environment"]
        with self.assertRaises(ValueError):
            configure({})
        values = {"EEZ_SETTLEMENT_PURE_L2_MODE": "interval",
                  "EEZ_SETTLEMENT_START_AFTER_BLOCK": "3",
                  "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS": "60000",
                  "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS": "150",
                  "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE": "2000000000",
                  "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS": "100000000000",
                  "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS": "100000000000"}
        self.assertEqual(configure(values), values)
        for position in ("3", "", "0", "invalid"):
            explicit = dict(values, EEZ_SETTLEMENT_START_AFTER_BLOCK=position)
            self.assertEqual(configure(explicit), explicit)
        without_position = dict(values)
        without_position.pop("EEZ_SETTLEMENT_START_AFTER_BLOCK")
        with self.assertRaisesRegex(ValueError, "requires EEZ_SETTLEMENT_START_AFTER_BLOCK"):
            configure(without_position)
        self.assertNotIn("EEZ_SETTLEMENT_START_DELAY_MS", configure(values))
        for name in ("EEZ_SETTLEMENT_START_DELAY_MS", "EEZ_PREPARATION_TIME_MS"):
            for value in ("0", "250", "", "-1", "invalid"):
                with self.subTest(name=name, value=value):
                    # Preserve explicit empty/obsolete/conflicting values for runtime rejection.
                    explicit = dict(values, **{name: value})
                    self.assertEqual(configure(explicit), explicit)
        for margin in ("", "500", "2000", "bad"):
            explicit = dict(values, EEZ_L1_POSTBATCH_GAS_MARGIN_BPS=margin)
            self.assertEqual(configure(explicit), explicit)
        # Transported unchanged; the node parses and bounds it.
        for percentile in ("50", "90", "0"):
            explicit = dict(values, EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE=percentile)
            self.assertEqual(configure(explicit), explicit)
        cross_chain = dict(values,
                           EEZ_L1_POSTBATCH_CROSS_CHAIN_PRIORITY_PERCENTILE="90",
                           EEZ_L1_POSTBATCH_CROSS_CHAIN_MAX_PRIORITY_FEE="3500000000")
        self.assertEqual(configure(cross_chain), cross_chain)
        for name in [
            'EEZ_FEES_L1_TO_L2_ENABLED',
            'EEZ_FEES_L2_TO_L1_ENABLED',
            'EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP',
            'EEZ_FEES_L1_PER_L2_NUMERATOR',
            'EEZ_FEES_L1_PER_L2_DENOMINATOR',
            'EEZ_FEES_PROVING_COST_L1',
            'EEZ_FEES_OPERATIONS_COST_L1',
            'EEZ_FEES_TREASURY_COST_L1',
            'EEZ_FEES_RISK_BPS',
            'EEZ_FEES_PROFIT_BPS',
            'EEZ_FEES_EXCLUDED_SENDERS',
            'EEZ_FEES_L1_REFUND_RECIPIENT',
            'EEZ_FEES_L1_REFUND_PERCENT',
            'EEZ_L2_FEE_SHADOW_POLICY',
            'EEZ_L1_FEE_REFUND_POLICY',
            'EEZ_L2_FEE_ENFORCE_TARIFF',
        ]:
            self.assertNotIn(name, configure(values))
            for value in ("true", "false", "", "invalid", "90"):
                explicit = dict(values, **{name: value})
                self.assertEqual(configure(explicit), explicit)
            with self.assertRaises(ValueError):
                configure(dict(values, **{name: True}))
        self.assertIn('get("EEZ_FEES_L2_TO_L1_ENABLED", "false")', (REPO / "testing/kurtosis/main.star").read_text())
        without_priority_bound = dict(values)
        without_priority_bound.pop("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE")
        with self.assertRaises(ValueError):
            configure(without_priority_bound)
        without_priority_bound["EEZ_L1_POSTBATCH_PRIORITY_FEE"] = "1000000000"
        self.assertEqual(configure(without_priority_bound), without_priority_bound)
        for invalid in ({"EEZ_SETTLEMENT_PURE_L2_MOD": "always"},
                        {"EEZ_SETTLEMENT_PURE_L2_MODE": True},
                        {"EEZ_MAX_BLOCKS_PER_BATCH": "300"}):
            with self.assertRaises(ValueError):
                configure(invalid)

    @unittest.skipIf(yaml is None, "PyYAML not installed")
    def test_every_kurtosis_network_profile_supplies_mandatory_policy(self):
        for filename in (
            "blob-args.yaml",
            "ci-args.yaml",
            "crosschain-limit-args.yaml",
            "devnet-args.yaml",
            "partial-consumption-args.yaml",
            "settlement-policy-args.yaml",
        ):
            with self.subTest(filename=filename):
                args = yaml.safe_load((REPO / "testing/kurtosis" / filename).read_text())
                values = args["eez"]["settlement_env"]
                self.assertEqual(values["EEZ_L1_POSTBATCH_GAS_MARGIN_BPS"], "500")
                self.assertTrue(self.REQUIRED.issubset(values))
                self.assertTrue(all(isinstance(value, str) for value in values.values()))
                l1_ms = args["ethereum_package"]["network_params"]["seconds_per_slot"] * 1000
                l2_ms = args["eez"]["l2_block_time_ms"]
                position = int(values["EEZ_SETTLEMENT_START_AFTER_BLOCK"])
                delay = int(values.get("EEZ_SETTLEMENT_START_DELAY_MS", "0"))
                reserved = args["eez"]["proof_time_ms"] + args["eez"]["submission_slack_ms"] + 150
                self.assertGreaterEqual(position, 1)
                self.assertLess(position, l1_ms // l2_ms)
                self.assertGreaterEqual(delay, 0)
                self.assertLess(delay, l2_ms)
                self.assertLess(reserved, l1_ms - position * l2_ms - delay)
                self.assertNotIn("EEZ_PREPARATION_TIME_MS", values)

    def test_signer_block_limit_is_optional_and_passed_to_the_signer(self):
        source = (REPO / "testing/kurtosis/main.star").read_text()
        start = source.index('    signer_cmd = " ".join(')
        end = source.index('\n    plan.add_service(\n        name="eez-proof-signer"', start)
        import textwrap
        command_setup = textwrap.dedent(source[start:end])
        for options, expected in [({}, None), ({"proof_signer_max_request_blocks": 1024}, 1024)]:
            namespace = {"eez": options, "PROOF_SIGNER_GRPC_PORT": 50061, "fail": fail,
                         "type": lambda value: type(value).__name__}
            exec(compile(command_setup, "signer-command", "exec"), namespace)
            command = namespace["signer_cmd"]
            if expected is None:
                self.assertNotIn("--max-request-blocks", command)
            else:
                self.assertIn(f"--max-request-blocks={expected}", command)
        for value in (0, -1, True, "1024", "1024; unexpected-command", 1.5):
            namespace = {"eez": {"proof_signer_max_request_blocks": value},
                         "PROOF_SIGNER_GRPC_PORT": 50061, "fail": fail,
                         "type": lambda value: type(value).__name__}
            with self.subTest(value=value), self.assertRaises(ValueError):
                exec(compile(command_setup, "signer-command", "exec"), namespace)

class SettlementReadinessTest(unittest.TestCase):
    def run_readiness(self, log, log_status=0, gate="settlement"):
        script = (REPO / "testing/kurtosis/run-ci.sh").read_text()
        boundaries = {
            "settlement": ('while (( SECONDS < deadline )); do\n    node_logs=',
                           '\necho "==> running native cross-chain wave suite"', "eez-node"),
            "relay": ('relay_deadline=', '\necho "==> refreshing relay housekeeper', "mev-relay-api"),
            "builder": ('builder_deadline=', '\n# Canonical replay proves readiness', "fixture-builder"),
        }
        first, last, service = boundaries[gate]
        start = script.index(first)
        end = script.index(last, start)
        setup = r'''
deadline=$((SECONDS + 60))
readiness_started=$SECONDS
RESULT_DIR=$(dirname "$READINESS_LOG")
KURTOSIS_ENCLAVE=fixture-ci
KURTOSIS_BUILDER_SERVICE=fixture-builder
kurtosis() {
    if [[ "$*" == "service logs --all fixture-ci $READINESS_SERVICE" ]]; then
        cat "$READINESS_LOG"
    elif [[ "$*" == "service logs fixture-ci $READINESS_SERVICE" ]]; then
        tail -n 200 "$READINESS_LOG"
    else
        return 99
    fi
    return "$READINESS_LOG_STATUS"
}
timeout() { shift; "$@"; }
sleep() { SECONDS=$((SECONDS + 1000)); }
'''
        with tempfile.TemporaryDirectory(prefix="eez-readiness.") as temporary:
            path = Path(temporary) / "node.log"
            path.write_text(log)
            return subprocess.run(
                ["bash", "-euo", "pipefail", "-c", setup + script[start:end]],
                env={**os.environ, "READINESS_LOG": str(path),
                     "READINESS_SERVICE": service,
                     "READINESS_LOG_STATUS": str(log_status)},
                capture_output=True, text=True, timeout=10,
            )

    def test_readiness_survives_startup_evidence_leaving_the_default_log_tail(self):
        signals = {
            "relay": "validator registrations call processed numRegistrations=128",
            "builder": "Updated validator registrations len=128",
            "settlement": '{"event_name":"eez.deriver.safe.advanced"}',
        }
        for gate, signal in signals.items():
            with self.subTest(gate=gate):
                log = signal + "\n" + "later unrelated service activity\n" * 250
                result = self.run_readiness(log, gate=gate)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_empty_registration_sets_do_not_open_readiness_gates(self):
        for gate, log in (
            ("relay", "validator registrations call processed numRegistrations=0 numRegistrationsNew=0"),
            ("builder", "Updated validator registrations len=0"),
        ):
            with self.subTest(gate=gate):
                result = self.run_readiness(log, gate=gate)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("timeout", result.stderr)

    def test_canonical_safe_advance_makes_the_network_ready(self):
        log = '\x1b[36m[eez-node]\x1b[0m ' + json.dumps({"fields": {
            "event_name": "eez.deriver.safe.advanced", "to_block": 480,
        }}, separators=(",", ":"))
        result = self.run_readiness(log)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("network ready", result.stdout)

    def test_observer_outcomes_alone_do_not_prove_readiness(self):
        for outcome in ("Included { ... }", "Ok(Included { ... })",
                        "Ok(Dropped { ... })", 'Err("RPC unavailable")'):
            with self.subTest(outcome=outcome):
                log = json.dumps({"fields": {
                    "event_name": "eez.composer.bundle.observed", "outcome": outcome,
                }}, separators=(",", ":"))
                result = self.run_readiness(log)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("readiness timeout", result.stderr)
                self.assertNotIn("network ready", result.stdout)

    def test_unavailable_logs_do_not_prove_readiness(self):
        result = self.run_readiness("", log_status=1)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("readiness timeout", result.stderr)


class FailureArtifactsTest(unittest.TestCase):
    def test_full_logs_survive_cleanup_and_failure_is_not_hidden(self):
        with tempfile.TemporaryDirectory(prefix="eez-kurtosis-logs.") as temporary:
            root = Path(temporary)
            package = root / "testing/kurtosis"
            package.mkdir(parents=True)
            tools = root / "bin"
            tools.mkdir()
            for name in ("run-ci.sh", "ci-args.yaml"):
                shutil.copy2(REPO / "testing/kurtosis" / name, package / name)
            (root / "scripts").mkdir()
            genesis_check = root / "scripts/update-eezl2-genesis.sh"
            genesis_check.write_text('[[ "$1" == --check ]]\n')
            # No network is started or stopped. Fail at the start boundary and
            # exercise the real EXIT trap, artifact capture and result writer.
            (package / "start.sh").write_text('touch "$START_MARKER"\nexit 17\n')
            (package / "stop.sh").write_text("exit 0\n")
            (tools / "git").write_text("#!/bin/sh\ncase \"$*\" in *ls-files*) printf '160000 ';; esac\nprintf '%040d\\n' 1\n")
            (tools / "forge").write_text("#!/bin/sh\nexit 0\n")
            (tools / "kurtosis").write_text('''#!/usr/bin/env python3
import sys
args = sys.argv[1:]
if args[:2] == ["enclave", "inspect"]:
    print("fixture enclave")
elif args[:2] == ["service", "logs"]:
    assert args[2:] == ["--all", "fixture-ci", args[-1]], args
    for line in range(250):
        print(f"fixture log {line}")
else:
    raise SystemExit(f"Unexpected invocation: {args}")
''')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            env = {**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"],
                   "EEZ_CI_RESULT_DIR": str(root / "results"),
                   "START_MARKER": str(root / "started"),
                   "KURTOSIS_ENCLAVE": "fixture-ci", "KURTOSIS_ARGS_FILE": str(package / "ci-args.yaml")}
            result = subprocess.run(["bash", str(package / "run-ci.sh")], env=env,
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
            evidence = json.loads((root / "results/result.json").read_text())
            self.assertEqual((evidence["result"], evidence["exit_code"]), ("fail", 17))
            for service in ("eez-node", "eez-proof-signer", "el-2-reth-builder-lighthouse",
                            "el-1-reth-lighthouse", "cl-1-lighthouse-reth",
                            "cl-2-lighthouse-reth-builder", "vc-1-reth-lighthouse",
                            "mev-boost-1-lighthouse-reth",
                            "mev-relay-api", "mev-relay-housekeeper"):
                lines = (root / "results" / f"{service}.log").read_text().splitlines()
                self.assertEqual(len(lines), 250, lines)
                self.assertEqual(lines[0], "fixture log 0")
            # Relay/proposer diagnostics must remain available even when the
            # independent Actions artifact upload fails.
            for service in ("mev-relay-api", "mev-relay-housekeeper",
                            "el-2-reth-builder-lighthouse", "cl-1-lighthouse-reth",
                            "cl-2-lighthouse-reth-builder", "vc-1-reth-lighthouse",
                            "mev-boost-1-lighthouse-reth"):
                self.assertIn(f"==> {service} log tail", result.stderr)
            self.assertNotIn("fixture log 0\n", result.stderr)
            self.assertIn("fixture log 249\n", result.stderr)

            # A failed genesis check must fail before any network is started,
            # and must not leave a prior result available on a local rerun.
            (root / "started").unlink()
            genesis_check.write_text("exit 23\n")
            result = subprocess.run(["bash", str(package / "run-ci.sh")], env=env,
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
            self.assertFalse((root / "started").exists())
            self.assertFalse((root / "results/result.json").exists())

    def test_ci_genesis_check_rejects_missing_tool_before_any_compilation(self):
        with tempfile.TemporaryDirectory(prefix="eez-genesis-preflight.") as temporary:
            tools = Path(temporary)
            (tools / "dirname").symlink_to(shutil.which("dirname"))
            for name in ("cargo", "forge"):
                tool = tools / name
                tool.write_text('#!/bin/sh\necho "UNEXPECTED COMPILATION" >&2\nexit 99\n')
                tool.chmod(0o755)
            result = subprocess.run([shutil.which("bash"), str(REPO / "scripts/update-eezl2-genesis.sh"), "--check"],
                                    env={**os.environ, "PATH": str(tools), "GITHUB_ACTIONS": "true"},
                                    text=True, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertIn("refusing Cargo fallback", result.stderr)
            self.assertNotIn("UNEXPECTED COMPILATION", result.stderr)


@unittest.skipUnless(os.environ.get("EEZ_TEST_KURTOSIS_RESOURCES") == "1",
                     "set EEZ_TEST_KURTOSIS_RESOURCES=1 for an isolated real Kurtosis probe")
class RealRelayResourcesTest(unittest.TestCase):
    def test_readiness_wait_blocks_the_following_instruction_until_ready(self):
        with tempfile.TemporaryDirectory(prefix="eez-relay-readiness.") as temporary:
            root = Path(temporary)
            shutil.copy2(RESOURCES, root / "resources.star")
            (root / "kurtosis.yml").write_text("name: github.com/eez-ci/relay-readiness-probe\n")
            (root / "main.star").write_text('''resources = import_module("./resources.star")
def run(plan, args):
    if args["guarded"]:
        plan = resources.relay_startup_plan(plan)
    plan.add_service(name="mev-relay-housekeeper", config=ServiceConfig(
        image="alpine:3.20", cmd=["sh", "-c",
            "sleep 5; while true; do printf 'HTTP/1.0 200 OK\\\\r\\\\nContent-Length: 6\\\\r\\\\n\\\\r\\\\nready\\\\n' | nc -l -p 9064 -w 2; done"]))
    result = plan.exec(service_name="mev-relay-housekeeper", recipe=ExecRecipe(
        command=["wget", "-q", "-O", "-", "http://127.0.0.1:9064/metrics"]))
    plan.verify(value=result["code"], assertion="==", target_value=0)
''')
            for guarded in (False, True):
                enclave = "eez-relay-readiness-" + uuid.uuid4().hex[:12]
                env = dict(os.environ, KURTOSIS_ENCLAVE=enclave)
                try:
                    result = subprocess.run([
                        "kurtosis", "run", "--enclave", enclave, "--resource-check=true",
                        "--image-download", "missing", str(root), json.dumps({"guarded": guarded}),
                    ], env=env, capture_output=True, text=True, timeout=180)
                    output = result.stdout + result.stderr
                    if guarded:
                        self.assertEqual(result.returncode, 0, output)
                        self.assertIn("ready", result.stdout)
                    else:
                        self.assertNotEqual(result.returncode, 0, output)
                        self.assertIn("Connection refused", output)
                finally:
                    subprocess.run(["kurtosis", "enclave", "rm", "-f", enclave],
                                   env=env, check=True, timeout=60, capture_output=True)

    def test_overrides_apply_before_creation_and_preserve_memory_and_command(self):
        enclave = "eez-ci-resources-" + uuid.uuid4().hex[:12]
        with tempfile.TemporaryDirectory(prefix="eez-kurtosis-resources.") as temporary:
            root = Path(temporary)
            shutil.copy2(RESOURCES, root / "resources.star")
            (root / "kurtosis.yml").write_text("name: github.com/eez-ci/resource-probe\n")
            (root / "main.star").write_text('''resources = import_module("./resources.star")
def run(plan):
    for name in ["mev-relay-redis", "mev-relay-api", "mev-relay-housekeeper", "mev-relay-website"]:
        plan.add_service(name=name, config=ServiceConfig(
            image="redis:alpine" if name == "mev-relay-redis" else "alpine:3.20",
            cmd=["sh", "-c", "echo probe; sleep 120"],
            min_cpu=100000, max_cpu=100000, max_memory=32))
    resources.configure_mev_relay_cpu(plan, {
        "mev_type": "flashbots", "mev_params": {"mev_relay_image": "alpine:3.20"}
    }, {"min": 100, "max": 2000})
''')
            try:
                result = subprocess.run([
                    "kurtosis", "run", "--enclave", enclave, "--resource-check=true",
                    "--image-download", "missing", str(root),
                ], capture_output=True, text=True, timeout=120)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                ids = subprocess.check_output([
                    "docker", "ps", "-q", "--filter", f"label=com.kurtosistech.enclave-name={enclave}",
                    "--filter", "label=com.kurtosistech.container-type=user-service",
                ], text=True).split()
                self.assertEqual(len(ids), 4)
                containers = json.loads(subprocess.check_output(["docker", "inspect", *ids], text=True))
                for container in containers:
                    self.assertEqual(container["HostConfig"]["NanoCpus"], 2_000_000_000)
                    self.assertEqual(container["HostConfig"]["Memory"], 32_000_000)
                    self.assertEqual(container["Config"]["Cmd"], ["sh", "-c", "echo probe; sleep 120"])
            finally:
                # Delete this uniquely named fixture only, never existing enclaves.
                subprocess.run(["kurtosis", "enclave", "rm", "-f", enclave], check=True, timeout=60)


if __name__ == "__main__":
    unittest.main()
