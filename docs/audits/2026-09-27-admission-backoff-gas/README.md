# Confirmed-failure backoff and companion reserve refinement

Status: implementation and local validation in progress; this revision has not
yet been deployed or measured on Chiado.

| Field | Value |
| --- | --- |
| Requested change | Wait for five errors before shrinking admission; improve measured gas estimation |
| Branch / baseline checkout | `feat/cross-chain-admission-throughput` / `69359e362476108fcfb6a030f07a8b289c800c0f` |
| Baseline runtime | `6c171fdb3221a6d6183e113e9d05e4c7f657099b` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Tools | Rust 1.94.0, Foundry 1.7.1, locked dependencies |
| Requirements | [Accepted follow-up](../../proposals/cross-chain-admission-throughput.md#september-27-failure-tolerance-and-companion-reserves-accepted-for-implementation), [settlement policy](../../configurable-settlement-policy.md), [ownership contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission) |
| Prior comparable wave | [Preparation iteration](../2026-09-27-admission-preparation/README.md): 870 sources, 770 semantic jobs, 28 posts, 206.023 seconds |

## Changes

Automatic admission preserves selection through four consecutive canonically
confirmed, charged incomplete settlements. The fifth and later failures halve
the next allowance; complete canonical settlement resets the streak. Skipped
slots, uncertain observations and preparation/dispatch do not train it. Real
pipeline deadline guards remain immediate. Numeric admission is unchanged.

Canonical reconciliation allows automatic sources six cumulative charged
failures, so the fifth can receive its smaller retry. Prover/immediate relay
rejection keeps three on the existing cumulative source counter. No new durable
queue, worker, wire field or protocol rule is introduced. Per-source dispositions,
nonce dependencies and old/replacement ownership remain authoritative.

| Companion reserve | Before | Candidate |
| --- | ---: | ---: |
| Root | 80,000 | 70,000 |
| Callback | 40,000 | 35,000 |
| Nested result row | 30,000 | 25,000 |
| Input/result word | 2,500 | 2,300 |
| Ordinary preceding identity scan | 3,000 | 2,600 |
| Matching identity scan | 12,500 | 12,500 |
| Revert span | 25,000 | 25,000 |

The existing bounded shapes, full-prefix scan allowance, application gas,
declared-limit fallback, signed-start constraints and final ordered execution
remain. Posting storage prices are unchanged. Policy is
`confirmed-failure-backoff-4`; estimator is `companion-reserve-7`. Candidate
`started` details and Grafana's preparation table expose the streak and threshold;
historical/numeric rows remain null. Existing collectors require no migration.

## Initial evidence

- The new five-failure regression failed on the unchanged baseline at failure
  one: selection was 40 instead of 100. The candidate Composer suite passed
  269 tests, with one existing manual diagnostic ignored.
- Baseline companion fixtures passed nine tests / 27 logged measurements.
  Candidate gas fixtures passed 51 tests on each of Cancun, Prague and Osaka;
  all 70 integration contract tests passed. Eighteen new measurement cases cover
  short/long result storage boundaries and 128-entry scans. All 27 matched
  reserves decrease; the smallest measured reserve/execution ratio is 1.0863.
  The fixture measurements include application work, which production estimates
  additionally charge. These bounded measurements are not universal gas bounds.
  See [gas results](gas-tests.json) and [comparison](gas-fixture-comparison.csv).
- The first process run demonstrated four full-size retries, halving after the
  fifth exclusion, and complete recovery of all four sources. Its new assertion
  incorrectly indexed an absent optional tracing field: `None` is omitted, not
  serialized as JSON null. The assertion has been corrected to accept either;
  the original failure and complete process diagnostics are retained. A passing
  rerun is required before deployment.

Raw evidence is local-only under `artifacts/admission-backoff-gas/` and is not
available from another checkout. No private operator inputs are included here.

## Remaining work

Complete focused process gates and workspace Clippy, build and verify exact
local images, deploy under the existing Chiado authorization, run the same
verified workload, measure canonical inclusion and gas, then finish broader
applicable regressions. Record source/image identities, failures and exclusions.
Independent replay must be distinguished from live semantic workload checks.
