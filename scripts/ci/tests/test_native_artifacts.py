"""Same-run test archives: identity checks, coverage, relocation and no rebuilds."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tarfile
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("native_tests", REPO / "scripts/ci/native-tests.py")
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


class NativeArtifactTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="eez-native-contract.")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.archive = self.root / "native-tests.tar.zst"
        for name in native.ARCHIVES:
            (self.root / name).write_bytes(b"fixture archive")
        self.identity = {"schema": 3, "root_commit": "a" * 40, "protocol_commit": "b" * 40,
                         "cargo_lock_sha256": "c" * 64, "platform": "linux/amd64",
                         "profile": "release", "rustflags": native.RUSTFLAGS,
                         "nextest": native.NEXTEST_VERSION, "rust": native.RUST_VERSION}
        self.frozen = self.root / "shipping"
        self.frozen.mkdir()
        for name in native.RUNTIME_BINARIES:
            path = self.frozen / name
            path.write_text("#!/bin/sh\nexit 0\n")
            path.chmod(0o755)
        self.manifest = dict(self.identity, archives_sha256={name: native.digest(self.root / name) for name in native.ARCHIVES},
                             glibc="glibc 2.39", runtime_archive_sha256="d" * 64,
                             runtime_sha256=native.runtime_hashes(self.frozen))
        self.target = self.root / "job-target"

    def run_archive(self, suite="unit", failures=(), partition=None):
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))
        def extract(command, **_):
            if command[:3] == ["cargo-nextest", "nextest", "list"]:
                extraction = Path(command[command.index("--extract-to") + 1])
                self.assertTrue(extraction.is_dir(), "nextest requires an existing extraction directory")
                shutil.copytree(self.frozen, extraction / "target/release/eez-runtime")
                if suite == "cross-chain":
                    metadata = extraction / "target/nextest/binaries-metadata.json"
                    metadata.parent.mkdir(parents=True)
                    binary = extraction / "target/release/deps/cross-chain-fixture"
                    binary.parent.mkdir(parents=True)
                    binary.write_text("fixture archived executable")
                    metadata.write_text(json.dumps({"rust-build-meta":{"target-directory":"/producer/target"},
                        "rust-binaries":{"eez-node-e2e::cross_chain":{"binary-path":"/producer/target/release/deps/cross-chain-fixture"}}}))
            elif command[:3] == ["cargo-nextest", "nextest", "run"]:
                if any(name in " ".join(command) for name in failures):
                    raise subprocess.CalledProcessError(100, command)
        with patch.object(native, "identity", return_value=self.identity), \
                patch.object(native, "check_nextest"), \
                patch.object(native, "output", return_value="glibc 2.39"), \
                patch.object(native.subprocess, "run", side_effect=extract) as run, \
                patch.dict(os.environ, CARGO_TARGET_DIR=str(self.target), EEZ_TEST_LOG_DIR=str(self.root / "logs"), EEZ_TEST_QUOTA_IMAGE="fixture-quota:exact"):
            try:
                native.run(self.root, suite, partition)
            finally:
                self.calls = run.call_args_list
            return run.call_args

    def test_consumer_executes_nextest_directly_with_explicit_relocation(self):
        args = self.run_archive("embedded")
        command = args.args[0]
        self.assertEqual(command[:3], ["cargo-nextest", "nextest", "run"])
        self.assertIn("--binaries-metadata", command)
        self.assertIn("--target-dir-remap", command)
        self.assertIn("--workspace-remap", command)
        self.assertNotIn("cargo", command)
        self.assertEqual(args.kwargs["env"]["EEZ_TEST_PREBUILT"], "1")
        self.assertEqual(args.kwargs["env"]["EEZ_TEST_PROOF_SIGNER_BIN"],
                         str(self.target / "native-tests/target/release/eez-runtime/eez-proof-signer"))
        self.assertEqual(args.kwargs["env"]["EEZ_TEST_RUNTIME_BIN_DIR"],
                         str(self.target / "native-tests/target/release/eez-runtime"))
        self.assertIn("--test-threads=1", command)
        self.assertEqual(command[command.index("--profile") + 1], "ci")

    def test_partition_retains_suite_filter_identity_and_serial_execution(self):
        for suite in ("embedded", "cross-chain"):
            for shard in range(1, 5):
                with self.subTest(suite=suite, shard=shard):
                    if self.target.exists():
                        shutil.rmtree(self.target)
                    partition = f"slice:{shard}/4"
                    self.run_archive(suite, partition=partition)
                    args = next(call for call in self.calls
                                if call.args[0][:3] == ["cargo-nextest", "nextest", "run"])
                    quota = [call for call in self.calls if call.args[0][0] == sys.executable]
                    self.assertEqual(len(quota), int(suite == "cross-chain" and shard == 1))
                    command = args.args[0]
                    self.assertEqual(command[command.index("--partition") + 1], partition)
                    self.assertEqual(command[command.index("-E") + 1], native.SUITES[suite])
                    self.assertIn("--test-threads=1", command)
                    self.assertIn("--no-tests=fail", command)
                    self.assertEqual(args.kwargs["env"]["EEZ_TEST_PREBUILT"], "1")

    def test_first_partition_quota_ownership_uses_numeric_index(self):
        for partition in ("hash:1/4", "hash:01/4", "slice:01/4"):
            with self.subTest(partition=partition):
                if self.target.exists():
                    shutil.rmtree(self.target)
                self.run_archive("cross-chain", partition=partition)
                quota = [call for call in self.calls if call.args[0][0] == sys.executable]
                self.assertEqual(len(quota), 1)

    def test_partition_cannot_certify_partial_unit_validation(self):
        with self.assertRaisesRegex(ValueError, "unit validation must run in full"):
            self.run_archive(partition="slice:1/4")
        self.assertEqual(self.calls, [])
        self.assertFalse((self.root / "unit-passed.json").exists())

    def test_partition_failure_is_propagated(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_archive("embedded", failures=("native-tests",), partition="slice:1/4")

    def test_failed_cross_chain_partition_does_not_run_quota(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_archive("cross-chain", failures=("native-tests",), partition="slice:1/4")
        self.assertFalse(any(call.args[0][0] == sys.executable for call in self.calls))

    def test_quota_failure_fails_its_cross_chain_partition(self):
        with patch.object(native, "run_native_quota", side_effect=subprocess.CalledProcessError(1, ["quota"])) as quota:
            with self.assertRaises(subprocess.CalledProcessError):
                self.run_archive("cross-chain", partition="slice:1/4")
            quota.assert_called_once()

    def test_wrong_revision_profile_lock_or_toolchain_fails_closed(self):
        for key in self.identity:
            with self.subTest(key=key):
                saved = self.manifest[key]
                self.manifest[key] = "wrong"
                with self.assertRaisesRegex(ValueError, key):
                    self.run_archive()
                self.manifest[key] = saved

    def test_corrupt_archive_fails_before_running_anything(self):
        self.archive.write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "checksum"):
            self.run_archive()

    def test_missing_unknown_or_corrupt_auxiliary_archive_fails_closed(self):
        hashes = self.manifest["archives_sha256"]
        for invalid in (None, {}, {"native-tests.tar.zst": hashes["native-tests.tar.zst"]},
                        dict(hashes, **{"../unexpected.tar.zst": "a" * 64})):
            with self.subTest(hashes=invalid):
                self.manifest["archives_sha256"] = invalid
                with self.assertRaisesRegex(ValueError, "inventory"):
                    self.run_archive()
        self.manifest["archives_sha256"] = hashes
        (self.root / "composer-unit-tests.tar.zst").write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "checksum"):
            self.run_archive("embedded")
        self.assertEqual(self.calls, [])

    def test_unit_suite_runs_both_archives_with_distinct_extractions(self):
        self.run_archive()
        commands = [call.args[0] for call in self.calls]
        extracts = [c[c.index("--extract-to") + 1] for c in commands if c[2] == "list"]
        self.assertEqual(extracts, [str(self.target / "native-tests"), str(self.target / "composer-unit-tests")])
        runs = [c for c in commands if c[2] == "run"]
        self.assertEqual(len(runs), 2)
        self.assertTrue(all("--no-fail-fast" in c for c in runs))

    def test_unit_failures_remain_failing_and_do_not_skip_other_archive(self):
        for name in ("native-tests", "composer-unit-tests"):
            with self.subTest(archive=name):
                if self.target.exists():
                    shutil.rmtree(self.target)
                with self.assertRaises(subprocess.CalledProcessError):
                    self.run_archive(failures=(name,))
                runs = [call.args[0] for call in self.calls if call.args[0][2] == "run"]
                self.assertEqual(len(runs), 2)

    def test_e2e_suites_do_not_run_composer_unit_tests(self):
        for suite in ("embedded", "cross-chain"):
            with self.subTest(suite=suite):
                if self.target.exists():
                    shutil.rmtree(self.target)
                self.run_archive(suite)
                self.assertEqual(len(self.calls), 3 if suite == "cross-chain" else 2)
                self.assertTrue(all("composer-unit-tests" not in " ".join(call.args[0]) for call in self.calls))

    def test_cross_chain_requires_physical_quota_using_verified_archived_inputs(self):
        self.run_archive("cross-chain")
        command = self.calls[-1].args[0]
        self.assertEqual(command[0], sys.executable)
        self.assertTrue(command[1].endswith("observability/tests/smoke-spool-quota.py"))
        self.assertEqual(Path(command[command.index("--native-test")+1]),
                         self.target / "native-tests/target/release/deps/cross-chain-fixture")
        self.assertEqual(Path(command[command.index("--runtime-bin")+1]),
                         self.target / "native-tests/target/release/eez-runtime")
        self.assertEqual(command[command.index("--image")+1], "fixture-quota:exact")
        self.assertTrue(self.calls[-1].kwargs["check"])
        self.assertNotIn("cargo", command)
        extracted = self.target / "native-tests/target"
        with patch.object(native.subprocess, "run") as invoked:
            with self.assertRaisesRegex(ValueError, "must be prepared"):
                native.run_native_quota(extracted, self.frozen, {})
            (extracted / "release/deps/cross-chain-fixture").unlink()
            with self.assertRaisesRegex(ValueError, "executable.*missing"):
                native.run_native_quota(extracted, self.frozen, {"EEZ_TEST_QUOTA_IMAGE":"fixture"})
            invoked.assert_not_called()

    def test_incompatible_glibc_or_native_cpu_flags_are_rejected(self):
        self.manifest["glibc"] = "glibc 99.1"
        with self.assertRaisesRegex(ValueError, "glibc"):
            self.run_archive()
        self.manifest["glibc"] = "glibc 2.39"
        self.manifest["rustflags"] = "-C target-cpu=native"
        with self.assertRaisesRegex(ValueError, "rustflags"):
            self.run_archive()

    def test_modified_or_missing_shipping_binary_cannot_run_tests(self):
        (self.frozen / "eez-composer").write_text("test-feature binary")
        with self.assertRaisesRegex(ValueError, "differs"):
            self.run_archive()

    def test_unsealed_archive_is_rejected(self):
        del self.manifest["runtime_archive_sha256"]
        with self.assertRaisesRegex(ValueError, "not sealed"):
            self.run_archive()

    def test_sealing_verifies_shipping_bytes_and_needs_no_nextest_or_compiler(self):
        runtime_archive = self.root / "runtime.tar.gz"
        with tarfile.open(runtime_archive, "w:gz") as archive:
            for name in native.RUNTIME_BINARIES:
                archive.add(self.frozen / name, arcname=f"bin/{name}")
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))
        with patch.object(native, "build_contract", return_value=self.identity), \
                patch.object(native, "identity", return_value=self.identity), \
                patch.object(native.subprocess, "run") as run:
            native.seal(self.root, runtime_archive)
            run.assert_not_called()
            sealed = json.loads((self.root / "manifest.json").read_text())
            self.assertEqual(sealed["runtime_archive_sha256"], native.digest(runtime_archive))
            sealed["runtime_sha256"]["eez-composer"] = "e" * 64
            (self.root / "manifest.json").write_text(json.dumps(sealed))
            with self.assertRaisesRegex(ValueError, "does not match packaged"):
                native.seal(self.root, runtime_archive)

    def test_existing_target_is_never_deleted_or_overwritten(self):
        self.target.mkdir()
        (self.target / "keep").write_text("active build")
        with self.assertRaisesRegex(ValueError, "empty job-local"):
            self.run_archive()
        self.assertEqual((self.target / "keep").read_text(), "active build")

    def export_fixture(self):
        self.run_archive()
        source = self.target / "native-tests/target"
        (source / "nextest").mkdir()
        binaries = {}
        for key in sorted(native.E2E_BINARIES | {"eez-node", "eez-protocol::rolling_hash_vectors"}):
            relative = f"release/deps/{key.replace('::', '-')}-123"
            path = source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("executable")
            binaries[key] = {"binary-path": f"/build/target/{relative}"}
        for relative in ("release/eez-composer", "release/test-helper", "release/deps/libfoo.so",
                         "release/deps/libfoo.rlib", "release/build/foo/out/libfoo.a",
                         "release/build/foo/out/code.o", "nextest/libdirs/host/libstd.so"):
            path = source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(relative)
        metadata = {"rust-binaries": binaries, "rust-build-meta": {
            "target-directory": "/build/target", "non-test-binaries": {"node": [
                {"name": "eez-composer", "kind": "bin-exe", "path": "release/eez-composer"},
                {"name": "test-helper", "kind": "bin-exe", "path": "release/test-helper"},
                {"name": "foo", "kind": "dylib", "path": "release/deps/libfoo.so"},
                {"name": "foo", "kind": "dylib", "path": "release/deps/libfoo.rlib"}]}}}
        (source / "nextest/binaries-metadata.json").write_text(json.dumps(metadata))
        (source / "nextest/cargo-metadata.json").write_text("{}")
        destination = self.root / "lean"
        with patch.object(native, "identity", return_value=self.identity), \
                patch.dict(os.environ, CARGO_TARGET_DIR=str(self.target)):
            native.export_e2e(self.root, destination)
        return destination, json.loads((destination / "manifest.json").read_text())

    def test_export_keeps_execution_inputs_not_units_or_link_inputs(self):
        destination, manifest = self.export_fixture()
        native.verify_e2e(destination, manifest)
        self.assertNotIn("archives_sha256", manifest)
        self.assertTrue(manifest["e2e_only"])
        self.assertFalse((destination / "target/release/eez-composer").exists())
        self.assertFalse((destination / "target/release/deps/eez-node-123").exists())
        self.assertFalse((destination / "target/release/deps/eez-protocol-rolling_hash_vectors-123").exists())
        for suffix in (".a", ".o", ".rlib"):
            self.assertFalse(list(destination.rglob(f"*{suffix}")))
        for relative in ("release/test-helper", "release/deps/libfoo.so", "nextest/libdirs/host/libstd.so"):
            self.assertTrue((destination / "target" / relative).is_file())
        self.assertEqual(native.runtime_hashes(destination / "target/release/eez-runtime"), self.manifest["runtime_sha256"])

    def test_lean_payload_detects_tampering_and_incomplete_inventory(self):
        destination, manifest = self.export_fixture()
        for relative in ("target/nextest/binaries-metadata.json", "target/nextest/cargo-metadata.json",
                         "target/release/deps/eez-node-e2e-e2e-123", "target/release/test-helper",
                         "target/release/eez-runtime/eez-composer"):
            with self.subTest(relative=relative):
                digest = manifest["files_sha256"].pop(relative)
                with self.assertRaisesRegex(ValueError, "inventory"):
                    native.verify_e2e(destination, manifest)
                manifest["files_sha256"][relative] = digest
        path = destination / "target/release/deps/eez-node-e2e-e2e-123"
        path.write_text("modified")
        with self.assertRaisesRegex(ValueError, "checksum"):
            native.verify_e2e(destination, manifest)

    def test_lean_payload_cannot_export_without_successful_units(self):
        self.run_archive()
        (self.root / "unit-passed.json").unlink()
        with self.assertRaises(FileNotFoundError):
            native.export_e2e(self.root, self.root / "lean")
        (self.root / "unit-passed.json").write_text("{}")
        with self.assertRaisesRegex(ValueError, "unit tests must pass"):
            native.export_e2e(self.root, self.root / "lean")

    def test_test_inventory_and_serialism_are_preserved(self):
        self.assertEqual(native.TARGETS, ["e2e", "cross_chain", "derivable_operations", "chained_interstate", "resumption", "partial_consumption",
                                         "settlement_policy", "settlement_policy_gas", "settlement_policy_cost", "kurtosis_policy_replay", "dynamic_admission",
                                         "queue_liveness", "queue_reorg",
                                         "public_inputs_hash_vectors", "rolling_hash_vectors",
                                         "ethereum_kzg_compatibility"])
        self.assertIn("kind(lib) | kind(bin)", native.SUITES["unit"])
        self.assertNotIn("--test-threads=1", self.run_archive().args[0])

    def test_every_standalone_integration_target_is_selected(self):
        # Cargo auto-discovers these test entrypoints. A new target must be
        # deliberately classified, not silently omitted from executable CI.
        paths = list(REPO.glob("crates/*/tests/*.rs")) + list(REPO.glob("crates/*/tests/*/main.rs"))
        discovered = {path.parent.name if path.name == "main.rs" else path.stem for path in paths}
        self.assertEqual(discovered, set(native.TARGETS))
        self.assertIn("package(=eez-blob-codec) & binary(=ethereum_kzg_compatibility)", native.SUITES["unit"])

    def test_every_archived_node_target_has_exactly_one_execution_suite(self):
        for binary in native.E2E_BINARIES:
            name = binary.split("::", 1)[1]
            selected = [suite for suite, expression in native.SUITES.items()
                        if f"binary(={name})" in expression]
            self.assertEqual(len(selected), 1, f"{binary}: {selected}")
            self.assertIn(name, native.TARGETS)
        self.assertIn("binary(=settlement_policy_gas)", native.SUITES["cross-chain"])
        self.assertIn("binary(=derivable_operations)", native.SUITES["cross-chain"])
        self.assertIn("binary(=dynamic_admission)", native.SUITES["cross-chain"])
        self.assertIn("binary(=queue_liveness)", native.SUITES["cross-chain"])
        self.assertIn("binary(=queue_reorg)", native.SUITES["cross-chain"])
        self.assertIn("binary(=settlement_policy)", native.SUITES["embedded"])

    def test_test_features_are_isolated_without_losing_binary_unit_tests(self):
        main = native.ARCHIVES["native-tests.tar.zst"]
        self.assertIn("--workspace", main)
        self.assertEqual(main[main.index("--exclude") + 1], "eez-composer")
        self.assertIn("--lib", main)
        self.assertNotIn("--bins", main)
        self.assertEqual(main[main.index("--bin") + 1], "eez-proof-signer")
        self.assertEqual([main[i + 1] for i, arg in enumerate(main) if arg == "--test"], native.TARGETS)
        self.assertEqual(native.ARCHIVES["composer-unit-tests.tar.zst"], ["--package", "eez-composer", "--lib"])
        self.assertEqual(list(native.ARCHIVES), ["native-tests.tar.zst", "composer-unit-tests.tar.zst"])

    def test_omitted_node_binary_test_programs_are_only_empty_entrypoint_shims(self):
        # If tests or extra logic are added here, reconsider the CI selection.
        for package, name, library, entrypoint in (
                ("eez-node", "eez-composer", "eez_node", "run_composer"),
                ("eez-follower", "eez-follower", "eez_follower", "run"),
                ("eez-node-e2e", "eez-composer", "eez_node", "run_composer"),
                ("eez-node-e2e", "eez-follower", "eez_follower", "run")):
            source = (REPO / f"crates/{package}/src/bin/{name}.rs").read_text()
            code = "\n".join(line for line in source.splitlines() if not line.startswith("//!"))
            self.assertEqual(" ".join(code.split()), f"fn main() -> eyre::Result<()> {{ {library}::{entrypoint}() }}")

    def test_ci_toolchains_are_aligned(self):
        workflow = (REPO / ".github/workflows/ci.yml").read_text()
        dockerfile = (REPO / "Dockerfile.runtime").read_text()
        self.assertIn(f'toolchain: "{native.RUST_VERSION}"', workflow)
        self.assertIn(f'ARG RUST_IMAGE=rust:{native.RUST_VERSION}-bookworm', dockerfile)
        installer = (REPO / "scripts/ci/install-nextest.sh").read_text()
        self.assertIn(f'https://get.nexte.st/{native.NEXTEST_VERSION}/linux', installer)
        self.assertIn('sha256sum --check', installer)
        self.assertNotIn('cargo install', installer)
        self.assertIn('install-nextest.sh /usr/local/bin', dockerfile)
        consumer = (REPO / ".github/actions/native-test-tools/action.yml").read_text()
        self.assertIn('bash scripts/ci/install-nextest.sh "$nextest_dir"', consumer)
        self.assertIn('echo "$nextest_dir" >> "$GITHUB_PATH"', consumer)

    def test_composites_receive_workflow_variables_through_inputs(self):
        # actionlint validates workflows, not composite manifests. The Actions
        # runner rejects vars/secrets contexts while loading composite steps.
        for path in (REPO / ".github/actions").rglob("action.yml"):
            for expression in re.findall(r"\$\{\{(.*?)\}\}", path.read_text(), re.DOTALL):
                self.assertNotRegex(expression, r"\b(?:vars|secrets)\s*[.\[]", str(path))
        action = (REPO / ".github/actions/compiler-cache/action.yml").read_text()
        self.assertIn('default: "true"', action)
        self.assertIn("EEZ_COMPILER_CACHE_ENABLED: ${{ inputs.enabled }}", action)
        for name in ("ci.yml", "build-runtime.yml"):
            workflow = (REPO / ".github/workflows" / name).read_text()
            self.assertEqual(workflow.count("enabled: ${{ vars.CI_COMPILER_CACHE != 'false' }}"), 1)

    def test_producer_rejects_flags_or_target_overrides_before_compiling(self):
        for override in ("CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_PROFILE_RELEASE_LTO"):
            with self.subTest(override=override), \
                    patch.object(native, "build_contract", return_value=self.identity), \
                    patch.object(native, "check_nextest"), \
                    patch.object(native, "output", return_value=f"rustc {native.RUST_VERSION}"), \
                    patch.object(native.subprocess, "run") as run, \
                    patch.dict(os.environ, {"CARGO_INCREMENTAL": "0", "RUSTFLAGS": native.RUSTFLAGS,
                                            "CARGO_ENCODED_RUSTFLAGS": "", "CARGO_BUILD_TARGET": "",
                                            override: "nonportable"}):
                with self.assertRaises(ValueError):
                    native.build(self.root / "producer", self.frozen)
                run.assert_not_called()

    def test_producer_reuses_release_target_with_composer_unit_archive_last(self):
        producer = self.root / "producer"
        self.target.mkdir()
        (self.target / "runtime-marker").write_text("existing Cargo objects")
        timing = self.target / "cargo-timings/cargo-timing.html"
        timing.parent.mkdir()
        timing.write_text("test timings")
        def archive(command, **_):
            self.assertIn("--release", command)
            self.assertIn("--locked", command)
            self.assertNotIn("build", command)
            self.assertTrue((self.target / "runtime-marker").exists())
            # Ordinary bin output may change, but never the frozen candidate.
            (self.target / "release/eez-composer").write_text("feature-unified binary")
            Path(command[command.index("--archive-file") + 1]).write_bytes(b"new archive")
        with patch.object(native, "build_contract", return_value=self.identity), \
                patch.object(native, "check_nextest"), \
                patch.object(native, "output", side_effect=[f"rustc {native.RUST_VERSION}", "glibc 2.36"]), \
                patch.object(native.subprocess, "run", side_effect=archive) as run, \
                patch.dict(os.environ, CARGO_TARGET_DIR=str(self.target), CARGO_INCREMENTAL="0",
                           RUSTFLAGS=native.RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS="", CARGO_BUILD_TARGET=""):
            native.build(producer, self.frozen)
        self.assertEqual(run.call_count, 2)
        self.assertEqual([call.args[0][2] for call in run.call_args_list], ["test-archive", "composer-unit-archive"])
        self.assertEqual((producer / "cargo-timing.html").read_text(), "test timings")
        self.assertEqual((producer / "cargo-timing-composer-unit.html").read_text(), "test timings")
        metadata = json.loads((producer / "manifest.json").read_text())
        self.assertEqual(set(metadata["archives_sha256"]), set(native.ARCHIVES))
        self.assertEqual(metadata["runtime_sha256"], native.runtime_hashes(self.frozen))
        self.assertEqual(native.runtime_hashes(self.target / "release/eez-runtime"), native.runtime_hashes(self.frozen))

    def test_workflows_do_not_compile_in_consumers_or_cache_targets(self):
        # Parse with PyYAML when available; the normal lint image has it.
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")
        jobs = yaml.safe_load((REPO / ".github/workflows/ci.yml").read_text())["jobs"]
        for job in ("embedded-e2e", "cross-chain-e2e"):
            self.assertEqual(jobs[job]["needs"], "build-runtime")
            self.assertNotIn("EEZ_TEST_LOG_DIR", jobs[job].get("env", {}))
            steps = jobs[job]["steps"]
            self.assertTrue(any(s.get("uses") == "./.github/actions/native-test-artifact" for s in steps))
            artifact_step = next(s for s in steps if s.get("uses") == "./.github/actions/native-test-artifact")
            self.assertEqual(artifact_step["with"]["payload-ref"], "${{ needs.build-runtime.outputs.tests-ref }}")
            commands = "\n".join(s.get("run", "") for s in steps)
            self.assertNotIn("cargo build", commands)
            self.assertNotIn("cargo nextest", commands)
            self.assertNotIn("update-eezl2-genesis", commands)
            self.assertNotIn("init-cargo-target.sh", commands)
        tools = (REPO / ".github/actions/native-test-tools/action.yml").read_text()
        consumer = (REPO / ".github/actions/native-test-artifact/action.yml").read_text()
        self.assertNotIn("init-cargo-target.sh", tools + consumer)
        self.assertNotIn("build-native-tests", jobs)
        self.assertEqual(jobs["kurtosis-e2e"]["needs"], "build-runtime")
        self.assertNotIn("unit-tests", jobs)
        producer = yaml.safe_load((REPO / ".github/workflows/build-runtime.yml").read_text())["jobs"]["build"]
        steps = producer["steps"]
        commands = "\n".join(s.get("run", "") for s in steps)
        self.assertEqual(commands.count("init-cargo-target.sh"), 1)
        self.assertLess(commands.index("init-cargo-target.sh"), commands.index('--suite unit'))
        self.assertIn('native-tests.py run "$EEZ_NATIVE_TEST_OUTPUT_DIR" --suite unit', commands)
        self.assertLess(commands.index('--suite unit'), commands.index('export-e2e'))
        self.assertLess(commands.index('export-e2e'), commands.index('ci-payload.sh publish'))
        for path in (REPO / ".github").rglob("*.yml"):
            document = yaml.safe_load(path.read_text())
            steps = document.get("runs", {}).get("steps", [])
            for job in document.get("jobs", {}).values():
                steps += job.get("steps", [])
            for step in steps:
                if step.get("uses", "").startswith("Swatinem/rust-cache@"):
                    self.assertEqual(step["with"]["cache-targets"], "false")
                    self.assertEqual(step["with"]["cache-on-failure"], "false")


@unittest.skipUnless(os.environ.get("EEZ_TEST_NATIVE_ARCHIVE") == "1",
                     "set EEZ_TEST_NATIVE_ARCHIVE=1 for real tiny-workspace relocation")
class NativeRelocationTest(unittest.TestCase):
    def test_real_producer_and_all_consumers_without_build_machine_or_compiler(self):
        with tempfile.TemporaryDirectory(prefix="eez-native-relocation.") as temporary:
            root = Path(temporary)
            repo = root / "build-machine"
            repo.mkdir()
            for path in ("scripts/ci/native-tests.py", "scripts/ci/with-compiler-cache.sh", ".config/nextest.toml"):
                dest = repo / path
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(REPO / path, dest)
            (repo / "Cargo.toml").write_text('[workspace]\nresolver="2"\nmembers=["crates/*"]\n'
                                           '[profile.release]\nlto="thin"\nstrip="symbols"\n')
            packages = ("eez-node", "eez-follower", "eez-node-e2e", "eez-proof-signer", "eez-protocol", "eez-composer", "eez-blob-codec", "eez-observer")
            for name in packages:
                package = repo / "crates" / name
                (package / "src").mkdir(parents=True)
                manifest = f'[package]\nname="{name}"\nversion="0.1.0"\nedition="2021"\n'
                source = '''
#[test] fn works() { assert_eq!(2+2,4); }
#[test] fn release_profile_still_runs_test_assertions() {
    assert!(!cfg!(debug_assertions));
    assert!(std::panic::catch_unwind(|| assert_eq!(std::hint::black_box(1), 2)).is_err());
}
'''
                # An offline equivalent of the real Tokio dev-feature fan-out.
                # Node binaries must reuse the production feature graph, while
                # composer's separately archived unit tests require test-util.
                if name == "eez-node":
                    manifest += '[dependencies]\neez-composer={path="../eez-composer"}\n'
                elif name == "eez-follower":
                    manifest += '[dependencies]\neez-composer={path="../eez-composer"}\n'
                elif name == "eez-node-e2e":
                    manifest += '[dependencies]\neez-node={path="../eez-node"}\neez-follower={path="../eez-follower"}\neez-composer={path="../eez-composer"}\n'
                    manifest += '[dev-dependencies]\nanyhow="1"\n'
                elif name == "eez-composer":
                    manifest += ('[dependencies]\neez-protocol={path="../eez-protocol"}\n'
                                 '[dev-dependencies]\neez-protocol={path="../eez-protocol",features=["test-util"]}\n')
                    source += ('pub fn test_clock() -> bool { eez_protocol::test_clock() }\n'
                               '#[test] fn paused_clock_is_enabled() { assert!(test_clock()); }\n')
                elif name == "eez-protocol":
                    manifest += '[features]\ntest-util=[]\n'
                    source += 'pub fn test_clock() -> bool { cfg!(feature="test-util") }\n'
                (package / "Cargo.toml").write_text(manifest)
                (package / "src/lib.rs").write_text(source)
                (package / "tests/fixtures").mkdir(parents=True)
                (package / "tests/fixtures/fixture.txt").write_text("relocated fixture")
            signer = repo / "crates/eez-proof-signer/src/main.rs"
            signer.write_text('fn main() { println!("signer"); }\n#[test] fn signer_is_tested() { assert_eq!(2+2,4); }\n')
            (repo / "crates/eez-observer/src/main.rs").write_text('fn main() { println!("release"); }\n')
            node = repo / "crates/eez-node-e2e"
            for package, binary in (("eez-node", "eez-composer"), ("eez-follower", "eez-follower"),
                                    ("eez-node-e2e", "eez-composer"), ("eez-node-e2e", "eez-follower")):
                directory = repo / f"crates/{package}/src/bin"
                directory.mkdir(exist_ok=True)
                (directory / f"{binary}.rs").write_text(
                    'fn main() { assert!(!eez_composer::test_clock()); println!("release"); }\n')
            # Exercise the integration test path-resolver code, not a copy of
            # the algorithm. The producer directory disappears before execution.
            common = (REPO / "crates/eez-testkit/src/lib.rs").read_text()
            manifest_fn = common.split("fn manifest_dir()", 1)[1].split("/// Stop dotenv", 1)[0]
            binary_type = common.split("pub enum NodeBinary", 1)[1].split("#[derive(Default)]\npub struct NodeConfig", 1)[0]
            test_source = 'use std::path::PathBuf;\nuse anyhow::{Context, Result, anyhow, bail, ensure};\nfn manifest_dir()' + manifest_fn + '\npub enum NodeBinary' + binary_type
            # The standalone enum copy needs its original derive attributes.
            test_source = test_source.replace("pub enum NodeBinary", "#[derive(Default)] pub enum NodeBinary")
            test_source += '''
#[test] fn fixture_is_relocated() {
    assert_eq!(std::fs::read_to_string(manifest_dir().join("tests/fixtures/fixture.txt")).unwrap(), "relocated fixture");
}
fn check_node(bin: NodeBinary) {
    let result = std::process::Command::new(bin.path().unwrap()).output().unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), "release");
}
#[test] fn composer_is_relocated() { check_node(NodeBinary::Composer); }
#[test] fn follower_is_relocated() { check_node(NodeBinary::Follower); }
#[test] fn signer_is_relocated() {
    let signer = std::env::var_os("EEZ_TEST_PROOF_SIGNER_BIN").unwrap();
    assert!(std::process::Command::new(signer).status().unwrap().success());
}
#[test] fn permanent_failure_is_not_retried() {
    if let Ok(path) = std::env::var("EEZ_FIXTURE_FAILURE_LOG") {
        use std::io::Write;
        let mut log = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        writeln!(log, "{}", file!()).unwrap();
        panic!("intentional fixture failure");
    }
}
'''
            node_targets = [target for target in native.TARGETS
                            if f"eez-node-e2e::{target}" in native.E2E_BINARIES]
            for target in node_targets:
                extra = "" if target != "cross_chain" else "\n#[test] #[ignore] fn dynamic_enospc_bidirectional_cross_chain_smoke() { fixture_is_relocated(); composer_is_relocated(); follower_is_relocated(); signer_is_relocated(); }\n"
                (node / f"tests/{target}.rs").write_text(test_source + extra)
            for target in (target for target in native.TARGETS if target not in node_targets):
                package = "eez-blob-codec" if target == "ethereum_kzg_compatibility" else "eez-protocol"
                (repo / f"crates/{package}/tests/{target}.rs").write_text('''
#[test] fn fixture_is_relocated() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    assert_eq!(std::fs::read_to_string(format!("{root}/tests/fixtures/fixture.txt")).unwrap(), "relocated fixture");
    std::fs::write(format!("{root}/compatibility.executed"), "passed").unwrap();
}
''')
            # This tiny test exercises required artifact handoff and ignored
            # executable selection, not physical ENOSPC. The real quota runner
            # separately verifies that behavior with actual native processes.
            quota = repo / "observability/tests/smoke-spool-quota.py"
            quota.parent.mkdir(parents=True)
            quota.write_text('''import argparse, json, os, pathlib, subprocess
parser=argparse.ArgumentParser()
for name in ('native-test','runtime-bin','image','report'): parser.add_argument('--'+name,required=True)
a=parser.parse_args()
assert a.image=='fixture-quota:not-run'
assert os.environ['EEZ_TEST_PREBUILT']=='1'
assert pathlib.Path(a.runtime_bin)==pathlib.Path(os.environ['EEZ_TEST_RUNTIME_BIN_DIR'])
env=dict(os.environ,CARGO_MANIFEST_DIR=str(pathlib.Path.cwd()/'crates/eez-node-e2e'))
r=subprocess.run([a.native_test,'dynamic_enospc_bidirectional_cross_chain_smoke','--ignored','--exact'],env=env,text=True,stdout=subprocess.PIPE,check=True)
assert '1 passed; 0 failed' in r.stdout
pathlib.Path('quota-relocation.executed').write_text('passed')
pathlib.Path(a.report).parent.mkdir(parents=True,exist_ok=True)
pathlib.Path(a.report).write_text(json.dumps({'scope':'archived handoff only','passed':True}))
''')
            protocol = repo / "eez-core-protocol"
            protocol.mkdir()
            (protocol / "fixture").touch()
            subprocess.run(["cargo", f"+{native.RUST_VERSION}", "generate-lockfile", "--offline"], cwd=repo, check=True)
            for directory in (protocol, repo):
                subprocess.run(["git", "init", "-q", str(directory)], check=True)
                subprocess.run(["git", "-C", str(directory), "add", "."], check=True, capture_output=True)
                subprocess.run(["git", "-C", str(directory), "-c", "user.name=CI Fixture",
                                "-c", "user.email=ci@example.invalid", "commit", "-qm", "fixture"], check=True)
            sha = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
            env = dict(os.environ, GITHUB_SHA=sha, RUSTUP_TOOLCHAIN=native.RUST_VERSION, CARGO_INCREMENTAL="0",
                       RUSTFLAGS=native.RUSTFLAGS, CARGO_TARGET_DIR=str(root / "producer-target"),
                       EEZ_COMPILER_CACHE="none", EEZ_BUILD_METRICS_DIR=str(root / "metrics"),
                       EEZ_TEST_LOG_DIR=str(root / "logs"), EEZ_TEST_QUOTA_IMAGE="fixture-quota:not-run")
            artifact = root / "artifact"
            subprocess.run(["cargo", "build", "--release", "--locked", "-p", "eez-node", "-p", "eez-follower", "-p", "eez-proof-signer", "-p", "eez-observer", "--bins"],
                           cwd=repo, env=env, check=True)
            frozen = root / "shipping"
            frozen.mkdir()
            for name in native.RUNTIME_BINARIES:
                shutil.copy2(root / f"producer-target/release/{name}", frozen / name)
            subprocess.run([sys.executable, "scripts/ci/native-tests.py", "build", str(artifact),
                            "--runtime-bin", str(frozen)], cwd=repo, env=env, check=True)
            # The harness wrappers may rebuild into the same target paths. The
            # producer preserves the exact shipping bytes before archiving them.
            for name in native.RUNTIME_BINARIES:
                self.assertEqual(subprocess.check_output([str(frozen / name)], text=True).strip(),
                                 "signer" if name == "eez-proof-signer" else "release")
            # Check actual Cargo metadata: signer unit tests retained, all node
            # library/E2E targets present, no empty node binary test programs.
            metadata = json.loads(subprocess.check_output([
                "tar", "--zstd", "-xOf", str(artifact / "native-tests.tar.zst"),
                "target/nextest/binaries-metadata.json"], text=True))["rust-binaries"]
            self.assertIn("eez-proof-signer::bin/eez-proof-signer", metadata)
            self.assertIn("eez-node", metadata)
            for name in node_targets:
                self.assertIn(f"eez-node-e2e::{name}", metadata)
            self.assertNotIn("eez-composer", metadata)
            self.assertFalse(any(key.startswith("eez-node::bin/") for key in metadata))
            composer_metadata = json.loads(subprocess.check_output([
                "tar", "--zstd", "-xOf", str(artifact / "composer-unit-tests.tar.zst"),
                "target/nextest/binaries-metadata.json"], text=True))["rust-binaries"]
            self.assertEqual(list(composer_metadata), ["eez-composer"])
            runtime_archive = root / "runtime.tar.gz"
            with tarfile.open(runtime_archive, "w:gz") as archive:
                for name in native.RUNTIME_BINARIES:
                    archive.add(frozen / name, arcname=f"bin/{name}")
            subprocess.run([sys.executable, "scripts/ci/native-tests.py", "seal", str(artifact),
                            "--runtime-archive", str(runtime_archive)], cwd=repo, env=env, check=True)
            consumer = root / "different-runner-checkout"
            shutil.copytree(repo, consumer)
            repo.rename(root / "producer-unavailable")
            (root / "producer-target").rename(root / "build-target-unavailable")
            tools = root / "no-compiler"
            tools.mkdir()
            for tool in ("cargo", "rustc"):
                path = tools / tool
                path.write_text('#!/bin/sh\necho "Unexpected compiler invocation" >&2\nexit 99\n')
                path.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + env["PATH"]
            lean = root / "e2e-only"
            for suite in native.SUITES:
                env["CARGO_TARGET_DIR"] = str(root / f"extract-{suite}")
                payload = artifact if suite == "unit" else lean
                command = [sys.executable, "scripts/ci/native-tests.py", "run", str(payload), "--suite", suite]
                result = subprocess.run(command, cwd=consumer, env=env, check=True, capture_output=True, text=True)
                if suite != "unit":
                    def passed(result):
                        return set(re.findall(r"PASS .*? (eez-node-e2e::\S+ \S+)", result.stderr))
                    complete = passed(result)
                    self.assertTrue(complete, result.stderr)
                    covered = set()
                    for shard in range(1, 5):
                        env["CARGO_TARGET_DIR"] = str(root / f"extract-{suite}-{shard}")
                        result = subprocess.run(command + ["--partition", f"slice:{shard}/4"],
                                                cwd=consumer, env=env, capture_output=True, text=True)
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                        selected = passed(result)
                        self.assertTrue(selected, result.stderr)
                        self.assertFalse(covered & selected, "a test ran in more than one partition")
                        covered |= selected
                    self.assertEqual(covered, complete, "partitioning changed the suite's coverage")
                    # An empty partition must not turn into a green validation lane.
                    env["CARGO_TARGET_DIR"] = str(root / f"extract-{suite}-empty")
                    empty = subprocess.run(command + ["--partition", "slice:100/100"],
                                           cwd=consumer, env=env, capture_output=True, text=True)
                    self.assertEqual(empty.returncode, 4, empty.stdout + empty.stderr)
                    if suite == "embedded":
                        env["CARGO_TARGET_DIR"] = str(root / "extract-permanent-failure")
                        failure_log = root / "permanent-failure.log"
                        failing = subprocess.run(command, cwd=consumer,
                                                 env=dict(env, EEZ_FIXTURE_FAILURE_LOG=str(failure_log)),
                                                 capture_output=True, text=True)
                        self.assertEqual(failing.returncode, 100, failing.stdout + failing.stderr)
                        attempts = failure_log.read_text().splitlines()
                        expected = sum(f"binary(={target})" in native.SUITES[suite] for target in node_targets)
                        self.assertEqual(len(attempts), expected, "CI must not rerun a failed test")
                        self.assertEqual(len(set(attempts)), expected, "all selected binaries must run")
                if suite == "cross-chain":
                    self.assertEqual((consumer / "quota-relocation.executed").read_text(), "passed")
                if suite == "unit":
                    self.assertEqual((consumer / "crates/eez-blob-codec/compatibility.executed").read_text(), "passed")
                    subprocess.run([sys.executable, "scripts/ci/native-tests.py", "export-e2e", str(artifact),
                                    "--output-dir", str(lean)], cwd=consumer, env=env, check=True)
                target = lean / "target/release"
                self.assertFalse((target / ".fingerprint").exists())
                self.assertEqual(list(target.glob("deps/*.rlib")), [])
                self.assertFalse((target / "eez-composer").exists())
                self.assertEqual(native.runtime_hashes(target / "eez-runtime"), native.runtime_hashes(frozen))


if __name__ == "__main__":
    unittest.main()
