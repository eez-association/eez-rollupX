# L1 quote probe readiness

| Field | Value |
| --- | --- |
| Date | 2026-10-20 UTC |
| Status | Controlled reproduction and final local validation complete; no CI run or deployment for this branch. |
| Scope | Reproduce the refund quote CI failure and synchronize the test with the completed L1 quote snapshot. |
| Baseline | `eez-association/eez-rollupX` main `a2d7b0a3be89df3c0b6ac945acd41a830278b0a7`; tree `8598c1715ba4185390632513d5ca0aa73ba80fca`. |
| Candidate | `fix/l1-quote-probe-readiness`; test-only change accompanying this record. |
| Protocol | `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`, initialized and unchanged. |
| Environment | Rust 1.95.0, Foundry 1.7.1, LLVM 22; source-built debug executables and isolated Ethereum Osaka process fixtures. |
| Contract | [Unsigned fee quote snapshots](../../configurable-settlement-policy.md#unsigned-l2-fee-quotes). |
| Related validation | [64 KiB calldata](../2026-10-20-64k-calldata/README.md). |

The final Rust source patch, computed with
`git diff --binary -- crates/eez-node-e2e/tests/fee_quotes.rs` against the
baseline, has SHA-256
`7b03e85981e9094ae15909383c1241dd0c5b2e63920e4d33560a6198dae4716a`.

## Findings

The [main CI job](https://github.com/eez-association/eez-rollupX/actions/runs/39427377462/job/118665898580)
failed `fee_quotes::l1_refund_quote_legacy_continues_after_missing_payment`.
Its probe expected `execution_reverted`, but both quote encodings reported
`unsupported_execution`, stage `gas_estimation`, with
`request did not produce a cross-chain action`. The request contained only the
four-byte `alwaysReverts()` selector.

The runtime intentionally quotes against the last completely scanned canonical
L1/L2 pair, including while a newer L1 scan is pending. A successful deployment
receipt does not establish that this snapshot includes the deployment. The
test introduced by `19685e7d9385dff338b862c2f3a8e24e64ceeed1` waited for the
receipt and immediately asserted the deployed contract's revert.

A controlled run reproduced the original assertion failure with unchanged main
runtime code. The existing RPC fault fixture rejected `eth_getLogs` while the
embedded L1 continued mining. A successful control quote exposed its pinned
parent hash; looking up that hash established the actual snapshot number.

| Observation | Controlled baseline result |
| --- | --- |
| Probe deployment receipt | Successful, L1 block 9 |
| Completed quote parent | Block 8, `0x20c3cacf34b7d49717da35f8f22e105bdaeec9d4bf89c423479a22349972db05` |
| Probe code at quote parent | Empty (`0x`) |
| Probe code at deployment block | 582 bytes |
| Probe simulation | Successful empty-code call, 21,160 gas |
| Quote diagnostics | Exact CI reason, stage and message above, for types 0 and 2 |
| Original assertion | Failed after 44.67 seconds; 1 selected, 163 filtered out |

The CI log independently records a quote at block 8 while block 9 is being
scanned, followed by the same successful 21,160-gas simulation and diagnostic.
The controlled run confirms this failure mechanism. The historical CI artifact
does not retain the probe receipt block or L1 database, so identifying its
deployment as block 9 remains an inference from its setup and logs.

## Fix and regression

Both refund roundtrips now deliberately hold scans behind the probe deployment.
They verify the old snapshot has no probe code, then start a readiness wait
using a successful quote against the fixture's established proxy. Rejected probe
quotes do not include a context, so they cannot provide this observation.
Inbound diagnostics expose the parent hash; the helper resolves that hash to
its L1 block and requires it to reach the deployment block.

The wait must remain pending while another L1 block is mined with scans blocked.
After restoring log reads, the same wait completes before the original revert
assertions run. This uses observable chain progress instead of a fixed sleep.
Unexpected quote errors still fail; the existing availability retry list and
the exact revert reason, message, data and stage assertions are preserved.
Production quoting, admission, the SDK and the 64 KiB limit are unchanged.

## Reproduction and evidence

Commands ran from the clean task worktree
`artifacts/calldata-64k-pr-20261020`, leaving the operator checkout unchanged.
The negative control was main plus fault injection and snapshot/code
observations, without the readiness/recovery block. To repeat that control from
this change, keep `eth_getLogs` blocked and omit the `ready` future, its
`select!`, and scan recovery immediately before the original probe assertion
loop.

Builds used:

```bash
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH="/usr/lib/llvm-22/bin:$PATH"
export CARGO_TARGET_DIR=/home/edu/Development/rollupX/target
export CARGO_BUILD_JOBS=2
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
forge build --root contracts
cargo +1.95.0 test --locked --offline -p eez-node-e2e \
  --test node_e2e --no-run --message-format=json
cargo +1.95.0 build --locked --offline -p eez-proof-signer \
  --bin eez-proof-signer --message-format=json
```

Cargo compiler-artifact records identify each executable and worktree manifest.
Baseline executables were copied into the evidence directory's `runtime/`
immediately after building, with SHA-256 checks. The final process run uses the
new final-source test runner with these same Composer, follower and signer
copies. The patch changes no production source, dependency or protocol pin,
so identical main runtime bytes provide the controlled before/after comparison.
These are local source builds, not published CI artifacts.
Runs set `EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` to those
copies, and `EEZ_TEST_LOG_DIR` / `EEZ_TEST_DATADIR_DIR` to retained evidence
directories.

The baseline invocation was:

```bash
"$EEZ_TEST_RUNTIME_BIN_DIR/node_e2e" \
  fee_quotes::l1_refund_quote_legacy_continues_after_missing_payment \
  --exact --test-threads=1 --nocapture
```

| Evidence | SHA-256 / reference | Availability |
| --- | --- | --- |
| Baseline test-only reproduction patch | `e3ab13dbb5e06b00fb09f4efce007ebc1fde358ebb49db13e01c8c6e5f7d27d0` | Local only: `artifacts/l1-quote-readiness/baseline/reproduction.patch` |
| Baseline test log | `cad7f9a2b5f161b8a95efbb23e45e3f3c2a83e8f4b60d14bc52d36cf1f01c886` | Local only: `artifacts/l1-quote-readiness/baseline/test.log`; process logs and failing datadirs retained beside it |
| Final process test log | `b5a65e0c5b1d602d70c98f7c13a990e5807a51419c8bb2cad6d3c7f233f5faaf` | Local only: `artifacts/l1-quote-readiness/final/test.log`; process logs and executable manifest retained beside it |
| CI failure | Run `39427377462`, artifact `cross-chain-e2e-0-39427377462-1`, artifact ID `12273251900` | Retrievable through the linked GitHub run while retained |
| Baseline test runner | `e9aa3377593397842ad52539952cdb90c170ccdff84c87a2cafb6a4c853995da` | Local only |
| Final test runner | `18ee1818cba0ba87b95d0cdc41269c39a2121d1a0c826ec9c4a829c8d5af982c` | Local only: `artifacts/l1-quote-readiness/final/runtime/node_e2e` |
| Rejected runner with cached testkit from another checkout | `c577fcd923a4ec10bb7892342c9355b83fe33b9c5caa10e42e80c5ed03fdf6d0` | Local only: `artifacts/l1-quote-readiness/cache-mismatch/runtime/node_e2e`; not credited as final-source validation |
| Composer | `621c252f5e88c51bd0e0f55bc00699962e90fe58bf7226699ea1191db1739c88` | Local only |
| Follower | `3c45372550760eee52398541b55f78a9118e15414a41cb62e27ace1162966476` | Local only |
| Signer | `eb58c024aa60936e3a54656b74a4fff35232ac2d720c99e1f4aab243f957f94e` | Local only |

An earlier instrumentation attempt incorrectly expected `l1ParentNumber` in
inbound diagnostics and stopped before exercising the probe. Its logs, patch,
executables and failing datadir remain in
`artifacts/l1-quote-readiness/harness-error/`; it is neither a reproduction of
the CI failure nor a passing check. No failure was discarded or counted as a
retry-only pass.

The first fixed candidate passed all four selected process tests in 231.67
seconds, including both controlled refund cases and both 64 KiB boundary cases.
Clippy identified a 16,624-byte refund-test future. The readiness future
was moved from a stack pin to `Box::pin`; no lint was suppressed. The first
candidate's source patch, executable manifest and test log remain in
`artifacts/l1-quote-readiness/candidate/`. The initial Clippy failure remains in
`artifacts/l1-quote-readiness/checks/clippy-initial.log`. Final-source validation
is recorded separately below.

An intermediate Cargo build succeeded, but its post-build runtime identity check
failed before running tests: the copied Composer/follower hashes differed from
the preserved baseline. Another checkout was building into the same target
directory, and the shared follower hash changed again during inspection.
Those copies are retained in
`artifacts/l1-quote-readiness/cache-mismatch/post-build-runtime/`
and were not executed for validation. Using the baseline runtime copies then
passed four tests, but fixture paths revealed a cached testkit from the other
checkout. Its testkit source also differed. Cargo's compiler-artifact record
named this worktree while reporting that library as fresh; that metadata alone
did not establish the linked library's source identity. This entire run is
retained in `cache-mismatch/` and excluded from final-source validation.

The final build uses distinct package profiles for the three workspace libraries
modified in the competing checkout. Debug information is disabled for those
libraries to select separate Cargo artifacts; optimization and features are
unchanged. No repository configuration was edited. All three compiler-artifact
records must report `fresh: false` and this worktree's manifest path.
The test runner is copied as its compiler-artifact event arrives, and the
fixture root is explicitly remapped using the testkit's existing runtime
`CARGO_MANIFEST_DIR` support.
The final build satisfied all three fresh-library checks; its process logs
confirm fixture paths under this worktree.

```bash
export CARGO_BUILD_JOBS=4
cargo +1.95.0 test --locked --offline \
  --config 'profile.dev.package.eez-composer.debug=0' \
  --config 'profile.dev.package.eez-node.debug=0' \
  --config 'profile.dev.package.eez-testkit.debug=0' \
  -p eez-node-e2e --test node_e2e --no-run --message-format=json
export CARGO_MANIFEST_DIR="$PWD/crates/eez-node-e2e"
```

## Final-source checks

The selected process tests run serially:

```bash
"$EEZ_TEST_RUNTIME_BIN_DIR/node_e2e" \
  fee_quotes::l1_refund_quote_ fee_quotes::calldata:: \
  --test-threads=1 --nocapture
```

| Check | Result |
| --- | --- |
| Selected `fee_quotes::l1_refund_quote_` and `fee_quotes::calldata::` process cases | Four passed, zero failed or ignored, 160 filtered out; 227.54 seconds. Both refund encodings observed deployment at block 9 and an empty-code quote parent at block 8, then waited for parent 10 before passing the original revert assertions. Both 64 KiB cases passed boundary/rejection, native settlement and independent follower replay assertions. Refund cases also ran semantic checks and independent replay. |
| `cargo +1.95.0 fmt --all --check` | Passed. |
| `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings`, with the same package profile overrides above | Passed after the future-size correction and again with freshly checked libraries; `artifacts/l1-quote-readiness/checks/clippy-final.log` retains the latter run. |
| `node sdk/l2-fees.test.mjs` | Seven passed, none skipped, including the 64 KiB boundary. |
| Documentation | Relative links resolve; shell examples pass `bash -n`; `git diff --check` passes. |

The other fee-quote cases and full workspace unit, release-artifact and Kurtosis
matrices were not rerun for this test-only change. No live network deployment
or operator service restart was performed. This record establishes local process
behavior, not a successful CI run for the new branch.
