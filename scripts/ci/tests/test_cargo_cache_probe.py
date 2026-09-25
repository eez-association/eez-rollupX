"""Contracts for the isolated, opt-in target-cache transport experiment."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import time
import unittest
from unittest.mock import patch

import yaml


REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("cache_probe", REPO / "scripts/ci/cargo-cache-probe.py")
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class CargoCacheProbeTest(unittest.TestCase):
    def test_container_is_bounded_and_has_no_shared_cache_or_credentials(self):
        args = probe.container(Path("/example/job"), "cargo", "build")
        for flag in ("--cpus=1", "--memory=1g", "--memory-swap=1g", "--network=none",
                     "--read-only", "--cgroupns=private", "--env=CARGO_INCREMENTAL=0"):
            self.assertIn(flag, args)
        self.assertEqual(args.count("--mount"), 1)
        self.assertEqual(args[args.index("--mount") + 1], "type=bind,src=/example/job,dst=/probe")
        self.assertIn(probe.RUST_IMAGE, args)
        self.assertNotIn("sccache", " ".join(args))
        self.assertNotIn("docker.sock", " ".join(args))

    def test_artifact_selection_requires_nonempty_compilation_evidence(self):
        record = {"reason": "compiler-artifact", "fresh": True}
        self.assertEqual(probe.artifacts(json.dumps(record)), [record])
        with self.assertRaisesRegex(ValueError, "no compiler artifacts"):
            probe.artifacts('{"reason":"build-finished","success":true}')

    def test_restore_without_cache_fails_instead_of_building_cold(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "no cold-build fallback"):
                probe.probe("restore", Path(directory))

    def test_seed_refuses_existing_state(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "state").mkdir()
            with self.assertRaises(FileExistsError):
                probe.probe("seed", Path(directory))

    def test_reused_source_directory_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "source").mkdir()
            with self.assertRaises(FileExistsError):
                probe.probe("restore", Path(directory))

    @unittest.skipUnless(shutil.which("rsync"), "source synchronization probe requires rsync")
    def test_checksum_sync_preserves_unchanged_mtimes_and_applies_real_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            checkout = root / "checkout"
            destination = root / "source"
            probe.create_source(destination)
            old = time.time_ns() - 10_000_000_000
            for path in destination.rglob("*"):
                os.utime(path, ns=(old, old))
            probe.create_source(checkout)
            # Same size AND same mtime as cached input: only content detects this.
            (checkout / "src/data.txt").write_text("edit")
            os.utime(checkout / "src/data.txt", ns=(old, old))
            (checkout / "config/value").unlink()
            (checkout / "config/new").write_text("new")
            probe.sync_source(checkout, destination, old)
            self.assertEqual((destination / "src/main.rs").stat().st_mtime_ns, old)
            self.assertEqual((destination / "src/data.txt").read_text(), "edit")
            self.assertGreater((destination / "src/data.txt").stat().st_mtime_ns, old)
            self.assertFalse((destination / "config/value").exists())
            self.assertEqual((destination / "config/new").read_text(), "new")

    def test_future_cache_and_same_tree_rejected_before_rsync(self):
        with patch.object(probe.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "clock is ahead"):
                probe.sync_source(Path("checkout"), Path("source"), time.time_ns() + 10**12)
            with self.assertRaisesRegex(ValueError, "must be distinct"):
                probe.sync_source(Path("source"), Path("source"), 0)
            run.assert_not_called()

    def test_sync_does_not_overwrite_unchanged_timestamps_from_checkout(self):
        with patch.object(probe.subprocess, "run") as run:
            probe.sync_source(Path("checkout"), Path("source"), 0)
        args = run.call_args.args[0]
        self.assertIn("--checksum", args)
        self.assertIn("--delete", args)
        self.assertNotIn("--times", args)
        self.assertNotIn("--archive", args)
        self.assertNotIn("-a", args)

    def test_workflow_is_branch_scoped_and_cache_is_scoped_to_one_attempt(self):
        workflow = yaml.safe_load((REPO / ".github/workflows/cargo-cache-probe.yml").read_text())
        events = workflow.get("on", workflow.get(True))
        self.assertEqual(set(events), {"workflow_dispatch", "push"})
        self.assertEqual(events["push"], {"branches": ["test/cargo-cache-probe-run"]})
        ci = yaml.safe_load((REPO / ".github/workflows/ci.yml").read_text())
        self.assertEqual(ci.get("on", ci.get(True))["push"], {"branches": ["main"]})
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        self.assertEqual(workflow["jobs"]["restore"]["needs"], "seed")
        key = workflow["env"]["PROBE_CACHE_KEY"]
        self.assertIn("github.run_id", key)
        self.assertIn("github.run_attempt", key)
        self.assertEqual(workflow["env"]["PROBE_IMAGE"], probe.RUST_IMAGE)
        caches = []
        for job in workflow["jobs"].values():
            self.assertEqual(job["runs-on"], "${{ inputs.runner || 'ubuntu-24.04' }}")
            for step in job["steps"]:
                if step.get("uses", "").startswith("actions/cache/"):
                    caches.append(step)
        self.assertEqual(len(caches), 2)
        self.assertEqual(caches[0]["with"]["path"], caches[1]["with"]["path"])
        self.assertEqual(caches[0]["with"]["key"], caches[1]["with"]["key"])
        self.assertTrue(caches[1]["with"]["fail-on-cache-miss"])
        self.assertNotIn("restore-keys", caches[1]["with"])
        restore_steps = workflow["jobs"]["restore"]["steps"]
        commands = [step.get("run", "") for step in restore_steps]
        copy_step = next(i for i, command in enumerate(commands) if "cp -a" in command)
        raw_step = next(i for i, command in enumerate(commands) if "cargo-cache-probe.py restore " in command)
        sync_step = next(i for i, command in enumerate(commands) if "cargo-cache-probe.py freshness " in command)
        self.assertLess(copy_step, raw_step)
        self.assertLess(raw_step, sync_step)
        self.assertIn("command -v rsync", commands[copy_step])
        self.assertNotIn("continue-on-error", restore_steps[sync_step])
