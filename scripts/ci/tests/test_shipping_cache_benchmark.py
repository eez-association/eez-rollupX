"""Boundaries of the opt-in real shipping-cache sizing experiment."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import tempfile
import unittest
from unittest.mock import patch

import yaml

REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("shipping_benchmark", REPO / "scripts/ci/rollupx-cache-benchmark.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class ShippingCacheBenchmarkTest(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("EEZ_TEST_SHIPPING_CACHE_IMAGE"), "opt-in bounded Docker cache orchestration fixture")
    def test_real_seed_archive_restore_orchestration_with_tiny_fixture(self):
        # Never builds RollupX: only exercise the new orchestration against
        # seven tiny executables, including the real archive round trip.
        (REPO / "artifacts").mkdir(exist_ok=True)
        root = Path(tempfile.mkdtemp(prefix="shipping-cache-fixture-", dir=REPO / "artifacts"))
        print(f"Preserving fixture evidence: {root}", flush=True)
        fixture = root / "fixture"
        (fixture / "src/bin").mkdir(parents=True)
        (fixture / "examples").mkdir()
        (fixture / "Cargo.toml").write_text('[package]\nname="shipping-cache-fixture"\nversion="0.1.0"\nedition="2021"\n')
        (fixture / "Cargo.lock").write_text('version = 4\n[[package]]\nname="shipping-cache-fixture"\nversion="0.1.0"\n')
        for name in bench.BINARIES:
            relative = name if name.startswith("examples/") else "src/bin/" + name
            (fixture / (relative + ".rs")).write_text('fn main() { println!("fixture"); }\n')
        def stage(destination):
            shutil.copytree(fixture, destination)
            return "controlled-fixture"
        with patch.object(bench, "stage_source", side_effect=stage), patch.object(bench, "BUILD",
                ("cargo", "build", "--release", "--locked", "--bins", "--examples")):
            bench.benchmark("seed", root / "seed", os.environ["EEZ_TEST_SHIPPING_CACHE_IMAGE"])
            (root / "restore").mkdir()
            shutil.copy2(root / "seed/state.tar.gz", root / "restore/state.tar.gz")
            bench.benchmark("restore", root / "restore", os.environ["EEZ_TEST_SHIPPING_CACHE_IMAGE"])
        restored = json.loads((root / "restore/result.json").read_text())
        self.assertTrue(restored["reuse"]["all_fresh"])
        self.assertEqual(len(restored["hashes"]), 7)

    def test_command_matches_production_shipping_selection(self):
        dockerfile = (REPO / "Dockerfile.runtime").read_text().replace("\\\n", " ")
        production = re.search(r"runtime (cargo build .*?)\s+&& install", dockerfile).group(1)
        self.assertEqual(shlex.split(production), list(bench.BUILD))

    def test_native_packages_match_production(self):
        packages = []
        for name in ("Dockerfile.runtime", "scripts/ci/Dockerfile.cache-benchmark"):
            content = (REPO / name).read_text().replace("\\\n", " ")
            packages.append(shlex.split(re.search(r"install -y --no-install-recommends\s+(.*?)&&", content).group(1)))
        self.assertEqual(*packages)

    def test_container_caps_and_private_state(self):
        args = bench.container(Path("/owned/experiment"), "image", ["cargo", "build"])
        for option in ("--cpus=4", "--memory=20g", "--memory-swap=20g", "--network=none",
                       "--read-only", "--env=CARGO_INCREMENTAL=0", "--env=CARGO_BUILD_JOBS=4",
                       "--env=RUSTFLAGS=-C link-arg=-fuse-ld=mold"):
            self.assertIn(option, args)
        self.assertEqual(args.count("--mount"), 1)
        self.assertNotIn("docker.sock", " ".join(args))
        self.assertNotIn("sccache", " ".join(args))
        self.assertIn("--network=bridge", bench.container(Path("/owned"), "image", ["cargo", "fetch"], True))

    def test_freshness_requires_artifacts_and_counts_recompilation(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "cargo.log"
            log.write_text('{"reason":"build-finished","success":true}\n')
            with self.assertRaisesRegex(ValueError, "no compilation evidence"):
                bench.freshness(log)
            log.write_text("\n".join(json.dumps({"reason": "compiler-artifact", "fresh": f}) for f in (True, False)))
            self.assertEqual(bench.freshness(log), {"artifacts": 2, "fresh": 1, "all_fresh": False})

    def test_archive_size_is_a_hard_upload_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "state.tar.gz"
            archive.write_bytes(b"abc")
            self.assertEqual(bench.check_archive_size(archive), 3)
            with patch.object(bench, "MAX_ARCHIVE", 2):
                with self.assertRaisesRegex(ValueError, "do not upload"):
                    bench.check_archive_size(archive)

    def test_staging_excludes_untracked_files_and_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / "Cargo.toml").write_text("tracked")
            (repo / ".env").write_text("not for the cache")
            with patch.object(bench, "REPO", repo), patch.object(bench, "output", return_value="Cargo.toml\0"), \
                    patch.object(bench.subprocess, "run"):
                bench.stage_source(repo / "copy")
                self.assertEqual([p.name for p in (repo / "copy").iterdir()], ["Cargo.toml"])
                (repo / "Cargo.toml").unlink()
                (repo / "Cargo.toml").symlink_to(repo / ".env")
                with self.assertRaisesRegex(ValueError, "regular tracked file"):
                    bench.stage_source(repo / "other")

    def test_only_execution_branch_triggers_and_no_publication(self):
        workflow = yaml.safe_load((REPO / ".github/workflows/rollupx-cache-benchmark.yml").read_text())
        self.assertEqual(workflow.get("on", workflow.get(True)),
                         {"push": {"branches": ["test/rollupx-cache-benchmark"]}})
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        self.assertEqual(workflow["jobs"]["restore"]["needs"], "seed")
        key = workflow["env"]["BENCH_CACHE_KEY"]
        self.assertIn("github.run_id", key)
        self.assertIn("github.run_attempt", key)
        caches = []
        for job in workflow["jobs"].values():
            self.assertEqual(job["runs-on"], "homelab-rust")
            for step in job["steps"]:
                self.assertNotIn("docker push", step.get("run", ""))
                if step.get("uses", "").startswith("actions/cache/"):
                    caches.append(step["with"])
        self.assertEqual(caches[0]["path"], "artifacts/shipping-cache/state.tar.gz")
        self.assertEqual(caches[0]["path"], caches[1]["path"])
        self.assertEqual(caches[0]["key"], caches[1]["key"])
        self.assertTrue(caches[1]["fail-on-cache-miss"])
        self.assertNotIn("restore-keys", caches[1])
