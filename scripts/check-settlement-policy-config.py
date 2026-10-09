#!/usr/bin/env python3
"""Check actual Compose propagation using examples and disposable public bindings.

Never reads the checkout's .env files, private keys, or deployment bindings.
Requires the existing Docker Compose CLI; does not start containers.
"""

import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    repo = Path(__file__).resolve().parent.parent
    policy = {
        "EEZ_SETTLEMENT_PURE_L2_MODE": "interval",
        "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS": "900000",
        "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS": "150",
        "EEZ_SETTLEMENT_START_AFTER_BLOCK": "2",
        "EEZ_SETTLEMENT_START_DELAY_MS": "250",
        "EEZ_SETTLEMENT_BLOB_FULLNESS_BPS": "1000",
        "EEZ_MAX_BLOBS_PER_BATCH": "2",
        "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS": "100000000000",
        "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS": "200000000000",
        "EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE": "2",
        "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE": "100",
        "EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE": "50",
        "EEZ_L1_POSTBATCH_CROSS_CHAIN_PRIORITY_PERCENTILE": "90",
        "EEZ_L1_POSTBATCH_CROSS_CHAIN_MAX_PRIORITY_FEE": "150",
        "EEZ_L1_POSTBATCH_EXECUTION_HEADROOM_BPS": "15000",
        "EEZ_L1_POSTBATCH_BLOB_HEADROOM_BPS": "5000",
        "EEZ_L1_POSTBATCH_GAS_MARGIN_BPS": "2500",
        "EEZ_MAX_USER_TXS_PER_BUNDLE": "auto",
    }
    signer_l1_profile = {
        "chiado": "chiado",
        "kurtosis": "/l1-genesis/genesis.json",
        "ethereum": "mainnet",
    }
    for profile in signer_l1_profile:
        with tempfile.TemporaryDirectory(prefix="eez-policy-compose-") as directory:
            root = Path(directory)
            (root / "deployments.env").write_text("")
            example = repo / f".env.{profile}.example"
            fixture = example.read_text()
            assert not any(line.startswith("EEZ_PREPARATION_TIME_MS=")
                           for line in fixture.splitlines()), profile
            fixture += "\n" + "\n".join(f"{key}={value}" for key, value in policy.items())
            # Required operator choice; the examples leave it empty on purpose.
            fixture += "\nEEZ_L2_FEE_RECIPIENT=0x0000000000000000000000000000000000000f00\n"
            env_file = root / f".env.{profile}"
            env_file.write_text(fixture)
            env = os.environ.copy()
            for key in (*policy, "EEZ_PREPARATION_TIME_MS", "EEZ_L1_POSTBATCH_PRIORITY_FEE",
                        "EEZ_MAX_BLOCKS_PER_BATCH", "EEZ_MAX_POSTBATCH_GAS",
                        "EEZ_MAX_SPECULATIVE_DEPTH"):
                env.pop(key, None)
            env.update(policy)
            compose_command = [
                "docker", "compose", "--project-directory", directory,
                "--env-file", str(env_file), "-f",
                str(repo / f"docker-compose.{profile}-node.yml"),
                "config", "--format", "json",
            ]
            try:
                result = subprocess.run(
                    compose_command,
                    env=env, capture_output=True, text=True, check=True,
                )
            except subprocess.CalledProcessError as error:
                raise RuntimeError(
                    f"{profile} Compose config failed: {error.stderr.strip()}"
                ) from error
            services = json.loads(result.stdout)["services"]
            node = services["eez-node"]["environment"]
            mismatches = {
                key: (node.get(key), value)
                for key, value in policy.items()
                if node.get(key) != value
            }
            assert not mismatches, (profile, mismatches)
            assert "EEZ_SETTLEMENT_START_AFTER_BLOCK" not in services["eez-proof-signer"]["environment"]
            assert "EEZ_SETTLEMENT_START_DELAY_MS" not in services["eez-proof-signer"]["environment"]
            assert node.get("EEZ_PREPARATION_TIME_MS") is None, profile
            assert node.get("EEZ_MAX_BLOCKS_PER_BATCH") is None, profile
            assert node.get("EEZ_MAX_POSTBATCH_GAS") is None, profile
            assert node["EEZ_MAX_SPECULATIVE_DEPTH"] == "0", profile
            expected = signer_l1_profile[profile]
            assert services["eez-proof-signer"]["environment"]["EEZ_L1_CHAIN_CONFIG"] == expected
            if profile == "ethereum":
                # One network value selects all three services consistently.
                assert node["EEZ_L1_CHAIN"] == "mainnet", node["EEZ_L1_CHAIN"]
                assert node["EEZ_L1_CHAIN_ID"] == node["EEZ_CROSS_CHAIN_SOURCE_CHAIN_IDS"] == "1"
                assert node.get("EEZ_L1_CHAIN_PATH") is None
                assert "--network=mainnet" in services["lighthouse"]["command"]
            # These are transport checks, not runtime acceptance: missing P,
            # malformed values and every obsolete preparation value must reach
            # the node unchanged so startup rejects them, never selects legacy.
            scheduler_keys = ("EEZ_SETTLEMENT_START_AFTER_BLOCK",
                              "EEZ_SETTLEMENT_START_DELAY_MS", "EEZ_PREPARATION_TIME_MS")
            fixture = "\n".join(line for line in fixture.splitlines()
                                if not line.startswith(tuple(key + "=" for key in scheduler_keys)))
            env_file.write_text(fixture)
            for key in scheduler_keys:
                env.pop(key, None)
            for key in scheduler_keys:
                for value in (None, "", "0", "250", "invalid"):
                    selected_env = dict(env)
                    if value is not None:
                        selected_env[key] = value
                    if key != "EEZ_SETTLEMENT_START_AFTER_BLOCK":
                        selected_env["EEZ_SETTLEMENT_START_AFTER_BLOCK"] = "2"
                    result = subprocess.run(compose_command, env=selected_env,
                                            capture_output=True, text=True, check=True)
                    services = json.loads(result.stdout)["services"]
                    assert services["eez-node"]["environment"].get(key) == value, (profile, key, value)
                    assert key not in services["eez-proof-signer"]["environment"], (profile, key)
            print(f"{profile}: mandatory policy configuration PASS")


if __name__ == "__main__":
    main()
