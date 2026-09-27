# Upstream Reth migration

Status: implemented in [PR #41](https://github.com/eduadiez/rollupX/pull/41);
local and CI validation passed before the documentation/vendor cleanup.
Final-head CI and review remain required. See the
[validation record](../audits/2026-09-26-reth-27429/README.md).

## Scope

Move from Reth `fd59fd2222b51239abebd9aa234f28b0b5f336eb` to unmodified upstream
`444e867155c2a7a4c028e479ca7201b0b6871579`, containing #27429. Official v2.6.0
was rejected after reproducing its stale-persisted-head forkchoice issue.
Align EEZ, Gnosis, Stateless, REVM, Alloy and the Rust toolchain with the chosen
revision; retain the dependency ports' provenance and regression tests.

Preserve protocol pin `9d13218a355e7d543ca8a9927eea2962600b716d`, genesis
commitments, independent signer validation, queue semantics and production
persistence defaults. Private miner persistence settings and restart fixtures
must distinguish graceful shutdown from forced-crash recovery.

This change targets fresh networks. Existing database migration and mixed-version
deployments are outside scope. The protocol test-runner correction investigated
during validation is a separate follow-up and is not required by this migration.

## Acceptance

| ID | Required result | Evidence |
| --- | --- | --- |
| R1 | Shipping binaries use the exact upstream pin | Locked dependency graph, production-feature builds, executable/image identities |
| R2 | Execution, signer validation and commitments retain their contract | Rust/protocol tests, Stateless checkpoint oracle, independent replay |
| R3 | Fresh deployment settles real cross-chain work | Kurtosis protocol suite, bidirectional waves, state chaining, real signer and canonical blob evidence |
| R4 | Reorg and restart behavior remains sound | Upstream forkchoice regressions, EEZ reorg tests, graceful restart and controlled masked-state crash recovery |
| R5 | Changes are reproducible and reviewable | Dependency provenance, build configuration and dated evidence retaining failures and exclusions |

The audit records completed checks and their limits. Passing startup or throughput
alone does not satisfy R2–R4; a later pass does not erase a recorded failure.
