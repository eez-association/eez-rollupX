"""Shared shipping selection and freeze-before-tests ordering."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("builder", REPO / "scripts/ci/build-native-runtime.py")
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class NativeRuntimeBuildTest(unittest.TestCase):
    def test_locked_shipping_selection_and_seven_exports_are_unchanged(self):
        args = builder.BUILD
        self.assertEqual(args[:7], ("cargo", "build", "--locked", "--timings", "--profile", "release", "-p"))
        selections = lambda flag: [args[index + 1] for index, value in enumerate(args) if value == flag]
        self.assertEqual(selections("-p"), ["eez-node", "eez-follower", "eez-proof-signer", "eez-protocol", "eez-l1"])
        self.assertEqual(selections("--bin"), ["eez-composer", "eez-follower", "eez-proof-signer"])
        self.assertEqual(selections("--example"), ["genesis_state_root", "genesis_block_hash", "e2e-causal-verify", "verify_blob_evidence"])
        self.assertEqual(len(builder.BINARIES), 7)
        self.assertNotIn("--all-features", args)
        self.assertNotIn("--tests", args)

    def test_export_precedes_native_archives_and_survives_test_role_rebuilds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target, output = root / "target", root / "output"
            def run(args, **kwargs):
                if args[0] == "bash":
                    for name in builder.BINARIES:
                        path = target / "release" / name
                        path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_bytes(name.encode())
                    (target / "cargo-timings").mkdir()
                    (target / "cargo-timings/cargo-timing.html").write_text("timings")
                elif args[0] == "python3":
                    self.assertEqual(args[:3], ["python3", "scripts/ci/native-tests.py", "build"])
                    for source, name in builder.BINARIES.items():
                        self.assertEqual((output / "bin" / name).read_bytes(), source.encode())
                        self.assertTrue(os.access(output / "bin" / name, os.X_OK))
                        (target / "release" / source).write_bytes(b"test feature variant")
                else:
                    kwargs["stdout"].write("tool version\n")
            with patch.dict(os.environ, CARGO_TARGET_DIR=str(target)), \
                    patch.object(builder.subprocess, "run", side_effect=run):
                builder.build(output)
            self.assertEqual((output / "bin/eez-composer").read_bytes(), b"eez-composer")
            self.assertEqual((output / "cargo-timing.html").read_text(), "timings")

    def test_build_failure_is_never_retried_or_packaged(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(os.environ, CARGO_TARGET_DIR=str(root / "target")), \
                    patch.object(builder.subprocess, "run", side_effect=subprocess.CalledProcessError(7, "build")) as run:
                with self.assertRaises(subprocess.CalledProcessError):
                    builder.build(root / "output")
                self.assertEqual(run.call_count, 1)
                self.assertFalse((root / "output/bin").exists())

    def test_existing_outputs_are_preserved_and_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "keep").write_bytes(b"existing")
            with self.assertRaisesRegex(ValueError, "empty"):
                builder.build(root)
            self.assertEqual((root / "keep").read_bytes(), b"existing")
