#!/usr/bin/env python3
"""Adapt the pinned parallel runner to CI's per-scenario evidence layout.

This is only scheduling/log collection: run-protocol-e2e.sh still owns scenario
discovery, strict result accounting and independent on-chain verification.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import time


RESULT = re.compile(r"^RESULT ([^:\s]+): ([A-Z]+)(?:\s.*)?$")
RUN_DIR = re.compile(r" — logs in (tmp/e2e-parallel-net/[0-9]{8}-[0-9]{6})$")


def positive_int(value):
    if not re.fullmatch(r"[1-9][0-9]*", value):
        raise argparse.ArgumentTypeError("must be a positive integer")
    return int(value)


def parallelism(value):
    return value if value == "all" else positive_int(value)


def stop_process_group(process):
    # Only descendants of this invocation, including workers left after timeout.
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


def collect_logs(protocol, logs, names, text, old_dirs):
    paths = [match.group(1) for line in text.splitlines() if (match := RUN_DIR.search(line))]
    if len(paths) != 1:
        raise ValueError("parallel runner did not report exactly one run directory")
    source = protocol / paths[0]
    if source.name in old_dirs or not source.is_dir() or source.is_symlink():
        raise ValueError("parallel run directory is stale, missing or a symlink")
    if source.resolve() != source:
        raise ValueError("scenario log directory or an ancestor is a symlink")
    for name in names:
        path = source / f"{name}-1.log"
        try:
            info = path.lstat()
        except FileNotFoundError:
            continue  # A reported PASS without a log will fail evidence collection.
        if not stat.S_ISREG(info.st_mode):
            raise ValueError(f"not a regular scenario log: {path}")
        shutil.copyfile(path, logs / f"{name}.log")
    # Deliberately do not copy wallets.csv, faucet/pool keys or broadcast files.


def print_failure_log(logs, name, env):
    path = logs / f"{name}.log"
    print(f"==> {name}: failure diagnostics (last 80 lines, at most 16 KiB)", flush=True)
    if not path.is_file():
        print(f"[{name}] No scenario log was produced", flush=True)
        return
    with path.open("rb") as stream:
        stream.seek(max(0, path.stat().st_size - 16384))
        tail = stream.read(16384).decode(errors="replace")
    if env.get("SOURCE_PK"):
        tail = tail.replace(env["SOURCE_PK"], "[REDACTED]")
    tail = re.sub(r"(?i)(--private-key\s+|\b(?:private[_-]?key|source_pk|pk)\s*=\s*)\S+",
                  r"\1[REDACTED]", tail)
    for line in tail.splitlines()[-80:]:
        # Prefix untrusted scenario output: a logged RESULT must not become a
        # second result in the outer wrapper's strict accounting stream.
        print(f"[{name}] {line}", flush=True)


def run_network(protocol, logs, names, env, deadline, phase="parallel"):
    started = time.monotonic()
    record = {"phase": phase, "scenarios": names, "exit_code": 124, "seconds": 0, "results": []}
    if started >= deadline:
        record["error"] = "suite deadline exceeded before phase start"
        return record
    old_dirs = {path.name for path in (protocol / "tmp/e2e-parallel-net").glob("*")}
    command = ["bash", "script/e2e/run/network-parallel.sh", "--fresh", *names]
    raw_log = logs / f"{phase}.log"
    print(f"==> protocol {phase}: {len(names)} scenarios (MAX_PARALLEL={env['MAX_PARALLEL']})", flush=True)
    with raw_log.open("x") as log:
        process = subprocess.Popen(command, cwd=protocol, env=env, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            record["exit_code"] = process.wait(timeout=max(0, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            record["error"] = "suite deadline exceeded; stopped this runner's process group"
        except SystemExit as error:
            record["exit_code"] = error.code
            record["error"] = "interrupted; stopped this runner's process group"
            record["interrupted"] = True
        finally:
            stop_process_group(process)
    output = raw_log.read_text(errors="replace")
    expected = {f"{name}-1": name for name in names}
    # Normalize only exact selected upstream job IDs. Unknown jobs must remain
    # unexpected, not collide with selected names or silently lose a '-1' suffix.
    for line in output.splitlines():
        match = RESULT.fullmatch(line)
        if match:
            job, verdict = match.groups()
            name = expected.get(job, f"unexpected-parallel/{job}")
            record["results"].append([name, verdict])
            print(f"RESULT {name}: {verdict}", flush=True)
        else:
            print(line, flush=True)
            if line.startswith("RESULT "):
                record["error"] = "malformed upstream RESULT line"
                record["exit_code"] = record["exit_code"] or 1
    try:
        collect_logs(protocol, logs, names, output, old_dirs)
    except ValueError as error:
        record["error"] = str(error)
        record["exit_code"] = record["exit_code"] or 1
    for name in names:
        if [r for r in record["results"] if r[0] == name] != [[name, "PASS"]]:
            print_failure_log(logs, name, env)
    record["seconds"] = round(time.monotonic() - started, 2)
    print(f"==> protocol {phase}: exit={record['exit_code']} elapsed={record['seconds']}s", flush=True)
    if record.get("error"):
        print(f"ERROR: {record['error']}", flush=True)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol-dir", required=True, type=Path)
    parser.add_argument("--env-file", required=True, type=Path)
    parser.add_argument("--log-dir", required=True, type=Path)
    parser.add_argument("--max-parallel", type=parallelism, default="all",
                        help="Concurrent scenarios, or 'all' for the selected inventory")
    parser.add_argument("--suite-timeout", type=positive_int, default=2400)
    parser.add_argument("scenarios", nargs="+")
    args = parser.parse_args()
    # Categories/count syntax is expanded by the caller. Accept only unique
    # exact basenames, so flags, path traversal and repetitions cannot sneak in.
    if len(set(args.scenarios)) != len(args.scenarios) or any(
            not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]*", name) for name in args.scenarios):
        parser.error("expected unique, exact scenario names")
    protocol = args.protocol_dir.resolve(strict=True)
    logs = args.log_dir.resolve(strict=True)
    max_parallel = len(args.scenarios) if args.max_parallel == "all" else args.max_parallel
    env = dict(os.environ, DEVNET_ENV=str(args.env_file.resolve(strict=True)),
               MAX_PARALLEL=str(max_parallel))
    execution = {"runner": "network-parallel", "max_parallel": max_parallel,
                 "suite_timeout_seconds": args.suite_timeout, "phases": []}
    deadline = time.monotonic() + args.suite_timeout
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    failed = False
    try:
        names = args.scenarios
        if "bridge" in names and "bridgeL2" in names:
            # The pinned bridgeL2 scenario releases escrow deposited by bridge.
            # Fresh worker wallets isolate nonces, not shared rollup escrow.
            result = run_network(protocol, logs, ["bridge"], env, deadline,
                                 phase="bridge-prerequisite")
            execution["phases"].append(result)
            failed = result["exit_code"] != 0 or result["results"] != [["bridge", "PASS"]]
            if result.get("interrupted"):
                return result["exit_code"]
            names = [name for name in names if name != "bridge"]
            if failed:
                names.remove("bridgeL2")
                execution["blocked"] = {"bridgeL2": "bridge prerequisite did not pass"}
                (logs / "bridgeL2.log").write_text("NOT RUN: bridge prerequisite did not pass\n")
                print("RESULT bridgeL2: SKIP (bridge prerequisite did not pass)", flush=True)
                print_failure_log(logs, "bridgeL2", env)
            # Upstream names run directories to second precision. Avoid a
            # collision between short phases without replacing retained logs.
            while names and time.monotonic() < deadline and (
                    protocol / "tmp/e2e-parallel-net" / time.strftime("%Y%m%d-%H%M%S")).exists():
                time.sleep(0.05)
        if names:
            result = run_network(protocol, logs, names, env, deadline)
            execution["phases"].append(result)
            if result.get("interrupted"):
                return result["exit_code"]
            failed |= result["exit_code"] != 0
    finally:
        (logs / "execution.json").write_text(json.dumps(execution, indent=2) + "\n")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
