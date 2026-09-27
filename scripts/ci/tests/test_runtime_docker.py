"""Opt-in packaging integration test; real Docker, tiny Rust workspace, no GHCR writes."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("native_tests", REPO / "scripts/ci/native-tests.py")
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


@unittest.skipUnless(os.environ.get("EEZ_TEST_DOCKER_RUNTIME") == "1",
                     "set EEZ_TEST_DOCKER_RUNTIME=1 to build real Docker fixture images")
class RuntimeDockerTest(unittest.TestCase):
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
                "contracts/test/PostBatchGasPins.t.sol",
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
            }
            (repo / "Cargo.toml").write_text(
                '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\nexclude = ["vendor/fixture-port"]\n'
                '[profile.release]\nlto = "thin"\nstrip = true\ncodegen-units = 16\n'
            )
            (repo / "Cargo.lock").write_text("version = 4\n" + "".join(
                f'\n[[package]]\nname = "{name}"\nversion = "0.1.0"\n'
                + ('dependencies = ["fixture-port"]\n' if name == 'eez-follower' else '')
                for name in sorted([*packages, 'fixture-port'])
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
                for test in ({"eez-node-e2e": native.E2E_TARGETS,
                              "eez-protocol": ("public_inputs_hash_vectors", "rolling_hash_vectors", "causal_evidence"),
                              "eez-blob-codec": ("ethereum_kzg_compatibility",)}.get(name, ())):
                    (package / "tests").mkdir(exist_ok=True)
                    (package / f"tests/{test}.rs").write_text(
                        '#[test] fn release_test() { assert!(!cfg!(debug_assertions)); assert_eq!(2+2,4); }\n')
                for path, output in targets.items():
                    target = package / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(f'fn main() {{ println!("{output}"); }}\n')

            port = repo / "vendor/fixture-port"
            (port / "src").mkdir(parents=True)
            (port / "Cargo.toml").write_text(
                '[package]\nname = "fixture-port"\nversion = "0.1.0"\nedition = "2021"\n')
            (port / "src/lib.rs").write_text('pub const NAME: &str = "eez-follower";\n')
            follower_package = repo / "crates/eez-follower"
            with (follower_package / "Cargo.toml").open("a") as manifest:
                manifest.write('[dependencies]\nfixture-port = { path = "../../vendor/fixture-port" }\n')
            (follower_package / "build-value.txt").write_text("")
            (follower_package / "obsolete.txt").write_text("must disappear after source deletion")
            (follower_package / "build.rs").write_text(
                # Distinct input per invocation: old fixture layers must not
                # satisfy assertions that deliberately require recompilation.
                f'// fixture {root.name}\n'
                'fn main() {\n'
                '  println!("cargo:rerun-if-changed=build-value.txt");\n'
                '  let value = std::fs::read_to_string("build-value.txt").unwrap();\n'
                '  if !value.is_empty() && std::path::Path::new("obsolete.txt").exists() {\n'
                '    panic!("DELETED_SOURCE_WAS_RESTORED");\n'
                '  }\n'
                '  println!("cargo:rustc-env=FIXTURE_MARKER={value}");\n'
                '}\n'
            )
            (follower_package / "src/bin/eez-follower.rs").write_text(
                'fn main() { println!("{}{}", fixture_port::NAME, env!("FIXTURE_MARKER")); }\n')

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
                "EEZ_CARGO_STATE_CACHE": "false", "EEZ_COMPILER_CACHE": "none", "EEZ_RUNTIME_CACHE": "false",
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
                    "node": ("eez-composer", "eez-follower", "eez-genesis-state-root", "eez-genesis-block-hash"),
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
                if os.environ.get("EEZ_TEST_DOCKER_FLAT_CACHE") == "1":
                    self.exercise_flat_cache(repo, root, env)
                if os.environ.get("EEZ_TEST_DOCKER_CARGO_STATE") == "1":
                    self.exercise_cargo_state(repo, root, env, consumer_env, images)
            except BaseException:
                evidence_root = REPO / "artifacts/docker-state-fixture-failures"
                evidence_root.mkdir(parents=True, exist_ok=True)
                evidence = Path(tempfile.mkdtemp(prefix="failure-", dir=evidence_root))
                for log in root.glob("*.log"):
                    shutil.copy2(log, evidence / log.name)
                print(f"Fixture failure logs: {evidence}", file=sys.stderr)
                raise
            finally:
                # Unique fixture tags only; no shared images, caches or networks pruned.
                subprocess.run(["docker", "image", "rm", *dict.fromkeys(images)], env=env, check=False)

    def exercise_flat_cache(self, repo, root, env):
        """mode=min must reuse outputs in an empty builder, not just this daemon."""
        cache = root / "flat-cache"
        args = ["--platform", "linux/amd64", "--provenance=false", "--progress", "plain",
                "-f", str(repo / "Dockerfile.runtime"), "--target", "binaries"]
        for name in ("RUST_IMAGE", "RUNTIME_IMAGE"):
            if name in env:
                args += ["--build-arg", f"{name}={env[name]}"]
        subprocess.run(["docker", "buildx", "build", "--builder", "default", *args,
                        "--cache-to", f"type=local,dest={cache},mode=min,compression=zstd,compression-level=1",
                        "--output", "type=cacheonly", str(repo)], env=env, check=True)
        index = json.loads((cache / "index.json").read_text())
        digest = index["manifests"][0]["digest"].removeprefix("sha256:")
        manifest = json.loads((cache / "blobs/sha256" / digest).read_text())
        self.assertTrue(manifest["layers"])
        # Already-exported base blobs can remain gzip: do not force a costly
        # conversion of reusable layers just to change their compression.
        self.assertIn("application/vnd.oci.image.layer.v1.tar+zstd",
                      {layer["mediaType"] for layer in manifest["layers"]})
        builder = "eez-flat-cache-" + hashlib.sha256(root.name.encode()).hexdigest()[:16]
        subprocess.run(["docker", "buildx", "create", "--name", builder, "--driver", "docker-container",
                        "--driver-opt", "image=moby/buildkit:v0.31.1,memory=2g,memory-swap=2g,cpu-period=100000,cpu-quota=200000"],
                       env=env, check=True)
        try:
            with (root / "flat-fresh-builder.log").open("w") as stream:
                result = subprocess.run(["docker", "buildx", "build", "--builder", builder, *args,
                                         "--cache-from", f"type=local,src={cache}",
                                         "--output", f"type=local,dest={root / 'flat-output'}", str(repo)],
                                        env=env, stdout=stream, stderr=subprocess.STDOUT)
            log = (root / "flat-fresh-builder.log").read_text()
            self.assertEqual(result.returncode, 0, log[-16000:])
            self.assertNotIn("Compiling ", log)
            self.assertNotIn("==> runtime: compiler cache=", log)
            self.assertTrue((root / "flat-output/bin/eez-follower").is_file())
            self.assertTrue((root / "flat-output/native-tests/native-tests.tar.zst").is_file())
            expected_bin = Path(Path(env["GITHUB_PATH"]).read_text().strip())
            actual_bin = root / "flat-output/bin"
            self.assertEqual({p.name: native.digest(p) for p in actual_bin.iterdir()},
                             {p.name: native.digest(p) for p in expected_bin.iterdir()})
            tests = json.loads((root / "test-artifacts/manifest.json").read_text())
            native.verify_archives(root / "flat-output/native-tests", tests)
        finally:
            # Only the unique builder and its own disposable volume are removed.
            subprocess.run(["docker", "buildx", "rm", builder], env=env, check=True)

    def exercise_cargo_state(self, repo, root, env, consumer_env, images):
        """Real producer/OCI transport, not a second implementation of the build."""
        env = dict(env, EEZ_CARGO_STATE_CACHE="true", EEZ_COMPILER_CACHE="none",
                   GITHUB_EVENT_NAME="push", GITHUB_REF="refs/heads/main", EEZ_DEFAULT_BRANCH="main",
                   GITHUB_OUTPUT=str(root / "state-output"))
        producer = ["bash", str(repo / "scripts/ci/build-runtime-artifacts.sh")]
        state = repo / ".ci-cargo-state"

        def build(name, *, prepare=False, expected_unit_failure=False):
            env.update(EEZ_RUNTIME_OUTPUT_DIR=str(root / name),
                       EEZ_NATIVE_TEST_OUTPUT_DIR=str(root / (name + "-tests")))
            if prepare:
                subprocess.run(producer + ["prepare-state"], env=env, check=True)
            log = root / (name + ".log")
            with log.open("w") as stream:
                result = subprocess.run(producer, env=env, stdout=stream, stderr=subprocess.STDOUT)
            build_log = log.read_text()
            self.assertEqual(result.returncode, 0, build_log[-16000:])
            manifest = json.loads((root / name / "manifest.json").read_text())
            images.extend(value["reference"] for value in manifest["images"].values())
            tests = json.loads((root / (name + "-tests") / "manifest.json").read_text())
            run_env = dict(consumer_env, GITHUB_SHA=env["GITHUB_SHA"],
                           CARGO_TARGET_DIR=str(root / (name + "-execution")))
            units = subprocess.run([sys.executable, str(repo / "scripts/ci/native-tests.py"), "run",
                                    env["EEZ_NATIVE_TEST_OUTPUT_DIR"], "--suite", "unit"],
                                   env=run_env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            (root / (name + "-units.log")).write_text(units.stdout)
            if expected_unit_failure:
                self.assertNotEqual(units.returncode, 0, units.stdout)
                self.assertIn("OCI_TEST_FAILURE_EXECUTED", units.stdout)
                self.assertFalse((root / (name + "-tests") / "unit-passed.json").exists())
            else:
                self.assertEqual(units.returncode, 0, units.stdout[-16000:])
                self.assertIn("passed", units.stdout)
            return build_log, tests

        cold, cold_manifest = build("state-cold", prepare=True)
        self.assertIn("Docker Cargo state: miss", cold)
        subprocess.run(producer + ["export-state"], env=env, check=True)
        self.assertTrue((state / "transport/index.json").is_file())
        warm, warm_manifest = build("state-warm")
        self.assertIn("Docker Cargo state: hit", warm)
        self.assertNotIn("Compiling ", warm)
        self.assertEqual(cold_manifest["runtime_sha256"], warm_manifest["runtime_sha256"])

        # Reach Docker's actual restore/check path, not just JSON validation.
        good_transport = root / "good-transport"
        (state / "transport").rename(good_transport)
        for damage in ("corrupt-layer", "future-time", "escaping-link"):
            transport = state / "transport"
            if damage == "corrupt-layer":
                shutil.copytree(good_transport, transport)
                index = json.loads((transport / "index.json").read_text())
                blobs = transport / "blobs/sha256"
                manifest_path = blobs / index["manifests"][0]["digest"].split(":")[1]
                manifest = json.loads(manifest_path.read_text())
                invalid = b"not a compressed OCI layer"
                digest = hashlib.sha256(invalid).hexdigest()
                (blobs / digest).write_bytes(invalid)
                manifest["layers"][0].update(digest="sha256:" + digest, size=len(invalid))
                # Force a new rootfs identity so a valid daemon-local snapshot
                # cannot satisfy this intentionally broken transport layer.
                config = json.loads((blobs / manifest["config"]["digest"].split(":")[1]).read_text())
                config["rootfs"]["diff_ids"][0] = "sha256:" + digest
                encoded = json.dumps(config).encode()
                digest = hashlib.sha256(encoded).hexdigest()
                (blobs / digest).write_bytes(encoded)
                manifest["config"].update(digest="sha256:" + digest, size=len(encoded))
                encoded = json.dumps(manifest).encode()
                digest = hashlib.sha256(encoded).hexdigest()
                (blobs / digest).write_bytes(encoded)
                index["manifests"][0].update(digest="sha256:" + digest, size=len(encoded))
                (transport / "index.json").write_text(json.dumps(index))
            else:
                digest = json.loads((good_transport / "index.json").read_text())["manifests"][0]["digest"]
                command = ("touch -d 2099-01-01 /build/target/future" if damage == "future-time"
                           else "ln -s /etc/passwd /build/target/escape")
                # A shell comes from the same pinned tools stage; the exported
                # seed is still flattened scratch data, not a runnable image.
                dockerfile = ("# syntax=docker/dockerfile:1\nFROM " + env.get("RUNTIME_IMAGE", "debian:bookworm-slim") +
                              " AS changed\nCOPY --from=seed / /\nRUN " + command +
                              "\nFROM scratch\nCOPY --from=changed /build /build\n"
                              "COPY --from=changed /cargo /cargo\n")
                compatibility = json.loads((state / "plan.json").read_text())["compatibility"]
                subprocess.run(["docker", "buildx", "build", "--builder", "default", "--provenance=false",
                                "--build-context", f"seed=oci-layout://{good_transport}@{digest}",
                                "--label", "io.eez.cargo.compatibility=" + compatibility,
                                "--resource", "cpu-quota=100000", "--resource", "memory=256m",
                                "--resource", "memory-swap=256m", "--network=none",
                                "--output", f"type=oci,tar=false,dest={transport}", "-"],
                               input=dockerfile, text=True, env=env, check=True)
            rejected, _ = build("state-" + damage)
            self.assertIn("Cargo state rejected before compilation", rejected)
            self.assertIn("Docker Cargo state: miss", rejected)
            transport.rename(root / (damage + "-transport"))
        good_transport.rename(state / "transport")

        # A genuinely new commit reuses the compatible prefix, not an exact key.
        def changed_commit(message):
            subprocess.run(["git", "-C", str(repo), "add", "crates"], check=True)
            subprocess.run(["git", "-C", str(repo), "-c", "user.name=CI Test",
                            "-c", "user.email=ci@example.invalid", "commit", "-qm", message], check=True)
            env["GITHUB_SHA"] = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
            saved = root / (message + "-state")
            state.rename(saved)
            subprocess.run(producer + ["prepare-state"], env=env, check=True)
            shutil.copytree(saved / "transport", state / "transport")

        follower_package = repo / "crates/eez-follower"
        (follower_package / "build-value.txt").write_text("-OCI_BUILD_INPUT_CHANGED")
        (follower_package / "obsolete.txt").unlink()
        changed_commit("changed-build-input")
        build_input, input_manifest = build("state-build-input")
        self.assertIn("Docker Cargo state: hit", build_input)
        self.assertIn("Compiling eez-follower", build_input)
        self.assertNotEqual(cold_manifest["runtime_sha256"]["eez-follower"],
                            input_manifest["runtime_sha256"]["eez-follower"])
        env["GITHUB_PATH"] = str(root / "build-input-path")
        subprocess.run(["bash", str(repo / "scripts/ci/prepare-runtime-artifacts.sh"),
                        env["EEZ_RUNTIME_OUTPUT_DIR"]], env=env, check=True)
        input_bin = Path(Path(env["GITHUB_PATH"]).read_text().strip()) / "eez-follower"
        self.assertEqual(subprocess.check_output([str(input_bin)], text=True).strip(),
                         "eez-follower-OCI_BUILD_INPUT_CHANGED")

        follower = follower_package / "src/bin/eez-follower.rs"
        follower.write_text('fn main() { println!("OCI_CHANGED_FOLLOWER"); }\n')
        changed_commit("changed-shipping")
        changed, changed_manifest = build("state-changed")
        self.assertIn("Docker Cargo state: hit", changed)
        self.assertIn("Compiling eez-follower", changed)
        self.assertNotEqual(cold_manifest["runtime_sha256"]["eez-follower"],
                            changed_manifest["runtime_sha256"]["eez-follower"])
        env["GITHUB_PATH"] = str(root / "changed-path")
        subprocess.run(["bash", str(repo / "scripts/ci/prepare-runtime-artifacts.sh"),
                        env["EEZ_RUNTIME_OUTPUT_DIR"]], env=env, check=True)
        binary_dir = Path(Path(env["GITHUB_PATH"]).read_text().strip())
        self.assertEqual(subprocess.check_output([str(binary_dir / "eez-follower")], text=True).strip(),
                         "OCI_CHANGED_FOLLOWER")

        composer = repo / "crates/eez-composer/src/lib.rs"
        composer.write_text(composer.read_text() + '\n#[test] fn new_failure() { panic!("OCI_TEST_FAILURE_EXECUTED"); }\n')
        changed_commit("changed-test")
        build("state-failing-test", expected_unit_failure=True)
        rejected = subprocess.run(producer + ["export-state"], env=env, capture_output=True)
        self.assertNotEqual(rejected.returncode, 0, "failed units must not permit a new seed")


if __name__ == "__main__":
    unittest.main()
