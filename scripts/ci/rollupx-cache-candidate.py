#!/usr/bin/env python3
"""Turn the measured cache into a same-job, unpublished candidate for full E2E."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import uuid

spec = importlib.util.spec_from_file_location("followup", Path(__file__).with_name("rollupx-cache-followup.py"))
follow = importlib.util.module_from_spec(spec)
spec.loader.exec_module(follow)
bench, native = follow.bench, follow.native
TEST_SEED_COMMIT = "c96e2ef56e855f062eb472a869b74dc41b13c7d1"
EXPORT_NAMES = dict(zip(bench.BINARIES, (
    "eez-composer", "eez-follower", "eez-proof-signer", "eez-genesis-state-root",
    "eez-genesis-block-hash", "e2e-causal-verify", "verify_blob_evidence")))


def checked(*args):
    subprocess.run(args, cwd=bench.REPO, check=True)


def checkout_identity():
    identity = native.identity()  # Exact workflow commit and initialized protocol pin.
    checked("git", "diff", "--quiet", "HEAD")
    status = bench.output("git", "submodule", "status", "--recursive")
    if any(line.startswith(("-", "+", "U")) for line in status.splitlines()):
        raise ValueError("submodule checkout differs from pinned revision")
    checked("git", "submodule", "foreach", "--recursive", "git diff --quiet HEAD")
    return identity


def prepare(work, image):
    identity = checkout_identity()
    work.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(work).free < 50 * 1024**3:
        raise ValueError("candidate needs 50 GiB free; no automatic cleanup")
    source_identity = {"commit": identity["root_commit"], "protocol": identity["protocol_commit"],
                       "source": bench.stage_source(work / "checkout"), "build": list(bench.BUILD)}
    follow.extract(work)
    state = work / "state"
    seed = json.loads((state / "followup.json").read_text())
    follow.check_identity(seed, source_identity, TEST_SEED_COMMIT)
    if follow.frozen_hashes(work) != seed["hashes"]:
        raise ValueError("cached shipping binary identity mismatch")
    checked("rsync", "--recursive", "--checksum", "--delete", "--perms", "--links", "--",
            str(work / "checkout") + "/", str(state / "source") + "/")
    report = {"identity": source_identity, "seed_commit": TEST_SEED_COMMIT, "status": "incomplete"}
    try:
        bench.run(work, image, "environment", ["sh", "-ec", "rustc -Vv; cargo -V; mold --version; dpkg-query -W"])
        if (work / "environment.stdout.log").read_text() != seed["environment"]:
            raise ValueError("toolchain/native package identity changed")
        report["shipping"] = follow.compile_phase(work, image, "shipping", bench.BUILD)
        if bench.hashes(state) != seed["hashes"]:
            raise ValueError("rebuilt shipping bytes differ from unchanged seed")
        output = work / "output"
        (output / "bin").mkdir(parents=True)
        for source, name in EXPORT_NAMES.items():
            shutil.copy2(state / "target/release" / source, output / "bin" / name)
        for label, command in (("rustc-version", ["rustc", "-Vv"]), ("cargo-version", ["cargo", "-V"])):
            bench.run(work, image, label, command)
            shutil.copy2(work / f"{label}.stdout.log", output / f"{label}.txt")

        # Use the unmodified production producer. It freezes the freshly validated
        # shipping executables itself; retain the cached frozen copy as evidence.
        (state / "target/release/eez-runtime").rename(work / "cached-frozen-runtime")
        scripts = state / "source/scripts/ci"
        scripts.mkdir(parents=True)
        for name in ("native-tests.py", "with-compiler-cache.sh"):
            shutil.copy2(bench.REPO / "scripts/ci" / name, scripts / name)
        report["test_archives"] = bench.run(work, image, "test-archives",
            ["env", "CARGO_NET_OFFLINE=true", "EEZ_BUILD_METRICS_DIR=/probe/test-build-metrics",
             "python3", "scripts/ci/native-tests.py", "build", "/probe/native-tests",
             "--runtime-bin", "/probe/output/bin"])
        tests = work / "native-tests"
        manifest = json.loads((tests / "manifest.json").read_text())
        native.verify_archives(tests, manifest)
        if manifest["runtime_sha256"] != native.runtime_hashes(output / "bin"):
            raise ValueError("test archives are not bound to shipping candidate")
        runtime = work / "runtime"
        runtime.mkdir()
        with tarfile.open(runtime / "runtime-binaries.tar.gz", "w:gz") as archive:
            for name in ("bin", "rustc-version.txt", "cargo-version.txt"):
                archive.add(output / name, arcname=name)
        native.seal(tests, runtime / "runtime-binaries.tar.gz")
        report["status"] = "candidate built and sealed; correctness suites pending"
        with open(os.environ["GITHUB_ENV"], "a") as stream:
            stream.write(f"EEZ_RUNTIME_OUTPUT_DIR={runtime}\nEEZ_NATIVE_TEST_OUTPUT_DIR={tests}\n")
    finally:
        bench.write_report(work, report)


def packaging_dockerfile():
    # Reuse the exact production runtime stages; only replace their Rust builder
    # with a data-only stage. No second set of runtime packages/configuration.
    marker = "FROM ${RUNTIME_IMAGE} AS runtime-base"
    text = (bench.REPO / "Dockerfile.runtime").read_text()
    if text.count(marker) != 1:
        raise ValueError("production runtime stage boundary changed")
    return ("# syntax=docker/dockerfile:1\nARG RUNTIME_IMAGE\n"
            "FROM scratch AS builder\nCOPY bin /out/bin/\n\n" + marker + text.split(marker)[1])


def stage_deploy(destination):
    # Never send the workspace/cache, untracked data, .git files or credentials
    # to Docker. Recurse only through pinned, initialized tracked submodules.
    names = subprocess.check_output(["git", "ls-files", "-z", "--recurse-submodules", "--",
        "scripts", "contracts", "eez-core-protocol", "genesis.json", "testing/kurtosis/Dockerfile.deploy"],
        cwd=bench.REPO).decode().rstrip("\0").split("\0")
    destination.mkdir()
    for name in names:
        source = bench.REPO / name
        if not source.is_file() or source.is_symlink():
            raise ValueError(f"deploy input is not a regular tracked file: {name}")
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)


def verify_image_bytes(image, names, binaries, destination):
    destination.mkdir()
    container = "eez-cache-image-check-" + uuid.uuid4().hex
    try:
        checked("docker", "create", "--name", container, "--network=none", image)
        for name in names:
            checked("docker", "cp", f"{container}:/usr/local/bin/{name}", str(destination / name))
            if bench.digest(destination / name) != bench.digest(binaries / name):
                raise ValueError(f"packaged image changed {name}")
    finally:
        subprocess.run(["docker", "rm", "--force", container], check=False,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def package(work):
    identity = checkout_identity()
    binaries, runtime = work / "output/bin", work / "runtime"
    tests = json.loads((work / "native-tests/manifest.json").read_text())
    native.verify_archives(work / "native-tests", tests)
    if any(tests.get(key) != value for key, value in identity.items()):
        raise ValueError("test archives are not sealed for this revision")
    if tests["runtime_sha256"] != native.runtime_hashes(binaries):
        raise ValueError("packaging inputs differ from the sealed candidate")
    if tests["runtime_archive_sha256"] != bench.digest(runtime / "runtime-binaries.tar.gz"):
        raise ValueError("runtime archive changed after sealing")
    with tarfile.open(runtime / "runtime-binaries.tar.gz") as archive:
        for name in EXPORT_NAMES.values():
            with archive.extractfile("bin/" + name) as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != bench.digest(binaries / name):
                    raise ValueError(f"packaging input differs from runtime archive: {name}")
    driver = bench.output("docker", "buildx", "inspect", "default")
    if not re.search(r"^Driver:\s+docker$", driver, re.MULTILINE):
        raise ValueError("packaging requires the default Docker driver for local node tools")
    digest = bench.output("docker", "buildx", "imagetools", "inspect", "debian:bookworm-slim",
                          "--format", "{{.Manifest.Digest}}")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise ValueError("invalid runtime base digest")
    base = "debian@" + digest
    checked("docker", "pull", "--platform", "linux/amd64", base)
    context = work / "image-context"
    context.mkdir()
    shutil.copytree(binaries, context / "bin")
    (context / "Dockerfile").write_text(packaging_dockerfile())
    prefix = "ghcr.io/" + os.environ["GITHUB_REPOSITORY"].lower()
    tag = f"sha-{identity['root_commit']}-{os.environ['GITHUB_RUN_ID']}-{os.environ['GITHUB_RUN_ATTEMPT']}"
    labels = {"org.opencontainers.image.revision": identity["root_commit"],
              "org.opencontainers.image.source": "https://github.com/" + os.environ["GITHUB_REPOSITORY"],
              "io.eez.protocol.revision": identity["protocol_commit"], "io.eez.build.profile": "release"}
    build = ["docker", "buildx", "build", "--builder", "default", "--platform", "linux/amd64",
             "--progress", "plain", "--provenance=false", "--load", "--build-arg", "RUNTIME_IMAGE=" + base]
    for key, value in labels.items():
        build += ["--label", key + "=" + value]
    images = {}
    names_by_role = {"node": ("eez-composer", "eez-follower", "eez-genesis-state-root", "eez-genesis-block-hash"),
                     "signer": ("eez-proof-signer",), "deploy": ("eez-genesis-state-root", "eez-genesis-block-hash")}
    for role in ("node", "signer", "deploy"):
        suffix = "proof-signer" if role == "signer" else role
        image = f"{prefix}-{suffix}:{tag}"
        if role == "deploy":
            deploy = work / "deploy-context"
            stage_deploy(deploy)
            checked(*build, "-f", str(deploy / "testing/kurtosis/Dockerfile.deploy"),
                    "--build-arg", "EEZ_NODE_IMAGE=" + images["node"]["reference"], "-t", image, str(deploy))
        else:
            checked(*build, "--target", role, "-t", image, str(context))
        verify_image_bytes(image, names_by_role[role], binaries, work / f"{role}-image-bytes")
        images[role] = {"reference": image, "image_id": bench.output("docker", "image", "inspect", "--format", "{{.Id}}", image)}
    manifest = {"schema": 2, "root_commit": identity["root_commit"], "protocol_commit": identity["protocol_commit"],
                "cargo_lock_sha256": identity["cargo_lock_sha256"], "platform": "linux/amd64", "profile": "release",
                "rustflags": native.RUSTFLAGS, "published": False, "images": images,
                "binaries_sha256": bench.digest(runtime / "runtime-binaries.tar.gz"), "base_images": {"runtime": base}}
    (runtime / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("prepare", "package"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("--image")
    args = parser.parse_args()
    if args.operation == "prepare":
        if not args.image:
            parser.error("prepare requires --image")
        prepare(args.directory.resolve(), args.image)
    else:
        package(args.directory.resolve())
