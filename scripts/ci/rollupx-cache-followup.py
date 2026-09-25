#!/usr/bin/env python3
"""Bounded follow-up to the successful shipping cache experiment, never CI publication."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import time


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


bench = load("shipping_benchmark", "rollupx-cache-benchmark.py")
native = load("native_tests", "native-tests.py")
SEED_COMMIT = "5a097a19057a86dd3f8188fa0c0eb72903b5ae75"
TEST_FILE = "crates/eez-composer/src/settlement_policy/tests.rs"
PRODUCTION_FILE = "crates/eez-proof-signer/src/config.rs"
CANARY = "eez_cache_probe_rejects_stale_test"
TEST_EDIT = f'\n#[test]\nfn {CANARY}() {{ panic!("EEZ_CACHE_PROBE_NEW_TEST"); }}\n'
ABOUT = "EEZ window validation and attestation daemon"
MARKER = "EEZ_CACHE_PROBE_CHANGED_SIGNER"


def extract(work):
    if (work / "state").exists():
        raise ValueError("follow-up requires fresh job-local state")
    bench.check_archive_size(work / "state.tar.gz")
    with tarfile.open(work / "state.tar.gz", "r:gz") as archive:
        for member in archive.getmembers():
            parts = Path(member.name).parts
            if not parts or parts[0] != "state" or ".." in parts:
                raise ValueError("cache archive contains paths outside state")
        archive.extractall(work, filter="data")


def check_identity(seed, identity, expected_commit):
    expected = dict(identity, commit=expected_commit)
    if seed["identity"] != expected:
        raise ValueError("cache source/protocol/build identity mismatch")
    if time.time_ns() <= seed["completed_ns"]:
        raise ValueError("cache is from the future")


def compile_phase(work, image, label, command):
    metric = bench.run(work, image, label, [*command, "--offline", "--message-format=json"])
    metric["reuse"] = bench.freshness(work / f"{label}.stdout.log")
    print(label + ": " + json.dumps(metric), flush=True)
    return metric


def test_executables(work, label):
    result = {}
    for line in (work / f"{label}.stdout.log").read_text().splitlines():
        if not line.startswith("{"):
            continue
        record = json.loads(line)
        if record.get("reason") != "compiler-artifact" or not record["profile"]["test"]:
            continue
        if filename := record.get("executable"):
            relative = Path(filename).relative_to("/probe/state/target")
            if ".." in relative.parts:
                raise ValueError("test executable outside target")
            result[str(relative)] = bench.digest(work / "state/target" / relative)
    if not result:
        raise ValueError("no release test executables selected")
    return result


def frozen_hashes(work):
    return {name: bench.digest(work / "state/shipping" / name) for name in bench.BINARIES}


def freeze_shipping(work):
    for name in bench.BINARIES:
        destination = work / "state/shipping" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(work / "state/target/release" / name, destination)
    frozen = work / "state/target/release/eez-runtime"
    frozen.mkdir(exist_ok=True)
    for name in native.RUNTIME_BINARIES:
        shutil.copy2(work / "state/shipping" / name, frozen / name)


def archive_phase(work, image, label, name, selection):
    # Match nextest's Cargo compilation selection while retaining machine-readable
    # freshness evidence. Packaging immediately afterwards reuses that variant.
    metric = compile_phase(work, image, label,
        ["cargo", "test", "--release", "--locked", "--timings", "--no-run", *selection])
    metric["executables"] = test_executables(work, label)
    metric["package"] = bench.run(work, image, label + "-package",
        ["cargo", "nextest", "archive", "--release", "--locked", "--offline", "--timings",
         *selection, "--profile", "ci-archive", "--zstd-level", "1",
         "--archive-file", f"/probe/{label}-{name}"])
    metric["archive_bytes"] = (work / f"{label}-{name}").stat().st_size
    # Exercise the archive consumer, not just existence of a .tar.zst file.
    extraction = work / (label + "-extracted")
    extraction.mkdir()
    bench.run(work, image, label + "-extract",
        ["cargo-nextest", "nextest", "list", "--archive-file", f"/probe/{label}-{name}",
         "--extract-to", f"/probe/{label}-extracted", "--workspace-remap", "/probe/state/source",
         "--list-type", "binaries-only", "--message-format", "json"])
    for path, expected in metric["executables"].items():
        if bench.digest(extraction / "target" / path) != expected:
            raise ValueError("archived test executable differs from compiler output")
    for name in native.RUNTIME_BINARIES:
        if bench.digest(extraction / "target/release/eez-runtime" / name) != frozen_hashes(work)[name]:
            raise ValueError("archive changed frozen shipping binary")
    print(label + ": " + json.dumps(metric), flush=True)
    return metric


def mutations(work, image, report):
    original = bench.hashes(work / "state")
    source = work / "state/source"
    tests = source / TEST_FILE
    tests.write_text(tests.read_text() + TEST_EDIT)
    report["test_only"] = compile_phase(work, image, "test-only", bench.BUILD)
    if not report["test_only"]["reuse"]["all_fresh"] or bench.hashes(work / "state") != original:
        raise ValueError("test-only module edit invalidated shipping artifacts")
    config = source / PRODUCTION_FILE
    text = config.read_text()
    if text.count(ABOUT) != 1:
        raise ValueError("production mutation anchor changed")
    config.write_text(text.replace(ABOUT, MARKER))
    report["production_edit"] = compile_phase(work, image, "production-edit", bench.BUILD)
    changed = bench.hashes(work / "state")
    if report["production_edit"]["reuse"]["all_fresh"] or changed["eez-proof-signer"] == original["eez-proof-signer"]:
        raise ValueError("production edit did not invalidate the signer")
    bench.run(work, image, "changed-signer", ["/probe/state/target/release/eez-proof-signer", "--help"], timeout=60)
    if MARKER not in (work / "changed-signer.stdout.log").read_text():
        raise ValueError("production execution used stale signer")
    report["changed_binaries"] = [name for name in original if original[name] != changed[name]]
    # Mutations are confined to this disposable copy. Never export its target.


def followup(mode, work, image):
    work = work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(work).free < 50 * 1024**3:
        raise ValueError("follow-up needs 50 GiB free; no automatic cleanup")
    identity = {"commit": bench.output("git", "rev-parse", "HEAD"),
                "protocol": bench.output("git", "ls-tree", "HEAD", "eez-core-protocol").split()[2],
                "source": bench.stage_source(work / "checkout"), "build": list(bench.BUILD)}
    started = time.monotonic()
    extract(work)
    state = work / "state"
    restored_tests = mode == "archives-restore"
    seed = json.loads((state / ("followup.json" if restored_tests else "seed.json")).read_text())
    check_identity(seed, identity, identity["commit"] if restored_tests else SEED_COMMIT)
    if (frozen_hashes(work) if restored_tests else bench.hashes(state)) != seed["hashes"]:
        raise ValueError("restored shipping hashes differ")
    subprocess.run(["rsync", "--recursive", "--checksum", "--delete", "--perms", "--links",
                    "--", str(work / "checkout") + "/", str(state / "source") + "/"], check=True)
    report = {"mode": mode, "identity": identity, "seed_commit": seed["identity"]["commit"],
              "prepare_seconds": round(time.monotonic() - started, 3), "status": "incomplete"}
    try:
        bench.run(work, image, "environment", ["sh", "-ec", "rustc -Vv; cargo -V; mold --version; dpkg-query -W"])
        environment = (work / "environment.stdout.log").read_text()
        if seed["environment"] != environment:
            raise ValueError("toolchain/native packages changed since seed")
        report["shipping"] = compile_phase(work, image, "shipping", bench.BUILD)
        if bench.hashes(state) != seed["hashes"]:
            raise ValueError("unchanged shipping executable hashes differ")
        if mode == "mutations":
            mutations(work, image, report)
        else:
            bench.run(work, image, "nextest-version", ["cargo-nextest", "nextest", "--version"])
            if (work / "nextest-version.stdout.log").read_text().split()[1] != native.NEXTEST_VERSION:
                raise ValueError("wrong nextest version")
            freeze_shipping(work)
            report["archives"] = {}
            for index, (name, selection) in enumerate(native.ARCHIVES.items()):
                label = "native" if index == 0 else "composer"
                report["archives"][name] = archive_phase(work, image, label, name, selection)
                if restored_tests and report["archives"][name]["executables"] != seed["tests"][name]:
                    raise ValueError("restored test executable hashes differ")
                bench.write_report(work, report)
            report["target_bytes"] = int(bench.output("du", "-sb", str(state / "target")).split()[0])
            report["cargo_home_bytes"] = int(bench.output("du", "-sb", str(state / "cargo")).split()[0])
            if not restored_tests:
                (state / "followup.json").write_text(json.dumps({"identity": identity, "environment": environment,
                    "hashes": frozen_hashes(work), "tests": {name: item["executables"] for name, item in report["archives"].items()},
                    "completed_ns": time.time_ns()}, indent=2))
                started = time.monotonic()
                subprocess.run(["tar", "--format=posix", "-I", "gzip -1", "-cf", str(work / "test-state.tar.gz"),
                                "-C", str(work), "state"], check=True)
                report["archive_seconds"] = round(time.monotonic() - started, 3)
                report["archive_bytes"] = (work / "test-state.tar.gz").stat().st_size
                bench.check_archive_size(work / "test-state.tar.gz")
            else:
                # New failing test must be compiled and executed, not hidden by a
                # previously successful test archive. No E2E services are started.
                tests = state / "source" / TEST_FILE
                tests.write_text(tests.read_text() + TEST_EDIT)
                selection = native.ARCHIVES["composer-unit-tests.tar.zst"]
                report["test_edit"] = archive_phase(work, image, "test-edit", "composer-unit-tests.tar.zst", selection)
                if report["test_edit"]["reuse"]["all_fresh"]:
                    raise ValueError("new test did not invalidate test compilation")
                executables = report["test_edit"]["executables"]
                if len(executables) != 1:
                    raise ValueError("composer canary requires exactly one unit-test executable")
                executable = next(iter(executables))
                bench.run(work, image, "new-test-execution",
                    [f"/probe/test-edit-extracted/target/{executable}", "--exact",
                     f"settlement_policy::tests::{CANARY}", "--nocapture"], expected_code=101, timeout=60)
                stdout = (work / "new-test-execution.stdout.log").read_text()
                stderr = (work / "new-test-execution.stderr.log").read_text()
                if "1 failed" not in stdout or "EEZ_CACHE_PROBE_NEW_TEST" not in stderr:
                    raise ValueError("did not execute the newly added failing test")
        report["status"] = "complete"
    finally:
        bench.write_report(work, report)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("mutations", "archives-seed", "archives-restore"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("--image", required=True)
    args = parser.parse_args()
    followup(args.mode, args.directory, args.image)
