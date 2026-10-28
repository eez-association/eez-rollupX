#!/usr/bin/env python3
"""Check frozen v2 vectors; explicitly record provenance after regeneration.

Neither mode obtains expected responses from a server. The Rust fixture generator
constructs those expectations from captured headers and request-side settlement.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[3]
VECTORS = ROOT / "crates/eez-proof-signer/tests/fixtures/streaming-v2"

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_manifest():
    sources = [ROOT / name for name in ["Cargo.toml", "Cargo.lock",
        "crates/eez-control-rpc/build.rs", "crates/eez-control-rpc/conformance/check_vectors.py",
        "crates/eez-control-rpc/conformance/replay.rs",
        "crates/eez-control-rpc/examples/replay_streaming.rs"]]
    for package in ["eez-proof-signer", "eez-control-rpc", "eez-protocol", "eez-primitives",
                    "eez-evm", "eez-evm-inspector", "eez-l1", "eez-driver",
                    "eez-payload-codec", "eez-blob-codec"]:
        sources.append(ROOT / f"crates/{package}/Cargo.toml")
        sources.extend((ROOT / f"crates/{package}/src").rglob("*.rs"))
    sources.extend((ROOT / "crates/eez-control-rpc/proto").glob("*.proto"))
    sources.extend([ROOT / "contracts/src/L2/L1BlockhashContract.sol",
                    ROOT / "contracts/src/OfflineRollup.sol",
                    ROOT / "contracts/src/L2/FeeVault.sol",
                    ROOT / "scripts/solidity/GenerateEEZL2Genesis.s.sol",
                    ROOT / "scripts/update-eezl2-genesis.sh",
                    ROOT / "crates/eez-protocol/src/l1_history/runtime.bin",
                    ROOT / "crates/eez-protocol/src/l1_history/implementation.bin",
                    ROOT / "crates/eez-protocol/src/l1_history/proxy_admin.bin"])
    fixtures = []
    for name in ["native-offline-bidirectional", "native-osaka-lookup",
                 "native-osaka-changing-static"]:
        for filename in ["identity.json", "l1-genesis.json", "l2-genesis.json",
                         "request.v2.protobuf", "response.json"]:
            fixtures.append(VECTORS.parent / name / filename)
    vectors = {}
    for path in sorted(VECTORS.glob("*.json")):
        if path.name == "manifest.json":
            continue
        vector = json.loads(path.read_text())
        record = {"sha256": digest(path), "events": len(vector["events"]),
                  "proofs": vector["expected_proofs"]}
        for direction, field in [("client", "requests"), ("server", "responses"),
                                 ("rpc_error", "rpc_errors"), ("rpc_open_error", "rpc_open_errors")]:
            record[field] = sum(e["direction"] == direction for e in vector["events"])
        vectors[path.name] = record
    revision = lambda directory: subprocess.check_output(
        ["git", "-C", str(directory), "rev-parse", "HEAD"], text=True).strip()
    manifest = {
        "format": 2, "base_commit": revision(ROOT),
        "protocol_gitlink": revision(ROOT / "eez-core-protocol"),
        "implementation": "Versionless cross-chain 0x45 with execution-group SOURCE_ENTRY_V1 identity, optional history 0x46/v1 with explicit pairs and an initially empty upgradeable store, complete-group and block-finalization cleanup, Profile, validation-contract version 6, v2 capability profile 2; source digests include uncommitted changes at generation",
        "authority": "Complete captured L1 evidence; signatures are conditional on the independently checked on-chain context",
        "generator": "crates/eez-proof-signer/src/service/sessions/tests/vectors.rs",
        "source_sha256": {str(p.relative_to(ROOT)): digest(p) for p in sorted(set(sources))},
        "fixture_inputs_sha256": {str(p.relative_to(ROOT)): digest(p) for p in sorted(fixtures)},
        "vectors": vectors,
    }
    (VECTORS / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


def verify():
    manifest = json.loads((VECTORS / "manifest.json").read_text())
    actual_names = {p.name for p in VECTORS.glob("*.json") if p.name != "manifest.json"}
    assert actual_names == set(manifest["vectors"]), "missing or unexpected vector"
    assert actual_names, "no vectors"
    for name, expected in manifest["vectors"].items():
        path = VECTORS / name
        assert hashlib.sha256(path.read_bytes()).hexdigest() == expected["sha256"], path
        vector = json.loads(path.read_text())
        assert vector["format"] == 2 and vector["case"] == path.stem, path
        assert len(vector["events"]) == expected["events"] > 0, path
        for direction, field in [("client", "requests"), ("server", "responses"),
                                 ("rpc_error", "rpc_errors"), ("rpc_open_error", "rpc_open_errors")]:
            assert sum(e["direction"] == direction for e in vector["events"]) == expected[field], path
        assert vector["expected_proofs"] == expected["proofs"], path
    inputs = dict(manifest["fixture_inputs_sha256"])
    inputs.update((name, digest) for name, digest in manifest["source_sha256"].items()
                  if name.endswith(".proto"))
    for name, expected in inputs.items():
        assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
    print(f"Verified {len(actual_names)} vectors, fixture inputs and schemas")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write-manifest", action="store_true",
                        help="record current fixture/generator/schema/vector digests after explicit regeneration")
    if parser.parse_args().write_manifest:
        write_manifest()
    verify()
