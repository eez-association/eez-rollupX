# Bounded comparative telemetry overhead gate

Status: **measurement harness implemented and unit checked; no comparative result yet**. The proposal requires repeated
comparable staged workloads, throughput and preparation p95, CPU/RAM and queue-lock
measurements. Its provisional limit is less than 5% throughput/preparation-p95
regression. Neither a single canary duration nor the spool publisher microbenchmark
establishes that limit.

## Reuse and measurement boundaries

Use the existing native `dynamic_admission.rs::mixed_wave` workload and testkit,
not the Chiado performance script: that script targets a funded deployment and
uses `--no-verify`. The native workload already submits 64 inbound and 64 outbound
setters with deterministic values/nonces, checks every receipt, both final effects,
nonce deltas and independently derived follower convergence. Retain those checks,
plus the existing recomputed posted-proof signature assertion. Extract the workload
into its existing test module's helper only as needed for repeated stages; do not
replace the regression test or introduce a parallel transaction generator.

The initial sizing pilot uses `eez.composer.admission.prepared.preparation_ms`.
The next candidate adds `preparation_ns` from the same captured Duration, retaining
legacy milliseconds. Declare source/unit/resolution before collection; never
reinterpret older millisecond data as nanos. This is the common preparation clock. It is emitted by `composer/admission.rs::Attempt::prepared` from an existing
monotonic `Instant`, independently of optional observability. Keep automatic
admission and the same INFO JSON logging configuration in both arms. This measures
automatic admission through the start of proof generation, not end-to-end
settlement, signer time or an average of native histogram quantiles. The separate
`settlement.preparation_time` event describes anchor-only policy work and must not
be substituted. Verify nonzero intended user-group samples in both arms.

The one-/two-request smoke is insufficient for p95 or sustainable-throughput
claims. Repeated measured stages must include a small queue and finite busy waves;
use the existing setters and nonce discipline for all of them. Do not send a new
wave using stale on-chain nonces before the prior wave resolves.

## Fixed candidate and experiment

1. Build and freeze one complete shipping candidate and one measurement test
   executable, including final signer changes. Record full binary/source/protocol
   hashes, compiler/profile, genesis/native-L1 profile, CPU model/affinity, worker
   counts and effective protocol configuration. Both arms use exactly those bytes.
   Compile nothing during a measured run.
2. A disables the optional EEZ telemetry configuration. B enables the selected
   rollout profile: native metrics, the bounded lifecycle spool and tracing at an
   explicit fixed sample ratio with a healthy local collector. Keep identities,
   logging, protocol policy, fees, gas/blob limits and admission configuration
   identical. Keep the same pinned backend processes/resource reservations present
   in both arms; only the intended producer/collector activity changes. Record
   scrape cadence, sampler, queue limits and spool placement. A result for native
   instrumentation must not be labelled a full observer/PostgreSQL deployment
   overhead result unless those collectors are also included in the stated B arm.
3. Each run starts an isolated fresh world, performs the same small warm-up that
   trains automatic admission, and waits for its canonical completion. Exclude
   build/startup/warm-up from measured intervals. Preserve all failed-run logs;
   never rerun only an inconvenient arm and silently replace it.
4. Pilot two counterbalanced pairs (AB, BA), using the same staged workload and
   proposed resource collection. Record sample counts, elapsed times, preparation
   timing resolution, pair variation and environmental noise. Pilot data selects
   the fixed measured-wave count and a realistic bounded run budget; it is not
   counted as confirmatory evidence.
5. Freeze that count and run six pairs in predeclared order AB, BA, AB, BA, AB, BA.
   Defaults remain 10 minutes per arm and 150 minutes per runner experiment.
   The sizing pilot proved the original 12-wave ceiling insufficient for the
   required tail population. The test-only runner now permits an explicitly
   predeclared maximum of 64 waves, 1,800 seconds per arm and 21,600 seconds per
   experiment. A frozen plan must bind both budgets and account for pilot and
   confirmation together; increasing capacity does not change sample minima or
   the 5% threshold.
   Budget exhaustion, insufficient samples, semantic failure or an unmeasurable
   tail produces **inconclusive/fail**, not a relaxed threshold. If the pilot shows
   the cap cannot provide useful precision, record that and propose a larger
   dedicated window before starting the confirmatory set.
   Preflight capture size as well as elapsed time: the offline adapter accepts
   32 MiB per input file and 128 MiB across the complete input set. The runner's
   maximum time/sample settings do not guarantee that twelve resource captures
   fit that separate bound. Estimate the full bundle from the pilot's measured
   bytes per sample, including fixed records and repeated evidence in result
   files, before freezing the count and window. An oversized capture remains
   unusable for acceptance; do not omit raw samples or select fewer pairs after
   observing the result.
6. Use the same machine and CPU allocation, serial worlds and disjoint owned
   endpoints. Run after all Cargo/backend smoke activity is finished. Record
   system CPU/steal, memory pressure, swap, major faults and disk I/O; read only
   existing host state, never stop unrelated services to manufacture quietness.
   Predeclare contamination limits based on the pilot. Keep contaminated pairs
   visible and report them separately; do not select the fastest repeats.

## Outputs and decision

| Quantity | Measurement | Rule |
| --- | --- | --- |
| Successful throughput | Fixed semantically validated source count divided by the sum of per-wave monotonic first-submission to final canonical effect/root intervals | Include admission/submission/settlement cost; exclude inter-wave provider/nonce setup and independently recomputed signature verification; retain outer resource brackets separately |
| Preparation p95 | Raw common `admission.prepared` durations for measured user work, with attempt/group counts and explicit source clock resolution | Compute quantiles from samples, never average instance p95s; report each workload stage and group-count distribution |
| Failures/retries/target misses | Existing semantic checks and structured events in both arms | No lost work; timing improvements caused by omitted/failed work are invalid |
| CPU/RAM | Same external sampling of Composer/signer/follower and collection processes, process start identities retained | Report CPU seconds per successful source, peak RSS and sampling cadence; backend load is separate and included in total rollout cost |
| Storage/collection | Spool bytes, queued/dropped events and trace/export/reader health in B | Healthy-profile overhead requires no unexpected loss; outage testing is a different experiment |
| Queue-lock hold and wait | Optional actual HeldPool guard probe below | Do not infer from preparation latency, snapshot-call latency or futex counts |

Report all paired ratios, median/dispersion and a predeclared paired confidence
interval that resamples whole run pairs rather than treating correlated
preparations as independent runs. For the provisional gate, require both the point
estimate and the conservative upper uncertainty bound to remain below 5%
regression, with sufficient measured tail samples and no semantic regression.
Use raw-stage quantiles plus the paired run distribution; do not pool away a noisy
or regressed stage. A wide interval crossing 5% is inconclusive, even when the point
estimate is small. The pilot must expose whether millisecond quantization makes
a 5% p95 comparison unresolvable. More clock precision, if required, must apply
identically to both arms and be measured separately before a claim.

## Queue-lock measurement implementation and remaining gate

`HeldPool` retains its private `std::sync::Mutex<PoolState>` by default. The
nondefault `benchmark-lock-timing` feature selects a wrapper around that same
mutex. It times acquisition and the actual guard lifetime using `Instant`; it
drops the inner guard before recording/serializing. Poisoning and recovery retain
the standard mutex contract. An explicit `EEZ_BENCH_HELD_POOL_TIMING=true` enables
samples only in that variant. Five focused tests and all 245 Composer tests pass
with the feature, including post-unlock recording, poisoning, disabled behavior
and finite sample capacity. Strict Composer/node Clippy passed for the variant.

The sample limit is 65,536 per process, followed by one explicit overflow marker.
Every sample carries sequence, pool, static callsite, wait/hold boundaries and
bracketed monotonic/wall clocks. The parser requires complete sequence coverage,
before/after measurement coverage, no overflow and consistent clocks. A guard
crossing a phase boundary makes that phase inconclusive; margin-only exclusions
are separately reported. Raw samples and callsite populations remain available.
These benchmark events are excluded from the ordinary exported-log allowlist.

The source E2E logger uses pinned Reth's JSON stdout layer directly (the
`formatter.rs::LogFormat::apply` no-file-writer branch), redirected to an owned
regular file. `NodeHandle` disables Reth file logging with `--log.file.max-files 0`.
The nonblocking lossy file-appender path is therefore not this measurement path.
Sequence checks still reject any missing probe records; bounded capture rejects
oversized logs. Both arms use identical synchronous logging configuration.

`common/overhead.rs` now reuses the existing mixed-wave generator/oracles, checks
an independently recomputed signature after every wave, reserves identical owned
PostgreSQL in A and B, and launches the actual observer only in B. The owned spool is mode 0700; enabled writer/trace gauges are checked before warm-up, and final observer validation covers the exact warm-up and measured source hashes, not only any previously committed canary. Observer catch-up/health work has a separate resource phase and is included in the overall resource bracket. Ready/start and
evidence/finish barriers let the external sampler bracket phases while every
sampled PID and PostgreSQL are still live. Four explicit queue-status RPCs per
arm provide probe boundary coverage outside the measured windows. B additionally
runs the normal native queue sampler and observer RPC polling; those extra calls
are part of its measured cost and remain identifiable by probe callsite.

`run-overhead.py` accepts a frozen candidate manifest and compiles nothing. It
retains failed arms, runs declared AB/BA order serially, enforces the fixed budget,
and reports raw capture rather than acceptance. `overhead_resources.py` measures
native processes, the collector/sampler and the whole owned PostgreSQL cgroup;
it records conservative sample brackets and host contamination. Use its overall first-stage-to-final-catchup bracket for total CPU cost, since per-phase sample brackets can overlap and must not be summed. A healthy OTLP
HTTP sink measures export cost, not Tempo storage. Native metrics scrape transport
is included at the configured cadence; Prometheus storage is outside this scope.

The controller reserves private database and socket/configuration parents before
launching each native arm. Its internal `observer_fixture` configuration binds
those parents and the exact container name. Native setup records the created
container ID; the controller owns teardown on both success and failure. Ordinary
functional fixtures retain their existing native cleanup. Repository-root
invocation supplies the native manifest directory when `CARGO_MANIFEST_DIR` is
absent; an explicit value takes precedence and is validated before launch.

Forced termination verifies the owned process session and reaps its leader before
database capture or removal. Failed arms with a verified container attempt bounded
database/log/memory capture outside CI log-upload directories. Capture deadlines are 15/5/5
seconds, with a two-second forced-reap allowance; stdout and stderr limits are
128/4/1 MiB respectively. Incomplete creation, preservation, or removal retains
the known owned paths and reports incomplete cleanup. Cleanup after a measurement
deadline never turns the arm into a successful measurement. Controller crashes
require explicit reconciliation using the retained `ownership.json` record.

Primary throughput/preparation evidence must use identical default shipping
feature binaries. Record the compiler/profile explicitly: optimized development
pilot bytes are not a release-profile deployment claim. The separate probe
variant needs on/off calibration and A/B lock comparison; never subtract an
estimated probe cost to manufacture a passing result. A four-arm optimized-development sizing pilot completed (overhead-pilot.json);
it has insufficient tails and explicit methodological limits. No live probe
calibration or confirmatory confidence decision has passed yet. The full R27 gate stays open
until real repeated results satisfy the preparation/throughput, resource and
actual-lock requirements together.


Capacity is validated in both Rust and the controller before startup. Omitted
native budget fields retain 600/9000-second defaults; CLI defaults remain one
wave, 600 seconds per arm and 9000 seconds per experiment. The experiment budget
must cover at least one full declared arm allowance. The controller applies an
absolute remaining deadline to each arm and checks total elapsed time again after
collector teardown and candidate rehashing. Cleanup remains mandatory after a
deadline, but cannot produce a successful budget result. Resource sample capacity
is the actual declared arm allowance plus 32 boundary samples, and is retained in
the effective configuration and resource report. Prospective longer studies need
a separately frozen count/budget decision; prior pilot limits remain unchanged.
