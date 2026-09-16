"""Exercise the Blobscan verifier with complete blob bytes and offline services."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
SCRIPT = REPO / "testing/kurtosis/scripts/verify-blobscan.sh"


@unittest.skipUnless(all(shutil.which(name) for name in ("jq", "xxd", "sha256sum")),
                     "requires the verifier's local JSON and hashing tools")
class BlobscanEvidenceTest(unittest.TestCase):
    def run_verifier(self, *, objects=False, corrupt=False):
        with tempfile.TemporaryDirectory(prefix="eez-blobscan-evidence.") as temporary:
            root = Path(temporary)
            tools = root / "bin"
            tools.mkdir()
            blob = bytes(range(256)) * 512
            tx_hash = "0x" + "12" * 32
            versioned_hash = "0x01" + "34" * 31
            commitment = "0x" + "56" * 48
            encoded = "0x" + blob.hex()
            beacon = "0x" + (b"\xff" * len(blob)).hex() if corrupt else encoded
            responses = {
                "healthcheck": "yay!",
                "transaction": {"hash": tx_hash, "blockNumber": 7,
                                "blobs": [{"versionedHash": versioned_hash}]},
                "detail": {"versionedHash": versioned_hash, "commitment": commitment,
                           "size": len(blob), "dataStorageReferences": [{"storage": "postgres", "url": "fixture"}],
                           "transactions": [{"txHash": tx_hash}]},
                "indexed": encoded.upper().replace("0X", "0x"),
                "beacon": {"data": [{"blob": beacon}] if objects else [beacon]},
            }
            (root / "responses.json").write_text(json.dumps(responses))
            (root / "blob.bin").write_bytes(blob)
            summary = root / "summary.json"
            summary.write_text(json.dumps({"verified": True, "verified_blob_batches": [{
                "l1_chain_id": 7331, "l1_transaction_hash": tx_hash, "l1_block_number": 7,
                "beacon_slot": 7, "blob_count": 1, "blob_versioned_hashes": [versioned_hash],
                "kzg_commitments": [commitment], "blob_indices": [0],
            }]}))
            (tools / "cast").write_text("#!/bin/sh\nprintf '7331\\n'\n")
            (tools / "kurtosis").write_text("#!/bin/sh\nprintf '127.0.0.1:9999\\n'\n")
            (tools / "curl").write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root=Path(os.environ['EEZ_BLOBSCAN_FIXTURE'])
responses=json.loads((root/'responses.json').read_text())
args=sys.argv[1:]
url=next(value for value in args if value.startswith('http'))
if '/api/blob-data?' in url:
    content=(root/'blob.bin').read_bytes()
else:
    if url.endswith('/healthcheck'): value=responses['healthcheck']
    elif '/transactions/' in url: value=responses['transaction']
    elif '/eth/v1/beacon/' in url: value=responses['beacon']
    elif url.endswith('/data'): value=responses['indexed']
    elif '/blobs/' in url: value=responses['detail']
    else: value='fixture page'
    content=json.dumps(value).encode()
if '-o' in args: Path(args[args.index('-o')+1]).write_bytes(content)
else: sys.stdout.buffer.write(content)
if '-w' in args: sys.stdout.write('200')
''')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            output = root / "result.json"
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"],
                       EEZ_BLOBSCAN_FIXTURE=str(root), EEZ_BLOBSCAN_TIMEOUT="1",
                       KURTOSIS_ENCLAVE="fixture", BLOBSCAN_API="http://fixture-api",
                       BLOBSCAN_WEB="http://fixture-web", L1_RPC="http://fixture-l1",
                       EEZ_L1_BEACON_URL="http://fixture-beacon")
            completed = subprocess.run(["bash", str(SCRIPT), str(summary), str(output)],
                                       env=env, capture_output=True, text=True, timeout=20)
            evidence = json.loads(output.read_text()) if output.exists() else None
            return completed, evidence, hashlib.sha256(blob).hexdigest()

    def test_full_size_blob_matches_beacon_and_web_storage_in_both_api_layouts(self):
        for objects in (False, True):
            with self.subTest(objects=objects):
                completed, evidence, digest = self.run_verifier(objects=objects)
                self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
                self.assertTrue(evidence["verified"])
                self.assertEqual(evidence["blobs"][0]["blob_sha256"], digest)
                self.assertTrue(evidence["blobs"][0]["web_proxy_matches_beacon_sidecar"])

    def test_different_full_size_beacon_bytes_fail_without_success_evidence(self):
        completed, evidence, _ = self.run_verifier(corrupt=True)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIsNone(evidence)


if __name__ == "__main__":
    unittest.main()
