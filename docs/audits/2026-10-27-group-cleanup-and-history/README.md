# Complete-group cleanup and optional history updates

| Field | Value |
| --- | --- |
| Status | Implementation in progress; combined process, offline conformance and final-head checks pending |
| Scope | PR #103, accepted issues #105 (complete-group cleanup) and #108 (dedicated optional history update); new networks only |
| Base | `6c3106fb482bb323eefa8898a0bd922fc7520480` on `codex/fee-vault-upgradeable`; PR base `codex/mixed-direction-fees` at `641e768f2e6605f7563c808a00937d374e98e8fb` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| Environment | Rust 1.95.0, locked dependencies; LLVM 22; Foundry 1.7.1, Solidity 0.8.34, via IR, optimizer runs 1 |
| Isolation | `/home/edu/Development/rollupX/artifacts/fee-vault-upgradeable-20261026`; operator checkout, deployments and wallets untouched |

The [native execution proposal](../../proposals/native-system-transactions.md)
owns the accepted complete-group rule. The existing funding investigation at
`bf8a7011fd6658ef6a9c811f22a8f8b197b24a31` established independent canonical
table funding. Its eight real-core-contract cases are included as
`contracts/test/ProtocolGroupFunding.t.sol`; their `vm.deal` cleanup models do
not establish production hook placement.

The implementation shares a derived group cursor and balance-delta helper
between full block execution (including Stateless) and Composer incremental
execution. Snapshots restore the cursor with state and gas. Included revert/halt
results retain their normal receipt and gas while clearing protocol residue;
validation errors and discarded previews commit neither the cleanup nor the
cursor. No state change runs after checkpoint capture.

The fixed validation-contract version is 2, used in both capabilities and the
canonical on-chain profile preimage. Cross-chain envelope version 1 and prove.v2
remain unchanged. Frozen execution recordings must be recaptured for the final
combined execution rules; old recordings are not evidence for this change.

## Iteration evidence

Logs are local-only under `artifacts/group-cleanup-20261027/` in the isolated
worktree. They are retained diagnostics, not portable CI artifacts.

| Check | Observed result |
| --- | --- |
| Initial `cargo check -p eez-evm -p eez-composer` | Missing `DatabaseCommit` trait import; corrected. |
| First EVM test compile | Two fixture type/conversion errors; corrected. |
| EVM unit execution | 40 passed, 1 old-policy balance assertion failed; all four new production executor group tests passed. Updated the intentional old post-inbound balance expectation from 13 to 0. |
| Composer initial unit execution | 411 passed, 2 failed, 1 ignored. Two native gas-budget fixtures represented dangling loads; changed those independent operations to checked inbound deliveries. |
| Composer new regression compile | Incorrect ABI struct and missing explicit fields; corrected. |
| Composer subsequent unit execution | 413 passed, 1 failed, 1 ignored. Manager code changed with the canonical profile; its exact bytecode recognition fingerprint required regeneration. |
| Integration contract suite | 164 passed, 1 stale generated runtime-fixture failure. Eight independent group-funding tests passed, including 256 fuzz cases. |
| Runtime-fixture focused rerun | Passed after regeneration from the completed Solidity build. |

These are development failures with code/fixture changes between runs, not
retry-only flakes. Final combined results, source/executable identities and
remaining limitations will be recorded before the PR update. No network has
been deployed or restarted by this work.
