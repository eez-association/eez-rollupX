# Checkpoint-aware startup with pruned deployment receipts

| Field | Value |
| --- | --- |
| Date/status | 2026-10-11 UTC; local validation passed; release CI and mainnet validation pending |
| Scope | Composer/follower startup preflight; no protocol, scheduling, storage or pruning-policy change |
| Baseline | `eduadiez/rollupX` main `264301894d12b671c0f52107fd2b6bc1a02bfb7f` |
| Candidate | Runtime/test sources at `36ea595487c50bfbf19ac1a4143a31a11bb98bb0`; subsequent record-only update in PR #83 |
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
- After rebuilding workspace artifacts: **251 Rust unit tests passed** (shared
  startup 27, deriver 44, L1 180; no ignored or filtered tests).
- Fixture relay tests: **16 passed**, including the inclusive pruning boundary
  and retention of header access.
- Full Python tooling suite: **351 passed, 5 skipped** (356 discovered). The
  skipped opt-in checks require real Docker payload/runtime images, native
  archive relocation, or a Kurtosis resource probe; none is counted as coverage.
- `forge build --root contracts`: passed with existing compiler/lint warnings.
- Final workspace Clippy (`-D warnings`), formatting, diff whitespace and edited
  shell-example syntax checks: passed.

The first test invocation could not bind a loopback socket in the execution
sandbox; the baseline above was repeated with socket access. The first process
build also found stale workspace artifacts (an enum variant present in source
was missing from compiled metadata). Workspace package artifacts were cleaned
before rebuilding; dependencies and the lockfile were not changed. These were
setup/build failures, not successful executions of the process regression.

The first executed process regression reached checkpoint-based restart but then
failed with connection refused: its test client still used the pre-restart L2
RPC port. The fixture allocates a new port on reopen. The test now rebuilds that
client and waits for Composer readiness. The failure's logs/datadirs were retained
locally under `artifacts/native-e2e/` (test process `2659057`); it is not reported
as a passing process run or silently retried unchanged.

The process regression is
`settlement_policy::restart_with_pruned_deployment_logs_settles_and_replays_from_checkpoint`.
It uses the existing isolated native network and a reversible fixture relay
control, not deletion of a real Reth database. It covers full-replay rejection
without deployment history, checkpoint-based restart, a subsequent ordinary
transfer's settlement and independent follower replay, rejection when the
checkpoint block's logs are missing, and resumption when they return.

The corrected process regression **passed**: 1 executed, 136 filtered out, no
ignored tests; 173.17 seconds including fixture setup/build waiting, not a
performance benchmark. Test process `2674922` retained these observations:

- Fresh follower scanned from L1 block 0 and rejected pruned logs before spawning
  the deriver.
- Restart seeded the saved checkpoint at L1 block 7 / L2 block 145, scanned
  inclusively, and started Composer. The saved safe block's hash was unchanged.
- A new ordinary transfer settled and replayed independently with a successful
  receipt and identical block hash on the follower.
- With logs through checkpoint L1 block 11 pruned, the next restart seeded the
  checkpoint but stopped at history scanning. Restoring logs from block 11 let
  the same process finish recovery; Composer started only afterwards.

Commands (run from the candidate worktree, native toolchain configured):

```bash
cargo +1.95.0 test --locked -p eez-node-common -p eez-deriver -p eez-l1 \
  --features eez-node-common/l1-startup --lib
python3 -m unittest discover -s scripts/ci/tests -v
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
EEZ_TEST_LOG_DIR="$PWD/artifacts/native-e2e/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-e2e/datadirs" \
  cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  settlement_policy::restart_with_pruned_deployment_logs_settles_and_replays_from_checkpoint \
  --exact --test-threads=1 --nocapture
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 fmt --all --check
git diff --check
```

Local debug-profile executable SHA-256 values:

| Executable | SHA-256 |
| --- | --- |
| Composer | `a87085cfc21e5511a4158c620b0dbb6db3fbe4a879862bbdb29f20060ef2cb78` |
| Follower | `f6e6be607f806268d1de3079d99bd85281a09c771f65f2b6fb367645c2229252` |
| Proof signer | `66b08bc009a54bad936f5344e45d0b9b822d019f27a8bfdb6cd9ab5ca6216901` |
| Process test | `2c141c362b27b00ce908cf94e887859bacc004374dc0c73069f51fb478a7623f` |

These are source-built test artifacts, not promoted shipping binaries. Local
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
