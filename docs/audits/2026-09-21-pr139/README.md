# PR #139 adaptation: funding waits and test diagnostics

This records the initial adaptation at `459a6ba4`. Subsequent pre-PR fixes and
their separate validation are recorded in [the pre-PR review](pre-pr-review.md).
The subsequent release-profile local CI run is recorded separately in
[full local CI validation](full-local-ci.md), including failures and build-cost
findings; it must not be interpreted as an all-green CI result.
The later [main synchronization and lint repair](main-sync-lint.md) records
the merge of `37f6f5a5`, the targeted dependency security update, and its checks.
The limitations and executable hashes below describe the initial revision only.

## Baseline and scope

- Fork base: `4f4e73e6e6542eb9ba17097693c240a8cd2ea41d` (live RollupX main
  at branch creation).
- Upstream change: [#139](https://github.com/eez-association/eez-rollup0/pull/139),
  `abf0ad4fc1852a0afd037de5af7f1973b303f73f`, against parent `c97b83f142b947243c2d7ef6a76b627d4bfcb71b`.
- Protocol remains `3c0cdf4955941a5c23639346932b55b33babebbd`.
- Branch: `backport/pr-139-funding-deferral`.

The fork already defers missing physical backing and per-rollup escrow using
typed source conditions, preserves sender/direction nonce dependencies, and
selects independent work beyond a blocked prefix. Funding waits do not consume
settlement attempts. These are existing requirements in the
[queue contract](../../proposals/cross-chain-queue-liveness.md).

## Upstream disposition

| #139 component | Fork disposition |
| --- | --- |
| Typed insufficient-target-funds error | Existing `SourceBlocked(SourceCondition::Escrow)` checks physical manager backing and pinned per-rollup escrow. Retained. |
| Defer withdrawal and its nonce suffix | Already implemented with eligibility-aware selection; consolidate the three simulation/escrow deferral paths in `defer_source`. |
| Charge funding waits and evict at three attempts | Not adopted: conflicts with the fork's queue contract. Regression covers repeated waits with an already nearly exhausted settlement counter. |
| Preserve deferred requests through transient abort | Retain guarded `requeue_unprocessed`/`requeue_composing`; regression exercises a later abort with independent candidates. |
| Fail receipt waits on terminal failures | Adapted to process-failure checks and current retained transaction dispositions. Do not infer terminal state from historical human-readable eviction messages. |
| Deposit-funded wave assertions | Retain fork semantic/follower checks; require four distinct pinned L1 escrow evaluations before funding in native escrow scenarios. Add a full 12-withdrawal blocked prefix at numeric limit 12 alongside existing limit-one and automatic profiles; independent work must progress before funding, then every original withdrawal must settle. |

The existing `wait_for` deliberately retries poll errors. Merely returning an
error from its closure, as #139 does, does not terminate our wait. The new
`wait_for_with_failure` checks terminal process failure outside that retry path;
receipt terminal dispositions exit as completed outcomes. Existing ordinary
poll errors remain retryable. The existing batch-or-node-failure wait now uses
the same mechanism.

Receipt polling checks the source receipt first and does not reject a canonical
success whose receipt is not visible yet. Missing queue status is not evidence
of eviction. Pre-submission removals without retained status may still time out.
Receipts alone do not establish cross-chain completion; existing semantic and
follower assertions remain required.

No wire/configuration/storage changes, queue expiry, admission limits, contract
updates, deployment, or upstream issue publication are part of this change.

## Validation

Toolchain: Rust 1.94.0 and Foundry/Anvil 1.7.1. Commands run from the new worktree.

- `cargo +1.94.0 fmt --all --check` and `git diff --check`: passed.
- `forge build --root contracts`: passed with existing Solidity/lint warnings.
- `cargo +1.94.0 test --offline --locked -p eez-testkit --lib framework_tests
  --target-dir /home/edu/Development/eez-rollup0-fork/target/backport-rust-1.94 -j 3`:
  **7 passed**, 0 failed, 5 filtered out. Includes terminal failure gating with synthetic callbacks,
  terminal queue outcomes, retryable transport errors, and reverted receipts.
- `cargo +1.94.0 clippy --offline --locked --workspace --all-targets
  --target-dir /home/edu/Development/eez-rollup0-fork/target/pr139-check -j 2
  -- -D warnings`: passed, including all native test targets.

- `cargo +1.94.0 test --offline --locked -p eez-composer --lib
  --target-dir /home/edu/Development/eez-rollup0-fork/target/backport-rust-1.94 -j 2`:
  corrected run **204 passed**, 0 failed, 0 ignored.
- Runtime build: `cargo +1.94.0 build --offline --locked -p eez-node -p eez-follower
  -p eez-proof-signer --bin eez-composer --bin eez-follower --bin eez-proof-signer
  --target-dir /home/edu/Development/eez-rollup0-fork/target/backport-rust-1.94
  --config profile.dev.package.eez-node.opt-level=0
  --config profile.dev.package.eez-follower.opt-level=0 -j 1`: passed.
  Runtime binaries were copied to `artifacts/pr139/bin` before building the
  native tests, and are local-only artifacts. Lower role optimization bounds
  local build memory; this is not release-profile performance evidence.

| Frozen runtime | SHA-256 |
| --- | --- |
| Composer | `8f18be148969b674376aa0475f7f187fdc140326fca32111545676971819ec86` |
| Follower | `7676f64c26c7393fc6fca0aa82697442e9b390731f30bb3db257a61a0d06cfed` |
| Proof signer | `c0ac682a08a43962b477c94ec0861c5187e3a41a01bc2fafd3f28ace58d41c62` |

Native tests were built with `cargo +1.94.0 test --offline --locked -p eez-node-e2e
--test queue_liveness --test cross_chain --no-run`, the same target directory and
role overrides above, plus `--config profile.dev.package.eez-node-e2e.opt-level=0
-j 1`. The resulting test executables ran directly with `--test-threads=1
--nocapture`, `EEZ_TEST_PREBUILT=1`, `EEZ_TEST_RUNTIME_BIN_DIR` set to the frozen
directory, and `EEZ_TEST_PROOF_SIGNER_BIN` set to its signer. Logs and failure
datadirs use `artifacts/pr139/logs` and `artifacts/pr139/datadirs` (local only).

| Native check | Result |
| --- | --- |
| `queue_liveness`: Ethereum limit 12 with 12 blocked withdrawals, Ethereum limit 1, Gnosis automatic admission | **3 passed**, 0 failed, 0 ignored, no retries; 227.09 seconds. Each requires four distinct pinned L1 funding evaluations, independent progress, later funding, exact balance/nonce outcomes, zero charged attempts, and composer/follower convergence. |
| `cross_chain`: `both_directions_return_value_and_wrapper_success_repeated_waves --exact` | **1 passed**, 0 failed, 21 filtered out, no retries; 65.80 seconds. Exercises the adapted receipt helper across both directions and repeated deposit/withdrawal waves. |

Test executable SHA-256:

- `queue_liveness-60a3a82223dfc94f`: `dc934f6e1d2d0a72f29497da716d3ed72561562ca83fd4bac35e8b538561d2f5`.
- `cross_chain-425a8c5f53b10d35`: `04b65da99ce628cf1c5f261af5e85d001901987106205a3010c7a1c8c9f9e300`.

All three queue runs emitted three structured dispatch events each, so their
zero-attempt assertions were exercised. Runtime hashes were rechecked after
building the test executables and remained identical. RollupX main was also
rechecked before handoff and still matched the recorded fork base.

The terminal-wait problem was established by inspecting the old poll-error
handling; the new failure-gating tests use synthetic callbacks and retained-status
values. A process-death comparison against an unmodified baseline binary was not
run. The native tests exercise successful recovery and receipt paths with actual
processes, proofs, blobs and follower replay. No production deployment, upstream
reproduction, full native suite, release-profile CI or Kurtosis run was performed.
Historical validation does not establish those gates for this branch.

The first composer run completed 204 tests: 203 passed, and the new funding
regression failed because it assumed an independent candidate kept its original
snapshot index after recovery. Recovery can move it ahead of untouched suffixes
while preserving nonce order. The corrected test resolves the candidate and
successor by identity on each snapshot. No runtime correction was made for
this assertion failure; the original failure is retained here.
