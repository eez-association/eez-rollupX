# Recovery and scheduler regression repair

Follow-up to [the bug review](bug-review.md). Implementation starts from PR #79
head `a4536050b6cecf80b247344a08348277ba098f92`, protocol
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. No protocol or dependency version
changes; two already-pinned Reth crates become direct dependencies for the
maintenance adapter's trait signatures.

## Scope

- Only Reth maintenance receives `RecoveryPool`. Its orphan reinsertion batch
  uses local origin and the original validator. RPC/network callers retain the
  original pool, external limits, and replacement rules. No second queue,
  polling task, worker, or retry journal is added. Explicit no-local exemptions
  remain authoritative. This applies to Composer and follower pools.
- Empty local checkpoints carry the configured fee recipient, matching the
  ordinary/minimal-production paths. The historical replacement assertion is
  retained, not weakened. Recorded historical environments remain authoritative.
- Idle test barriers observe `slot.plan` / `prepare=false`; their waves arrive
  after a finite cutoff and before the next one. They do not wait for skipped
  final preparation or infer canonical settlement from a planning hint.
- The existing stale-seed process test now displaces 20 ordinary transactions
  from one sender. It requires all receipts to move without client resubmission,
  exact recipient balance, settlement and independent follower replay.

## Baseline reproduction

Rust 1.95.0, Foundry 1.7.1; `forge build --root contracts` succeeded with existing
lint/remapping warnings. Process-test prebuilt overrides were unset. A sandboxed
attempt failed at local port reservation before testing behavior; it is not
the bug reproduction. The subsequent isolated localhost run used:

```bash
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  conditional_settlement_requeues_ordinary_stale_seed_descendants --test-threads=1
```

With only the 20-transaction regression compiled into the baseline executable,
the test failed after settlement reconciliation: transaction
`0xe7c4947a9533ca2a2e49e853dd770ee4677e307451c4398ae04d375733dbad83`
did not return within 180 seconds. Node and signer remained running. Total
test time was 290.24 seconds, including the fixture's signer build/startup.
Baseline executable SHA-256:
`a3505a37815bf588d19236489d9a8733bb974ddb22dbf0120a1f55dd2d24c7c0`.

Retained **local-only** evidence directory:
`/tmp/rollupx-pr79-ci-review-y7axfhr9/`, with `baseline-logs/` and
`baseline-datadirs/eez-cross-chain-2011329-ik1Qee`. These artifacts are unavailable
from another checkout. No live services, wallets, or stalled transactions were
modified by this repair procedure.

## Candidate validation

- Workspace `clippy --locked --workspace --all-targets -- -D warnings` passed.
- Formatting and `git diff --check` passed.
- `cargo deny check advisories bans sources` passed with warnings;
  `cargo audit` passed with nine allowed warnings. No dependency versions changed.
- `cargo +1.95.0 test --locked -p eez-node-common -p eez-composer --lib`:
  **330 passed**, one intentionally ignored manual component timing profile
  (`held_pool::tests::profile_blocked_backlog_discovery`). All four new recovery
  tests passed, including the 20-predecessor / 44-successor case, unchanged
  external caps, explicit no-local policy, and specific ownership rejections.

The first unit run rejected the new fixture's 1-wei transactions before reaching
recovery. Raising only that test fixture's fee to 1 gwei made it exercise the
intended capacity boundary; the failing run is retained in `fix-units.log`, the
corrected run in `fix-units-valid-fees.log`. This was a fixture correction, not
a retry-only pass. An interrupted narrow Cargo check is not passing evidence.

The source-built candidate process executable has SHA-256
`63061b96bd9117ebcfa4c7e7890320fc1b767f8b05ba55a1f8ce5dcb3526c2e3`.
It was built with `cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e
--no-run` and invoked directly with eight named filters and `--test-threads=1`.
Subsequent source edits before this run changed only imports and unit fixtures,
not its runtime or process-test behavior. All prebuilt overrides were unset;
the fixture builds the signer from source. The first direct invocation omitted
the inherited `RUSTUP_TOOLCHAIN`; it was stopped as a setup error, with its local
process group terminated and logs retained in `fix-process.log`. The corrected
invocation explicitly sets `RUSTUP_TOOLCHAIN=1.95.0` and records results in
`fix-process-rust195.log`, with `fix-rust195-logs/` and retained failure datadirs.
The failed setup let rustup auto-install dependency-requested toolchains 1.56.0
and 1.83.0; neither is used for the corrected validation. No global toolchain
cleanup or Cargo clean was performed.

Corrected process run: **8 passed / 0 failed**, 128 filtered out, 476.41 seconds:

- `dynamic_admission::automatic_admission_reorg_keeps_outbound_sources_out_of_ordinary_sequencing`
- `dynamic_admission::automatic_selection_survives_a_canonically_skipped_slot_without_shrinking`
- `dynamic_admission::ethereum_cold_and_warm_fitting_waves_have_no_calibration_post`
- `dynamic_admission::historical_prefix_replacement_keeps_the_empty_boundary_with_a_configured_coinbase`
- `partial_consumption::first_source_omission_keeps_the_registration_anchor`
- `settlement_policy::conditional_settlement_requeues_ordinary_stale_seed_descendants`
- `settlement_policy::interval_preserves_execution_but_waits_below_all_thresholds`
- `settlement_policy_gas::ethereum_older_inbound_precedes_capacity_filling_outbound`

The same 20-transaction regression that failed on the baseline now passed,
including receipt movement without resubmission, exact balance, canonical
settlement, and independent follower replay. The nonzero-coinbase assertion was
retained and passed. This is focused local process coverage, not the full native
matrix. The separate live workload has not been repeated.

Final workspace Clippy rerun passed after the unit-fixture correction; 43 local
Markdown links resolved and Bash examples parsed without executing procedures.
The source-built signer SHA-256 was
`a71733f6f3acd747957f3feb91db5771e961a028667b9d1153e44be0ba85bd0c`.
CI still owns release/shipping builds, the full native matrix, and Kurtosis.
No new CI result, live upgrade, or successful repeat of the 15-minute workload
is implied by these local checks. The live nonce gap is not recovered by merely
updating source code.

Retained local log SHA-256 values:

| File | SHA-256 |
| --- | --- |
| `fix-process-rust195.log` | `80203394a87ca5353461cf83769ef4e7e8322c306036a18a199f1fe257fd8fdb` |
| `fix-units-valid-fees.log` | `f238c339a1e9404be2c1e19477caf0c402c898c0a9486a89f844b25b786e62e5` |
| `fix-clippy-final.log` | `75bc9569417c36c37bcf06d2545bc217a59b5401abca0c4c42a8659f2f9d36d8` |
