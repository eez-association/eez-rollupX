# Checkpoint-aware startup with pruned deployment receipts

| Field | Value |
| --- | --- |
| Date/status | 2026-10-11 UTC; implementation and local validation in progress |
| Scope | Composer/follower startup preflight; no protocol, scheduling, storage or pruning-policy change |
| Baseline | `eduadiez/rollupX` main `264301894d12b671c0f52107fd2b6bc1a02bfb7f` |
| Candidate | This change's shared-startup patch and regressions; exact revision is the containing PR head |
| Protocol | Unchanged `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Environment | Rust 1.95.0, LLVM 22, Foundry 1.7.1; local loopback fixtures, no public-chain writes |

## Finding and scope

The released preflight calls `L1Reader::serves_history(deploy_block)` before the
deriver can choose its validated persistent correlation checkpoint. A synced
source with deployment receipts pruned therefore blocks a restart even when the
actual checkpoint's recovery range remains available.

The fix leaves chain/height/finality checks in preflight and delegates history
availability to the existing deriver scan. `correlation_seed` validates canonical
L1 and local L2 identities; `sync_batches_inner` scans the checkpoint's containing
L1 block inclusively. Missing or invalid checkpoints retain the existing fallback
toward deployment. Composer/follower do not start L1-active tasks before successful
reconciliation. No trusted-height override, new checkpoint format, or pruning
configuration is introduced.

This does not restore pruned receipts, remove older-header requirements, or replace
native blob availability. It cannot guarantee recovery after arbitrary downtime
or index loss. See the [operator procedure](../../runbooks/ethereum-node.md#recovery-with-a-pruned-l1).

## Reproduction and evidence

The new `eez-node-common` regression runs a loopback JSON-RPC server that serves
chain ID 1, head 100, finalized block 80 and headers, but returns Reth's receipt
pruning error (`4444`, `pruned history unavailable`) for log queries. Genesis
reference 40 is final; deployment block 5 is old.

Run from the repository root with Rust 1.95.0 and the required native dependencies:

```bash
cargo +1.95.0 test --locked -p eez-node-common --features l1-startup --lib \
  readiness_defers_pruned_history_to_checkpoint_aware_derivation -- --nocapture
```

- Baseline plus regression: **failed**, 1 selected test. After one second:
  `deployment receipts must not block checkpoint selection: Elapsed(())`.
- After the production change: shared-startup package **27 passed**, including
  the regression, wrong-chain rejection, and an unfinalized-reference case.
- Fixture relay tests: **16 passed**, including the inclusive pruning boundary
  and retention of header access.
- `forge build --root contracts`: passed with existing compiler/lint warnings.

The first test invocation could not bind a loopback socket in the execution
sandbox; the baseline above was repeated with socket access. The first process
build also found stale workspace artifacts (an enum variant present in source
was missing from compiled metadata). Workspace package artifacts were cleaned
before rebuilding; dependencies and the lockfile were not changed. These were
setup/build failures, not successful executions of the process regression.

The process regression is
`settlement_policy::restart_with_pruned_deployment_logs_settles_and_replays_from_checkpoint`.
It uses the existing isolated native network and a reversible fixture relay
control, not deletion of a real Reth database. It covers full-replay rejection
without deployment history, checkpoint-based restart, a subsequent ordinary
transfer's settlement and independent follower replay, rejection when the
checkpoint block's logs are missing, and resumption when they return.

Process results and remaining checks are recorded below when completed. Local
logs/datadirs under `artifacts/native-e2e/` are local-only diagnostics, not published
evidence. The committed test inputs and failure summary above are reproducible.

## Operations and remaining validation

No mainnet services, operator settings, databases, contracts, or wallets were
changed. Only generated local Cargo workspace artifacts were removed for the
rebuild; they are reproducible from source.

Full release-artifact/Kurtosis CI and a mainnet restart on the released candidate
remain separate acceptance checks. Local simulated pruning does not validate the
retention of any deployed mainnet database. Preserve its current working RPC and
datadirs until local recovery-range availability and a subsequent canonical
settlement have been verified.
