#!/usr/bin/env python3
"""Build once, then execute exact-revision nextest archives without Cargo/rustc.

This is a same-run executable artifact, never a persistent Cargo target cache.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile


REPO = Path(__file__).resolve().parents[2]
NEXTEST_VERSION = "0.9.143"
RUST_VERSION = "1.95.0"
RUSTFLAGS = "-C link-arg=-fuse-ld=mold"
RUNTIME_BINARIES = ("eez-composer", "eez-follower", "eez-proof-signer")
# Written only after the producer suite passes for this exact manifest. It gates
# the E2E export and the optional retained Cargo-state export; it says nothing
# about the debug unit lane, which ci.yml's gate requires separately.
PRODUCER_PASSED = "producer-passed.json"
# All node process tests are modules of one test binary: in the release test
# profile each separate target re-optimized and linked the whole node stack.
E2E_TARGET = "node_e2e"
EMBEDDED_MODULES = ("e2e", "settlement_policy", "settlement_policy_cost", "kurtosis_policy_replay")
CROSS_CHAIN_MODULES = (
    "cross_chain", "native_system", "derivable_operations", "chained_interstate", "partial_consumption",
    "settlement_policy_gas", "dynamic_admission", "queue_liveness", "queue_reorg", "fee_quotes", "safe_fee_quotes", "transaction_references")
E2E_MODULES = (*EMBEDDED_MODULES, *CROSS_CHAIN_MODULES)
E2E_BINARIES = {f"eez-node-e2e::{E2E_TARGET}"}
# Standalone protocol/codec regressions, keyed by test target, with their package.
VECTOR_TARGETS = {
    "public_inputs_hash_vectors": "eez-protocol", "rolling_hash_vectors": "eez-protocol",
    "causal_evidence": "eez-protocol", "ethereum_kzg_compatibility": "eez-blob-codec",
    "issue35": "eez-payload-codec",
}
TARGETS = [E2E_TARGET, *VECTOR_TARGETS]
# The release archive holds node_e2e (which runs the frozen shipping binaries),
# the protocol vectors and verify_blob_evidence's example tests. One Cargo
# invocation keeps their dependency feature set identical to the shipping
# build, so it reuses the release target's dependency artifacts; the build guard
# fails if any non-workspace crate is compiled. Keep the eez-l1 example here:
# without it, eez-node-e2e alone resolves another stateless/alloy feature set.
# Workspace lib/bin unit tests are excluded: eez-composer and eez-deriver
# dev-dependencies enable tokio/test-util, which rebuilt ~126 dependencies. They
# run in ci.yml's separate debug unit lane. Shipping bytes are unaffected: the
# producer copies the release candidate into release/eez-runtime first and
# rechecks its hashes after archiving.
RELEASE_PACKAGES = ("eez-node-e2e", "eez-l1", *dict.fromkeys(VECTOR_TARGETS.values()))
ARCHIVES = {
    "native-tests.tar.zst": [*[arg for package in RELEASE_PACKAGES for arg in ("-p", package)],
                             "--example", "verify_blob_evidence",
                             *[arg for target in TARGETS for arg in ("--test", target)]],
}


def e2e_modules(modules):
    """Nextest filter for tests in the given node_e2e modules."""
    tests = " | ".join(f"test(/^{module}::/)" for module in modules)
    return f"package(=eez-node-e2e) & binary(={E2E_TARGET}) & ({tests})"


SUITES = {
    # Release tests that run in the producer before the E2E payload is exported.
    "producer": " | ".join(["(package(=eez-l1) & binary(=verify_blob_evidence))",
                            *(f"(package(={package}) & binary(={target}))"
                              for target, package in VECTOR_TARGETS.items())]),
    # The Kurtosis module is archived too; its test stays explicitly ignored
    # here and is run by verify-policy-replay.sh against a live enclave.
    "embedded": e2e_modules(EMBEDDED_MODULES),
    "cross-chain": e2e_modules(CROSS_CHAIN_MODULES),
}


def output(*command):
    return subprocess.check_output(command, cwd=REPO, text=True).strip()


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def build_contract():
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("native test artifacts currently require Linux amd64")
    return {"schema": 4, "cargo_lock_sha256": digest(REPO / "Cargo.lock"),
            "platform": "linux/amd64", "profile": "release", "rustflags": RUSTFLAGS,
            "nextest": NEXTEST_VERSION, "rust": RUST_VERSION}


def check_nextest():
    if output("cargo-nextest", "nextest", "--version").split()[1] != NEXTEST_VERSION:
        raise ValueError(f"native test artifacts require nextest {NEXTEST_VERSION}")


def identity():
    root = output("git", "rev-parse", "HEAD")
    protocol = output("git", "-C", "eez-core-protocol", "rev-parse", "HEAD")
    if root != os.environ["GITHUB_SHA"] or protocol != output("git", "ls-files", "-s", "eez-core-protocol").split()[1]:
        raise ValueError("test artifacts require the workflow revision and pinned submodule")
    return dict(build_contract(), root_commit=root, protocol_commit=protocol)


def runtime_hashes(directory):
    for name in RUNTIME_BINARIES:
        path = directory / name
        if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError(f"missing executable release candidate: {path}")
    return {name: digest(directory / name) for name in RUNTIME_BINARIES}


def workspace_members():
    metadata = subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"],
                                       cwd=REPO, text=True)
    return set(json.loads(metadata)["workspace_members"])


def compiled_dependencies(lines, members):
    """Yield the package ID of every non-workspace unit Cargo compiled (fail closed).

    Non-JSON lines (wrapper/status output) are echoed. Missing fields count as
    compiled: a guard that cannot read a message must not pass it.
    """
    for line in lines:
        try:
            message = json.loads(line)
        except ValueError:
            message = None
        if not isinstance(message, dict):
            sys.stdout.write(line)
            continue
        if (message.get("reason") == "compiler-artifact" and message.get("fresh") is not True
                and message.get("package_id") not in members):
            yield str(message.get("package_id"))


def guarded_archive(command, members):
    """Run one archive invocation; fail if it compiled any non-workspace crate.

    The shipping build has already compiled every dependency of the release test
    graph with the same features. A dependency compiled here means a test-only
    feature (or a dev-only crate) entered that graph, which would silently spend
    most of the archive step rebuilding the dependency stack. Cargo's JSON goes
    to stdout; rendered diagnostics and progress stay on stderr.
    """
    with subprocess.Popen(command, cwd=REPO, stdout=subprocess.PIPE, text=True) as process:
        compiled = sorted(set(compiled_dependencies(process.stdout, members)))
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, command)
    if compiled:
        listed = "\n  ".join(compiled[:40]) + ("\n  ..." if len(compiled) > 40 else "")
        raise ValueError(f"release test archive compiled {len(compiled)} non-workspace crate(s); its dependency "
                         "feature set must equal the shipping build's. Run tests whose dev-dependencies enable "
                         f"test-only features in the debug unit lane instead:\n  {listed}")


def build(directory, runtime_bin):
    # Git metadata stays outside Docker's build context. The exact-revision host
    # producer seals this metadata after export; doc-only edits can reuse layers.
    manifest = build_contract()
    check_nextest()
    if output("rustc", "--version").split()[1] != RUST_VERSION:
        raise ValueError(f"native producer requires Rust {RUST_VERSION}")
    if os.environ.get("CARGO_INCREMENTAL") != "0":
        raise ValueError("CI test builds require CARGO_INCREMENTAL=0")
    if os.environ.get("RUSTFLAGS") != RUSTFLAGS:
        raise ValueError(f"release test builds require RUSTFLAGS={RUSTFLAGS}")
    if os.environ.get("CARGO_ENCODED_RUSTFLAGS"):
        raise ValueError("CARGO_ENCODED_RUSTFLAGS must not override the portable CI flags")
    if os.environ.get("CARGO_BUILD_TARGET"):
        raise ValueError("native test archives require the default host target layout")
    if any(key.startswith("CARGO_PROFILE_RELEASE_") for key in os.environ):
        raise ValueError("release profile overrides must not change the shipping configuration")
    directory.mkdir(parents=True, exist_ok=True)
    if any(directory.iterdir()):
        raise ValueError("native artifact output directory must be empty")
    hashes = runtime_hashes(runtime_bin)
    members = workspace_members()
    target = Path(os.environ["CARGO_TARGET_DIR"]).resolve()
    frozen = target / "release/eez-runtime"
    frozen.mkdir(parents=True, exist_ok=False)
    for name in RUNTIME_BINARIES:
        shutil.copy2(runtime_bin / name, frozen / name)
    command = ["bash", "scripts/ci/with-compiler-cache.sh"]
    for name, selection in ARCHIVES.items():
        args = ["cargo", "nextest", "archive", "--release", "--locked", "--timings", *selection,
                "--cargo-message-format", "json-render-diagnostics",
                "--profile", "ci-archive", "--zstd-level", "1", "--archive-file", str(directory / name)]
        guarded_archive(command + ["test-archive", *args], members)
        if runtime_hashes(frozen) != hashes or runtime_hashes(runtime_bin) != hashes:
            raise ValueError("test compilation modified the frozen release candidate")
        shutil.copy2(target / "cargo-timings/cargo-timing.html", directory / "cargo-timing.html")
    manifest.update(archives_sha256={name: digest(directory / name) for name in ARCHIVES}, runtime_sha256=hashes,
                    glibc=output("getconf", "GNU_LIBC_VERSION"))
    (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


def verify_archives(directory, manifest):
    hashes = manifest.get("archives_sha256")
    if not isinstance(hashes, dict) or set(hashes) != set(ARCHIVES):
        raise ValueError("native test archive inventory mismatch")
    for name in ARCHIVES:
        if digest(directory / name) != hashes[name]:
            raise ValueError(f"native test archive checksum mismatch: {name}")


def verify_e2e(directory, manifest):
    files = manifest.get("files_sha256")
    if not isinstance(files, dict) or not files:
        raise ValueError("missing E2E file inventory")
    for name, expected in files.items():
        relative = Path(name)
        if not relative.parts or relative.is_absolute() or ".." in relative.parts or relative.parts[0] != "target":
            raise ValueError("invalid E2E file path")
        path = directory / relative
        if path.is_symlink() or path.resolve() != path.absolute() or digest(path) != expected:
            raise ValueError(f"E2E file checksum/path mismatch: {name}")
    required = {"target/nextest/binaries-metadata.json", "target/nextest/cargo-metadata.json"}
    if not required.issubset(files):
        raise ValueError("E2E metadata missing from file inventory")
    metadata = json.loads((directory / "target/nextest/binaries-metadata.json").read_text())
    if set(metadata["rust-binaries"]) != E2E_BINARIES:
        raise ValueError("E2E binary inventory mismatch")
    original_target = Path(metadata["rust-build-meta"]["target-directory"])
    for binary in metadata["rust-binaries"].values():
        relative = Path(binary["binary-path"]).relative_to(original_target)
        required.add(str(Path("target") / relative))
    for entries in metadata["rust-build-meta"]["non-test-binaries"].values():
        required.update(str(Path("target") / entry["path"]) for entry in entries)
    required.update(f"target/release/eez-runtime/{name}" for name in RUNTIME_BINARIES)
    if not required.issubset(files):
        raise ValueError("E2E execution input missing from file inventory")
    if runtime_hashes(directory / "target/release/eez-runtime") != manifest["runtime_sha256"]:
        raise ValueError("E2E runtime differs from shipping candidate")


def export_e2e(directory, destination):
    """Keep only execution inputs; the producer suite has already passed here.

    Nextest supports manually transferred binaries/metadata. Do not recompile,
    reconstruct Cargo state, or introduce another compression/archive format.
    """
    manifest = json.loads((directory / "manifest.json").read_text())
    if json.loads((directory / PRODUCER_PASSED).read_text()) != manifest:
        raise ValueError("producer release tests must pass for this exact candidate before export")
    for key, expected in identity().items():
        if manifest.get(key) != expected:
            raise ValueError(f"native artifact mismatch: {key}")
    verify_archives(directory, manifest)
    source = Path(os.environ["CARGO_TARGET_DIR"]).resolve() / "native-tests/target"
    if runtime_hashes(source / "release/eez-runtime") != manifest["runtime_sha256"]:
        raise ValueError("extracted runtime differs from shipping candidate")
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError("E2E export directory must be empty")
    metadata = json.loads((source / "nextest/binaries-metadata.json").read_text())
    binaries = metadata["rust-binaries"]
    if not E2E_BINARIES.issubset(binaries):
        raise ValueError("E2E binary inventory is incomplete")
    original_target = Path(metadata["rust-build-meta"]["target-directory"])
    excluded = {Path(binary["binary-path"]).relative_to(original_target)
                for key, binary in binaries.items() if key not in E2E_BINARIES}
    metadata["rust-binaries"] = {key: value for key, value in binaries.items() if key in E2E_BINARIES}
    non_test = metadata["rust-build-meta"]["non-test-binaries"]
    for package, entries in non_test.items():
        non_test[package] = [entry for entry in entries if Path(entry["path"]).suffix not in (".rlib", ".a", ".o")]
        for entry in non_test[package]:
            if entry["kind"] == "bin-exe" and entry["name"] in RUNTIME_BINARIES:
                excluded.add(Path(entry["path"]))
                entry["path"] = f"release/eez-runtime/{entry['name']}"
    target = destination / "target"
    for path in sorted(source.rglob("*")):
        relative = path.relative_to(source)
        if path.is_symlink():
            raise ValueError(f"unexpected symlink in E2E export: {relative}")
        if relative in excluded or path.suffix in (".a", ".o", ".rlib"):
            continue
        output_path = target / relative
        if path.is_dir():
            output_path.mkdir(parents=True, exist_ok=True)
        elif path.is_file():
            output_path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, output_path)
        else:
            raise ValueError(f"unexpected E2E input: {relative}")
    (target / "nextest/binaries-metadata.json").write_text(json.dumps(metadata) + "\n")
    manifest.pop("archives_sha256")
    manifest.update(e2e_only=True, files_sha256={str(path.relative_to(destination)): digest(path)
                    for path in sorted(target.rglob("*")) if path.is_file()})
    verify_e2e(destination, manifest)
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"E2E payload: {len(manifest['files_sha256'])} files, "
          f"{sum(p.stat().st_size for p in target.rglob('*') if p.is_file()) / 1024**2:.1f} MiB uncompressed")


def seal(directory, runtime_archive):
    """Bind cached build outputs to this checkout and the packaged runtime bytes."""
    path = directory / "manifest.json"
    manifest = json.loads(path.read_text())
    for key, expected in build_contract().items():
        if manifest.get(key) != expected:
            raise ValueError(f"native artifact mismatch: {key}")
    verify_archives(directory, manifest)
    hashes = {}
    with tarfile.open(runtime_archive, "r:gz") as archive:
        for name in RUNTIME_BINARIES:
            members = [m for m in archive.getmembers() if m.name == f"bin/{name}"]
            if len(members) != 1 or not members[0].isfile():
                raise ValueError(f"invalid packaged release candidate: {name}")
            with archive.extractfile(members[0]) as stream:
                hashes[name] = hashlib.file_digest(stream, "sha256").hexdigest()
    if hashes != manifest.get("runtime_sha256"):
        raise ValueError("test runtime does not match packaged release binaries")
    manifest.update(identity(), runtime_archive_sha256=digest(runtime_archive))
    path.write_text(json.dumps(manifest, indent=2) + "\n")


def run(directory, suite, partition=None):
    if partition is not None and suite == "producer":
        raise ValueError("producer validation must run in full before exporting E2E inputs")
    if suite == "producer":
        (directory / PRODUCER_PASSED).unlink(missing_ok=True)
    check_nextest()
    manifest = json.loads((directory / "manifest.json").read_text())
    for key, expected in identity().items():
        if manifest.get(key) != expected:
            raise ValueError(f"native artifact mismatch: {key}")
    if manifest.get("e2e_only") is True:
        if suite == "producer":
            raise ValueError("producer-suite executables are not in the E2E payload")
        verify_e2e(directory, manifest)
    else:
        verify_archives(directory, manifest)
    runtime_digest = manifest.get("runtime_archive_sha256")
    if not isinstance(runtime_digest, str) or len(runtime_digest) != 64 or any(c not in "0123456789abcdef" for c in runtime_digest):
        raise ValueError("native artifact is not sealed to a packaged runtime")
    version = lambda value: tuple(int(n) for n in value.removeprefix("glibc ").split("."))
    if version(output("getconf", "GNU_LIBC_VERSION")) < version(manifest["glibc"]):
        raise ValueError("consumer glibc is older than the native test producer's")
    target = Path(os.environ["CARGO_TARGET_DIR"]).resolve()
    target.mkdir(parents=True, exist_ok=True)
    if any(target.iterdir()):
        raise ValueError("native archive extraction requires an empty job-local target")
    selected = ARCHIVES if suite == "producer" else ("native-tests.tar.zst",)
    failure = None
    for name in selected:
        # Each archive extracts to its own path. Never overlay archives or rely
        # on Cargo to reconcile their fingerprints.
        extraction = target / name.removesuffix(".tar.zst")
        extraction.mkdir()
        if manifest.get("e2e_only") is True:
            extracted = directory / "target"
        else:
            subprocess.run(["cargo-nextest", "nextest", "list", "--archive-file", str(directory / name),
                        "--extract-to", str(extraction), "--workspace-remap", str(REPO),
                        "--list-type", "binaries-only", "--message-format", "json"],
                       cwd=REPO, stdout=subprocess.DEVNULL, check=True)
            extracted = extraction / "target"
        frozen = extracted / "release/eez-runtime"
        if runtime_hashes(frozen) != manifest.get("runtime_sha256"):
            raise ValueError("extracted test runtime differs from the packaged release candidate")
        env = dict(os.environ, EEZ_TEST_PREBUILT="1", EEZ_TEST_RUNTIME_BIN_DIR=str(frozen),
                   EEZ_TEST_PROOF_SIGNER_BIN=str(frozen / "eez-proof-signer"))
        # Run the verified extraction directly, without Cargo or rustc.
        command = ["cargo-nextest", "nextest", "run",
                   "--cargo-metadata", str(extracted / "nextest/cargo-metadata.json"),
                   "--binaries-metadata", str(extracted / "nextest/binaries-metadata.json"),
                   "--target-dir-remap", str(extracted), "--workspace-remap", str(REPO),
                   "--no-fail-fast", "--no-tests=fail", "-E", SUITES[suite]]
        if suite != "producer":
            command.extend(["--profile", "ci", "--test-threads=1", "--color=never"])
        if partition is not None:
            command.extend(["--partition", partition])
        print(f"==> {suite}: {name}", flush=True)
        try:
            subprocess.run(command, cwd=REPO, env=env, check=True)
        except subprocess.CalledProcessError as error:
            # Preserve --no-fail-fast across archives as well.
            failure = failure or error
    if failure is not None:
        raise failure
    if suite == "producer":
        # Generated only by actual successful execution, never by a cached build.
        (directory / PRODUCER_PASSED).write_text(json.dumps(manifest) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["build", "seal", "run", "export-e2e"])
    parser.add_argument("directory", type=Path)
    parser.add_argument("--suite", choices=SUITES, default="producer")
    parser.add_argument("--partition", help="nextest partition for an E2E run, e.g. slice:1/4")
    parser.add_argument("--runtime-bin", type=Path)
    parser.add_argument("--runtime-archive", type=Path)
    parser.add_argument("--output-dir", type=Path)
    args = parser.parse_args()
    if args.partition is not None and (args.operation != "run" or args.suite == "producer"):
        parser.error("--partition is only supported for E2E runs")
    try:
        if args.operation == "build":
            if args.runtime_bin is None:
                parser.error("build requires --runtime-bin (frozen release executables)")
            build(args.directory.resolve(), args.runtime_bin.resolve())
        elif args.operation == "seal":
            if args.runtime_archive is None:
                parser.error("seal requires --runtime-archive")
            seal(args.directory.resolve(), args.runtime_archive.resolve())
        elif args.operation == "export-e2e":
            if args.output_dir is None:
                parser.error("export-e2e requires --output-dir")
            export_e2e(args.directory.resolve(), args.output_dir.resolve())
        else:
            run(args.directory.resolve(), args.suite, args.partition)
    except (ValueError, KeyError, OSError, tarfile.TarError, subprocess.CalledProcessError) as error:
        print(f"Native test artifacts: {error}", file=sys.stderr)
        return error.returncode if isinstance(error, subprocess.CalledProcessError) else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
