# Checkpoint-aware startup with pruned deployment receipts

| Field | Value |
| --- | --- |
| Date/status | 2026-10-11 UTC; revised local validation passed; release CI and mainnet validation pending |
| Scope | Composer/follower startup preflight; no protocol, scheduling, storage or pruning-policy change |
| Baseline | `eduadiez/rollupX` main `264301894d12b671c0f52107fd2b6bc1a02bfb7f` |
| Candidate | PR #83; original validation at `36ea595487c50bfbf19ac1a4143a31a11bb98bb0`; revised source and validation described below |
| Protocol | Unchanged `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Environment | Rust 1.95.0, LLVM 22, Foundry 1.7.1; local loopback fixtures, no public-chain writes |

## Finding and scope

The released preflight calls `L1Reader::serves_history(deploy_block)` before the
deriver can choose its validated persistent correlation checkpoint. A synced
source with deployment receipts pruned therefore blocks a restart even when the
actual checkpoint's recovery range remains available.

The [startup contract](../../configurable-settlement-policy.md) delegates history
validation to the existing inclusive deriver scan. See the
[operator procedure](../../runbooks/ethereum-node.md#recovery-with-a-pruned-l1)
for retained-history requirements, fresh-node fallback and rollback limits.

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

## Claude review and simplification (2026-10-11)

Claude Opus 5.5 reviewed PR head `2ea463c8e9694b62a9ff1957a2be05b150f50e39`
against the recorded base, using only the diff and scoped tracked source. It
reported no correctness blocker, requested the policy-bootstrap caller, and
recommended removing dead code and duplicate assertions while preserving the
three process scenarios. `canonical_policy_anchor` uses `registration_anchor`
only when no commitment exists and the endpoint is genesis; interval mode does
not rescan deployment after a validated settlement. No policy change is needed.

The revision removes the unused history probe and sync-status query/wrapper,
uses the existing signal-count helper, directly checks the recovery scan starts
at the checkpoint, and trims duplicated prose. Block-range fault parsing now
runs only when pruning is enabled. Wrong-chain/finality checks and all three
recovery scenarios remain. Executable hashes and first-failure evidence above
are retained as historical evidence, not proof that the revision passes.

Claude's second review found no runtime correctness blockers in the supplied
revision. Its record-status correction, wrong-chain timeout, stale scan-range
comment correction, and further prose trimming were applied. Source inspection
closed the remaining caller questions: `validate_settlement_policy_startup`
uses the same canonical anchor and retained L2 blocks; `update_activity_index`
reads L2 only. The inclusive scan publishes commitment evidence used by
`policy_anchor_snapshot`; fixture restarts create fresh log files.

The receipt-pruning RPC error is currently a `Provider` error, so catch-up uses
the existing bounded 32-failure retry path, not unlimited retries. The runbook
now states this. Registration and recovery log calls use block ranges; the
fault simulation deliberately does not model block-hash queries or physical
database pruning. Broader provider/error-classification changes were not added.
Historical evidence and all safety scenarios were retained despite suggestions
to trim the record further.

Revised validation (same commands above):

- Full Python tooling suite: **351 passed, 5 opt-in infrastructure checks
  skipped** (356 discovered, 304.803 seconds); exclusions unchanged.
- Workspace Clippy with `-D warnings`, formatting, diff whitespace and the
  edited runbooks' shell-example syntax: **passed**.
- Final affected unit suites: **252 passed** (shared startup 28, deriver 44,
  L1 180; no ignored or filtered tests).
- Revised native regression: **1 passed**, 136 unrelated tests filtered, none
  ignored; 143.11 seconds including fixture setup/build waiting. This is not a
  performance comparison. Both exact recovery-block assertions passed, as did
  fresh-node rejection, unchanged safe hash, a new settled transfer with
  independent follower convergence, and missing-checkpoint-log rejection then
  recovery in the same process. Local-only logs: test process `2773599` under
  `artifacts/native-e2e/logs/`. No retry was needed for the revised test.

Revised local debug executable SHA-256 values (source in this revision; not
promoted release artifacts):

| Executable | SHA-256 |
| --- | --- |
| Composer | `ffec1f8e5af317c693c863e2e0be4a6b064de7ec41c8cc69fd1a2eabe54035ca` |
| Follower | `07f7a0f8b0b3d01750e5a422748ce9d295fd9d2f913eaa0c821f4be693fbcd14` |
| Proof signer | `2bd862f0ddd54c9387169552aa32bf66cae4935591d2618b671beb335ab24d13` |
| Process test | `1fd393f552d1bd19fa33ee387770dbe25551b74774fb1663a8d413d11cce98dc` |

Full release-artifact/Kurtosis CI and a mainnet restart on the released candidate
remain separate acceptance checks. Local simulated pruning does not validate the
retention of any deployed mainnet database. Preserve its current working RPC and
datadirs until local recovery-range availability and a subsequent canonical
settlement have been verified.
