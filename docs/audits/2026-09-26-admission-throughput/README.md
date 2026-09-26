# Admission throughput implementation and validation

| Field | Value |
| --- | --- |
| Date/status | 2026-09-26; implementation and validation in progress, deployment pending |
| Authorization | User requested a new branch, implementation, local tests, local images, deployment and measurement with correctness checks |
| Branch/base | `feat/cross-chain-admission-throughput`, fork baseline `7c90266a5fe8cfdb46b249c61079badc2dce71a3` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d`; no pin/ABI changes |
| Toolchain | Rust 1.94.0 with locked dependencies; Foundry 1.7.1 |
| Design | [Accepted implementation direction](../../proposals/cross-chain-admission-throughput.md), [runtime policy](../../configurable-settlement-policy.md), [RPC cutoff](../../../crates/eez-control-rpc/SPEC.md#63-retry-procedure-and-settlement-cutoff) |
| Earlier evidence | [September 25 waves](../2026-09-25-latest-wave-analysis/README.md) |

## Implementation

- Exact-target cross-chain preparation and proving share the exclusive target
  timestamp minus submission slack. Finalization/delivery are reserved. Typed
  retries and cache repair stay inside the original window; uncertain delivery
  retains ownership. Ordinary-only and `NextBlock` timing behavior is preserved.
- Cold automatic admission considers the finite queue without a mandatory
  one-source calibration post. Preparation overruns alone do not discard an
  accepted candidate or halve capacity. Bounded workload-aware timing history
  replaces bootstrap estimates only using equal-or-larger measured requests.
- Gas admission discounts fixed words observed nonzero in retained deferred
  storage at the exact parent. Unknown shapes and dynamic slots keep existing
  reserves. Full-ABI sizing, signed gas checks, native complete-effect validation,
  grouped calls and original target binding remain authoritative.
- A canonically proven skipped target does not measure capacity, including when
  replacement consumption makes the old source dispositions mixed. Produced
  targets still apply normal partial-consumption checks.
- Candidate/source gas and timing events use the retained tracing sink and the
  existing collector checkpoint. Additive SQL views and **EEZ Cross-chain
  admission** show estimates versus final preflight and canonical receipt gas,
  source decisions, rebuilds, delivery and missing-event coverage.
- The staged workload wrapper adds `--verify`, preserving its shared-wallet lock
  and default throughput-only mode.

## Evidence so far

All large artifacts below are **local-only**, under ignored
`artifacts/admission-throughput/`; they are not available in another checkout.
A sanitized final summary and executable/image identities will accompany the
completed validation record.

| Check | Kind | Result |
| --- | --- | --- |
| Baseline `automatic_selection_learns_once_then_uses_finite_snapshot_and_actual_time` in a tracked-source archive | Unit | 1 selected/passed; establishes the original one-source cold selection (`baseline/cold-admission.log`) |
| Composer/driver/L1 unit suites after shared deadline and delivery wiring | Unit/local RPC | 237 Composer passed, 1 existing ignored; 57 driver passed; 141 L1 passed (`runtime-unit-tests.log`) |
| Retained-word gas helper and workload calibration | Unit | Later Composer runs: 238 and 239 passed, 1 existing ignored; further changes require final rerun |
| Original gas pin fixture | Embedded EVM | 22 passed (`gas-pins-baseline.log`) |
| Retained-word transitions: old/new 0/31/32/64-byte return data, 1/10/127 entries, changed hashes/roots, pinned storage layout | Embedded EVM | 26 passed (`gas-pins-candidate-2.log`); native repeated-wave validation pending |
| Indexer existing tests | Unit/mocked HTTP | Passed after allowing disposable localhost servers; database recovery test intentionally opt-in |
| Admission schema, replay, duplicate/conflict atomicity, gaps and every actual dashboard SQL statement | Unit/disposable PostgreSQL | 2 passed, zero skipped (`admission-db-tests-2.log`) |
| Staged wrapper profile and semantic-mode forwarding through wallet lock | Mocked shell | 4 passed (`staged-runner-tests.log`); does not establish live correctness |
| Cold/warm Gnosis waves with follower | Native process | 1 selected/passed: 30 sources in one first posting without calibration, then 30 in one second posting; canonical receipts, values/nonces and independent replay asserted (`native-cold-gnosis.log`) |
| Workspace Clippy, all targets | Static | Passed with `-D warnings` (`workspace-clippy.log`) |
| Cancun/Prague/Osaka gas pin fixtures | Embedded EVM | 26 passed per fork, zero skipped (`gas-pins-{cancun,prague,osaka}.log`) |
| Existing indexer canonical restart/reorg/checkpoint recovery | PostgreSQL/mocked RPC | 1 selected/passed (`indexer-recovery.log`) |
| Chiado configuration propagation | Mocked Compose/config | 13 passed; Chiado and Kurtosis policy checker passed |

### First failures retained

Sandbox socket restrictions prevented 13 L1 RPC tests and one indexer HTTP test
from binding localhost. Their logs were retained; the authorized local-server
runs subsequently passed. These were environment failures, not retry-only
application passes. An initial new Solidity test combined all retained-slot
transitions into one test and exhausted the test transaction gas budget. It was
split into four tests by prior return-data shape, preserving every transition;
all pass. Initial compile errors while extending internal test fixtures and the
submitter helper are retained in iteration logs.

## Operations and remaining validation

The original operator configuration, keys, datadirs and running Chiado services
have been preserved. A separate owned PostgreSQL container,
`eez-admission-test-db-20260926`, uses localhost port 19433 and database
`eez_indexer_test`; production monitoring uses a different container/port. No
production schema reset or contract deployment is part of this change.

Outstanding: final unit/format checks, wider native semantic/replay and
reconciliation regressions, image builds with recorded source/digests, additive
monitoring rollout, queue-drained local-image rollout, comparable baseline/candidate
waves, semantic verification and live Grafana coverage. Earlier receipt-only
waves are performance context and do not replace those correctness checks.
