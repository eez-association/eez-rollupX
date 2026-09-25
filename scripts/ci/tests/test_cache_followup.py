"""Boundaries and real small-workspace producer/consumer test for the follow-up."""
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch

REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("followup", REPO / "scripts/ci/rollupx-cache-followup.py")
follow = importlib.util.module_from_spec(spec)
spec.loader.exec_module(follow)


class FollowupTest(unittest.TestCase):
    def test_real_mutation_anchors_still_match_the_measured_baseline(self):
        self.assertEqual((REPO / follow.PRODUCTION_FILE).read_text().count(follow.ABOUT), 1)
        tests = REPO / follow.TEST_FILE
        self.assertNotIn(follow.CANARY, tests.read_text())
        self.assertIn("#[cfg(test)]\nmod tests;", (tests.parent / "mod.rs").read_text())

    def test_seed_commit_exception_never_ignores_source_protocol_or_command(self):
        identity = {"commit": "new", "source": "hash", "protocol": "pin", "build": ["cargo", "build"]}
        seed = {"identity": dict(identity, commit="old"), "completed_ns": 1}
        follow.check_identity(seed, identity, "old")
        for key in identity:
            invalid = dict(identity, **{key: "wrong"})
            if key == "commit":
                with self.assertRaisesRegex(ValueError, "identity"):
                    follow.check_identity(seed, identity, "wrong")
            else:
                with self.assertRaisesRegex(ValueError, "identity"):
                    follow.check_identity(seed, invalid, "old")
        seed["completed_ns"] = time.time_ns() + 10**12
        with self.assertRaisesRegex(ValueError, "future"):
            follow.check_identity(seed, identity, "old")

    def test_extraction_rejects_foreign_paths_and_warm_state(self):
        for name in ("outside/file", "state/../../escape", "/absolute"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                with tarfile.open(work / "state.tar.gz", "w:gz") as archive:
                    entry = tarfile.TarInfo(name)
                    entry.size = 1
                    archive.addfile(entry, io.BytesIO(b"x"))
                with self.assertRaisesRegex(ValueError, "outside"):
                    follow.extract(work)
                (work / "state").mkdir()
                with self.assertRaisesRegex(ValueError, "fresh"):
                    follow.extract(work)

    def test_test_inventory_requires_actual_test_executables(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            binary = work / "state/target/release/deps/test-123"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"compiled test")
            record = {"reason": "compiler-artifact", "profile": {"test": True},
                      "executable": "/probe/state/target/release/deps/test-123"}
            log = work / "tests.stdout.log"
            log.write_text(json.dumps(record))
            self.assertEqual(follow.test_executables(work, "tests"),
                             {"release/deps/test-123": follow.bench.digest(binary)})
            record["profile"]["test"] = False
            log.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "no release test"):
                follow.test_executables(work, "tests")

    @unittest.skipUnless(os.environ.get("EEZ_TEST_CACHE_FOLLOWUP_IMAGE"), "opt-in real nextest cache follow-up fixture")
    def test_real_mutations_two_archives_restore_and_new_failing_test(self):
        root = Path(tempfile.mkdtemp(prefix="cache-followup-fixture-", dir=REPO / "artifacts"))
        print(f"Preserving follow-up evidence: {root}", flush=True)
        source = root / "fixture"
        (source / "src/bin").mkdir(parents=True)
        (source / "src/settlement_policy").mkdir()
        (source / "examples").mkdir()
        (source / ".config").mkdir()
        (source / ".config/nextest.toml").write_text(
            (REPO / ".config/nextest.toml").read_text().replace("package(eez-node-e2e)", "package(eez-composer)"))
        (source / "Cargo.toml").write_text('[package]\nname="eez-composer"\nversion="0.1.0"\nedition="2021"\n')
        (source / "Cargo.lock").write_text('version = 4\n[[package]]\nname="eez-composer"\nversion="0.1.0"\n')
        (source / "src/lib.rs").write_text('pub mod settlement_policy { #[cfg(test)] mod tests; }\n')
        test_file = "src/settlement_policy/tests.rs"
        (source / test_file).write_text('#[test] fn original_test() { assert_eq!(2 + 2, 4); }\n')
        for name in follow.bench.BINARIES:
            relative = name if name.startswith("examples/") else "src/bin/" + name
            (source / (relative + ".rs")).write_text('''fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--invalid-cli-fixture") {
        eprintln!("error: unexpected argument '--invalid-cli-fixture'");
        std::process::exit(2);
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Reth Version: 1.0.0");
    } else {
        println!("Usage: fixture - EEZ window validation and attestation daemon");
    }
}\n''')
        def stage(destination):
            shutil.copytree(source, destination)
            return "controlled-source"
        image = os.environ["EEZ_TEST_CACHE_FOLLOWUP_IMAGE"]
        with patch.object(follow.bench, "stage_source", side_effect=stage), \
                patch.object(follow.bench, "BUILD", ("cargo", "build", "--release", "--locked", "--bins", "--examples")), \
                patch.object(follow, "SEED_COMMIT", follow.bench.output("git", "rev-parse", "HEAD")), \
                patch.object(follow, "TEST_FILE", test_file), \
                patch.object(follow, "PRODUCTION_FILE", "src/bin/eez-proof-signer.rs"), \
                patch.object(follow.native, "ARCHIVES", {name: ["--package", "eez-composer", "--lib"] for name in follow.native.ARCHIVES}):
            follow.bench.benchmark("seed", root / "shipping", image)
            for mode in ("mutations", "archives-seed", "archives-restore"):
                work = root / mode
                work.mkdir()
                archive = root / ("archives-seed/test-state.tar.gz" if mode == "archives-restore" else "shipping/state.tar.gz")
                shutil.copy2(archive, work / "state.tar.gz")
                follow.followup(mode, work, image)
                report = json.loads((work / "result.json").read_text())
                self.assertEqual(report["status"], "complete")
                if mode == "mutations":
                    self.assertTrue(report["test_only"]["reuse"]["all_fresh"])
                    self.assertEqual(report["changed_binaries"], ["eez-proof-signer"])
                if mode == "archives-restore":
                    self.assertTrue(all(value["reuse"]["all_fresh"] for value in report["archives"].values()))
                    self.assertFalse(report["test_edit"]["reuse"]["all_fresh"])
        self.assertNotIn(follow.CANARY, (source / test_file).read_text())
