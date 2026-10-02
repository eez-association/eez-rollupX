"""Static Ethereum Compose validation: no daemon, deployment, or credentials required."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
FEE_RECIPIENT = "0x0000000000000000000000000000000000000f00"
COMMIT = "fa916b13254a36423cad130080dde367bb18a7d6"


@unittest.skipUnless(shutil.which("docker"),
                     "Docker Compose CLI is required (no daemon needed)")
class EthereumConfigTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-ethereum-config-test.")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name) / "repo"
        self.repo.mkdir()
        for name in ("docker-compose.ethereum-node.yml", ".env.ethereum.example"):
            shutil.copy2(REPO / name, self.repo / name)
        example = (self.repo / ".env.ethereum.example").read_text()
        self.assertIn("\nEEZ_L2_FEE_RECIPIENT=\n", example)
        self.assertIn(":<full-commit>\n", example)
        # The example pins nothing on purpose; a real .env pins one commit.
        configured = example.replace(
            "\nEEZ_L2_FEE_RECIPIENT=\n", f"\nEEZ_L2_FEE_RECIPIENT={FEE_RECIPIENT}\n"
        ).replace(":<full-commit>\n", f":{COMMIT}\n")
        (self.repo / ".env").write_text(configured)
        (self.repo / "deployments.env").touch()

    def render(self, overrides=None, dotenv=None):
        if dotenv is not None:
            (self.repo / ".env").write_text(dotenv)
        environment = {k: v for k, v in os.environ.items()
                       if not k.startswith(("EEZ_", "LH_", "COMPOSE_"))}
        return subprocess.run(
            ["docker", "compose", "config", "--no-env-resolution", "--format", "json"],
            cwd=self.repo, env={**environment, **(overrides or {})},
            text=True, capture_output=True,
        )

    def services(self, overrides=None):
        result = self.render(overrides)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)["services"]

    def test_example_selects_mainnet_for_every_service(self):
        services = self.services()
        node = services["eez-node"]["environment"]
        self.assertEqual(node["EEZ_L1_CHAIN"], "mainnet")
        self.assertEqual(node["EEZ_L1_CHAIN_ID"], "1")
        self.assertEqual(node["EEZ_CROSS_CHAIN_SOURCE_CHAIN_IDS"], "1")
        self.assertNotIn("EEZ_L1_CHAIN_PATH", node)
        self.assertEqual((node["EEZ_L1_BLOCK_TIME_MS"], node["EEZ_L2_BLOCK_TIME_MS"]),
                         ("12000", "2000"))
        self.assertEqual(node["EEZ_L1_BLOB_SIDECAR_ENCODING"], "eip7594")
        signer = services["eez-proof-signer"]["environment"]
        self.assertEqual(signer["EEZ_L1_CHAIN_CONFIG"], "mainnet")
        lighthouse = services["lighthouse"]["command"]
        self.assertIn("--network=mainnet", lighthouse)
        self.assertIn("--semi-supernode", lighthouse)
        self.assertIn("--checkpoint-sync-url=https://mainnet.checkpoint.sigp.io", lighthouse)
        self.assertFalse([arg for arg in lighthouse if arg.startswith("--testnet-dir")])
        self.assertEqual(
            {s["container_name"] for s in services.values()},
            {"eez-node-ethereum", "eez-proof-signer-ethereum", "eez-ethereum-lighthouse"},
        )

    def test_one_network_value_switches_all_services_for_a_rehearsal(self):
        services = self.services({"EEZ_L1_NETWORK": "hoodi", "EEZ_L1_CHAIN_ID": "560048",
                                  "LH_CHECKPOINT_URL": "https://checkpoint-sync.hoodi.ethpandaops.io"})
        node = services["eez-node"]["environment"]
        self.assertEqual(node["EEZ_L1_CHAIN"], "hoodi")
        self.assertEqual(node["EEZ_CROSS_CHAIN_SOURCE_CHAIN_IDS"], "560048")
        self.assertEqual(services["eez-proof-signer"]["environment"]["EEZ_L1_CHAIN_CONFIG"], "hoodi")
        self.assertIn("--network=hoodi", services["lighthouse"]["command"])

    def test_bundle_signing_key_and_builders_reach_only_the_node(self):
        services = self.services({"EEZ_L1_BUNDLE_SIGNING_KEY": "0x" + "11" * 32})
        node = services["eez-node"]["environment"]
        self.assertEqual(node["EEZ_L1_BUNDLE_SIGNING_KEY"], "0x" + "11" * 32)
        self.assertEqual(
            node["EEZ_L1_BUILDER_RPC_URL"],
            "https://rpc.titanbuilder.xyz,https://rpc.buildernet.org,https://rpc.quasar.win",
        )
        for name in ("EEZ_L1_BUNDLE_SIGNING_KEY", "EEZ_L1_POSTER_KEY"):
            self.assertNotIn(name, services["eez-proof-signer"]["environment"])
        self.assertNotIn("EEZ_PROOF_SIGNER_KEY", node)

    def test_example_fee_limits_and_cadence_reach_the_node(self):
        services = self.services()
        node = services["eez-node"]["environment"]
        for name, value in (
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "10000000000"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "2000000000"),
            ("EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE", "50"),
            ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "50000000"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "2000000000"),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "7200"),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000"),
        ):
            self.assertEqual(node.get(name), value, name)
        self.assertIsNone(node.get("EEZ_L1_POSTBATCH_PRIORITY_FEE"))
        self.assertIsNone(node.get("EEZ_MAX_BLOCKS_PER_BATCH"))
        signer = services["eez-proof-signer"]["environment"]
        # One proof window covers a whole idle cycle.
        self.assertGreaterEqual(int(signer["EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS"]),
                                int(node["EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS"]))
        budget = sum(int(node[name]) for name in (
            "EEZ_PREPARATION_TIME_MS", "EEZ_PROOF_TIME_MS", "EEZ_SUBMISSION_SLACK_MS"))
        self.assertLessEqual(budget, (6 - 1) * 2000)

    def test_priority_percentile_is_absent_unless_configured(self):
        configured = (self.repo / ".env").read_text()
        line = "EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE=50\n"
        self.assertIn(line, configured)
        result = self.render(dotenv=configured.replace(line, ""))
        self.assertEqual(result.returncode, 0, result.stderr)
        node = json.loads(result.stdout)["services"]["eez-node"]["environment"]
        # Absent keeps the runtime default (90) rather than a Compose value.
        self.assertIsNone(node.get("EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE"))

    def test_required_choices_fail_when_absent_or_empty(self):
        configured = (self.repo / ".env").read_text()
        for line, name in (
            ("EEZ_L1_NETWORK=mainnet\n", "EEZ_L1_NETWORK"),
            ("EEZ_L1_CHAIN_ID=1\n", "EEZ_L1_CHAIN_ID"),
            (f"EEZ_NODE_IMAGE=ghcr.io/eduadiez/rollupx-node:{COMMIT}\n", "EEZ_NODE_IMAGE"),
            (f"EEZ_PROOF_SIGNER_IMAGE=ghcr.io/eduadiez/rollupx-proof-signer:{COMMIT}\n",
             "EEZ_PROOF_SIGNER_IMAGE"),
            ("LH_CHECKPOINT_URL=https://mainnet.checkpoint.sigp.io\n", "LH_CHECKPOINT_URL"),
            ("L1_SNAPSHOT_DIR=./data/ethereum-l1\n", "L1_SNAPSHOT_DIR"),
        ):
            self.assertIn(line, configured)
            for replacement in ("", line.split("=")[0] + "=\n"):
                with self.subTest(name=name, replacement=replacement):
                    result = self.render(dotenv=configured.replace(line, replacement))
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(name, result.stderr)
        self.render(dotenv=configured)


if __name__ == "__main__":
    unittest.main()
