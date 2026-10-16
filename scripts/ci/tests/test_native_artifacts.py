"""Same-run test archives: identity checks, coverage, relocation and no rebuilds."""
from contextlib import redirect_stdout
import hashlib
import importlib.util
import io
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
        self.identity = {"schema": 4, "root_commit": "a" * 40, "protocol_commit": "b" * 40,
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

    def run_archive(self, suite="producer", failures=(), partition=None):
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))
        def extract(command, **_):
            if command[:3] == ["cargo-nextest", "nextest", "list"]:
                extraction = Path(command[command.index("--extract-to") + 1])
                self.assertTrue(extraction.is_dir(), "nextest requires an existing extraction directory")
                shutil.copytree(self.frozen, extraction / "target/release/eez-runtime")
            elif command[:3] == ["cargo-nextest", "nextest", "run"]:
                if any(name in " ".join(command) for name in failures):
                    raise subprocess.CalledProcessError(100, command)
        with patch.object(native, "identity", return_value=self.identity), \
                patch.object(native, "check_nextest"), \
                patch.object(native, "output", return_value="glibc 2.39"), \
                patch.object(native.subprocess, "run", side_effect=extract) as run, \
                patch.dict(os.environ, CARGO_TARGET_DIR=str(self.target)):
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
                    args = self.run_archive(suite, partition=partition)
                    command = args.args[0]
                    self.assertEqual(command[command.index("--partition") + 1], partition)
                    self.assertEqual(command[command.index("-E") + 1], native.SUITES[suite])
                    self.assertIn("--test-threads=1", command)
                    self.assertIn("--no-tests=fail", command)
                    self.assertEqual(args.kwargs["env"]["EEZ_TEST_PREBUILT"], "1")

    def test_partition_cannot_certify_partial_producer_validation(self):
        with self.assertRaisesRegex(ValueError, "producer validation must run in full"):
            self.run_archive(partition="slice:1/4")
        self.assertEqual(self.calls, [])
        self.assertFalse((self.root / native.PRODUCER_PASSED).exists())

    def test_partition_failure_is_propagated(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_archive("embedded", failures=("native-tests",), partition="slice:1/4")

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

    def test_missing_or_unknown_archive_inventory_fails_closed(self):
        hashes = self.manifest["archives_sha256"]
        for invalid in (None, {}, dict(hashes, **{"../unexpected.tar.zst": "a" * 64}),
                        dict(hashes, **{"composer-unit-tests.tar.zst": "a" * 64})):
            with self.subTest(hashes=invalid):
                self.manifest["archives_sha256"] = invalid
                with self.assertRaisesRegex(ValueError, "inventory"):
                    self.run_archive()
        self.assertEqual(self.calls, [])

    def test_producer_suite_runs_the_single_archive_without_fail_fast(self):
        self.run_archive()
        commands = [call.args[0] for call in self.calls]
        extracts = [c[c.index("--extract-to") + 1] for c in commands if c[2] == "list"]
        self.assertEqual(extracts, [str(self.target / "native-tests")])
        runs = [c for c in commands if c[2] == "run"]
        self.assertEqual(len(runs), 1)
        self.assertIn("--no-fail-fast", runs[0])
        self.assertIn("--no-tests=fail", runs[0])
        self.assertEqual(runs[0][runs[0].index("-E") + 1], native.SUITES["producer"])
        self.assertEqual(json.loads((self.root / native.PRODUCER_PASSED).read_text()), self.manifest)

    def test_producer_failures_remain_failing(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_archive(failures=("native-tests",))
        self.assertFalse((self.root / native.PRODUCER_PASSED).exists())

    def test_e2e_suites_select_only_node_e2e_modules(self):
        for suite in ("embedded", "cross-chain"):
            with self.subTest(suite=suite):
                if self.target.exists():
                    shutil.rmtree(self.target)
                args = self.run_archive(suite)
                self.assertEqual(len(self.calls), 2)
                expression = args.args[0][args.args[0].index("-E") + 1]
                self.assertTrue(expression.startswith(f"package(=eez-node-e2e) & binary(={native.E2E_TARGET}) & ("))
                self.assertNotIn("kind(lib)", expression)

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
                         "target/release/deps/eez-node-e2e-node_e2e-123", "target/release/test-helper",
                         "target/release/eez-runtime/eez-composer"):
            with self.subTest(relative=relative):
                digest = manifest["files_sha256"].pop(relative)
                with self.assertRaisesRegex(ValueError, "inventory"):
                    native.verify_e2e(destination, manifest)
                manifest["files_sha256"][relative] = digest
        path = destination / "target/release/deps/eez-node-e2e-node_e2e-123"
        path.write_text("modified")
        with self.assertRaisesRegex(ValueError, "checksum"):
            native.verify_e2e(destination, manifest)

    def test_lean_payload_cannot_export_without_successful_producer_suite(self):
        self.run_archive()
        (self.root / native.PRODUCER_PASSED).unlink()
        with self.assertRaises(FileNotFoundError):
            native.export_e2e(self.root, self.root / "lean")
        (self.root / native.PRODUCER_PASSED).write_text("{}")
        with self.assertRaisesRegex(ValueError, "producer release tests must pass"):
            native.export_e2e(self.root, self.root / "lean")
        # The previous marker name certifies nothing.
        (self.root / native.PRODUCER_PASSED).unlink()
        (self.root / "unit-passed.json").write_text(json.dumps(self.manifest))
        with self.assertRaises(FileNotFoundError):
            native.export_e2e(self.root, self.root / "lean")

    def test_test_inventory_and_serialism_are_preserved(self):
        self.assertEqual(native.TARGETS, ["node_e2e",
                                         "public_inputs_hash_vectors", "rolling_hash_vectors", "causal_evidence",
                                         "ethereum_kzg_compatibility", "issue35"])
        self.assertEqual(sorted(native.E2E_MODULES), sorted([
            "e2e", "cross_chain", "native_system", "derivable_operations", "chained_interstate", "partial_consumption",
            "settlement_policy", "settlement_policy_gas", "settlement_policy_cost", "kurtosis_policy_replay",
            "dynamic_admission", "queue_liveness", "queue_reorg", "fee_quotes", "safe_fee_quotes", "transaction_references"]))
        # Workspace lib/bin unit tests belong to the debug lane, not the archive.
        self.assertNotIn("kind(lib)", native.SUITES["producer"])
        self.assertNotIn("kind(bin)", native.SUITES["producer"])
        self.assertIn("binary(=causal_evidence)", native.SUITES["producer"])
        self.assertNotIn("--test-threads=1", self.run_archive().args[0])

    def test_every_standalone_integration_target_is_selected(self):
        # A new test target must be deliberately classified, not silently
        # omitted from executable CI. Cargo auto-discovers tests/*.rs unless a
        # package sets autotests = false and declares its [[test]] targets.
        import tomllib
        discovered = set()
        for manifest_path in REPO.glob("crates/*/Cargo.toml"):
            package = tomllib.loads(manifest_path.read_text())
            tests = manifest_path.parent / "tests"
            if package["package"].get("autotests", True):
                discovered |= {path.stem for path in tests.glob("*.rs")}
                discovered |= {path.parent.name for path in tests.glob("*/main.rs")}
            discovered |= {target["name"] for target in package.get("test", [])}
        self.assertEqual(discovered, set(native.TARGETS))
        self.assertIn("package(=eez-blob-codec) & binary(=ethereum_kzg_compatibility)", native.SUITES["producer"])
        self.assertIn("package(=eez-payload-codec) & binary(=issue35)", native.SUITES["producer"])
        for target, package in native.VECTOR_TARGETS.items():
            self.assertTrue((REPO / f"crates/{package}/tests/{target}.rs").is_file(), target)
            self.assertIn(f"(package(={package}) & binary(={target}))", native.SUITES["producer"])

    def test_every_node_e2e_module_has_exactly_one_execution_suite(self):
        # A module declared in main.rs but absent from both suites would compile
        # and never run in CI.
        import tomllib
        self.assertEqual(native.E2E_BINARIES, {f"eez-node-e2e::{native.E2E_TARGET}"})
        package = tomllib.loads((REPO / "crates/eez-node-e2e/Cargo.toml").read_text())
        self.assertIs(package["package"]["autotests"], False)
        self.assertEqual(package["test"], [{"name": native.E2E_TARGET, "path": "tests/main.rs"}])
        root = (REPO / "crates/eez-node-e2e/tests/main.rs").read_text()
        declared = re.findall(r"^mod (\w+);$", root, re.MULTILINE)
        self.assertEqual(sorted(declared), sorted(native.E2E_MODULES))
        self.assertEqual(len(native.E2E_MODULES), len(set(native.E2E_MODULES)))
        for module in native.E2E_MODULES:
            self.assertTrue((REPO / f"crates/eez-node-e2e/tests/{module}.rs").is_file(), module)
            selected = [suite for suite, expression in native.SUITES.items()
                        if f"test(/^{module}::/)" in expression]
            self.assertEqual(len(selected), 1, f"{module}: {selected}")
        self.assertIn("test(/^settlement_policy_gas::/)", native.SUITES["cross-chain"])
        self.assertIn("test(/^settlement_policy::/)", native.SUITES["embedded"])

    def test_release_tests_share_one_cargo_invocation_without_workspace_units(self):
        # One selection, one feature set: E2E, vectors and the eez-l1 example
        # together equal the shipping build's dependency features (dropping the
        # example re-resolved stateless/alloy and rebuilt the Reth stack).
        # Workspace lib/bin units would add tokio/test-util via dev-dependencies.
        self.assertEqual(list(native.ARCHIVES), ["native-tests.tar.zst"])
        main = native.ARCHIVES["native-tests.tar.zst"]
        for selector in ("--workspace", "--lib", "--bins", "--bin", "--tests", "--examples", "--all-targets"):
            self.assertNotIn(selector, main)
        packages = [main[i + 1] for i, arg in enumerate(main) if arg == "-p"]
        self.assertEqual(packages, list(native.RELEASE_PACKAGES))
        self.assertEqual(sorted(packages), sorted({"eez-node-e2e", "eez-l1", *native.VECTOR_TARGETS.values()}))
        self.assertEqual([main[i + 1] for i, arg in enumerate(main) if arg == "--example"], ["verify_blob_evidence"])
        self.assertEqual([main[i + 1] for i, arg in enumerate(main) if arg == "--test"], native.TARGETS)
        self.assertEqual(native.TARGETS[0], native.E2E_TARGET)
        self.assertEqual(native.TARGETS[1:], list(native.VECTOR_TARGETS))

    def debug_unit_command(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")
        job = yaml.safe_load((REPO / ".github/workflows/ci.yml").read_text())["jobs"]["debug-unit-tests"]
        step = next(s for s in job["steps"] if "cargo nextest run" in s.get("run", ""))
        return job, step, step["run"].split()

    def test_debug_lane_and_producer_suite_cover_every_former_unit_test(self):
        # Former unit filter: kind(lib) | kind(bin) | verify_blob_evidence | vectors.
        # The debug lane owns every workspace lib/bin target; the producer the rest.
        _, step, command = self.debug_unit_command()
        for argument in ("--workspace", "--lib", "--bins", "--locked", "--no-fail-fast", "--no-tests=fail"):
            self.assertIn(argument, command)
        for narrowing in ("--exclude", "-p", "--package", "--bin", "--release", "--partition", "-E"):
            self.assertNotIn(narrowing, command)
        self.assertEqual(command[command.index("--profile") + 1], "ci")
        self.assertEqual(step["env"]["EEZ_TEST_PREBUILT"], "1")
        self.assertIn("(package(=eez-l1) & binary(=verify_blob_evidence))", native.SUITES["producer"])
        for target, package in native.VECTOR_TARGETS.items():
            self.assertIn(f"(package(={package}) & binary(={target}))", native.SUITES["producer"])

    def test_debug_lane_runs_in_parallel_and_gates_promotion(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")
        jobs = yaml.safe_load((REPO / ".github/workflows/ci.yml").read_text())["jobs"]
        job, _, _ = self.debug_unit_command()
        self.assertNotIn("needs", job)
        self.assertNotIn("continue-on-error", job)
        self.assertIn("debug-unit-tests", jobs["ci-result"]["needs"])
        steps = job["steps"]
        self.assertTrue(any(s.get("uses") == "./.github/actions/native-test-tools" for s in steps))
        self.assertFalse(any(s.get("uses") == "./.github/actions/native-test-artifact" for s in steps))
        commands = [s.get("run", "") for s in steps]
        self.assertEqual(commands.count("bash scripts/ci/init-cargo-target.sh"), 1)
        self.assertLess(commands.index("bash scripts/ci/init-cargo-target.sh"),
                        next(i for i, c in enumerate(commands) if "cargo nextest run" in c))
        self.assertIn("install-llvm.sh", "\n".join(commands))
        toolchain = next(s for s in steps if s.get("uses", "").startswith("dtolnay/rust-toolchain@"))
        self.assertEqual(toolchain["with"]["toolchain"], native.RUST_VERSION)

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
        # Lint and the debug unit lane in ci.yml; the producer in build-runtime.yml.
        for name, uses in (("ci.yml", 2), ("build-runtime.yml", 1)):
            workflow = (REPO / ".github/workflows" / name).read_text()
            self.assertEqual(workflow.count("enabled: ${{ vars.CI_COMPILER_CACHE != 'false' }}"), uses)
            self.assertEqual(workflow.count("uses: ./.github/actions/compiler-cache"), uses)

    def test_producer_rejects_flags_or_target_overrides_before_compiling(self):
        for override in ("CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_PROFILE_RELEASE_LTO"):
            with self.subTest(override=override), \
                    patch.object(native, "build_contract", return_value=self.identity), \
                    patch.object(native, "check_nextest"), \
                    patch.object(native, "output", return_value=f"rustc {native.RUST_VERSION}"), \
                    patch.object(native.subprocess, "Popen") as popen, \
                    patch.object(native.subprocess, "run") as run, \
                    patch.dict(os.environ, {"CARGO_INCREMENTAL": "0", "RUSTFLAGS": native.RUSTFLAGS,
                                            "CARGO_ENCODED_RUSTFLAGS": "", "CARGO_BUILD_TARGET": "",
                                            override: "nonportable"}):
                with self.assertRaises(ValueError):
                    native.build(self.root / "producer", self.frozen)
                run.assert_not_called()
                popen.assert_not_called()

    WORKSPACE = "path+file:///build/crates/eez-node#0.1.0"
    DEPENDENCY = "registry+https://github.com/rust-lang/crates.io-index#tokio@1.48.0"

    @staticmethod
    def artifact(package_id, fresh):
        return json.dumps({"reason": "compiler-artifact", "package_id": package_id, "fresh": fresh,
                           "target": {"name": "fixture"}}) + "\n"

    def producer_build(self, messages=(), returncode=0):
        """Run build() with a fake archive process that emits Cargo JSON lines."""
        producer = self.root / "producer"
        self.target.mkdir(exist_ok=True)
        (self.target / "runtime-marker").write_text("existing Cargo objects")
        timing = self.target / "cargo-timings/cargo-timing.html"
        timing.parent.mkdir(exist_ok=True)
        timing.write_text("test timings")
        commands = []
        test = self

        class Archive:
            def __init__(self, command, **kwargs):
                commands.append(command)
                test.assertIs(kwargs["stdout"], subprocess.PIPE)
                test.assertTrue((test.target / "runtime-marker").exists())
                # Ordinary bin output may change, but never the frozen candidate.
                (test.target / "release/eez-composer").write_text("feature-unified binary")
                Path(command[command.index("--archive-file") + 1]).write_bytes(b"new archive")
                self.stdout = iter(["==> test-archive: compiler cache=none\n", *messages])
                self.returncode = None

            def __enter__(self):
                return self

            def __exit__(self, *_):
                self.returncode = returncode

        with patch.object(native, "build_contract", return_value=self.identity), \
                patch.object(native, "check_nextest"), \
                patch.object(native, "workspace_members", return_value={self.WORKSPACE}), \
                patch.object(native, "output", side_effect=[f"rustc {native.RUST_VERSION}", "glibc 2.36"]), \
                patch.object(native.subprocess, "Popen", Archive), \
                patch.object(native.subprocess, "run") as run, \
                patch.dict(os.environ, CARGO_TARGET_DIR=str(self.target), CARGO_INCREMENTAL="0",
                           RUSTFLAGS=native.RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS="", CARGO_BUILD_TARGET=""):
            try:
                native.build(producer, self.frozen)
            finally:
                run.assert_not_called()
        return producer, commands

    def test_archive_guard_accepts_reused_dependencies_and_rebuilt_workspace_crates(self):
        producer, _ = self.producer_build([self.artifact(self.DEPENDENCY, True),
                                           self.artifact(self.WORKSPACE, False),
                                           json.dumps({"reason": "build-finished", "success": True}) + "\n"])
        self.assertTrue((producer / "manifest.json").is_file())

    def test_archive_guard_rejects_any_compiled_dependency(self):
        for name, message in (
                ("rebuilt", self.artifact(self.DEPENDENCY, False)),
                ("unknown freshness", json.dumps({"reason": "compiler-artifact", "package_id": self.DEPENDENCY}) + "\n"),
                ("unknown package", json.dumps({"reason": "compiler-artifact", "fresh": False}) + "\n")):
            with self.subTest(case=name):
                for directory in (self.target, self.root / "producer"):
                    shutil.rmtree(directory, ignore_errors=True)
                with self.assertRaisesRegex(ValueError, "compiled 1 non-workspace crate"):
                    self.producer_build([self.artifact(self.DEPENDENCY, True), message, message])
                self.assertFalse((self.root / "producer/manifest.json").exists())

    def test_archive_guard_reports_cargo_failure_first(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.producer_build([self.artifact(self.DEPENDENCY, False)], returncode=101)
        self.assertFalse((self.root / "producer/manifest.json").exists())

    def test_archive_guard_echoes_non_json_output(self):
        stream = io.StringIO()
        with redirect_stdout(stream):
            found = list(native.compiled_dependencies(["plain\n", "[1]\n", self.artifact(self.DEPENDENCY, False)],
                                                      {self.WORKSPACE}))
        self.assertEqual(found, [self.DEPENDENCY])
        self.assertEqual(stream.getvalue(), "plain\n[1]\n")

    def test_producer_reuses_release_target_in_one_archive_invocation(self):
        producer, commands = self.producer_build()
        self.assertEqual(len(commands), 1)
        command = commands[0]
        self.assertEqual(command[:3], ["bash", "scripts/ci/with-compiler-cache.sh", "test-archive"])
        self.assertIn("--release", command)
        self.assertIn("--locked", command)
        self.assertNotIn("build", command)
        self.assertEqual(command[command.index("--cargo-message-format") + 1], "json-render-diagnostics")
        self.assertEqual((producer / "cargo-timing.html").read_text(), "test timings")
        self.assertFalse((producer / "cargo-timing-composer-unit.html").exists())
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
        self.assertLess(commands.index("init-cargo-target.sh"), commands.index('--suite producer'))
        self.assertIn('native-tests.py run "$EEZ_NATIVE_TEST_OUTPUT_DIR" --suite producer', commands)
        self.assertLess(commands.index('--suite producer'), commands.index('export-e2e'))
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
            packages = ("eez-node", "eez-follower", "eez-node-e2e", "eez-proof-signer", "eez-protocol", "eez-composer", "eez-blob-codec", "eez-l1", "eez-payload-codec")
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
                # Composer's unit tests require test-util; they run in the debug
                # lane, so the release archive must resolve shipping features.
                # anyhow stands in for third-party crates shared by the shipping
                # build and the E2E dev-dependencies (the archive guard's subject).
                if name == "eez-node":
                    manifest += ('[dependencies]\neez-composer={path="../eez-composer"}\n'
                                 'anyhow={version="1",default-features=ANYHOW_DEFAULT_FEATURES}\n')
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
            example = repo / "crates/eez-l1/examples/verify_blob_evidence.rs"
            example.parent.mkdir()
            example.write_text('''
fn main() {}
#[test] fn prefix_regression_is_in_the_release_archive() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    std::fs::write(format!("{root}/prefix.executed"), "passed").unwrap();
}
''')
            signer = repo / "crates/eez-proof-signer/src/main.rs"
            signer.write_text('fn main() { println!("signer"); }\n#[test] fn signer_is_tested() { assert_eq!(2+2,4); }\n')
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
        writeln!(log, "{}", module_path!()).unwrap();
        panic!("intentional fixture failure");
    }
}
'''
            node_targets = [target for target in native.TARGETS
                            if f"eez-node-e2e::{target}" in native.E2E_BINARIES]
            self.assertEqual(node_targets, [native.E2E_TARGET])
            (node / f"tests/{native.E2E_TARGET}.rs").write_text("".join(
                f"mod {module} {{\n{test_source}\n}}\n" for module in native.E2E_MODULES))
            for target in (target for target in native.TARGETS if target not in node_targets):
                package = {"ethereum_kzg_compatibility": "eez-blob-codec", "issue35": "eez-payload-codec"}.get(target, "eez-protocol")
                (repo / f"crates/{package}/tests/{target}.rs").write_text('''
#[test] fn fixture_is_relocated() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    assert_eq!(std::fs::read_to_string(format!("{root}/tests/fixtures/fixture.txt")).unwrap(), "relocated fixture");
    std::fs::write(format!("{root}/compatibility.executed"), "passed").unwrap();
}
''')
            node_manifest = repo / "crates/eez-node/Cargo.toml"
            shipping_features = node_manifest.read_text()
            node_manifest.write_text(shipping_features.replace("ANYHOW_DEFAULT_FEATURES", "true"))
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
                       EEZ_COMPILER_CACHE="none", EEZ_BUILD_METRICS_DIR=str(root / "metrics"))
            artifact = root / "artifact"
            def shipping_build(name):
                subprocess.run(["cargo", "build", "--release", "--locked", "-p", "eez-node", "-p", "eez-follower",
                                "-p", "eez-proof-signer", "--bins"], cwd=repo, env=env, check=True)
                frozen = root / name
                frozen.mkdir()
                for binary in native.RUNTIME_BINARIES:
                    shutil.copy2(root / f"producer-target/release/{binary}", frozen / binary)
                return frozen

            # A test-only feature on a shared third-party crate must fail the
            # archive step instead of silently rebuilding it (the real guard).
            node_manifest.write_text(shipping_features.replace("ANYHOW_DEFAULT_FEATURES", "false"))
            divergent = shipping_build("shipping-divergent")
            rejected = subprocess.run([sys.executable, "scripts/ci/native-tests.py", "build", str(root / "rejected"),
                                       "--runtime-bin", str(divergent)], cwd=repo, env=env,
                                      capture_output=True, text=True)
            self.assertEqual(rejected.returncode, 1, rejected.stdout + rejected.stderr)
            self.assertIn("compiled 1 non-workspace crate", rejected.stderr)
            self.assertIn("anyhow@", rejected.stderr)
            self.assertFalse((root / "rejected/manifest.json").exists())
            shutil.rmtree(root / "producer-target/release/eez-runtime")
            node_manifest.write_text(shipping_features.replace("ANYHOW_DEFAULT_FEATURES", "true"))
            frozen = shipping_build("shipping")
            produced = subprocess.run([sys.executable, "scripts/ci/native-tests.py", "build", str(artifact),
                                       "--runtime-bin", str(frozen)], cwd=repo, env=env,
                                      capture_output=True, text=True)
            # The rejected run already compiled the workspace test targets here.
            self.assertEqual(produced.returncode, 0, produced.stdout + produced.stderr)
            self.assertNotIn("Compiling anyhow", produced.stderr)
            # The harness wrappers may rebuild into the same target paths. The
            # producer preserves the exact shipping bytes before archiving them.
            for name in native.RUNTIME_BINARIES:
                self.assertEqual(subprocess.check_output([str(frozen / name)], text=True).strip(),
                                 "signer" if name == "eez-proof-signer" else "release")
            # Check actual Cargo metadata: exactly the E2E target, the vectors
            # and the eez-l1 example; workspace lib/bin units are debug-lane only.
            metadata = json.loads(subprocess.check_output([
                "tar", "--zstd", "-xOf", str(artifact / "native-tests.tar.zst"),
                "target/nextest/binaries-metadata.json"], text=True))["rust-binaries"]
            self.assertEqual(set(metadata), {
                *(f"eez-node-e2e::{name}" for name in node_targets),
                *(f"{package}::{target}" for target, package in native.VECTOR_TARGETS.items()),
                "eez-l1::example/verify_blob_evidence"})
            self.assertEqual(sorted(path.name for path in artifact.glob("*.tar.zst")), ["native-tests.tar.zst"])
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
                payload = artifact if suite == "producer" else lean
                command = [sys.executable, "scripts/ci/native-tests.py", "run", str(payload), "--suite", suite]
                result = subprocess.run(command, cwd=consumer, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                if suite == "producer":
                    self.assertEqual((consumer / "crates/eez-l1/prefix.executed").read_text(), "passed")
                if suite != "producer":
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
                        expected = sum(f"test(/^{module}::/)" in native.SUITES[suite] for module in native.E2E_MODULES)
                        self.assertEqual(len(attempts), expected, "CI must not rerun a failed test")
                        self.assertEqual(len(set(attempts)), expected, "all selected modules must run")
                if suite == "producer":
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
