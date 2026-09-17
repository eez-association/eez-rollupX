"""Bounded, offline paired analysis. See ../validation/overhead-analysis-schema.md.

No workload runs here. A passing native gate is not full R27 acceptance: lock
probe evidence and backend resource scope require separate review. Invalid runs
remain in the report and invalidate the set; they are never silently replaced.
"""

import argparse
import json
import math
import random
import statistics
from collections import Counter
from pathlib import Path


THRESHOLD = 0.05
MAX_SAMPLES = 100_000
MAX_BYTES = 32 * 1024 * 1024
IDENTITY_KEYS = (
    "shipping_binaries_sha256", "test_binary_sha256", "source_revision",
    "protocol_revision", "common_config_sha256", "arm_config_sha256",
)


def _number(value, positive=False):
    try:
        return (type(value) in (int, float) and math.isfinite(value)
                and (value > 0 if positive else value >= 0))
    except OverflowError:
        return False


def _integer(value, low, high):
    return type(value) is int and low <= value <= high


def quantile(values, probability):
    """Nearest-rank quantile of raw samples, including the p95 boundary."""
    ordered = sorted(values)
    return ordered[max(0, math.ceil(probability * len(ordered)) - 1)]


def paired_summary(ratios, kind, seed, replicates, conservative=None):
    """Bootstrap entire run pairs, never individual preparation observations.

    The point estimate is the median paired regression. The two-sided 95%
    percentile interval uses fixed-seed resampling. Preparation's conservative
    upper bound additionally allows one recorded clock tick in each direction.
    The producer floors milliseconds; this symmetric allowance is deliberately
    more conservative than the floor-only upper bound, never a cost correction.
    """
    regressions = [1 - r if kind == "throughput" else r - 1 for r in ratios]
    upper_values = conservative if conservative is not None else regressions
    rng = random.Random(seed)
    draws, upper_draws = [], []
    for _ in range(replicates):
        indices = [rng.randrange(len(ratios)) for _ in ratios]
        draws.append(statistics.median(regressions[i] for i in indices))
        upper_draws.append(statistics.median(upper_values[i] for i in indices))
    estimate = statistics.median(regressions)
    upper = quantile(upper_draws, .975)
    # Guard binary floating point's representation of an exact 5% boundary.
    below = lambda value: value < THRESHOLD and not math.isclose(
        value, THRESHOLD, rel_tol=0, abs_tol=1e-12)
    status = "pass" if below(estimate) and below(upper) else (
        "fail" if not below(estimate) else "inconclusive")
    return {
        "paired_b_over_a": ratios,
        "paired_regression": regressions,
        "median_regression": estimate,
        "mad_regression": statistics.median(abs(x - estimate) for x in regressions),
        "min_regression": min(regressions), "max_regression": max(regressions),
        "ci95_regression": [quantile(draws, .025), quantile(draws, .975)],
        "conservative_upper_regression": upper,
        "status": status,
    }


def analyze(document):
    """Return fail/inconclusive on bad evidence; only clean confirmatory sets pass.

    The caller freezes the plan before confirmatory collection. This function
    verifies recorded identities and plan adherence, not executable provenance or
    honesty of collection. All fractions are dimensionless (0.05 means 5%).
    """
    report = {"schema_version": 1, "status": "inconclusive", "issues": [],
              "runs": [], "stages": {}, "full_r27_status": "inconclusive",
              "scope": "shipping_candidate_native_throughput_preparation",
              "lock_gate": "separate benchmark-only evidence required",
              "resources": {"native": [], "backend": []}}

    def issue(path, message, severity="inconclusive"):
        report["issues"].append({"path": path, "message": message, "severity": severity})

    if (not isinstance(document, dict) or type(document.get("schema_version")) is not int
            or document["schema_version"] != 1):
        issue("schema_version", "expected schema version 1")
        return report
    phase = document.get("phase")
    report["phase"] = phase
    if phase not in ("pilot", "confirmatory"):
        issue("phase", "expected pilot or confirmatory")
        return report
    plan = document.get("frozen")
    if not isinstance(plan, dict):
        issue("frozen", "missing frozen plan")
        return report
    required = ("decision_id", "measured_waves", "min_preparation_samples",
                "min_tail_samples", "resolution_ms", "bootstrap_seed",
                "bootstrap_replicates", "expected_identity", "stages",
                "contamination_limits", "sample_scope", "time_unit",
                "measurement_scope")
    for key in required:
        if key not in plan:
            issue("frozen." + key, "missing frozen field")
    if report["issues"]:
        return report
    if not isinstance(plan["decision_id"], str) or not plan["decision_id"].strip():
        issue("frozen.decision_id", "requires predeclared plan/pilot decision reference")
    if plan["sample_scope"] != "shipping" or plan["time_unit"] != "ms":
        issue("frozen", "requires shipping sample scope and explicit ms time unit")
    scope = plan["measurement_scope"]
    if (not isinstance(scope, dict) or not isinstance(scope.get("name"), str)
            or not scope["name"] or not isinstance(scope.get("backend_included"), list)
            or not all(isinstance(x, str) and x for x in scope["backend_included"])
            or len(set(scope["backend_included"])) != len(scope["backend_included"])):
        issue("frozen.measurement_scope", "requires named scope and explicit included backend list")
    else:
        report["measurement_scope"] = scope
    report["sample_scope"] = plan["sample_scope"]
    report["preparation_clock"] = {
        "time_unit": plan["time_unit"], "resolution": plan["resolution_ms"],
        "rounding": "floor", "uncertainty_allowance": "symmetric_one_tick_extra_conservative",
    }
    for key, low, high in (("measured_waves", 1, 10000),
                           ("min_preparation_samples", 1, MAX_SAMPLES),
                           ("min_tail_samples", 2 if phase == "confirmatory" else 1, MAX_SAMPLES // 20),
                           ("bootstrap_seed", 0, 2**32 - 1),
                           ("bootstrap_replicates", 1000, 20000)):
        if not _integer(plan[key], low, high):
            issue("frozen." + key, f"expected integer in [{low}, {high}]")
    if not _number(plan["resolution_ms"], positive=True):
        issue("frozen.resolution_ms", "requires positive finite clock resolution")
    expected = plan["expected_identity"]
    if not isinstance(expected, dict) or any(key not in expected for key in IDENTITY_KEYS):
        issue("frozen.expected_identity", "missing binary/source/protocol/config identity")
    else:
        hashes = expected["shipping_binaries_sha256"]
        if not isinstance(hashes, dict) or not hashes or not all(
                isinstance(k, str) and k and _hex(v, 64) for k, v in hashes.items()):
            issue("frozen.expected_identity", "shipping binary hashes must be a nonempty map")
        for key in ("test_binary_sha256", "common_config_sha256"):
            if not _hex(expected[key], 64):
                issue("frozen.expected_identity." + key, "requires SHA-256")
        for key in ("source_revision", "protocol_revision"):
            if not (_hex(expected[key], 40) or _hex(expected[key], 64)):
                issue("frozen.expected_identity." + key, "requires full source revision")
        configs = expected["arm_config_sha256"]
        if (not isinstance(configs, dict) or set(configs) != {"A", "B"}
                or not all(_hex(v, 64) for v in configs.values())):
            issue("frozen.expected_identity.arm_config_sha256", "requires both arm config hashes")
    stages = plan["stages"]
    if not isinstance(stages, list) or not 1 <= len(stages) <= 16 or any(
            not isinstance(s, dict) or not isinstance(s.get("name"), str)
            or not s["name"] or not _integer(s.get("successful_sources"), 1, 10**9)
            for s in stages):
        issue("frozen.stages", "requires 1..16 named stages with fixed source counts")
    elif len({s["name"] for s in stages}) != len(stages):
        issue("frozen.stages", "duplicate stage")
    limits = plan["contamination_limits"]
    if not isinstance(limits, dict) or not limits or not all(
            isinstance(k, str) and k and _number(v) for k, v in limits.items()):
        issue("frozen.contamination_limits", "requires named predeclared finite upper limits")
    if report["issues"]:
        return report

    runs = document.get("runs")
    pairs = 2 if phase == "pilot" else 6
    if not isinstance(runs, list) or len(runs) > 12:
        issue("runs", "requires bounded chronological run list (maximum 12)")
        return report
    if len(runs) != pairs * 2:
        issue("runs", f"{phase} requires exactly {pairs} complete pairs")
    seen = set()
    for index, run in enumerate(runs):
        path = f"runs[{index}]"
        result = {"index": index, "stages": {}}
        report["runs"].append(result)
        if not isinstance(run, dict):
            issue(path, "missing run record")
            continue
        result.update({k: run.get(k) for k in ("run_id", "pair", "arm", "phase")})
        run_id = run.get("run_id")
        if not isinstance(run_id, str) or not run_id or run_id in seen:
            issue(path + ".run_id", "requires unique run ID", "fail")
        else:
            seen.add(run_id)
        pair = index // 2 + 1
        arm = ("AB" if pair % 2 else "BA")[index % 2]
        if type(run.get("pair")) is not int or run.get("pair") != pair or run.get("arm") != arm:
            issue(path, "wrong counterbalanced chronological pair/arm order", "fail")
        if run.get("phase") != phase:
            issue(path + ".phase", "pilot and confirmatory evidence must remain separate", "fail")
        if run.get("sample_scope") != "shipping":
            issue(path + ".sample_scope", "benchmark probe samples cannot enter shipping gate", "fail")
        identity = dict(expected, arm_config_sha256=expected["arm_config_sha256"][arm])
        if run.get("identity") != identity:
            issue(path + ".identity", "binary/source/protocol/config identity mismatch", "fail")
        if type(run.get("measured_waves")) is not int or run.get("measured_waves") != plan["measured_waves"]:
            issue(path + ".measured_waves", "frozen workload count mismatch", "fail")
        for field in ("semantic_ok", "telemetry_healthy"):
            if run.get(field) is not True:
                issue(path + "." + field, "missing/failed validation",
                      "fail" if run.get(field) is False else "inconclusive")
        for field in ("sample_overflow", "logger_gaps"):
            if run.get(field) is not False:
                issue(path + "." + field, "sample completeness not established",
                      "fail" if run.get(field) is True else "inconclusive")
        if run.get("budget_exhausted") is not False:
            issue(path + ".budget_exhausted", "budget completion not established")
        if run.get("lock_probe") != {"enabled": False}:
            issue(path + ".lock_probe", "native shipping gate requires probe-disabled runs", "fail")
        observed = run.get("contamination")
        for key, limit in limits.items():
            value = observed.get(key) if isinstance(observed, dict) else None
            if not _number(value):
                issue(path + ".contamination." + key, "missing/invalid contamination measurement")
            elif value > limit:
                issue(path + ".contamination." + key, "predeclared contamination limit exceeded")
        result["contamination"] = observed
        resource = run.get("resources", {})
        if isinstance(resource, dict):
            for scope in ("native", "backend"):
                report["resources"][scope].append({"run_id": run_id, "raw": resource.get(scope)})
        raw_stages = run.get("stages")
        if not isinstance(raw_stages, list) or len(raw_stages) != len(stages):
            issue(path + ".stages", "missing/unexpected stages")
            continue
        for stage_index, (frozen_stage, stage) in enumerate(zip(stages, raw_stages)):
            spath = f"{path}.stages[{stage_index}]"
            if not isinstance(stage, dict) or stage.get("name") != frozen_stage["name"]:
                issue(spath, "stage identity/order mismatch", "fail")
                continue
            count, elapsed = stage.get("successful_sources"), stage.get("elapsed_seconds")
            if type(count) is not int or count != frozen_stage["successful_sources"]:
                issue(spath, "semantically validated source count mismatch", "fail")
                continue
            if not _number(elapsed, positive=True):
                issue(spath + ".elapsed_seconds", "requires positive monotonic measured interval")
                continue
            throughput = count / elapsed
            if not _number(throughput, positive=True):
                issue(spath + ".elapsed_seconds", "unrepresentable source throughput")
                continue
            samples, groups = stage.get("preparation_ms"), stage.get("preparation_group_counts")
            if (not isinstance(samples, list) or not 1 <= len(samples) <= MAX_SAMPLES
                    or not all(_number(x) for x in samples)):
                issue(spath + ".preparation_ms", "missing, invalid or over-limit raw samples")
                continue
            n = len(samples)
            tail = n - math.ceil(.95 * n)
            if n < plan["min_preparation_samples"] or tail < plan["min_tail_samples"]:
                issue(spath + ".preparation_ms", "insufficient frozen total/tail sample count")
            if (not isinstance(groups, list) or len(groups) != n
                    or not all(_integer(g, 1, 10**9) for g in groups)):
                issue(spath + ".preparation_group_counts", "requires group count for every attempt")
                continue
            p95 = quantile(samples, .95)
            if p95 <= plan["resolution_ms"]:
                issue(spath + ".preparation_ms", "preparation tail unresolvable at clock resolution")
            distribution = {str(g): count for g, count in sorted(Counter(groups).items())}
            result["stages"][stage["name"]] = {
                "successful_sources": count, "elapsed_seconds": elapsed,
                "throughput_sources_per_second": throughput,
                "preparation_p95_ms": p95, "preparation_samples": n,
                "tail_samples_above_p95_rank": tail,
                "preparation_group_count_distribution": distribution,
            }

    # Never construct a favorable subset after rejecting one pair.
    if report["issues"]:
        report["status"] = "fail" if any(i["severity"] == "fail" for i in report["issues"]) else "inconclusive"
        return report
    for stage in stages:
        name = stage["name"]
        throughput, preparation, conservative = [], [], []
        for pair_index in range(pairs):
            arms = {r["arm"]: r["stages"][name]
                    for r in report["runs"][pair_index * 2:pair_index * 2 + 2]}
            a, b = arms["A"], arms["B"]
            throughput.append(b["throughput_sources_per_second"] / a["throughput_sources_per_second"])
            preparation.append(b["preparation_p95_ms"] / a["preparation_p95_ms"])
            tick = plan["resolution_ms"]
            conservative.append((b["preparation_p95_ms"] + tick)
                                / (a["preparation_p95_ms"] - tick) - 1)
        if not all(math.isfinite(x) for x in throughput + preparation + conservative):
            issue("stages." + name, "unrepresentable paired ratio")
            report["stages"] = {}
            return report
        report["stages"][name] = {
            "throughput": paired_summary(throughput, "throughput", plan["bootstrap_seed"], plan["bootstrap_replicates"]),
            "preparation": paired_summary(preparation, "preparation", plan["bootstrap_seed"], plan["bootstrap_replicates"], conservative),
        }
    statuses = [metric["status"] for stage in report["stages"].values() for metric in stage.values()]
    report["status"] = ("pilot_complete" if phase == "pilot" else
                        "fail" if "fail" in statuses else
                        "inconclusive" if "inconclusive" in statuses else "pass")
    report["decision_id"] = plan["decision_id"]
    report["bootstrap"] = {"unit": "whole_run_pair", "statistic": "median_paired_regression",
                           "seed": plan["bootstrap_seed"], "replicates": plan["bootstrap_replicates"],
                           "interval": "two-sided percentile 95%"}
    report["resources"]["status"] = "reported separately; no resource acceptance decision"
    return report


def _hex(value, size):
    return isinstance(value, str) and len(value) == size and all(c in "0123456789abcdef" for c in value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    args = parser.parse_args()
    if args.input.stat().st_size > MAX_BYTES:
        parser.error("input exceeds 32 MiB bound")
    try:
        document = json.loads(args.input.read_text())
        result = analyze(document)
    except (OSError, ValueError, RecursionError) as error:
        parser.error(str(error))
    print(json.dumps(result, indent=2, allow_nan=False))
    return 0 if result["status"] in ("pass", "pilot_complete") else 1


if __name__ == "__main__":
    raise SystemExit(main())
