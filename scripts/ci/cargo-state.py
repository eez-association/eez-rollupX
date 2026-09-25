#!/usr/bin/env python3
"""Optional, job-private Cargo state transport; never an executable-artifact gate.

The workflow owns cache lookup/upload. This helper owns compatibility, source
reconciliation and bounded compilation. Unusable state falls back before Cargo;
compile/test failures never trigger a second build.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tarfile
import time
import uuid

REPO = Path(__file__).resolve().parents[2]
INPUTS = ("Cargo.toml", "Cargo.lock", "crates", "contracts/test/PostBatchGasPins.t.sol",
          ".config/nextest.toml", "scripts/ci/build-native-runtime.py",
          "scripts/ci/native-tests.py", "scripts/ci/with-compiler-cache.sh")
MAX_ARCHIVE = 4 * 1024**3
MAX_EXPANDED = 32 * 1024**3
PREFIX = "cargo-state-v1-linux-amd64-"


def load(filename):
    spec = importlib.util.spec_from_file_location(filename, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


builder = load("build-native-runtime.py")
native = load("native-tests.py")


def capture(*args):
    return subprocess.check_output(args, cwd=REPO, text=True).strip()


def checked(*args):
    subprocess.run(args, cwd=REPO, check=True)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def hash_json(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def emit(filename, **values):
    if filename in os.environ:
        with open(os.environ[filename], "a") as stream:
            for name, value in values.items():
                if "\n" in str(value) or "\r" in str(value):
                    raise ValueError("multiline workflow output")
                stream.write(f"{name}={value}\n")


def inventory(source):
    result = {}
    for path in sorted(source.rglob("*")):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise ValueError("source snapshot contains a non-regular input")
        if path.is_file():
            # Git tracks executable status, not checkout umask. Safe tar
            # extraction removes group-write bits without changing the input.
            result[str(path.relative_to(source))] = [digest(path), bool(path.stat().st_mode & 0o100)]
    if not result:
        raise ValueError("empty compilation source inventory")
    return hash_json(result)


def stage(destination):
    checked("git", "diff", "--quiet", "HEAD", "--", *INPUTS)
    names = capture("git", "ls-files", "-z", "--", *INPUTS).rstrip("\0").split("\0")
    destination.mkdir()
    for name in names:
        source = REPO / name
        if source.is_symlink() or not source.is_file():
            raise ValueError(f"compilation input is not a regular tracked file: {name}")
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        target.chmod(0o755 if source.stat().st_mode & 0o100 else 0o644)
    for name in INPUTS:
        if not (destination / name).exists():
            raise ValueError(f"missing tracked compilation input: {name}")
    return inventory(destination)


def container(work, image, command, network=False):
    return ["docker", "run", "--rm", "--pull=never", "--platform=linux/amd64",
            "--name", "eez-cargo-state-" + uuid.uuid4().hex,
            "--network=" + ("bridge" if network else "none"), "--read-only", "--cap-drop=ALL",
            "--security-opt=no-new-privileges", "--cgroupns=private", "--cpus=4",
            "--memory=20g", "--memory-swap=20g", "--pids-limit=1024",
            "--user", f"{os.getuid()}:{os.getgid()}", "--tmpfs", "/tmp:rw,size=1g,mode=1777",
            "--mount", f"type=bind,src={work},dst=/probe", "--workdir=/probe",
            "--env=CARGO_HOME=/probe/state/cargo", "--env=CARGO_TARGET_DIR=/probe/state/target",
            "--env=CARGO_INCREMENTAL=0", "--env=CARGO_BUILD_JOBS=4",
            "--env=RUSTFLAGS=" + native.RUSTFLAGS, "--entrypoint=/bin/sh", image, "-ec",
            'test "$(cat /sys/fs/cgroup/cpu.max)" = "400000 100000"; '
            'test "$(cat /sys/fs/cgroup/memory.max)" = "21474836480"; '
            'test "$(cat /sys/fs/cgroup/memory.swap.max)" = "0"; '
            'set +e; "$@"; status=$?; '
            'printf "\\nContainer memory peak: "; cat /sys/fs/cgroup/memory.peak; exit "$status"',
            "cargo-state", *command]


def run(work, image, phase, command, network=False):
    args = container(work, image, command, network)
    started = time.monotonic()
    status = None
    try:
        with (work / f"{phase}.log").open("w") as log:
            process = subprocess.run(args, stdout=log, stderr=subprocess.STDOUT, timeout=5400)
        status = process.returncode
        if status:
            raise subprocess.CalledProcessError(status, command)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        with (work / f"{phase}.log").open("rb") as log:
            log.seek(max(0, (work / f"{phase}.log").stat().st_size - 16384))
            for line in log.read().decode(errors="replace").splitlines()[-50:]:
                print(f"{phase}: {json.dumps(line)}", flush=True)
        raise
    finally:
        try:
            subprocess.run(["docker", "rm", "-f", args[args.index("--name") + 1]],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30, check=False)
        except (OSError, subprocess.TimeoutExpired):
            print("Owned build-container cleanup failed; original build result retained", flush=True)
        metric = {"seconds": round(time.monotonic() - started, 3), "exit_code": status}
        (work / f"{phase}.json").write_text(json.dumps(metric) + "\n")
        print(f"Cargo state {phase}: {json.dumps(metric)}", flush=True)


def enough_space(work):
    if shutil.disk_usage(work).free < 50 * 1024**3:
        raise ValueError("Cargo state build needs 50 GiB free; no automatic cleanup")


def prepare(work):
    identity = native.identity()
    # A fixed per-checkout directory gives Actions a stable cache path/version.
    # Never reuse a prior job's directory merely because it is on the same host.
    work.mkdir()
    enough_space(work)
    if int(capture("docker", "info", "--format", "{{.MemTotal}}")) < 24 * 1024**3:
        raise ValueError("20-GiB Cargo builds require a Docker host with at least 24 GiB RAM")
    source = stage(work / "checkout")
    base = capture("docker", "buildx", "imagetools", "inspect", "rust:1.94.0-bookworm",
                   "--format", "{{.Manifest.Digest}}")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", base):
        raise ValueError("invalid Rust image digest")
    base = "rust@" + base
    image = "eez-cargo-tools:" + uuid.uuid4().hex
    checked("docker", "buildx", "build", "--builder", "default", "--platform", "linux/amd64",
            "--progress", "plain", "--load", "-f", "Dockerfile.runtime", "--target", "build-tools",
            "--build-arg", "RUST_IMAGE=" + base, "--build-arg", "EEZ_COMPILER_CACHE=none", "-t", image, ".")
    run(work, image, "environment", ["sh", "-ec", "rustc -Vv; cargo -V; mold --version; dpkg-query -W"])
    compatibility = hash_json({"environment": (work / "environment.log").read_text().split("\nContainer memory peak:")[0],
        "rust_base": base, "flags": native.RUSTFLAGS, "build": builder.BUILD, "archives": native.ARCHIVES})
    prefix = PREFIX + compatibility + "-"
    plan = {"identity": identity, "source": source, "compatibility": compatibility,
            "image": image, "rust_base": base, "prefix": prefix, "key": prefix + source}
    (work / "plan.json").write_text(json.dumps(plan, indent=2) + "\n")
    emit("GITHUB_OUTPUT", key=plan["key"], prefix=prefix)
    emit("GITHUB_ENV", EEZ_CARGO_STATE_DIR=work, EEZ_CARGO_RUST_BASE=base)


def restore(work, compatibility):
    """Quarantine rejected state and fall back only before invoking Cargo."""
    destination, archive_path = work / "state", work / "transport.tar.gz"
    if destination.exists() or destination.is_symlink():
        raise ValueError("Cargo restore requires fresh job-local state")
    if not archive_path.exists():
        return "miss"
    # data_filter confines links to the extraction directory, not to the top
    # level archive entry. Never extract beside the authoritative checkout:
    # state/link -> ../checkout could otherwise overwrite it before rejection.
    extraction = work / "restore-staging"
    extraction.mkdir()
    state = extraction / "state"
    try:
        if archive_path.stat().st_size > MAX_ARCHIVE:
            raise ValueError("compressed cache exceeds 4 GiB")
        with tarfile.open(archive_path, "r:gz") as archive:
            expanded = 0
            for member in archive:
                parts = Path(member.name).parts
                if not parts or parts[0] != "state" or ".." in parts:
                    raise ValueError("cache entry outside state")
                expanded += member.size
                if expanded > MAX_EXPANDED:
                    raise ValueError("expanded cache exceeds 32 GiB")
            archive.extractall(extraction, filter="data")
        if state.is_symlink() or not state.is_dir():
            raise ValueError("cache state root must be a directory")
        if list(extraction.iterdir()) != [state]:
            raise ValueError("cache extraction contains entries outside state")
        # Moving accepted state must not turn a previously contained relative
        # link into a reference to the job's checkout or other sibling inputs.
        for path in state.rglob("*"):
            if path.is_symlink():
                try:
                    contained = path.resolve().is_relative_to(state)
                except RuntimeError as error:
                    raise ValueError("cache contains a cyclic symlink") from error
                if not contained:
                    raise ValueError("cache link points outside state")
        metadata = json.loads((state / "cache.json").read_text())
        if metadata["compatibility"] != compatibility:
            raise ValueError("cache compatibility mismatch")
        if not 0 < metadata["completed_ns"] < time.time_ns():
            raise ValueError("cache completion timestamp is invalid or in the future")
        if metadata["source"] != inventory(state / "source"):
            raise ValueError("cached source snapshot is inconsistent")
        for name in ("source", "cargo", "target"):
            if (state / name).is_symlink() or not (state / name).is_dir():
                raise ValueError("cache directory inventory mismatch")
        state.rename(destination)
        return "hit"
    except (ValueError, KeyError, TypeError, OSError, tarfile.TarError, EOFError) as error:
        # Keep first-failure evidence. No broad deletion or shared-cache cleanup.
        extraction.rename(work / "rejected-state")
        print("Cargo cache rejected; cold build: " + json.dumps(str(error)), flush=True)
        return "rejected"


def build(work, destination):
    plan = json.loads((work / "plan.json").read_text())
    if native.identity() != plan["identity"] or inventory(work / "checkout") != plan["source"]:
        raise ValueError("prepared checkout identity changed")
    # Detect tracked source edits after planning; the original checkout is still
    # authoritative, not the cached snapshot or the staged copy.
    if stage(work / "verified-checkout") != plan["source"]:
        raise ValueError("compilation inputs changed after cache planning")
    result = restore(work, plan["compatibility"])
    if (work / "transport.tar.gz").exists():
        (work / "transport.tar.gz").rename(work / "restored.tar.gz")
    enough_space(work)
    state = work / "state"
    for name in ("source", "target", "cargo"):
        (state / name).mkdir(parents=True, exist_ok=True)
    checked("rsync", "--recursive", "--checksum", "--delete", "--perms", "--links", "--",
            str(work / "checkout") + "/", str(state / "source") + "/")
    # The producer must freeze this invocation's shipping bytes, not cached ones.
    frozen = state / "target/release/eez-runtime"
    if frozen.exists() or frozen.is_symlink():
        frozen.rename(work / "cached-frozen-runtime")
    image = plan["image"]
    run(work, image, "fetch", ["sh", "-ec", "cd /probe/state/source; cargo fetch --locked"], network=True)
    run(work, image, "build", ["sh", "-ec", "cd /probe/state/source; "
        "CARGO_NET_OFFLINE=true python3 scripts/ci/build-native-runtime.py /probe/output"])
    shutil.copytree(work / "output", destination)
    metadata = {"compatibility": plan["compatibility"], "source": plan["source"], "completed_ns": time.time_ns()}
    (state / "cache.json").write_text(json.dumps(metadata) + "\n")
    (work / "built.json").write_text(json.dumps(dict(metadata, restore=result)) + "\n")
    context = work / "runtime-context"
    shutil.copytree(work / "output/bin", context / "bin")
    marker = "FROM ${RUNTIME_IMAGE} AS runtime-base"
    dockerfile = (REPO / "Dockerfile.runtime").read_text()
    if dockerfile.count(marker) != 1:
        raise ValueError("runtime packaging boundary changed")
    (context / "Dockerfile").write_text("# syntax=docker/dockerfile:1\nARG RUNTIME_IMAGE=debian:bookworm-slim\n"
        "FROM scratch AS builder\nCOPY bin /out/bin/\n\n" + marker + dockerfile.split(marker)[1])
    print(f"Cargo state build completed ({result}); artifacts still require sealing and testing", flush=True)


def archive(work):
    emit("GITHUB_OUTPUT", upload="false")
    plan = json.loads((work / "plan.json").read_text())
    built = json.loads((work / "built.json").read_text())
    if built["source"] != plan["source"] or built["compatibility"] != plan["compatibility"]:
        raise ValueError("cannot save state from a different build")
    # Restore and save use the same path (Actions includes paths in cache version).
    # The build has already moved the restored archive aside as evidence.
    output = work / "transport.tar.gz"
    if output.exists():
        raise ValueError("cache upload file already exists")
    checked("tar", "--format=posix", "-I", "gzip -1", "-cf", str(output), "-C", str(work), "state")
    size = output.stat().st_size
    print(f"Cargo state archive: {size} bytes (limit {MAX_ARCHIVE})", flush=True)
    if size <= MAX_ARCHIVE:
        emit("GITHUB_OUTPUT", upload="true")


if __name__ == "__main__":
    # Let the run() finally block remove its exact owned container on cancellation.
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(SystemExit(143)))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("prepare", "build", "archive"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.operation == "build":
        if args.output is None:
            parser.error("build requires --output")
        build(args.directory.resolve(), args.output.resolve())
    else:
        globals()[args.operation](args.directory.resolve())
