#!/usr/bin/env python3
"""Compile shipping outputs, freeze them, then produce the two release archives.

Shared by the Docker builder and the opt-in bounded Cargo-state producer. This
module never restores caches, publishes images, seals revisions or runs tests.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

REPO = Path(__file__).resolve().parents[2]
BUILD = ("cargo", "build", "--locked", "--timings", "--profile", "release",
         "-p", "eez-node", "-p", "eez-follower", "-p", "eez-proof-signer",
         "-p", "eez-protocol", "-p", "eez-l1", "--bin", "eez-composer",
         "--bin", "eez-follower", "--bin", "eez-proof-signer",
         "--example", "genesis_state_root", "--example", "genesis_block_hash",
         "--example", "e2e-causal-verify", "--example", "verify_blob_evidence")
BINARIES = {
    "eez-composer": "eez-composer", "eez-follower": "eez-follower",
    "eez-proof-signer": "eez-proof-signer",
    "examples/genesis_state_root": "eez-genesis-state-root",
    "examples/genesis_block_hash": "eez-genesis-block-hash",
    "examples/e2e-causal-verify": "e2e-causal-verify",
    "examples/verify_blob_evidence": "verify_blob_evidence",
}


def build(output):
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise ValueError("runtime build output must be empty")
    target = Path(os.environ["CARGO_TARGET_DIR"]).resolve()
    env = dict(os.environ, EEZ_BUILD_METRICS_DIR=str(output / "build-metrics"))
    subprocess.run(["bash", "scripts/ci/with-compiler-cache.sh", "runtime", *BUILD],
                   cwd=REPO, env=env, check=True)
    (output / "bin").mkdir()
    for source, name in BINARIES.items():
        shutil.copy2(target / "release" / source, output / "bin" / name)
        (output / "bin" / name).chmod(0o755)
    shutil.copy2(target / "cargo-timings/cargo-timing.html", output / "cargo-timing.html")
    for name, args in (("rustc-version", ["rustc", "-Vv"]), ("cargo-version", ["cargo", "-V"])):
        with (output / f"{name}.txt").open("w") as stream:
            subprocess.run(args, cwd=REPO, stdout=stream, check=True)
    # Keep the production producer's separate feature selections and frozen-byte
    # checks. Run it as a subprocess so build metrics use this private output.
    subprocess.run(["python3", "scripts/ci/native-tests.py", "build", str(output / "native-tests"),
                    "--runtime-bin", str(output / "bin")], cwd=REPO, env=env, check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    build(parser.parse_args().output.resolve())
