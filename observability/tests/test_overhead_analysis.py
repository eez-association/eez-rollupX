"""Synthetic unit fixtures only: none are measured overhead evidence."""

import copy
import unittest

from overhead_analysis import analyze, paired_summary, quantile


def fixture(phase="confirmatory", preparation_factors=None):
    """Small complete schema example; hashes identify synthetic fixture bytes."""
    identity = {
        "shipping_binaries_sha256": {"composer": "a" * 64, "signer": "b" * 64,
                                     "follower": "c" * 64},
        "test_binary_sha256": "d" * 64, "source_revision": "e" * 40,
        "protocol_revision": "f" * 40, "common_config_sha256": "1" * 64,
        "arm_config_sha256": {"A": "2" * 64, "B": "3" * 64},
    }
    plan = {
        "decision_id": "synthetic-pilot-decision-not-evidence",
        "measured_waves": 4, "min_preparation_samples": 40, "min_tail_samples": 2,
        "sample_scope": "shipping", "time_unit": "ms", "resolution_ms": 1,
        "measurement_scope": {"name": "native-telemetry", "backend_included": ["collector"]},
        "bootstrap_seed": 27, "bootstrap_replicates": 1000,
        "expected_identity": identity,
        "stages": [{"name": "small_queue", "successful_sources": 8},
                   {"name": "busy_waves", "successful_sources": 128}],
        "contamination_limits": {"cpu_steal_percent": 1, "major_faults": 0},
    }
    pairs = 2 if phase == "pilot" else 6
    factors = preparation_factors or [1] * pairs
    runs = []
    for pair in range(1, pairs + 1):
        for arm in ("AB" if pair % 2 else "BA"):
            runs.append({
                "run_id": f"{phase}-{pair}-{arm}", "pair": pair, "arm": arm,
                "phase": phase, "sample_scope": "shipping",
                "identity": dict(identity, arm_config_sha256=identity["arm_config_sha256"][arm]),
                "measured_waves": 4, "semantic_ok": True, "telemetry_healthy": True,
                "sample_overflow": False, "logger_gaps": False, "budget_exhausted": False,
                "lock_probe": {"enabled": False},
                "contamination": {"cpu_steal_percent": 0, "major_faults": 0},
                "resources": {"native": {"cpu_seconds": 4, "peak_rss_bytes": 1000},
                              "backend": {"cpu_seconds": 1, "peak_rss_bytes": 2000}},
                "stages": [dict(stage, elapsed_seconds=10,
                                preparation_ms=[100 * (factors[pair - 1] if arm == "B" else 1)] * 40,
                                preparation_group_counts=[1] * 20 + [2] * 20)
                           for stage in plan["stages"]],
            })
    return {"schema_version": 1, "phase": phase, "frozen": plan, "runs": runs}


class OverheadAnalysisTests(unittest.TestCase):
    def test_complete_confirmatory_set_has_native_pass_but_full_gate_open(self):
        result = analyze(fixture())
        self.assertEqual(result["status"], "pass")
        self.assertEqual(result["full_r27_status"], "inconclusive")
        self.assertEqual(len(result["runs"]), 12)
        self.assertEqual(set(result["stages"]), {"small_queue", "busy_waves"})
        stage = result["runs"][0]["stages"]["small_queue"]
        self.assertEqual(stage["throughput_sources_per_second"], .8)
        self.assertEqual(stage["preparation_p95_ms"], 100)
        self.assertEqual(stage["preparation_group_count_distribution"], {"1": 20, "2": 20})
        self.assertEqual(result["bootstrap"]["unit"], "whole_run_pair")
        self.assertEqual(len(result["stages"]["busy_waves"]["preparation"]["paired_b_over_a"]), 6)

    def test_pilot_is_never_confirmatory_acceptance(self):
        result = analyze(fixture("pilot"))
        self.assertEqual(result["status"], "pilot_complete")
        self.assertEqual(len(result["stages"]["small_queue"]["preparation"]["paired_b_over_a"]), 2)
        data = fixture()
        data["runs"][0]["phase"] = "pilot"
        self.assertEqual(analyze(data)["status"], "fail")

    def test_nearest_rank_uses_raw_samples(self):
        self.assertEqual(quantile(list(range(1, 41)), .95), 38)
        data = fixture()
        data["runs"][0]["stages"][0]["preparation_ms"] = [100] * 38 + [10000] * 2
        self.assertEqual(analyze(data)["runs"][0]["stages"]["small_queue"]["preparation_p95_ms"], 100)

    def test_exact_five_percent_is_not_below_gate(self):
        preparation = paired_summary([1.05] * 6, "preparation", 27, 1000)
        throughput = paired_summary([.95] * 6, "throughput", 27, 1000)
        self.assertEqual(preparation["status"], "fail")
        self.assertEqual(throughput["status"], "fail")
        self.assertEqual(paired_summary([1.049] * 6, "preparation", 27, 1000)["status"], "pass")

    def test_interval_crossing_five_percent_is_inconclusive(self):
        result = analyze(fixture(preparation_factors=[1.02] * 4 + [1.12] * 2))
        metric = result["stages"]["busy_waves"]["preparation"]
        self.assertLess(metric["median_regression"], .05)
        self.assertGreater(metric["ci95_regression"][1], .05)
        self.assertEqual(result["status"], "inconclusive")

    def test_quantization_can_prevent_passing_even_identical_samples(self):
        data = fixture()
        for run in data["runs"]:
            for stage in run["stages"]:
                stage["preparation_ms"] = [20] * 40
        result = analyze(data)
        self.assertEqual(result["status"], "inconclusive")
        self.assertGreater(result["stages"]["small_queue"]["preparation"]["conservative_upper_regression"], .05)
        data["runs"][0]["stages"][0]["preparation_ms"] = [0] * 40
        self.assertEqual(analyze(data)["status"], "inconclusive")

    def test_missing_and_insufficient_samples_are_not_zero_overhead(self):
        for change in (None, [], [100] * 39, [float("nan")] * 40, [True] * 40):
            with self.subTest(samples=change):
                data = fixture()
                data["runs"][0]["stages"][0]["preparation_ms"] = change
                result = analyze(data)
                self.assertEqual(result["status"], "inconclusive")
                self.assertEqual(result["stages"], {})
                self.assertEqual(len(result["runs"]), 12)

    def test_missing_pairs_stages_or_completeness_remain_inconclusive(self):
        for field in ("logger_gaps", "sample_overflow", "semantic_ok", "telemetry_healthy", "budget_exhausted"):
            data = fixture()
            del data["runs"][0][field]
            self.assertEqual(analyze(data)["status"], "inconclusive", field)
        data = fixture()
        data["runs"].pop()
        self.assertEqual(analyze(data)["status"], "inconclusive")
        data = fixture()
        data["runs"][0]["stages"].pop()
        self.assertEqual(analyze(data)["status"], "inconclusive")

    def test_counterbalancing_and_duplicate_run_rejected(self):
        for phase in ("pilot", "confirmatory"):
            data = fixture(phase)
            data["runs"][2:4] = reversed(data["runs"][2:4])
            self.assertEqual(analyze(data)["status"], "fail")
        data = fixture()
        data["runs"][1]["run_id"] = data["runs"][0]["run_id"]
        self.assertEqual(analyze(data)["status"], "fail")

    def test_wrong_identity_and_changed_workload_fail(self):
        for field in ("shipping_binaries_sha256", "test_binary_sha256", "source_revision",
                      "protocol_revision", "common_config_sha256", "arm_config_sha256"):
            data = fixture()
            data["runs"][0]["identity"] = copy.deepcopy(data["runs"][0]["identity"])
            data["runs"][0]["identity"][field] = "wrong"
            self.assertEqual(analyze(data)["status"], "fail", field)
        data = fixture()
        data["runs"][0]["measured_waves"] = 5
        self.assertEqual(analyze(data)["status"], "fail")

    def test_semantic_loss_overflow_and_logger_failures_poison_whole_set(self):
        for field, value in (("semantic_ok", False), ("telemetry_healthy", False),
                             ("sample_overflow", True), ("logger_gaps", True)):
            data = fixture()
            data["runs"][3][field] = value
            result = analyze(data)
            self.assertEqual(result["status"], "fail", field)
            self.assertEqual(len(result["runs"]), 12)
            self.assertEqual(result["stages"], {})

    def test_contamination_is_visible_and_not_dropped(self):
        data = fixture()
        data["runs"][3]["contamination"]["major_faults"] = 1
        result = analyze(data)
        self.assertEqual(result["status"], "inconclusive")
        self.assertEqual(len(result["runs"]), 12)
        self.assertEqual(result["runs"][3]["contamination"]["major_faults"], 1)
        self.assertEqual(result["stages"], {})

    def test_probe_enabled_data_cannot_pass_shipping_gate(self):
        for field, value in (("lock_probe", {"enabled": True}), ("sample_scope", "lock_probe")):
            data = fixture()
            data["runs"][0][field] = value
            self.assertEqual(analyze(data)["status"], "fail")

    def test_stage_regression_cannot_be_pooled_away(self):
        data = fixture()
        for run in data["runs"]:
            if run["arm"] == "B":
                run["stages"][1]["preparation_ms"] = [110] * 40
        result = analyze(data)
        self.assertEqual(result["status"], "fail")
        self.assertEqual(result["stages"]["small_queue"]["preparation"]["status"], "pass")
        self.assertEqual(result["stages"]["busy_waves"]["preparation"]["status"], "fail")

    def test_native_backend_resources_remain_explicit_separate_scope(self):
        data = fixture()
        data["frozen"]["measurement_scope"] = {"name": "full-observer", "backend_included": ["observer", "postgresql"]}
        result = analyze(data)
        self.assertEqual(result["resources"]["native"][0]["raw"]["cpu_seconds"], 4)
        self.assertEqual(result["resources"]["backend"][0]["raw"]["cpu_seconds"], 1)
        self.assertEqual(result["measurement_scope"], data["frozen"]["measurement_scope"])

    def test_frozen_decision_and_tail_minimum_required(self):
        for key, value in (("decision_id", ""), ("min_tail_samples", 1),
                           ("bootstrap_replicates", 1000000), ("min_preparation_samples", True),
                           ("resolution_ms", float("inf")), ("contamination_limits", {})):
            data = fixture()
            data["frozen"][key] = value
            self.assertEqual(analyze(data)["status"], "inconclusive", key)
        data = fixture("pilot")
        data["frozen"]["min_tail_samples"] = 1
        self.assertEqual(analyze(data)["status"], "pilot_complete")

    def test_tail_count_is_independent_of_total_sample_minimum(self):
        data = fixture()
        data["frozen"]["min_preparation_samples"] = 20
        stage = data["runs"][0]["stages"][0]
        stage["preparation_ms"] = [100] * 39
        stage["preparation_group_counts"] = [1] * 39
        result = analyze(data)
        self.assertEqual(result["status"], "inconclusive")
        self.assertEqual(result["runs"][0]["stages"]["small_queue"]["tail_samples_above_p95_rank"], 1)

    def test_throughput_uses_successful_sources_per_canonical_interval(self):
        data = fixture()
        for run in data["runs"]:
            if run["arm"] == "B":
                run["stages"][0]["elapsed_seconds"] = 10 / .95
        result = analyze(data)
        self.assertEqual(result["status"], "fail")
        self.assertAlmostEqual(result["stages"]["small_queue"]["throughput"]["median_regression"], .05)
        self.assertEqual(result["stages"]["busy_waves"]["throughput"]["status"], "pass")

    def test_bounds_and_extreme_numbers_do_not_produce_nonfinite_reports(self):
        for samples in ([100] * 100001, [10**1000] * 40):
            data = fixture()
            data["runs"][0]["stages"][0]["preparation_ms"] = samples
            self.assertEqual(analyze(data)["status"], "inconclusive")
        data = fixture()
        data["runs"][0]["stages"][0]["elapsed_seconds"] = 1e-320
        self.assertEqual(analyze(data)["status"], "inconclusive")

    def test_bootstrap_is_deterministic_and_duplicates_do_not_add_run_pairs(self):
        data = fixture(preparation_factors=[.99, 1, 1.01, 1.02, 1.03, 1.04])
        first = analyze(data)
        self.assertEqual(first, analyze(copy.deepcopy(data)))
        for run in data["runs"]:
            for stage in run["stages"]:
                stage["preparation_ms"] *= 2
                stage["preparation_group_counts"] *= 2
        self.assertEqual(first["stages"], analyze(data)["stages"])


if __name__ == "__main__":
    unittest.main()
