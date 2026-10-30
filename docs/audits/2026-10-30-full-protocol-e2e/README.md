# Complete protocol E2E support: implementation record

Status: in progress. No complete-suite or signer-soundness qualification yet.

| Field | Value |
| --- | --- |
| Date | 2026-10-30 |
| Source baseline | `c702f90da19cc149890d0a17a4ef6c200dcb6a21` (fork main) |
| Protocol gitlink | `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Branch | `codex/full-protocol-e2e-20261030` |
| Scope | [Authorized proposal](../../proposals/full-protocol-e2e.md) |
| Signer review | [Interim findings and remaining closure](signer-review.md) |
| Inventory | [64 exact scenario paths and SHA-256 hashes](scenarios.json); 34 default enabled, 30 excluded by upstream comments |
| Toolchain | Rust 1.95.0; Foundry 1.7.1; Kurtosis CLI 1.20.0 |

## Isolation

The operator's original checkout and submodule edits were preserved. A new
worktree under `artifacts/full-protocol-e2e-20261030` owns this branch and its
initialized recursive protocol checkout. Existing enclaves, containers and ports
were inventoried. `kurtosis enclave ls` automatically started the absent Kurtosis
engine; it did not restart or remove an existing enclave. The intended new enclave
name is `eez-full-protocol-20261030`, unused at inventory time. No Chiado lifecycle
operation or public-network funding is authorized or performed by this work.

Local diagnostic files under this worktree's `artifacts/full-protocol` are
local-only until compact results are recorded here or an accessible artifact is
published. Do not treat those paths as retrievable evidence from another checkout.

## Baseline work

- Identified exact fork/protocol revisions and initialized the pinned submodules.
- Discovered all 64 unique scenario paths independently of the network runner.
- Inspected pinned static/reverted-call, rolling-hash, rollback and proof rules,
  then current materialization/verification restrictions.
- Baseline protocol/inspector unit tests: 239 passed, zero failed or ignored.
  These tests encode the old restrictions and do not establish full support.
- Pinned Solidity compilation passed. The first complete reference Anvil run
  attempted 64 scenarios: 57 passed and seven failed with `Address already in
  use`. Initial ports 40545–40672 collided with ephemeral host connections
  despite being free at preflight. Individual results are retained in the
  inventory. A full rerun on checked ports 22545–22672, below the host's
  32768–60999 ephemeral range, passed all 64 scenarios with zero omissions.
  This validates the pinned reference contracts' assertions, not RollupX runtime
  integration. The first run's environmental failures remain recorded.
- The documented core unit command passed 778 tests and skipped three optional
  mainnet-upgrade fork tests (`testForkL1UpgradeAndRepeat`,
  `testForkL2UpgradeAndRepeat`, `testForkRejectsWrongSigningKey`) because no
  external RPC/manager/owner inputs were supplied. It also retained the two
  AGENTS.md exclusions, `test_Doc_OriginalValue_SeedInSetup` and
  `test_PostCost_Incremental`, and the `script/*` exclusion. These are unit-test
  limits, separate from the complete, unfiltered 64-scenario Anvil run.
- The baseline network run uses CI run `40935307103`, attempt 1, whose successful
  head equals the source baseline. Its runtime payload is
  `ghcr.io/eez-association/eez-rollupx-ci-runtime@sha256:fa9ecd0365a2a035338834eb9a2242b1c4caee28c472b2096a21be4b97435d6b`.
  The repository artifact verifier checked source/protocol/lock identities,
  archive checksum and all three runtime image identities before startup.
  This run uses the new full-scenario selector with baseline runtime binaries.
  All four cross-chain waves and state-chaining checks passed. The complete
  network suite finished with 33 passes, 31 failures and zero skips; all 64
  scenarios were selected exactly once, with no missing or unexpected results.
  The 33 passing scenarios also passed independent evidence verification.
  Overall evidence/causal/semantic coverage remains false because 31 failed.
  The inventory retains per-scenario claims, log hashes and failure excerpts:
  21 fee-quote rejections, three unavailable fee quotes and seven receipt waits
  without success. Receipt waits require runtime diagnosis; they are not
  automatically distinct protocol support gaps.
  The suite exited 1, but the enclosing command ended with status 143 before
  producing its final CI result or cleaning up. The owned enclave remained
  running. Additional logs were captured, then the enclave was stopped without
  deleting its state (UUID `b6fccb8d9c7c`, confirmed `STOPPED`). No operator
  deployment was touched. The [sanitized rejection excerpt](baseline-receipt-rejection.json)
  records repeated signer rejection of conflicting static lookup results during
  the first receipt wait; later waits are not independent failure attributions.

## Mutable lookup replay under development

The shared materializer now derives a complete scratch L2 delivery for each
mutable lookup, including its source callbacks. The driver executes that table
through the authenticated manager without committing the synthetic transaction,
requires successful manager validation, and captures the target frame's own
success/revert bytes separately. Third-chain execution remains rejected.
The settlement lookup classifier now admits callback tables with unchanged
root and zero ether delta, subject to exact native semantic binding and
independent replay. This is not yet complete mutable lookup support.

Interim checks: protocol unit tests 220 passed and call-pattern tests 15 passed,
zero ignored (`candidate/lookup-table-protocol.log`); driver, Composer and signer
including test targets passed `cargo check --locked` (`candidate/lookup-table-check.log`).
Authenticated flat mutable replay and rejection of missing delivery tables,
forged rolling commitments and forged output passed (one intended test,
`candidate/lookup-table-authenticated.log`). All four history regressions passed,
including service-level assertions of zero signing calls for invalid observations
(`candidate/lookup-table-history-signer-network.log`). The initial service run
failed to bind its loopback test socket under the sandbox; the permitted rerun
passed. This is not a flaky semantic result.

The initial replay attempts exposed the reserved system sender's native-type
requirement and native gas cap; scratch replay now uses those existing rules.
An old captured fixture then failed closed on an incomplete manager storage
witness. The regression now uses the history fixture's complete account/storage
proofs and independent expected outcome. Captured witness fixtures still need
regeneration and qualification for the extended replay reads. Earlier failures
remain in `candidate/lookup-table-signer*.log`. An initial invocation incorrectly
selected `--lib` for a binary-only package and executed no tests.

The dedicated target-revert regression also passed: actual target REVERT with
exact `deadbeef` bytes is accepted only after successful manager validation;
changing the claimed target success bit rejects it. Together with the table
mutation regression, both selected tests passed with zero ignored
(`candidate/lookup-table-target-revert.log`).

The classification extension passed all 122 signer settlement tests
(`candidate/lookup-classification-signer-after.log`). The source-reverted
callback regression now verifies exact native/source-table binding and rejects
forged callback calldata. Two older applied-inbound fixtures used identical
pre/post block commitments; they now use a distinct post commitment to represent
an applied delivery. Failed top-level value projection returns zero after
validating every array; malformed arrays still reject. The preceding failing
runs remain retained in `candidate/lookup-classification-*.log`.

Multi-root transient rollback now derives nested observation ranges from bound
native action spans and replays them in a disposable overlay at one canonical
prefix. Driver/Composer/signer test targets passed `cargo check --locked`.
Protocol validation passed 221 unit tests and 15 call-pattern regressions with
zero ignored (`candidate/rollback-observation-protocol.log`). The authenticated
nested-write/read test verifies inner then outer restoration and rejects a
forged intermediate read and crossing scopes
(`candidate/rollback-observation-signer.log`, one test). Its service-level
counterpart derives the ranges from native semantics, accepts the valid proof,
and asserts zero signing calls for a forged intermediate read
(`candidate/rollback-observation-service.log`, one test). These are local
regressions, not full network qualification. The broader signer observation
selection passed 27 tests with zero failures; its one ignored item is the
explicit fixture-regeneration utility, not an executed regression
(`candidate/rollback-observation-regressions.log`). The fixture-match regression
itself passed. No ignored test is counted as coverage.

Reused mutable rows now independently rebuild and compare their complete
callback arrays; repeated failed invocations must also have identical execution
commitments. A rollback covering repeated invocations counts table positions
once when assigning its markers. The protocol run passed 222 unit tests and
15 call-pattern regressions with zero ignored
(`candidate/reused-callback-rows-checked.log`). The dedicated regression covers
both source rollback and target failure, verifies the resulting pair with the
independent inverse verifier, and rejects changed callback calldata. Its added
trace mutation also rejects a changed repeated execution commitment (one
intended test, `candidate/reused-callback-forgery.log`; unrelated filtered targets
selected zero tests and are not counted).

The six static-callback process regressions finished on source manifest
`candidate/static-callback-source.json`: four passed and two failed, zero
ignored. The passing cases include inbound/outbound static callbacks and
inbound/outbound missing-proxy creation/retry. Both negative cases reached
terminal `SimulationFailed` with the exact 36-byte missing-proxy revert, then
failed their old diagnostic-string assertion. The test now expects source
execution failure and additionally checks selector/address bytes; all receipt,
nonce, balance and state assertions remain. Those revised assertions and later
runtime changes still require process requalification. Logs/datadirs are
retained in `candidate/static-callback-process/`.
Full callback-tree EVM replay and the final signer soundness review remain
outstanding.

## Soundness review worklist (not final qualification)

The production settlement job obtains lookup claims from
`verify_native_semantics_with_observations` after DA/payload binding, then
executes `verify_observations` before returning attestation material. Neither
the scratch delivery program nor its rollback ranges are independent RPC
fields. Prefix selection uses validated block/prefix commitments, and missing
supplemental trie evidence rejects. Existing service mutations exercise rejection
before signing. This tracing is static evidence, not a completed independent
review of the final candidate.

Finding R-1, addressed locally: rollback scopes cloned the whole scratch cache,
allowing nested source snapshots to multiply cache memory without corresponding
execution work. The implementation now uses REVM's existing bundle reverts:
retain transitions in the disposable overlay, save revert-stack indices, restore
bundle state at scope ends, and clear/reload the scratch cache from that bundle.
The canonical database remains the read-only backing store. All 27 observation
regressions passed after this change; one explicit generation utility remained
ignored (`candidate/rollback-journal-regressions.log`). The added 1024-scope
shared-boundary case and final source qualification remain to run.

The static callback process run uses the earlier source snapshot: all three
runtime SHA-256 values were recorded in
`candidate/static-callback-runtime-sha256.json` after compilation completed and
before applying the journal change. Its results must not be described as
qualification of the later journal/version-8 changes.

Both validation-version constants are now 8. The three portable identity
vectors were independently RLP-encoded in Python and hashed with Foundry cast.
Native, offline-vector and history-fixture generation utilities passed. Three
independent identity tests passed, and the manifest checker verified 29 vectors.
The combined regression command passed all seven RPC tests; the signer then
reported 334 passes, two failures and four ignored generation/inventory
utilities, so later packages in that command are not counted as run.
Both failures use the old `native-osaka-lookup` capture. The diagnostic replay
fails closed on a missing storage exclusion proof required by manager-table
execution (`candidate/version8-lookup-diagnostic.log`). Preserve the successful
proof expectation and enrich the captured witness from verified execution;
never substitute an unchecked result or change that vector to expected rejection.
The original block/genesis data should be retained where possible.
 This changes the network-profile commitment
and registered vkey as documented before the update.

## Interim implementation evidence

- Full inventory selection failed against the baseline wrapper, then passed
  with the exclusions removed. All 27 network-runner tooling tests passed;
  these use mocked services and are not live semantic evidence.
- `call_patterns::static_failed_callback_preserves_revert_bytes_and_binds_success_bit`
  reproduced the baseline's unsupported failed-callback error. After the initial
  extension it and all 210 protocol unit tests pass. The regression independently
  calculates the callback commitment and rejects a forged callback success bit.
  This is table-level evidence, not a signer end-to-end soundness qualification.
- Observation replay now preserves the claimed callback's success/revert status
  and exact bytes. The witness test passed with `--features witness,derivation`
  (one intended test). The initial default-feature run selected zero tests and
  does not count; witness-only compilation exposed the fixture's dependence on
  the shipping derivation feature. Final signer rejection tests remain pending.
- Recursive static materialization now uses pending read frames. The independent
  verifier binds descendant callback commitments; the regression rejects an
  omitted descendant hash. Static source pools are partitioned as complete
  signed groups, while mutable-only host slicing is preserved. Composer, signer
  and deriver passed an interim `cargo check`; later edits still require checks.
- The new call-pattern target is included in CI's explicit release archive.
  The real tiny-workspace artifact relocation check initially failed because
  the host lacks `mold`. A linker package was downloaded and extracted only into
  task-owned storage. After also extracting its runtime dependencies, all 35
  tooling tests passed, including real archive relocation, with zero skips.
  No host packages were installed.
- One recursive test command queued behind Cargo and compiled candidate sources
  after editing. Its log was moved to candidate evidence; it is not a baseline
  failure reproduction. The earlier failed-static-callback reproduction remains
  valid baseline evidence.
- Validation identity/version and compatibility documentation must be finalized
  with the full accepted profile before candidate runtime qualification.
- Six call-pattern regressions pass, including inbound recursive observations,
  static callbacks inside mutable hosts, callback-state alternatives, forged
  commitments and excessive depth. All 210 protocol unit tests and 30 signer
  outbound authorization tests pass at this interim stage. A static pool split
  now rejects during effect binding (`EffectCountMismatch`), before outbound
  authorization; its regression asserts that earlier gate explicitly.
- The full Python tooling run selected 420 tests: one failed and eight were
  skipped. The failure was the CI preparation mock omitting the newly required
  protocol summary. After updating that fixture, all nine preparation tests
  pass, including rejection of a successful runner with incomplete, excluded,
  skipped or absent inventory evidence. The full rerun selected 421 tests and
  passed with the same eight skips. Those comprise one opt-in Docker payload test, two opt-in
  Kurtosis resource tests, three opt-in runtime Docker tests and two Gitleaks
  tests whose executable was unavailable.
- Both existing static-callback process tests passed: inbound settlement and
  outbound settlement with independent follower replay (two selected, zero
  ignored). The source snapshot and three runtime SHA-256 hashes were retained
  locally before subsequent rollback edits. These passes qualify that interim
  static implementation only; they do not qualify later changes.
- A focused stateful session regression reproduced nested rollback restoring
  state `1` instead of the original `0` when the inner checkpoint was queued
  first. Ordering pending restores by call index fixes that case. A second
  branch reproduced state `5` instead of `0` after an inner restore consumed
  the checkpoint and local execution resumed. Checkpoint restoration now
  borrows the retained snapshot. All 25 composition unit tests pass after both
  fixes. Full nested scope representation and runtime validation remain open.
- The native blob regression reproduced rejection of nested scopes sharing the
  first call. Actions now retain all such scopes, outermost first, and encoding
  preserves the exact Snapshot/Revert sequence. Materialization uses nested
  scope stacks and assigns destination markers after closing inner scopes.
  All 214 protocol unit tests passed at this stage, including exact round-trip
  and marker assertions. The prior test rejected properly nested scopes as
  overlapping; it now asserts lookup-only behavior for valid nesting, with a
  separate rejection regression for genuinely crossing scopes. Inverse
  verification, transient observation replay and full runtime checks remain
  outstanding; these unit passes do not establish complete rollback support.
- Forward source-entry scanning after rollback now passes its positive and
  omitted-entry regression. A separate regression reproduced closing a scope
  at its last descendant before the owning sibling's `NESTED_END`, producing
  the wrong rolling hash. Scope closure now checks the sibling level as well
  as the end index. All 216 protocol unit tests and eight call-pattern tests
  pass. The latest signer settlement run passed 122 tests before this final
  scope-level fix; the lookup tooling passed all 33 tests after its diagnostic
  field changed from `revert_span` to `revert_spans`.

- Subsequent call-pattern regressions cover forward reentrant scans, failed
  mutable frames at three depths, nested rollback windows with continuation,
  and static-first rollback scopes. Failed-frame hash mutation rejects at the
  reverted rolling-hash check. These are table-level checks, not service-level
  proof that rejection precedes signing.
- A failed source call followed by a static read reproduced an incorrect source
  cursor (1 instead of 0). Materialization and independent binding now restore
  the pre-lookup cursor, matching the pinned EEZL2 terminal-revert behavior.
  The resulting run passed 216 unit and 12 call-pattern tests, with zero ignored.
- Value projection separately rejected a properly nested rollback window. It
  now uses the shared array-shape check before excluding the complete outer
  window; a crossing-window mutation remains rejected. Local diagnostics are
  `candidate/nested-value-repro.log` and `candidate/nested-value-after.log`.

- Two identical failed root attempts reproduced a premature source-count
  rejection although the pinned resolver reuses one slot. The inverse walk now
  binds attempts directly and checks complete slot coverage. Where load and host
  counts are equal, their one-host-per-load boundaries are unique and retain
  the complete source group rather than guessing slot count from attempts.
- Value projection also rejected successful static rows with nonzero callback
  commitments. It now computes value independently of row-kind commitment
  checks; contextual inverse validation still rejects nonzero hashes on
  successful mutable rows. The latest protocol run passed 218 unit and 13
  call-pattern tests, zero ignored. Composer, signer and deriver compiled after
  the rollback changes; that check preceded the final value/group corrections.
  The subsequent signer settlement run passed all 122 selected tests with zero
  ignored (`candidate/rollback-signer-settlement-bin.log`). The first invocation
  used `--lib`, but this package has no library target; it selected no tests and
  is not counted as validation.

- The missing-proxy regression reproduced a fatal Composer error where the
  protocol permits catch/deploy/retry. The owning static read now returns the
  exact `StaticCallProxyNotDeployed(address)` payload and removes its speculative
  pending read/callback records. An independent selector/address encoding oracle
  checks the bytes; a second dispatch after deployment succeeds. All 219 unit
  and 13 call-pattern tests pass. Bidirectional process regressions additionally
  assert the caught error hash, created proxy, resulting application state and
  follower replay; their source-built run is pending. Source hashes are retained
  in `candidate/missing-proxy-source.json` and logs in
  `candidate/missing-proxy-process.log`. No passing process result is claimed yet.

- With the lockfile unchanged, `cargo deny --locked check advisories bans sources`
  passed all three checks. `cargo audit` exited successfully with 19 warnings
  allowed by the existing configuration; this is not a claim that the dependency
  graph has no advisories. Logs: `candidate/dependency-policy.log` and
  `candidate/dependency-audit.log`.
- Installed CI-pinned Gitleaks 8.30.1 in task-local artifacts and verified the
  workflow's SHA-256 digest. Both previously skipped secret-scanner regressions
  now pass (`candidate/secret-scan-regressions.log`, two selected, zero skipped).
  The literal `--all -m` scan found two verification-key fields in unrelated
  local-ref commits (`baffa59762e01fc221f01ef7972cde1d09267edb`,
  `b1790e02e72c9606665c99c191c01a4a1dc5ddc5`), neither reachable from this
  branch. It scanned 1,192 commits and exited 1; no allowlist was changed.
  A separate `HEAD -m` scan passed across all 802 commits in this branch's
  ancestry. Redacted diagnostics are retained. Uncommitted candidate changes
  are not covered by either history-only scan.
- Integration Solidity formatting passed; `forge test --root contracts`
  passed 153 tests across 17 suites, zero failures/skips. This includes the
  existing contract regressions; the new catch/deploy/retry behavior still
  requires its pending process tests.

- The catch/deploy/retry process run completed with one pass and one failure:
  outbound (L2-origin) passed its error hash, proxy creation, state and follower
  replay assertions; inbound timed out because ordered L1 preflight counted the
  resolver-level missing-proxy failure as a second remote call (expected 1,
  observed 2). This is a remaining implementation gap, not a flake or a passing
  inbound test. The fixed runtime source snapshot and binary hashes are in
  `candidate/missing-proxy-source.json` and `missing-proxy-runtime-sha256.txt`.
- A new table regression reproduced static lookup advancing/restricting the
  mutable cursor after a failed row. Construction and inverse traversal now
  leave that cursor unchanged, reuse the failed mutable row, and reject a
  conflicting retry outcome. All 219 unit and 14 call-pattern tests pass.
  Static observations retain their complete callback commitments and exact
  replay obligations; table traversal is not itself live execution evidence.
- Nested rollback decoding now collects closed scopes inner-first and reverses
  once per action at the transaction boundary, avoiding repeated prepends while
  retaining the existing exact nested-scope round-trip regressions.

- A focused preflight test reproduced the missing-proxy accounting failure.
  The observer now recognizes only exact `StaticCallProxyNotDeployed` bytes,
  an empty-code account already read by the real manager, and a matching intended
  static call. That failed lookup does not satisfy the required read: missing
  retry, deployed-proxy errors and trailing error bytes remain rejected. All
  12 posting tests pass; the inbound process test must still be repeated.
- A failed inbound root with callbacks reproduced an empty source table. The
  materializer now retains source callback execution and discards only its
  canonical destination outputs. The previous source-revert fixture now asserts
  the retained callback identity and the absence of its destination delivery.
  All 219 unit and 15 call-pattern tests pass. Complex lookup classification and
  independent destination replay remain unfinished, so this does not establish
  end-to-end support for those roots.

## Validation-contract version 8 and observation evidence

The locally derived manager delivery now replays mutable lookup callbacks and
source rollback groups in disposable authenticated state. REVM bundle journals
retain nested rollback checkpoints without cloning the entire account cache per
scope. The canonical state remains a read-only witness source. Shared-boundary
rollback coverage includes 1,024 scopes. Repeated mutable rows rebuild and compare
their complete callback table and reverted rolling-hash commitment before reuse.
These changes still need complete network qualification and the final independent
soundness review.

Validation identity is now version 8 in protocol and RPC constants. Owning
specifications and portable identity vectors were updated; existing deployments
were not modified. The proposal's compatibility boundary applies: version-7
network registrations and keys do not authorize these new proof semantics.

The native lookup fixture initially failed manager replay because its old witness
omitted newly required storage proofs. The explicit reconstruction utility replayed
all 45 blocks from the published genesis and reproduced every block's exact RLP
and state root before supplementing only terminal trie/code evidence. The request
grew from 473,911 to 495,704 bytes; its new Keccak is
`0x73895e280887b30e3bd6d0066f3573f8a9ae7da25aefc228f6c492e56063df80`.
The fixture README records provenance and the previous hash. Expected proof bytes
were not changed by witness supplementation. Dependent streaming vectors were
regenerated; the manifest verifier checked all 29 vectors. Local diagnostics are
`candidate/lookup-witness-reconstruction.log` and
`candidate/version8-complete-witness-{vectors,manifest,regressions}.log`.

That package run passed 335 signer tests and failed one old missing-account test:
the now-complete witness proves absence for the arbitrary address formerly chosen
to reach a blinded path. The test now explicitly removes the destination leaf,
asserts exactly one node was removed, and requires the incomplete-account error.
Its missing-bytecode check also passes. A separate service regression removes only
the manager implementation code: canonical execution and semantic binding pass,
then `observation_replay` rejects before any signing call. Both focused regressions
pass. Full affected-package validation is being repeated; these local results do
not replace final source/runtime qualification.

The six-test static callback process run passed four positive cases (including
both missing-proxy/create/retry directions) and failed two negative tests at an
outdated error assertion. The actual source failure included exact
`StaticCallProxyNotDeployed(address)` bytes. Those assertions now require the source
failure and exact selector/address output while retaining receipt/nonce/balance
checks; the revised tests still need a repeat. That process run used binaries
recorded in `candidate/static-callback-runtime-sha256.json`, before the journal and
version-8 changes. It does not qualify the current source.

## Current affected-package regression results

The combined run in `candidate/version8-integrated-regressions.log` passed:

| Target | Passed | Ignored |
| --- | ---: | ---: |
| Blob codec / independent KZG oracle | 21 / 1 | 0 |
| Composer | 421 | 1 manual performance profile |
| Control RPC | 7 | 0 |
| Deriver | 46 | 0 |
| Driver | 74 | 0 |
| Payload codec / issue35 | 16 / 2 | 0 |
| Signer | 337 | 5 explicit fixture generation/inventory utilities |

The run stopped at the protocol's independent profile-vector test because its
ABI fixture still encoded version 7. The two profile and five verification-key
vectors were independently reconstructed with Python static ABI words and
cross-checked with Foundry `cast abi-encode` and `cast keccak` for version 8.
The subsequent `candidate/version8-profile-regressions.log` passed 222 protocol
unit tests, 15 call patterns, 10 causal evidence, three public-input hash vectors,
two rolling-hash vectors and 34 prover-client tests, with zero ignored tests.
No positive expected value was learned from a failing implementation result.
These are package/vector results; process and full-network qualification remain
separate obligations.

The current CI tooling run selected 421 tests: 412 passed and nine were skipped
(`candidate/version8-tooling-tests.log`). The two Gitleaks tests subsequently
passed with the pinned local scanner (`version8-secret-scan-tests-executed.log`).
The real archive relocation test passed in the earlier recorded 35-test tooling
run; its implementation has not changed since then. The remaining six opt-in
Docker/Kurtosis packaging/resource tests were not run here. Mandatory Chiado,
Kurtosis and Ethereum settlement configuration checks pass. Formatting and
`git diff --check` pass. Workspace Clippy found two unnecessary Result wrappers,
a collapsible condition and a test `.err().expect()`; cleanup and a repeat are
still due. These are recorded failures, not a passing lint result.

## Current runtime process checks

The source-based `node_e2e` run selected seven intended tests and passed all
seven, zero failed or ignored (184 other process tests filtered). It covered the
caught destination revert, inbound/outbound static callbacks, both missing-proxy
creation/retry directions and both uncaught missing-proxy rejections. The outbound
callback case includes independent follower replay. The revised negative tests
assert exact source-error ABI bytes and retain nonce, balance and receipt checks.

Diagnostics are local under `candidate/version8-process/`; the full command log is
`candidate/version8-process.log`. Compilation and the selected tests completed in
653.35 seconds after the harness build. `version8-process-source.json` records
589 source inputs, verified unchanged after the runtime build;
`version8-process-runtime.json` binds that manifest to all three runtime binary
SHA-256 hashes. Subsequent Clippy cleanup does not change those recorded binaries.
This is focused process evidence, not a complete protocol or CI qualification.

Workspace Clippy now passes after removing two infallible Result wrappers,
collapsing a condition and cleaning up three test expressions. The signer's
all-features Clippy and warning-denying rustdoc checks also pass. Full workspace
unit tests are in progress. First lint failures remain recorded separately.

The separate review identified a remaining source-group accounting gap in the
signer: mutable-only loads still infer source slot counts from root attempts.
A naturally reverted retry may reuse a single slot without an explicit rollback
marker. A focused regression has been added before implementation; reproduce
that failure and then use the same complete-group accounting as Composer and
follower. This finding is still open and prevents final soundness closure.

## Shared signer retry accounting

The new natural-retry regression reproduced `outbound host exceeds its source
load` before the fix (`candidate/natural-retry-accounting-before.log`). Two target
attempts fail naturally, restoring the L2 source cursor, and therefore share one
loaded source slot with no surviving receipt events. The signer now uses the
same complete-load accounting as Composer and follower rather than a separate
mutable/static partition rule. Borrowed entry iterators avoid copying ABI tables
merely to calculate these boundaries.

All 123 signer settlement tests pass after the fix. Split static pools and missing
loads still reject at effect-prefix validation with the shared helper's specific
error. Extra successful calls or rows now reach independent table binding and
are rejected there; a checkpoint partition cannot authorize execution coverage.
The retry regression additionally inserts a well-shaped unused source slot and
requires the inverse verifier's `did not exercise every source entry` rejection.
It passes (`candidate/natural-retry-unused-slot.log`). The first post-fix run's
three outdated gate/error assertions remain in
`candidate/natural-retry-accounting-after.log`; the corrected rejection assertions
and positive retry pass in `candidate/natural-retry-accounting-regressions.log`.

The first full workspace unit lane passed 1,631 tests with six ignored generation/
manual-profile utilities. Its compiled signer binary preceded the newly added
retry regression. A new full workspace run is in progress after the accounting
fix. Workspace Clippy passes for that fix. Contract formatting and all 153 tests
across 17 suites pass, and all three edited shell scripts pass `bash -n`.

The changed-file secret scan identified four newly derived public version-8
fixture verification keys. Their exact values and exact identity-file paths were
added to the existing allowlist, preserving historical fixture exceptions.
Both scanner regressions pass, including rejection of new credentials beside
allowed values and detection of allowed values outside their reviewed paths.
A repository-relative scan of the changed/new files passes with no findings
(`candidate/version8-working-tree-secrets-after.log`). The initial directory scan
and its redacted findings are retained separately; no operator credentials were
read or modified.

## Initial release qualification

Implementation commit `039b288cf03e039875f1f155d53035f665334732` passed
the subsequent complete workspace unit lane: 1,632 tests passed, with six
explicitly ignored fixture-generation/manual-profile utilities. Workspace
Clippy, signer all-feature Clippy and rustdoc, seven SDK tests, 29 conformance
vectors, and both vendored Reth shutdown/quota regressions passed. Dependency
deny passed; audit retained 19 existing allowed warnings.

A separate [stack probe](nesting-stack-probe.rs) exercised static and mutable
trees on an explicit 2 MiB thread stack. Materialization and independent table
verification passed through depth 1,024; depth 1,025 rejected before recursion.
The [results](nesting-stack-results.txt) are manual diagnostic evidence, not an
additional CI regression. The probe links this checkout's debug `eez_protocol`
rlib; its local executable/library identities are retained under
`candidate/nesting-stack-identity.json`. The recorded command used
`rustc +1.95.0 --edition=2024 -L dependency=target/debug/deps --extern
eez_protocol=target/debug/deps/libeez_protocol-76cfb18c0ff2ed8b.rlib` and passed
each depth as the first argument, with `mutable` as the optional second argument.

The CI release producer built shipping binaries in 1,002 seconds and the test
archive in 101 seconds, enforcing six CPUs and 28 GiB and preserving the shipping
dependency-feature guard. Local image/archive identity checks and the generated
genesis check passed. No images were published. The first archived producer run
executed 53 tests: 47 passed and six prefix-history tests failed. Both captured
prefix fixtures still held their version-7 derived verification key. Independent
Solidity ABI encoding and Keccak reproduced the version-8 key from the existing
bidirectional fixture oracle. Only the two `verification_key` fields changed;
blocks, witnesses, receipts and accepted-root evidence remain identical. The
fixture provenance and narrowly scoped secret-scan exceptions were updated.
The failed run is retained in `candidate/release-producer-tests.log`; it does not
qualify the candidate or permit E2E artifact export.
After correcting the fixture keys, all 20 blob-verifier example tests passed
with `cargo +1.95.0 test --locked -p eez-l1 --features chain-spec --example
verify_blob_evidence`; no tests were skipped. The initial focused command omitted
the example's required `chain-spec` feature and selected no tests. Both secret-scan
regressions pass with the updated exact-path exceptions.

The initial runtime is running a diagnostic network campaign in the new enclave
`eez-full-protocol-candidate-20261030`. A local copy of `run-ci.sh` preserves all
selection and validation assertions, resolves `HERE` to the original script
directory, and leaves the owned enclave running on exit for diagnostics and
independent replay. Its source/copy hashes and exact two changes are recorded in
`candidate/run-ci-retain-identity.json`. This runtime remains bound to commit
`039b288`; the corrected candidate must pass release qualification separately.
All 64 scenario paths and hashes were rechecked against the unchanged pin before
startup. Existing operator deployments and the stopped baseline enclave remain
untouched.

## Release qualification and native regression follow-up

Candidate `4c5060780d7cec124c9cafb1de61bb557e85b60c` passed the release
producer's shipping-feature guard, all 53 archived producer tests (zero skips),
verified E2E artifact export, consumer identity checks and the generated-genesis
check. The runtime archive SHA-256 is
`bdf76d2c8599f584fefde3176cd4a4f1c424ddba6b425adf94f37f6bac43f0ea`.
Local evidence is under `candidate/revision-4c506078/`. An early consumer attempt
saw an unsealed manifest and was correctly rejected before test selection; it
is recorded separately from the subsequent sealed-artifact qualification.

The initial diagnostic enclave completed its inbound, outbound, mixed,
mixed-pure and state-chaining prechecks. It was interrupted to switch to the
qualified candidate just after the protocol phase began: zero of 64 scenarios
completed. Its resulting 64 missing/failed accounting entries are interrupted
work, not scenario coverage or established runtime failures. Service logs and
state were retained and the owned enclave was stopped without removal. The new
`eez-full-protocol-final-4c506078` enclave uses the verified candidate artifacts.

The full native campaign selects 43 embedded tests and 142 cross-chain tests,
with the latter partitioned into four disjoint CI slices (36/36/35/35). Its first
failure was `overlay_failed_l1_call_discards_its_callback_state`: the archived
test still required rejection of failed calls with descendants. The candidate
instead included the transaction successfully, which is the newly authorized
behavior. The replacement regression requires the contract's caught-failure and
rollback assertions to succeed, full canonical settlement, a subsequent
successful cross-chain transaction, and independent follower reconstruction of
the canonical block/state. The original failure and datadirs remain under
`revision-4c506078/cross-chain-slice-2`; the replacement still needs qualification.

The candidate plus fetched remote/tag history secret scan passed (1,148 commits).
The local `--all` scan also includes unrelated operator-only branches and reported
two public commitment fields in their older audit records. Neither commit is in
the candidate ancestry or fetched remote/tag refs. No unrelated branch or
allowlist was modified; redacted findings and ref-scope evidence are retained in
`revision-4c506078/secret-scan-scope.json` and the corresponding scan logs.

## Open validation

Candidate per-scenario semantic results, independent follower replay, final signer
soundness review, full applicable CI checks and PR delivery remain outstanding.
The goal is not complete. Review legacy source-group partitioning, repeated-row
rollback boundaries and static candidate selection against the pinned contracts.
Exercise complete callback trees in the current runtime, then qualify all 64
scenarios through Kurtosis with no omissions. Local ignored diagnostics are not
retrievable evidence from another checkout; retain a compact final record and
artifact locations/digests before delivery.
