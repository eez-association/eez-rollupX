#!/usr/bin/env python3
"""Small opt-in Cargo target transport probe; never builds the real workspace.

Run seed and restore in distinct job-private directories. Only state/ travels
between jobs. Raw restore recreates sources normally; freshness compares it with
checksum synchronization of a cached source snapshot. This fixture-only
experiment does not authorize using a source snapshot as production authority.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import uuid


RUST_IMAGE = "rust@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f"
MANIFEST = '[package]\nname = "cargo-cache-probe"\nversion = "0.1.0"\nedition = "2021"\n'
LOCK = 'version = 4\n\n[[package]]\nname = "cargo-cache-probe"\nversion = "0.1.0"\n'
BUILD = ("cargo", "build", "--release", "--locked", "--offline", "--message-format=json")
TEST = ("cargo", "test", "--release", "--locked", "--offline")
BUILD_SCRIPT = '''fn main() {
    println!("cargo:rerun-if-changed=config");
    println!("cargo:rerun-if-env-changed=PROBE_SETTING");
    let mut files: Vec<_> = std::fs::read_dir("config").unwrap()
        .map(|entry| entry.unwrap().path()).collect();
    files.sort();
    let value = files.iter().map(|path| std::fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>().join("+");
    println!("cargo:rustc-env=PROBE_CONFIG={value}");
    println!("cargo:rustc-env=PROBE_ENV={}", std::env::var("PROBE_SETTING").unwrap_or_default());
}
'''
SEED_OUTPUT = "seed:module:data:config:"


def source(version="seed", failing=False):
    return ('mod value;\n'
            f'fn main() {{ println!("{version}:{{}}:{{}}:{{}}:{{}}", '
            'value::VALUE, include_str!("data.txt"), env!("PROBE_CONFIG"), env!("PROBE_ENV")); }\n'
            '#[cfg(test)] mod tests {\n'
            '    #[test] fn rejects_stale_test_cache() {\n'
            f'        assert_eq!(1, {2 if failing else 1});\n'
            '    }\n}\n')


def create_source(src):
    src.mkdir()
    (src / "src").mkdir()
    (src / "config").mkdir()
    for name, contents in {"Cargo.toml": MANIFEST, "Cargo.lock": LOCK,
                           "src/main.rs": source(), "src/value.rs": 'pub const VALUE: &str = "module";\n',
                           "src/data.txt": "data", "config/value": "config",
                           "build.rs": BUILD_SCRIPT}.items():
        (src / name).write_text(contents)


def sync_source(checkout, destination, completed_ns):
    """Only for owned fixture trees; current checkout is authoritative.

    Do not use -a/--times: that would refresh unchanged files' timestamps from
    checkout. --checksum detects same-size changes even with backdated mtimes.
    Refuse a cache from the future, otherwise newly copied files might appear
    older than Cargo's cached build metadata. No process may edit either tree
    during synchronization/build. Production generalization is out of scope.
    """
    if time.time_ns() <= completed_ns:
        raise ValueError("cache clock is ahead of this runner; cannot safely reuse it")
    if checkout.resolve() == destination.resolve():
        raise ValueError("checkout and cached source must be distinct")
    subprocess.run(["rsync", "--recursive", "--checksum", "--delete", "--perms", "--links",
                    "--", str(checkout) + "/", str(destination) + "/"], check=True)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def artifacts(stdout):
    records = []
    for line in stdout.splitlines():
        if line.startswith("{"):
            record = json.loads(line)
            if record.get("reason") == "compiler-artifact":
                records.append(record)
    if not records:
        raise ValueError("Cargo reported no compiler artifacts")
    return records


def container(work, *command, setting=None):
    # Mount only the fixture directory, never the checkout, Docker socket,
    # shared CARGO_HOME, operator credentials or live service state.
    return ["docker", "run", "--rm", "--pull=never", "--platform=linux/amd64",
            "--network=none", "--read-only", "--cap-drop=ALL",
            "--security-opt=no-new-privileges", "--cgroupns=private",
            "--cpus=1", "--memory=1g", "--memory-swap=1g", "--pids-limit=256",
            "--user", f"{os.getuid()}:{os.getgid()}", "--tmpfs", "/tmp:rw,size=128m,mode=1777",
            "--mount", f"type=bind,src={work},dst=/probe", "--workdir=/probe/source",
            "--env=CARGO_TARGET_DIR=/probe/state/target", "--env=CARGO_HOME=/tmp/cargo",
            "--env=CARGO_INCREMENTAL=0", "--env=CARGO_BUILD_JOBS=1",
            "--env=CARGO_LOG=cargo::core::compiler::fingerprint=info",
            "--entrypoint=/bin/sh", "--name", "eez-cargo-probe-" + uuid.uuid4().hex,
            *([] if setting is None else ["--env=PROBE_SETTING=" + setting]), RUST_IMAGE, "-ec",
            'test "$(cat /sys/fs/cgroup/cpu.max)" = "100000 100000"; '
            'test "$(cat /sys/fs/cgroup/memory.max)" = "1073741824"; '
            'test "$(cat /sys/fs/cgroup/memory.swap.max)" = "0"; exec "$@"',
            "probe", *command]


def run(work, label, *command, expected=0, setting=None):
    args = container(work, *command, setting=setting)
    name = args[args.index("--name") + 1]
    started = time.monotonic()
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=180)
    finally:
        # Also removes this exact owned container if the client timed out.
        subprocess.run(["docker", "rm", "--force", name], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=20, check=False)
    (work / f"{label}.stdout.log").write_text(result.stdout)
    (work / f"{label}.stderr.log").write_text(result.stderr)
    if result.returncode != expected:
        raise RuntimeError(f"{label}: exit {result.returncode}, expected {expected}; see {work}")
    return result, round(time.monotonic() - started, 3)


def probe(mode, work):
    work = work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    # Refuse a reused source directory, even when state was restored successfully.
    src = work / "source"
    create_source(src)
    main = src / "src/main.rs"
    state = work / "state"
    if mode == "seed":
        state.mkdir()  # Refuse an accidentally warm seed.
        shutil.copytree(src, state / "source")
    elif not (state / "seed.json").is_file():
        raise ValueError("restore requires the seed job's state; no cold-build fallback")
    binary = state / "target/release/cargo-cache-probe"
    report = {"mode": mode, "image": RUST_IMAGE,
              "runner": os.environ.get("RUNNER_NAME", "local")}
    if mode == "restore":
        seed = json.loads((state / "seed.json").read_text())
        if seed["image"] != RUST_IMAGE or digest(binary) != seed["binary_sha256"]:
            raise ValueError("restored seed binary/image mismatch")
        report["seed_runner"] = seed["runner"]
        report["restored_binary_verified"] = True
    result, report["build_seconds"] = run(work, "initial-build", *BUILD)
    fresh = all(item["fresh"] for item in artifacts(result.stdout))
    report["restored_all_fresh" if mode == "restore" else "cold_all_fresh"] = fresh
    if mode == "seed" and fresh:
        raise ValueError("cold seed unexpectedly reused compilation")
    result, report["same_job_warm_seconds"] = run(work, "same-job-warm", *BUILD)
    if not all(item["fresh"] for item in artifacts(result.stdout)):
        raise ValueError("unchanged same-job repeat recompiled the fixture")
    result, _ = run(work, "seed-execution", "/probe/state/target/release/cargo-cache-probe")
    if result.stdout.strip() != SEED_OUTPUT:
        raise ValueError("seed executable has wrong behavior")
    result, _ = run(work, "passing-test", *TEST)
    if "1 passed; 0 failed" not in result.stdout:
        raise ValueError("expected fixture test did not execute")
    if mode == "seed":
        report["binary_sha256"] = digest(binary)
        report["completed_ns"] = time.time_ns()
        (state / "seed.json").write_text(json.dumps(report, indent=2) + "\n")
    else:
        old_hash = digest(binary)
        main.write_text(source("changed"))
        result, report["changed_build_seconds"] = run(work, "changed-build", *BUILD)
        if all(item["fresh"] for item in artifacts(result.stdout)) or digest(binary) == old_hash:
            raise ValueError("source edit did not rebuild the executable")
        result, _ = run(work, "changed-execution", "/probe/state/target/release/cargo-cache-probe")
        if result.stdout.strip() != SEED_OUTPUT.replace("seed:", "changed:", 1):
            raise ValueError("source edit did not reach executable behavior")
        main.write_text(source("changed", failing=True))
        result, _ = run(work, "failing-test", *TEST, expected=101)
        if "tests::rejects_stale_test_cache ... FAILED" not in result.stdout:
            raise ValueError("expected failing test was not executed")
        report["source_and_test_invalidation_verified"] = True
    (work / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as stream:
            stream.write("\n### Cargo cache transport probe\n\n```json\n"
                         + json.dumps(report, indent=2) + "\n```\n")


def freshness(work):
    """Each case starts from an independent copy of exactly the same seed."""
    work = work.resolve()
    state = work / "state"
    seed = json.loads((state / "seed.json").read_text())
    if seed["image"] != RUST_IMAGE or digest(state / "target/release/cargo-cache-probe") != seed["binary_sha256"]:
        raise ValueError("seed binary/image mismatch")
    cases = ("unchanged", "main", "same-size-backdated", "module", "include",
             "build-input", "added-input", "deleted-input", "build-script", "manifest",
             "environment", "deleted-module", "renamed-module", "failing-test")
    results = {}
    for case in cases:
        root = work / case
        root.mkdir()  # Never overwrite an earlier result.
        checkout = root / "checkout"
        create_source(checkout)
        expected = SEED_OUTPUT
        if case in ("main", "same-size-backdated"):
            (checkout / "src/main.rs").write_text(source("edit"))
            expected = expected.replace("seed:", "edit:", 1)
            if case == "same-size-backdated":
                old = (state / "source/src/main.rs").stat().st_mtime_ns
                os.utime(checkout / "src/main.rs", ns=(old, old))
        elif case == "module":
            (checkout / "src/value.rs").write_text('pub const VALUE: &str = "edited";\n')
            expected = expected.replace(":module:", ":edited:")
        elif case == "include":
            (checkout / "src/data.txt").write_text("edit")
            expected = expected.replace(":data:", ":edit:")
        elif case == "build-input":
            (checkout / "config/value").write_text("edited")
            expected = expected.replace(":config:", ":edited:")
        elif case == "added-input":
            (checkout / "config/extra").write_text("extra")
            expected = expected.replace(":config:", ":extra+config:")
        elif case == "deleted-input":
            (checkout / "config/value").unlink()
            expected = expected.replace(":config:", "::")
        elif case == "build-script":
            (checkout / "build.rs").write_text(BUILD_SCRIPT.replace('join("+")', 'join("+") + "edited"'))
            expected = expected.replace(":config:", ":configedited:")
        elif case == "manifest":
            (checkout / "Cargo.toml").write_text(MANIFEST + '\n[features]\ndefault = ["extra"]\nextra = []\n')
        elif case == "environment":
            expected += "edited"
        elif case == "deleted-module":
            (checkout / "src/value.rs").unlink()
        elif case == "renamed-module":
            (checkout / "src/value.rs").rename(checkout / "src/other.rs")
        elif case == "failing-test":
            (checkout / "src/main.rs").write_text(source(failing=True))
        shutil.copytree(state, root / "state")
        shutil.copytree(state / "source", root / "source")
        sync_source(checkout, root / "source", seed["completed_ns"])
        setting = "edited" if case == "environment" else None
        if case in ("deleted-module", "renamed-module"):
            result, seconds = run(root, "build", *BUILD, expected=101)
            if 'file not found for module' not in result.stdout or 'value' not in result.stdout:
                raise ValueError(f"{case}: expected missing module diagnostic")
            results[case] = {"missing_module_rejected": True, "seconds": seconds}
        elif case == "failing-test":
            result, seconds = run(root, "test", *TEST, expected=101)
            if "tests::rejects_stale_test_cache ... FAILED" not in result.stdout:
                raise ValueError("restored cache concealed the changed failing test")
            results[case] = {"changed_test_failed": True, "seconds": seconds}
        else:
            result, seconds = run(root, "build", *BUILD, setting=setting)
            all_fresh = all(item["fresh"] for item in artifacts(result.stdout))
            if all_fresh != (case == "unchanged"):
                raise ValueError(f"{case}: unexpected all_fresh={all_fresh}")
            result, _ = run(root, "execution", "/probe/state/target/release/cargo-cache-probe")
            if result.stdout.strip() != expected:
                raise ValueError(f"{case}: stale executable behavior {result.stdout!r}")
            results[case] = {"all_fresh": all_fresh, "behavior_verified": True, "seconds": seconds}
            if case == "unchanged":
                result, _ = run(root, "cached-test", *TEST, "--message-format=json")
                if (not all(item["fresh"] for item in artifacts(result.stdout))
                        or "1 passed; 0 failed" not in result.stdout):
                    raise ValueError("unchanged test artifact not reused/executed")
                results[case]["test_reuse_verified"] = True
            elif case == "main":
                # Revert after rebuilding: sync against the CURRENT build tree,
                # not just the seed's hash/mtime, or edited artifacts can survive.
                (checkout / "src/main.rs").write_text(source())
                sync_source(checkout, root / "source", seed["completed_ns"])
                result, _ = run(root, "reverted-build", *BUILD)
                if all(item["fresh"] for item in artifacts(result.stdout)):
                    raise ValueError("revert did not rebuild the changed executable")
                result, _ = run(root, "reverted-execution", "/probe/state/target/release/cargo-cache-probe")
                if result.stdout.strip() != SEED_OUTPUT:
                    raise ValueError("revert retained changed executable behavior")
                results[case]["revert_verified"] = True
        (root / "result.json").write_text(json.dumps(results[case], indent=2) + "\n")
        print(case + ": " + json.dumps(results[case]), flush=True)
    (work / "freshness-result.json").write_text(json.dumps(results, indent=2) + "\n")
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as stream:
            stream.write("\n### Checksum-synchronized source reuse (fixture only)\n\n```json\n"
                         + json.dumps(results, indent=2) + "\n```\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("seed", "restore", "freshness"))
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.mode == "freshness":
        freshness(args.directory)
    else:
        probe(args.mode, args.directory)
