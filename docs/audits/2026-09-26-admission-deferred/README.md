# Deferred posting storage estimates

| Field | Value |
| --- | --- |
| Status | Implementation and local validation in progress; candidate deployment/measurement pending |
| Authorization | User approved inbound storage/overwrite estimates and fallback telemetry; existing authorization includes local build, deployment and measurement |
| Branch / baseline | `feat/cross-chain-admission-throughput`, `bfcb68dce74b2196bff1df4a648ee0dc127f469d`; runtime baseline `036b50a3c7d0dee45e6308388a6233ded7a7d2ca` |
| Protocol | Clean `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Tools | Rust 1.94.0, locked dependencies, Foundry 1.7.1, Solidity 0.8.34 |
| Contract | [Proposal](../../proposals/cross-chain-admission-throughput.md), [settlement policy](../../configurable-settlement-policy.md), [monitoring](../../../monitoring/indexer/README.md#admission-history) |
| Prior evidence | [Baseline live wave](../2026-09-26-admission-shapes/README.md) |

## Implementation

The `deferred-storage-3` model bounds current and retained entry layouts before
walking them. It prices storage writes, nonzero fresh words and deleted tails;
refunds never reduce admission. Canonical-parent evidence includes inactive
mapping entries. No speculative writes earn reuse credit. Roots, rolling hashes
and nested position hashes are charged as nonzero before stitching. The previous
reserve remains available for unsupported or more expensive layouts. Immediate
pricing, whole-group ownership and final native validation remain unchanged.

Grafana exposes per-entry pricing-reason counts and a separate whole-prefix
discount revocation flag. New reasons are additive; old history remains null.
Keep upgraded monitoring views/indexer when rolling back Composer. No protocol,
signer profile, configuration, genesis or queue persistence changes are made.

Pinned CORE/EXECUTION_ENTRY prose still describes arrays/deletion; Solidity uses
retained mappings and active bounds. This is a documented source/specification
disagreement. Measurements and storage-layout assertions use the pinned Solidity
and existing fork settlement contract.

## Validation history

Full logs and datadirs are local-only under ignored `artifacts/admission-deferred/`;
they are not available from another checkout. Small final summaries will be
retained here with the implementation/image identities.

- Baseline pinned gas fixture: 34 passed (`gas-baseline.log`).
- Initial current Rust suite: 251 passed, one existing manual profile ignored.
- New/old admission history and all dashboard queries: two PostgreSQL tests
  passed, zero ignored (`indexer-db-first.log`). The task-owned disposable
  container is `eez-admission-deferred-db`, port 19433; production is separate.
- Workspace and indexer Clippy passed with `-D warnings` before final test-fixture
  edits. Native process validation is pending.
- Initial storage ladders passed three shapes; two maximum-size ladders exhausted
  Foundry's test transaction budget in fixture preparation. Setup/assertion work
  is now outside gas metering; the actual posting call remains metered and cold.
  All five ladders then passed. A later zero-payload extension exposed Solidity
  stack-allocation failures. Grouping fixture state in a memory struct resolved
  them; the resulting Cancun fixture passed all 39 tests. First failures remain
  in `gas-storage-first.log`, `gas-final*-*.log`.
- Initial Rust test compilation required explicit ABI struct fields; its first
  numeric fixture expectation omitted one fresh word and was corrected from
  343,000 to 358,000. These are test corrections, not passing initial results.

Remaining acceptance: final checks at the shipping revision, native semantic
and follower replay, image build/provenance, state-preserving deployment, live
wave and Grafana verification, comparison and owned-resource cleanup. Full CI
release/Kurtosis validation is outside this local iteration.
