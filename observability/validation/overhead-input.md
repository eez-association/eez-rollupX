# Raw overhead evidence adapter

`overhead_input.py` is an offline adapter to the existing
[analysis contract](overhead-analysis-schema.md). Its synthetic tests establish
conversion and rejection behavior, not measured overhead acceptance. It never
chooses wave counts, contamination limits, precision or confidence settings from
observed performance. A separate frozen plan is mandatory.

The Python API is `adapt(raw_artifact, plan_artifact, artifacts)`. Each artifact
contains `value` and the SHA-256 of its original bytes. The bounded CLI loads
these itself; in-memory callers must preserve that provenance. `artifact(value)`
is a convenience for synthetic fixtures and hashes canonical JSON, not a claim
about bytes from an existing file.

```bash
python3 observability/tests/overhead_input.py \
  --raw /path/to/capture/report.json \
  --plan /path/to/predeclared-plan.json \
  --output /path/to/adapter-report.json
```

The result is `ready` only if all required evidence is consistent. Its
`analysis_input` can then be passed unchanged to `overhead_analysis.analyze`.
`ready` means valid conversion, not that the statistical gate passes. Missing,
failed, partial or incompatible captures return `inconclusive`, retain raw runs,
source/candidate/file hashes and issues, and set `analysis_input` to null. No
favorable subset is built. A feature-enabled benchmark candidate is rejected
even when its probe flag is false. Current captures lacking the new effective
configuration/completeness records remain diagnostic; those records must not be
retrospectively invented.

The CLI reads only immediate owned `pair-arm` directories beside the raw
report, validating integer pairs 1–6 and literal A/B before constructing paths.
Relocating the whole bundle preserves its original absolute directory strings
for normalization/provenance; they are never used to find local files. Escaped
local symlinks and directory aliases are rejected. JSON is limited to 32 MiB per file and
128 MiB combined input; output is capped at 32 MiB. Nonfinite JSON is invalid.
Raw files remain untouched. Output must be a new file: direct/symlink input aliases
and existing destinations are rejected, and output creation is exclusive. Native execution, network requests, builds and
resource changes are outside this adapter.

## Frozen plan and exact identities

The plan has `schema_version: 1`, `phase`, and the existing analyzer's `frozen`
object, plus `config_preimages: {common, arms: {A, B}}`. The canonical hashes of
these explicit preimages must equal the predeclared `expected_identity` hashes.
Canonical JSON uses sorted keys, compact separators, ASCII escapes and no
nonfinite numbers. All generated observations must match; the adapter does not
rewrite the frozen hashes to fit actual runs.

For this fixed harness, `frozen.measurement_scope.backend_included` must contain
exactly `observer`, `postgresql`, `otlp_http_sink` and `metrics_scrape_transport`,
without duplicates. The freeform scope description is retained. Tempo or
Prometheus storage cannot be claimed from this evidence.

Shipping binary keys are `eez-composer`, `eez-proof-signer`, `eez-follower` and
`eez-observer`; their hashes and the native test hash come from the recorded
candidate manifest. `source_revision` is the complete `source_map_sha256`, not
Git HEAD: this binds dirty source content. Runtime-source reuse and every other
candidate provenance field are retained for review. The recorded compiler hash
must agree with `rustc.txt` in that manifest. These are identity comparisons;
they do not independently establish the truth or timing of collection.

The plan also declares exact integer `frozen.arm_seconds` (1–1800) and
`frozen.experiment_seconds` (1–21600, at least one arm). They must match the raw
report, every actual arm configuration and `common.workload`; their values are
therefore bound by the common preimage hash. Actual reported arm and experiment
elapsed seconds must be finite, positive and within those bounds, even when
completion flags claim success; missing elapsed evidence remains unknown.
Measured waves are integers 1–64.
Default collection still uses one wave, 600 seconds per arm and 9000 seconds
overall. Larger test-only capacity never relaxes sample minima or statistical
thresholds and cannot be selected retroactively from favorable results.

The plan also declares `frozen.preparation_clock`. The compatibility clock is
`{source: "admission.prepared.preparation_ms", unit: "ms", resolution_ms: 1}`.
Existing integer milliseconds are never reinterpreted as nanoseconds. A new
producer may explicitly declare
`{source: "admission.prepared.preparation_ns", unit: "ns", resolution_ns: 1}`.
Only that clock permits conversion of `preparation_ns` to fractional milliseconds;
its parallel legacy integers must equal floor(ns / 1,000,000). Raw nanoseconds
and legacy integers remain visible. The frozen analysis resolution must be at
least the declared producer resolution converted to milliseconds. No precision
is selected from favorable observed values. Values beyond exact integer numeric
representation are rejected.

## Per-run configuration and completeness

Each run retains `config.json`, `ready.json`, the three native role profiles,
`effective_config.json`, `native-effective.json`, `resources.json`, `evidence.json`, `result.json`, actual
`l1-genesis.json`/`l2-genesis.json` and `genesis_evidence.json`. Report/result and
native evidence must agree byte-independently as JSON values; original file
hashes are retained separately.

`effective_config.json` has exactly:

- `schema_version: 1`;
- `common`, with `compiler`, `profile`, `cpu_allocation`, `workers`, `genesis`,
  `native_l1_profile`, `protocol`, `logging`, `postgres_reservation`,
  `resource_sampling`, `observer_indexer` and `workload`;
- `telemetry_enabled` and `observer_running`, matching the actual ready process
  identities and enabled arm.

The six native common fields (workers, profile, protocol, logging, observer
selection and workload) must equal the original `native-effective.json` fields;
that original artifact is retained and hashed too. Here profile is the
`native_l1_profile` field, separate from the compiler build profile.
Common values describe effective or explicitly reviewed runtime-default policy.
Workers are configured policies, not sampled dynamic thread counts. Workload is
`{measured_waves, stages, arm_seconds, experiment_seconds}` with the exact
ordered stage names/source counts and declared budgets.
PostgreSQL memory/CPU reservations and resource/metrics sampling cadence are
checked against the resource report. `resource_sampling.max_samples` must equal
the actual resource report capacity and `arm_seconds + 32`, so the per-arm
capacity cannot be parsed and ignored. The PostgreSQL container name and first/last
sampled native PIDs must belong to that run. Effective metadata is produced by
the harness; copying expected plan values into missing observations is invalid.

Only these ephemeral profile fields are normalized:

- the exact absolute run directory and its exact `lifecycle` child;
- ports of literal loopback native metrics listeners;
- the trace receiver port, after it equals the exact owned receiver in arm config.

Hosts, role identities, exporter settings, trace sample ratio/queue limits and
all other profile fields remain in the digest. An unknown profile field is not
discarded; it must match the frozen preimage. B must expose all configured role
metrics and an actual observer PID; A must report telemetry disabled and no
observer process while preserving the identical PostgreSQL reservation.

Genesis has one separately declared mutation: testkit uses the same current Unix
seconds value for both fixture chains. Each `common.genesis.l1/l2` contains
`{template_sha256, timestamp_policy: "testkit_current_unix_seconds_shared_l1_l2"}`.
The template hash covers the complete canonical actual genesis JSON after
replacing **only its top-level timestamp** with `"<RUN_UNIX_SECONDS>"`. Balances,
code, chain configuration and every other field must remain identical. Both
actual timestamps, exact JSON hashes and header hashes are retained. Per-chain
`genesis_evidence` contains `timestamp_seconds`, `json_sha256`, `header_hash`,
`rpc_genesis_hash`, and `header_hash_source: "eez_testkit::genesis_block_hash"`.
The existing native helper must agree with RPC block zero. Python implements no
protocol header hashing algorithm.

Native evidence requires explicit `semantic_ok`, `telemetry_healthy`,
`sample_overflow`, `logger_gaps`, `budget_exhausted`, `preparation_clock`, stage
records and collection health. Missing booleans never become false/success.
Producer completeness claims require their own native/log validation; the adapter
cannot manufacture them from an absence of error text.

## Time and resource scope

Stage throughput time must equal the sum of actual measured wave intervals, with
the exact frozen successful source count. The larger outer resource bracket and
post-wave observer catch-up must not substitute for throughput time. Resource
phases must include `observer_catchup_and_health` and the overall bracket.
Native role CPU/RAM and observer/collector/PostgreSQL costs remain separate.

Supported frozen upper-limit contamination keys are `other_cpu_cores` (the
resource sampler's conservative upper bound), `cpu_steal_percent`,
`cpu_iowait_percent`, `major_faults` (host pgmajfault count), `swap_in_pages`, and
`swap_out_pages`. CPU measures use the maximum of overall and individual phase
brackets. Fault/swap counts use the single overall bracket; overlapping phase
brackets are never summed. Missing measurements and unsupported keys remain
unknown. The adapter does not pick the limits, discard noisy runs, or claim that
this limited backend profile includes Prometheus/Tempo storage.

Local validation: **17 adapter tests and 48 combined adapter/statistical/lock
analysis tests passed** in 1.060 seconds (synthetic evidence only). Tests include
actual relocated-bundle CLI loading with absent original paths, exact pair/arm
path bounds, direct/symlink output aliases and exclusive destination
creation, exact input hashes and budgets, boolean schema rejection, ephemeral
normalization, allowed genesis timestamp changes, ns/ms precision, missing
attestations, failed/partial pairs, feature candidates, original native-effective field equality, backend scope
claims, and resource/run identity.
Log: `/tmp/eez-overhead-offline-adapter-reviewed.log`.

The necessary test-only capacity extension subsequently passed **19 adapter /
50 combined pure analysis tests** in 1.250 seconds, including the complete
64-wave, 1800-second-arm, 21600-second-experiment path and its 1832-sample resource
capacity, invalid types/ranges and mismatched actual budgets. Log:
`/tmp/eez-overhead-offline-adapter-budget.log`. These are synthetic propagation
tests; they do not constitute a larger measured experiment or permit changing
statistical criteria. The integrated 107-case checkpoint is recorded in
[Python/config validation](python-config-final.md).
