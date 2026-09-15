#!/usr/bin/env python3
"""Prepare isolated explorer credentials without reading any chain signing keys."""
import argparse
import json
from pathlib import Path
import secrets


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--genesis", type=Path, required=True)
    args = parser.parse_args()
    genesis = args.genesis.resolve(strict=True)
    config = json.loads(genesis.read_text())["config"]
    if config["chainId"] != 6290:
        parser.error("This deployment profile requires EEZ L2 chain 6290")
    runtime = args.runtime.resolve()
    runtime.mkdir(parents=True, exist_ok=True, mode=0o700)
    env_file = runtime / "explorers.env"
    if env_file.exists():
        print(f"Keeping existing explorer credentials: {env_file}")
        return
    values = {
        "BLOCKSCOUT_DB_PASSWORD": secrets.token_hex(24),
        "BLOCKSCOUT_SECRET_KEY_BASE": secrets.token_hex(48),
        "BLOBSCAN_DB_PASSWORD": secrets.token_hex(24),
        "BLOBSCAN_SECRET_KEY": secrets.token_hex(48),
        "L2_GENESIS": str(genesis),
    }
    with env_file.open("x") as output:
        env_file.chmod(0o600)
        output.write("".join(f"{key}={value}\n" for key, value in values.items()))
    print(f"Prepared explorer credentials: {env_file}")


if __name__ == "__main__":
    main()
