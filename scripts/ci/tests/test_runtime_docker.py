"""Opt-in packaging integration test; real Docker, tiny Rust workspace, no GHCR writes."""
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
import unittest


REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("native_tests", REPO / "scripts/ci/native-tests.py")
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


def observer_compile_inputs():
    """Literal external include inputs used by the actual workspace unit archive."""
    inputs = set()
    for source in (REPO / "crates/eez-observer/src").rglob("*.rs"):
        for relative in re.findall(r'include_(?:str|bytes)!\(\s*"([^"]*observability/[^"]+)"', source.read_text()):
            inputs.add((source.parent / relative).resolve().relative_to(REPO).as_posix())
    if not inputs:
        raise AssertionError("observer archive must exercise real external include inputs")
    return sorted(inputs)


@unittest.skipUnless(os.environ.get("EEZ_TEST_DOCKER_RUNTIME") == "1",
                     "set EEZ_TEST_DOCKER_RUNTIME=1 to build real Docker fixture images")
class RuntimeDockerTest(unittest.TestCase):
    def test_observer_compile_inputs_survive_runtime_allowlist(self):
        with tempfile.TemporaryDirectory(prefix="eez-runtime-context.") as temporary:
            root = Path(temporary)
            context = root / "context"
            context.mkdir()
            inputs = observer_compile_inputs()
            for name in inputs:
                target = context / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(REPO / name, target)
            # Public dummy sentinels prove we did not broaden the Docker context
            # to arbitrary monitoring configuration or unrelated fixture files.
            for name in ("observability/operator-config.json", "observability/tests/fixtures/unrelated-private.json"):
                (context / name).write_text("fixture-only excluded sentinel")
            instructions = (REPO / "Dockerfile.runtime").read_text().replace("\\\n", " ").splitlines()
            copies = [line for line in instructions if line.startswith("COPY observability/")]
            self.assertTrue(copies)
            (context / "Dockerfile.runtime").write_text(
                "FROM scratch AS compile-inputs\n" + "\n".join(copies)
                + "\nFROM scratch AS allowed-context\nCOPY . /\n")
            ignore = context / "Dockerfile.runtime.dockerignore"
            rules = (REPO / "Dockerfile.runtime.dockerignore").read_text()
            command = ["docker", "buildx", "build", "--network=none", "--progress=plain",
                       "--file", str(context / "Dockerfile.runtime")]
            # Reproduce the prior omission using Docker's actual matcher,
            # rather than a Python approximation of dockerignore semantics.
            ignore.write_text("\n".join(line for line in rules.splitlines()
                                       if not line.startswith("!observability")) + "\n")
            missing = subprocess.run(command + ["--target", "compile-inputs", "--output",
                f"type=local,dest={root / 'missing'}", str(context)],
                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
            self.assertNotEqual(missing.returncode, 0, missing.stdout)
            self.assertIn("not found", missing.stdout)
            self.assertIn("observability/", missing.stdout)
            ignore.write_text(rules)
            for target in ("compile-inputs", "allowed-context"):
                destination = root / target
                built = subprocess.run(command + ["--target", target, "--output",
                    f"type=local,dest={destination}", str(context)],
                    text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
                self.assertEqual(built.returncode, 0, built.stdout)
                for name in inputs:
                    self.assertEqual((destination / name).read_bytes(), (REPO / name).read_bytes())
                self.assertFalse((destination / "observability/operator-config.json").exists())
                self.assertFalse((destination / "observability/tests/fixtures/unrelated-private.json").exists())

    def test_real_build_and_same_job_execution(self):
        with tempfile.TemporaryDirectory(prefix="eez-runtime-docker.") as temporary:
            root = Path(temporary)
            repo = root / "repo"
            repo.mkdir()
            for name in (
                "Dockerfile.runtime", "Dockerfile.runtime.dockerignore", ".dockerignore",
                "testing/kurtosis/Dockerfile.deploy", "testing/kurtosis/Dockerfile.deploy.dockerignore",
                "scripts/ci/build-runtime-artifacts.sh", "scripts/ci/prepare-runtime-artifacts.sh",
                "scripts/ci/install-sccache.sh", "scripts/ci/with-compiler-cache.sh",
                "scripts/ci/install-nextest.sh", "scripts/ci/native-tests.py", ".config/nextest.toml",
                "contracts/test/PostBatchGasPins.t.sol", *observer_compile_inputs(),
            ):
                path = repo / name
                path.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(REPO / name, path)

            packages = {
                "eez-node": {
                    "src/bin/eez-composer.rs": "eez-composer",
                    "examples/genesis_state_root.rs": "eez-genesis-state-root",
                    "examples/genesis_block_hash.rs": "eez-genesis-block-hash",
                },
                "eez-follower": {"src/bin/eez-follower.rs": "eez-follower"},
                "eez-node-e2e": {},
                "eez-proof-signer": {"src/main.rs": "eez-proof-signer"},
                "eez-protocol": {"examples/e2e-causal-verify.rs": "e2e-causal-verify"},
                "eez-l1": {"examples/verify_blob_evidence.rs": "verify_blob_evidence"},
                "eez-composer": {},
                "eez-blob-codec": {},
                "eez-observer": {"src/main.rs": "eez-observer"},
            }
            (repo / "Cargo.toml").write_text(
                '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n'
                '[profile.release]\nlto = "thin"\nstrip = true\ncodegen-units = 16\n'
            )
            (repo / "Cargo.lock").write_text("version = 4\n" + "".join(
                f'\n[[package]]\nname = "{name}"\nversion = "0.1.0"\n'
                for name in sorted(packages)
            ))
            for name, targets in packages.items():
                package = repo / "crates" / name
                (package / "src").mkdir(parents=True)
                (package / "Cargo.toml").write_text(
                    f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
                )
                (package / "src/lib.rs").write_text("#[test] fn unit_test() { assert_eq!(2+2,4); }\n")
                if name == "eez-composer":
                    # Match the real composer's compile-time dependency on a
                    # Solidity fixture outside crates/. Both the Docker build
                    # context and the builder stage must carry this input.
                    (package / "src/lib.rs").write_text(
                        '#[test] fn gas_pin_source_is_packaged() {\n'
                        '    let source = include_str!("../../../contracts/test/PostBatchGasPins.t.sol");\n'
                        '    assert!(source.contains("contract PostBatchGasPinsTest"));\n'
                        '}\n'
                    )
                if name == "eez-observer":
                    inputs = observer_compile_inputs()
                    (package / "src/lib.rs").write_text(
                        "const INPUTS: &[&str] = &[\n" + "".join(
                            f'    include_str!("../../../{path}"),\n' for path in inputs
                        ) + "];\n#[test] fn observer_external_inputs_are_packaged() {\n"
                        "    assert!(INPUTS.iter().all(|input| !input.is_empty()));\n}\n"
                    )
                for test in ({"eez-node-e2e": native.E2E_TARGETS,
                              "eez-protocol": ("public_inputs_hash_vectors", "rolling_hash_vectors"),
                              "eez-blob-codec": ("ethereum_kzg_compatibility",)}.get(name, ())):
                    (package / "tests").mkdir(exist_ok=True)
                    (package / f"tests/{test}.rs").write_text(
                        '#[test] fn release_test() { assert!(!cfg!(debug_assertions)); assert_eq!(2+2,4); }\n')
                for path, output in targets.items():
                    target = package / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(f'fn main() {{ println!("{output}"); }}\n')

            (repo / "contracts").mkdir(exist_ok=True)
            (repo / "contracts/fixture").touch()
            (repo / "genesis.json").write_text("{}")
            protocol = repo / "eez-core-protocol"
            protocol.mkdir()
            (protocol / "fixture").touch()
            for directory in (protocol, repo):
                subprocess.run(["git", "-C", str(directory), "init", "-q"], check=True)
                subprocess.run(["git", "-C", str(directory), "add", "."], check=True)
                subprocess.run(["git", "-C", str(directory), "-c", "user.name=CI Test",
                                "-c", "user.email=ci@example.invalid", "commit", "-qm", "fixture"], check=True)
            sha = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
            env = os.environ.copy()
            env.update({
                "GITHUB_REPOSITORY": "eez-ci-fixture/runtime-" + hashlib.sha256(root.name.encode()).hexdigest()[:16],
                "GITHUB_SHA": sha,
                "GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "1", "RUNNER_TEMP": str(root),
                "EEZ_RUNTIME_OUTPUT_DIR": str(root / "artifacts"), "EEZ_PUBLISH_IMAGES": "false",
                "EEZ_NATIVE_TEST_OUTPUT_DIR": str(root / "test-artifacts"),
                "EEZ_EXPECT_PUBLISHED": "false", "GITHUB_ENV": str(root / "github-env"),
                "GITHUB_PATH": str(root / "github-path"), "GITHUB_STEP_SUMMARY": str(root / "summary"),
                "DOCKER_CONFIG": str(root / "docker-config"),
            })
            Path(env["DOCKER_CONFIG"]).mkdir()
            # Exercise a specific Buildx release without replacing the host's
            # plugin or reading its Docker credentials.
            if plugin := os.environ.get("EEZ_TEST_BUILDX_PLUGIN"):
                plugins = Path(env["DOCKER_CONFIG"]) / "cli-plugins"
                plugins.mkdir()
                (plugins / "docker-buildx").symlink_to(Path(plugin).resolve(strict=True))
            images = [f"ghcr.io/{env['GITHUB_REPOSITORY']}-{role}:sha-{sha}-1-1"
                      for role in ("node", "proof-signer", "deploy")]
            try:
                subprocess.run(["bash", str(repo / "scripts/ci/build-runtime-artifacts.sh")], env=env, check=True)
                # Without publication, validation uses the producer's daemon.
                subprocess.run(["bash", str(repo / "scripts/ci/prepare-runtime-artifacts.sh"),
                                env["EEZ_RUNTIME_OUTPUT_DIR"]], env=env, check=True)
                binary_dir = Path(Path(env["GITHUB_PATH"]).read_text().strip())
                manifest = json.loads((root / "artifacts/manifest.json").read_text())
                test_manifest = json.loads((root / "test-artifacts/manifest.json").read_text())
                self.assertEqual(test_manifest["runtime_archive_sha256"], manifest["binaries_sha256"])
                self.assertEqual(test_manifest["profile"], "release")
                self.assertFalse((binary_dir / "eez-dev-node").exists(), "removed dev node must not ship")
                # Bookworm-built release tests must execute on the consumer host
                # without its Cargo/rustc. Install nextest outside CARGO_HOME.
                tools = root / "consumer-tools"
                subprocess.run(["bash", str(repo / "scripts/ci/install-nextest.sh"), str(tools)], check=True)
                for tool in ("cargo", "rustc"):
                    blocked = tools / tool
                    blocked.write_text('#!/bin/sh\necho "Unexpected compilation" >&2\nexit 99\n')
                    blocked.chmod(0o755)
                consumer_env = dict(env, PATH=str(tools) + os.pathsep + env["PATH"])
                for suite in ("unit", "embedded", "cross-chain"):
                    consumer_env["CARGO_TARGET_DIR"] = str(root / f"consumer-{suite}")
                    payload = root / ("test-artifacts" if suite == "unit" else "e2e-only")
                    subprocess.run([sys.executable, str(repo / "scripts/ci/native-tests.py"), "run",
                                    str(payload), "--suite", suite],
                                   env=consumer_env, check=True)
                    if suite == "unit":
                        subprocess.run([sys.executable, str(repo / "scripts/ci/native-tests.py"), "export-e2e",
                                        str(payload), "--output-dir", str(root / "e2e-only")],
                                       env=consumer_env, check=True)
                self.assertFalse(manifest["published"])
                for role, binaries in {
                    "node": ("eez-composer", "eez-follower", "eez-observer", "eez-genesis-state-root", "eez-genesis-block-hash"),
                    "signer": ("eez-proof-signer",),
                    "deploy": ("eez-genesis-state-root", "eez-genesis-block-hash"),
                }.items():
                    image = manifest["images"][role]["reference"]
                    container = subprocess.check_output(["docker", "create", image, "--help"], env=env, text=True).strip()
                    try:
                        for binary in binaries:
                            copied = root / "copied-binary"
                            subprocess.run(["docker", "cp", f"{container}:/usr/local/bin/{binary}", str(copied)], env=env, check=True)
                            self.assertEqual(hashlib.sha256(copied.read_bytes()).hexdigest(),
                                             hashlib.sha256((binary_dir / binary).read_bytes()).hexdigest())
                    finally:
                        # Never started: remove only the container created by this test.
                        subprocess.run(["docker", "rm", container], env=env, check=True)
                for binary in binary_dir.iterdir():
                    self.assertEqual(subprocess.check_output([str(binary)], text=True).strip(), binary.name)
            finally:
                # Unique fixture tags only; no shared images, caches or networks pruned.
                subprocess.run(["docker", "image", "rm", *images], env=env, check=False)


if __name__ == "__main__":
    unittest.main()
