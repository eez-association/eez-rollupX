# RollupX manager compatibility and simplification

| Field | Value |
| --- | --- |
| Date and status | 2026-10-28; implementation and local unit/contract validation complete; process/CI validation tracked below |
| Scope | Rename the manager and deployment initializer; simplify shared anchor validation without changing existing network rules |
| Baseline | `6215df1a4395f58bb58bdb78bf4dc22f907c4169`, PR 112 after merged PR 115; identical tree to green `f965725872be23dc514911abe73877053b4f7f65` |
| Candidate | This commit's `codex/simplify-rollupx-contract` changes |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged and clean |
| Tools | Rust 1.95.0, Foundry 1.7.1 (`4072e487`), Solidity 0.8.34, optimizer runs 1, via IR, repository relative remappings |
| Requirements | [Accepted scope and invariants](../../proposals/rollupx-manager-simplification.md) |
| Operation | No network deployment, upgrade, funding, restart or operator-state change |

## Compatibility findings

`RollupX` retains the reference core proof-system policy, ownership and root escape
implementation. Core's initializer, registration and custom-data methods are
nonvirtual. Direct inheritance cannot preserve this network's mandatory profile
initializer, genesis registration and context checks. No core source or gitlink
was changed, and no second production implementation or compatibility wrapper
was added.

The manager now selects an anchor once, performs one `BLOCKHASH` lookup and checks
its nonzero result. EVM availability rules reject current/future blocks and blocks
older than 256 with the same `BlockHashUnavailable(number)` error. The coordinate,
profile, registry, accepted-root and rule checks remain in the same order. The
profile and rollup ID are reused across a registry `STATICCALL`, which cannot
modify them. The two flat ABI encodings remain explicit; there is no production
assembly or additional context object.

Both `getCustomData` modes return identical bytes to the baseline for the same
proxy, state and L1 coordinates. Profile/domain hashes, validation version 6,
activation epochs, historical proof applicability, accepted roots, consensus,
native transactions, DA and public inputs remain unchanged. Existing followers,
providers and signers need no activation, configuration change or migration.
Historical streaming vectors and their provenance manifest remain unchanged.

The only function selector change is deployment-only:

| Initialization function | Selector |
| --- | --- |
| Baseline `initializeOffline(address,uint256,address[],bytes32[],Profile)` | `0xbe66e26d` |
| `RollupX.initialize(address,uint256,address[],bytes32[],Profile)` | `0x22e0da25` |

The Profile tuple retains exactly the same fields and widths. Every other method
identifier and all event/error ABIs are identical. The former initializer has no
alias. Existing proxies have already consumed their namespaced initialization
lock; upgrades use empty data. The historical names in
`OfflineProfileInitialized`, `InvalidOfflineProfile` and
`UnregisteredOfflineRollup` preserve observable event/error selectors rather than
introduce alternative implementations.

All eight storage entries, offsets, slots and complete types match
[the baseline layout](storage-layout.json). The unchanged OpenZeppelin
namespaced ownership/initializer storage and transparent-proxy admin remain in
use. Tests initialize the actual frozen baseline creation bytes, alter ownership
and proof policy, upgrade the same proxy, accept a pre-upgrade proof, reject
reinitialization and unauthorized mutation, then roll back with state intact.
The registry immutable must remain the same; tests and the
[upgrade runbook](../../runbooks/chiado-deployment.md#rollupx-manager-implementation-upgrades)
retain that requirement.

Composer fee estimation recognizes exactly two equivalent executable fingerprints.
It still executes the actual deployed code and retains final admission/execution
checks. The baseline fingerprint permits Composer-first rollout without forcing
an implementation upgrade; the new fingerprint permits estimation afterwards.
An old Composer rejects the new executable for conditional fee estimation, so
manager rollback precedes Composer rollback. Unknown/mutated code is rejected.
The retained baseline bytecode is test evidence, not a second runtime code path.

## Size and gas evidence

The frozen baseline and candidate use identical compiler settings. Full values
are in [gas.json](gas.json), produced by `RollupXTest` with cold proxy,
implementation and registry accesses. Foundry `lastCallGas` measures the context
call and, separately, the initializer delegatecall during atomic proxy creation.
A verbose trace confirms the latter; it is not the complete deployment transaction
cost. Each measurement is deterministic EVM gas, not a wall-clock benchmark.

| Size, bytes | Baseline | RollupX | Change |
| --- | ---: | ---: | ---: |
| Creation code, excluding constructor arguments | 7,020 | 6,954 | −66 |
| Deployed runtime including metadata | 6,663 | 6,597 | −66 |
| Executable runtime excluding the 53-byte metadata trailer | 6,610 | 6,544 | −66 |

| Operation | 1 epoch: baseline → RollupX | 64 epochs: baseline → RollupX | Gas change at all seven tested sizes |
| --- | ---: | ---: | ---: |
| Actual-parent context | 25,174 → 25,145 | 34,238 → 34,209 | −29 |
| Historical context | 25,186 → 25,057 | 34,250 → 34,121 | −129 |
| Initialization delegatecall | 221,699 → 221,347 | 724,442 → 724,090 | −352 |

Rule-table sizes are 1, 2, 4, 8, 16, 32 and 64. The gas assertions forbid
regressions on these paths. These small savings do not establish a material
total-post throughput improvement. Larger source reductions would require
removing active profile/security semantics or introducing abstraction overhead.

Creation/runtime hashes change, including source metadata. A new CREATE2
deployment using these bytes would have a different address; upgrading an existing
proxy preserves its address and profile. EEZ, EEZL2, history code and genesis
fixtures were not changed.

## Validation and retained first failures

Commands run from the isolated candidate worktree. Build outputs, full logs and
process datadirs are local-only under `artifacts/validation` and
`artifacts/native-e2e`; those ignored directories are not portable evidence.
The frozen baseline creation/runtime fixtures, compact layout and gas records,
and the tests themselves are checked in and reproducible.

| Requirement | Check and evidence type | Result at source preparation |
| --- | --- | --- |
| R1 | `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec -p eez-proof-signer -p eez-control-rpc -p eez-prover-client -p eez-composer`; unit/oracle | 1,058 passed, four explicit utilities/benchmarks ignored; includes independent public-input, rolling-hash and Ethereum KZG vectors |
| R1 | `python3 crates/eez-control-rpc/conformance/check_vectors.py`; frozen fixture/schema check | 29 vectors verified without regeneration |
| R2–R4 | `forge test --root contracts`; contract tests | 168 passed, including 21 manager tests and two 256-case differential fuzz tests |
| R2 | Full compiler storage layout and method/event/error ABI comparison; static | Identical storage/types; only the initializer function selector changes |
| R3 | Baseline-bytecode upgrade contexts; contract differential | Both modes, accepted roots, 64 rule epochs, zero/current/future/expired/unavailable anchors, invalid coordinates, registry mismatch and rule boundaries covered |
| R5 | Exact old/new fingerprints plus mutated and unrelated bytecode rejection; Rust and fixture/build coupling | Passing; final fingerprint regression rerun tracked below |
| R6 | `forge test --root contracts --match-contract RollupXTest -vv`; comparative EVM gas | Seven rule-table sizes, no measured path regression |
| Shared core contract | Pinned core unit suite with required exclusions | 778 passed, three network-fork tests skipped; exclusions listed below |
| Rust quality | `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| Formatting | Rust workspace and integration Solidity format checks; `git diff --check` | Passed |
| Deployment consumer | `python3 -m unittest discover -s scripts/ci/tests -p 'test_deploy_preflight.py' -v`; mocked preflight | 15 passed; no broadcast performed |
| Process and shipping artifacts | Source `cross_chain::`, protocol Anvil scenarios, CI release/Kurtosis lanes | See remaining validation below |

The baseline manager's original 16 tests passed before edits. Early refactoring
variants increased context gas: the first shared encoding added 85/55 gas and
the high-level memory-tuple version added 309/252 gas on actual/historical paths.
Those versions were discarded; the same gas guard passes on the selected version.
An initial gas-harness attempt used an empty transparent-proxy initializer and
correctly reverted `ERC1967ProxyUninitialized`; the harness now uses normal atomic
constructor initialization. These were implementation/harness iterations, not
retry-only passing semantic tests.

The first source-based process build failed before selecting/executing tests:
`eez-node`'s `rustc` received SIGKILL with three Cargo jobs under host memory
pressure. Its log is retained as `artifacts/validation/cross-chain.log`; this is
build-resource failure, not process coverage. A one-job retry is tracked below.
An independent copy of the baseline's dependency build cache reduces rebuilding;
all source-test executable overrides are cleared and Cargo compiles the candidate
workspace in its own target directory.

The core command explicitly excludes `script/*` and
`test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental` per AGENTS.md.
Additionally skipped: `testForkL1UpgradeAndRepeat`, `testForkL2UpgradeAndRepeat`,
`testForkRejectsWrongSigningKey` (network-dependent fork tests). These are exclusions,
not coverage. Rust's four ignored tests are the manual held-pool timing profile,
two fixture regenerators and the native fixture inventory utility. Frozen vector
expectations were not regenerated to obtain a pass.

An independent read-only review of the contract, core callers, initializer and
artifact consumers, estimator, compatibility tests and operator guidance found
no blocking correctness issue. It is source review, not evidence of a live
upgrade or CI completion.

## Remaining validation at this record's commit

The one-job source process retry, final fingerprint regression, pinned protocol
Anvil suite/core build and signer contributor checks are recorded here as their
results complete. Full release-artifact, shard and Kurtosis validation must pass
on the proposed PR head; the green dependency alone does not validate this change.
No live upgrade rehearsal or existing deployed network operation was performed.
