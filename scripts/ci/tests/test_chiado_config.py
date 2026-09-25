"""Static Compose validation: no daemon, deployment, or credentials required."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]


@unittest.skipUnless(shutil.which("docker"),
                     "Docker Compose CLI is required (no daemon needed)")
class ChiadoConfigTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-chiado-config-test.")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        for name in (
            "docker-compose.chiado-node.yml", "docker-compose.chiado.yml",
            "docker-compose.chiado-postings.yml",
            ".env.chiado.example", "scripts/chiado-up.sh",
            "configs/chiado/bootnodes.yaml", "configs/chiado/bootstrap_nodes.yaml",
        ):
            target = self.repo / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO / name, target, follow_symlinks=False)
        shutil.copy2(self.repo / ".env.chiado.example", self.repo / ".env")
        (self.repo / "deployments.env").touch()

    def render(self, overrides=None, compose_file=None):
        environment = {k: v for k, v in os.environ.items()
                       if not k.startswith(("EEZ_", "LH_", "COMPOSE_"))}
        command = ["docker", "compose"]
        if compose_file:
            command += ["-f", compose_file]
        return subprocess.run(
            command + ["config", "--no-env-resolution", "--format", "json"],
            cwd=self.repo, env={**environment, **(overrides or {})},
            text=True, capture_output=True,
        )

    def test_default_profile_loads_from_dotenv(self):
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stderr)
        services = json.loads(result.stdout)["services"]
        self.assertEqual(set(services), {"eez-node", "eez-proof-signer", "lighthouse"})
        node = services["eez-node"]["environment"]
        for key, value in {
            "EEZ_L1_CHAIN_ID": "10200", "EEZ_L1_BLOCK_TIME_MS": "5000",
            "EEZ_PROOF_TIME_MS": "750", "EEZ_PREPARATION_TIME_MS": "500",
            "EEZ_SUBMISSION_SLACK_MS": "700",
            "EEZ_BUNDLE_SUBMISSION_LEAD_MS": "7500",
            "EEZ_L1_BEACON_URL": "http://127.0.0.1:6152/",
            "EEZ_L1_BLOB_SIDECAR_ENCODING": "eip7594",
            "EEZ_SETTLEMENT_PURE_L2_MODE": "interval",
            "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS": "900000",
            "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS": "150",
            "EEZ_MAX_SPECULATIVE_DEPTH": "0",
            "EEZ_MAX_USER_TXS_PER_BUNDLE": "auto",
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS": "100000000000",
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS": "100000000000",
            "EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE": "1000000000",
            "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE": "2000000000",
        }.items():
            self.assertEqual(node[key], value, key)
        self.assertNotIn("EEZ_PROOF_SIGNER_KEY", node)
        self.assertNotIn("EEZ_L1_POSTER_KEY", services["eez-proof-signer"]["environment"])
        self.assertEqual(services["eez-proof-signer"]["environment"]["EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS"], "2048")
        self.assertEqual(services["eez-proof-signer"]["stop_grace_period"], "30s")
        self.assertEqual(services["eez-proof-signer"]["healthcheck"]["test"],
                         ["CMD", "/bin/bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/50061"])
        self.assertIn("--http.api=eth,net,web3,debug,trace", services["eez-node"]["command"])
        self.assertEqual(services["eez-node"]["command"].count(
            "--metrics=127.0.0.1:19091"), 1)
        self.assertIn("--log.stdout.format=json", services["eez-node"]["command"])
        for key in ("EEZ_SETTLEMENT_POLICY_ENABLED", "EEZ_MAX_BLOCKS_PER_BATCH",
                    "EEZ_SETTLEMENT_BLOB_FULLNESS_BPS", "EEZ_L1_POSTBATCH_PRIORITY_FEE",
                    "EEZ_MAX_BLOBS_PER_BATCH"):
            self.assertIsNone(node.get(key), key)
        self.assertEqual(services["lighthouse"]["image"], "sigp/lighthouse:v8.1.3")
        command = services["lighthouse"]["command"]
        self.assertIn("--semi-supernode", command)
        self.assertNotIn("--supernode", command)
        self.assertIn("--http-port=6152", command)

    def test_existing_bindings_are_not_read_by_static_render(self):
        deployment = self.repo / "deployments.env"
        contents = "MUST_NOT_RESOLVE=${NONEXISTENT:?must not read deployment bindings}\n"
        deployment.write_text(contents)
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(deployment.read_text(), contents)

    def test_posting_logging_survives_normal_startup_and_legacy_overlay(self):
        for configured in (None, "", str(self.root / "posting-history")):
            with self.subTest(configured=configured):
                overrides = {} if configured is None else {"POSTING_LOGS_DIR": configured}
                result = self.render(overrides, "docker-compose.chiado-node.yml")
                self.assertEqual(result.returncode, 0, result.stderr)
                node = json.loads(result.stdout)["services"]["eez-node"]
                mounts = [v for v in node["volumes"] if v["target"] == "/var/log/eez-posting"]
                self.assertEqual(len(mounts), 1)
                self.assertEqual(mounts[0]["source"], configured or str(self.repo / "data/posting-logs"))
                for flag in ("--log.file.format=json", "--log.file.filter=off,eez_posting=info",
                             "--log.file.directory=/var/log/eez-posting", "--log.file.name=posting.log",
                             "--log.file.max-size=10", "--log.file.max-files=20"):
                    self.assertEqual(node["command"].count(flag), 1)
                if configured:
                    legacy = self.render({**overrides, "COMPOSE_FILE":
                        "docker-compose.chiado-node.yml:docker-compose.chiado-postings.yml"})
                    self.assertEqual(legacy.returncode, 0, legacy.stderr)
                    self.assertEqual(json.loads(legacy.stdout)["services"]["eez-node"], node)

    def test_invalid_posting_mount_is_rejected(self):
        result = self.render({"POSTING_LOGS_DIR": "bad:mount:spec"})
        self.assertNotEqual(result.returncode, 0)

    def test_images_default_to_latest_when_unset_or_empty(self):
        for images in ({}, {"EEZ_NODE_IMAGE": "", "EEZ_PROOF_SIGNER_IMAGE": ""},
                       {"EEZ_NODE_IMAGE": "eez-node:local"}):
            with self.subTest(images=images):
                result = self.render(images)
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                self.assertEqual(services["eez-node"]["image"],
                                 images.get("EEZ_NODE_IMAGE") or "ghcr.io/eduadiez/rollupx-node:latest")
                self.assertEqual(services["eez-proof-signer"]["image"],
                                 "ghcr.io/eduadiez/rollupx-proof-signer:latest")

    def test_published_and_local_images_share_configuration_without_builds(self):
        for node, signer in (
            ("ghcr.io/eduadiez/rollupx-node@sha256:" + "a" * 64,
             "ghcr.io/eduadiez/rollupx-proof-signer@sha256:" + "b" * 64),
            ("ghcr.io/eduadiez/rollupx-node:" + "c" * 40,
             "ghcr.io/eduadiez/rollupx-proof-signer:" + "c" * 40),
            ("eez-node:local", "eez-proof-signer:local"),
        ):
            with self.subTest(node=node):
                result = self.render({"EEZ_NODE_IMAGE": node,
                                             "EEZ_PROOF_SIGNER_IMAGE": signer})
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                self.assertEqual(services["eez-node"]["image"], node)
                self.assertEqual(services["eez-proof-signer"]["image"], signer)
                for service in ("eez-node", "eez-proof-signer"):
                    self.assertNotIn("build", services[service])
                    self.assertEqual(services[service]["platform"], "linux/amd64")
                self.assertNotIn("EEZ_PROOF_SIGNER_KEY", services["eez-node"]["environment"])
                self.assertIn("EEZ_PROOF_SIGNER_KEY", services["eez-proof-signer"]["environment"])

    def test_missing_bindings_startup_preserves_existing_data(self):
        (self.repo / ".env").write_text("EEZ_L1_RPC_URL=http://example.invalid\n")
        (self.repo / "deployments.env").unlink()
        sentinels = []
        for directory in ("data/eez-l2", "data/chiado-lighthouse"):
            path = self.repo / directory / "keep"
            path.parent.mkdir(parents=True)
            path.write_text("existing deployment data")
            sentinels.append(path)
        # Fail if startup invokes a tool after finding missing bindings; never
        # contact Docker or a network endpoint during this regression test.
        tool_dir = self.root / "bin"
        tool_dir.mkdir()
        log = self.root / "invoked"
        for tool in ("docker", "cast", "curl", "jq"):
            stub = tool_dir / tool
            stub.write_text(f'#!/bin/sh\nprintf "%s\\n" "$0" >> "{log}"\nexit 99\n')
            stub.chmod(0o755)
        result = subprocess.run(
            ["bash", str(self.repo / "scripts/chiado-up.sh")],
            env={**os.environ, "PATH": str(tool_dir) + os.pathsep + os.environ["PATH"]},
            text=True, capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("deployments.env missing", result.stderr)
        self.assertFalse(log.exists(), "Startup must stop before invoking deployment or services")
        for path in sentinels:
            self.assertEqual(path.read_text(), "existing deployment data")

    @unittest.skipUnless(shutil.which("jq"), "jq is required")
    def test_startup_delegates_image_pulling_to_compose(self):
        for directory in ("data/chiado-l1", "datadir"):
            (self.repo / directory).mkdir(parents=True)
        genesis = self.repo / "datadir/genesis.json"
        genesis.write_text("{}")
        (self.repo / "data/jwt.hex").write_text("a" * 64)
        (self.repo / "deployments.env").write_text(
            "EEZ_REGISTRY_ADDRESS=0x1111111111111111111111111111111111111111\n"
            "EEZ_INITIAL_BLOCK_HASH=0x1234\n" + f"EEZ_L2_GENESIS_PATH={genesis}\n"
        )
        tool_dir = self.root / "bin"
        tool_dir.mkdir()
        log = self.root / "docker.log"
        stubs = {
            "docker": f'''printf '%s\\n' "$*" >> '{log}'
case "$*" in
    'compose -f docker-compose.chiado-node.yml config --quiet') exit 0 ;;
    'compose -f docker-compose.chiado-node.yml up -d') exit 71 ;;
    *) exit 99 ;;
esac
''',
            "cast": '''case "$1" in
    block-number) echo 10 ;;
    call) printf '0x1111\\n0x1234\\n0\\n' ;;
    *) exit 99 ;;
esac
''',
            "curl": '''echo '{"error":{"code":-32602}}'\n''',
        }
        for name, body in stubs.items():
            path = tool_dir / name
            path.write_text("#!/bin/sh\n" + body)
            path.chmod(0o755)
        result = subprocess.run(
            ["bash", str(self.repo / "scripts/chiado-up.sh")],
            env={**os.environ, "PATH": str(tool_dir) + os.pathsep + os.environ["PATH"]},
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 71, result.stdout + result.stderr)
        self.assertEqual(log.read_text().splitlines(), [
            "compose -f docker-compose.chiado-node.yml config --quiet",
            "compose -f docker-compose.chiado-node.yml up -d",
        ])

    def test_bootstrap_seeds_match_the_canonical_list(self):
        bootstrap = (self.repo / "configs/chiado/bootstrap_nodes.yaml").read_bytes()
        self.assertTrue(bootstrap.strip())
        self.assertEqual(bootstrap, (self.repo / "configs/chiado/bootnodes.yaml").read_bytes())

    def test_optional_enr_arguments_reach_both_beacons(self):
        # Exercise Compose's actual argv rendering. Empty optional arguments
        # must disappear, while explicit public endpoints must reach Lighthouse.
        environment = {
            "EEZ_CHIADO_LIGHTHOUSE_DATA": str(self.root / "beacon"),
            "EEZ_L1_JWT_SECRET": str(self.root / "jwt.hex"),
            "EEZ_CHIADO_CONFIG_DIR": str(self.repo / "configs/chiado"),
        }
        for profile in ("docker-compose.chiado-node.yml", "docker-compose.chiado.yml"):
            baseline = None
            for overrides in ({}, {"LH_ENR_ADDRESS": "", "LH_ENR_UDP_PORT": ""},
                              {"LH_ENR_ADDRESS": "198.51.100.10", "LH_ENR_UDP_PORT": "19005"},
                              {"LH_ENR_ADDRESS": "beacon.example.org", "LH_ENR_UDP_PORT": "9405"}):
                with self.subTest(profile=profile, overrides=overrides):
                    result = self.render({**environment, **overrides}, profile)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    command = json.loads(result.stdout)["services"]["lighthouse"]["command"]
                    self.assertNotIn("", command)
                    if baseline is None:
                        baseline = command
                        self.assertFalse(any(arg.startswith("--enr-") for arg in command))
                    if overrides.get("LH_ENR_ADDRESS"):
                        self.assertIn("--enr-address=" + overrides["LH_ENR_ADDRESS"], command)
                        self.assertIn("--enr-udp-port=" + overrides["LH_ENR_UDP_PORT"], command)
                        self.assertEqual([arg for arg in command if not arg.startswith("--enr-")],
                                         baseline)
                    else:
                        self.assertEqual(command, baseline)

    def test_optional_preparation_preserves_unset_empty_and_explicit_values(self):
        # The supplied preset is explicit; omission remains available to operators
        # choosing the runtime's rounded trigger instead.
        env_file = self.repo / ".env"
        env_file.write_text(env_file.read_text().replace("EEZ_PREPARATION_TIME_MS=500\n", ""))
        for value in (None, "500", ""):
            with self.subTest(value=value):
                overrides = {} if value is None else {"EEZ_PREPARATION_TIME_MS": value}
                result = self.render(overrides)
                self.assertEqual(result.returncode, 0, result.stderr)
                actual = json.loads(result.stdout)["services"]["eez-node"]["environment"]
                self.assertEqual(actual.get("EEZ_PREPARATION_TIME_MS"), value)
                # Explicit empty reaches Rust's startup validation; Compose must
                # not silently replace it with a default or an absent setting.
                self.assertEqual(actual["EEZ_PROOF_TIME_MS"], "750")
                self.assertEqual(actual["EEZ_SUBMISSION_SLACK_MS"], "700")

    def test_proof_budget_profile_and_shell_override_reach_node(self):
        for value in ("750", "1000", "1500", "", "invalid"):
            with self.subTest(value=value):
                result = self.render({"EEZ_PROOF_TIME_MS": value})
                self.assertEqual(result.returncode, 0, result.stderr)
                actual = json.loads(result.stdout)["services"]["eez-node"]["environment"]
                self.assertEqual(actual["EEZ_PROOF_TIME_MS"], value)
        # Compose supplies the default when no profile value exists. An empty
        # or invalid explicit value reaches Rust, whose timing validation fails.
        env_file = self.repo / ".env"
        env_file.write_text(env_file.read_text().replace("EEZ_PROOF_TIME_MS=750\n", ""))
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stderr)
        actual = json.loads(result.stdout)["services"]["eez-node"]["environment"]
        self.assertEqual(actual["EEZ_PROOF_TIME_MS"], "750")

    def test_selected_gas_trace_reaches_only_node(self):
        keys = ("EEZ_COMPOSER_GAS_TRACE_TX_HASH", "EEZ_COMPOSER_GAS_TRACE_ADDRESS", "EEZ_COMPOSER_GAS_TRACE_DIR")
        selected = {keys[0]: "0x" + "a" * 64, keys[2]: "/l2/gas-traces"}
        paired = {keys[0]: selected[keys[0]] + ",0x" + "b" * 64, keys[2]: selected[keys[2]]}
        address = {keys[1]: "0x" + "c" * 40, keys[2]: "/l2/gas-traces"}
        for overrides in ({}, dict.fromkeys(keys, ""), selected, paired, address):
            with self.subTest(overrides=overrides):
                result = self.render(overrides)
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                node = services["eez-node"]["environment"]
                signer = services["eez-proof-signer"]["environment"]
                for key in keys:
                    self.assertEqual(node[key], overrides.get(key, ""))
                    self.assertNotIn(key, signer)



if __name__ == "__main__":
    unittest.main()
