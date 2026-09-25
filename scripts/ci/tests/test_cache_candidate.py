"""Fail-closed boundaries for the temporary unpublished candidate producer."""
import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("candidate", REPO / "scripts/ci/rollupx-cache-candidate.py")
candidate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(candidate)


class CandidateTest(unittest.TestCase):
    def test_packaging_uses_production_runtime_stages_without_rust_builder(self):
        text = candidate.packaging_dockerfile()
        marker = "FROM ${RUNTIME_IMAGE} AS runtime-base"
        self.assertEqual(text.split(marker)[1], (REPO / "Dockerfile.runtime").read_text().split(marker)[1])
        self.assertIn("FROM scratch AS builder\nCOPY bin /out/bin/", text)
        self.assertNotIn("cargo build", text)

    def test_changed_runtime_boundary_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / "Dockerfile.runtime").write_text("FROM unexpected")
            with patch.object(candidate.bench, "REPO", repo):
                with self.assertRaisesRegex(ValueError, "boundary"):
                    candidate.packaging_dockerfile()

    def test_deploy_context_contains_only_regular_tracked_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / "genesis.json").write_text("{}")
            (repo / ".env").write_text("private")
            with patch.object(candidate.bench, "REPO", repo), \
                    patch.object(candidate.subprocess, "check_output", return_value=b"genesis.json\0"):
                candidate.stage_deploy(repo / "context")
                self.assertEqual([p.name for p in (repo / "context").iterdir()], ["genesis.json"])
                (repo / "genesis.json").unlink()
                (repo / "genesis.json").symlink_to(repo / ".env")
                with self.assertRaisesRegex(ValueError, "regular tracked"):
                    candidate.stage_deploy(repo / "bad-context")

    def test_package_rejects_changed_revision_runtime_archive_or_helper_before_docker(self):
        for mutation, message in (("revision", "revision"), ("runtime", "sealed candidate"),
                                  ("archive", "changed after sealing"), ("helper", "differs from runtime archive")):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                binaries = work / "output/bin"
                binaries.mkdir(parents=True)
                for name in candidate.EXPORT_NAMES.values():
                    (binaries / name).write_text("#!/bin/sh\nexit 0\n")
                    (binaries / name).chmod(0o755)
                runtime = work / "runtime/runtime-binaries.tar.gz"
                runtime.parent.mkdir()
                with tarfile.open(runtime, "w:gz") as archive:
                    archive.add(binaries, arcname="bin")
                manifest = {"root_commit": "root", "runtime_sha256": candidate.native.runtime_hashes(binaries),
                            "runtime_archive_sha256": candidate.bench.digest(runtime)}
                if mutation == "revision":
                    manifest["root_commit"] = "wrong"
                elif mutation == "runtime":
                    (binaries / "eez-composer").write_text("changed")
                elif mutation == "archive":
                    runtime.write_bytes(b"changed")
                else:
                    (binaries / "eez-genesis-state-root").write_text("changed")
                (work / "native-tests").mkdir()
                (work / "native-tests/manifest.json").write_text(json.dumps(manifest))
                with patch.object(candidate, "checkout_identity", return_value={"root_commit": "root"}), \
                        patch.object(candidate.native, "verify_archives"), \
                        patch.object(candidate.bench, "output") as docker:
                    with self.assertRaisesRegex(ValueError, message):
                        candidate.package(work)
                    docker.assert_not_called()

    def test_image_byte_mismatch_removes_only_owned_container(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "binary").write_bytes(b"expected")
            def checked(*args):
                if args[:2] == ("docker", "cp"):
                    Path(args[-1]).write_bytes(b"wrong image bytes")
            with patch.object(candidate, "checked", side_effect=checked) as docker, \
                    patch.object(candidate.subprocess, "run") as cleanup:
                with self.assertRaisesRegex(ValueError, "packaged image changed"):
                    candidate.verify_image_bytes("local-image", ["binary"], root, root / "copy")
                created = docker.call_args_list[0].args[3]
                self.assertTrue(created.startswith("eez-cache-image-check-"))
                self.assertEqual(cleanup.call_args.args[0], ["docker", "rm", "--force", created])
