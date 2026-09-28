# Native system backport and issue #35

| Field | Value |
| --- | --- |
| Date and status | 2026-09-28; implementation complete for the explicitly amended RollupX profile; local validation complete with the exceptions and limits below |
| Source | Current PR #42 base `0b7beed5ca48157d720d6685435ba2121c1e9c1e`; initial head `ca370742920852ad1105df4e40d70625d8749145`; branch `backport/native-system-txs-issue-35` |
| Worktree | `/home/edu/Development/rollupX/artifacts/native-system-backport/rollupX` |
| Protocol | Clean initialized pin `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Toolchain | Rust 1.95.0; Foundry 1.7.1 |
| Requirements | [Proposal and explicit #35 amendment](../../proposals/native-system-transactions.md) |

The [implementation map](implementation-map.md) records upstream dispositions and
the purpose of each meaningful file group. Entries below are chronological; later
corrections supersede earlier pending statuses without erasing failure evidence.

## Final result

The approved `0x76`/nonce/2M contract, checked inbound reconstruction and direct V0
derivation are implemented on the final PR #42 head. The original #35 fixture
fails on the unchanged #42 decoder and passes here; codec replacement alone was
insufficient, so all producers, authenticated derivation, terminal checkpoints,
execution rules and independent signer validation were updated together.
No compatibility reader, migration path or dual transaction contract remains.

| Check | Final local result |
| --- | --- |
| Workspace tests, excluding separately run E2E package | 1,277 passed, zero failed; three explicit utilities ignored |
| Affected E2E-target tests on final #42 base | 28 passed across ownership, resume, skipped slots, multiblob, cold replay, four nodes and cross-chain gates; three large/retained-history utilities ignored |
| Pinned Stateless checkpoint/trie tests | Nine passed |
| Workspace all-target/all-feature Clippy, warnings denied | Passed |
| Shipping target selection, default features | Passed; local optimized dev build, not a release image |
| Signer rustdoc, warnings denied | Passed |
| Integration / pinned core contract tests | 83 / 517 passed; mandated core exclusions retained |
| Pinned protocol Anvil scenarios | 36 passed |
| CI tooling | 274 passed, four explicit opt-in checks skipped; affected artifact tests repeated after final Docker input fix |
| Actual Docker source-stage export | Passed; 24 literal Rust include paths resolve, exact genesis included |
| Genesis generation consistency, Rust formatting, shell syntax, diff whitespace | Passed |
| Solidity formatting | Modified test passes; full check fails on two unchanged PR #42 files |
| Dependency policy and audit | Passed under existing policy; zero vulnerability entries, nine unchanged warnings including the `lru` unsoundness advisory |

The [command/result record](validation-results.json) retains sanitized counts,
log digests and runtime identity; the [invariant map](implementation-map.md#acceptance-evidence-by-invariant)
connects requirements to regression boundaries. Detailed logs/datadirs and the
873-file source manifest remain local-only in this worktree's ignored artifacts.

Not run: the complete release-artifact/four-shard CI matrix, full Kurtosis PoS/PBS
devnet (requiring fresh images and an isolated enclave), all remaining E2E targets,
or the explicitly ignored large-history/profiling utilities. The directly affected
local suites above were used instead of rebuilding that full matrix. No deployment,
existing-network reset or operator service lifecycle change was performed.

The wider #35 section F manager/wrapper, trigger-policy, validator/private-delivery
and gossip conformance items retain the explicit [dispositions](../../proposals/native-system-transactions.md#wider-draft-conformance-dispositions-35-section-f).
This is not full draft Rollup0 conformance, and no external issue was closed or
amended. PR #42 is still open; retarget/rebase this branch when its resulting main
commit is available, preserving only this task's delta.

## Baseline and source evidence

PR #42 was open at inspection. Its base was
`fd77a2ebc147850333171045dbd263ec74156268`, and its exact head was used to create
the dedicated branch and worktree. `git merge-base HEAD refs/remotes/origin/pr-42`
returned `ca370742920852ad1105df4e40d70625d8749145` before implementation.
The original checkout contained unrelated documentation/evidence changes and was
preserved. The worktree was moved from `/tmp` to the disk-backed ignored
`artifacts/` area before implementation; its Git and submodule metadata were
repaired and the clean baseline and lineage checked again.

| Source | Exact inspected identity |
| --- | --- |
| Upstream #154 | base `7cb514f29b434ada5b2a88c9c3663d5c78166491`, head `683e86b8b7c6092ef6415dbe976807e93f40d1f8` |
| Upstream #162 | base `e4039ca068f70bf925e41dda64b2c3bcba4e23c0`, head `cb2d9ef1d71919d5d740480f08c1385f515fef43` |
| Draft referenced by #35 | `7c99bcf923e0ee0f867549a750d1ab1373bf845d`, read from the exact revision in a separate read-only checkout |

PR metadata, comments, reviews, commits and full diffs are local-only under
`artifacts/native-system-backport/review/` in this worktree. They are not committed
evidence available to another checkout; the public PRs and exact identities above
are the retrievable source references.

## Findings

#42's architectural changes include strict FIFO admission across batches,
outbound-before-inbound order within a batch, bounded candidate rebuilding from
the same parent, shared deadlines, retained pool boundaries, and canonical-only
consumption. Its prefix cache clones execution state and receipts while preserving
gas counters and applying pre-execution once. Native integration must adapt those
types without restoring the older upstream builder.

The fork uses newer Reth/REVM/Alloy than #154. Its pinned Stateless checkpoint
implementation still fixes Ethereum primitives. Generic native replay needs a
targeted compatibility adaptation. Existing signer authorization supports richer
nested/static/reverted groups than the simple upstream inbound shape and must be
preserved. The issue's original native transaction contract conflicts with #154;
the owner explicitly selected #154's contract. This does not resolve #35's other
wire-format, header, execution and authenticated-derivation requirements.

## Executed validation

All commands below ran in the dedicated worktree.

| Command | Evidence | Result |
| --- | --- | --- |
| `cargo +1.95.0 test --locked --offline -p eez-payload-codec --lib -- --test-threads=1` | Unit baseline; local-only `artifacts/native-system-backport/logs/baseline-codec.log` | 43 passed, 0 failed, 0 ignored |

This baseline success establishes existing codec behavior, not #35 conformance.
No process replay or deployment is validated yet.

### Native foundation and checked input

New native primitives and EVM adapters were semantically ported from #154/#162
without changing external package versions. Reth's current built-payload API
retains recovered blocks, and REVM's database error marker replaces the older
error bound. The first added raw-environment regression failed because upstream
accepted a missing native chain ID. The guard now also rejects priority fees,
access lists, blob fields, authorization lists, and inconsistent native recovery;
it preserves a supplied wrong sender long enough to reject it rather than
silently replacing it. These are additional checks at the direct EVM boundary.

| Command | Result | Local-only evidence |
| --- | --- | --- |
| `cargo +1.95.0 test --locked --offline -p eez-primitives -p eez-evm` before raw-environment fix | 10 EVM tests passed; added regression failed at missing chain ID; primitives did not run after failure | `logs/native-tests-adapt-1.log` |
| Same command after guard fix | 11 EVM and 8 primitives tests passed; 0 ignored | `logs/native-tests-adapt-2.log` |
| `cargo +1.95.0 test --locked --offline -p eez-protocol --lib` after typed reconstruction | 170 passed; 0 failed/ignored | `logs/protocol-sidecar-integration-3.log` |

Log paths above are under `artifacts/native-system-backport/`. Initial compile
failures from API differences are retained there as well. Binary hashes for the
first diagnostic runs were not captured before rebuilding, so those runs are
local source-level evidence rather than immutable executable provenance.

`InboundSidecar` preserves the fork's full nested table. It rejects supplied L1
root updates, wrong routing/source/root hashes, unsupported root shape, invalid
flat rolling hashes and value overflow. Nested rolling/position authorization
continues to require the independent complementary-table verifier. Reconstruction
accepts the checked type; raw ABI access is reserved for projection/authorization.
The signer now constructs native transactions without a system key. Composer,
follower, runtime and signer integration, configuration tests, generic Stateless
replay and #35's remaining implementation are still underway; passing foundation
tests must not be read as whole-system validation.

## Open acceptance

All proposal requirements remain open pending implementation and specific
regressions. Recheck #42's remote head and merge status before finalizing; inspect
the complete final diff relative to that base. Preserve baseline failures and
report unavailable validation explicitly. No deployment or service lifecycle
operation has been performed.

## Integration progress (not final validation)

PR #42 was rechecked through the GitHub API during integration: still open,
unmerged, with head `ca370742920852ad1105df4e40d70625d8749145`.

The native envelope now reaches composer execution and its candidate-local
prefix cache, the driver, deriver, runtime node/follower, witness capture, and
signer reconstruction. L1 final-bundle execution still uses Ethereum or Gnosis
executors and their chain specifications. Neither the FIFO shrinking algorithm
nor the prefix-cache ownership/lifetime rules were replaced.

Stateless is vendored at its existing `ffe41a37f67b7d77a4f32c19d7fcbd10ca7ea0f3`
pin. Only recovered-block/receipt generic bounds and the test workspace member
list change. `vendor/patches/stateless.patch` records these differences. Its
separate test lock starts from the root lock, adding ten serialization test
packages needed by upstream's retained tests; runtime versions are unchanged.

| Check | Result | Local-only log under `artifacts/native-system-backport/` |
| --- | --- | --- |
| Driver native integration, all-feature library tests | 59 passed | `logs/driver-native-tests-1.log` |
| Shared node library tests | 8 passed, including native raw pooled decoding and consensus/reorg conversion rejection | `logs/node-common-native-tests-2.log` |
| Composer and deriver library tests with `--locked --offline` | 291 composer and 48 deriver passed; one existing manual component profile ignored | `logs/composer-deriver-native-tests-2.log` |
| Vendored Stateless and tries tests | 9 passed; doc tests select zero tests | `logs/stateless-generic-tests-3.log` |
| Node and follower `check --locked --offline` | Passed; a now-removed follower unused import warning remains in this historical log | `logs/node-follower-native-check-2.log` |
| Initial integrated signer test run | 244 passed, 86 failed, 2 ignored; many RPC tests could not bind loopback in the sandbox. Other failures identified old signed-identity assumptions, an earlier checked-sidecar rejection boundary, and old captured proving fixtures | `logs/signer-native-tests-4.log` |

The signer RPC suite is being repeated with loopback access after unit-fixture
updates. Old captured proving requests must be regenerated by source-based E2E
execution; changing only their identity metadata would not establish replay.
No deployment, existing service restart, or existing datadir reset was performed.
Issue #35's full columnar format, authenticated header inputs and fixed execution
profile remain outstanding at this stage; these native integration results do
not close the issue.

Subsequent native integration evidence:

- `logs/signer-native-tests-6.log`: 326 passed, five failed, two pre-existing
  ignored. All remaining failures are retained captures with the previous
  system identity. The RPC tests now ran with loopback access; the earlier
  permission failures remain recorded above. An independent native Stateless
  regression passes: two consecutive transactions with nonzero nonces, zero
  funded balance despite a positive base fee, independently encoded `0x76`
  receipt roots, account-state roots, block hashes and transaction checkpoints.
- `logs/native-genesis-update-1.log`: generated both EEZL2 genesis fixtures with
  the reserved system immutable. Current state root is
  `0xa87041467bb5234c56c66dc344205edca09c14969dccceee7a447ea579065250`.
  This is intermediate native-backport evidence, before the issue #35 execution
  profile changes. It does not validate a deployed rollup.
- `logs/native-deploy-preflight-1.log`: both mocked preflight tests passed after
  removal of system-key derivation. No broadcast ran.

The earlier `derivable-native-mixed` captured oracle predates the current
selector profile and must remain historical rejection evidence. Regenerate the
current changing-static and caught-revert/lookup captures from source process
tests; do not fabricate a new identity around old witness bytes.

## Required base advanced during implementation

A subsequent API check found #42 open at
`dc9f50f8e9fa694c6bb5d2d1af9a4ca70e87447c`. Its sole added commit changes the
settlement-policy test to wait for the exact canonical funding-post count before
asserting it, and adds FIFO packing/correctness/operational evidence. Production
code and protocol/dependency pins are unchanged. The complete added diff and
evidence were inspected; none of its files overlapped this worktree's edits.

`git merge --ff-only refs/remotes/origin/pr-42` advanced the dedicated branch
without stashing, discarding or recreating the native work. Both `HEAD` and
`git merge-base HEAD refs/remotes/origin/pr-42` became `dc9f50f8...`. All earlier
results retain their original base identity; final validation must use the new
base. These #42 test/audit additions are not part of this task's diff.

## Native process regression and direct V0 work

The first source-based `minimal_bidirectional_cross_chain_smoke` failed because
#42's incremental terminal DA sizing still decoded internally generated L2
transactions with Ethereum's closed type enum. The composer retained/requeued
the requests, but no native candidate reached proving. The focused
`terminal_admission_counts_native_transaction_bytes_without_a_signing_key`
regression reproduced the same `unexpected tx type` error. The L2 estimator now
uses the native envelope; L1 companion blob decoding still uses Ethereum types.
Adjacent full-block DA decoding and ordinary-preparation classification were
adapted as well. The FIFO/prefix algorithm remains unchanged.

| Check | Result | Local-only log |
| --- | --- | --- |
| Initial native cross-chain smoke | Failed; retained its datadir and executable hashes | `logs/native-cross-chain-smoke-1.log`, `e2e/smoke-1-executed-identity.json` |
| Focused admission regression before correction | Failed with `admission transaction decode: unexpected tx type` | `logs/native-admission-before-1.log` |
| Composer library suite after correction | 292 passed, one existing ignored manual profile | `logs/native-admission-codec-after-2.log` |
| Same cross-chain smoke after correction | One selected test passed, 23 filtered out; real signer and bidirectional settlement | `logs/native-cross-chain-smoke-2.log`, `e2e/smoke-2-executed-identity.json` |
| Genesis check | Passed with the intermediate native state root recorded above | `logs/native-genesis-check-1.log` |
| Chiado configuration tests | 14 passed | `logs/native-chiado-config-1.log` |
| Integration contract build | Passed with existing compiler warnings | `logs/native-contract-build-1.log` |

The first added retained-size test had an incorrect assertion: Rust `Vec<u8>`'s
RLP encoding is a list, whereas the codec sizes a byte slice. The assertion now
uses `rlp.as_slice().length()`. Its codec/native round trip was already passing;
the earlier assertion failure remains in `native-admission-codec-after-2.log`.

Issue #35's exact 31-byte, 600-empty-block fixture failed against the existing
shipping decoder with `Rlp(UnexpectedLength)` (`logs/issue35-before-1.log`). This
was the updated #42 base plus native changes, before replacing the surrounding
format; its RLP framing still matched #42. The new `v0` module borrows columns,
validates full-u32 runs without expansion, checks canonical metadata and exact
ordinary transaction bytes, and provides lazy block plans and constant-size
composable sizing. It is **not wired into shipping callers yet**.
`logs/issue35-codec-after-1.log` records 53 unit tests and the one dedicated
issue regression passing. This does not establish end-to-end #35 conformance.

A separate L2 block-lifecycle adapter now has a passing adversarial-predeploy
test: two empty blocks execute the real EIP-2935 history code while leaving
beacon/withdrawal/consolidation sentinel storage unchanged. The explicit L1
mode performs the original Ethereum calls. All 12 EVM tests passed in
`logs/issue35-lifecycle-2.log`; production configuration has not selected the new
lifecycle yet. The first four-node attempt stopped during signer compilation
because this new adapter needed a corrected borrowed-spec conversion; it did
not reach multi-node execution (`logs/native-four-node-1.log`).

The corrected four-node process test passed in `logs/native-four-node-2.log`:
two independent Composer databases, two keyless followers (one joining after
both directions settled), exact complete RPC blocks/raw native bytes/receipts,
deposit and outbound accounting, nonces, and public native RPC rejection on all
four nodes. `e2e/four-node-2-executed-identity.json` retains executable hashes and
the source-file hash inventory. The second Composer's L1 is an Engine API mirror
of the fixture's development L1; the selected-builder relay allows one poster
at a time and retains the fork's blob-capture/Beacon path. This is independent
execution/derivation evidence, not PoS consensus or production builder testing.

For an exact requested-baseline comparison, the codec sources were extracted
unchanged from `dc9f50f8...` into an isolated test harness under this worktree's
`artifacts/native-system-backport/issue35-baseline/`. Only workspace membership
and local dependency paths were narrowed. Every resolved dependency version and
source was checked against that commit's lockfile; none changed. The same #35
fixture failed with `Rlp(UnexpectedLength)` in `logs/issue35-exact-base-2.log`.
`source-identity.json` records every extracted source hash. The first harness
attempt needed its narrowed lock graph refreshed offline; the actual regression
ran with `--locked --offline`. No source or runtime in the original checkout was
modified. The succeeding direct-codec test remains separate from production
adoption, which still requires the full header and action-reconstruction work.

## Fixed L2 execution profile integration

The separate lifecycle is now selected by production `EezEvmConfig`. L1
Ethereum/Gnosis source adapters explicitly select `for_l1_simulation`; their
normal beacon/request behavior remains. Composer prefix reuse still checkpoints
the same inner state/receipts/gas counters before finalization.

`eez-protocol::l2` owns fixed header fields and the 2/50 fee constants. The shared
L2 genesis loader rejects incompatible supplied values and missing/changed
history code, then installs the fixed fee parameters that generic Reth Genesis
loading does not express. Composer and follower now use the same strict CLI
loader, and production signer loading requires a full Genesis document. Validation
identity schema 3 includes the revised native/profile rules; its external hash
vector was independently recomputed using a small Python RLP encoder and `cast
keccak`. Original fixture captures have not been rewritten.

| Check | Result | Local-only log |
| --- | --- | --- |
| Wired lifecycle, composer/EVM/codec libraries | 292 / 12 / 53 passed; one existing composer manual test ignored | `logs/issue35-lifecycle-wired-2.log` |
| Protocol/profile/codec checks before loader wiring | EVM 14, codec 53 plus two dedicated issue/vector tests, protocol 172 plus independent protocol-vector integrations passed | `logs/issue35-profile-2.log` |
| Composer and RPC libraries with profile wiring | Composer 292 passed, one manual ignored; RPC six passed; then EVM gas-test assertion failed | `logs/issue35-profile-wired-2.log` |
| Corrected EVM gas test and node-common startup feature | EVM 14 and node-common 14 passed, no ignored tests | `logs/issue35-profile-wired-3.log` |

The gas test previously changed the block's gas limit below 2M, which the new
fixed-profile guard correctly rejects before transaction metering. Its replacement
executes fifteen 2M out-of-gas native transactions in a valid 30M block and rejects
a sixteenth at the shared gas boundary. Earlier failures from L1 test adapters
using the L2 constructor are retained in `issue35-profile-wired-1.log`; they were
corrected to use the same explicit L1 constructor as runtime wiring. The first
genesis-profile unit fixture used `ChainSpecBuilder` fork changes that are not
written back into its Genesis document; it now uses the actual repository Genesis
(`issue35-profile-1.log` retains the failure).

These checks do not validate the unfinished V0 production switch, authenticated
RANDAO context, or old capture migration. The passing four-node test above predates
this profile change and must be repeated against the final implementation.

The Engine/header ingress regression passes in `issue35-profile-ingress-3.log`.
Its earlier test adapter used a nonexistent `SealedBlock::block()` method, then
assumed a standalone slot-number header field survives V3 Engine conversion.
The corrected Engine case supplies the V4 access-list field as well and asserts
rejection; both diagnostic failures remain in the preceding numbered logs.

The targeted signer blob regression now checks the specific early transaction
admission error through real stateless validation, then checks the RPC's deliberately
sanitized rejection and zero signing calls. It passes in `signer-native-blob-1.log`.
All eight independent synthetic signer cases pass in
`signer-native-synthetic-profile-1.log`. The full signer suite then reports
317 passed, 15 failed and two existing ignored tests in
`signer-native-profile-all-1.log`. Four failures are old signed-system captures;
the others depend on old 2/8 fee headers, including tests whose intended DA
rejection is now preceded by that execution failure. These need current fixtures
and specific boundary assertions, not relaxed profile checks.

`cargo +1.95.0 fmt --all --check`, `git diff --check`, and reverse application of
the regenerated Stateless patch passed after formatting. A complete lockfile
identity comparison against the #42 base found no added external package
version/source pairs. A fresh GitHub read still reports PR #42 open at
`dc9f50f8e9fa694c6bb5d2d1af9a4ca70e87447c`; the local merge-base matches it.
The new four-node profile run is pending and retains fresh fixture exports under
`artifacts/native-system-backport/fixtures/native-profile-1/`.

The fresh profile four-node run passed (one selected test, 98.60 seconds) in
`profile-four-node-1.log`. Its retained executable hashes and 453-file source
inventory are in `e2e/profile-four-node-1-executed-identity.json`; every source
hash still matched at completion. This validates the new lifecycle and fixed fee
profile across the same two Composer/two follower topology, while production DA
and seed-context changes remain pending.

Three exact consecutive empty-block witnesses from that run are now the small
committed `native-osaka-empty` fixture. Each has its own history-write transition;
its oracle records block/state/header values and file hashes. Original historical
captures remain unchanged. The full signer rerun with these fixtures has
328 passing tests, four failing old signed-system capture tests, and two existing
ignored diagnostics (`signer-native-profile-all-2.log`). The empty-window tests
now exercise actual EIP-2935 state changes and 2/50 fees rather than cloning an
unchanged-state witness. The opt-in cold-span diagnostic requires an actual
current-profile capture and rejects an input too short for any intended case.


## Fresh native captures and second base update

The source process cases `outbound_changing_static_reads_and_write_settle_and_replay`
and `inbound_reverting_destination_preserves_caught_result_without_destination_writes`
passed in 85.35 and 119.90 seconds respectively. Logs are
`native-changing-static-profile-1.log` and `native-lookup-profile-1.log`; full source
and executable identities are in `e2e/profile-captures-1-executed-identity.json`.
All overrides selecting prebuilt runtime binaries were unset. Both captures use
the fixed native/Osaka execution profile; their operations still use the intermediate
RLP format and constant seed. They do not establish final issue #35 conformance.

The new committed `native-osaka-changing-static` and `native-osaka-lookup` fixtures
retain exact original requests/responses and fully expanded preparation witnesses.
The test loader now uses the production full-Genesis validator. It requires the
original digest and signature rather than adapting old DA bytes. All four affected
positive/rejection regressions and the historical deployment rejection pass in
`signer-profile-captured-3.log` (five passed, one explicit inventory tool ignored).
The first build exposed an undeclared temporary-file helper; using the committed
fixture path avoids a new dependency. The second invocation was blocked at loopback
socket creation by sandbox permissions; the third used authorized local sockets.
Those first failures remain in numbered logs. The four prior semantic failures
were not bypassed or converted into ignored tests.

PR #42 moved again to `c8260cec8e76a267c96cea1313d31b55a8383f75`. The added commit
only waits for isolated L1 persistence in one restart fixture and adds audit/live
validation records. All seven changed paths were inspected; none overlap this task.
The dedicated branch was fast-forwarded, and its complete tracked binary diff plus
every untracked task-file hash remained identical across the update. The merge-base
matches the new head. The original remote SSH URL was unavailable in this environment;
the public HTTPS PR ref was fetched instead. The task did not perform the deployments
or live workload described in #42's inherited records.

The shared semantic reconstruction conversion has four passing unit regressions
(`native-semantic-reconstruction-3.log`). They cover whole outbound groups, checked
inbound deliveries, native nonce progression, caught failure/static no-delivery,
foreign origins/targets, phase ordering, noncanonical/native origin transaction bytes,
settlement substitution, table/hash mutations and unused settlement suffixes. The
first compile needed public API documentation and the explicit native transaction
constructor fields. The next run corrected a test's assumption: one complete outbound
group has one native load, so that group plus two deliveries advances three nonces.
The assertion now protects that existing grouping behavior.

The L1 blob reader now retains authenticated semantic transactions instead of
discarding them after decoding. Live events and catch-up carry the same immutable
representation. Follower reconstruction consumes checked semantic groups/deliveries
and binds all settlement tables; the intermediate transported sidecars must still
match exactly until direct V0 replaces that redundant representation. The follower
no longer decodes checked inbound values through a generic outbound ABI vector.
The corresponding composer/deriver compile passes in
`native-semantic-followers-check-2.log`; affected runtime tests are in progress.


The retained-semantics blob-source tests pass (seven selected tests), all 48
follower library tests pass, and the five captured signer cases still pass with
an additional check that the shared semantic conversion binds their exact native
settlement tables. Logs: `native-semantic-blob-source-1.log`,
`native-semantic-deriver-1.log`, and `signer-profile-captured-4.log`.

The L2 genesis validator now also requires pre-Osaka consensus activations at
zero and merge completion, not merely `osakaTime = 0`. Otherwise the EVM could
select Osaka while sender recovery still accepted pre-Homestead high-s signatures.
The two profile tests pass (`native-genesis-profile-strict-1.log`). The complete
signer suite now passes: **332 passed, two existing opt-in diagnostics ignored**
(`signer-native-profile-all-3.log`). Formatting, whitespace validation and reverse
application of the Stateless patch also pass. A fresh four-node source run for
the retained semantic/follower changes is in progress; its pre-run inventory is
`e2e/semantic-four-node-1-source.json` (480 source/fixture files).

That four-node run passed (one selected test, zero ignored, 99.92 seconds):
`semantic-four-node-1.log`. Both composers and both followers agreed on native
bytes, receipts, block hashes and deposit/withdrawal state. Every source hash in
the pre-run inventory still matched at completion; binary digests and base
`c8260ce` are in `e2e/semantic-four-node-1-executed-identity.json`. This is evidence
for retained semantic reconstruction and the fixed execution profile, not for
the still-pending direct V0 and authenticated seed transition.

The shared timing/schedule conversion has three passing regressions
(`issue35-schedule-1.log`), including exact mainnet/Chiado grids, complete intervals,
live/catch-up classification, invalid parent/genesis/configuration rejection, and
full-u32 catch-up arithmetic without allocation. Runtime wiring remains pending.

The owner subsequently waived backward compatibility and requested the simplest
implementation. The proposal now requires one new-network profile and removal
of retired wire/configuration fallback paths. No migration or historical-format
replay is required; same-format partial consumption, reorg/restart correctness
and the accepted #42 architecture remain required.

The required `config.rollupX` genesis namespace now passes through both node CLI
loaders and the production signer loader. It is rejected by the separate L1
document schema. The complete original JSON remains the validation-identity
input. Composer/follower startup rejects runtime cadence that disagrees with
genesis. Fixtures bind their actual L1 genesis header before calculating the L2
registration commitment. Production L1 reference authentication and accepted seed
history have checked, tested APIs but are **not yet wired** into role lifecycle,
canonical publication or the signer's final authorization gate.

Validation for this stage:

- `issue35-l1-derivation-2.log`: four new L1 authentication/history tests pass.
  The first compile log retains a missing explicit generic RPC block type.
- `issue35-l2-genesis-parser-1.log`: two strict document tests pass.
- `issue35-required-genesis-profile-1.log`: two profile tests pass (before the
  subsequent added missing/invalid namespace mutation cases).
- `issue35-required-genesis-check-2.log`: locked/offline node, follower, signer
  and testkit check passes. Earlier compile failures exposed required local
  dependency edges and a protocol dependency incorrectly limited to observation.
- `issue35-l1-all-2.log`: all 150 L1 tests pass, none ignored. The first full run
  had 135 passes and 15 loopback-bind permission failures; the authorized socket
  run resolved those environment failures, not code failures.
- `issue35-genesis-timing-1.log`: all 37 selected timing tests pass.
- `issue35-required-genesis-cli-1.log`: both actual CLI parsing tests pass.
- `issue35-reference-preflight-1.log`: four mocked deployment preflight tests
  pass. Deployment now captures finalized reference identity/timestamp/RANDAO
  before the first broadcast and does not change genesis after deployment.
  This is mocked/syntax evidence only; no deployment was performed.

The lockfile refresh added only a required local follower-to-EVM edge. No
external package identity was added or upgraded relative to #42. The only
removed external sources are the three intentionally vendored Stateless crates;
the detailed comparison is `review/dependency-identity-delta.json`.

The example genesis hash helper agrees with an independent RLP/Keccak header
calculation (`issue35-genesis-reference-fixture-1.log` and
`review/genesis-references-independent.json`). The two committed genesis files
have different trie roots because the process fixture retains adversarial beacon
and request predeploys. Each gets its own correct development reference hash;
the fixture writer replaces this with the actual fresh test L1 reference.

A source four-node run of the mandatory genesis changes is in progress:
`required-genesis-four-node-1.log`, with 484 source/fixture hashes recorded in
`e2e/required-genesis-four-node-1-source.json`. Runtime source is frozen for this
run. It still uses the intermediate DA and constant runtime seed; passing it
does not finish #35 or validate the pending authenticated seed integration.

The mandatory-genesis four-node run passed: one selected test, zero ignored,
101.30 seconds, 98 identical blocks and two successful native transactions.
All 484 recorded source/fixture hashes stayed unchanged throughout the run;
binary hashes and completion are in
`e2e/required-genesis-four-node-1-executed-identity.json`. This exercises the new
production genesis loaders and cadence agreement, not the still-pending complete
Sync-interval admission or live seed selection. In particular, terminal height
98 is not divisible by the fixture's K=5: Reth's generic development miner uses
wall-clock timestamps without preserving genesis slot phase. Its test-only L1
attribute builder must be aligned before enforcing the new complete-interval
boundary; public L1 execution/consensus must remain unchanged.


## Authenticated runtime seed wiring (in progress)

PR #42 was rechecked during this phase and remains open at
`c8260cec8e76a267c96cea1313d31b55a8383f75`. No base change was necessary.

The node and follower now authenticate the configured finalized genesis L1
reference before constructing their deriver. Every L2 execution and Engine
payload checks genesis-derived number/timestamp placement. The driver accepts
only the fixed Osaka execution profile; its independent header oracle now uses
2/50 fee adjustment and verifies history writes without beacon-root writes.

The canonical settlement owner now holds seed history under the same mutex as
cursor/evidence publication. Composer candidates capture their seed once and
retain it across prefix-cache use, final reconstruction and retries. Ordinary
sequencing obtains the same parent-derived context. Derivation checks supplied
inputs before local block reuse, updates seeds only after accepted endpoints,
and rewinds stale-seed descendants. Same-height partial variants use the old
execution seed; catch-up acceptance does not refresh it.

The existing correlation index retains exact endpoint headers, including replaced
same-height variants. Startup reauthenticates containing canonical L1 headers and
rebuilds ordered seed provenance before readiness. The one-record checkpoint
reader/writer was removed. The correlation schema is now
`eez-batch-correlation-native-v0`; old databases are rejected, with no migration.
The owner authorized fresh genesis/datadirs rather than backward compatibility.

Evidence at this intermediate stage:

- `issue35-execution-position-1.log`: 15 EVM tests passed, including Engine/time
  rejection, native mint/revert, ordinary fees and nested zero GASPRICE.
- `issue35-seed-components-3.log`: 292 Composer tests passed (one existing manual
  test ignored), 40 Deriver tests passed, and 58/59 driver tests passed. The
  remaining observation-witness fixture still used pre-profile header fields.
- `issue35-seed-driver-4.log`: after updating that fixture, all 59 driver tests
  passed with `witness,derivation` features.
- `issue35-seed-owner-1.log`: five L1 source/seed tests passed, including atomic
  cursor/seed publication and same-height reorg. A subsequent change replaces
  linear duplicate/seed searches with ordered partition searches; revalidation
  of that final implementation remains necessary.
- `issue35-seed-wiring-3.log`: locked/offline checks of node, follower and signer
  passed. One local driver test dependency on already-pinned `serde_json` was
  added; no external package was upgraded.
- `issue35-l1-fixture-phase-1.log`: the private dev-miner phase test passed.
- `issue35-reference-preflight-2.log`: five mocked deployment preflight tests
  passed, including absent public-network presets versus explicit empty or
  conflicting timing. `bash -n scripts/deploy.sh` passed. No deployment ran.

The source four-node test is running in `authenticated-seed-four-node-1.log`.
It adds an independent oracle from accepted L1 occurrences for genesis timing
and live-anchor RANDAO refresh, beyond byte/hash/state agreement across nodes.
Its source/executable records are under `e2e/authenticated-seed-four-node-1-*`.

This is not completion of #35: the shipping DA body still uses the intermediate
full-terminal/environment representation. The signer still needs independent
canonical-parent/seed authentication and latest-L1-context public-input binding;
full interval admission and the direct V0 switch remain pending. The earlier
captured signer fixtures predate the mandatory genesis namespace and will be
replaced with final-format captures, not accepted through a legacy fallback.


The first authenticated-seed four-node run failed during the independent mirror
Composer's startup. The L1 history gate returned at genesis before the owned
Engine feed supplied finalized forkchoice; strict reference authentication then
reported `canonical header absent`. The primary Composer had already authenticated
its reference and produced blocks. The original failure, datadirs and binary/source
identity are retained in `authenticated-seed-four-node-1.log` and the matching
local-only `e2e/` records (258 recorded source files unchanged during that run).

The existing bounded L1 readiness gate now also waits for finalized height to
cover the genesis reference. It does not authenticate a hash or grant readiness
for derivation by itself: the deriver still checks the exact canonical reference,
network identity and seed. This handles an embedded execution client awaiting its
first consensus forkchoice without weakening finality. The four-node rerun is
`authenticated-seed-four-node-2.log`. The ordered seed implementation passed all
five selected tests in `issue35-seed-owner-2.log`.


`authenticated-seed-four-node-2.log` failed after 164.91 seconds with a 90-second
settlement timeout. The mirror passed the new finalized-reference readiness wait.
No input source changed during this run (578 file hashes recorded). The private
miner's ceil-to-grid timestamps were ahead of wall time, so new L1 heads repeatedly
superseded composition timers before they fired. This is distinct from the first
startup failure. The next clock change uses the last completed grid slot with the
actual parent's next slot as its monotonic lower bound. No real-network consensus
clock is changed. Both failures and their isolated datadirs remain retained.


### Latest-context public-input binding (intermediate)

The RPC now carries one mandatory L1 target (chain ID, parent number/hash and
child timestamp), with removed PostBatch fields 2/3 reserved. The batch uses the
pinned manager's `uint64.max` sentinel. Composer, prover client and signer recompute
its exact `(timestamp, parentBlockHash)` custom data. Composer checks the same
parent again after proving. This does not yet authenticate the signer's supplied
L1 context or accepted L2 ancestry; that separate gate remains unfinished.

- `issue35-miner-clock-2.log`: one private-miner phase/monotonicity test passed.
- `issue35-latest-context-rust-1.log`: all 14 public-input unit tests passed,
  including the independent Solidity digest and invalid-context rejections.
- `issue35-latest-solidity-vectors-2.log`: all seven Solidity vector tests passed.
  The new vector executes the pinned manager/EEZ and asserts `InvalidProof` after
  timestamp, parent-hash or timeless-sentinel substitution.
- `issue35-latest-post-batch-1.log`: 12 signer profile/hash tests passed.
- `issue35-latest-window-1.log`: 24 bounded-admission tests passed.
- `issue35-latest-attestation-1.log`: 37 in-process signer tests passed, including
  exact target-field attestation binding (stub execution, not canonical L1 auth).
- `issue35-latest-prover-client-1.log`: 30 client tests passed, including complete
  context and nine-blob RPC transport.
- Locked/offline shipping checks of Composer, signer, client and testkit passed;
  the affected Rust test targets also compiled.

Whole-contract `forge fmt --check --root contracts` reports existing formatting
in unchanged `ECDSAProofSystem.sol` and `DeployRollup.s.sol`; these unrelated files
were not changed. The edited Solidity vector file is formatted. The old ignored
cold-history prototype suite needs target/lifecycle adaptation before being
claimed as validation; it cannot post a proof under a different future parent.

### Four-node latest-context run (2026-09-28 16:34 UTC)

`latest-context-four-node-1.log`: source test passed (1 test, 98.79s).
Two composers and two keyless followers, including a late joiner, agreed on
95 blocks and both successful native transactions (inbound deposit and outbound
load), receipts, nonce progression, zero prices, state, and external admission
rejection. The test also observed live L1 seed refresh and ten descendant blocks.
All 578 files in the frozen source manifest remained unchanged; executable
SHA-256 identities are retained in local-only
`artifacts/native-system-backport/e2e/latest-context-four-node-1-executed-identity.json`.
The independent L1 signer proof module was then unreferenced and was not covered;
this run still used the existing RLP operations DA. These are intermediate native
execution and latest public-input binding results, not completion of issue #35.

### Independent signer L1 context (intermediate, 2026-09-28)

The production path now requires the operator's canonical L1 RPC and registry.
`eez-l1::derivation::proof` independently authenticates registered genesis, parent
acceptance, complete ordered live-slot endpoints, descendant seed, exact target
and current registry state. Request admission exact-decodes bounded parent/header
provenance. The blocking signer checks all replayed blocks, including cache hits,
and only execution-bound terminal checkpoints can match an already-consumed
registry prefix. The async request rechecks the L1 target after replay and before
constructing the attestable hash. Unavailable/replaced source context cannot
produce a candidate-eviction hint. There is no production offline fallback.

- `issue35-proof-source-2.log`: four mock-RPC tests pass (registration, false
  parents, manual resets, missed slots, complete/reordered endpoint sets,
  same-height live then catch-up, source failures, and changed-head revalidation).
- `issue35-provenance-window-1.log`: 25 admission tests pass, including required
  provenance, exact RLP/exhaustion, per-header bounds and aggregate charging.
- `issue35-l1-source-config-2.log`: 27 configuration tests pass, including source
  URL redaction, nonzero registry, environment fallback and CLI precedence.
- `issue35-l1-candidate-gates-1.log`: two synthetic replay-context/status tests
  pass; these isolate the gate and do not substitute for actual EVM/L1 replay.
- Shipping node/composer/signer/testkit checks pass in
  `issue35-signer-l1-wiring-2.log`; affected Rust test targets compile in
  `issue35-signer-l1-tests-check-1.log`.

Retained first failures: proof-source test compilation omitted a Schedule import;
wire compilation required public-field docs; signer wiring initially attempted
private raw batch access and was changed to consume execution-bound checkpoints;
configuration fixtures initially omitted the newly mandatory source fields.
These were code/fixture fixes, not retry-only test passes. A broad E2E target
check also found a missing latest-context argument in the old ignored cold-span
prototype (`issue35-l1-auth-e2e-check-1.log`); it remains to be adapted with the
V0 switch. The four-node source run with L1 authentication is still in progress.

PR #42 was rechecked live during this work: OPEN, unmerged, still
`c8260cec8e76a267c96cea1313d31b55a8383f75`; merge-base matches that head.

`issue35-signer-source-compose-1.log`: all 14 Chiado Compose configuration tests
pass. `issue35-signer-source-policy-config-1.log`: Chiado and Kurtosis mandatory
policy checks pass. Formatting, diff whitespace and reverse applicability of
the recorded Stateless patch pass at the L1-authentication source snapshot.
These are static/mocked configuration checks, not deployment evidence.

### L1 inclusion failure and private-miner phase (2026-09-28)

`l1-auth-four-node-1.log` failed after 98.82s: the first inbound L1 transaction
reverted. The signer authenticated and signed the 100-block window in 18ms;
ordered signed-bundle preflight then succeeded for L1 block 9, timestamp
1790614816. The test miner included the bundle before that timestamp, and both
postBatch and companion reverted. The default Reth interval was anchored to
process startup, while the custom timestamp builder rounded down to the genesis
slot phase. This can mine in the preceding slot after the Composer has already
prepared the next slot's timestamp-bound proof. This is not evidence of a
successful authenticated four-node run.

The private miner now uses an interval aligned to the next genesis slot and
skips missed timer ticks. Its existing timestamp builder still derives from
the actual parent. Ethereum/Gnosis consensus-driven deployment paths do not use
the private miner. The test additionally retains the inbound receipt and
containing L1 header, including on failure.

- `issue35-private-miner-phase-1.log`: four embedded-L1 tests pass, including
  fractional startup phase, elapsed slots and restart timestamp behavior.
- `issue35-l1-auth-attestation-1.log`: 37 signer attestation tests pass.
- `issue35-l1-auth-runtime-3.log`: 19 signer runtime tests pass. The first run
  found a fixture using the L1 parser for the L2 genesis extension; an initial
  correction referenced a private module and failed compilation. Both failures
  are retained in `-1` and `-2`; the fixture now uses the public L2 loader.
- `issue35-parent-provenance-client-1.log`: 30 prover-client tests pass.

The failed run's 448 compiled-source files were unchanged; source and executable
identities, logs and failure datadirs remain local-only under
`artifacts/native-system-backport/e2e/`. The corrected four-node run is pending.
PR #42 remains open at `c8260cec`; both upstream PRs and issue #35 were reread
live during this pass. No GitHub comments, issue changes or deployment occurred.

The corrected `l1-auth-phase-four-node-1.log` run subsequently passed: one test
in 92.22s, 90 identical blocks across two composers and two followers (including
a late joiner), two successful native transactions, exact receipts/state/nonces,
public RPC rejection, and ten blocks after an authenticated live seed refresh.
The mandatory independent L1 signer gate was active. All 486 frozen source
files were unchanged; executable digests are retained in
`e2e/l1-auth-phase-four-node-1-executed-identity.json`. This run still used the
old operations envelope; it does not validate the subsequent direct V0 switch.

### Direct V0 switch (in progress)

Producer, follower and signer now use the direct columns, with only terminal
ordinary prefixes in operations and checked action reconstruction outside them.
Exact activity sizing merges V0 metadata/count runs. The empty-terminal admission
budget charges semantic bytes once, without duplicating native envelopes or
outbound origin bytes. Ordinary sequencing bounds only V0 metadata/count and
transaction-length contributions. Tests and remaining format callers are being
adapted before process validation.

`issue35-v0-follower-check-1.log`: Composer and follower libraries compiled at
the first switch snapshot. `issue35-v0-signer-check-1.log`: signer compiled with
obsolete-field warnings still requiring cleanup. These are intermediate checks.
`issue35-v0-producer-1.log` selected zero tests because the derivation feature
was omitted, so it supplies no coverage. `-2` enabled the feature and found a
new fixture comparison of a byte slice against `Bytes`; the fixture was fixed
before the next run. No passing producer regression is credited to either run.

The corrected producer invocation (`issue35-v0-producer-3.log`) passes all 11
selected derivation regressions, including the actual producer's independent
31-byte / 600-empty-block oracle. Composer tests pass in
`issue35-v0-composer-tests-1.log`: 291 passed, one existing manual performance
case ignored. The V0 index tests retain hash/reorg/anchor cache invariants and
compare exact native-stream sizes; admission charges reconstructed actions only
through their semantic bytes. The former count-limit test was replaced because
native action transactions are not members of the pure-transaction count column.
The 16,383 bound remains tested in the V0 codec and ordinary sequencing sizer.

The obsolete operations reader, writer, RLP environment overrides, full-terminal
escape, and their size summaries have been removed. Count-token/property tests
moved into the sole V0 codec. `issue35-v0-only-codec-1.log`: all 15 unit and both
issue-35/vector integration tests pass. Wire u32 limits and operator quotas are
now distinct; the default signer quota remains 65,536. Terminal metadata and
ordinary-prefix binding tests are being ported to the actual direct columns.

Inspection found a required terminal-variant correction: the leading anchor
used H-1 instead of the independently executed terminal ordinary prefix B[0].
For the current Composer's empty ordinary prefix, its already-built admission
template supplies B[0]; no additional prefix execution or cache redesign is
needed. Signer checkpoint replay now records the state after pre-execution
changes, so an empty B[0] includes EIP-2935 and cannot substitute the parent
root. Ordinary-prefix checkpoints are separated from action checkpoints;
observation pins use the corresponding transaction count. Existing failed
outbound root carriers remain intact.

`issue35-terminal-base-check-1.log` first found an incorrect SealedBlock accessor
in the new observation position calculation; it was corrected. The stateless
suite (`issue35-terminal-base-stateless-1.log`) then reported 15 passes and 15
failures, primarily older captures without the mandatory genesis schedule and
tests retaining the earlier ordinary/effect checkpoint interpretation. Those
failures are retained and are not counted as validation. Replacement captures
must use the accepted schedule and direct format; no production fallback is
being added. A source-frozen direct-V0 four-node run is in progress with its
inventory at `e2e/direct-v0-base-four-node-1-source.json`.

Live base recheck after the codec removal: PR #42 remains OPEN/unmerged at
`c8260cec8e76a267c96cea1313d31b55a8383f75` (updated 2026-09-28T17:14:23Z), and the
branch's merge-base still matches. The first network check was sandbox-denied;
the approved read-only retry succeeded.


### Direct V0 process result and candidate-boundary follow-up

`direct-v0-base-four-node-1.log`: **PASS**, one selected test in 118.79s.
Two Composers and two keyless followers (including a late joiner) agreed on
115 blocks and native receipts. Both native actions succeeded; deposit/withdrawal
accounting and rejection at all four public RPCs passed. Fifteen blocks followed
a live seed refresh. The mandatory independent L1 signer authorization and the
terminal B[0] correction were active. All 330 inventoried Rust/manifest files
were unchanged through execution; executable digests are retained in
`e2e/direct-v0-base-four-node-1-executed-identity.json`. This result supersedes
the older-format process result for the direct V0 behavior, but precedes the
following additional candidate checks.

The signer and follower now call the shared complete Sync-interval/catch-up
validator. The follower validates the original whole candidate even when only
a consumed terminal suffix is being resumed. The L1 input is the exact canonical
posting block; deferred consumption in this pinned protocol is attributed within
that same L1 block. The signer checks action presence after semantic binding,
including static/failed actions that emit no native transaction.

An overlapping batch can re-index an accepted endpoint as an ordinary block.
Using the general block-hash lookup for parent provenance then loses its earlier
acceptance. A dedicated endpoint index retains the most recent actual terminal
acceptance without changing public block lookup semantics. Reorg rebuilds it in
canonical L1 order. `endpoint-provenance-1.log`: all five correlation tests pass,
including overlap, persistent reopen, and reorg regression. No old-schema
migration is added.

`candidate-gates-check-1.log`: signer and deriver compile, with obsolete test-oracle
fields/getters still requiring cleanup. `v0-fixture-compile-1.log` found four
remaining test-only Genesis-vs-ChainConfig field uses during fixture porting;
this failure supplies no runtime validation. Exact new cross-chain captures are
being generated before the corresponding replay fixture replacement.


`contracts-final-v0-1.log`: **83 integration contract tests passed**, none skipped.
`core-final-v0-1.log`: **517 pinned core tests passed**, with the required
`script/*`, `test_Doc_OriginalValue_SeedInSetup`, and `test_PostCost_Incremental`
exclusions; these exclusions are not credited as coverage.
`genesis-final-v0-1.log`: `scripts/update-eezl2-genesis.sh --check` passed using
the local source-built helper. State root remains
`0xa87041467bb5234c56c66dc344205edca09c14969dccceee7a447ea579065250`.

`direct-v0-cross-chain-1.log`: **FAIL**, one test in 186.86s. The changing-static
outbound action settled and received an independently authorized signature, but
its subsequent `eth_call` failed: Reth's generic pending defaults use parent
`timestamp + 12`, violating the L2's genesis-derived schedule. The source snapshot
and executable hashes are retained in
`e2e/direct-v0-cross-chain-1-executed-identity.json`; owned failing datadirs/logs
remain available. This is not counted as a successful cross-chain/follower run.
The exported successful proving request is an exact replay input, even though
the later read/follower assertions did not complete.

The RPC now installs a chain-specific pending-environment builder. It derives
the next timestamp from the pinned schedule, uses the fixed empty beacon and
withdrawal profile, and inherits the parent's RANDAO for uncommitted simulation
instead of selecting random bytes. Committed candidate seeds still come from
canonical L1 history; this RPC default grants no settlement authority. Explicit
nonzero beacon overrides fail. `rpc-pending-profile-check-1.log` compiled before
the focused regression was added; the regression/process repeat remains pending.

`v0-stateless-2.log`: 19 passed, 11 failed at the fixture-port snapshot. Fresh
empty-block execution and its EIP-2935 commitments now pass; the remaining old
observation/checkpoint captures and a receipt-count mismatch in the prefix-policy
unit input are being replaced/corrected. No legacy execution fallback is added.


`rpc-pending-profile-test-1.log`: **5 tests passed** (the new pending schedule/
determinism regression and four existing correlation RPC regressions). The new
case exercises `next_evm_env` with the RPC's actual constructed defaults and
rejects a nonzero beacon override. The shipping RPC builder is installed for
both Composer and follower roles. `reth-rpc-eth-api` is now an unconditional
node-common dependency because RPC wiring is unconditional; no dependency
revision changed. The V0 codec's unused `reth-ethereum-primitives` edge was
removed from its manifest and package lock entry.

A second source-frozen changing-static process run is recorded under
`direct-v0-cross-chain-2`; its result is pending. Its source manifest additionally
includes root `Cargo.toml` and `Cargo.lock`. The earlier failed run remains the
before-fix reproduction; a later pass will be reported as a fix comparison,
not as a retry-only pass of unchanged code.


`direct-v0-cross-chain-2.log`: **PASS**, one selected changing-static outbound
scenario in 95.37s. It verifies the mixed-read totals (including the changed
read after the write), successful L1 settlement, L2 effects, and a clean follower
reconstructing the accepted block from L1/blobs. The candidate interval gates
and pending-RPC fix were active. All 332 source/manifest/lock files were unchanged;
executable digests are in `e2e/direct-v0-cross-chain-2-executed-identity.json`.
The committed changing-static replay fixture now contains the exact successful
request/response from this run, replacing the earlier failed-process export.
PR #42 was rechecked again: OPEN, unmerged, unchanged `c8260ce`.


### Direct V0 lookup capture and sole-format signer regressions

`direct-v0-lookup-1.log`: **PASS**, one selected source process test in 110.05s.
Three inbound caught reverts preserved outcomes without destination writes; the
following successful inbound call applied. All 332 source/manifest files remained
unchanged, with executable digests in
`e2e/direct-v0-lookup-1-executed-identity.json`. The inventory helper selected the
first successful proof capture (blocks 1–95, failed lookup, empty terminal action
suffix) for `native-osaka-lookup`; request, response and expanded cold witnesses
are unmodified. `direct-v0-lookup-inventory-1.log`: one explicitly selected
inventory utility passed; that utility is not runtime correctness coverage.

`v0-stateless-3.log`: 29 passed, one failed. The failure was an obsolete test's
assumption that the historical signed-selector fixture had an invalid header;
header shape alone does not identify native privilege. The corrected regression
checks its real boundary: signed selectors remain ordinary transactions. The
fresh static/mutable observation, FIFO, checkpoint and exact replay cases passed.

Removed the test-only projected RLP decoder and its service selector entirely.
All DA and service tests now call the shipping V0 gate. Deleted obsolete sidecar
array/tag/group-count encoding tests; their safety requirements remain covered
at checked inbound/outbound/semantic boundaries and by current captured-request
mutations. Retained exact full inbound, outbound, grouped, mixed and nonce
reconstruction tests, with V0 metadata/count/transaction rejection cases.
Synthetic settlement/lifecycle tests explicitly construct test-only replay facts;
`successful_backend_without_reconstruction_cannot_authorize_v0` removes those
facts and still rejects. Real replay remains mandatory in production and in the
captured/independent fixture suites. Removed unused callback-position storage and
obsolete DA error variants that existed solely for the old decoder.

`v0-only-signer-compile-1.log`: three test compile errors during this conversion,
subsequently corrected. `v0-only-signer-1.log`: 243 passed, 85 failed; most RPC
cases could not bind loopback sockets in the sandbox. The unrestricted local-test
repeat `v0-only-signer-2.log` selected the full suite: 300 passed, 28 failed from
old fixture framing/metadata and gate expectations. `v0-only-signer-3.log`: 327
passed, one independent native oracle failed because its ChainSpec still used
Ethereum's fee denominator. `v0-only-signer-4.log`: **328 passed**, zero failures,
two explicit local utilities ignored. The final run reproduces original public
inputs/signatures for both fresh native captures and covers real execution,
observation rejection, cache facts, request lifecycle and attestation. These
fixes alter fixture construction or remove obsolete readers; no production
validation was disabled to pass them.

`v0-cold-harness-check-1.log`: the cold-following process harness compiles. It now
uses the genesis Sync grid, exact genesis parent provenance, and canonical L1
parent/next-slot context selected after preparation and KZG work. Repeated proofs
compare signatures only within an identical L1 target; each returned digest is
checked against its bound context. Retained fixtures require a live canonical
L1 source, with no stale-genesis or wall-clock target fallback. The source-frozen
process run `v0-cold-four-node-1` is pending.


`fmt-v0-1.log`: formatting passed. Edited shell scripts passed `bash -n`;
`check-settlement-policy-config.py` passed both Chiado and Kurtosis configurations.
`dependency-deny-v0-1.log`: advisories/bans/sources passed under repository policy.
`dependency-audit-v0-1.json`: cargo-audit exited successfully with zero vulnerability
entries and nine warnings. All warned package/version pairs already exist in
PR #42's lockfile: six unmaintained packages, `lru 0.16.4` unsoundness advisory
`RUSTSEC-2026-0253`, and yanked `chacha20 0.10.0` / `spin 0.9.8`. No dependency
upgrade or advisory-policy exception was introduced to suppress these warnings.

`ci-tooling-v0-1.log`: 278 selected, one failure, four opt-in Docker/Kurtosis checks
skipped. The failure correctly found two new test targets missing from CI's exact
artifact inventory. After reading the runtime producer, verifier and native runner,
`native_system` was added to the cross-chain suite and codec `issue35` vectors to
the unit suite, preserving separate Composer archives and four-slice E2E consumers.
The first focused repeat found the same missing entries in an explicit expected
inventory list; corrected that fixture too. `native-artifact-inventory-v0-3.log`
records the final focused result. The full tooling repeat is still due after
rebasing onto the latest PR #42.


### Third required-base update and pre-update process evidence

`v0-cold-four-node-1.log`: **PASS**. `derivable_operations` selected three normal
tests (Ethereum cold replay, Gnosis cold replay, genesis-grid arithmetic), all
passed in 124.81s; three explicit large/retained-history utilities were ignored.
Both profiles verified actual signed native blob settlement and a clean follower
using only L1/DA/genesis. The final four-node case passed in 53.00s: 50 identical
blocks, two successful native transactions with identical receipts, all-node RPC
rejection and ten blocks following a live seed refresh. All 332 inventoried
Rust/manifest files remained unchanged. Executable/source identity is retained
in `e2e/v0-cold-four-node-1-executed-identity.json`. These results predate the next
base update and are not credited as validating its consecutive-skip extension.

PR #42 moved during this run to `0b7beed5ca48157d720d6685435ba2121c1e9c1e`, still
OPEN. Inspected both commits (`ba4cbab`, `0b7beed`) and all affected runtime,
contract/docs, process tests and audit scope. The extension shares a four-retry
bound between scheduling/submission, retains all previous owners' history, and
adds consecutive skipped-slot/late-original tests. This is an upstream base
policy change, not part of the backport. Our authenticated target rules already
allow aligned future child slots on the exact unchanged canonical parent; no
protocol relaxation is needed. The cold test chooses the next slot normally.

After the frozen run ended, preserved a binary patch, untracked-file archive and
named stash, advanced the dedicated branch, and restored the task. The only
three-way overlap was in control-RPC SPEC and merged cleanly in distinct sections.
`git merge-base HEAD refs/remotes/origin/pr-42` is now exactly `0b7beed...`; protocol
remains `9d13218...`. No original working-tree changes were made. Revalidation on
this updated base includes its affected scheduler/submitter/ownership regressions.
The backup stash identity is recorded locally in
`review/task-before-0b7beed-stash.txt` and remains retained.


### Workspace validation and documentation audit on 0b7beed

`ci-tooling-v0-2.log`: **278 selected, 274 passed, four explicit opt-in checks
skipped**, zero failures. This includes the revised exact artifact inventory.
The later public-replay fixture cleanup removes its obsolete system-key input;
its focused repeat is recorded separately.

The all-feature workspace Clippy passes exposed lint-only adaptations in the new
native adapters, L1 derivation and signer tests, and the increased slot-future
size. Retained logs `clippy-workspace-v0-1` through `-5` record these failures.
The slot orchestration future is boxed; the authenticated proof context is boxed
as it crosses the async/blocking boundary. Two single-variant production enums
retain their direct values, with a test-only large-variant allowance for the
synthetic test alternative. No runtime validation is waived.

The documentation audit removed the stale signer `.env.example` system key and
updated startup, data provenance, checkpoint and request-lifecycle guides. L1
source authentication and the post-worker target recheck now precede the sole
construction of `AttestablePublicInputsHash`; the guides no longer describe a
timeless signature or a test-only DA decoder.

`protocol-anvil-v0-1.log` was interrupted after environment failures. Ports
54000–54071 passed the initial bind check but overlap Linux's ephemeral range
32768–60999; later Anvil starts collided before contract execution. Six scenarios
passed before the first collision. Stopped only the owned runner and its process
group. Logs remain under protocol `tmp/e2e-ci-JOe0lGfD`. The complete repeat uses
checked unused ports 24000–24071 and chain IDs starting at 52337, outside that
range, with public fixture wallets. It does not use operator services or wallets.


`clippy-workspace-v0-7.log`: **PASS** for locked/offline workspace, all targets,
all features, warnings denied. `workspace-tests-v0-1.log` selected the complete
workspace except separately run `eez-node-e2e`: **1,275 passed, one failed, three
explicit utilities ignored**. The failure exposed a missed production V0 sizing
consumer: completed/cached/empty payload checks still charged the old full-block
RLP while transaction admission charged V0 columns. Replaced that check with
`BlockDaSizer::retained_item_bytes`; invalid ordinary blocks remain inadmissible.
The regression independently encodes V0 columns across empty/nonempty and RLP/
length boundaries. `payload-v0-columns-1.log`: **eight selected default-feature
node-common tests passed**, including the corrected completed-payload regression.
The workspace's additional RPC/CLI feature tests passed in the preceding run.
A final full workspace repeat remains due after process validation.

`public-network-tooling-v0-1.log`: **11 passed** after removing the obsolete
system-key input from the mocked public-replay fixture. This is tooling evidence,
not a live public-network replay.

The `post42-ownership-1` process run freezes 873 source/manifest/proto/contract/
fixture files (including vendored runtime sources) against base `0b7beed...`.
It selects chained interstate calls, partial consumption/restart and L1 reorg
recovery serially with source-built runtime binaries and retained logs/datadirs.
Results and executable/source comparisons will follow; the manifest alone is
not passing evidence.


`protocol-anvil-v0-2.log`: **36/36 pinned protocol scenarios passed** on the
separate non-ephemeral port range. The [per-scenario results](protocol-anvil-results.tsv)
retain statuses and durations; detailed local-only logs are under protocol
`tmp/e2e-ci-2UoGcpzE`. This validates local core contract behavior, including all
static/reverted cases selected by the Anvil runner. It is not a Kurtosis/PBS
network result or evidence for the Rust native-envelope adapter by itself.


Final-search checkpoint: active runtime/configuration/CI sources contain no
`EEZ_L2_SYSTEM_KEY` dependency. Remaining occurrences are the removed-flag rejection
test, a keyless-fixture assertion and documentation of removal. Native construction
in production goes through `eez-protocol::system_tx`; direct struct constructions
outside primitives are tests. Checked inbound sidecars remain typed in reconstruction;
the raw ABI clone in outbound authorization is used for complementary table checking,
not reconstruction. Remaining hash-sentinel tests classify the pinned ABI at its
boundary, while semantic reconstruction carries explicit direction.

The original exact-baseline codec reproduction remains applicable to the latest
PR #42: `git diff dc9f50f8 refs/remotes/origin/pr-42 -- crates/eez-payload-codec`
is empty. The #42 head was rechecked OPEN and unchanged at `0b7beed...` during
this validation. Reverse application checks of all retained vendor patches pass.
All edited Markdown local file targets resolved in the first documentation pass.

### Final combined checks on 0b7beed

`post42-ownership-1.log`: **ten passed, one failed**. All five chained-interstate
cases passed (including three inbound calls, identical calls and mixed direction),
as did four partial-consumption/restart cases and the queue-reorg recovery case.
The same-block resume reached the expected Composer block and safe head, then
failed in the test's Ethereum-only full-transaction RPC decoder on type `0x76`.
It did not yet reach its follower parity assertion. The test now uses Alloy's
`AnyNetwork` decoder and retains full-block, exact user uniqueness and convergence
assertions. The failed logs/datadirs and executable/source manifest are retained;
all 873 frozen source files were unchanged during that run. The rerun is recorded
below when complete.

`ordinary-native-prefix-1.log`: **one selected test passed**. The new independent
Stateless oracle executes a paid ordinary prefix followed by two native actions
with nonzero starting system nonce. It hand-builds the initial and all resulting
account tries, independently computes receipt/transaction roots and asserts B[0],
both action block-hash checkpoints, final state, nonce progression and ordinary
fee deduction. Its codeless manager fixture isolates checkpoint selection; the
captured/process suites supply contract execution and L1 authorization coverage.

`clippy-workspace-v0-8.log` caught the `AnyNetwork` test's missing transaction-hash
trait method. After using `TransactionResponse::tx_hash`, the full workspace,
all-target, all-feature Clippy run with warnings denied **passed** in
`clippy-workspace-v0-9.log`. Formatting, diff whitespace and edited-shell syntax
checks also passed. `dynamic_admission.rs` contains only a rustfmt line wrap from
the latest required base, not a duplicated behavioral change from PR #42.

The final source-frozen process group (`post42-final-1-source.json`) selects the
same-block resume, skipped-slot/late-original recovery, more-than-six-blob native
settlement, cold Ethereum/Gnosis replay, four keyless nodes and affected real-signer
cross-chain cases. Results are not inferred from selection or compilation.

Live GitHub reread: PR #42 remains OPEN/unmerged at `0b7beed...`; upstream #154 and
#162 retain the inspected heads. Re-read their descriptions, issue #35 and the
#162 validation-boundary review. The supplied destination/root-update normalization
finding is covered by checking the original ABI before reconstruction. The native
contract's zero gas price intentionally causes positive-price guards to fail and
zero-price reimbursements to evaluate to zero; it is not application compatibility
with contracts that require a paid transaction.

`contracts-test-final.log`: **83/83 integration contract tests passed**.
`core-test-final.log`: **517/517 pinned core tests passed**, retaining the mandated
`script/*`, `test_Doc_OriginalValue_SeedInSetup` and `test_PostCost_Incremental`
exclusions. These exclusions are not coverage.

`forge fmt --check --root contracts` **failed on two unchanged baseline files**:
`contracts/src/ECDSAProofSystem.sol` and `contracts/script/DeployRollup.s.sol`.
Their exact contents match PR #42 (`git diff --exit-code origin/pr-42 -- ...`
passed). They are preserved to avoid unrelated source/metadata changes. The sole
modified Solidity file, `PublicInputsHashVectors.t.sol`, passes its focused format
check (`contracts-changed-fmt-final.log`). The full-format failure is retained in
`contracts-fmt-final.log`, not reported as a pass.

`post42-resume-2.log`: **one selected same-block resume test passed** in 90.40s,
including complete Composer/follower block convergence and exact-once signed-user
inclusion after partial consumption. This is the repeat of the failed decoder
case, with the decoder adaptation described above.

`post42-skipped-slots-1.log`: **five selected tests passed** in 376.27s: one and
consecutive skipped slots, a historical blob-prefix replacement, late original
after one replacement, and late original after multiple replacements. These
exercise PR #42's final bounded-replacement extension with native transactions,
canonical ownership, full user groups and follower parity. No retry was needed.

`post42-multiblob-1.log`: **one selected process test passed** in 98.16s. Retained
ordinary history settled in a single native Prague batch with more than six blobs;
the test checked actual payload capacity and independent follower reconstruction.

The final CI input-closure audit found that four new L2 profile tests embed the
root `genesis.json`, while `Dockerfile.runtime`'s source stage previously omitted
it. Added that one COPY input after rereading the runtime producer, verifier and
native runner. This changes no Rust, contract or process-test source in the frozen
run. `runtime-source-stage-final.log` records a **successful actual BuildKit source
stage export** from an explicit source-only tar context, with no operator files
or service lifecycle operations. Exported genesis matches byte-for-byte; all
**24 literal Rust include paths** resolve in the exported crate sources. The
export's genesis SHA-256 is
`3ab7026ba84cdcec253f9e05fdcf661e118dcca0b6e59e527156fe189529d616`.
This is source-stage validation, not a complete release-image build.
`native-artifacts-final.log`: **28 passed, one explicitly opt-in archive utility
skipped** after that correction.

`post42-cold-four-node-1.log`: **all four selected normal tests passed**. Ethereum
and Gnosis cold L1/DA/genesis-only replay plus the schedule-grid regression passed
in 119.55s; three explicitly ignored large/retained-history utilities were not
run. The separate two-Composer/two-keyless-follower process test passed in 52.67s,
including the late joiner, independent production by both Composers, native
bytes/receipts/block-hash equality, nonce/balance assertions, live-anchor seed
refresh and public RPC rejection on all four nodes. These results use the final
PR #42 base, unlike the earlier historical captures.

`post42-cross-chain-1.log`: **seven selected real-process tests passed** in 400.68s:
nested inbound attribution, inbound and outbound caught reverts, inbound
value/calldata atomicity, changing static reads with settlement/follower replay,
and real-signer rejection of tampered PostBatch calldata and tampered witnesses.
Both rejection tests reached the signer; unrelated passing tests are not their
evidence.

The final combined process group completed with zero failed commands. All 873
inventoried runtime/test/fixture source files remained byte-identical throughout;
`e2e/post42-final-1-executed-identity.json` retains runtime binary digests and the
per-command exit codes. Its source manifest and detailed logs/datadirs are
local-only. The later Docker source COPY correction and documentation changes
were outside those runtime inputs.

`workspace-tests-v0-2.log`: **1,277 passed, zero failed, three explicitly ignored**
across the workspace excluding `eez-node-e2e`, whose affected targets ran serially
above. This repeats the earlier full workspace run after fixing completed-payload
V0 sizing and adding the paid-prefix/native checkpoint oracle. All 42 test/doc-test
programs finished successfully; programs selecting zero doc tests are not extra
coverage. The ignored tests are existing manual profiling/fixture utilities.

The dedicated branch's merge-base was checked again after fetching PR #42:
`0b7beed5ca48157d720d6685435ba2121c1e9c1e`, still OPEN/unmerged. Its protocol gitlink
and initialized checkout remain clean at `9d13218...`. All retained vendor patches
pass reverse-application checks. No task source change was made in the original
checkout or counted as part of PR #42.

`shipping-build-final.log`: **PASS**, using the default-feature shipping target
selection from `Dockerfile.runtime` (Composer, follower, signer and the four
shipping helper examples), with Rust 1.95.0 and locked/offline dependencies.
This was the optimized local dev profile, not a release-image publication.
`signer-rustdoc-final.log`: **PASS** with `RUSTDOCFLAGS='-D warnings'`, package
`eez-proof-signer`, no dependency documentation. No public API documentation
warning was suppressed.

`genesis-check-final.log`: **PASS**, artifacts current with state root
`0xa87041467bb5234c56c66dc344205edca09c14969dccceee7a447ea579065250`.
The local check built this revision's helper; it did not rewrite fixtures or
broadcast a deployment. `rust-fmt-final.log`: **PASS**.
`stateless-final.log`: **nine upstream checkpoint/trie tests passed**, including
recorded full-state-root checkpoint and blinded-storage reset oracles; zero
ignored. Zero-selected doc tests are not counted as validation.

Final Cargo package-identity comparison against PR #42 adds only the two native
workspace packages and switches the same three pinned Stateless package identities
to their local source adaptation. No external package version/source pair changed.
The final diff includes no protocol gitlink or original-worktree edits. Its one
inherited Rust formatting adjustment is identified above; PR #42's own commits,
cache/recovery architecture and changes are not included as task additions.
