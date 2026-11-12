#!/usr/bin/env python3
"""Run the native #148 driver only inside an observed, shared-host quiet window.

Build and validate first. This runner never builds, restarts production, or kills
unrelated work. Its only termination target is the process group it creates.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import statistics
import subprocess
import time

INTERVAL = 10
STABLE_SECONDS = 120
TEST = "streaming::deadlines::characterize_native_streaming_workload"
HEAVY = re.compile(r"^(cargo|rustc|clippy|clang|solc|gcc|g\+\+|cc$|c\+\+$|cc1|rust-lld|ld$|ld\.lld|cmake|ninja|make|forge|anvil|nextest|node_e2e|eez_|du$|rsync$|zstd$|tar$|gzip$)")


def descendants(processes, root):
    owned = {root}
    while True:
        children = {p[0] for p in processes if p[1] in owned}
        if children <= owned:
            return owned
        owned |= children


def heavy_processes(owned_root=None):
    processes = []
    for line in subprocess.check_output(["ps", "-eo", "pid=,ppid=,comm="], text=True).splitlines():
        pid, parent, command = line.split(maxsplit=2)
        processes.append((int(pid), int(parent), command))
    owned = descendants(processes, owned_root) if owned_root is not None else set()
    # Only the expected test harness is exempted. Even an accidental owned Cargo
    # build would invalidate timing; the prebuilt driver should never start one.
    heavy = [dict(pid=pid, command=command) for pid, _, command in processes
             if HEAVY.search(command) and not (pid in owned and command.startswith("node_e2e"))]
    return len(processes), heavy


def wait_interval(owned_root=None):
    observed = {}
    for _ in range(INTERVAL):
        time.sleep(1)
        _, heavy = heavy_processes(owned_root)
        observed.update((p["pid"], p) for p in heavy)
        if owned_root is not None and heavy:
            break
    return list(observed.values())


def snapshot(output, owned_root=None):
    mem = {k: int(v.split()[0]) for k, v in
           (line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines())}
    vm = dict(line.split() for line in Path("/proc/vmstat").read_text().splitlines())
    process_count, heavy = heavy_processes(owned_root)
    disks = {}
    for name in ("/", "/tmp", "/dev/shm", str(output)):
        fs = os.statvfs(name)
        disks[name] = fs.f_bavail * fs.f_frsize
    return dict(utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), monotonic=time.monotonic(),
                cpu=list(map(int, Path("/proc/stat").read_text().splitlines()[0].split()[1:9])),
                load=os.getloadavg(), affinity_cpus=len(os.sched_getaffinity(0)),
                host_cpus=os.cpu_count(), process_count=process_count, heavy=heavy,
                mem_available_kib=mem["MemAvailable"], swap_used_kib=mem["SwapTotal"]-mem["SwapFree"],
                swapin=int(vm["pswpin"]), swapout=int(vm["pswpout"]), free_bytes=disks,
                pressure={k: Path(f"/proc/pressure/{k}").read_text().strip() for k in ("cpu", "io", "memory")})


def reasons(previous, current):
    bad = []
    if current["process_count"] < 50:
        bad.append("host process visibility unavailable")
    if current["heavy"]:
        bad.append("unrelated compiler/test/IO work")
    if min(current["free_bytes"].values()) < 5 * 2**30:
        bad.append("less than 5 GiB free")
    if current["mem_available_kib"] < 12 * 2**20:
        bad.append("less than 12 GiB available memory")
    if current["load"][0] > current["affinity_cpus"] / 2:
        bad.append("one-minute load too high")
    memory_some = current["pressure"]["memory"].splitlines()[0]
    if float(dict(item.split("=") for item in memory_some.split()[1:])["avg10"]) >= 1:
        bad.append("memory pressure")
    if previous is not None:
        cpu = [b-a for a, b in zip(previous["cpu"], current["cpu"])]
        total = sum(cpu)
        if total <= 0 or cpu[3] / total < .70:
            bad.append("CPU idle below 70%")
        if total <= 0 or cpu[4] / total >= .05:
            bad.append("IO wait at least 5%")
        if current["swapin"] != previous["swapin"] or current["swapout"] != previous["swapout"]:
            bad.append("swap traffic")
    return bad


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1024*1024), b""):
            digest.update(block)
    return digest.hexdigest()


def distributions(rows):
    groups = {}
    for row in rows:
        if row.get("event") != "attempt_result" or row["warmup"]:
            continue
        result = row["result"]
        key = result["fixture"] + "/" + result["scenario"]
        group = groups.setdefault(key, dict(successes=0, failures=0, metrics={}, operation_intervals_ms={}))
        group["successes" if row["success"] else "failures"] += 1
        if row["success"]:
            for name, value in result.items():
                if name.endswith("_ms"):
                    group["metrics"].setdefault(name, []).append(value)
            pending = {}
            for event in result.get("events", []):
                identity = (event["transport"], event.get("request_id"))
                if event["direction"] == "client":
                    pending[identity] = event
                elif event["direction"] == "server" and identity in pending:
                    request = pending.pop(identity)
                    kind = request["kind"] + "/" + event["kind"]
                    elapsed = event["elapsed_ms"]-request["elapsed_ms"]
                    if elapsed < 0:
                        raise ValueError("non-monotonic operation trace")
                    group["operation_intervals_ms"].setdefault(kind, []).append(elapsed)
    for group in groups.values():
        for category in ("metrics", "operation_intervals_ms"):
            for name, values in group[category].items():
                values.sort()
                rank = lambda p: values[max(0, math.ceil(p*len(values))-1)]
                group[category][name] = dict(n=len(values), minimum=values[0], median=statistics.median(values),
                    p90=rank(.9), p95=rank(.95), maximum=values[-1], iqr=rank(.75)-rank(.25))
    return groups


def terminate_owned(child):
    # start_new_session gives this child a fresh group. Never select by name.
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    # The harness may exit before its signer. Reaping only the group leader
    # does not establish that all owned descendants stopped.
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("test_binary", type=Path)
    parser.add_argument("signer_binary", type=Path)
    parser.add_argument("output", type=Path, help="new owned evidence directory")
    parser.add_argument("--wait-seconds", type=int, default=1800)
    args = parser.parse_args()
    OUTPUT = args.output.resolve()
    OUTPUT.mkdir(parents=True, exist_ok=False)
    test, signer = args.test_binary.resolve(), args.signer_binary.resolve()
    if test.parent.parent.name != "release" or signer.parent.name != "release":
        raise RuntimeError("reported timing requires the prepared release test and signer binaries")
    root = Path(__file__).resolve().parents[1]
    # Hash/read identities before quiet observation; this IO is not a workload sample.
    manifest = dict(source=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        changes=subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).splitlines(),
        protocol=subprocess.check_output(["git", "ls-tree", "HEAD", "eez-core-protocol"], cwd=root, text=True).strip(),
        binaries={str(p): sha256(p) for p in (test, signer)}, lockfile_sha256=sha256(root/"Cargo.lock"),
        time_source="monotonic elapsed; UTC only for correlation", host_isolated=False,
        profile="workspace release, default features; test driver includes E2E dev dependencies",
        kernel=os.uname().release,
        stable_seconds=STABLE_SECONDS, interval_seconds=INTERVAL)
    if manifest["changes"]:
        raise RuntimeError("commit the reviewed driver before collecting reported timings")
    listing = subprocess.check_output([str(test), "--list", "--ignored"], text=True)
    if TEST + ": test" not in listing.splitlines():
        raise RuntimeError("measurement test not present in exact binary")
    (OUTPUT/"manifest.json").write_text(json.dumps(manifest, indent=2)+"\n")
    previous = None
    interval_heavy = []
    stable_since = None
    wait_until = time.monotonic()+args.wait_seconds
    child = None
    contaminated = False
    status = "quiet window unavailable"
    raw = OUTPUT/"attempts.jsonl"
    with (OUTPUT/"host.jsonl").open("x") as host, (OUTPUT/"driver.log").open("x") as log:
        try:
            while True:
                current = snapshot(OUTPUT, child.pid if child else None)
                current["heavy"].extend(interval_heavy)
                bad = reasons(previous, current)
                host.write(json.dumps(dict(phase="before" if child is None else "during", snapshot=current, reasons=bad))+"\n")
                host.flush()
                print(current["utc"], ", ".join(bad) or "eligible interval", flush=True)
                if child is not None:
                    if bad:
                        contaminated = True
                        status = "contaminated; retained diagnostics, repeat in a new quiet window"
                        terminate_owned(child)
                        break
                    if child.poll() is not None:
                        status = "driver completed" if child.returncode == 0 else "driver failed; retain first failure"
                        break
                else:
                    if bad:
                        stable_since = None
                    elif previous is not None:
                        stable_since = stable_since if stable_since is not None else previous["monotonic"]
                    if stable_since is not None and current["monotonic"]-stable_since >= STABLE_SECONDS:
                        env = dict(os.environ, EEZ_TEST_PREBUILT="1", EEZ_TEST_PROOF_SIGNER_BIN=str(signer),
                                   EEZ_TEST_LOG_DIR=str(OUTPUT/"signer-logs"), EEZ_STREAMING_TIMINGS_OUTPUT=str(raw),
                                   CARGO_MANIFEST_DIR=str(root/"crates/eez-node-e2e"))
                        child = subprocess.Popen([str(test), "--ignored", "--exact", TEST, "--nocapture", "--test-threads=1"],
                            cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                    elif time.monotonic() >= wait_until:
                        break
                previous = current
                interval_heavy = wait_interval(child.pid if child else None)
            if child is not None:
                # Post-observation is required even after failure or contamination.
                for _ in range(STABLE_SECONDS//INTERVAL):
                    interval_heavy = wait_interval()
                    current = snapshot(OUTPUT)
                    current["heavy"].extend(interval_heavy)
                    bad = reasons(previous, current)
                    host.write(json.dumps(dict(phase="after", snapshot=current, reasons=bad))+"\n")
                    host.flush()
                    contaminated |= bool(bad)
                    previous = current
        except (OSError, ValueError) as error:
            contaminated = True
            status = f"resource/observer failure: {error}; retain diagnostics"
        finally:
            if child is not None:
                terminate_owned(child)
    try:
        rows = [json.loads(line) for line in raw.read_text().splitlines()] if raw.exists() else []
    except (OSError, ValueError) as error:
        contaminated = True
        status = f"incomplete/unreadable attempt records: {error}; retain diagnostics"
        rows = []
    groups = distributions(rows)
    complete = len(groups) == 12 and all(g["successes"] == 20 and g["failures"] == 0 for g in groups.values())
    eligible = child is not None and child.returncode == 0 and not contaminated and complete
    summary = dict(status=status, eligible_performance_evidence=eligible, contaminated=contaminated,
                   complete=complete, groups=groups,
                   operation_sample_note="Proxy receipt-to-response intervals include queueing; operation samples are correlated within each workload repetition",
                   limits="Small fixed native workload through an instrumented loopback proxy on a shared host; no SLA or production-capacity claim")
    (OUTPUT/"summary.json").write_text(json.dumps(summary, indent=2)+"\n")
    print(json.dumps({k: v for k, v in summary.items() if k != "groups"}), flush=True)
    return 0 if eligible else 1


if __name__ == "__main__":
    raise SystemExit(main())
