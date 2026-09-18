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

        # RollupTiming::future_count rounds the proof reservation up to an L2
        # block. prove_with_retry must start by T - proof - slack. The gap is
        # the time available for composition, witnesses and blob preparation.
        future = (proof + slack + l2 - 1) // l2 - 1
        trigger_lead = (future + 1) * l2
        self.assertEqual((future, trigger_lead), (2, 6000))
        self.assertGreaterEqual(proof, 4000)
        self.assertGreaterEqual(trigger_lead - proof - slack, 1500)


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
                  "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS": "60000",
                  "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS": "150",
                  "EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS": "0",
                  "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE": "2000000000",
                  "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS": "100000000000",
                  "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS": "100000000000"}
        self.assertEqual(configure(values), values)
        without_priority_bound = dict(values)
        without_priority_bound.pop("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE")
        with self.assertRaises(ValueError):
            configure(without_priority_bound)
        without_priority_bound["EEZ_L1_POSTBATCH_PRIORITY_FEE"] = "1000000000"
        self.assertEqual(configure(without_priority_bound), without_priority_bound)
        for invalid in ({"EEZ_SETTLEMENT_PURE_L2_MOD": "always"},
                        {"EEZ_SETTLEMENT_PURE_L2_MODE": True},
                        {"EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS": 0},
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
                self.assertTrue(self.REQUIRED.issubset(values))
                self.assertTrue(all(isinstance(value, str) for value in values.values()))

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
    def run_readiness(self, log, log_status=0):
        script = (REPO / "testing/kurtosis/run-ci.sh").read_text()
        start = script.index('while (( SECONDS < deadline )); do\n    node_logs=')
        end = script.index('\necho "==> running native cross-chain wave suite"', start)
        setup = r'''
deadline=$((SECONDS + 60))
KURTOSIS_ENCLAVE=fixture-ci
kurtosis() {
    [[ "$*" == "service logs fixture-ci eez-node" ]] || return 99
    cat "$READINESS_LOG"
    return "$READINESS_LOG_STATUS"
}
timeout() { shift; "$@"; }
sleep() { SECONDS=$deadline; }
'''
        with tempfile.TemporaryDirectory(prefix="eez-readiness.") as temporary:
            path = Path(temporary) / "node.log"
            path.write_text(log)
            return subprocess.run(
                ["bash", "-euo", "pipefail", "-c", setup + script[start:end]],
                env={**os.environ, "READINESS_LOG": str(path),
                     "READINESS_LOG_STATUS": str(log_status)},
                capture_output=True, text=True, timeout=10,
            )

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
            (tools / "git").write_text("#!/bin/sh\nprintf '%040d\\n' 1\n")
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
                            "mev-relay-api", "mev-relay-housekeeper"):
                lines = (root / "results" / f"{service}.log").read_text().splitlines()
                self.assertEqual(len(lines), 250, lines)
                self.assertEqual(lines[0], "fixture log 0")

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
