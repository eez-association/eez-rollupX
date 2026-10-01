"""Offline build/consume contract tests. Docker/Cargo are mocked, never invoked."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
BINARIES = (
    "eez-composer", "eez-follower", "eez-proof-signer",
    "eez-genesis-state-root", "eez-genesis-block-hash",
    "e2e-causal-verify", "verify_blob_evidence",
)
MOCK_TOOL = r'''#!/usr/bin/env python3
import hashlib, json, os, sys
from pathlib import Path

args = sys.argv[1:]
tool = Path(sys.argv[0]).name
with open(os.environ["MOCK_LOG"], "a") as stream:
    stream.write(json.dumps([tool] + args) + "\n")
def option(name):
    return args[args.index(name) + 1]
def role(image):
    return 1 if "-proof-signer" in image else 2 if "-deploy" in image else 0
def oci_layout(output, compatibility):
    blobs = output / "blobs/sha256"
    blobs.mkdir(parents=True)
    def blob(value):
        data = json.dumps(value).encode()
        digest = hashlib.sha256(data).hexdigest()
        (blobs / digest).write_bytes(data)
        return {"digest": "sha256:" + digest, "size": len(data)}
    config = blob({"os": "linux", "architecture": "amd64",
                   "config": {"Labels": {"io.eez.cargo.compatibility": compatibility}}})
    manifest = blob({"schemaVersion": 2, "config": config, "layers": []})
    (output / "index.json").write_text(json.dumps({"schemaVersion": 2, "manifests": [manifest]}))
    (output / "oci-layout").write_text('{"imageLayoutVersion":"1.0.0"}')
if tool == "df":
    print("Filesystem 1-blocks Used Available Use% Mounted")
    print("fixture 100000000000 0 " + os.environ.get("MOCK_FREE_DISK", "90000000000") + " 0% /")
elif tool == "cargo":
    binary = Path(os.environ["CARGO_TARGET_DIR"]) / "debug/examples" / option("--example")
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_text("#!/bin/sh\nexit 0\n")
    binary.chmod(0o755)
elif tool == "kurtosis":
    # Stop at the launch boundary: no services or ports are needed in this test.
    assert args[0] == "run"
    sys.exit(23)
elif args[:2] == ["buildx", "inspect"]:
    print("Name: default\nDriver: " + os.environ.get("MOCK_DRIVER", "docker"))
elif args[:3] == ["buildx", "imagetools", "inspect"]:
    if args[3] == os.environ.get("MOCK_MISSING_CACHE_REF"):
        sys.exit(1)
    print("sha256:" + "d" * 64)
elif args[0] == "info":
    if option("--format") == "{{json .DriverStatus}}":
        print(json.dumps([["driver-type", "io.containerd.snapshotter.v1"]]
                         if os.environ.get("MOCK_IMAGE_STORE") == "containerd" else []))
    elif option("--format") == "{{.MemTotal}}":
        print(os.environ.get("MOCK_MEMORY", "64000000000"))
    else:
        print("x86_64")
elif args[:2] == ["buildx", "build"]:
    target = option("--target") if "--target" in args else ""
    if os.environ.get("MOCK_BUILD_FAIL") or os.environ.get("MOCK_FAIL_TARGET") == target:
        sys.exit(1)
    if target == "build-contract":
        output = Path(option("--output").split("dest=", 1)[1])
        output.mkdir(parents=True)
        (output / "toolchain.txt").write_text("Rust/native toolchain fixture")
        sys.exit(0)
    if target == "checked-state":
        sys.exit(0)
    if target == "cargo-state":
        output = Path(option("--output").split("dest=", 1)[1])
        oci_layout(output, option("--label").split("=", 1)[1])
        sys.exit(0)
    index = role(option("-t")) if "-t" in args else 0
    metadata = {"containerimage.digest": "sha256:" + str(index + 1) * 64}
    Path(option("--metadata-file")).write_text(json.dumps(metadata))
    if "--output" in args:
        output = Path(option("--output").split("dest=", 1)[1])
        (output / "bin").mkdir(parents=True)
        for binary in os.environ["MOCK_BINARIES"].split(","):
            path = output / "bin" / binary
            path.write_text("#!/bin/sh\nexit 0\n")
            path.chmod(0o755)
        for name in ("rustc-version.txt", "cargo-version.txt", "cargo-timing.html"):
            (output / name).write_text("fixture " + name)
        tests = output / "native-tests"
        tests.mkdir()
        (tests / "native-tests.tar.zst").write_bytes(b"fixture tests")
        (tests / "manifest.json").write_text(json.dumps({
            "schema": 4, "cargo_lock_sha256": hashlib.sha256(Path("Cargo.lock").read_bytes()).hexdigest(),
            "platform": "linux/amd64", "profile": "release", "rustflags": "-C link-arg=-fuse-ld=mold",
            "nextest": "0.9.143", "rust": "1.95.0", "glibc": "glibc 2.36",
            "archives_sha256": {"native-tests.tar.zst": hashlib.sha256(b"fixture tests").hexdigest()},
            "runtime_sha256": {name: hashlib.sha256((output / "bin" / name).read_bytes()).hexdigest()
                               for name in ("eez-composer", "eez-follower", "eez-proof-signer")},
        }))
elif args[:2] == ["image", "inspect"]:
    image = args[-1]
    if image.startswith(("rust@", "debian@")) and os.environ.get("MOCK_BASE_MISSING"):
        sys.exit(1)
    index = role(image)
    if "--format" not in args:
        print(json.dumps([{
            "Id": "sha256:" + (str(index + 1) if os.environ.get("MOCK_IMAGE_STORE") == "containerd" else "abc"[index]) * 64,
            "Os": "linux", "Architecture": os.environ.get("MOCK_IMAGE_ARCH", "amd64"),
            "Config": {"Labels": {
                "org.opencontainers.image.revision": os.environ.get("MOCK_IMAGE_REVISION", os.environ["GITHUB_SHA"]),
                "org.opencontainers.image.source": "https://github.com/" + os.environ["GITHUB_REPOSITORY"],
                "io.eez.protocol.revision": os.environ["MOCK_PROTOCOL_SHA"],
                "io.eez.build.profile": "release",
            }},
        }]))
    elif "RepoDigests" in option("--format"):
        print(json.dumps([image.rsplit(":", 1)[0] + "@sha256:" + str(index + 1) * 64]))
    else:
        digit = str(index + 1) if os.environ.get("MOCK_IMAGE_STORE") == "containerd" else "abc"[index]
        print("sha256:" + digit * 64)
elif args[0] == "pull" and os.environ.get("MOCK_PULL_FAIL"):
    sys.exit(23)
elif args[0] not in ("pull", "push"):
    raise SystemExit("Unexpected mocked command: " + repr(args))
'''


class RuntimeArtifactsTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eez-runtime-test.")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.output = self.root / "artifacts"
        self.log = self.root / "commands.jsonl"
        self.log.touch()
        for name in ("docker", "cargo", "kurtosis", "df"):
            path = self.tools / name
            path.write_text(MOCK_TOOL)
            path.chmod(0o755)
        for path in (
            "Dockerfile.runtime", ".config/nextest.toml",
            "scripts/ci/build-runtime-artifacts.sh",
            "scripts/ci/prepare-runtime-artifacts.sh",
            "scripts/ci/native-tests.py",
            "testing/kurtosis/scripts/runtime-tools.sh",
            "testing/kurtosis/start.sh",
            "testing/kurtosis/ci-args.yaml",
        ):
            destination = self.repo / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO / path, destination)
        (self.repo / "Cargo.lock").write_text("fixture lockfile\n")
        (self.repo / "Cargo.toml").write_text("[workspace]\n")
        port = self.repo / "crates/fixture-port/src/lib.rs"
        port.parent.mkdir(parents=True)
        port.write_text("pub const VALUE: u8 = 1;\n")
        protocol = self.repo / "eez-core-protocol"
        protocol.mkdir()
        (protocol / "fixture").write_text("protocol\n")
        for directory in (protocol, self.repo):
            self.git(directory, "init", "-q")
            self.git(directory, "add", ".")
            self.git(directory, "-c", "user.name=CI Test", "-c", "user.email=ci@example.invalid",
                     "commit", "-qm", "fixture")
        self.sha = self.git(self.repo, "rev-parse", "HEAD").strip()
        self.env_file = self.root / "github-env"
        self.path_file = self.root / "github-path"
        self.env_file.touch()
        self.path_file.touch()
        self.env = os.environ.copy()
        self.env.update({
            "PATH": str(self.tools) + os.pathsep + os.environ["PATH"],
            "MOCK_LOG": str(self.log), "MOCK_BINARIES": ",".join(BINARIES),
            "MOCK_PROTOCOL_SHA": self.git(protocol, "rev-parse", "HEAD").strip(),
            "GITHUB_REPOSITORY": "eduadiez/rollupX", "GITHUB_SHA": self.sha,
            "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
            "GITHUB_REF": "refs/pull/14/merge", "EEZ_DEFAULT_BRANCH": "main",
            "EEZ_RUNTIME_CACHE": "true",
            "EEZ_COMPILER_CACHE": "none",
            "EEZ_CARGO_STATE_CACHE": "false", "GITHUB_OUTPUT": str(self.root / "github-output"),
            "RUNNER_TEMP": str(self.root), "GITHUB_ENV": str(self.env_file),
            "GITHUB_PATH": str(self.path_file), "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
            "EEZ_RUNTIME_OUTPUT_DIR": str(self.output),
            "EEZ_NATIVE_TEST_OUTPUT_DIR": str(self.root / "test-artifacts"),
            "EEZ_PUBLISH_IMAGES": "true", "EEZ_EXPECT_PUBLISHED": "true",
        })

    @staticmethod
    def git(directory, *args):
        return subprocess.check_output(["git", "-C", str(directory), *args], text=True, stderr=subprocess.DEVNULL)

    def run_script(self, name, *args, success=True):
        if name == "build-runtime-artifacts.sh":
            self.env["EEZ_NATIVE_TEST_OUTPUT_DIR"] = self.env["EEZ_RUNTIME_OUTPUT_DIR"] + "-tests"
        result = subprocess.run(
            ["bash", str(self.repo / "scripts/ci" / name), *map(str, args)],
            env=self.env, text=True, capture_output=True,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def build(self, published=True):
        self.env["EEZ_PUBLISH_IMAGES"] = str(published).lower()
        self.env["EEZ_EXPECT_PUBLISHED"] = str(published).lower()
        self.run_script("build-runtime-artifacts.sh")

    def prepare(self, success=True):
        self.log.write_text("")
        return self.run_script("prepare-runtime-artifacts.sh", self.output, success=success)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def manifest(self):
        return json.loads((self.output / "manifest.json").read_text())

    def prepare_state(self, seed=True):
        self.env["EEZ_CARGO_STATE_CACHE"] = "true"
        self.run_script("build-runtime-artifacts.sh", "prepare-state")
        state = self.repo / ".ci-cargo-state"
        if seed:
            compatibility = json.loads((state / "plan.json").read_text())["compatibility"]
            subprocess.run([str(self.tools / "docker"), "buildx", "build", "--target", "cargo-state",
                            "--label", f"io.eez.cargo.compatibility={compatibility}",
                            "--output", f"type=oci,dest={state / 'transport'}"], env=self.env, check=True)
        self.log.write_text("")
        return state

    def test_oci_state_hit_is_bounded_offline_and_does_not_pass_compiler_secrets(self):
        self.prepare_state()
        self.env.update(EEZ_COMPILER_CACHE="redis", SCCACHE_REDIS_ENDPOINT="private-fixture")
        result = self.run_script("build-runtime-artifacts.sh")
        self.assertIn("Docker Cargo state: hit", result.stdout)
        calls = [c for c in self.commands() if c[:3] == ["docker", "buildx", "build"]]
        self.assertEqual(sum("binaries" in c for c in calls), 1)
        self.assertEqual(sum("checked-state" in c for c in calls), 1)
        for command in calls:
            if "Dockerfile.runtime" not in command:
                continue
            for setting in ("cpu-quota=400000", "memory=20g", "memory-swap=20g",
                            "BUILD_STAGE=build-offline", "EEZ_COMPILER_CACHE=none"):
                self.assertIn(setting, command)
            self.assertNotIn("--secret", command)
            self.assertTrue(any(c.startswith("previous-state=oci-layout://") for c in command))

    def test_unusable_oci_layouts_fall_back_before_compilation(self):
        state = self.prepare_state()
        transport = state / "transport"
        original = self.root / "original-state"
        shutil.copytree(transport, original)
        for case in ("missing", "invalid-index", "oversized", "symlink", "incompatible"):
            with self.subTest(case=case):
                shutil.rmtree(transport, ignore_errors=True)
                if case != "missing":
                    shutil.copytree(original, transport)
                    if case == "invalid-index":
                        (transport / "index.json").write_text("not JSON")
                    elif case == "oversized":
                        with (transport / "oversized").open("wb") as stream:
                            stream.truncate(4 * 1024**3 + 1)
                    elif case == "symlink":
                        (transport / "outside").symlink_to(self.repo, target_is_directory=True)
                    else:
                        plan = json.loads((state / "plan.json").read_text())
                        plan["compatibility"] = "incompatible"
                        (state / "plan.json").write_text(json.dumps(plan))
                self.env["EEZ_RUNTIME_OUTPUT_DIR"] = str(self.root / case)
                self.log.write_text("")
                result = self.run_script("build-runtime-artifacts.sh")
                self.assertIn("Docker Cargo state: miss", result.stdout)
                self.assertFalse(any("checked-state" in c for c in self.commands()))
                self.assertEqual(sum("binaries" in c for c in self.commands()), 1)

    def test_rejected_docker_state_falls_back_but_failed_cargo_is_not_retried(self):
        self.prepare_state()
        self.env["MOCK_FAIL_TARGET"] = "checked-state"
        result = self.run_script("build-runtime-artifacts.sh")
        self.assertIn("rejected before compilation", result.stdout)
        self.assertEqual(sum("checked-state" in c for c in self.commands()), 1)
        self.assertEqual(sum("binaries" in c for c in self.commands()), 1)
        self.env.update(MOCK_FAIL_TARGET="binaries", EEZ_RUNTIME_OUTPUT_DIR=str(self.root / "failed"))
        self.log.write_text("")
        self.run_script("build-runtime-artifacts.sh", success=False)
        self.assertEqual(sum("binaries" in c for c in self.commands()), 1)
        self.assertFalse(any(c[:2] == ["docker", "push"] for c in self.commands()))

    def test_oci_export_requires_main_push_and_exact_unit_success(self):
        state = self.prepare_state(seed=False)
        self.run_script("build-runtime-artifacts.sh")
        self.env.update(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main")
        self.run_script("build-runtime-artifacts.sh", "export-state", success=False)
        self.env["GITHUB_EVENT_NAME"] = "push"
        self.run_script("build-runtime-artifacts.sh", "export-state", success=False)
        tests = Path(self.env["EEZ_NATIVE_TEST_OUTPUT_DIR"])
        (tests / "unit-passed.json").write_text("wrong candidate")
        self.run_script("build-runtime-artifacts.sh", "export-state", success=False)
        # The real runner writes compact JSON, while sealing uses indentation.
        (tests / "unit-passed.json").write_text(json.dumps(json.loads((tests / "manifest.json").read_text())))
        self.log.write_text("")
        self.run_script("build-runtime-artifacts.sh", "export-state")
        self.assertTrue((state / "transport/index.json").is_file())
        self.assertEqual(sum("cargo-state" in c for c in self.commands()), 1)
        self.assertNotIn("mode=max", json.dumps(self.commands()))
        self.assertIn("upload=true", Path(self.env["GITHUB_OUTPUT"]).read_text())

    def test_state_prepare_rejects_insufficient_memory_and_disk(self):
        self.env["EEZ_CARGO_STATE_CACHE"] = "true"
        for memory, disk in (("1000000000", "90000000000"), ("64000000000", "1000000000")):
            with self.subTest(memory=memory, disk=disk):
                self.env.update(MOCK_MEMORY=memory, MOCK_FREE_DISK=disk)
                self.log.write_text("")
                self.run_script("build-runtime-artifacts.sh", "prepare-state", success=False)
                self.assertFalse(any(c[:3] == ["docker", "buildx", "build"] for c in self.commands()))

    def test_cargo_state_rejects_modified_or_untracked_workspace_sources(self):
        self.env.update(EEZ_CARGO_STATE_CACHE="true", EEZ_PUBLISH_IMAGES="false")
        port = self.repo / "crates/fixture-port/src/lib.rs"
        port.write_text("pub const VALUE: u8 = 2;\n")
        result = self.run_script("build-runtime-artifacts.sh", "prepare-state", success=False)
        self.assertIn("Cargo state requires clean compilation inputs", result.stderr)
        self.git(self.repo, "restore", "crates")
        (port.parent / "untracked.rs").write_text("pub const VALUE: u8 = 3;\n")
        result = self.run_script("build-runtime-artifacts.sh", "prepare-state", success=False)
        self.assertIn("Cargo state requires tracked compilation inputs", result.stderr)
        self.assertFalse(any(c[:3] == ["docker", "buildx", "build"] for c in self.commands()))

    def test_workspace_commit_changes_cargo_state_source_key(self):
        state = self.prepare_state(seed=False)
        before = json.loads((state / "plan.json").read_text())
        shutil.rmtree(state)
        (self.repo / "crates/fixture-port/src/lib.rs").write_text("pub const VALUE: u8 = 2;\n")
        self.git(self.repo, "add", "crates")
        self.git(self.repo, "-c", "user.name=CI Test", "-c", "user.email=ci@example.invalid",
                 "commit", "-qm", "change port")
        self.env["GITHUB_SHA"] = self.git(self.repo, "rev-parse", "HEAD").strip()
        state = self.prepare_state(seed=False)
        after = json.loads((state / "plan.json").read_text())
        self.assertEqual(before["compatibility"], after["compatibility"])
        self.assertNotEqual(before["source"], after["source"])

    def test_failed_base_pull_stops_before_building(self):
        self.env.update(EEZ_CARGO_STATE_CACHE="true", MOCK_BASE_MISSING="1", MOCK_PULL_FAIL="1",
                        RUST_IMAGE="rust@sha256:" + "d" * 64)
        self.run_script("build-runtime-artifacts.sh", "prepare-state", success=False)
        self.assertTrue(any(c[1] == "pull" for c in self.commands()))
        self.assertFalse(any(c[:3] == ["docker", "buildx", "build"] for c in self.commands()))

    def test_source_reconciliation_and_cache_validation_precede_cargo(self):
        dockerfile = (REPO / "Dockerfile.runtime").read_text()
        self.assertIn("--checksum --delete --perms --exclude=/target", dockerfile)
        self.assertNotIn("--times", dockerfile)
        self.assertIn("-newermt now", dockerfile)
        self.assertIn("34359738368", dockerfile)
        self.assertIn('readlink -m "$link"', dockerfile)
        self.assertLess(dockerfile.index("-newermt now"), dockerfile.index("cargo fetch --locked"))
        fetched = dockerfile.split("FROM checked-state AS build-inputs", 1)[1].split("FROM build-inputs AS build-online", 1)[0]
        self.assertIn("cargo fetch --locked", fetched)
        self.assertIn("FROM build-inputs AS build-offline\nENV EEZ_EXPORT_CARGO_STATE=true\nRUN --network=none bash /usr/local/lib/eez/build-runtime.sh", dockerfile)
        self.assertIn("export CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true", dockerfile)

    def test_no_publication_same_job_needs_no_image_archive_or_pull(self):
        self.build(published=False)
        self.assertFalse(self.manifest()["published"])
        self.assertFalse(any(c[1] == "push" for c in self.commands()))
        self.assertFalse((self.output / "runtime-images.tar.gz").exists())
        self.prepare()
        self.assertFalse(any(command[1:3] == ["image", "load"] or command[1] == "pull"
                             for command in self.commands()))

    def test_local_image_identity_must_not_change_before_consumption(self):
        self.build(published=False)
        manifest = self.manifest()
        manifest["images"]["node"]["image_id"] = "sha256:" + "f" * 64
        self.save_manifest(manifest)
        self.prepare(success=False)
        self.assertNotIn("EEZ_SKIP_NODE_BUILD", self.env_file.read_text())

    def save_manifest(self, manifest):
        (self.output / "manifest.json").write_text(json.dumps(manifest))

    def test_published_handoff_uses_digests_and_does_not_compile(self):
        self.build()
        summary = (self.root / "summary").read_text()
        self.assertIn("Runtime payload: binaries and manifest only.", summary)
        self.assertIn("Build metadata and Cargo timings are uploaded separately on failure.", summary)
        builds = [c for c in self.commands() if c[:3] == ["docker", "buildx", "build"]]
        self.assertEqual(len(builds), 4)
        self.assertEqual(sum("binaries" in c for c in builds), 1)
        self.assertEqual(sum(c[:2] == ["docker", "push"] for c in self.commands()), 3)
        self.assertFalse((self.output / "runtime-images.tar.gz").exists())
        self.prepare()
        self.assertEqual(sum(c[:2] == ["docker", "pull"] for c in self.commands()), 3)
        self.assertFalse(any(c[0] == "cargo" or "build" in c for c in self.commands()))
        native_manifest = json.loads((Path(self.env["EEZ_NATIVE_TEST_OUTPUT_DIR"]) / "manifest.json").read_text())
        self.assertEqual(native_manifest["runtime_archive_sha256"], self.manifest()["binaries_sha256"])
        self.assertEqual(native_manifest["root_commit"], self.sha)
        values = dict(line.split("=", 1) for line in self.env_file.read_text().splitlines())
        for key in ("EEZ_NODE_IMAGE", "EEZ_PROOF_SIGNER_IMAGE", "EEZ_DEPLOY_IMAGE"):
            self.assertIn("@sha256:", values[key])
        self.assertEqual(values["EEZ_SKIP_NODE_BUILD"], "1")
        self.assertEqual(values["EEZ_SKIP_PROOF_SIGNER_BUILD"], "1")
        self.assertEqual(values["EEZ_SKIP_DEPLOY_BUILD"], "1")
        self.assertTrue(os.access(values["EEZ_BLOB_VERIFIER_BIN"], os.X_OK))

    def test_all_outputs_use_the_same_resolved_base_images(self):
        self.build()
        manifest = self.manifest()
        builds = [c for c in self.commands() if c[:3] == ["docker", "buildx", "build"]]
        for command in builds:
            self.assertIn("RUNTIME_IMAGE=" + manifest["base_images"]["runtime"], command)
            if "Dockerfile.runtime" in command:
                self.assertIn("RUST_IMAGE=" + manifest["base_images"]["rust"], command)
        self.assertIn("@sha256:", manifest["base_images"]["rust"])

    def test_compiler_cache_credentials_use_secret_mounts_only(self):
        for backend, keys in (("gha", ["ACTIONS_RESULTS_URL", "ACTIONS_RUNTIME_TOKEN"]),
                              ("redis", ["SCCACHE_REDIS_ENDPOINT", "SCCACHE_REDIS_PASSWORD", "SCCACHE_REDIS_KEY_PREFIX"])):
            with self.subTest(backend=backend):
                self.output = self.root / f"compiler-cache-{backend}"
                self.env.update(EEZ_COMPILER_CACHE=backend, EEZ_RUNTIME_OUTPUT_DIR=str(self.output))
                self.env.update({key: f"private-{key}" for key in keys})
                self.log.write_text("")
                self.build()
                builds = [c for c in self.commands() if c[:3] == ["docker", "buildx", "build"]]
                for command in builds:
                    if "Dockerfile.runtime" in command:
                        self.assertIn(f"EEZ_COMPILER_CACHE={backend}", command)
                        for key in keys:
                            self.assertIn(f"id={key},env={key}", command)
                    else:
                        self.assertNotIn("--secret", command)
                    self.assertNotIn("private-", json.dumps(command))
                self.assertEqual(len(builds), 4, "compiler caching must not add a control build")

    def test_registry_cache_prefers_ref_scoped_source(self):
        self.env["MOCK_IMAGE_STORE"] = "containerd"
        self.build()
        builds = [c for c in self.commands() if c[:3] == ["docker", "buildx", "build"]]
        def tag(ref):
            return "buildcache-v1-linux-amd64-" + hashlib.sha256(ref.encode()).hexdigest()[:16]
        for command in builds:
            role = "deploy" if "testing/kurtosis/Dockerfile.deploy" in command else "node"
            repo = f"ghcr.io/eduadiez/rollupx-{role}"
            imports = [command[i+1] for i, arg in enumerate(command) if arg == "--cache-from"]
            self.assertEqual(imports, [f"type=registry,ref={repo}:{tag(self.env['GITHUB_REF'])}"])
            exports = [command[i+1] for i, arg in enumerate(command) if arg == "--cache-to"]
            if "binaries" in command or role == "deploy":
                settings = "mode=max" if role == "deploy" else "mode=min,compression=zstd,compression-level=1"
                self.assertEqual(exports, [f"type=registry,ref={repo}:{tag(self.env['GITHUB_REF'])},{settings},ignore-error=true"])
            else:
                self.assertEqual(exports, [], "export flat executable output only once, never Cargo state")
        self.assertEqual(len(builds), 4, "caching must not add another compilation")
        self.assertTrue(all("@sha256:" in image["reference"] for image in self.manifest()["images"].values()))

    def test_missing_ref_cache_uses_read_only_default_branch_fallback(self):
        self.env["MOCK_IMAGE_STORE"] = "containerd"
        tag = lambda ref: "buildcache-v1-linux-amd64-" + hashlib.sha256(ref.encode()).hexdigest()[:16]
        self.env["MOCK_MISSING_CACHE_REF"] = (
            f"ghcr.io/eduadiez/rollupx-node:{tag(self.env['GITHUB_REF'])}")
        self.build()
        builds = [c for c in self.commands() if c[:3] == ["docker", "buildx", "build"]]
        for command in builds:
            role = "deploy" if "testing/kurtosis/Dockerfile.deploy" in command else "node"
            repo = f"ghcr.io/eduadiez/rollupx-{role}"
            imports = [command[i+1] for i, arg in enumerate(command) if arg == "--cache-from"]
            expected_ref = "refs/heads/main" if role == "node" else self.env["GITHUB_REF"]
            self.assertEqual(imports, [f"type=registry,ref={repo}:{tag(expected_ref)}"])
            exports = [command[i+1] for i, arg in enumerate(command) if arg == "--cache-to"]
            if "binaries" in command or role == "deploy":
                settings = "mode=max" if role == "deploy" else "mode=min,compression=zstd,compression-level=1"
                self.assertEqual(exports, [f"type=registry,ref={repo}:{tag(self.env['GITHUB_REF'])},{settings},ignore-error=true"])

    def test_main_cache_is_imported_only_once(self):
        self.env.update(MOCK_IMAGE_STORE="containerd", GITHUB_REF="refs/heads/trunk", EEZ_DEFAULT_BRANCH="trunk")
        self.build()
        builds = [c for c in self.commands() if c[:3] == ["docker", "buildx", "build"]]
        self.assertTrue(all(c.count("--cache-from") == 1 for c in builds))

    def test_cache_is_optional_for_classic_stores_read_only_and_opt_out(self):
        for name, store, enabled, published, ref in (
            ("classic", "classic", "true", True, "refs/heads/main"),
            ("fork", "containerd", "true", False, "refs/pull/1/merge"),
            ("opt-out", "containerd", "false", True, "refs/heads/main"),
            ("missing-ref", "containerd", "true", True, ""),
        ):
            with self.subTest(case=name):
                self.output = self.root / f"artifacts-cache-{name}"
                self.env.update(EEZ_RUNTIME_OUTPUT_DIR=str(self.output), MOCK_IMAGE_STORE=store,
                                EEZ_RUNTIME_CACHE=enabled, GITHUB_REF=ref)
                self.log.write_text("")
                self.build(published=published)
                for call in self.commands():
                    self.assertNotIn("--cache-from", call)
                    self.assertNotIn("--cache-to", call)

    def test_target_is_not_in_persistent_runtime_cache_layers(self):
        dockerfile = (REPO / "Dockerfile.runtime").read_text()
        self.assertEqual(dockerfile.count("runtime cargo build"), 1)
        compile_steps = [dockerfile.split("COPY <<'BUILD_RUNTIME'", 1)[1].split("\nBUILD_RUNTIME", 1)[0]]
        self.assertIn("ENV EEZ_EXPORT_CARGO_STATE=false", dockerfile)
        cleanup = 'if [ "$EEZ_EXPORT_CARGO_STATE" = false ]; then'
        self.assertIn("rm -rf /build/target /usr/local/cargo/registry /usr/local/cargo/git", compile_steps[0])
        self.assertIn("native-tests.py build /out/native-tests --runtime-bin /out/bin", compile_steps[0])
        self.assertLess(compile_steps[0].index("install -D target/release/eez-composer"),
                        compile_steps[0].index("native-tests.py build"))
        self.assertLess(compile_steps[0].index("native-tests.py build"),
                        compile_steps[0].index(cleanup))
        self.assertIn("FROM scratch AS cargo-state", dockerfile)
        self.assertIn("FROM scratch AS binaries\nCOPY --from=builder /out/ /", dockerfile)
        runtime = dockerfile.split("FROM ${RUNTIME_IMAGE} AS runtime-base", 1)[1]
        self.assertNotIn("--from=builder", runtime)
        self.assertIn("COPY --from=binaries", runtime)
        self.assertNotIn("/build", runtime)
        self.assertNotIn("/cargo", runtime)
        self.assertNotIn("target=/build/target", dockerfile)
        self.assertIn("CARGO_INCREMENTAL=0", dockerfile)

    def test_invalid_cache_setting_fails_before_docker(self):
        self.env["EEZ_RUNTIME_CACHE"] = "typo"
        self.run_script("build-runtime-artifacts.sh", success=False)
        self.assertEqual(self.commands(), [])

    def test_handoff_between_classic_and_containerd_image_stores(self):
        for producer, consumer in (("containerd", "classic"), ("classic", "containerd")):
            with self.subTest(producer=producer, consumer=consumer):
                self.output = self.root / f"artifacts-{producer}"
                self.env["EEZ_RUNTIME_OUTPUT_DIR"] = str(self.output)
                self.env["MOCK_IMAGE_STORE"] = producer
                self.build()
                self.env["MOCK_IMAGE_STORE"] = consumer
                self.prepare()

    def test_kurtosis_resource_check_is_explicit_and_preserves_image_policy(self):
        self.env.update({"EEZ_SKIP_NODE_BUILD": "1", "EEZ_SKIP_PROOF_SIGNER_BUILD": "1",
                         "EEZ_SKIP_DEPLOY_BUILD": "1", "EEZ_SKIP_UI_BUILD": "1",
                         "KURTOSIS_PRIVILEGED": "0"})
        for setting in ("", "true", "false", "invalid"):
            with self.subTest(setting=setting):
                self.log.write_text("")
                self.env["KURTOSIS_RESOURCE_CHECK"] = setting
                result = subprocess.run(["bash", str(self.repo / "testing/kurtosis/start.sh")],
                                        env=self.env, text=True, capture_output=True)
                if setting == "invalid":
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(self.commands(), [])
                    continue
                self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
                launch, = self.commands()
                self.assertEqual(launch[:2], ["kurtosis", "run"])
                self.assertIn(f"--resource-check={setting or 'true'}", launch)
                self.assertEqual(launch[launch.index("--image-download") + 1], "missing")
                self.assertNotIn("--privileged", launch)

    def test_wrong_manifest_identity_is_rejected_before_docker(self):
        self.build()
        original = self.manifest()
        for field, value in (
            ("root_commit", "f" * 40), ("protocol_commit", "f" * 40),
            ("cargo_lock_sha256", "f" * 64), ("platform", "linux/arm64"),
            ("profile", "maxperf"), ("schema", 1), ("published", False),
        ):
            with self.subTest(field=field):
                self.save_manifest(dict(original, **{field: value}))
                self.prepare(success=False)
                self.assertEqual(self.commands(), [])

    def test_wrong_workflow_revision_is_rejected(self):
        self.build()
        self.env["GITHUB_SHA"] = "f" * 40
        self.prepare(success=False)
        self.assertEqual(self.commands(), [])

    def test_corrupt_binary_archive_is_rejected_before_docker(self):
        self.build()
        with (self.output / "runtime-binaries.tar.gz").open("ab") as stream:
            stream.write(b"corruption")
        self.prepare(success=False)
        self.assertEqual(self.commands(), [])

    def test_mutable_or_foreign_image_reference_is_rejected(self):
        self.build()
        original = self.manifest()
        for reference in ("ghcr.io/eduadiez/rollupx-node:latest", "ghcr.io/other/node@sha256:" + "1" * 64):
            with self.subTest(reference=reference):
                original["images"]["node"]["reference"] = reference
                self.save_manifest(original)
                self.prepare(success=False)
                self.assertEqual(self.commands(), [])

    def test_loaded_image_revision_must_match_producer(self):
        self.build()
        self.env["MOCK_IMAGE_REVISION"] = "f" * 40
        self.prepare(success=False)
        self.assertNotIn("EEZ_SKIP_NODE_BUILD", self.env_file.read_text())

    def test_loaded_image_platform_must_match_producer(self):
        self.build()
        self.env["MOCK_IMAGE_ARCH"] = "arm64"
        self.prepare(success=False)
        self.assertNotIn("EEZ_SKIP_NODE_BUILD", self.env_file.read_text())

    def test_missing_runtime_binary_is_rejected(self):
        self.env["MOCK_BINARIES"] = ",".join(BINARIES[:-1])
        self.build()
        self.prepare(success=False)
        self.assertNotIn("EEZ_SKIP_NODE_BUILD", self.env_file.read_text())

    def test_archive_path_escape_is_rejected(self):
        self.build()
        archive = self.output / "runtime-binaries.tar.gz"
        with tarfile.open(archive, "w:gz") as stream:
            entry = tarfile.TarInfo("../escaped")
            entry.size = 1
            stream.addfile(entry, io.BytesIO(b"x"))
        manifest = self.manifest()
        manifest["binaries_sha256"] = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.save_manifest(manifest)
        self.prepare(success=False)
        self.assertFalse((self.root / "escaped").exists())

    def test_dirty_checkout_cannot_publish(self):
        (self.repo / "Cargo.lock").write_text("modified lockfile")
        self.run_script("build-runtime-artifacts.sh", success=False)
        self.assertEqual(self.commands(), [])

    def test_failed_build_does_not_publish_or_finish_manifest(self):
        self.env["MOCK_BUILD_FAIL"] = "1"
        result = self.run_script("build-runtime-artifacts.sh", success=False)
        self.assertIn("failed during compilation", result.stderr)
        self.assertFalse((self.output / "manifest.json").exists())
        self.assertFalse(any(c[1] == "push" for c in self.commands()))

    def test_local_image_dependency_requires_docker_driver(self):
        self.env["MOCK_DRIVER"] = "docker-container"
        self.run_script("build-runtime-artifacts.sh", success=False)
        self.assertFalse(any("build" in c for c in self.commands()))

    def test_nonempty_output_directory_is_not_overwritten(self):
        self.output.mkdir()
        marker = self.output / "existing"
        marker.write_text("keep")
        self.run_script("build-runtime-artifacts.sh", success=False)
        self.assertEqual(marker.read_text(), "keep")

    def resolve_tool(self, prebuilt="", success=True):
        helper = self.repo / "testing/kurtosis/scripts/runtime-tools.sh"
        result = subprocess.run(
            ["bash", "-c", 'source "$1"; resolve_runtime_tool eez-l1 verify_blob_evidence "$2"',
             "bash", str(helper), str(prebuilt)], env=self.env, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def test_explicit_tool_override_avoids_cargo(self):
        self.env["GITHUB_ACTIONS"] = "true"
        tool = self.root / "prebuilt"
        tool.write_text("#!/bin/sh\nexit 0\n")
        tool.chmod(0o755)
        self.assertEqual(self.resolve_tool(tool).stdout.strip(), str(tool))
        self.assertEqual(self.commands(), [])

    def test_missing_explicit_tool_never_silently_rebuilds(self):
        self.resolve_tool(self.root / "missing", success=False)
        self.assertEqual(self.commands(), [])

    def test_local_tool_fallback_uses_locked_job_target(self):
        self.env["GITHUB_ACTIONS"] = "false"
        self.env["CARGO_TARGET_DIR"] = str(self.root / "job-target")
        result = self.resolve_tool()
        self.assertEqual(result.stdout.strip(), str(self.root / "job-target/debug/examples/verify_blob_evidence"))
        self.assertEqual(len(self.commands()), 1)
        self.assertIn("--locked", self.commands()[0])

    def test_ci_missing_tool_override_never_silently_rebuilds(self):
        self.env["GITHUB_ACTIONS"] = "true"
        result = self.resolve_tool(success=False)
        self.assertIn("refusing Cargo fallback", result.stderr)
        self.assertEqual(self.commands(), [])


if __name__ == "__main__":
    unittest.main()
