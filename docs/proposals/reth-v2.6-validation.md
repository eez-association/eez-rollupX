# Official Reth v2.6.0 validation

| Field | Value |
| --- | --- |
| Design status | Draft PR preparation; prior validation recorded; main-based candidate checks in progress; upstream forkchoice blocker remains |
| Date | 2026-09-26 |
| PR base | Latest `origin/main` fetched for preparation: `7c90266a5fe8cfdb46b249c61079badc2dce71a3`; unrelated admission-branch commits excluded |
| Prior validation baseline | `85f3691cb9c7ccbf8c0b8700576733283e5af7fc` on an isolated validation branch |
| Protocol baseline | `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |

The user requests a working official Reth v2.6.0 stack, validated locally with
Kurtosis. Existing database migration and mixed-version deployment compatibility
are outside this fresh-network scope. This proposal does not claim acceptance
of the implementation before the checks below pass.

The [completed validation audit](../audits/2026-09-26-reth-v2.6-validation/README.md)
records two passing full Kurtosis runs, independent replay, the original native
failures and their controls. R4 remains unmet for unmodified official v2.6.0;
passing normal settlement is not release acceptance.

The main-based PR separates graceful policy-change restarts from intentional
crashes, retains production L2 persistence defaults, and establishes persisted
fixture state before crash assertions that depend on it. The earlier `2/0/0`
controls diagnosed the test failures; they did not establish a production
durability requirement or zero-loss guarantee. Earlier binaries and Kurtosis
results do not validate this new base. Unmodified v2.6.0 remains blocked by the
independently reproduced #27429 forkchoice issue; no backport is authorized by
the official-release requirement.

## Implementation boundary

Use official Reth commit `73a3a00862a8f14f89e30da8de001456f18cfae0` without local
Reth patches. Align EEZ, the checkpoint-capable Stateless fork, Gnosis, REVM,
Alloy, and the Rust toolchain. Local dependency ports are needed because no
examined released/EEZ dependency revision targets this Reth release; keep their
source provenance and preserve the signer's independent validation. Retain the
current protocol, genesis commitments, signing gates, and queue semantics.

## Acceptance

| ID | Required result | Check |
| --- | --- | --- |
| R1 | Shipping binaries use the exact official Reth release | Locked dependency graph, shipping-feature builds, recorded image identities |
| R2 | Execution, signer validation, and commitments retain their contract | Relevant Rust/protocol tests, independent Stateless replay and malformed-input fixtures |
| R3 | Fresh deployment settles real cross-chain work | New Kurtosis enclave; complete eligible protocol suite, bidirectional waves and state chaining, real signer attestations, canonical blob/receipt evidence |
| R4 | Reorg and restart behavior remains sound | Persisted fork-choice regressions including upstream #27429's scenario, EEZ reorg tests, local node restart recovery |
| R5 | Changes are reproducible and reviewable | Dependency source provenance, toolchain/build configuration, test logs and an audit distinguishing failures, exclusions, and live evidence |

Validation owns enclaves `eez-reth260-20260926` and
`eez-reth260-20260926-v2`, and uniquely tagged candidate images. Existing services and operator state remain outside its scope. Use the
[Kurtosis guide](../../testing/kurtosis/README.md) and record failures before
cleanup. Passing startup or throughput alone does not satisfy R2–R4. A failure
on official v2.6.0 must be reported as such, rather than silently patching Reth
or weakening validation.
