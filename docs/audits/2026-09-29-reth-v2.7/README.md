# PR #44: Reth v2.7.0 and local CI validation

Status: local validation in progress; nothing pushed from this follow-up.
The [upgrade contract](../../proposals/reth-v2.7-upgrade.md) defines scope and
acceptance. No public network or operator service was changed.

## Source and environment

| Input | Identity |
| --- | --- |
| PR #44 baseline | `3f95f1848576cb19f9bf6e7ae4151c2e5eb3757c` |
| Required PR #42 head | `cf8500e80cc10b58ab6acd59b2125dd098aa770c` |
| Merged main base | `de5030c782e29ff07150b2728c7b564fd607f802` |
| Fixture-path fix | `b56fb13` |
| Reth upgrade / compiled candidate | `239d32c9c7a57c4c01ed79d7feefb438b246b2dd` |
| Reth v2.7.0 | `3d592ece6de8c4559987416a544fc215fd6d6921` |
| Protocol gitlink | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Tools | Rust 1.95.0; Foundry 1.7.1; Nextest 0.9.143 |
| Branch | `backport/native-system-txs-issue-35` |
| Dedicated worktree | `/home/edu/Development/rollupX/artifacts/native-system-backport/rollupX` |

The original operator checkout's uncommitted work was preserved. The candidate's
merge base with PR #42 is its exact head above. Main remained unchanged during
the initial review and subsequent local checks.

Reth v2.7.0 is 32 commits beyond PR #41's `444e867` pin and includes its required
forkchoice fix. No runtime Rust API adaptations were needed. All 102 Reth Git
packages in the shipping lockfile and all 19 in the standalone Stateless lockfile
use the same release commit. Required registry changes are `revm-inspectors`
0.43.0 → 0.44.0 and `alloy-eip2930` 0.2.3 → 0.2.4. REVM, core Alloy, Rust,
protocol source, and genesis fixture bytes are unchanged. Vendor patch reverse
checks pass. Adapter source links to `444e867` retain their original provenance;
the linked upstream source files are byte-identical at v2.7.0.

## CI failure and regression

[Run 36546438201](https://github.com/eduadiez/rollupX/actions/runs/36546438201)
built the release artifacts but failed 13 signer tests. Two fixture helpers
used compile-time `CARGO_MANIFEST_DIR`, pointing to Docker's `/build` after
Nextest moved execution to the host checkout. Other fixture helpers already
honored Nextest's runtime environment.

One shared test-only manifest-directory helper now handles all four runtime
fixture readers. Production signer validation is unchanged. Before the fix,
two exact tests representing both broken helpers failed with `NotFound` when
the original fixture directory was temporarily moved out of the way. The same
two checks passed afterward, reading identical fixtures from the relocated
directory. The original directory was restored after each check.

The actual Docker producer and exported release archives also ran locally:
**990 unit/vector tests plus 292 Composer tests passed**, including all 13
previous failures. Three manual utilities remain ignored. This uses the real
Docker build context and Nextest remapping, not a synthetic packaging fixture.

GitHub additionally rejected failure-diagnostic uploads because the repository's
artifact quota was exhausted. That optional upload failure did not cause the
unit-test failure. No historical artifacts were deleted.

## Fresh-devnet startup gap

The first local Kurtosis run at `239d32c` failed before its first deployment
broadcast: `scripts/deploy.sh` queried `finalized` before the new L1 exposed a
finalized block. No EEZ node or signer had started. The native genesis profile
requires that finalized reference; substituting another block is invalid.

The Kurtosis plan now waits up to 1,800 seconds for an error-free finalized-header
response before entering deployment. The deployment script retains its complete
header/genesis validation. The two new orchestration regressions fail against
the original plan and pass with the readiness gate. They cover delayed finality,
null/invalid response types, RPC errors, and absence of deployment on timeout.
The [Kurtosis wait primitive](https://docs.kurtosis.com/api-reference/starlark-reference/plan/#wait)
owns the bounded polling; no custom retry process or genesis fallback was added.

A first 900-second readiness run exhausted its bound while the mainnet-preset
L1 was still at finalized epoch zero. Its logs are retained. The corrected
30-minute bound remains fail-closed. Live validation of that plan is pending. Its exact source hash is
retained beside the run, separately from the frozen runtime candidate identity.

## Completed local checks

| Command / path | Result |
| --- | --- |
| `cargo +1.95.0 fmt --all --check`; `git diff --check` | Passed |
| `cargo +1.95.0 clippy --locked --workspace --all-targets --all-features -- -D warnings` | Passed |
| CI's default-feature workspace Clippy command | Passed |
| `cargo +1.95.0 test --locked --workspace --exclude eez-node-e2e` | 1,282 passed; 3 manual utilities ignored |
| `cargo +1.95.0 test --locked --manifest-path vendor/stateless/Cargo.toml --workspace` | 26 passed |
| Signer Rustdoc with `RUSTDOCFLAGS='-D warnings'` | Passed |
| `cargo deny check advisories bans sources`; `cargo audit` | Passed under existing policy; 9 allowed audit warnings |
| Initial CI tooling suite; readiness extension | 274 then 276 passed; 5 opt-in skips |
| `python3 scripts/check-settlement-policy-config.py` | Chiado and Kurtosis passed |
| `forge test --root contracts` | 84 passed |
| Pinned core tests | 517 passed, with the required exclusions below |
| Genesis fixture check with Rust 1.95.0; verified release-helper check | Passed; fixture state root unchanged |
| Real `scripts/ci/build-runtime-artifacts.sh` producer, publication disabled | Shipping binaries, both release test archives and node/signer/deploy images built |
| `scripts/ci/prepare-runtime-artifacts.sh` | Exact source, protocol, lockfile, image and binary checks passed |
| Exported release unit lane | 1,282 passed; 3 ignored |

Core command:

```bash
forge test --root eez-core-protocol --no-match-path 'script/*' \
  --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'
```

The excluded cases are not coverage. The initial native process run stopped
after reproducing two failures: 21 passed, two failed, one interrupted and 12
not run. Cross-chain had not started. Its owned processes were stopped before
rebuilding. Full corrected process suites and Kurtosis remain pending. The final
readiness tooling suite passed 276 tests with five opt-in skips; corrected
Composer/L1 unit tests passed 447 with one manual utility ignored. All-features
workspace Clippy passed after the scheduling change.

## Retained failures and evidence limits

`BASE_PORT=23545 bash scripts/ci/run-protocol-anvil.sh` passed 35/36 scenarios.
`deepNested` timed out after 300 seconds during L1 infrastructure deployment.
Its next transaction remained queued with nonce 3 while the account nonce was
also 3. The owned Anvil state, pool and broadcast records were retained before
cleanup. An isolated retry with identical source, chain IDs and ports passed,
including entry/call verification. This is a **flake**, not a clean suite pass;
neither the protocol nor Anvil source changed in this upgrade.

The first diagnostic retry had an incorrect scenario path and stopped before
execution; the verified path was then used. An initial E2E export similarly
stopped before tests because its shell lacked `CARGO_TARGET_DIR`; the subsequent
export used the actual verified unit extraction. The genesis script's first
Cargo fallback selected host Rust 1.96.1; that owned build was stopped and the
check rerun successfully with `RUSTUP_TOOLCHAIN=1.95.0`. None of these setup
attempts counts as validation.

Bulk logs, datadirs, manifests, source/executable hashes and before/after checks
are **local-only**, under `artifacts/reth-v2.7-review/` in the dedicated worktree.
They are not present in another checkout. The canonical local L1 uses official
Reth v2.7.0 image digest
`sha256:a7f059d470878a5a59f3c575f3fc81b536d0d2d56d7a1cbed73e717a4f766336`.

## Compatibility

PR #44's fresh genesis/datadir requirement remains in force. This follow-up
introduces no additional genesis or protocol-format change and no migration or
legacy decoder. Reth v2.7.0 enables persistent block-access-list storage; historical
database upgrades, mixed binaries, and downgrade of candidate-written databases
are not validated. Deploy matching Composer/follower/signer versions. No public
Chiado deployment, release-tag promotion, or operator data recovery was attempted.

## Ordinary historical-drain failure found by process validation

At runtime candidate `239d32c`,
`ethereum_ordinary_burst_is_budgeted_across_sync_intervals` failed after 242.5 s.
Preparation took about 929 ms and crossed the old proof-start cutoff by 44 ms.
The recent preparation estimate then became 2,022 ms. With only two seconds
until each target, every subsequent selection chose head + 2. The native proof
check correctly rejected the still-unknown canonical parent, and settlement
never progressed. The Gnosis variant passed in the same first run.

The correction keeps target height at the known canonical head's immediate
child, bounds missing-slot timestamp offsets, and uses the existing complete
pipeline deadline for ordinary proofs too. It removes the incompatible recent
preparation predictor, preserves measured diagnostics, and retains independent
signer authorization, exact target binding, and final submission checks.
The focused target-selection regression failed before the correction and passed
afterward. Full corrected process evidence is pending; the initial failing
logs/datadir remain under `release/embedded.log` and `release/native-datadirs/`.

The baseline `late_sync_can_post_due_history_below_three_hundred_blocks_to_a_fresh_target`
case also failed: resuming 3.5 seconds into a five-second slot selected head + 2
and failed the same parent check. Its regression now requires settlement in the
original target's remaining 800 ms, exact L1 root application and follower
convergence. The former assertion demanding a later timestamp represented the
obsolete full-proof start allocation, not the native proof contract.
