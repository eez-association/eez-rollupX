# Extended branch validation

Status: in progress, 2026-10-24 UTC. This extends the
[PR-readiness evidence](../2026-10-24-pr-readiness/README.md); earlier failures and
binary identities remain part of that record.

| Field | Value |
| --- | --- |
| Authorization | The owner requested closing the remaining validation gaps and strengthening this branch, including sustained mixed traffic and comparison with the previous implementation. |
| Starting source | `b0195b0bc1ca90d71792fcfbc8613e8b00b00473`, clean dedicated branch `codex/l1-history-offline-proving`. |
| PR base | `a50c326d99f7cd1c21406eedbf370a344015a6b5`; compare supported behavior, not legacy wire compatibility. |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, clean pinned checkout. |
| Live environment | Existing task-owned Kurtosis enclave `eez-native-v1-review-20261024`; other networks, wallets and worktrees are outside operational scope. |
| Starting runtime | Verified CI `40104687331` roles from `319ebc2d`; final prefix-verification helper has its separate identity in the preceding audit. |

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

No sustained-load or old-versus-new performance result is claimed by this plan.
Comparable measurements require the same workload, resource allocation, build
profile and network rules. A replay or a large empty-block backlog is not a TPS
benchmark. Shared-host contention and deliberate maintenance must be reported.
Load phases must not overlap task-owned heavy builds. Abort only task-owned
work if resource headroom becomes unsafe; retain diagnostics before cleanup.

The Nethermind implementation is maintained externally and is not present here.
Portable conformance evidence cannot establish interoperability with an actual
Nethermind prover. The scope remains fresh networks with one native profile and
one v2 RPC service; legacy network migration is deliberately excluded.
