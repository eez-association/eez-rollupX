#!/usr/bin/env python3
"""Schedule independent protocol checks; never weaken or synthesize a PASS."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time


REPO = Path(__file__).resolve().parents[2]
COLLECTOR = REPO / "testing/kurtosis/scripts/collect-protocol-e2e-evidence.sh"


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def stop(process):
    # Kill only this collector's process group, including timed-out RPC clients.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def collect(names, paths, logs, output, workers, timeout):
    deadline = time.monotonic() + timeout
    cancelled = threading.Event()
    active = set()
    lock = threading.Lock()

    def worker(name):
        started = time.monotonic()
        record = {"scenario": name, "exit_code": 124, "seconds": 0}
        log = output / f"{name}.collect.log"
        with log.open("x") as stream:
            with lock:
                if cancelled.is_set() or started >= deadline:
                    stream.write("Evidence collection deadline exceeded before start\n")
                    return record
                process = subprocess.Popen(
                    ["bash", str(COLLECTOR), name, str(paths[name]),
                     str(logs / f"{name}.log"), str(output / f"{name}.json")],
                    stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
                active.add(process)
            try:
                record["exit_code"] = process.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                stream.write("Evidence collection deadline exceeded\n")
            finally:
                with lock:
                    stop(process)
                    active.remove(process)
        record["seconds"] = round(time.monotonic() - started, 3)
        if record["exit_code"] == 0:
            try:
                result = json.loads((output / f"{name}.json").read_text())
                if result.get("scenario") != name or result.get("result") != "PASS":
                    raise ValueError("missing exact scenario PASS")
            except (OSError, ValueError, AttributeError) as error:
                record["exit_code"] = 1
                with log.open("a") as stream:
                    stream.write(f"Invalid verifier result: {error}\n")
        return record

    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)

    previous = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGINT, signal.SIGTERM)}
    pool = ThreadPoolExecutor(max_workers=workers)
    try:
        # map preserves input order for deterministic accounting, not execution.
        return list(pool.map(worker, names))
    finally:
        cancelled.set()
        with lock:
            for process in active:
                stop(process)
        pool.shutdown(wait=True, cancel_futures=True)
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--log-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--workers", type=positive, default=4)
    parser.add_argument("--timeout", type=positive, default=600)
    parser.add_argument("scenarios", nargs="+")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    paths = {item["name"]: REPO / "eez-core-protocol" / item["path"]
             for item in manifest["scenarios"]}
    if len(set(args.scenarios)) != len(args.scenarios) or any(n not in paths for n in args.scenarios):
        parser.error("expected unique scenarios from the suite manifest")
    started = time.monotonic()
    records = collect(args.scenarios, paths, args.log_dir, args.output_dir, args.workers, args.timeout)
    execution = {"workers": args.workers, "seconds": round(time.monotonic() - started, 3),
                 "scenarios": records}
    (args.output_dir / "execution.json").write_text(json.dumps(execution, indent=2) + "\n")
    print(f"==> independent verification: {len(records)} scenarios, "
          f"{args.workers} workers, {execution['seconds']}s", flush=True)
    return int(any(record["exit_code"] != 0 for record in records))


if __name__ == "__main__":
    raise SystemExit(main())
