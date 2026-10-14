#!/usr/bin/env python3
"""Verify saved vectors, fixture inputs and schema against their generation manifest.

The manifest's other source hashes identify the separate generating implementation;
they deliberately do not describe the vector-only checkout. Never rewrite expected
vectors or generation provenance from the server being checked.
"""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
VECTORS = ROOT / "crates/eez-proof-signer/tests/fixtures/streaming-v2"


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
    verify()
