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
        "EEZ_SETTLEMENT_POLICY_ENABLED": "true",
        "EEZ_SETTLEMENT_PURE_L2_MODE": "interval",
        "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS": "900000",
        "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS": "150",
        "EEZ_SETTLEMENT_BLOB_FULLNESS_BPS": "1000",
        "EEZ_MAX_BLOBS_PER_BATCH": "2",
        "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS": "100000000000",
        "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS": "200000000000",
        "EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE": "2",
        "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE": "100",
        "EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS": "2000",
        "EEZ_L1_POSTBATCH_EXECUTION_HEADROOM_BPS": "15000",
        "EEZ_L1_POSTBATCH_BLOB_HEADROOM_BPS": "5000",
        "EEZ_L1_POSTBATCH_GAS_MARGIN_BPS": "2500",
        "EEZ_MAX_USER_TXS_PER_BUNDLE": "auto",
    }
    for profile in ("chiado", "kurtosis"):
        for enabled in (False, True):
            with tempfile.TemporaryDirectory(prefix="eez-policy-compose-") as directory:
                root = Path(directory)
                (root / "deployments.env").write_text("")
                example = repo / f".env.{profile}.example"
                env = os.environ.copy()
                for key in (*policy, "EEZ_L1_POSTBATCH_PRIORITY_FEE",
                            "EEZ_MAX_BLOCKS_PER_BATCH", "EEZ_MAX_POSTBATCH_GAS",
                            "EEZ_MAX_SPECULATIVE_DEPTH"):
                    env.pop(key, None)
                fixture = example.read_text()
                if enabled:
                    env.update(policy)
                    fixture += "\n" + "\n".join(f"{key}={value}" for key, value in policy.items())
                else:
                    # Legacy mode must omit policy-only inputs and use a numeric
                    # admission limit.
                    fixture = "\n".join(line for line in fixture.splitlines()
                                        if line.split("=", 1)[0] not in policy) + "\n"
                    env["EEZ_SETTLEMENT_POLICY_ENABLED"] = "false"
                    env["EEZ_MAX_USER_TXS_PER_BUNDLE"] = "12" if profile == "chiado" else "50"
                    fixture += ("EEZ_SETTLEMENT_POLICY_ENABLED=false\n"
                                f"EEZ_MAX_USER_TXS_PER_BUNDLE={env['EEZ_MAX_USER_TXS_PER_BUNDLE']}\n")
                env_file = root / f".env.{profile}"
                env_file.write_text(fixture)
                result = subprocess.run(
                    ["docker", "compose", "--project-directory", directory,
                     "--env-file", str(env_file), "-f",
                     str(repo / f"docker-compose.{profile}-node.yml"),
                     "config", "--format", "json"],
                    env=env, capture_output=True, text=True, check=True,
                )
                services = json.loads(result.stdout)["services"]
                node = services["eez-node"]["environment"]
                assert node["EEZ_SETTLEMENT_POLICY_ENABLED"] == str(enabled).lower(), profile
                if enabled:
                    assert all(node.get(key) == value for key, value in policy.items()), profile
                else:
                    assert node.get("EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS") is None, profile
                assert node.get("EEZ_MAX_BLOCKS_PER_BATCH") is None, profile
                assert node.get("EEZ_MAX_POSTBATCH_GAS") is None, profile
                assert node["EEZ_MAX_SPECULATIVE_DEPTH"] == "0", profile
                expected = "chiado" if profile == "chiado" else "/l1-genesis/genesis.json"
                assert services["eez-proof-signer"]["environment"]["EEZ_L1_CHAIN_CONFIG"] == expected
                print(f"{profile}: {'enabled' if enabled else 'legacy'} configuration PASS")


if __name__ == "__main__":
    main()
