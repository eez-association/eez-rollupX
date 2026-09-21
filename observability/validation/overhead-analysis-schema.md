# Offline overhead analysis input, version 1

Status: **implementation with synthetic fixtures; no measured result**. This is
the input contract for [`overhead_analysis.py`](../tests/overhead_analysis.py),
following the [experiment plan](overhead-plan.md). The executable fixture builder
`fixture()` in [`test_overhead_analysis.py`](../tests/test_overhead_analysis.py)
provides a complete small example. Its hashes and samples are synthetic and must
never be used as a pilot decision or measured evidence.

Invoke `python3 observability/tests/overhead_analysis.py input.json`. JSON is
written to stdout; exit 0 means `pass` or `pilot_complete`, exit 1 means `fail` or
`inconclusive`, and malformed JSON/CLI input exits 2. Check `status` and `phase`,
not just the exit code. The pure-Python API is `analyze(document) -> report`.

The strict [raw capture adapter](overhead-input.md) preserves source/configuration
provenance and refuses missing collection attestations before producing this input.

## Frozen plan and chronology

Top-level keys are `schema_version: 1`, `phase: "pilot" | "confirmatory"`,
`frozen: {...}`, and `runs: [...]`. The runner retains this input unchanged beside
the report, raw logs and executable provenance. Analysis compares recorded hashes;
it cannot establish the truth of collector attestations or prove when a decision
was frozen. The runner owns that evidence and the wall-clock run budget.

Required `frozen` fields:

| Field | Meaning |
| --- | --- |
| `decision_id` | Nonempty predeclared plan reference for pilot; frozen pilot decision reference for confirmatory |
| `measured_waves` | Fixed positive count, identical in every run; maximum 10,000 |
| `min_preparation_samples` | Frozen minimum **per stage per arm per run**, 1–100,000 |
| `min_tail_samples` | Frozen minimum number of samples strictly above the nearest-rank p95 position; at least 2 for confirmatory, 1 for pilot |
| `sample_scope` | Exactly `"shipping"`; benchmark-only probe variants require separate analysis |
| `time_unit`, `resolution_ms` | Exactly `"ms"` and positive raw clock resolution; existing `admission.prepared.preparation_ms` uses integer milliseconds, so record `1` |
| `measurement_scope` | `{ "name": "<explicit rollout scope>", "backend_included": ["<process/service>", ...] }`; use `[]` when none; list what B actually includes |
| `bootstrap_seed`, `bootstrap_replicates` | Frozen integer seed 0–2³²−1 and 1,000–20,000 draws; no adaptive choice after looking at results |
| `stages` | Ordered list of 1–16 `{ "name": "<unique stage>", "successful_sources": <fixed positive count> }` |
| `contamination_limits` | Nonempty map from named host measurements to finite nonnegative upper limits, in explicitly documented units |
| `expected_identity` | Identity object described below |

`expected_identity` contains `shipping_binaries_sha256` (nonempty executable-name
to SHA-256 map), `test_binary_sha256`, `source_revision`, `protocol_revision`,
`common_config_sha256`, and `arm_config_sha256: { "A": "<sha256>", "B": "<sha256>" }`.
Use lowercase full hashes. The common configuration digest must cover the frozen
compiler/profile, CPU allocation, workers, genesis/native-L1 profile, protocol
policy, INFO JSON logging, staged workload, identical backend reservations and
resource sampling settings. Arm digests cover the exact intended A/B telemetry
configuration, including exporter/sampler/queue/spool/scrape settings. Keep the
human-readable preimages with the evidence. Record source/config worktree content
identity as appropriate: a Git HEAD alone does not identify dirty source bytes.

`runs` is the actual chronological sequence, never sorted after collection.
Pilot has exactly four runs: pair 1 A,B; pair 2 B,A. Confirmatory has exactly twelve:
pairs 1–6 in AB,BA,AB,BA,AB,BA order. Never append pilot runs to the confirmatory
document or replace an inconvenient arm. Missing runs invalidate the set.

## Run records

Every run requires:

| Field | Meaning |
| --- | --- |
| `run_id`, `pair`, `arm`, `phase`, `sample_scope` | Unique string, one-based pair number, A/B, same document phase, and `"shipping"` |
| `identity` | Same fields as frozen expected identity, but `arm_config_sha256` is the single digest for this arm |
| `measured_waves` | Actual measured count, equal to frozen count |
| `semantic_ok` | Boolean attesting the full native workload's semantic assertions and independent follower/root agreement, not merely receipts |
| `telemetry_healthy` | Boolean attesting complete intended collection with no unexpected trace/spool/export/reader loss; A attests its intended baseline collection |
| `sample_overflow`, `logger_gaps`, `budget_exhausted` | Required booleans, all false for valid evidence; preserve observed true values |
| `lock_probe` | Exactly `{ "enabled": false }` |
| `contamination` | Actual measurements for every frozen contamination limit, in the same units |
| `stages` | Exact frozen stage order and population, using the fields below |
| `resources` | Optional `{ "native": <raw scoped totals>, "backend": <raw scoped totals> }`; these are retained separately, not used to claim a resource acceptance result |

Each stage has `name`, `successful_sources`, `elapsed_seconds`,
`preparation_ms: [<raw sample>, ...]`, and
`preparation_group_counts: [<positive group count for the corresponding sample>, ...]`.
The elapsed time is monotonic first measured submission through final canonical
effects/root agreement, excluding startup/build/warm-up. Durations come only from
the common `eez.composer.admission.prepared` event for measured intended user work.
Do not substitute anchor-only events, warm-up samples, settlement latency or
instance histogram quantiles. Keep attempt/group identity and log extraction
provenance in the evidence bundle. Repeated attempts remain samples with their
actual group count; never interpret them as independent runs. Maximum samples are
100,000 per stage; input files are bounded to 32 MiB.

The collector must detect overflow/gaps rather than truncate an array and report
it as complete. Missing booleans/measurements are unknown, never implicit success.
Unknown extra fields do not establish any acceptance criterion.

## Decisions and interpretation

For each stage separately, compute source count/elapsed seconds and nearest-rank
raw preparation p95 in each run. Pair ratios are B/A. Throughput regression is
`1 − B/A`; preparation regression is `B/A − 1`. Report all ratios, median paired
regression, median absolute deviation, min/max and a deterministic two-sided 95%
percentile bootstrap interval of the median, resampling whole pairs. Correlated
samples never increase the number of run pairs. Group-count distributions and
raw-stage sample/tail counts remain visible. The native result passes only when
**every** stage's throughput and preparation estimate and conservative upper bound
are strictly below 5%. Exactly 5% fails; an estimate below 5% with an upper bound
crossing the limit is inconclusive.

For preparation, the conservative upper bound bootstraps
`(B_p95 + resolution_ms) / (A_p95 − resolution_ms) − 1`. The actual
[`Attempt::prepared` producer](../../crates/eez-composer/src/composer/admission.rs)
uses `Duration::as_millis()`, which **floors** values into
`[recorded, recorded + tick)`. The analyzer intentionally retains a symmetric
one-tick allowance as **extra conservatism**; the floor-only upper bound would be
`(B_p95 + tick) / A_p95 − 1`. The report explicitly records floor rounding and this
extra allowance. Neither bound subtracts instrumentation or probe costs.
A p95 at or below one tick is unresolvable under the chosen allowance. Even identical short
durations can remain inconclusive when clock precision cannot distinguish 5%.
This six-pair bootstrap is the predeclared uncertainty method, not a guarantee of
frequentist coverage or a substitute for pilot precision assessment.

Semantic failure, telemetry loss, overflow/gaps, wrong identities, probe-enabled
data, wrong order or changed populations fail the set. Missing/insufficient data,
contamination and exhausted budgets remain inconclusive. Failed/contaminated runs
remain in `runs`; no favorable subset is computed. Pilot reports `pilot_complete`
only for intact data and exposes diagnostic metric decisions; this never means
confirmatory acceptance. `full_r27_status` remains inconclusive even after native
pass: backend/resource requirements and the separate benchmark-only queue-lock
probe/calibration/control experiment still need review. Preserve the probe's own
binary digests and raw wait/hold data in a separate document; this shipping
analyzer intentionally has no path that mixes or cost-adjusts those samples.
