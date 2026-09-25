"""Cache rejection, authoritative sources, resource limits and workflow gates."""
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch
import yaml

REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("state", REPO / "scripts/ci/cargo-state.py")
state = importlib.util.module_from_spec(spec)
spec.loader.exec_module(state)


class CargoStateTest(unittest.TestCase):
    def seed(self, root, **overrides):
        source = root / "seed/state"
        for name in ("source", "cargo", "target"):
            (source / name).mkdir(parents=True)
        (source / "source/Cargo.lock").write_text("fixture")
        metadata = dict(compatibility="tools", source=state.inventory(source / "source"), completed_ns=time.time_ns())
        metadata.update(overrides)
        (source / "cache.json").write_text(json.dumps(metadata))
        work = root / "work"
        work.mkdir()
        with tarfile.open(work / "transport.tar.gz", "w:gz") as archive:
            archive.add(source, arcname="state")
        return work

    def test_cache_hit_and_miss_do_not_run_compilers(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(state, "run") as run:
            root = Path(directory)
            self.assertEqual(state.restore(root, "tools"), "miss")
            work = self.seed(root)
            self.assertEqual(state.restore(work, "tools"), "hit")
            run.assert_not_called()
            with self.assertRaisesRegex(ValueError, "fresh"):
                state.restore(work, "tools")

    def test_incompatible_future_and_inconsistent_state_is_quarantined(self):
        for changes in ({"compatibility": "wrong"}, {"source": "wrong"},
                        {"completed_ns": time.time_ns() + 10**15}):
            with self.subTest(changes=changes), tempfile.TemporaryDirectory() as directory:
                work = self.seed(Path(directory), **changes)
                self.assertEqual(state.restore(work, "tools"), "rejected")
                self.assertFalse((work / "state").exists())
                self.assertTrue((work / "rejected-state/cache.json").exists())
                self.assertTrue((work / "transport.tar.gz").exists())

    def test_safe_extraction_permission_normalization_preserves_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = self.seed(root)
            source = root / "seed/state/source/Cargo.lock"
            source.chmod(0o664)
            before = state.inventory(source.parent)
            with tarfile.open(work / "transport.tar.gz", "w:gz") as archive:
                archive.add(root / "seed/state", arcname="state")
            self.assertEqual(state.restore(work, "tools"), "hit")
            self.assertEqual((work / "state/source/Cargo.lock").stat().st_mode & 0o777, 0o644)
            self.assertEqual(state.inventory(work / "state/source"), before)

    def test_bad_archive_paths_corruption_and_size_are_cold_fallbacks(self):
        for name in ("../escaped", "/absolute", "state/../../escaped"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                with tarfile.open(work / "transport.tar.gz", "w:gz") as archive:
                    entry = tarfile.TarInfo(name)
                    entry.size = 1
                    archive.addfile(entry, io.BytesIO(b"x"))
                self.assertEqual(state.restore(work, "tools"), "rejected")
                self.assertFalse((work / "state").exists())
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            (work / "transport.tar.gz").write_bytes(b"invalid gzip")
            self.assertEqual(state.restore(work, "tools"), "rejected")
        with tempfile.TemporaryDirectory() as directory:
            work = self.seed(Path(directory))
            for cap in ("MAX_ARCHIVE", "MAX_EXPANDED"):
                with patch.object(state, cap, 1):
                    self.assertEqual(state.restore(work, "tools"), "rejected")

    def test_mode_content_and_deletions_change_source_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input"
            source.write_text("one")
            identities = [state.inventory(root)]
            source.write_text("two")
            identities.append(state.inventory(root))
            source.chmod(0o755)
            identities.append(state.inventory(root))
            other = root / "added"
            other.write_text("new")
            identities.append(state.inventory(root))
            source.unlink()
            identities.append(state.inventory(root))
            self.assertEqual(len(set(identities)), len(identities))
            source.symlink_to(other)
            with self.assertRaisesRegex(ValueError, "non-regular"):
                state.inventory(root)

    def test_resource_and_credentials_boundary(self):
        args = state.container(Path("/job/private"), "tools", ["cargo", "build"])
        for arg in ("--cpus=4", "--memory=20g", "--memory-swap=20g", "--network=none", "--read-only",
                    "--cap-drop=ALL", "--security-opt=no-new-privileges", "--cgroupns=private"):
            self.assertIn(arg, args)
        self.assertEqual(args.count("--mount"), 1)
        self.assertNotIn("docker.sock", " ".join(args))
        self.assertNotIn("SCCACHE", " ".join(args))
        self.assertIn("test", args[args.index("-ec") + 1])

    def test_stage_uses_only_clean_tracked_inputs_and_normalizes_permissions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            (repo / "crates").mkdir()
            source = repo / "crates/input.rs"
            source.write_text("tracked")
            source.chmod(0o664)
            for args in (("init", "-q"), ("add", "."),
                         ("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture")):
                subprocess.run(["git", "-C", str(repo), *args], check=True)
            (repo / "crates/private.env").write_text("not a build input")
            with patch.object(state, "REPO", repo), patch.object(state, "INPUTS", ("crates",)):
                output = root / "staged"
                state.stage(output)
                self.assertEqual(list((output / "crates").iterdir()), [output / "crates/input.rs"])
                self.assertEqual((output / "crates/input.rs").stat().st_mode & 0o777, 0o644)
                source.write_text("dirty")
                with self.assertRaises(subprocess.CalledProcessError):
                    state.stage(root / "dirty")
                source.unlink()
                with self.assertRaises(subprocess.CalledProcessError):
                    state.stage(root / "deleted")

    def test_compilation_failure_runs_once_and_keeps_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            def execute(args, **kwargs):
                if args[:2] == ["docker", "run"]:
                    kwargs["stdout"].write("fixture compile failure\n")
                    return subprocess.CompletedProcess(args, 7)
                return subprocess.CompletedProcess(args, 0)
            with patch.object(state.subprocess, "run", side_effect=execute) as run:
                with self.assertRaises(subprocess.CalledProcessError) as failure:
                    state.run(work, "tools", "build", ["cargo", "build"])
                self.assertEqual(failure.exception.returncode, 7)
                self.assertEqual(sum(c.args[0][:2] == ["docker", "run"] for c in run.call_args_list), 1)
                self.assertIn("fixture compile failure", (work / "build.log").read_text())

    def test_oversize_upload_is_optional_and_never_removes_state(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            metadata = {"source": "source", "compatibility": "tools"}
            for name in ("plan", "built"):
                (work / f"{name}.json").write_text(json.dumps(metadata))
            def archive(*args):
                (work / "transport.tar.gz").write_bytes(b"too large")
            output = work / "outputs"
            with patch.object(state, "checked", side_effect=archive), patch.object(state, "MAX_ARCHIVE", 1), \
                    patch.dict(os.environ, GITHUB_OUTPUT=str(output)):
                state.archive(work)
            self.assertEqual(output.read_text(), "upload=false\n")
            self.assertTrue((work / "transport.tar.gz").exists())

    def test_workflow_default_rollback_main_only_save_and_required_build(self):
        workflow = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())
        job = workflow["jobs"]["build"]
        self.assertEqual(job["env"]["EEZ_CARGO_STATE_CACHE"],
                         "${{ vars.CI_CARGO_STATE_CACHE == 'true' && inputs.runner == 'homelab-rust' }}")
        steps = job["steps"]
        restore = next(s for s in steps if s.get("id") == "cargo-restore")
        archive = next(s for s in steps if s.get("id") == "cargo-archive")
        save = next(s for s in steps if s.get("uses", "").startswith("actions/cache/save@"))
        self.assertEqual(restore["with"]["path"], save["with"]["path"])
        self.assertEqual(restore["with"]["key"], save["with"]["key"])
        self.assertIn("steps.cargo-state.outputs.prefix", restore["with"]["restore-keys"])
        self.assertIn("github.event_name == 'push'", archive["if"])
        self.assertIn("github.event.repository.default_branch", archive["if"])
        self.assertIn("cache-hit != 'true'", archive["if"])
        self.assertIn("steps.cargo-archive.outputs.upload == 'true'", save["if"])
        for step in (restore, archive, save):
            self.assertTrue(step["continue-on-error"])
        for step in steps:
            if "build-runtime-artifacts.sh" in step.get("run", "") or "--suite unit" in step.get("run", ""):
                self.assertNotIn("continue-on-error", step)
        self.assertIn(".ci-cargo-state/", (REPO / ".dockerignore").read_text())
