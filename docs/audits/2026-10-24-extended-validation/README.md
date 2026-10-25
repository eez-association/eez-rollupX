# Extended branch validation

Status: execution validation complete with unresolved integration blockers,
2026-10-24 UTC. This extends the
[PR-readiness evidence](../2026-10-24-pr-readiness/README.md); earlier failures and
binary identities remain part of that record.

The subsequent `d46f0a14` CI exposed two additional process failures. Their
preserved evidence and the authorized ongoing corrections are tracked in the
[follow-up stabilization](stabilization.md); the successful runtime campaign
below does not erase those later failures.

| Field | Value |
| --- | --- |
| Authorization | The owner requested closing the remaining validation gaps and strengthening this branch, including sustained mixed traffic and comparison with the previous implementation. |
| Starting source | `b0195b0bc1ca90d71792fcfbc8613e8b00b00473`, clean dedicated branch `codex/l1-history-offline-proving`. |
| PR base | `a50c326d99f7cd1c21406eedbf370a344015a6b5`; compare supported behavior, not legacy wire compatibility. |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, clean pinned checkout. |
| Worktree | `artifacts/l1-history-offline-proving`, on `codex/l1-history-offline-proving`; the original checkout and its unrelated edits were preserved. |
| Live environments | Task-owned `eez-native-v1-review-20261024`, reference `eez-main-ab-20261024`, and fresh final candidate `eez-final-ab-20261024`. Other networks and wallets were outside operational scope. |
| Starting runtime | Verified CI `40104687331` roles from `319ebc2d`; final prefix-verification helper has its separate identity in the preceding audit. |
| Final tested runtime | `28049ff734ab931cbde01246c5d238cce6ef65bf`, exact producer/consumer identity in [final CI](ci-final.md). Subsequent source changes are protocol tests only; production bytes were not rebuilt for the final records. |

## Acceptance checks

- Compare the previous and candidate execution paths and tests for the supported
  profile. Identify intentional changes, exclusions and actual regressions.
- Reach the intended terminal assertions for both MissingProxy negatives; an
  earlier fee-quote rejection does not cover them.
- Exercise sustained mixed traffic with explicit resource and concurrency caps,
  semantic balance/state checks, queue observations and separate inclusion and
  settlement measurements. Record offered and accepted traffic separately.
- Exercise bounded failures during traffic and verify canonical recovery without
  claiming persistence of in-memory pending requests.
- Review the dependency gate and apply compatible fixes with affected lifecycle,
  streaming, signing and execution regressions. Do not waive advisories or
  replace the pinned cryptographic/API family just to clear the gate.
- Complete the candidate CI inventory, preserving first failures and exclusions.

## Evidence boundaries

Both the reference and final candidate campaigns have executed.
See the [live evidence](kurtosis/README.md) and its exact workload sources for
measured counts, semantic assertions and execution limits. Comparable
measurements require the same workload, resource allocation, build
profile and network rules. A replay or a large empty-block backlog is not a TPS
benchmark. Shared-host contention and deliberate maintenance must be reported.
Load phases must not overlap task-owned heavy builds. Abort only task-owned
work if resource headroom becomes unsafe; retain diagnostics before cleanup.

The Nethermind implementation is maintained externally and is not present here.
Portable conformance evidence cannot establish interoperability with an actual
Nethermind prover. The scope remains fresh networks with one native profile and
one v2 RPC service; legacy network migration is deliberately excluded.

## Investigation and correction records

| Record | Finding / validation boundary |
| --- | --- |
| [Functional parity](parity.md) | Static comparison against the exact previous fork; existing unsupported cases remain exclusions, not regressions newly attributed to this branch. |
| [Pending debug tracing](pending-debug.md) | A public RPC regression reproduced on the candidate and absent from the baseline; the correction includes real prefix execution and cancellation-safe shared tracing admission. The 33 unit tests pass, including two cancellation controls verified with a deliberately broken clone. The exact shipping process regression and the same live input both pass. |
| [Dependencies](dependencies.md) | The final candidate passes 1,571 workspace unit tests and both independent vendor regressions with the corrected Tokio lock. The crypto-bigint advisory still blocks the dependency gate. |
| [Dynamic admission](dynamic-admission.md) | The original CI failure is retained; successful state/receipt checks do not erase its failed packing assertion. Both exact diagnostic payloads and the final candidate pass the unchanged test. The retained timing/FIFO analysis does not establish the original failure's cause. |
| [Final runtime CI](ci-final.md) | Exact `28049ff7` shipping bytes: 1,571 unit tests, two separate Reth regressions, 174 process tests, 38 producer checks and 34 pinned protocol scenarios pass. The aggregate gate fails on crypto-bigint; excluded tests and optional telemetry limits remain explicit. |
| [Live campaigns](kurtosis/README.md) | Separate serial and concurrent offered workloads, cross-chain effects, fee accounting, signer interruption and cold replay. Reference evidence does not validate later candidate binaries. |

## Final local tooling and lint

Workspace format and strict all-target Clippy pass after the additional
protocol negatives. The protocol library passes **208/208** without filtering.
The full Python tooling run selected 417 tests: **410 PASS**, one environment
error, and six explicit opt-in skips. The relocation fixture failed before its
assertions because the host lacked the required `mold` linker. Supplying the
linker and missing shared libraries solely under task-local artifacts allows
the unchanged directed fixture to pass **1/1** in 60.234 seconds. No system
package, repository configuration, test or assertion was changed for this
correction. The first failed invocation remains recorded.

The [tooling record](tooling.json) contains commands, package checksums, log
digests and exclusions: 411 distinct tests pass across those executions, not a
single all-green full invocation. Six real Docker-image/Kurtosis tooling probes
remain explicit opt-in exclusions. This synthetic fixture validates packaging
behavior; the exact final runtime's provenance and executed process/live checks
have separate records.

## Result and remaining conditions

The final candidate passes the comparable serial/concurrent workloads and all
96 mixed cross-chain operations. Three separate 72-second interruption controls
verify ordinary execution and canonical recovery; delayed/missing L1 controls
also check history/PREVRANDAO and preserve every captured L2 block hash. The
combined collector authenticates 124 complete batches and exact fee/accounting
results. A fresh follower replays to block 1332 and checks 8,669 selected
receipts, including native transactions and two separately recorded preparatory
transfers. The live record owns the detailed counts, first failed preparations,
read-only parser correction, runtime identities and resource closure.

These observations do not establish capacity or a wall-clock block-delivery
SLA. The sequential settlement p95 is **31.60 s**, versus **25.41 s** in the
reference; the concurrent p95 is **52.26 s**, versus **53.02 s**. The slower
sequential result remains visible, without a causal claim from one campaign.
Local inclusion timings and future header timestamps are recorded separately.

The branch remains **not merge-ready**: crypto-bigint's security advisory is
unresolved. The original intermittent admission/packing failure also lacks a
conclusive diagnosis despite unchanged-test passes in subsequent CI and exact
local diagnostics. Actual Nethermind interoperability remains external; the
26 pre-existing signer-profile exclusions, non-durable in-memory queues and
unverified optional telemetry/transport paths are not presented as covered.

## Final independent source review

A separate reviewer compared the additional protocol tests with `28049ff7`
and the pinned signer profile. The production portion is byte-identical; the
new negatives reach the intended shape guards with accepted neighboring cases.
No material finding remained. The reviewer also checked the tooling, dependency,
pending-debug and parity records against their retained evidence, preserving
both the unresolved original admission failure and the limits of maintenance
recovery. This was a static review, not another test execution. The live and CI
results have their separate execution identities and inventories; review does
not substitute for those checks.

The final evidence review independently checked the CI inventory, the 124 linked
claims covering blocks 1–1332, and the range membership of all 8,669 selected
receipts. It compared the fault controls and fee summaries with their retained
outputs, including the slower sequential settlement p95 and the three-enclave
closure record. One wording correction makes explicit that the fee oracle does
not authenticate `refundTxHashes`; no material finding remained after that
correction. Blocks produced after the replay endpoint remain outside its scope.
