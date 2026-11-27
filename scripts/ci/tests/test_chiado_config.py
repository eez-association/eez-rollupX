"""Static Compose validation: no daemon, deployment, or credentials required."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
FEE_RECIPIENT = "0x0000000000000000000000000000000000000f00"


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
            "docker-compose.chiado-postings.yml", "docker-compose.remote-provers.yml",
            "docker-compose.chiado-shared-prover.yml",
            ".env.chiado.example", "scripts/chiado-up.sh", "scripts/builder-relays.sh",
            "scripts/teardown-chiado.sh",
            "configs/chiado/bootnodes.yaml", "configs/chiado/bootstrap_nodes.yaml",
        ):
            target = self.repo / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO / name, target, follow_symlinks=False)
        example = (self.repo / ".env.chiado.example").read_text()
        self.assertIn("\nEEZ_L2_FEE_RECIPIENT=\n", example)
        (self.repo / ".env").write_text(example.replace(
            "\nEEZ_L2_FEE_RECIPIENT=\n", f"\nEEZ_L2_FEE_RECIPIENT={FEE_RECIPIENT}\n"))
        (self.repo / "deployments.env").touch()

    def render(self, overrides=None, compose_file=None):
        environment = {k: v for k, v in os.environ.items()
                       if not k.startswith(("EEZ_", "LH_", "L1_", "L2_", "COMPOSE_"))}
        command = ["docker", "compose"]
        if compose_file:
            command += ["-f", compose_file]
        return subprocess.run(
            command + ["config", "--no-env-resolution", "--format", "json"],
            cwd=self.repo, env={**environment, **(overrides or {})},
            text=True, capture_output=True,
        )

    def test_shared_prover_keeps_both_roles_and_isolates_their_credentials(self):
        origin = "https://signer.example:51061"
        env = {"COMPOSE_FILE": "docker-compose.chiado-node.yml:docker-compose.chiado-shared-prover.yml",
               "EEZ_PROOF_SIGNER_ADDR": "192.0.2.10:51061", "EEZ_PROOF_SIGNER_TLS_DIR": "/fixture/server",
               "EEZ_PROVER_TLS_DIR": "/fixture/client", "EEZ_PROVER_URL": origin}
        roster = f"{origin}=0x{'11'*20}=0x{'22'*20},https://other.example=0x{'33'*20}=0x{'44'*20}"
        for mode in ("singleton", "roster"):
            with self.subTest(mode=mode):
                options = dict(env)
                if mode == "roster":
                    del options["EEZ_PROVER_URL"]
                    options["EEZ_PROVERS"] = roster
                result = self.render(options)
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                self.assertEqual(set(services), {"eez-node", "eez-proof-signer", "lighthouse"})
                node, signer = services["eez-node"], services["eez-proof-signer"]
                self.assertEqual(node["depends_on"]["eez-proof-signer"]["condition"], "service_healthy")
                self.assertEqual(signer["environment"]["EEZ_PROOF_SIGNER_ADDR"], env["EEZ_PROOF_SIGNER_ADDR"])
                self.assertEqual(signer["environment"]["EEZ_PROOF_SIGNER_KEY"], "0xCHANGE_ME")
                self.assertNotIn("EEZ_PROOF_SIGNER_KEY", node["environment"])
                if mode == "singleton":
                    self.assertEqual(node["environment"]["EEZ_PROVER_URL"], origin)
                else:
                    self.assertEqual(node["environment"]["EEZ_PROVERS"], roster)
                    self.assertFalse(node["environment"].get("EEZ_PROVER_URL"))
                for role, setting in ((node, "EEZ_PROVER_TLS_DIR"), (signer, "EEZ_PROOF_SIGNER_TLS_DIR")):
                    mounts = {v["source"]: v for v in role["volumes"]}
                    mount = mounts[env[setting]]
                    self.assertEqual(mount["target"], role["environment"][setting])
                    self.assertTrue(mount["read_only"])
                    self.assertFalse(mount["bind"].get("create_host_path", False))
                    other = "EEZ_PROOF_SIGNER_TLS_DIR" if role is node else "EEZ_PROVER_TLS_DIR"
                    self.assertNotIn(env[other], mounts)
        # Compose config escapes dollars for round-tripping the health check.
        mode, probe = signer["healthcheck"]["test"]
        self.assertEqual(mode, "CMD-SHELL")
        nc = self.root / "nc"
        nc.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
        nc.chmod(0o755)
        for socket, target in (("192.0.2.10:51061", "192.0.2.10"), ("0.0.0.0:51061", "127.0.0.1"),
                               ("[2001:db8::10]:51061", "2001:db8::10"), ("[::]:51061", "::1")):
            with self.subTest(bind=socket):
                result = subprocess.run(["/bin/sh", "-ec", probe.replace("$$", "$")],
                                        env={"PATH": str(self.root), "EEZ_PROOF_SIGNER_ADDR": socket},
                                        check=True, capture_output=True, text=True)
                self.assertEqual(result.stdout.splitlines(), ["-z", target, "51061"])
        for setting in ("EEZ_PROOF_SIGNER_ADDR", "EEZ_PROOF_SIGNER_TLS_DIR", "EEZ_PROVER_TLS_DIR", "EEZ_PROOF_SIGNER_KEY"):
            with self.subTest(missing=setting):
                result = self.render({**env, setting: ""})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(setting, result.stderr)

    def test_project_name_selects_all_three_containers(self):
        env_file = self.repo / ".env"
        preset = env_file.read_text()
        for project in ("", "devnet", "testnet"):
            with self.subTest(project=project):
                env_file.write_text(preset.replace("# COMPOSE_PROJECT_NAME=devnet",
                                                   f"COMPOSE_PROJECT_NAME={project}"))
                result = self.render()
                self.assertEqual(result.returncode, 0, result.stderr)
                config = json.loads(result.stdout)
                self.assertEqual(config["name"], project or self.repo.name)
                label = project or "chiado"
                self.assertEqual(
                    {service: spec["container_name"] for service, spec in config["services"].items()},
                    {"eez-node": f"eez-node-{label}", "eez-proof-signer": f"eez-proof-signer-{label}",
                     "lighthouse": f"eez-{label}-lighthouse"},
                )
        # An explicitly exported project has Compose's normal precedence over dotenv.
        shell = self.render({"COMPOSE_PROJECT_NAME": "shell-project"})
        self.assertEqual(shell.returncode, 0, shell.stderr)
        config = json.loads(shell.stdout)
        self.assertEqual(config["name"], "shell-project")
        self.assertEqual(config["services"]["eez-node"]["container_name"], "eez-node-shell-project")
        invalid = self.render({"COMPOSE_PROJECT_NAME": "Bad.Name"})
        self.assertNotEqual(invalid.returncode, 0)
        self.assertIn("invalid project name", invalid.stderr)

        # Existing dotenv files can use a custom project with the historic names.
        env_file.write_text(preset.replace("EEZ_STACK_NAME=${COMPOSE_PROJECT_NAME:-chiado}", ""))
        legacy = self.render({"COMPOSE_PROJECT_NAME": "old-project"})
        self.assertEqual(legacy.returncode, 0, legacy.stderr)
        config = json.loads(legacy.stdout)
        self.assertEqual(config["name"], "old-project")
        self.assertEqual(config["services"]["eez-node"]["container_name"], "eez-node-chiado")

    def test_alternate_ports_reach_local_clients_and_listeners(self):
        result = self.render({
            "L1_HTTP_PORT": "21645", "L1_AUTH_PORT": "21551", "L2_HTTP_PORT": "21688",
            "EEZ_SIGNER_PORT": "51061", "LH_HTTP_PORT": "9152", "L2_METRICS_PORT": "22091",
            "EEZ_L1_XCHAIN_PORT": "21999", "EEZ_L2_XCHAIN_PORT": "21998",
            "EEZ_L1_BEACON_URL": "http://127.0.0.1:9152/",
        })
        self.assertEqual(result.returncode, 0, result.stderr)
        services = json.loads(result.stdout)["services"]
        node = services["eez-node"]
        expected = {"EEZ_L1_HTTP_PORT": "21645", "EEZ_L1_RPC_URL": "http://127.0.0.1:21645",
                    "EEZ_L2_RPC_URL": "http://127.0.0.1:21688", "EEZ_L1_AUTH_PORT": "21551",
                    "EEZ_L1_XCHAIN_PORT": "21999", "EEZ_L2_XCHAIN_PORT": "21998",
                    "EEZ_PROVER_URL": "http://127.0.0.1:51061",
                    "EEZ_L1_BEACON_URL": "http://127.0.0.1:9152/"}
        for key, value in expected.items():
            self.assertEqual(node["environment"][key], value, key)
        self.assertIn("--http.port=21688", node["command"])
        self.assertIn("--metrics=127.0.0.1:22091", node["command"])
        self.assertIn("--execution-endpoints=http://127.0.0.1:21551", services["lighthouse"]["command"])
        self.assertIn("--http-port=9152", services["lighthouse"]["command"])
        signer = services["eez-proof-signer"]
        self.assertEqual(signer["environment"]["EEZ_PROOF_SIGNER_ADDR"], "127.0.0.1:51061")
        self.assertIn("/dev/tcp/127.0.0.1/51061", signer["healthcheck"]["test"][-1])

    def test_local_startup_requires_key_and_remote_cutover_rejects_project_orphan(self):
        for name in ("docker", "cast", "curl", "jq"):
            stub = self.root / name
            stub.write_text("#!/bin/sh\n" + (
                'case "$*" in *"config --services") echo "$RENDERED";; *"ps --all --orphans --services") echo eez-proof-signer;; *) exit 99;; esac\n'
                if name == "docker" else "exit 99\n"))
            stub.chmod(0o755)
        (self.repo / "deployments.env").write_text("EEZ_ROLLUP_ID=1\n")
        dotenv = self.repo / ".env"
        dotenv.write_text(dotenv.read_text().replace("EEZ_PROOF_SIGNER_KEY=0xCHANGE_ME", "EEZ_PROOF_SIGNER_KEY="))
        for flags, rendered, expected in (([], "", "set the local attestation key"),
                                          (["--remote-provers"], "eez-node", "Retire the previous local signer"),
                                          (["--remote-provers"], "eez-proof-signer", "still contains a local signer")):
            with self.subTest(flags=flags):
                result = subprocess.run(
                    ["bash", self.repo / "scripts/chiado-up.sh", *flags],
                    env={**os.environ, "PATH": f"{self.root}:{os.environ['PATH']}", "RENDERED": rendered},
                    capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(expected, result.stderr)

    def test_teardown_checks_project_and_previous_checkout_identity(self):
        stub = self.root / "docker"
        log = self.root / "docker.log"
        stub.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{log}"\n'
                        'if [ "$1" = ps ]; then echo "$PREVIOUS_CONTAINER"; fi\n')
        stub.chmod(0o755)
        for previous in ("", "previous-project-container"):
            with self.subTest(previous=previous):
                log.write_text("")
                result = subprocess.run(
                    ["bash", self.repo / "scripts/teardown-chiado.sh"],
                    env={**os.environ, "PATH": f"{self.root}:{os.environ['PATH']}",
                         "PREVIOUS_CONTAINER": previous}, capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 1 if previous else 0, result.stderr)
                if previous:
                    self.assertIn(previous, result.stdout)
                self.assertEqual(log.read_text().splitlines(), [
                    "compose -f docker-compose.chiado-node.yml down",
                    "compose -f docker-compose.chiado-node.yml ps --all --quiet eez-node eez-proof-signer lighthouse",
                    f"ps --all --quiet --filter label=com.docker.compose.project.working_dir={self.repo} "
                    f"--filter label=com.docker.compose.project.config_files={self.repo}/docker-compose.chiado-node.yml",
                ])

        log.write_text("")
        remote = subprocess.run(
            ["bash", self.repo / "scripts/teardown-chiado.sh", "--remote-provers"],
            env={**os.environ, "PATH": f"{self.root}:{os.environ['PATH']}", "PREVIOUS_CONTAINER": ""},
            capture_output=True, text=True)
        self.assertEqual(remote.returncode, 0, remote.stderr)
        prefix = "compose -f docker-compose.chiado-node.yml -f docker-compose.remote-provers.yml"
        self.assertEqual(log.read_text().splitlines(), [
            f"{prefix} down", f"{prefix} ps --all --quiet --orphans",
            f"ps --all --quiet --filter label=com.docker.compose.project.working_dir={self.repo} "
            f"--filter label=com.docker.compose.project.config_files={self.repo}/docker-compose.chiado-node.yml,"
            f"{self.repo}/docker-compose.remote-provers.yml",
        ])

        log.write_text("")
        shared = subprocess.run(
            ["bash", self.repo / "scripts/teardown-chiado.sh", "--share-prover"],
            env={**os.environ, "PATH": f"{self.root}:{os.environ['PATH']}", "PREVIOUS_CONTAINER": ""},
            capture_output=True, text=True)
        self.assertEqual(shared.returncode, 0, shared.stderr)
        overlay = "docker-compose.chiado-shared-prover.yml"
        prefix = f"compose -f docker-compose.chiado-node.yml -f {overlay}"
        self.assertEqual(log.read_text().splitlines(), [
            f"{prefix} down", f"{prefix} ps --all --quiet eez-node eez-proof-signer lighthouse",
            f"ps --all --quiet --filter label=com.docker.compose.project.working_dir={self.repo} "
            f"--filter label=com.docker.compose.project.config_files={self.repo}/docker-compose.chiado-node.yml,"
            f"{self.repo}/{overlay}",
        ])

    def test_offline_signer_has_local_profiles_and_no_chain_rpc(self):
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stderr)
        services = json.loads(result.stdout)["services"]
        signer = services["eez-proof-signer"]["environment"]
        self.assertFalse(any("RPC_URL" in key for key in signer))
        for key in ("EEZ_L2_TREASURY_OWNER", "EEZ_L2_TREASURY_RECIPIENT"):
            self.assertNotIn(key, signer)
            self.assertNotIn(key, services["eez-node"]["environment"])
        self.assertEqual(signer["EEZ_CHAIN_CONFIG"], "/app/genesis.json")
        self.assertEqual(signer["EEZ_L1_CHAIN_CONFIG"], "chiado")
        self.assertEqual(services["eez-node"]["environment"]["EEZ_L1_RPC_URL"],
                         "http://127.0.0.1:18645")

    def test_fee_recipient_is_required_and_reaches_only_the_node(self):
        env_file = self.repo / ".env"
        configured = env_file.read_text()
        for dotenv, shell, expected in (
            (configured, {}, FEE_RECIPIENT),
            (configured, {"EEZ_L2_FEE_RECIPIENT": "0x" + "0" * 40}, "0x" + "0" * 40),
            (configured.replace(f"EEZ_L2_FEE_RECIPIENT={FEE_RECIPIENT}\n", ""), {}, None),
            (configured.replace(f"EEZ_L2_FEE_RECIPIENT={FEE_RECIPIENT}\n",
                                "EEZ_L2_FEE_RECIPIENT=\n"), {}, None),
            (configured, {"EEZ_L2_FEE_RECIPIENT": ""}, None),
        ):
            with self.subTest(shell=shell, expected=expected):
                env_file.write_text(dotenv)
                result = self.render(shell)
                if expected is None:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("EEZ_L2_FEE_RECIPIENT", result.stderr)
                    continue
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                self.assertEqual(
                    services["eez-node"]["environment"]["EEZ_L2_FEE_RECIPIENT"], expected)
                self.assertNotIn("EEZ_L2_FEE_RECIPIENT",
                                 services["eez-proof-signer"]["environment"])

    def test_default_profile_loads_from_dotenv(self):
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stderr)
        services = json.loads(result.stdout)["services"]
        self.assertEqual(set(services), {"eez-node", "eez-proof-signer", "lighthouse"})
        node = services["eez-node"]["environment"]
        for key, value in {
            "EEZ_L1_CHAIN_ID": "10200", "EEZ_L1_BLOCK_TIME_MS": "5000",
            "EEZ_PROOF_TIME_MS": "750", "EEZ_SETTLEMENT_START_AFTER_BLOCK": "2",
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
        self.assertEqual(services["lighthouse"]["image"], "sigp/lighthouse@sha256:53f9063d98c3d3a2d290207873f019ea2298cfc02f8cd58d87c33b04aab856d1")
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

    def test_postbatch_gas_margin_is_owned_by_runtime_and_preserves_overrides(self):
        key = "EEZ_L1_POSTBATCH_GAS_MARGIN_BPS"
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNone(json.loads(result.stdout)["services"]["eez-node"]["environment"].get(key))
        with (self.repo / ".env").open("a") as stream:
            stream.write(f"\n{key}=1500\n")
        for override in (None, "", "500", "2000", "bad"):
            with self.subTest(override=override):
                result = self.render({} if override is None else {key: override})
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                self.assertEqual(services["eez-node"]["environment"][key],
                                 "1500" if override is None else override)
                self.assertNotIn(key, services["eez-proof-signer"]["environment"])

    def test_fee_settings_preserve_absence_and_shell_precedence(self):
        keys = [
            'EEZ_FEES_L1_TO_L2_ENABLED',
            'EEZ_FEES_L2_TO_L1_ENABLED',
            'EEZ_FEES_L1_PER_L2_NUMERATOR',
            'EEZ_FEES_L1_PER_L2_DENOMINATOR',
            'EEZ_FEES_PROVING_COST_L1',
            'EEZ_FEES_OPERATIONS_COST_L1',
            'EEZ_FEES_TREASURY_COST_L1',
            'EEZ_FEES_RISK_BPS',
            'EEZ_FEES_PROFIT_BPS',
            'EEZ_FEES_EXCLUDED_SENDERS',
            'EEZ_FEES_QUOTE_BASE_FEE_PERCENT',
            'EEZ_FEES_L1_REFUND_RECIPIENT',
            'EEZ_FEES_L1_REFUND_PERCENT',
        ]
        # Obsolete names still reach the node so it rejects them explicitly.
        keys += ["EEZ_L2_FEE_SHADOW_POLICY", "EEZ_L1_FEE_REFUND_POLICY",
                 "EEZ_L2_FEE_ENFORCE_TARIFF", "EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP"]
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stderr)
        node = json.loads(result.stdout)["services"]["eez-node"]["environment"]
        for key in keys:
            self.assertIsNone(node.get(key), key)
        with (self.repo / ".env").open("a") as stream:
            for key in keys:
                stream.write(f"\n{key}=fixture\n")
        # Scalar transport, not acceptance: runtime tests validate the values.
        for override in (None, "", "bad", "false", "true", "90"):
            with self.subTest(override=override):
                result = self.render({} if override is None else dict.fromkeys(keys, override))
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                for key in keys:
                    self.assertEqual(services["eez-node"]["environment"][key], "fixture" if override is None else override)
                    self.assertNotIn(key, services["eez-proof-signer"]["environment"])

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
        env_file = self.repo / ".env"
        env_file.write_text(env_file.read_text().replace("# COMPOSE_PROJECT_NAME=devnet",
                                                         "COMPOSE_PROJECT_NAME=devnet"))
        stubs = {
            "docker": f'''printf '%s\\n' "$*" >> '{log}'
case "$*" in
    'compose -f docker-compose.chiado-node.yml config --quiet')
        '{shutil.which("docker")}' compose -f docker-compose.chiado-node.yml config --format json |
            jq -e '.name == "shell-project" and .services["eez-node"].container_name == "eez-node-shell-project"' ;;
    *'config --services'|*'config --quiet') '{shutil.which("docker")}' "$@" ;;
    *'ps --all --orphans --services') exit 0 ;;
    *'up -d') exit 71 ;;
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
            env={**os.environ, "COMPOSE_PROJECT_NAME": "shell-project", "PATH": str(tool_dir) + os.pathsep + os.environ["PATH"]},
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 71, result.stdout + result.stderr)
        self.assertEqual(log.read_text().splitlines(), [
            "compose -f docker-compose.chiado-node.yml config --quiet",
            "compose -f docker-compose.chiado-node.yml up -d",
        ])

        log.write_text("")
        server_dir = self.root / "server-tls"
        client_dir = self.root / "client-tls"
        server_dir.mkdir()
        client_dir.mkdir()
        alias = self.root / "client-alias"
        alias.symlink_to(client_dir)
        nested = server_dir / "nested"
        nested.mkdir()
        shared_env = {**os.environ, "PATH": f"{tool_dir}:{os.environ['PATH']}",
                      "EEZ_PROOF_SIGNER_ADDR": "192.0.2.10:51061", "EEZ_PROOF_SIGNER_TLS_DIR": str(server_dir),
                      "EEZ_PROVER_URL": "https://p.example:51061", "EEZ_PROVER_TLS_DIR": str(client_dir)}
        for flags, overrides, error in (
            ([], {}, "Select --share-prover"),
            (["--share-prover"], {"EEZ_PROVER_URL": ""}, "HTTPS origin"),
            (["--share-prover"], {"EEZ_PROVERS": ""}, "set the HTTPS roster"),
            (["--share-prover"], {"EEZ_PROOF_SIGNER_TLS_DIR": str(alias)}, "disjoint"),
            (["--share-prover"], {"EEZ_PROVER_TLS_DIR": str(nested)}, "disjoint"),
            (["--share-prover"], {"EEZ_PROVER_TLS_DIR": str(self.root / "absent")}, "existing absolute"),
        ):
            with self.subTest(flags=flags, overrides=overrides):
                bad = subprocess.run(["bash", self.repo / "scripts/chiado-up.sh", *flags],
                                     env={**shared_env, **overrides}, capture_output=True, text=True)
                self.assertNotEqual(bad.returncode, 0)
                self.assertIn(error, bad.stderr)
                self.assertEqual(log.read_text(), "")
        shared = subprocess.run(
            ["bash", self.repo / "scripts/chiado-up.sh", "--share-prover"],
            env=shared_env,
            capture_output=True, text=True)
        self.assertEqual(shared.returncode, 71, shared.stdout + shared.stderr)
        prefix = "compose -f docker-compose.chiado-node.yml -f docker-compose.chiado-shared-prover.yml"
        self.assertEqual(log.read_text().splitlines(), [f"{prefix} config --quiet", f"{prefix} up -d"])

        env_file.write_text(env_file.read_text().replace("EEZ_PROOF_SIGNER_KEY=0xCHANGE_ME", ""))
        log.write_text("")
        remote = subprocess.run(
            ["bash", self.repo / "scripts/chiado-up.sh", "--remote-provers"],
            env={**os.environ, "PATH": f"{tool_dir}:{os.environ['PATH']}",
                 "EEZ_PROVERS": "https://p.example:50061=0x" + "11"*20 + "=0x" + "22"*20,
                 "EEZ_PROVER_TLS_DIR": "/fixture/tls"},
            capture_output=True, text=True)
        self.assertEqual(remote.returncode, 71, remote.stdout + remote.stderr)
        prefix = "compose -f docker-compose.chiado-node.yml -f docker-compose.remote-provers.yml"
        self.assertEqual(log.read_text().splitlines(), [
            f"{prefix} config --services", f"{prefix} ps --all --orphans --services", f"{prefix} up -d",
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

    def test_obsolete_preparation_reaches_runtime_for_rejection(self):
        for value in (None, "500", "", "invalid"):
            with self.subTest(value=value):
                overrides = {} if value is None else {"EEZ_PREPARATION_TIME_MS": value}
                result = self.render(overrides)
                self.assertEqual(result.returncode, 0, result.stderr)
                actual = json.loads(result.stdout)["services"]["eez-node"]["environment"]
                self.assertEqual(actual.get("EEZ_PREPARATION_TIME_MS"), value)
                # Any explicit value must reach Rust's migration rejection,
                # including empty and with the required position also supplied.
                self.assertEqual(actual["EEZ_SETTLEMENT_START_AFTER_BLOCK"], "2")
                self.assertEqual(actual["EEZ_PROOF_TIME_MS"], "750")
                self.assertEqual(actual["EEZ_SUBMISSION_SLACK_MS"], "700")

    def test_scheduler_preserves_absence_empty_invalid_and_shell_precedence(self):
        env_file = self.repo / ".env"
        fixture = env_file.read_text()
        env_file.write_text(fixture.replace("EEZ_SETTLEMENT_START_AFTER_BLOCK=2\n", ""))
        for key in ("EEZ_SETTLEMENT_START_AFTER_BLOCK", "EEZ_SETTLEMENT_START_DELAY_MS"):
            for value in (None, "3", "250", "", "0", "-1", "invalid"):
                with self.subTest(key=key, value=value):
                    result = self.render({} if value is None else {key: value})
                    self.assertEqual(result.returncode, 0, result.stderr)
                    services = json.loads(result.stdout)["services"]
                    self.assertEqual(services["eez-node"]["environment"].get(key), value)
                    self.assertNotIn(key, services["eez-proof-signer"]["environment"])
        env_file.write_text(fixture + "\nEEZ_SETTLEMENT_START_DELAY_MS=100\n")
        result = self.render({"EEZ_SETTLEMENT_START_AFTER_BLOCK": "3",
                              "EEZ_SETTLEMENT_START_DELAY_MS": "250"})
        self.assertEqual(result.returncode, 0, result.stderr)
        node = json.loads(result.stdout)["services"]["eez-node"]["environment"]
        self.assertEqual(node["EEZ_SETTLEMENT_START_AFTER_BLOCK"], "3")
        self.assertEqual(node["EEZ_SETTLEMENT_START_DELAY_MS"], "250")
        self.assertIsNone(node.get("EEZ_PREPARATION_TIME_MS"))

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

    def test_builder_relay_list_reaches_only_the_node_unchanged(self):
        # The Composer parses and validates the list; Compose must forward the
        # exact value, including several comma-separated relays.
        key = "EEZ_L1_BUILDER_RPC_URL"
        relays = "https://a.example/rpc, https://u:p@b.example:8545/?k=v"
        for overrides, expected in (({}, "https://builder.chiado.gcd.ovh/"),
                                    ({key: relays}, relays), ({key: ""}, None)):
            with self.subTest(overrides=overrides):
                result = self.render(overrides, "docker-compose.chiado-node.yml")
                if expected is None:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(key, result.stderr)
                    continue
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                self.assertEqual(services["eez-node"]["environment"][key], expected)
                self.assertNotIn(key, services["eez-proof-signer"]["environment"])



if __name__ == "__main__":
    unittest.main()
