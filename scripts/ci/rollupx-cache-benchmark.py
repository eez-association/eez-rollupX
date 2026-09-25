#!/usr/bin/env python3
"""Isolated shipping-build sizing/reuse experiment, not a release producer.

Only a capped archive of owned Cargo state travels between jobs. No production
cache, image tag, runtime artifact manifest or deployment is updated.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import time
import uuid

REPO = Path(__file__).resolve().parents[2]
INPUTS = ("Cargo.toml", "Cargo.lock", "crates", "contracts/test/PostBatchGasPins.t.sol",
          ".config/nextest.toml")
BUILD = ("cargo", "build", "--locked", "--timings", "--profile", "release",
         "-p", "eez-node", "-p", "eez-follower", "-p", "eez-proof-signer",
         "-p", "eez-protocol", "-p", "eez-l1", "--bin", "eez-composer",
         "--bin", "eez-follower", "--bin", "eez-proof-signer",
         "--example", "genesis_state_root", "--example", "genesis_block_hash",
         "--example", "e2e-causal-verify", "--example", "verify_blob_evidence")
BINARIES = ("eez-composer", "eez-follower", "eez-proof-signer",
            "examples/genesis_state_root", "examples/genesis_block_hash",
            "examples/e2e-causal-verify", "examples/verify_blob_evidence")
MAX_ARCHIVE = 4 * 1024**3


def output(*args):
    return subprocess.check_output(args, cwd=REPO, text=True).strip()


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def stage_source(destination):
    # Exact tracked inputs only: never copy .git, credentials, host caches or
    # generated/untracked files. Matches this shipping build's Docker inputs.
    subprocess.run(["git", "diff", "--exit-code", "HEAD", "--", *INPUTS],
                   cwd=REPO, check=True, stdout=subprocess.DEVNULL)
    names = output("git", "ls-files", "-z", "--", *INPUTS).rstrip("\0").split("\0")
    destination.mkdir()
    inventory = {}
    for name in names:
        src = REPO / name
        if src.is_symlink() or not src.is_file():
            raise ValueError(f"source must be a regular tracked file: {name}")
        dst = destination / name
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
        inventory[name] = [digest(dst), dst.stat().st_mode & 0o777]
    return hashlib.sha256(json.dumps(inventory, sort_keys=True).encode()).hexdigest()


def container(work, image, command, network=False):
    return ["docker", "run", "--rm", "--pull=never", "--platform=linux/amd64",
            "--name", "eez-real-cache-" + uuid.uuid4().hex,
            "--network=" + ("bridge" if network else "none"), "--read-only",
            "--cap-drop=ALL", "--security-opt=no-new-privileges", "--cgroupns=private",
            "--cpus=4", "--memory=20g", "--memory-swap=20g", "--pids-limit=1024",
            "--user", f"{os.getuid()}:{os.getgid()}", "--tmpfs", "/tmp:rw,size=1g,mode=1777",
            "--mount", f"type=bind,src={work},dst=/probe",
            "--workdir=/probe/state/source", "--env=CARGO_HOME=/probe/state/cargo",
            "--env=CARGO_TARGET_DIR=/probe/state/target", "--env=CARGO_INCREMENTAL=0",
            "--env=CARGO_BUILD_JOBS=4", "--env=RUSTFLAGS=-C link-arg=-fuse-ld=mold",
            "--env=CARGO_LOG=cargo::core::compiler::fingerprint=info",
            "--entrypoint=/bin/sh", image, "-ec",
            'test "$(cat /sys/fs/cgroup/cpu.max)" = "400000 100000"; '
            'test "$(cat /sys/fs/cgroup/memory.max)" = "21474836480"; '
            'test "$(cat /sys/fs/cgroup/memory.swap.max)" = "0"; '
            'set +e; "$@"; result=$?; '
            'printf "\\nEEZ_BENCH_MEMORY_PEAK_BYTES=%s\\n" "$(cat /sys/fs/cgroup/memory.peak)" >&2; '
            'cat /sys/fs/cgroup/memory.events >&2; exit "$result"', "benchmark", *command]


def diagnostics(work, label):
    # Bounded tails are available even when GitHub's artifact quota is full.
    # Prefix and JSON-escape each line so output cannot inject workflow commands.
    for channel in ("stdout", "stderr"):
        path = work / f"{label}.{channel}.log"
        with path.open("rb") as stream:
            stream.seek(max(0, path.stat().st_size - 16384))
            tail = stream.read().decode(errors="replace")
        for line in tail.splitlines()[-40:]:
            print(f"{label} {channel}: {json.dumps(line)}", flush=True)


def run(work, image, label, command, network=False, *, expected_code=0, timeout=5400):
    args = container(work, image, command, network)
    name = args[args.index("--name") + 1]
    started = time.monotonic()
    print(f"Starting {label}", flush=True)
    code = None
    try:
        with (work / f"{label}.stdout.log").open("w") as stdout, \
                (work / f"{label}.stderr.log").open("w") as stderr:
            code = subprocess.run(args, stdout=stdout, stderr=stderr, timeout=timeout).returncode
    except subprocess.TimeoutExpired:
        pass  # Preserve timing and diagnostics before reporting the timeout.
    finally:
        subprocess.run(["docker", "rm", "--force", name], timeout=20,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    metric = {"seconds": round(time.monotonic() - started, 3), "exit_code": code,
              "timed_out": code is None}
    match = re.search(r"EEZ_BENCH_MEMORY_PEAK_BYTES=(\d+)", (work / f"{label}.stderr.log").read_text())
    if match:
        metric["container_memory_peak_bytes"] = int(match[1])
    (work / f"{label}.json").write_text(json.dumps(metric, indent=2) + "\n")
    print(label + ": " + json.dumps(metric), flush=True)
    if code != expected_code:
        diagnostics(work, label)
        raise RuntimeError(f"{label} failed; see preserved logs in {work}")
    return metric


def smoke_cli(work, image):
    for name in BINARIES[:3]:
        cases = [("help", ["--help"], 0, "Usage:")]
        if name != "eez-proof-signer":
            cases += [("short-help", ["-h"], 0, "Usage:"),
                      ("version", ["--version"], 0, "Version:"),
                      ("short-version", ["-V"], 0, "Reth "),
                      ("node-help", ["node", "--help"], 0, "Usage:")]
        cases += [("invalid", ["--invalid-cli-fixture"], 2,
                   "unexpected argument '--invalid-cli-fixture'")]
        for suffix, argv, expected_code, expected_text in cases:
            label = name + "-" + suffix
            run(work, image, label, ["/probe/state/target/release/" + name, *argv],
                expected_code=expected_code, timeout=60)
            channel = "stderr" if expected_code else "stdout"
            if expected_text not in (work / f"{label}.{channel}.log").read_text():
                diagnostics(work, label)
                raise ValueError(f"{label}: missing expected CLI output on {channel}")


def write_report(work, report):
    (work / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2), flush=True)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as stream:
            stream.write("\n### Shipping cache benchmark (not full CI)\n\n```json\n"
                         + json.dumps(report, indent=2) + "\n```\n")


def freshness(log):
    artifacts = [json.loads(line) for line in log.read_text().splitlines() if line.startswith("{")]
    artifacts = [item for item in artifacts if item.get("reason") == "compiler-artifact"]
    if not artifacts:
        raise ValueError("Cargo produced no compilation evidence")
    return {"artifacts": len(artifacts), "fresh": sum(item["fresh"] for item in artifacts),
            "all_fresh": all(item["fresh"] for item in artifacts)}


def hashes(state):
    return {name: digest(state / "target/release" / name) for name in BINARIES}


def check_archive_size(archive):
    size = archive.stat().st_size
    if not 0 < size <= MAX_ARCHIVE:
        raise ValueError(f"archive {size} bytes exceeds the 4 GiB experiment budget; do not upload")
    return size


def benchmark(mode, work, image):
    prepared_at = time.monotonic()
    work = work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(work).free < 30 * 1024**3:
        raise ValueError("benchmark needs at least 30 GiB free; no automatic cleanup")
    state = work / "state"
    identity = {"commit": output("git", "rev-parse", "HEAD"),
                "protocol": output("git", "ls-tree", "HEAD", "eez-core-protocol").split()[2],
                "source": stage_source(work / "checkout"), "build": list(BUILD)}
    if mode == "seed":
        state.mkdir()  # Fail if accidentally warm.
        shutil.copytree(work / "checkout", state / "source")
        (state / "cargo").mkdir()
        (state / "target").mkdir()
    else:
        if state.exists():
            raise ValueError("restore requires a fresh job-local state directory")
        check_archive_size(work / "state.tar.gz")
        with tarfile.open(work / "state.tar.gz", "r:gz") as archive:
            for member in archive.getmembers():
                parts = Path(member.name).parts
                if not parts or parts[0] != "state" or ".." in parts:
                    raise ValueError("cache archive contains paths outside state")
            archive.extractall(work, filter="data")
        seed = json.loads((state / "seed.json").read_text())
        if seed["identity"] != identity or seed["hashes"] != hashes(state):
            raise ValueError("cache identity or shipping executable hash mismatch")
        if time.time_ns() <= seed["completed_ns"]:
            raise ValueError("cache is from the future; refuse unsafe timestamp reuse")
        subprocess.run(["rsync", "--recursive", "--checksum", "--delete", "--perms", "--links",
                        "--", str(work / "checkout") + "/", str(state / "source") + "/"], check=True)
    report = {"mode": mode, "identity": identity, "runner": os.environ.get("RUNNER_NAME", "local"),
              "prepare_seconds": round(time.monotonic() - prepared_at, 3)}
    run(work, image, "environment", ["sh", "-ec",
        "rustc -Vv; cargo -V; mold --version; dpkg-query -W"])
    environment = (work / "environment.stdout.log").read_text()
    if mode == "restore" and seed["environment"] != environment:
        raise ValueError("toolchain/native packages differ between jobs")
    if mode == "seed":
        report["fetch"] = run(work, image, "fetch", ["cargo", "fetch", "--locked"], network=True)
    report["build"] = run(work, image, "build", [*BUILD, "--offline", "--message-format=json"])
    report["reuse"] = freshness(work / "build.stdout.log")
    if mode == "seed" and report["reuse"]["all_fresh"]:
        raise ValueError("cold build unexpectedly fresh")
    # A restore may recompile: report that honestly, never treat cache arrival
    # as proof of reuse. Hashes must still be identical for this unchanged case.
    report["hashes"] = hashes(state)
    if mode == "restore" and report["hashes"] != seed["hashes"]:
        raise ValueError("unchanged restored build changed shipping executable bytes")
    report["target_bytes"] = int(output("du", "-sb", str(state / "target")).split()[0])
    report["cargo_home_bytes"] = int(output("du", "-sb", str(state / "cargo")).split()[0])
    report["status"] = "build measured; CLI checks and cache preparation pending"
    write_report(work, report)
    smoke_cli(work, image)
    if mode == "seed":
        (state / "seed.json").write_text(json.dumps({"identity": identity, "environment": environment,
            "hashes": report["hashes"], "completed_ns": time.time_ns()}, indent=2) + "\n")
        started = time.monotonic()
        subprocess.run(["tar", "--format=posix", "-I", "gzip -1", "-cf", str(work / "state.tar.gz"),
                        "-C", str(work), "state"], check=True)
        report["archive_seconds"] = round(time.monotonic() - started, 3)
        report["archive_bytes"] = (work / "state.tar.gz").stat().st_size
    if mode == "seed":
        check_archive_size(work / "state.tar.gz")
    report["status"] = "complete"
    write_report(work, report)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("seed", "restore"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("--image", required=True)
    args = parser.parse_args()
    benchmark(args.mode, args.directory, args.image)
