# L1 history and offline v2 proving implementation

Status: implemented and locally validated under the clarified new-network scope.
The [proposal](../../proposals/l1-origin-and-offline-proving.md) is authorized for new networks only.
The [coverage map](coverage.md) links O1–O15 to the implementation and regression tests.
The [portable validation record](validation.json) retains selected exact commands,
results, executable identities and log digests; raw logs remain local-only.

On 2026-10-23 the user clarified that four days was an example of sources older
than L1's 256-block window, not a required exhaustive load test. The acceptance
gate is therefore a complete expired-source multipart/offline/follower exercise.
The larger optional run was intentionally stopped with its partial evidence retained.

## Current validation summary

Commands run from the isolated worktree with Rust 1.95.0, Foundry 1.7.1,
`--locked`, the owned `TMPDIR` and LLVM 22. Builds now use one Cargo job after
the recorded two-job OOM failure. Logs named
below are local-only under `artifacts/validation/`; historical failures and
their corrections are retained later in this record.

| Check | Current result |
| --- | --- |
| `cargo +1.95.0 fmt --all --check` | Passed after the final expired-source regression (`offline-expired-source-format_check.log`). |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed after the final expired-source regression (`offline-expired-source-clippy.log`). |
| `cargo +1.95.0 test --locked --workspace --no-fail-fast -- --test-threads=1` | Terminated with SIGTERM/143 during native E2E after 58 passed, two known failures and four ignored tests (`offline-workspace-complete.log`). Both failures have red/green fixes below. The termination source is not established. Continuation groups exposed further failures, corrected below. The final native inventory completed separately: 168 passed, four failed and five ignored in 9,891.29 seconds (`offline-native-final-rules.log`). All four failures subsequently passed unchanged after their two corrections: eight recovery cases and three helper callers passed on the explicit role build. These are separate completed runs, not an aggregate green workspace invocation. |
| Workspace continuation, `cargo +1.95.0 test --locked --workspace --exclude eez-node-e2e --no-fail-fast -- --test-threads=1` | Passed: 1,560 tests, zero failures, three explicitly ignored profiling/fixture utilities. Includes unit, integration and documentation targets; empty targets are not counted as coverage (`offline-workspace-non-process-complete.log`, `offline-workspace-non-process-results.json`). |
| Composer manager-recognition regression | Failed at the intended runtime-recognition gate before the fix, then the full Composer unit suite passed: 404 passed, one ignored utility. The unchanged native quote/settlement/follower scenario passed in 64.70 seconds (`offline-estimation-manager-red.log`, `offline-composer-manager-fixed.log`, `offline-nested-create2-ethereum-fixed.log`). |
| Offline admission/root-pin regression | The resumed native fitting-wave test failed: incremental estimate 10,220,656 versus final projection 12,229,444. The focused unit test reproduced the unexpected fallback. After correction, all 405 Composer unit tests pass, with one ignored profiling utility (`offline-admission-root-pin-final.log`). All 64 contract gas tests pass using the actual offline manager and a measured base bound of 190,000 (`offline-admission-manager-gas-final.log`). All 28 native admission cases pass in 1,686.41 seconds, including Ethereum/Gnosis fitting waves, conservative exclusions, replacements and follower replay (`offline-admission-native-fixed.log`). |
| Workspace build and production target selection | One-job workspace retry passed in 22m50s; production target selection passed in 7m18s (`offline-build-workspace-single-job.log`, `offline-build-production-selection.log`). These are earlier source checkpoints; subsequent affected runtime rebuilds are recorded below. The earlier OOM failure is retained. Debug builds do not establish CI release-artifact identity. |
| Six missed-slot/late-original process regressions | All six passed (`offline-recovery-six-process.log`). |
| Short multipart cold offline proof and clean follower | Passed again on the runtime with the pending-import restriction: 120 L2 blocks, two historical parts and one actual-parent part across BPO1; missing source and unused bridge-header rejection passed (`offline-pending-and-observation-process.log`). |
| Pending RPC and executed-state observations | All 29 node-common tests passed. The real-RPC import-boundary regression, corrected fee scenario and short multipart exercise passed together: three cases in 152.47 seconds (`offline-pending-and-observation-process.log`). |
| Revert-observation helper | All three actual callers passed with the corrected executed-state read, including the unchanged failing inbound scenario. They passed again on the final explicit role build: three passed, zero failed/ignored in 160.89 seconds (`offline-revert-helper-role-profile-results.json`). Every specific revert-byte and rollback assertion remains. |
| Two Composers and two keyless followers | Passed with an independent delayed-source oracle: 50 identical blocks, exact origin reads at every block on all four nodes, and every imported L1 hash checked on all four nodes (`offline-native-oracle-process.log`, 59.24 seconds). The obsolete settlement-triggered seed assertion failed before its migration. |
| Partial consumption, ingress and history recovery | All four focused native cases passed in 195.62 seconds with signer egress denied: conditional checkpoint re-proving and stale-proof rejection in the same L1 block; unchanged ordinary Safe routing; stable ordinary history through delayed settlement; twenty ordinary transactions recovered after an imported-history reorg, then settled and independently replayed (`offline-four-recovery-process.log`). |
| Fork-boundary anchor selection | The focused regression failed before the fix. All 406 Composer unit tests now pass, with one ignored profiling utility; all nine deployed-manager context tests pass (`offline-anchor-inclusion-rules-red.log`, `offline-anchor-rules-composer-unit.log`, `offline-anchor-rules-manager.log`). |
| Cold restart from a persisted settlement | Three existing native restart cases exposed an unprepared L1 history cache before accepted-endpoint validation. After correction, all 44 deriver unit tests, formatting and workspace Clippy pass. All eight focused process cases passed on the explicit role build, including the unchanged failures, partial consumption, retained witnesses and two reorg cases: zero failed/ignored in 476.52 seconds (`offline-restart-history-process-results.json`). |
| Expired-source multipart acceptance | Passed with the existing short regression: two passed, zero failed/ignored in 213.61 seconds. Source age 304 was rejected as an on-chain anchor; complete ancestry authenticated it through a recent anchor. Four parts settled with cold denied signers, both modes and BPO1. The clean follower compared every one of 2,000 blocks, all 401 imported hashes and 32 application records (`offline-expired-source-results.json`). |
| Optional four-day multipart stress | Earlier readiness/driver timeout failures remain recorded. The latest run was intentionally stopped after 16 settlements / 160,000 L2 blocks when the user clarified the acceptance scope. It did not reach final replay (`offline-four-day-current-intentionally-stopped.json`). This is partial stress evidence, not a pass or an unresolved four-day acceptance gate. |
| Integration contracts | 108 passed, zero failed/skipped after the gas-fixture correction; format passed (`offline-admission-contracts-all.log`, `offline-admission-contract-format.log`). |
| Pinned core contracts | 785 passed, zero failed; three live-fork skips and the two documented exclusions (`offline-core-final.log`). Build passed (`offline-core-build-final.log`). |
| Protocol Anvil scenarios | All 64 passed (`offline-protocol-anvil.log`). |
| Genesis generator and deployment-profile CLI | Fixture check and actual ABI/hash comparison passed (`offline-genesis-final-check.log`, `offline-genesis-helper-profile-decoded.txt`). |
| Python tooling/configuration | 395 tests completed, zero failures/errors, eight explicit opt-in skips (`offline-ci-tooling-owned-temp-final.log`). Temporary-directory workaround is documented below. |
| Mandatory policy configuration | Passed for Chiado, Kurtosis and Ethereum (`python3 scripts/check-settlement-policy-config.py`, `offline-settlement-policy-config.log`). |
| SDK fee contract | All seven existing tests passed with Node v26.7.0: quote/type selection, 64 KiB boundary, gas/nonce/account binding, exact send, expiry and clock skew (`node sdk/l2-fees.test.mjs`, `offline-sdk-fees-results.json`). These are SDK tests, not live wallet coverage. |
| Signer documentation and frozen wire corpus | Rustdoc with warnings denied passed; all 29 transcripts passed real replay and static manifest checks. |
| Dependency checks | `cargo deny` and `cargo audit` exited zero with existing warnings; no external dependency identity changed. |
| Documentation | Final check: 441 local links/anchors across 33 edited Markdown files resolved; 49 shell examples passed `bash -n`; no issues (`offline-final-documentation-check.json`). Git whitespace checks passed, and the protocol bundle verified against its recorded prerequisite. |

## Current protocol artifact

The isolated protocol commit is `b5d94eafab15eb3fdd08a73ad9a693f7a3fcb820`,
based on `84a5b5207880f192a6a5896cbfe0bac5249db47f`. It is not published to
the protocol remote. The verified [Git bundle](protocol.bundle) retains the
commit and its branch for review/reproduction; SHA-256
`64e21287757b47154803a08d0728c63dc2bd4696a937615d905751608d7d8562`.
After initializing the starting pin, import it with
`git -C eez-core-protocol fetch ../docs/audits/2026-10-22-l1-history-offline-proving/protocol.bundle codex/l1-history-offline-proving`,
then check out the recorded commit before submodule validation. Remote-only
submodule initialization cannot retrieve this unpublished commit.

### Review regressions and multipart validation

The entries below preserve intermediate results and failures in order. Statements
that a check was pending describe that checkpoint; the table above gives its
current status.

- The final documentation review found obsolete active upgrade instructions in
  the Chiado deployment and existing-rollup runbooks, including a removed signer
  specification anchor. The current procedure now requires the offline manager,
  history bootstrap and schedule from genesis, and limits runtime replacement,
  backup restoration and joining to that same profile. Earlier deployment audits
  remain historical evidence. Recovery explicitly needs all intermediate L1
  headers, not merely the selected origins; the signer still receives evidence
  without chain access. Link/anchor and shell-example checks are retained in
  `offline-edited-documentation-links.json` and
  `offline-runbook-shell-syntax.json`; these checks do not execute a live restore.
- The workspace continuation excluding native E2E completed successfully after
  the manager fingerprint and retained-session-predecessor fixes: 1,560 passed,
  zero failed, three ignored utilities across 22 nonempty test targets. Twenty
  additional targets selected no tests and supply no behavioral coverage. The
  command's compile phase took 21m47s with one Cargo job. It temporarily blocked
  source signer builds in both running process suites; no proof deadline was
  armed during that wait. The long fixture subsequently settled its fifth part
  at block 50,000. This checkpoint still does not establish four-day completion.
- The continued native admission suite exposed a missed consumer of the new
  mandatory accepted-root pin. Incremental admission used checked entry
  discounts, but `Projection::batch_gas` recognized only batches with no root
  pins and fell back to general reserves. The first real failing fitting wave
  and its exact numeric observations are retained in
  `offline-admission-root-pin-native-failure.json`. A focused regression failed
  before the fix (`offline-admission-root-pin-red.log`, exactly one test);
  the preceding short-name `--exact` invocation selected zero tests and supplies
  no evidence. A later forty-request wave also failed its one-Sync assertion
  on the unchanged earlier binary; its result remains to be retested.
- The updated admission model accepts exactly one nonzero own-rollup pin for
  shaped pricing and reserves its ABI bytes. Missing, foreign, zero and duplicate
  pins retain conservative pricing. The separate contract measurement fixture
  now deploys `OfflineRollup`, commits its context and root pin, and uses the
  maximum 64 rule epochs. It reproduced a real base-reserve overrun:
  165,440 gas with 64 blob references exceeded the former 160,000
  (`offline-admission-manager-gas-red.log`). Rust and Solidity now reserve
  190,000, measured cost plus 10% rounded up to 10,000. Both anchor modes and
  first/later settlements pass. All 64 gas tests pass with the new fixture;
  estimator events use `candidate-framing-11` to distinguish the changed model.
  The two-proxy positive forwarding case now budgets the new base and both
  possible creations; its insufficient-gas rejection, atomic rollback and full
  final-root assertions remain. Its intermediate fixed-750,000 failure is
  retained in `offline-admission-manager-gas-fixed.log`.
- The still-queued native groups 2–5 were stopped before any case started so
  they can use the rebuilt test executable and matching estimator label. Group 1
  and the long exercise continue on their retained binaries. The queued Composer
  unit command was also deliberately restarted with the native build environment;
  those intentional SIGTERM records are separate from earlier unexplained
  terminations. Rust and native post-fix results are still pending.
- The corrected Composer suite subsequently passed: 405 tests, zero failures,
  one ignored profiling utility. Both workspace format and Clippy checks pass.
  The first runtime rebuild command named the package instead of its binary
  (`--bin eez-node`) and failed before compilation; the corrected command uses
  `--bin eez-composer`. These invocation failures are retained in
  `offline-admission-build-validation-final.json`; runtime build results are
  recorded separately in `offline-admission-runtime-build-validation.json`.
- The corrected runtime build passed in 18m00s; the matching native test
  executable rebuilt in 37.30 seconds. Their SHA-256 identities are retained
  in `offline-admission-executables.json`. The complete admission module and
  remaining groups 2–5 are queued serially after the existing first group.
  The earlier long-run node keeps its original inode: the later fixes concern
  fee/gas estimation and retained client sessions, while this exercise uses
  explicit settlements and a cold signer per part. Its execution/header/proof
  rules are unchanged. At this checkpoint seven parts have settled through
  L2 block 70,000; no final follower result exists yet.

- The short multipart process exercise passed: 120 L2 blocks, 25 imported L1
  headers, three native blob settlements, a cold network-denied signer per part,
  and a fresh follower comparing every block hash/state root/PREVRANDAO plus
  application origin/history reads. Log:
  `artifacts/validation/offline-multipart-short-diagnostic-fixed.log`.
- The first long run was intentionally stopped after identifying competing
  automatic settlement triggers in the manual writer fixture. Owned process
  identities and the reason are retained in
  `artifacts/validation/offline-multipart-first-run-stopped.json`. The corrected
  fixture keeps automatic triggers beyond the complete history while each
  actual cold proof remains limited to its 10,000-block part. This interrupted
  run is not four-day acceptance evidence.
- The existing consecutive-skipped-slot process regression failed before the
  recovery fix: no next-slot dispatch, with `canonical L1 has not resolved
  source cutoff` from the sequencer. Log:
  `artifacts/validation/offline-consecutive-skipped-slots.log`. The fix gives
  only an owned, deadline-authorized replacement the actual-parent endpoint
  exception, revokes it on new canonical evidence, and retains the shared
  replacement bound. Post-fix runtime verification is pending.
- Final integration contract checks: `forge fmt --check --root contracts`
  passed; `forge test --root contracts`: 108 passed, zero failed/skipped
  (`offline-contracts-final.log`).
- Review reproduced a first-unsupported-L1-block gap: the parent header remained
  supported and `posting_blob_limit(2000)` accepted six blobs even though the
  fixed L1 profile activated an unsupported fork at 2000. The focused red test
  is `offline-unsupported-target-red.log`. Finalization now rejects the active
  target period independently of the supplied parent header layout. The initial
  `--lib` invocation selected no target (the signer is a binary); its corrected
  `--bin eez-proof-signer` invocation selected exactly one failing regression.
- The controlled four-day run produced 69,121 dense L1 headers and over 43,000
  L2 blocks, but failed before proving: the manual fixture's three-minute wait
  did not find the exact durable witness for block 10,000. There was no capture
  error in the retained node log. This does not establish whether capture was
  merely behind or stalled. The first witness directory was an ordinary
  temporary directory and was lost during teardown; the node datadir and logs
  remain. The fixture now uses the existing failure-retention type for witnesses,
  enables the existing witness-generation diagnostics, and gives the large
  part the same bounded 30-minute readiness budget as production. No execution
  or proof assertion is removed. Rerun and diagnosis are pending; this failed
  run (`offline-multipart-four-days-owned.log`) is not four-day acceptance.
- Documentation: 316 edited local Markdown paths resolved; edited shell scripts
  passed `bash -n`. The 29 conformance vectors passed their static manifest
  checks. Settlement-policy configuration passed for Chiado, Kurtosis and
  Ethereum. Both kernel network-denial harness tests passed on x86_64.
- The final genesis fixture check passed with the recorded state root
  (`offline-genesis-final-check.log`). Its local Rust helper build is included
  in that check; a separate deployment-profile CLI check is pending.
- The next signer run selected 329 tests: 326 passed, one failed and two fixture
  generation utilities were ignored (`offline-signer-final-unit.log`). The
  unsupported-inclusion regression passed. The failure was the successful retry
  in `finalize_deadline_preserves_prefix_and_allows_retry_in_the_same_epoch`:
  real replay exceeded the test's one-second request deadline under concurrent
  test load. Its admission failure and unchanged-prefix assertions had passed.
  The test now holds the same admission permit and advances Tokio's clock only
  while no worker can start, then resumes real time for the normal-budget retry.
  Tokio `test-util` is a dev-dependency only. Verification is pending; the failed
  run is preserved and is not counted as a passing signer suite.
- The multipart negative test now also removes a header later than its entire
  first L2 part, preserving every selected source and the final anchor. It must
  reach `nonconsecutive L1 ancestry` in the real network-denied signer. This
  adds coverage; it has not yet run in either multipart fixture.
- `offline-composer-driver-l1-final.log`: Composer 403 passed, one pre-existing
  ignored utility; L1 193 passed, including the new replacement permission and
  revocation test. Driver selected 75: 68 passed and seven failed. Two execution
  fixtures lacked the mandatory parent history account, four witness fixtures
  retained an invalid bootstrap/schedule, and one scheduler fixture placed
  genesis before its required bootstrap interval. These fixtures now provide
  the actual new-profile state/evidence while retaining their original gas,
  receipt, ancestry, observation and no-pool-fallback assertions. Their rerun
  is pending. The deriver's input-matching fixture also now derives randomness
  from checked history instead of its removed environment field.
- `cargo deny check advisories bans sources` and `cargo audit` exited zero
  (`offline-cargo-deny.log`, `offline-cargo-audit.log`). Audit reports nine
  existing allowed warnings, including unmaintained/yanked dependencies and
  `RUSTSEC-2026-0253` for `lru`. No dependency version/source/checksum was changed:
  `offline-lockfile-comparison.json` compares the merged baseline with this
  tree and records only six workspace dependency-list changes.
- `forge build --root eez-core-protocol` succeeded with warnings
  (`offline-core-build-final.log`). A subsequent command mistakenly applied
  `bash -n` to the Python seccomp launcher and failed; the corrected Python
  compile check and Bash checks of the actual shell scripts passed. That
  command-selection error is separate from the successful Solidity build.
- The real deployment-profile helper compile found a missing direct
  `alloy-sol-types` dependency (`offline-genesis-profile-helper-build.log`).
  It is now declared for the example. Offline Cargo resolution changed only
  the `eez-node` dependency list, bringing the lockfile comparison to seven
  workspace lists with no added/removed package or version/source/checksum
  changes. This changes the validation-identity digest, so subsequent process
  runs must rebuild both node and signer against the updated lockfile.
- The queued driver/signer/Clippy rerun exited 143 while still waiting for the
  artifact lock (`offline-driver-fixtures-final.log`); it executed no tests.
  The cause of that termination is not established. A complete Clippy pass is
  queued with `--keep-going` to collect remaining diagnostics together.
- The deployment helper's corrected build and actual CLI check passed. Its
  ABI-decoded profile names the same genesis hash as the standalone hash output
  (`offline-genesis-helper-profile-decoded.txt`, `offline-genesis-helper-hash.txt`).
  The fixture has delta=1, k=5, q=2 and the expected local L1 rules/code hashes.
- Full workspace Clippy with all targets and warnings denied passed after
  correcting the remaining stale unit fixtures (`offline-clippy-workspace-stack-fixed.log`).
  The old `native_blocks_are_excluded_from_ordinary_preparation` test was removed:
  it asserted the retired v1 ordinary-only preparation restriction. V2 retains
  validated native blocks and final settlement still independently authorizes
  their effects. Exact ancestry, persistence and reorg tests remain. Shared
  network setup now boxes its expanded deployment future; scenario assertions
  and production execution are unchanged. Subsequent test-only catch-up edits
  still need their compile/check pass.
- The long catch-up fixture now submits 32 real ordinary application transactions
  while producing historical blocks. Their state records origin/hash/opcode
  RANDAO at their actual L2 heights. Final independent replay must reproduce
  those records as well as every block and imported hash. These are semantic
  workload checks, not a throughput benchmark; execution is pending.
- The manual catch-up writer now uses the existing speculative-depth limit to
  produce one admitted part plus four boundary blocks. Its earlier million-block
  setting allowed production to outrun durable capture while the first part
  waited. This bounds the fixture's unsettled L2 work without shortening the
  dense four-day L1 bridge. The earlier timeout remains recorded; a passing
  bounded run would not establish performance at a million-block backlog.
- The first bounded configuration was correctly rejected at startup: depth 44
  cannot coexist with the million-block settlement trigger. No network test
  ran (`offline-multipart-short-bounded.log`). The corrected fixture sets both
  trigger and signer capacity to the admitted part, and uses the existing
  one-wei automatic fee cap to leave posting to the manual writer. That cap is
  below the private Ethereum fixture's execution base fee. Manual posts use
  independently calculated actual fees. The production configuration guard and
  fee admission are preserved.
- Focused execution/signer rerun passed: deriver 44, driver 66, EVM 26 and
  signer 327, with two explicit signer fixture utilities ignored
  (`offline-execution-signer-fixtures-fixed.log`). The controlled admission
  deadline test and all frozen v2 recordings passed. This package selection
  does not enable the driver's `witness` feature; its remaining nine tests
  still require the workspace feature selection. Empty doc-test selections are
  not counted as coverage. The updated integration contracts again passed all
  108 tests (`offline-contracts-application-final.log`).
- Fresh review corrected the streaming document's identity claim: the
  validation namespace binds execution/profile/dependency identity, while proof
  policy is independently checked in settlement and by the client signature
  gate. The actual 131,072-header evidence ceiling and eight-request acquisition
  bound are now explicit in the owning specifications. This ceiling does not
  bound the permanent mapping, but it can reject an excessively long unsettled
  ancestry bridge even after L2 range partitioning.
- The updated short multipart exercise passed all three 40-block parts
  (`offline-multipart-short-admission-bounded.log`, 153.29 seconds including
  the source signer build). Both omitted-source and bridge-only-gap requests
  reached their specific rejection gates before the complete request succeeded.
  A fresh follower matched every block hash/state root/PREVRANDAO, the sampled
  opcode/current-origin reads and all 25 imported hashes. The final hash was
  `eb79f1a68f1e8dbcc03b41b2a825b4aadc9f1b984793e186e2db21b653d66851`,
  with state root
  `6a1358e5f96a473d2f64a1d037af168cd12d29370f6504744e411b96ae5cb226`.
- The bounded four-day rerun has produced 69,121 dense L1 headers and captured
  the first 10,000-block part. It is still running; neither the first proof nor
  complete replay is claimed yet. The workspace unit/binary build and six
  missed-slot/late-original recovery process regressions are also pending.
  Their logs are `offline-multipart-four-days-bounded.log`,
  `offline-workspace-unit-binary.log` and `offline-recovery-six-process.log`.
- Current Rust/contract formatting, `git diff --check`, Python compilation and
  Bash syntax checks passed. An optional `perf` capture of the owned node was
  unavailable because the host's `perf_event_paranoid=4` disallows recording;
  no host setting was changed and no profiling result is claimed.
- The workspace unit/binary run selected 1,543 tests: 1,539 passed, one failed
  and three explicit utilities were ignored. The sole failure was the testkit's
  inventory check for the retired `eez.deriver.l1.reorg.noop` event. Every reorg
  now invalidates/rechecks history even when no settlement was removed, and
  emits the existing `retreated` event. The obsolete constant and its harness
  references are removed; the inventory assertion and required nonzero reorg
  detection remain. All 75 driver tests (including witness support), 327 signer
  tests, 204 protocol tests and 197 L1 tests passed in this run. The targeted
  inventory rerun and complete workspace command remain pending.
- The current full workspace Clippy pass and signer rustdoc with warnings denied
  passed (`offline-clippy-workspace-final-current.log`,
  `offline-signer-doc-final.log`). The subsequent test-only signal cleanup needs
  its final check. The first long part settled L2 1–10,000 with a cold denied
  signer, 69,301 headers, 156,278,080 request bytes and 632,165 witness items.
  Replay/proof took 12,086 ms in this run; this excludes source compilation,
  witness capture and settlement and is not a throughput comparison. The full
  35-part exercise remains pending.
- The post-fix recovery run passed all six selected process regressions
  (`offline-recovery-six-process.log`, 1,240.59 seconds including source builds):
  consecutive missed slots, automatic selection, historical-prefix replacement,
  configured-coinbase empty boundaries, and both late-original inclusion cases.
  The previously failing consecutive-missed-slot scenario now reaches its next
  canonical block without shrinking or losing the accepted group.
- The stale-event inventory regression passed its exact one-test rerun
  (`offline-stale-reorg-signal-fixed.log`). The complete workspace suite is now
  running with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`; its final result remains
  pending. The bounded long exercise has canonically settled its first two
  10,000-block parts with cold denied signers, but final replay is still pending.
- Source-build overhead during these concurrent runs includes repeated native
  dependency rebuilds. The `ring` build-script fingerprint records inherited
  `CARGO_MANIFEST_DIR`/`CARGO_PKG_*` values from Cargo-launched E2E tests; the
  directly launched long test does not inherit those values. Switching between
  them invalidates that fingerprint. No stale-binary fallback or shared tool
  modification was introduced. Recorded proof timings exclude this build cost.
- The two-job workspace build hit host memory exhaustion. The kernel journal
  confirms rustc PID 553090 was OOM-killed at 2026-10-22 22:31:58 UTC with
  11,629,944 KiB anonymous RSS. Cargo reported SIGKILL while its other compiler
  continued; no Rust diagnostic identifies a source error. The source/profile
  remain unchanged for the planned one-job retry. The sanitized evidence is
  `offline-build-workspace-sigkill.json`. Runtime tests remained alive, waiting
  for Cargo's artifact lock. The production feature graph independently excludes
  Tokio `test-util` (`offline-production-feature-tree.txt`); this static result
  does not replace the pending build.
- The documentation review corrected the remaining v1 window definition,
  obsolete full-terminal-DA wording, a moved chain-configuration test link and
  the Chiado deployment bootstrap/output-file description. Both source helper
  fallbacks now select Rust 1.95.0. All ten mocked deployment preflight tests
  passed again (`offline-deploy-preflight-final-doc-review.log`); this is not a
  deployment broadcast.
- The bounded four-day run failed after 4,339.99 seconds. Parts 1 and 2 were
  canonically settled; the cold signer completed part 3 (L2 20,001–30,000) in
  10,330 ms with 65,718 L1 headers and 183,508,529 request bytes. Its posting
  then timed out: the controlled L1 driver had exited on its ten-second
  `engine_getPayloadV5` HTTP timeout at 22:26:34 UTC, during the host memory
  pressure that later OOM-killed the compiler. This is not four-day acceptance.
  Logs, exact executable hashes, process resource observations and both failure
  datadirs are retained; `offline-four-day-bounded-failure.json` records their
  paths and completed checkpoints. The next run must follow completion of the
  standalone builds, with unchanged semantic assertions and no overlapping
  heavy compilation. A later pass must retain this environmental failure in
  its validation history.
- The one-job workspace build and the exact production package/bin/example
  selection both passed. The latter uses the development profile rather than
  CI's release profile; no release artifact or published deployment is claimed.
  The controlled dense-history fixture is now being strengthened before rerun:
  BPO1 occurs midway through its L1 ancestry, and its final ordinary range uses
  actual-parent anchoring after eleven explicitly missed L1 slots. Earlier
  parts retain historical anchors. The target is armed after signer compilation
  and remains exact; no assertion or production deadline is relaxed. This
  fixture extension still needs compilation, short process validation and the
  full four-day run.
- The refined short process test passed in 83.86 seconds: two historical parts
  and one actual-parent part, a BPO1 transition, three cold network-denied
  signers, both incomplete-evidence rejections and a fresh follower comparing
  all 120 block hashes/state roots/PREVRANDAO and all 25 imported L1 hashes.
  Final hash: `9d79e6d238f8a5cc03fa95febb2adfebaaaf7ac0e0b3a517208bfb04495e6fe1`;
  root: `641e9eb023ffdef1d7e42ccc2a09e4d2b331b22f16b0380bcfedf2de224620d5`.
  Log: `offline-multipart-short-two-mode.log`. The full workspace Clippy pass
  also passed after these test-only changes (`offline-clippy-two-mode-history.log`).
  The new four-day run uses the same flow and is still pending
  (`offline-multipart-four-days-two-mode.log`). No heavy build is deliberately
  overlapped with it. Direct launches use the same public Cargo package metadata
  as the Cargo-launched suite; each cold signer still performs its normal
  source build, now taking 0.48–1.08 seconds in the short run.
- The full workspace process suite exposed an omitted fee-estimation consumer:
  the strict deployment fingerprint still recognized the previous generic
  Rollup manager. The Ethereum nested-CREATE2 fee-quote case failed, and its
  exact diagnostic rerun failed again after 90 seconds of unavailable quotes
  (`offline-nested-create2-ethereum-diagnostic.log`). Both failed datadirs and
  node/signer logs remain. The current OfflineRollup runtime has 6,524 executable
  bytes, versus the obsolete 3,295-byte fingerprint. Independently normalized
  compiler artifacts confirm the other registry/proxy/verifier/wrapper
  fingerprints are unchanged (`offline-estimation-deployment-fingerprints.json`).
  A runtime-fixture regression is being run before changing recognition; its
  purpose is to accept the new manager while still rejecting modified code.
- The new regression failed at `matches(&code, &ROLLUP)` before the fix.
  Recognition now names only the new OfflineRollup runtime; it does not add a
  permissive fallback or retain the old manager. All 404 Composer unit tests
  passed, with one pre-existing ignored utility (`offline-composer-manager-fixed.log`).
  The updated node source build passed in 33.80 seconds, and the exact native
  fee-quote/settlement/follower scenario is running again. Executable identities
  are in `offline-manager-fixed-executables.json`. The ongoing long catch-up
  still uses its recorded earlier node inode; this change affects advisory
  fee/deployment recognition, not its execution/history/proof rules.
- The unchanged nested-CREATE2 process case passed with the fixed node in
  64.70 seconds (`offline-nested-create2-ethereum-fixed.log`): explicit-gas and
  estimated-gas quotes, existing and newly created child calls, nested reentry,
  canonical settlement and an independent follower all succeeded. The fix
  changes one strict manager fingerprint; deadlines and scenario assertions
  were not changed. Full workspace Clippy then passed with all targets and
  warnings denied (`offline-clippy-manager-fixed.log`). The stored manager
  runtime fixture exactly matches the compiled integration artifact, SHA-256
  `8552c91e282eea2a1fd22abcd1631f74eaa7dd38ef60283b432a4bc5d4760642`.

### Fresh correctness review, pending runtime completion

The review followed the actual callers again: shared schedule/header checks,
the mandatory EVM hook, streaming admission and Finalize, native effect/DA
binding, manager context, public follower reconstruction and replacement
authorization. In particular:

- `HeaderChain::verify_cutoff` resolves both inherited and new selections and
  requires exact endpoint termination. Only the actual-parent context permits
  the last-produced-header exception, with a strictly later target timestamp.
- `CandidateContext` rejects historical posting context and every decoded
  cross-chain action, including observations without an L2 delivery. The
  accepted-root comparison occurs after whole-group execution binding; the
  manager independently reads the registry's actual block-hash commitment.
- Composer chooses historical anchoring before proving only for an ordinary
  terminal at or before its canonical parent timestamp. Live semantic actions
  retain actual-parent anchoring. The batch selector changes the proof context,
  while the submission retains its separately checked actual parent/target and
  normal ownership/deadline accounting; this is not a fallback after rejection.
- The history hook checks code/storage identity, derives its own calldata and
  uses the fixed separate gas budget. Its temporary system-call journal cannot
  commit a reverted prefix, and the hook preserves the post-cleanup caller
  account while leaving receipts and transaction gas accounting alone.
- Historical rule identity is derived at the anchor but compared with the
  actual inclusion period by the manager and follower. The signer separately
  rejects an unsupported actual inclusion target even when its parent header
  still has a supported layout.
- Old signer providers, v1 proving/preparation RPCs and settlement-driven seed
  history have no production callers left. The `prove.v1` namespace retains
  only the capabilities query and shared messages imported by v2. The earlier
  native bidirectional, changing-static and caught-revert runs all used the
  network-denial launcher; offline process coverage includes actual-parent
  execution, not only historical catch-up.
- A further review of the concrete contract and signer inputs confirmed that
  profile initialization is atomic, registration requires the committed genesis,
  and manager context is read before the registry mutates its queues. The
  historical mode cannot use current/zero/reserved/expired anchors. The ordinary
  storage verifier hashes the exact imported header and binds account and slot
  keys independently, with explicit canonical non-inclusion and bounded nodes.
  These checks retain the configured signer and proxy-admin trust assumptions;
  they do not establish L1 consensus or turn the ECDSA backend into a ZK proof.
- The configuration/caller search found no signer chain provider, implicit
  `.env` loader, preparation lease or v1 proving implementation. Retained
  `PreparedSettlingBlock` execution facts and the shared v1 protobuf namespace
  are current internal infrastructure, not retired proving paths. The kernel
  launcher denies outgoing connections and inherited socket descriptors while
  allowing the test harness to connect to the signer; its aarch64 filter remains
  unexecuted on this x86_64 host.

The multipart fixtures use real Ethereum execution with the denser five-second
L1, one-second L2 cadence. The focused expired-source fixture passed with BPO1,
both modes and complete independent replay. The optional four-day stress run
was intentionally stopped after the user's scope clarification. Native Gnosis
has separate shorter checks; these results establish no four-day performance.

## Isolation and sources

- Base: `origin/main`, `a50c326d99f7cd1c21406eedbf370a344015a6b5` (fetched 2026-10-22).
- Branch: `codex/l1-history-offline-proving`.
- Worktree: `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`.
- Initial status: clean, before integrating dependencies or copying the proposal.
- Required unmerged dependency: v2 streaming backport `00fe7bc2541a56f26f088d0a7baf567dac5059de`.
  Latest main only contained the v1 schema. Integrated the dependency with merge
  `8621b0368f4f42dc871dd84c0aaf5b6c4599bfbd`; no textual conflicts, newer main retained.
- Starting protocol gitlink and actual initialized checkout:
  `84a5b5207880f192a6a5896cbfe0bac5249db47f`. Protocol changes use the separate
  `codex/l1-history-offline-proving` branch in this worktree's submodule clone.
- Original untracked proposal preserved byte-for-byte before worktree creation:
  SHA-256 `d0d7abb41f9dfc10081351338d0f030d6d0188fac74663b5dbba7c6d0d15de55`,
  45,039 bytes. Its old worktree remains untouched. This implementation copy
  removes existing-network activation, migration and fallback requirements.
- Original shared checkout is dirty on `feat/l2-fee-recovery`,
  `fa916b13254a36423cad130080dde367bb18a7d6`; no files there were edited.
- SSH fetch failed with `Permission denied (publickey)`; HTTPS fetch succeeded
  without changing remote configuration. Recursive pinned submodule initialization succeeded.
- Available tools: Rust 1.95.0, Foundry 1.7.1, protobuf compiler 3.21.12.

## Requirement and integration checklist

These are planned acceptance checks until a result is explicitly recorded.

| ID | Reuse and implementation path | Retired behavior | Required evidence |
| --- | --- | --- | --- |
| O1 | New history predeploy; shared storage/ABI and genesis generator | No prior L1 history exists | Every hash, never-selected records, absent and current reads |
| O2 | `eez-evm` pre-execution hook after EIP-2935 and balance cleanup | No alternate system transaction | Access, position, code, bounded gas, rollback, nonce/balance/receipts |
| O3 | Shared canonical L1 header chain and selection in `eez-protocol` | Prover provider assertions | Full linkage, exact anchor, malformed/gap/non-maximal rejection |
| O4 | Manager `getCustomData`, shared context encoder, public-input producers/consumers | Timestamp/hash-only context and zero selector | Independent Solidity/Rust vectors; identity/root/rules/mode substitutions |
| O5 | Existing stateless checkpoints and whole-group settlement validation | RPC acceptance/provenance retrieval | Exact parent and replayed accepted terminal variants; reject invented roots |
| O6 | Signer offline profile plus complete v2 evidence | Signer L1/L2 providers and RPC startup flags | Cold network-denied process, restart and missing-evidence failures |
| O7 | Shared executor and existing native DA reconstruction | Constant-seed candidate assumption | Independent follower reconstruction, including partial consumption |
| O8 | Existing session quotas, worker permits and cancellation | No weakening of lifecycle checks | Bounded parsing, exact duplicate, Rewind/Resume, cancellation and detached worker |
| O9 | Existing v2 service with typed witness field 5 and capability identity | v1 service/fallback; retain needed shared messages | Schema/conversion/accounting/cache checks and incompatible peers |
| O10 | Existing canonical reconciliation, ownership and reorg handling | Settlement-driven seed history | Selected and unselected history reorgs; stale root/anchor rejection |
| O11 | Extend genesis schedule with explicit fixed update offset | Arrival-time or accepted-live-anchor timing | q=0/intermediate/k-1, repeats, missed slots and integer bounds |
| O12 | Same checked source sets imports and EVM/header RANDAO | Descendant seed refresh after settlement | Opcode/header/stateful/stateless agreement across Sync |
| O13 | Existing range scheduling, admission, blobs and reconciliation | No receipt-as-completion or cache-required shortcut | Four-day multipart process exercise, cold prover and independent replay |
| O14 | Checked actual-parent/historical modes; separate attempt parent | Latest-only signer admission | Time/action/age 1/256/257/rule boundary and reuse tests |
| O15 | Ordinary contract verifier against stored header hash and trie roots | No new opcode or view execution framework | Value and canonical absence, exact block/address/slot, malformed proofs |

## Implementation sequence

1. Shared schedule/header rules, history contract/storage and genesis profile.
2. Shared stateful/stateless import and RANDAO execution; source acquisition and follower derivation.
3. Both on-chain contexts and exact shared public inputs; deployment configuration.
4. Offline v2 witness/session/finalization/client integration; remove superseded v1/providers/seed paths.
5. Continuous catch-up, recovery, application proof path and integrated process regressions.
6. Fresh review against O1–O15, owning specifications and full applicable validation.

## Findings and unresolved validation

At the baseline, the seed was refreshed by accepted live settlements and was constant
across a candidate. The implementation replaces it in production, pending RPC,
reconstruction and replay together. Core root pins are not public-input commitments: the manager must
read and bind the actual registry root. Historical mode must continue to reject static
and reverted observations even when they produce no L2 transaction.

### Foundation evidence (2026-10-22; integration still in progress)

- `cargo +1.95.0 test --locked -p eez-protocol --lib l1_history::`:
  12 passed, including canonical ancestry, delay, maximality and independent flat
  ABI context vectors. Initial compilation failures (missing API docs and Alloy's
  `uint8` encoding API) were fixed; retained in local validation logs.
- `forge test --match-contract L1BlockhashContractTest` in the isolated protocol:
  7 passed. Covers append-only records, absence, caller/timing gates, duplicates,
  empty imports, bounds and atomic failure. An initial address-checksum compile
  error was corrected.
- `forge test --root contracts --match-contract OfflineRollupTest`: 8 passed
  before canonical-runtime generation. The first run had one test-fixture error:
  Foundry's nonzero default block hash needed explicit clearing. The failed trace
  is retained; this was not a retry-only pass or a relaxed assertion.
- `bash scripts/update-eezl2-genesis.sh`: passed with Rust 1.95.0; generated
  history runtime, shared code hash and bootstrap allocations. Base state root
  `0xfe1ff3f82c1f2b42fb8fdb1d72d5698282e55e609075fa7f572af8b0d2798817`.
  The first owned helper build was interrupted after finding that the script
  selected host-stable Rust. The fallback command now explicitly pins 1.95.0.
- `cargo +1.95.0 test --offline -p eez-evm --lib`: 24 passed, zero skipped.
  Includes actual mandatory EVM imports, opcode/history agreement across Sync,
  never-selected headers, zero receipt/transaction-gas impact, preserved native
  nonce and existing balance cleanup, conflicting-write rollback and rejection
  of wrong RANDAO/runtime/storage/missing evidence. Existing EVM regressions pass.
  `--offline` resolved newly declared direct test dependencies already present in
  the lockfile; only dependency-list edges changed, no versions or sources.
  Subsequent commands use `--locked`. The first focused check's pinned-API compile
  diagnostics are retained separately from this passing run.
- `bash -n scripts/update-eezl2-genesis.sh`: passed.

Logs are local-only under this worktree's ignored `artifacts/validation/`; they
are not available from another checkout. These are unit/contract and generation
results, not process, network-denied prover, DA, settlement or independent
follower evidence. Full applicable checks, the four-day exercise, independent
replay, protocol gitlink finalization and final correctness review remain outstanding.
No public deployment, service lifecycle or operator-state operation has been performed.


### Runtime integration evidence (2026-10-22; not completion)

- Shared protocol suite: 202 passed; RPC identity tests: 6 passed; EVM suite:
  26 passed. Later changes still require rerunning the affected suites.
- Manager registration/profile suite: 9 passed. Application L1 storage verifier:
  8 passed (`offline-storage-proof-tests.log`), covering values, authenticated
  absence, exact fork layouts, address/slot binding, compact paths, inline
  children, malformed/substituted proofs and bounds. Independent real EIP-1186
  vectors and process coverage remain outstanding.
- Focused L1 derivation suite: 13 passed (`offline-canonical-history-tests.log`),
  including late/missing data, ancestry gaps, fork layouts, reorgs with unchanged
  RANDAO and no removed settlement, bounded source caching and acceptance
  without seed mutation.
- Production node/follower/signer checks passed after runtime acquisition,
  durable evidence, final anchor bridges, streaming rewinds and prebuilt block
  origin fencing (`offline-origin-fencing.log`). Subsequent follower inclusion
  checks and manual v2 test integration compile (`offline-process-check.log`).
- The process testkit now deploys the profile-bound manager and starts an
  offline signer. Its proxy forwards v2 identity metadata and captures bounded
  ACKed prefixes for cold replay. The independent manual native writer now
  proves historical mode through v2 with complete evidence.
- `python3 -m unittest discover -s scripts/ci/tests -p 'test_deploy_preflight.py'
  -v`: 10 passed, mocked preflight only, no broadcast. Covers delay configuration,
  profile/hash/render failure, bootstrap timing and existing output preservation.
  `bash -n scripts/deploy.sh` passed. Integration Solidity build passed; lint
  warnings remain to review.
- Initial process compilation exposed old v1 manual-writer and launch APIs;
  those callers were ported. A subsequent native smoke build was killed by
  SIGKILL while compiling `eez-node`, before tests ran
  (`offline-native-smoke-initial.log`). The shared host had about 9.5 GiB
  available and exhausted swap. Memory pressure is a plausible cause, not an
  established OOM diagnosis. A one-job retry is in progress.
- Signer unit compilation still exposes retired v1/cache/provider fixtures
  (`offline-signer-tests-initial.log`); these need migration to v2 and the fresh
  profile. Production compilation is not validation of these tests.

Outstanding: native settlement/follower convergence, cold network-denied proving,
fresh fixture generation, multipart four-day catch-up, partial-consumption,
replacement/restart/reorg regressions, application proof integration, obsolete
schema/config/docs cleanup, full checks and final independent review. No
end-to-end success is claimed yet. Logs above are local-only ignored artifacts.

### Native failures and profile-encoding correction (2026-10-22)

- The serial native smoke first failed at signer startup because the testkit
  still passed `--rollup-manager-address`; the supported flag is
  `--manager-address`. The launcher was corrected; the failure is retained in
  `offline-native-smoke-serial.log`.
- The next smoke reached four real v2 signatures, but all four posting
  simulations reverted. No settlement was accepted. Its capture proxy also
  exhausted its eight-session bound because it did not release detached
  sessions. The proxy now fences attachments by epoch and evicts only detached
  sessions. Log: `offline-native-smoke-launcher-fix.log`.
- A deployment-level Rust/Solidity comparison then reproduced unequal offline
  profile commitments before executing the scenario
  (`offline-native-smoke-context-check.log`). Rust encoded the dynamic outer
  tuple as a single ABI value, adding an unwanted leading offset. Solidity
  encodes five arguments. The new independent word-layout regression failed
  before the fix (`offline-profile-offset-reproduced.log`, one selected test)
  and the five context tests pass with `abi_encode_params`
  (`offline-profile-offset-fixed.log`). An earlier mistyped exact filter selected
  zero tests (`offline-profile-offset-before.log`) and supplies no coverage.
  A fresh native run is pending; earlier signatures are not positive settlement
  fixtures.
- `forge test --root contracts --match-contract L1StorageProofEip1186Test`
  passed two tests using actual EIP-1186 proofs captured from an owned Anvil.
  The retained fixture binds block, address, present slot, absent slot and
  absent account; it exercises three-node account/presence proofs and a
  two-node absence proof. This supplements handcrafted trie tests. Fixture:
  `contracts/test/fixtures/l1-storage-eip1186.json`; log:
  `offline-storage-eip1186-captured.log`. The initial missing-fixture failure
  remains in its separate log.
- `python3 -m unittest discover -s scripts/ci/tests -p
  'test_offline_signer_network.py' -v`: two tests passed on Linux x86_64.
  The test-only launcher allows listening/answering TCP requests, denies new
  connections, datagram/raw/local sockets, TCP Fast Open and io_uring, and
  closes inherited descriptors before exec. Tests include a real accepted
  TCP exchange. The aarch64 filter has not been executed. Filter success does
  not yet demonstrate a cold proof under the filter; that process test is
  pending.

Signer unit fixtures still require migration through the actual v2 validation
path. A proposed fixture shortcut around that path was rejected by automatic
approval review and was not applied; attestation tests must retain their full
authorization gates. No test failures are treated as successful coverage.

The corrected source-built native smoke passed (one selected test) with
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1` and all prebuilt overrides unset. It checks
L1-to-L2 value 41, L2-to-L1 value 43, successful source transactions, canonical
registry/safe-L2 hash agreement, the actual posted signature over independently
recomputed public inputs, and absence of poison/nonce-chain eviction. The deployed
manager/Rust profile, rule ID and actual-parent context comparisons also pass.
Log: `offline-native-smoke-profile-fixed.log`; captured complete v2 request:
`artifacts/offline-fixtures/smoke-profile-fixed/`. This is actual-parent process
coverage under the outbound-network filter, not historical-mode or fresh-follower
coverage. Setup took 404 seconds, mostly source compilation/waiting; this is not
a throughput measurement.

The next signer unit compile still failed with 73 migration errors
(`offline-signer-tests-migration.log`), primarily obsolete v1/cache APIs and old
fixture inputs. Subsequent fixes are not validated by that failed compile.

### Retired test contracts

`service/tests/preparation.rs` tested the removed PrepareOrdinary endpoint and
atomic prepared-cache/lease publication. Its header/upload/EOF deadlines, lease
publication and separate preparation slot no longer exist. That module is
removed with the endpoint, rather than preserving a test-only v1 service.
The still-required worker/deadline/cancellation invariants remain exercised by
the v2 controlled tests in `service/sessions/tests.rs`: queued-work quotas,
timed-out Block permit retention, Resume/Rewind/Cancel fencing, output
backpressure and shutdown. Finalize deadline/retry and invalid-witness/duplicate
vectors remain required. Their migration and execution are pending; this mapping
does not claim those tests currently pass.

### Historical-mode native posting and public follower replay

The source-built `node_e2e-969bf1dbfc9e03d4` executable passed
`derivable_operations::ethereum_native_v0_post_replays_from_blobs_without_sequencer_headers`
with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1` and every prebuilt-binary override unset.
The test selected exactly one case and exited successfully. Its source binaries
are recorded in `artifacts/validation/offline-native-source-binaries.sha256`;
output is retained locally in
`artifacts/validation/offline-native-historical-replay.log`.

The historical request replayed L2 blocks 1–1180, used explicit L1 anchor 6,
seven produced L1 headers and one physical blob. The request accounted for
6,858,783 witness/block bytes and 42,539 witness items. The signer digest and
recovered address matched the independent writer, L1 accepted the post, and a
fresh follower reconstructed every compared L2 block hash from L1/DA/genesis,
without sequencer headers. This is a semantic process test, not a four-day or
multipart result. The long setup duration (1184.806 seconds) was dominated by
native dependency compilation; it is not a throughput measurement. Missed L1
slots during compilation produced no fabricated headers.

### Test migration boundaries

The v1 preparation/lease/cache protocol is intentionally retired. Its tests in
`service/tests/preparation.rs`, the preparation/cache cases in `derivation.rs`,
and the preparation-terminal case in `derivation/captured.rs` are removed with
that API. V2 identity rejection, duplicate admission, prefix retention,
Resume/Rewind fencing, cancellation, quota and deadline tests own the surviving
requirements. The provider-snapshot test in `derivation/l1.rs` is superseded by
cold native v2 proving under the outbound-network-denying launcher, conditional
accepted-root rejection, and deployed-manager context/root checks; no provider
is constructed by the new signer.

The old stub-based attestation cases are retained as explicit pure settlement
checks in `service/tests/settlement_checks.rs`. They run the real backend-output,
effect, DA and digest gates but never create signing authority or invoke an
attester. Complete native v2 transcript tests cover signing after ancestry,
schedule, parent and anchor checks. This separation avoids a test-only authority
bypass. Retired v1 ingestion/header/EOF tests are replaced at the v2 session
boundary; shared worker fairness, cancellation and service-limit checks remain.
The v1 synthetic deadline-before-settlement helper is replaced by v2
`finalize_deadline_preserves_prefix_and_allows_retry_in_the_same_epoch` and the
controlled worker deadline/permit tests. These test migrations are still being
compiled and exercised; their presence is not a passing result.

The successful network-denied bidirectional request is preserved as
`crates/eez-proof-signer/tests/fixtures/native-offline-bidirectional`, including
its exact v2 protobuf bytes, digest, signature, local genesis documents and
public deployment identities. New tests replay it cold, remove L1 evidence,
and substitute an unrelated accepted commitment. Their results remain pending.

### Native regression captures and v2 cleanup

All three source-built native runs below used
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`; the standalone signer ran behind the
outbound-denying seccomp launcher. Node, Composer and follower still used their
owned private L1 services.

- `cross_chain::minimal_bidirectional_cross_chain_smoke`: passed, 411.45 s
  including source compilation. Log: `artifacts/validation/offline-native-smoke-profile-fixed.log`.
- `cross_chain::outbound_changing_static_reads_and_write_settle_and_replay`:
  passed, 140.26 s. Log: `artifacts/validation/offline-native-changing-static.log`.
- `cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`:
  passed, 64.92 s. Log: `artifacts/validation/offline-native-caught-revert.log`.

The latter two include independent follower replay. New frozen v2 requests,
responses and complete private L1/L2 profiles are retained under the signer's
`native-offline-bidirectional`, `native-osaka-changing-static` and
`native-osaka-lookup` fixture directories. Their byte counts and Keccak digests
are asserted before replay. These are short native exercises, not four-day
multipart acceptance evidence. Runtime/test binary hashes for these runs are in
`artifacts/validation/offline-native-source-binaries.sha256` (local-only).

After migrating the fixture setup, the selected signer execution/settlement
suite passed 110 tests, with one explicit fixture-inventory utility ignored:

```bash
cargo +1.95.0 test --locked -j1 -p eez-proof-signer -- \
  service::tests:: validate::stateless::tests --test-threads=1
```

Log: `artifacts/validation/offline-signer-no-legacy-wire.log`. This includes all
32 stateless tests, native cold signature reproduction, omitted L1 evidence,
forged accepted-root and observation rejection, transaction/receipt/nonce
oracles and checkpoint consistency. Earlier fixture failures remain in
`offline-signer-integrated-fixtures.log` and
`offline-signer-integrated-fixtures-slot-fixed.log`: the synthetic posting target
must occupy its configured L1 slot phase. No authorization check was relaxed.

The old preparation/cache/lease tests and messages were removed with their
retired service. Pure settlement regressions now use test-only case data and
never construct signing authority. Their corresponding actual signing tests
send v2 frames through the full service. The production capability service keeps
its existing shared package name, but only `prove.v2.Prover.ProveStream` proves.
Shared failure messages retain candidate attribution; the obsolete cache-miss
variant is removed.

The client transport tests now use the existing v2 session model instead of the
v1 proving stub. Preparation grouping/lease/reference-cache assertions were
retired with those APIs. V2 quota/recovery, complete evidence, terminal replacement,
partial-exchange cancellation and cold backfill cover the current behavior.

```bash
cargo +1.95.0 test --locked -j1 -p eez-prover-client -p eez-control-rpc --lib
```

Passed: 33 client tests and 6 RPC tests. Log:
`artifacts/validation/offline-client-evidence-lifecycle.log`. Intermediate migration
failures are retained in the `offline-client-*.log` files. The tests include the
closed retry-status list, request-local candidate attribution, signature/digest
binding, exact nine-blob transport, capability agreement, both ancestry fields,
missing/disconnected L1 evidence, changed terminal bridge with the same L2 hash,
volatile-session loss and cancellation releasing the original witness owner.

The old 29 frozen streaming conformance recordings are **not yet updated** to
the fresh genesis/profile. The combined signer run reported 124 passed, 27 failed
and one ignored utility; the failures belong to those recordings and their
deadline fixture. These failures remain acceptance gaps, not waived coverage.
The full workspace, independent final review and four-day multipart exercise
remain pending at this checkpoint.

### Complete signer and pending-RPC suites; remaining fixture integration

- The updated native-v2 conformance generator produced 29 frozen recordings.
  All 326 selected signer unit tests passed; two explicitly ignored fixture
  utilities were not counted as behavior coverage. This includes the full
  conformance replay, malformed evidence, cold authorization and lifecycle
  tests. Logs: `offline-vectors-regenerate.log` and
  `offline-signer-full-unit-new-vectors.log`. Source-built executable:
  `target/debug/deps/eez_proof_signer-e794df32abb5febd`.
- `eez-node-common` with `l1-startup`: all 28 tests passed, including delayed
  pending RANDAO, unavailable evidence and reorg reconstruction
  (`offline-node-common-full-unit.log`).
- The first full Composer run had 378 passes and 24 failures. Seventeen local
  execution fixtures still supplied old genesis/history state and are being
  migrated. Seven tracing failures were caused by the shared `/tmp` returning
  `EDQUOT` (errno 122, Disk quota exceeded) on an independently reproduced
  write/flush/fsync, despite filesystem free space. A focused test passed with
  `TMPDIR` pointing to this worktree's owned artifact directory. The original
  failure is retained; no tracing assertions or production tracing code were
  changed. All subsequent test commands must export this `TMPDIR`.
- The native test fixture now separates owned network setup from application
  deployment. This allows historical settlement before applications wait for
  the present L2 timestamp. The controlled L1 driver can produce every slot
  through the real payload builder, and the new multipart exercise compares
  hashes, state roots and ordinary-contract history/opcode observations using
  an independent follower. Its short and four-day runs are still pending.

### Contract and tooling validation

- `forge test --root contracts`: 108 passed, zero failed/skipped across 14
  suites (`offline-contracts-full.log`). Includes both manager modes and the
  independent EIP-1186 storage proofs. Subsequent formatting changes require
  rebuilding affected contract artifacts before process tests.
- Core suite with the two documented exclusions and `script/*` excluded:
  785 passed, zero failed, three pre-existing live fork tests skipped
  (`testForkL1UpgradeAndRepeat`, `testForkL2UpgradeAndRepeat`,
  `testForkRejectsWrongSigningKey`). These skips are not coverage.
  Log: `offline-core-full.log`.
- All 19 Chiado configuration tests pass after replacing the retired signer
  header-RPC forwarding assertion with local-profile/no-chain-RPC assertions.
  The settlement-policy config checker passes for Chiado, Kurtosis and
  Ethereum. Logs: `offline-chiado-config-clean.log` and
  `offline-settlement-policy-config.log`.
- The broader Python tooling run selected 395 tests but ended with 41 failures,
  17 errors and 8 opt-in skips. Four errors were the obsolete Compose assertion
  fixed above; numerous other cases invoke scripts with hard-coded `/tmp`
  paths and hit EDQUOT despite the test runner's owned TMPDIR. No expected
  result was weakened. A private unprivileged mount namespace is unavailable
  (`uid_map: Operation not permitted`); sudo requires interactive credentials.
  These affected checks remain unverified on this host until a safe temporary
  directory workaround or environment change is available.
- Conformance regeneration was byte-identical for all 29 files on a second
  explicit run. Manifest verification passes. The current corpus has 1,149
  requests, 1,148 replies, one transport error, one RPC admission error and
  24 proofs (2,376 total events). Old v1 fixture artifacts and uncompiled
  preparation-cache tests without remaining callers were removed; active
  malformed/old-input rejection fixtures remain.

### Protocol finalization and isolated tooling rerun

- Protocol source is committed locally as
  `b5d94eafab15eb3fdd08a73ad9a693f7a3fcb820` on the isolated protocol branch.
  The commit contains the history runtime/tests, derived-manager hooks,
  registry read interface and their specifications. It has not been pushed.
- Repeated core suite after formatting: 785 passed, zero failed, the same
  three live-fork skips and two documented exclusions (`offline-core-final.log`).
- `BASE_PORT=28545 BASE_CHAIN_ID=61337 bash scripts/ci/run-protocol-anvil.sh`:
  all 64 scenarios passed, zero failed/timed out/not run. The port range was
  checked unused first. Log: `offline-protocol-anvil.log`; per-scenario logs
  and results: `eez-core-protocol/tmp/e2e-ci-ZgzylU9j/` (local-only).
- All 395 Python tooling tests completed with zero failures/errors and eight
  explicit skips (`offline-ci-tooling-owned-temp-final.log`). Skips cover real
  Kurtosis resources, real artifact relocation, Docker runtime images and
  unavailable Gitleaks; they are not live/container/security-scan coverage.
  The test-only `artifacts/validation/tmp-tools/mktemp` redirects explicit
  `/tmp/` templates to the exported owned TMPDIR and executes `/usr/bin/mktemp`.
  Its interpreter is absolute so tests that mock `python3` cannot intercept it.
  An earlier wrapper attempt hit that interception in three CI-resource tests;
  those tests passed after fixing the wrapper, then the entire suite passed.
  Repository scripts and expected results were unchanged for this workaround.
- Rust formatting, edited shell syntax, conformance manifest and both Git
  whitespace checks passed at this checkpoint. A first protocol fmt command
  used incorrect relative file arguments; the corrected full paths passed.

The short multipart exercise compiled and reached a real network-denied signer.
Omitting the first block's L1 evidence correctly produced FailedPrecondition;
its diagnostic log identified `missing L1 source at cutoff`, with no signature.
The test initially expected this internal reason in the deliberately opaque RPC
message and failed. The corrected assertion checks the stable generic response
and the specific signer diagnostic separately. This failure is retained in
`offline-multipart-short.log`; the corrected integrated run is pending. No
settlement or four-day completion is inferred from this negative result.

The active operator and settlement specifications now distinguish the two
anchor modes and require fresh-network deployment. Contradictory old signer
RPC/seed instructions were removed. The older streaming proposal points to its
historical audit and current specifications instead of prescribing retired v1,
header-provider or preparation-lease behavior.

### Session predecessor regression

The full workspace run's `real_signer_attester_mismatch_never_submits_a_batch`
failed after two real signed proofs and two correct Composer signature rejections.
An unchanged focused rerun reproduced the failure in 38.30 seconds
(`offline-attester-mismatch-diagnostic.log`). Temporary proxy diagnostics then
identified the additional signer response: `Finalize range is outside the
session's validated active prefix` (`offline-attester-stream-diagnostic.log`).
The diagnostic logging was removed afterward.

After a proof, background prevalidation had opened a new session anchored later
than the still-unsettled candidate's predecessor. The client attempted Finalize
in that session before its existing cold-backfill recovery succeeded. It now
tracks the oldest retained predecessor from Begin and successful subrange proofs,
and starts a complete session before requesting an earlier range. Signature,
public-input, canonical-progress and rejection-counter assertions are unchanged.

The new client regression failed at the intended unexpected-Finalize assertion
before the fix (`offline-session-predecessor-red.log`), then the full client
suite passed: 34 passed, zero failed/ignored
(`offline-session-predecessor-fixed.log`). It checks complete retransmission,
including every block hash. The existing promoted-anchor rollback test now
expects one fewer redundant terminal transmission; all three successful proofs
and both session creations remain required. The rebuilt node then passed the unchanged exact native test in 42.62 seconds
(`offline-attester-mismatch-fixed.log`): two real signer proofs, no signer
rejections, no posted batch, unchanged registry commitment and source nonce,
and no source receipt. The source node rebuild passed in 15m31s; its SHA-256
is `e652b2d68f1cddc2116e688c0be66138c61fab95a6b7d2ceacf5914a5e1facc7`.
The running long fixture retains its earlier node inode; its manual cold-proof
path does not exercise this reused-session repair. Workspace Clippy passed after this fix in 19.78 seconds
(`offline-clippy-session-predecessor.log`), as did formatting. The combined
short fixture passed in 88.52 seconds (`offline-multipart-short-current.log`).
It reproduced all 120 block hashes, state roots and RANDAO values in a fresh
follower and compared all 25 imported L1 hashes. Its final L2 hash is
`ae164e22efc7d5cfedc7bafda5695384bce5074df66bf5029dbeb70a7f7a1f8e`,
with state root `1d7f43de2e056cae7f1b7b7a9bc5c092c6957677655e6df35dfb7cf9f3da0912`.

### Interrupted workspace command and explicit continuation (2026-10-23)

The full workspace command ended with status 143 during
`dynamic_admission::consecutive_skipped_slots_keep_the_full_group_and_reach_the_next_block`.
Its Cargo/test-runner processes exited and its owned node logs end near
00:19:04 UTC. No test assertion or Rust panic identifies the termination cause.
This is not counted as a passed suite. The original log, process logs and
available datadirs remain retained. The earlier two failures were separately
reproduced, fixed, and passed their unchanged native assertions as recorded above.

`artifacts/validation/offline-workspace-termination.json` records the original
log digest and exact 175-test inventory: 58 passed, two failed, four ignored,
and 111 without completed outcomes. Those 111 names are partitioned into five
explicit filter groups (25/25/25/25/11), with no overlap or omitted inventory
entry. Each group uses the current source-built native executable, `--exact`,
serial execution, chain-network-denied signers, and unmodified assertions.
The interrupted case is included again. Results will be combined with the
completed first-run cases and the recorded post-fix regressions; ignored tests
remain explicit exclusions, not coverage.

The separate Cargo command
`cargo +1.95.0 test --locked --workspace --exclude eez-node-e2e --no-fail-fast -- --test-threads=1`
completes workspace unit, integration and documentation targets independently
of those native groups. That non-process continuation subsequently passed all
1,560 selected tests; the native continuation remains in progress. The four-day
run is a separate owned process and remained alive through this interruption.

The resource sampler also returned SIGTERM/143 after its last sample at
00:28:39 UTC. Its 267 observations remain preserved in
`offline-four-day-two-mode-resource-observations-before-restart.json` and the
continued observation file. Sampling resumed with an explicit interruption
record; the gap and initial delayed start prevent any claim of an exact peak.
The long test itself remained alive and had captured beyond block 50,000.
The termination source for the sampler is also unestablished.

### Retiring the divergent Anvil rollup fixture (2026-10-23)

The first native continuation group completed with 13 passes and 12 failures.
Nine failures belong to the admission/root-pin issue above. The other three
(`e2e::anvil_reorg_preserves_wall_clock`, `e2e::failure_wrong_rollup_id`, and
`e2e::node_fixture_ignores_parent_dotenv`) stopped in setup with
`Anvil genesis differs from its local offline profile`, before their intended
assertions. The log and resource record remain in
`artifacts/validation/offline-native-remaining-1.*`.

An owned diagnostic compared the actual Anvil block-zero header against the
same locally computed genesis. Anvil reported the empty-trie state root
`56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421`
for the populated allocation. The independent genesis helper computed
`e239a3e0978421cabc365a6c5cffd07ad2f0aca3b646ad6ff30bb64f067fa5f1`.
Disabling default accounts and the default CREATE2 deployer did not correct
that discrepancy. Raw headers, commands and results are retained in
`artifacts/validation/anvil-genesis-diagnostic/`. No production genesis check
or hash comparison was relaxed.

The four remaining consumers of the old `Harness` now use the existing
canonical native fixture, except the Anvil clock test, which needs only a
standalone Anvil. The wrong-rollup case retains its closed readiness gate,
zero sequencing/proofs/posts and unchanged registration commitment checks.
The interval-policy case retains ordinary execution and all not-due/no-post
assertions. The poisoned-parent-environment case exercises the shared node
launcher with a keyless follower and requires receipt/block-hash agreement
after public replay; it no longer starts a Composer against a divergent
Anvil source. The clock case still checks replacement height/hash and
strictly increasing wall-clock timestamps, with a real transfer in the
replaced suffix. The unused Anvil rollup deployment and signer helpers were
removed after tracing their callers; shared native deployment and Anvil
reorg utilities remain.

Workspace Clippy and the process-test build pass after this fixture cleanup
(`offline-native-fixture-clippy-fixed.log`,
`offline-native-fixture-e2e-build.log`). The first Clippy attempt caught a
needless borrow, corrected without changing behavior; its failure is retained.
The first four-case run passed the environment-isolation and interval-policy
cases, and exposed two further fixture issues. A wrong-rollup node paired with
the original rollup's signer correctly failed the new capability identity
check before reaching registry readiness. The fixture now gives both processes
the same requested rollup ID while still registering only the real rollup on
L1. Anvil's `evm_setTime` also allowed an immediately mined successor to share
its predecessor's timestamp. Clock restoration now uses the greater of wall
time and predecessor timestamp plus one second, with checked arithmetic. The
strict timestamp assertion remains unchanged. The failed run remains in
`offline-native-fixtures-fixed.log`.

All four cases then passed in 70.16 seconds
(`offline-native-fixtures-gates-fixed.log`). Workspace Clippy and the native
test build passed after those corrections. The final test executable SHA-256
is `f373243eca85fa28c36357c03fe72807ab8066a1d57032335274d061a734e7f2`;
the production node remains
`e21fdc458eec9b256edfba4034c356cec931c1dbe26d1c44c62d44b052939e86`.

All 28 admission cases passed separately. Its continuation queue checks
executable hashes between groups, so rebuilding the test executable for this
fixture correction intentionally stopped that queue after its successful
admission group. The remaining groups restarted with the new executable
identity; the already-passed interval case is removed from that continuation
selection to avoid counting it twice.

### Executed-state reads and pending-import coherence (2026-10-23)

The second continuation group exposed a failure in
`fee_quotes::compact_nested_legacy_estimates_both_types_settles_and_replays`:
its post-receipt `completedProxyCalls()` read returned
`canonical L1 has not resolved source cutoff`. The shared testkit
`read_state_word` used Alloy 2.5.0's implicit `pending` call default. Its callers
assert executed effects, so it now explicitly reads `latest`. The production
missing-evidence check remains intact. The first failure, node/signer logs and
datadirs are retained under `offline-native-remaining-2-final.log`; post-fix
process validation is pending.

Tracing that call through pinned Reth `3d592ece` also found a separate RPC
consistency gap. `LoadState::evm_env_at` uses the next environment with the
parent's state when no executed pending payload exists. An environment builder
cannot run the mandatory import in that state. Returning a new RANDAO at a
scheduled import could therefore disagree with the old history contract.
The fallback now rejects positions whose shared schedule changes cutoff,
including repeated selections. Executed pending payloads and reads of an
executed block retain their normal paths. This is an explicit restriction of
the current RPC integration, documented in the proposal and README; it is not
a change to production/replay schedule rules or the v2 prover service.

The focused complete-evidence regression failed before the fix by returning
successful attributes at timestamp 1007 with the new source's RANDAO
(`offline-pending-import-red.log`, one intended test selected). After the fix,
all 29 node-common tests pass, including missing evidence, repeated-source
imports, readiness, Sync inheritance and reorg reconstruction
(`offline-pending-import-node-common.log`). Workspace Clippy passes
(`offline-pending-import-clippy.log`). Runtime and process validation remain
pending. This RPC-only change does not affect the running long fixture's
explicit executed-block reads or its execution/proof rules.


### Pending RPC process evidence and independent native oracle (2026-10-23)

The current node and follower were rebuilt from source after the pending-import
restriction. The following three process cases passed together in 152.47 seconds,
with signer egress denied (`offline-pending-and-observation-process.log`):

- `e2e::pending_history_import_requires_executed_state` reaches the specific
  pending-import error with complete L1 evidence, checks the previous executed
  origin, then checks the next executed block against an independent source oracle.
- `fee_quotes::compact_nested_legacy_estimates_both_types_settles_and_replays`
  retains the quote, execution, settlement and follower assertions; its
  post-receipt observation now requests executed state explicitly.
- `derivable_operations::multipart::bounded_parts_use_cold_offline_signers_and_public_replay`
  repeats the three-part/BPO1/two-mode flow with cold denied signers, both missing
  evidence rejections, and a clean final follower.

Current executable SHA-256 values are:

| Executable | SHA-256 |
| --- | --- |
| Composer | `584def7fe127cdb8a30b447993d6fb4c14384f124a8e044ea8fb8f818d033c0d` |
| Follower | `f133c92243d8c2a3f6c14052d38505bd64b2d9382edc05dfdb5f472fe4a8eb6d` |
| Signer | `6aae8d9e32b037d5542d86b5657795a2d5e03eb61fa879ab306aa7465b369956` |
| Three-case test executable | `76295db399817cf3a824a41b3781b302585a70fb37ff4a39b924e8e22e30b282` |
| Test executable after native-oracle migration | `b88ae01ebe4fefec923ae0d5109087f34d4f2e2bf529b1b3423811dbed477421` |

The second native continuation group finished with 22 passed, two failed and
one ignored in 1,912.90 seconds (`offline-native-remaining-2-final.log`). Its
second failure was the native-system scenario's retired rule that randomness
changes only after settlement. All earlier native execution, accounting and
replay checks had passed before that assertion. The replacement oracle selects
the latest L1 header at each fixed cutoff without using the shared selector,
checks PREVRANDAO and `current()` on all four nodes at explicit L2 block
positions, and checks the entire imported number-to-hash interval on each node.
All native transaction, receipt, balance, nonce, fee and RPC-rejection checks
remain. The migrated scenario passed in 59.24 seconds
(`offline-native-oracle-process.log`).

The second continuation group ran across the pending-RPC runtime rebuild. Its
loaded test executable was unchanged, but child processes launched at different
times could use the previous or current node/follower. Its start-of-group hash
record does not establish a single runtime identity for all 25 cases. Retain
those results as diagnostics; the final stable-runtime native run must cover
this group again. No later executable rebuild is planned while the current
remaining groups run. The native inventory now has 176 entries, including the
new pending-import process regression; default ignored/live tests remain explicit.


### Further native integration failures (2026-10-23)

The third continuation group reached three distinct failures. Its unchanged
runtime/test identities are in `offline-native-oracle-executables.json`; the
first failures remain in `offline-native-remaining-3-current.log`.

- Same-block partial consumption expected an empty root-pin list and identical
  proof reuse after root advancement. Those assertions conflict with the
  authorized accepted-root commitment. The adapted case obtains a second real
  v2 proof from the full captured execution/DA, changing only the expected root
  to the first user's fully replayed checkpoint. It additionally submits the
  old proof with that new pin and requires a mined `InvalidProof()` revert,
  followed by valid middle-group resumption in the same L1 block. All ordering,
  nonce, partial/full-root, reconciliation and follower assertions remain.
  No signing shortcut or production validation relaxation is used. Post-change
  process validation is pending. The proposal and RPC contracts now explicitly
  distinguish checkpoint re-proving from reuse of a stale root-bound proof.
- `safe_fee_quotes::entirely_l2_safe_keeps_ordinary_gas_rules` exposed a real
  ingress regression: pending trace classification at a required history import
  failed, the front held an ordinary Safe call, and composition then evicted it
  for having no cross-chain calls. The retained node log is
  `eez-node-node-1334940-27.log`; its public fixture transaction is
  `0x33951837638712cd1db112207cae6575c768757bee6a245e86b6ada7599d767b`.
  L2 ingress now traces the coherent executed head, matching the existing local
  gas classifier. L1 ingress retains pending classification. The focused HTTP
  regression first failed at the expected routing gate
  (`offline-ingress-executed-state-red.log`); after the fix all 98 node unit
  tests passed (`offline-ingress-executed-state-node-unit.log`). It checks
  ordinary forwarding and nested/caught dispatch rejection, in both directions.
  The Safe process test is unchanged; its post-fix result remains pending.
- The delayed-settlement test expected transaction block hashes to change solely
  when a settlement became visible. The fixed history/PREVRANDAO schedule
  intentionally removes that trigger. Its replacement requires stable receipt
  block hashes through delayed canonical acceptance, preserving all twenty
  ordinary transfers and independent final replay. A separate real L1 reorg
  case replaces an imported source with no accepted settlement removed, and
  requires all twenty ordinary transactions to reenter production without
  resubmission, exceed the external per-sender capacity, settle, and replay on
  a clean follower. Both process checks remain pending. This preserves the
  original transaction-recovery requirement under the actual new invalidation
  mechanism rather than treating an obsolete seed assertion as coverage.

An intermediate Clippy invocation caught an incorrect test-only `.latest()` call
on Alloy's contract-call wrapper. Inspection of pinned `alloy-contract` 2.0.5 established
that this wrapper already selects `BlockId::Latest`; the unnecessary changes
were removed, leaving the Safe test unchanged. The subsequent workspace Clippy
check passed (`offline-partial-ingress-clippy-final.log`). This differs from
`Provider::call`, whose pending default motivated the earlier shared observation
helper fix.

### Recovery results and complete native rerun (2026-10-23)

The third continuation group completed with 22 passes and the three failures
above in 2,749.93 seconds. Its original results and executable identities remain
in `offline-native-oracle-and-remaining-results.json`. The later results do not
replace that failure record.

After the ingress fix, formatting, workspace Clippy and the native source build
passed. The four focused process cases all passed, without diagnostic retries,
in 195.62 seconds (`offline-four-recovery-results.json`). This includes the new
history-reorg case: its fork retains all fixture deployments, removes no
accepted settlement, recovers all twenty ordinary transfers without resubmission,
and verifies replacement history and receipts on a fresh follower. The
same-block partial case reaches the specific `InvalidProof()` gate for the old
root-bound signature before successfully resuming with the new proof.

The complete native inventory now has 177 entries. It is being run serially with
the normal explicit ignores and `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`, using:

```sh
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- --test-threads=1 --nocapture
```

`offline-native-final-stable-results.json` records the full inventory, ignored
selection, environment and before/after executable hashes. This replaces the
planned separate continuation groups 4–5 and also covers group 2, whose earlier
child-runtime identity was mixed. The fresh four-day exercise remains separate
because that test is explicitly ignored in the normal suite. No runtime rebuild
is planned during the complete native run.

| Executable | SHA-256 for the four focused cases and complete native rerun |
| --- | --- |
| Native test | `0b06cba3c11f9009a33de38495e7f3cc49b094f2402042461ff4021e295a747d` |
| Composer | `2ae65d2945e3cf875bf7158c137fe6f5a669a98121b0491e863f317440e00233` |
| Follower | `f133c92243d8c2a3f6c14052d38505bd64b2d9382edc05dfdb5f472fe4a8eb6d` |
| Signer | `6aae8d9e32b037d5542d86b5657795a2d5e03eb61fa879ab306aa7465b369956` |

### Retained four-day execution materials (2026-10-23)

Before its running executable inodes could disappear, the owned long fixture's
exact test, Composer and signer binaries and its two genesis files were retained
under `artifacts/validation/offline-four-day-running-materials/`. Its local-only
`manifest.json` records process IDs, inodes, sizes, paths and SHA-256 hashes.
These are debug execution materials, not CI release artifacts or published
downloads. No operator credentials or process environments were copied.

| Material | SHA-256 |
| --- | --- |
| Long test | `37562945117f99d7ce9804767fab3a86bfb7b39e06db3209ba70e9cd5f0023b7` |
| Long Composer | `4c4a07b19421dbad780a04aef79a947b60bd3114ed9c3e1c69e62fbb7f319688` |
| Signer | `6aae8d9e32b037d5542d86b5657795a2d5e03eb61fa879ab306aa7465b369956` |
| L2 genesis | `940f78a253faab51a7e5f0557d32a1303ebee557413f607736b8e1d6e4930757` |
| L1 genesis | `44ffe003750d9f2d65fec4e2f13dfb6600587d5ddb18e5915853d3eba52314ef` |

The long Composer predates the later admission, pending-RPC and ingress fixes.
Its explicit cold-proof settlement path does not exercise those consumers;
the history execution and proof rules are unchanged. Its result must therefore
be read together with the current-runtime short exercise and native regressions,
not represented as an exact-current-binary four-day deployment test. At this
checkpoint twelve parts, totaling 120,000 L2 blocks, have settled; final replay
and full four-day completion remain pending.

### Final review: known inclusion rule changes (2026-10-23)

The fresh review traced both anchor modes from selection through context
construction, independent signing and manager verification. It found that an
ordinary catch-up candidate could select the current parent as a historical
anchor even when its intended inclusion timestamp enters a new L1 rule epoch.
The manager correctly rejects that old-rule context, so the issue is a
predictably unusable proof and lost settlement opportunity, not acceptance of
an invalid proof.

Composer now selects historical mode only when the anchor and intended target
share the same configured L1 rules. At a known transition it selects
actual-parent mode before proving, retaining the exact existing attempt target.
The selector and encoded context come from that same checked choice. This is
not a fallback from a failed live attempt and does not alter proof validation.
The regression `proof_anchor_uses_inclusion_rules_at_a_known_fork_boundary`
failed with the former choice (`offline-anchor-inclusion-rules-red.log`). Its
six cases cover before/at/after the transition, live endpoints and actions,
and compare the encoded rule ID with the target's rule ID. The entire Composer
unit suite then passed: 406 passed, zero failed, one ignored profiling utility
(`offline-anchor-rules-composer-unit.log`). Workspace Clippy passed afterward.
All nine `OfflineRollupTest` contract tests also passed, including independent
inclusion-time context and registry substitution checks.

The first complete native rerun was deliberately stopped before replacing its
runtime. It had 15 passes and one interrupted case, with no failures. This is
not a completed suite. `offline-native-first-stable-stopped.json` records the
reason and owned process identities; `offline-native-first-stable-selection.json`
records its selection. All five owned processes were confirmed gone. The
four-day exercise was not stopped. Its running Composer predates this mode
selection fix as well as the previously recorded admission/RPC fixes; the long
fixture chooses its proof modes explicitly, and its shared history/execution
rules are unchanged. A full native rerun against the rebuilt runtime is still
required.

The same review corrected two documentation claims: followers require their
own configured L1 connection, and restart revalidates retained ordinary history
rather than unconditionally discarding it. Unowned cross-chain work and invalid
descendants remain subject to the existing reconciliation rules; accepted
request ownership and retry counters remain in memory. The review found no
need for signer chain access. Local link/anchor checks covered 440 references
in 33 changed Markdown files, and `bash -n` accepted all 49 shell examples
(`offline-review-documentation-check.json`). These are static checks, not
execution of operator procedures.

The rebuilt Composer and native test executable passed their source builds at
04:25 UTC. The full 177-entry native inventory then started with its five normal
explicit ignores and signer network denial, using the command above.
`offline-native-final-rules-results.json` records the selection and before/after
identities; `offline-native-final-rules.log` retains the output. Initial hashes:

| Executable | SHA-256 |
| --- | --- |
| Native test | `8d45de85f7d457f10d4da5503da226c9fba0d7af5a36e9fd4181dc10973ffee1` |
| Composer | `de887ac68330e24b800c582fc731abf5de0e83d526934356418840df023026e0` |
| Follower | `f133c92243d8c2a3f6c14052d38505bd64b2d9382edc05dfdb5f472fe4a8eb6d` |
| Signer | `6aae8d9e32b037d5542d86b5657795a2d5e03eb61fa879ab306aa7465b369956` |


### Four-day readiness timeout and current-runtime rerun (2026-10-23)

The two-mode run failed after 18,658.35 seconds, with thirteen complete parts
and 130,000 canonically settled L2 blocks. It timed out waiting for production
of block 140,000 after 1,800 seconds. Durable capture continued through block
138,762 immediately before teardown; the failure was at the production wait,
not the proof, settlement or replay assertions. Final independent replay was
not reached. The original log's SHA-256 is
`174ecd7e7be768d49cae3edcdb0a61b0d7078c2f22c7a74f8d5c412353b6ff4c`.
`offline-four-day-two-mode-failure.json` preserves each part/proof, retained
owned datadir locations, executable-material manifest and sampled resources.
The resource sampler's completion flag means the runner exited, not that the
test passed. It observed up to 6,942,552 KiB of summed owned-process RSS; shared
pages may be counted more than once, sampling can miss peaks, and the previously
recorded sampling gap remains. This is not a throughput claim.

Only the explicit long fixture's wait budgets change: ninety minutes per
production/capture wait and six hours for the complete independent follower.
The small fixture keeps its existing bounds. All 345,600 L2 blocks, 35 parts,
full L1 bridges, both anchor modes, BPO1 checks, 32 ordinary application writes,
cold denied signers and final comparisons remain mandatory. Formatting,
workspace Clippy and the source test build passed after this adjustment
(`offline-large-wait-build-results.json`). This corrects a fixture readiness
budget; it is not a diagnostic retry passed off as the original result.

The fresh run started at 04:35 UTC, directly executing the source-built test
artifact with the exact long-test filter, `--ignored --exact --test-threads=1
--nocapture`. `offline-four-day-current-results.json` records the command,
current runtime identities and final exit status; its output is
`offline-multipart-four-days-current.log`, and owned process samples are in
`offline-four-day-current-resources.json`. The new test hash is
`1ddae56ceb17e910ddf443da0a16f1dea0fef0e1986a9afce1c803cb371f7bda`.
Composer, follower and signer hashes are unchanged from the 04:25 native run.

The normal native suite continues on its already-running test executable,
retained in `offline-native-final-rules-running-test` before the long-only test
rebuild. Its identity and process start ticks are in
`offline-native-final-rules-running-test.json`. The file at Cargo's test-output
path now differs; the running process's inode and all child runtime binaries
remain unchanged. Thus a differing final test-path hash is expected and must
not be described as changing a runtime underneath that suite. The normal suite
ignores the long case; its other test behavior is unchanged.


### Revert-data observation during the native rerun (2026-10-23)

The native rerun reached another existing helper that inherited Alloy's pending
call default. `inbound_reverting_destination_preserves_caught_result_without_destination_writes`
failed before its scenario, while obtaining the destination's expected revert
bytes: the synthetic pending block required an unexecuted history import.
`offline-revert-observation-failure.json` retains the specific RPC error and
source/executable identity. This does not establish a destination rollback or
proof failure; the intended assertion had not reached the contract.

`call_revert_data` now explicitly reads the latest executed state, matching
`read_state_word`. It still fails on unexpected success, missing RPC error data
or missing revert bytes. All custom-error/string/write-then-revert, exact-byte,
source-result and destination-rollback assertions remain. Its only callers are
the two directional caught-revert tests and direct CCM-L2 authorization test.
Other direct provider calls and raw RPC observations were checked: they already
select executed blocks, except the deliberate pending-boundary regression.
Generated contract-call wrappers already select latest in pinned
`alloy-contract` 2.0.5; the provider's pending default is in `alloy-provider` 2.5.0.

This is confined to test infrastructure. Shipping runtime packages do not
depend on `eez-testkit`, and the active suite's embedded helper remains unchanged.
The suite continues to collect all results; post-fix execution of the three
affected callers remains pending. The current long fixture does not call this
helper and continues unchanged.

Formatting and workspace Clippy passed after this helper correction
(`offline-revert-helper-checks.json`). The three post-fix process checks remain
pending until the complete native run finishes; no shipping binary was rebuilt.

### Cold restart with an already accepted correlation (2026-10-23)

The complete native run exposed a production restart failure in
`ordinary_data_admission_tracks_operator_capacity_without_losing_valid_work`.
The restarted Composer restored the canonical correlation at L1 block 7 / L2
block 30, then called `publish_accepted` before preparing its in-memory L1
history. Endpoint randomness validation correctly rejected the missing evidence;
the Composer refused startup. The later
`restart_after_intermediate_prefix_discards_old_intent_and_uses_current_rules`
case reached the same failure. These are failed checks, not successful recovery.

The first failure, retained datadirs and exact runtime identities are recorded in
`offline-restart-history-first-failure.json`. Its extracted log is
`offline-restart-history-first-failure.log`, SHA-256
`2331a49be29e39908e977584e6a878cae17d52aaf0cc0e58a4a98dc60158ebc9`.
The complete native log remains intact.

The persisted-correlation branch of `reconcile_scanned_batches` now acquires and
publishes the checked canonical ancestry for that endpoint before revalidating
and publishing its accepted commitment. It uses the existing bounded acquisition
and canonicality fence under the reconciliation guard. No endpoint, randomness,
canonical-root or retry check is bypassed; the correlation database does not
replace the required L1 evidence.

Formatting, all 44 deriver unit tests and workspace Clippy passed after this
correction (`offline-restart-history-source-checks.json`). Production binaries
have not yet been rebuilt: the active native inventory is still collecting
results on its recorded pre-fix runtime. The unchanged failing process cases
and affected restart/reorg paths require validation after that run completes.
The current four-day producer also predates this boot-reconciliation correction;
its exact executable is retained in the current-run materials manifest.

The full native inventory finished at 07:10 UTC: 168 passed, four failed and
five explicitly ignored in 9,891.29 seconds. The fourth failure was
`restart_with_pruned_bodies_and_logs_settles_and_replays_from_checkpoint`, at the
same cold-history gate. Together with the observation-helper failure, these
are all failures in the completed inventory. The log SHA-256 is
`b15dd59c9459d82e7dedc5e1abf7f853223e4d7b509421aa746009569f88a1ff`;
`offline-native-final-rules-identity-review.json` records every case's outcome.
All three shipping runtime hashes stayed unchanged. The wrapper's aggregate
`executables_unchanged = false` reflects only replacement of the test executable
pathname for the separately running long fixture: the inventory continued on
the retained original test inode with SHA-256
`8d45de85f7d457f10d4da5503da226c9fba0d7af5a36e9fd4181dc10973ffee1`.
This identity distinction does not turn the four failed cases into passes.

### Observation-helper process result and corrected-runtime recovery checks

The source harness rebuild finished at 07:34 UTC in 23m45s. The three exact
callers of `call_revert_data` then all passed in 170.59 seconds, with no ignored
cases, kernel-denied signers and unchanged executables throughout the run
(`offline-revert-helper-process-results.json`). The formerly failing inbound
revert scenario retains its specific error-byte, caught-result and no-write
assertions. This resolves that failure; it does not resolve the three separate
restart failures by itself.

This Cargo test build also rebuilt the Composer and follower after the
persisted-correlation preparation correction. The helper run used:

- test harness: `25ebfcf6e841cc0a01ec6335aa38684b3bd8dcd930d4e1a4945845ff0c2823ae`;
- Composer: `e55eb2076044ed3ad9430fa02836c10ce664ee83f148f55e813643890c8c9283`;
- follower: `08a25b7455951b4c9e1e60996ef5f20d41f6680c442d2b86cdf5524c4320efb4`;
- signer: `6aae8d9e32b037d5542d86b5657795a2d5e03eb61fa879ab306aa7465b369956`.

An explicit role-binary build and eight selected recovery process cases follow
in `offline-restart-history-process-results.json`. Selection includes the three
unchanged failures, retained/capture-store restart, partial consumption and
imported-history/shared-signer reorgs. Their results remain pending.

The long fixture's misleading lookahead comment was also corrected: its proof
ranges are bounded, but historical production uses the existing per-turn work
bound and can run ahead of a proof range. Its live speculative-depth setting is
not a global bound on historical production. This changes no test behavior,
counts or assertions. The long producer remains on its retained pre-restart-fix
executable; future child identities are recorded by the resource sampler.

The explicit role build completed successfully in 9.66 seconds, followed by all
eight recovery process cases passing in 476.52 seconds (zero failed or ignored).
The runtime hashes remained fixed throughout that suite:

- Composer: `e826656e1f7c3a0da0b92e3faba82a84c0e650bded58cad64aee03d6482d5d3f`;
- follower: `a56c4ee416bbda42b3dc8a8fadc29791faedc0f50f995e27211f172f1489c2d7`;
- signer and test harness: unchanged from the helper run above.

The eight tests include all three unchanged failing cases. Five startup events
also corroborate restoration of persisted correlations at L2 blocks 30 or 45,
including the original L1 7 / L2 30 boundary, with no boot-catch-up error. The
events supplement the tests' application, settlement and replay assertions;
they do not replace those assertions. See
`offline-restart-history-path-evidence.json`. The process log SHA-256 is
`82b51ef792f4814982109509e281f8cb94e93442295caa9364b1112e82d0dcca`.
The three revert-helper callers are also being checked against these explicit
role-build hashes (`offline-revert-helper-role-profile-results.json`).

Those three helper cases subsequently passed on the explicit role build in
160.89 seconds, with zero failed/ignored and unchanged test/role/signer hashes.
All four failures in the completed native inventory now have unchanged passing
post-fix process cases. The full native command's original failed result remains
recorded; it is not relabeled as an aggregate green run. The four-day fixture
has not yet completed its required final independent replay.

### Acceptance scope clarification and focused expired-source exercise

The user clarified that four days was an example of an L1 source whose hash is
outside the 256-produced-block window. A complete 345,600-block load test is
therefore not required to validate this design. The acceptance condition remains
strict: sources unavailable as direct L1 anchors must be authenticated through
complete ancestry to recent anchors, across bounded parts and cold offline
signers, followed by independent reconstruction of the complete tested chain.

At 10:08 UTC the optional long run was intentionally stopped after sixteen
canonical settlements through L2 block 160,000. It had no new semantic failure,
but had not started its final follower, so it is partial stress evidence only.
The stop record, exact per-part proof/settlement records and teardown identities
are in `offline-four-day-current-intentionally-stopped.json`; all nine owned
test/build processes were verified stopped. The final long-log SHA-256 is
`d1009f10e13414136b4542bec5a143bac225e50ce6b78f52cb9ece700928f87d`.
The resource sampler completed with 1,327 samples and maximum summed owned RSS
of 6,801,452 KiB; sampling can miss peaks and shared pages may count twice.
An attempted release-profile build was also intentionally stopped after
538.42 seconds, before completion; it supplies no build or runtime coverage.
Its original termination status is retained in `offline-optimized-build-results.json`.

The focused regression uses 2,000 L2 blocks in four 500-block parts. Even the
newest source in the first part must be more than 256 L1 blocks old. It calls
the real manager and checks the exact `BlockHashUnavailable` error for that old
source, then retains all complete-bridge, missing-evidence, BPO1, both-mode,
32-application-write, cold denied-signer and clean-follower comparisons. The
existing 120-block short regression also runs unchanged. The larger four-day
fixture remains explicitly ignored as an optional stress test.

Formatting, workspace Clippy and the source test build passed after this test
addition (`offline-expired-source-results.json`). Both process cases then passed
in 213.61 seconds, zero failed/ignored. The focused case completed in 129.452
seconds: source 100 was already 304 blocks behind observed L1 head 404, and the
first proof authenticated its ancestry through recent anchor 405. All four
parts settled, then the clean follower compared every L2 header hash/state
root/PREVRANDAO, all 401 stored L1 hashes and all 32 ordinary application records.
The final L2 hash was
`0x6c3f13201607b00e116690b15aeb308986e0acf16ab219592d9fd0eb621e20da`;
the state root was
`0x849bcaa617dee1f6556209a4db39f6f7e4fc6c9abff24c67c7e9e08107e25c01`.
The process log SHA-256 is
`1c3099da0fd21425134fc63516e48157c67f6dc8c0742308759cee02c5855cde`.

Runtime identities stayed fixed throughout these cases: harness `fbb4bb5a...`,
Composer `e55eb207...`, follower `08a25b74...`, signer `6aae8d9e...`; their full
hashes and per-part evidence are in the portable validation record. These are
the current-source Cargo-test role builds, distinct from the explicit role-build
hashes used by the eight recovery and three revert checks above. The sampler
started midway through the suite and observed 28 samples, maximum summed RSS
3,751,064 KiB. This is a sampled lower bound on the run's peak, not complete
resource or throughput coverage.

### Final correctness and scope review

Rechecked the amended O1–O15 contract against the implementation and executed
evidence. The review traced manager/Rust context encoding, rule-period and
anchor eligibility, accepted-root/checkpoint authorization, inherited-source
maximality, exact system imports and account preservation, public follower
reconstruction, strict storage-proof absence, offline configuration consumers
and removal of obsolete proving paths. The final test addition changes no
production rule; its exact contract error and complete replay assertions reach
the intended expired-source path. No further correctness defect was found.

The original checkout remains on `feat/l2-fee-recovery` at `fa916b1325...` with
its unrelated work intact; no implementation files were written there. The
preserved source proposal still has SHA-256 `d0d7abb41f9dfc10081351338d0f030d6d0188fac74663b5dbba7c6d0d15de55`.
The isolated protocol checkout is clean at `b5d94eaf...`. Public deployment,
CI release promotion, live Kurtosis/PBS topology and unsampled resource maxima
remain outside these local results. The protocol commit is still unpublished;
another checkout needs the supplied bundle before resolving the new gitlink.
