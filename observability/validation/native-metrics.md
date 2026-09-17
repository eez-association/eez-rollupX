# Native operational metrics validation

This records the R20 implementation slice in the `feat/network-observability`
worktree, based on `5183f56acd0903a10ad085e97d03aef4070e82a3`. It includes the
agreed frozen main baseline `5469c88860408daf776ee812156dd280c1a9e262` and retains
protocol pin `3c0cdf4955941a5c23639346932b55b33babebbd`. The slice was uncommitted
when these checks ran; this is not a deployment or complete R20 acceptance record.

## Requirement coverage

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Monotonic current ages and direction partitions | `held_pool/status/native.rs`, existing queue transition hooks, node executor sampler | Five new queue/preparation Composer tests: pending vs serviceable; claim/recovery/removal/readmission; replacement/duplicate; missing/stale/reorg/legacy direction states; capture bound; metrics-only preparation |
| Native admission and terminal outcomes | Accepted ingress plus existing reconciliation before/after partition and shared local-removal capture | Existing ownership/reconciliation regressions; two new local metric tests exclude unchanged reservations, duplicate removal and unresolved canonical owners without lifecycle export |
| Completed vs pending distributions | Terminal duration histogram and independently sampled current age bucket gauges in `operational.rs` | Histogram output preserves all nine outcomes and unknown-duration coverage; current bucket boundary checks and stale values becoming unavailable |
| Finite preparation durations | Existing future owns metric timer, with the existing typed error classifier | One observation on finish/drop, cancellation and panic outcomes; result/bytes preserved without a lifecycle writer |
| Disabled/default and metrics-only configuration | Metrics activation precedes the lifecycle early return; per-pool optional clocks | Config test verifies default/lifecycle-only leave metrics disabled and metrics-only enables clocks with no writer; Composer disabled test uses separate test process/per-pool controls |
| No protocol-lock export or extra protocol task ownership | Constant-size snapshot after bounded scan; recorder calls after lock release; Reth shutdown-aware sampler | Source review, workspace Clippy and integrated native composer startup; no claim of measured overhead |
| Real process metric exposure and semantic compatibility | Existing two bidirectional lifecycle canaries additionally scrape the private native endpoint | Both integrated default/policy canaries passed; see execution record below |

## Executed checks

- `cargo +1.94.0 test --locked -p eez-composer --lib`: **239 passed**,
  including seven new metric integration regressions. Local log:
  `/tmp/eez-observability-native-metrics-composer-reviewed.log`.
- `cargo +1.94.0 test --locked -p eez-telemetry --features native`:
  **33 passed**, including four operational-metric tests and one activation test.
  The suite also covers transport and the concurrent tracing slice. Local log:
  `/tmp/eez-observability-native-metrics-telemetry-reviewed.log`.
- Rustfmt on owned source files and `git diff --check`: passed. The final shared
  workspace formatting/Clippy checks remain part of integration acceptance.

The first Composer run compiled during a helper rename and retained an older
telemetry dependency artifact; the second attempt exposed the concurrently added
tracing bridge's incorrect pinned SDK type paths. Both failures remain in
`native-metrics-composer-tests.log` and `native-metrics-composer-retest.log` under
`/tmp/eez-observability-`. The reviewed Composer run used corrected sources.

The first telemetry run rejected the activation test's precreated temporary spool
directory permissions. The fixture now lets the writer create a private child
spool; the reviewed full suite passed. The failure remains in
`/tmp/eez-observability-native-metrics-telemetry-tests.log`.

The tracing lane's production node check found the pinned Reth executor's
`spawn_task` takes only the future. The sampler now uses that supported API;
the later integrated source-based native run exercised the corrected runtime
startup. This was a compile error, not a runtime failure or an accepted exclusion.

## Remaining acceptance

The integrated source-based lifecycle canaries passed **2/2** (default and enabled
settlement policy) in 257.23 seconds. They checked real Reth-prefixed admission,
committed-terminal and completed-duration samples in both directions, plus empty
current pending populations, while retaining the prior chain/effect/proof assertions.
Log: `/tmp/eez-observability-integrated-native.log`; diagnostics:
`artifacts/native-observability/2026-09-17-integrated`. Workspace all-target Clippy
also passed after replacing three exact floating-point test comparisons with
epsilon comparisons. These are integrated runtime correctness checks, not
telemetry-overhead measurements or live deployment evidence.

At that checkpoint, mixed blocked/retry/replacement/reorg workloads, additional
finite stage timers, source-switch recording rules, runtime outage/storage/overhead
budgets and the full observability rollout remained open. The extension below
updates the stage/source-switch implementation status. See the [native metric contract](../contracts/native-metrics.md)
and [requirement ledger](../requirements.md). Counters are native observations;
this work does not establish observer-validated canonical success or deploy any
monitoring service.

## R20 stage and explicit source-switch extension (after c36bbd6)

The extension adds closed stage timers at existing Composer simulation, blocking
witness, submission and remote prover boundaries; signer RPC/ingestion/validation/
settlement/signing guards; active immutable policy and fresh canonical-policy
observations; finite planning-cut and exact-slot-miss counters. Metrics publish
outside protocol locks. The existing cross-chain process canaries now require
simulation/witness/included-submission histograms and active policy configuration.
The expanded default/policy canaries passed 2/2 in 257.21 seconds on the final
shared candidate, including those stage/policy assertions and sanitized client
metric inventory. Log: `/tmp/eez-observability-final-native-lifecycle.log`. The
inventory parser regression passed 1/1 separately against the same test executable.
The older result above describes its own earlier source scope.

| Requirement | Implementation | New coverage |
| --- | --- | --- |
| Finite stage lifetime/default compatibility | `operational::StageTimer`, Composer and signer existing owners; typed prover status | Timer tests verify one finish, disabled no-series, panic and cancellation; signer status labels never include messages; existing full service lifecycle tests |
| Planning count excludes blocked-only work | `Selection::count_limited`, existing cut branches | Exhausted with eligible remainder, fee-blocked remainder, exact exhaustion without remainder |
| Policy values reach producer | Validated `ComposerSettings` constructor plus canonical policy evaluation | Existing constructor/config tests and extended process assertions; fresh policy Grafana fixture |
| Explicit current-source switch | `prepare.recording_rules`, `queue_source`, generated manifest | Three parsing/reconfiguration tests; nine pinned-promtool cases for no fallback, failure, stale sample and clock boundaries |

Executed before the final prover-client wrapper addition:

- Composer **240 passed**, log `/tmp/eez-observability-native-stages-tests.log`.
- Full signer **285 passed** and native-enabled telemetry **37 passed**, log
  `/tmp/eez-observability-native-signer-tests.log`.
- Observer **65 passed**, log `/tmp/eez-observability-native-stages-observer-tests.log`.
- Python discovery **16 passed**, log `/tmp/eez-observability-native-python-tests.log`.
- `python3 observability/tests/test_queue_source.py --promtool`: **9 cases passed**
  against pinned Prometheus. This proves actual generated expressions, not live
  RPC/native age equivalence.

The first telemetry run's4socket-fixture cases failed due to sandbox-denied TCP/
Unix socket binding. The permitted rerun passed all37; no production fallback or
weakened test was introduced. Prover-client added after these package runs and
requires the coordinated final compile/client regressions. Clippy, process canary
and current dashboard assertion reruns belong to the final integration record.

## Subsequent integrated evidence

The [connected native trace pilot](native-trace-pilot.md) subsequently passed with
the actual Composer/client/signer path and independently verified protocol
semantics. Its final package checks passed 43 telemetry, 287 signer, 15 prover-client
and four control-RPC tests, plus strict Clippy for the affected native/RPC targets.
This resolves the earlier paragraph's pending client regression, within that
recorded source snapshot.

The [live observer scenarios](native-observer.md) additionally passed the
legacy/policy, partial-consumption restart and real reorg workloads. Actual native
process metrics remained available during collection and after observer restart.
[Dynamic disk exhaustion](native-enospc.md) and
[real native trace-loss notifications](native-loss-alerts.md) now verify protocol
progress and explicit loss/recovery in their stated failure scopes. These are
functional results, not an overhead comparison or a shipping-artifact claim.
The comparative workload, final complete candidate checks and deployment baseline
remain open.
