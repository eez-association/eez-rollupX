"""Resolve the real single-host overlay without creating Docker resources."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ObserverCompose(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.env = dict(os.environ,
                        OBS_RUNTIME=str(self.root / "runtime"),
                        OBS_LOG_DIR=str(self.root / "logs"),
                        OBS_INDEXER_CONFIG=str(self.root / "indexer.json"),
                        OBS_LIFECYCLE_DIR=str(self.root / "lifecycle"),
                        OBS_OBSERVER_UID="1234", OBS_OBSERVER_GID="2345",
                        EEZ_NODE_IMAGE="fixture-node@sha256:" + "ab" * 32)
        self.env.pop("COMPOSE_PROFILES", None)

    def resolve(self, *, overlay=True, migration=False, node_network=False, gateway=False, env=None):
        command = ["docker", "compose", "--project-name", "eez-observer-config-test",
                   "-f", str(ROOT / "compose.yaml")]
        if overlay:
            command += ["-f", str(ROOT / "compose.observer.yaml")]
        if node_network:
            command += ["-f", str(ROOT / "compose.node-network.yaml")]
        if gateway:
            command += ["-f", str(ROOT / "compose.gateway.yaml")]
        command += ["--profile", "database"]
        if migration:
            command += ["--profile", "migration"]
        return subprocess.run(command + ["config", "--format", "json"],
                              env=self.env if env is None else env, text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)

    def test_existing_external_observer_setup_is_unchanged(self):
        result = self.resolve(overlay=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("observer", json.loads(result.stdout)["services"])

    def test_observer_has_only_writer_and_readonly_inputs(self):
        result = self.resolve()
        self.assertEqual(result.returncode, 0, result.stderr)
        services = json.loads(result.stdout)["services"]
        self.assertNotIn("observer-migrate", services)
        self.assertEqual(set(services["postgres"]["networks"]), {"ledger"})
        self.assertEqual(services["postgres"]["ports"][0]["host_ip"], "127.0.0.1")
        observer = services["observer"]
        self.assertEqual(observer["image"], self.env["EEZ_NODE_IMAGE"])
        self.assertEqual(observer["pull_policy"], "never")
        self.assertEqual(observer["user"], "1234:2345")
        self.assertEqual(set(observer["networks"]), {"default", "ledger"})
        self.assertNotIn("network_mode", observer)
        self.assertNotIn("ports", observer)
        self.assertTrue(observer["read_only"])
        self.assertEqual(observer["cap_drop"], ["ALL"])
        self.assertEqual(observer["command"], ["indexer", "--config", "/config/indexer.json"])
        self.assertEqual(observer["depends_on"]["postgres"]["condition"], "service_healthy")
        mounts = {mount["target"]: mount for mount in observer["volumes"]}
        self.assertEqual(set(mounts), {"/config/indexer.json", "/config/writer.json", "/lifecycle"})
        self.assertTrue(all(mount["read_only"] for mount in mounts.values()))
        self.assertTrue(all(not mount["bind"]["create_host_path"] for mount in mounts.values()))
        self.assertEqual(mounts["/config/writer.json"]["source"],
                         str(self.root / "runtime/database/writer-container.json"))

    def test_migration_is_explicit_and_admin_credentials_are_separate(self):
        result = self.resolve(migration=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        service = json.loads(result.stdout)["services"]["observer-migrate"]
        self.assertEqual(service["profiles"], ["migration"])
        self.assertEqual(service["restart"], "no")
        self.assertEqual(set(service["networks"]), {"ledger"})
        self.assertEqual(service["command"], ["migrate", "--database-config", "/config/admin.json"])
        self.assertEqual(len(service["volumes"]), 1)
        self.assertEqual(service["volumes"][0]["target"], "/config/admin.json")
        self.assertTrue(service["volumes"][0]["read_only"])

    def test_missing_runtime_identity_or_paths_fail_before_start(self):
        for key in ("EEZ_NODE_IMAGE", "OBS_INDEXER_CONFIG", "OBS_LIFECYCLE_DIR",
                    "OBS_OBSERVER_UID", "OBS_OBSERVER_GID"):
            with self.subTest(key=key):
                env = dict(self.env)
                env.pop(key)
                result = self.resolve(env=env)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(key, result.stderr)

    def test_optional_node_network_attaches_only_the_observer(self):
        env = dict(self.env, EEZ_NODE_NETWORK="existing-eez-network")
        result = self.resolve(node_network=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        config = json.loads(result.stdout)
        self.assertEqual(config["networks"]["eez-node"]["name"], "existing-eez-network")
        self.assertTrue(config["networks"]["eez-node"]["external"])
        joined = [name for name, service in config["services"].items()
                  if "eez-node" in service.get("networks", {})]
        self.assertEqual(joined, ["observer"])
        env.pop("EEZ_NODE_NETWORK")
        result = self.resolve(node_network=True, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("EEZ_NODE_NETWORK", result.stderr)

    def test_gateway_network_attaches_only_grafana_with_stable_alias(self):
        env = dict(self.env, GRAFANA_GATEWAY_NETWORK="existing-https-gateway")
        result = self.resolve(gateway=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        config = json.loads(result.stdout)
        self.assertEqual(config["networks"]["public-gateway"]["name"], "existing-https-gateway")
        self.assertTrue(config["networks"]["public-gateway"]["external"])
        joined = [name for name, service in config["services"].items()
                  if "public-gateway" in service.get("networks", {})]
        self.assertEqual(joined, ["grafana"])
        grafana = config["services"]["grafana"]
        self.assertIn("eez-observability-grafana", grafana["networks"]["public-gateway"]["aliases"])
        self.assertEqual(grafana["ports"][0]["host_ip"], "127.0.0.1")
        self.assertEqual(grafana["environment"]["GF_AUTH_ANONYMOUS_ENABLED"], "false")
        env.pop("GRAFANA_GATEWAY_NETWORK")
        result = self.resolve(gateway=True, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("GRAFANA_GATEWAY_NETWORK", result.stderr)


if __name__ == "__main__":
    unittest.main()
