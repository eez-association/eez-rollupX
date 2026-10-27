# Complete-group cleanup, optional history and source identities

| Field | Value |
| --- | --- |
| Status | Implementation and scoped local validation complete at `d37d2e86`; final-head CI/release provenance and independent review remain required |
| Scope | PR #103: existing upgradeable FeeVault and #107 reference collector; accepted #105 complete-group cleanup, #108 dedicated optional history and prefix-bound original-row source identity; new networks only, no core or allocator changes |
| Base | `6c3106fb482bb323eefa8898a0bd922fc7520480` on `codex/fee-vault-upgradeable`; PR base `codex/mixed-direction-fees` at `641e768f2e6605f7563c808a00937d374e98e8fb` |
| Protocol baseline/current | `84a5b5207880f192a6a5896cbfe0bac5249db47f`; unchanged. The owner prohibits core-contract/ABI/public-input changes and pin changes to introduce allocation support |
| Environment | Rust 1.95.0, locked dependencies; LLVM 22; Foundry 1.7.1, Solidity 0.8.34, via IR, optimizer runs 1 |
| Isolation | `/home/edu/Development/rollupX/artifacts/fee-vault-upgradeable-20261026`; operator checkout, deployments and wallets untouched |

The [native execution proposal](../../proposals/native-system-transactions.md)
owns the accepted complete-group rule. The existing funding investigation at
`bf8a7011fd6658ef6a9c811f22a8f8b197b24a31` established independent canonical
table funding. Its eight real-core-contract cases are included as
`contracts/test/ProtocolGroupFunding.t.sol`; their `vm.deal` cleanup models do
not establish production hook placement.

The implementation shares a derived group cursor and balance-delta helper
between full block execution (including Stateless) and Composer incremental
execution. Snapshots restore the cursor with state and gas. Included revert/halt
results retain their normal receipt and gas while clearing protocol residue;
validation errors and discarded previews commit neither the cleanup nor the
cursor. No state change runs after checkpoint capture.

The fixed validation-contract version is 2, used in both capabilities and the
canonical on-chain profile preimage. Cross-chain envelope version 1 and prove.v2
remain unchanged. Frozen execution recordings must be recaptured for the final
combined execution rules; old recordings are not evidence for this change.

The tables below preserve the sequence of intermediate findings. Their earlier
“pending” entries are superseded by the completed milestone at the end of this
record, not claims that a failed or skipped check passed. That milestone still
precedes the newly authorized source-identity change.

## Iteration evidence

Logs are local-only under `artifacts/group-cleanup-20261027/` in the isolated
worktree. They are retained diagnostics, not portable CI artifacts.

| Check | Observed result |
| --- | --- |
| Initial `cargo check -p eez-evm -p eez-composer` | Missing `DatabaseCommit` trait import; corrected. |
| First EVM test compile | Two fixture type/conversion errors; corrected. |
| EVM unit execution | 40 passed, 1 old-policy balance assertion failed; all four new production executor group tests passed. Updated the intentional old post-inbound balance expectation from 13 to 0. |
| Composer initial unit execution | 411 passed, 2 failed, 1 ignored. Two native gas-budget fixtures represented dangling loads; changed those independent operations to checked inbound deliveries. |
| Composer new regression compile | Incorrect ABI struct and missing explicit fields; corrected. |
| Composer subsequent unit execution | 413 passed, 1 failed, 1 ignored. Manager code changed with the canonical profile; its exact bytecode recognition fingerprint required regeneration. |
| Integration contract suite | 164 passed, 1 stale generated runtime-fixture failure. Eight independent group-funding tests passed, including 256 fuzz cases. |
| Runtime-fixture focused rerun | Passed after regeneration from the completed Solidity build. |

These are development failures with code/fixture changes between runs, not
retry-only flakes. Final combined results, source/executable identities and
remaining limitations will be recorded before the PR update. No network has
been deployed or restarted by this work.

## Combined validation progress

The dedicated optional history type is `0x46`, version 1. The cross-chain
`0x45` envelope, source formulas, protobuf messages and protocol gitlink remain
unchanged. Genesis history slot 3 is the immutable first cutoff. All execution
paths check inherited PREVRANDAO even when an eligible empty block omits its
update. The shared runtime is generated from Solidity with the pinned compiler.

- The first production node/signer checks found a missing trait import and a
  generic provider transaction assumption; both were corrected. The same
  `cargo +1.95.0 check --locked -p eez-node -p eez-proof-signer` then passed.
- Contract validation initially found the reference collector test still
  submitting an empty update. It now models omission and resumption while
  preserving the exact lookup/polling assertions. The complete suite passed:
  **165 tests, zero failures/skips**, including the eight funding cases.
- The default-profile genesis-helper build was killed with SIGKILL during
  `eez-node` code generation. The host's cgroup recorded OOM kills; the machine
  also hosts unrelated running networks. Those resources were left untouched.
  An owned queued workspace build was interrupted before repeating the failure.
  A package-scoped local invocation with
  `--config 'profile.dev.package.eez-node.opt-level=0'` and
  `--config 'profile.dev.package.eez-node.debug=0'`, one Cargo job, built the
  current-source helper successfully. This changes only local test build
  settings; shipping configuration and dependencies are unchanged.
- `scripts/update-eezl2-genesis.sh` and `--check` passed using that exact helper.
  The E2E genesis state root is
  `0x1143e2e4dfe4f0b15fce2d7077ca6a641182d7b13b06f59e626759d08335c1bf`.
- The first combined Composer execution had eight failing fixtures that assumed
  a mandatory empty history transaction. They now supply a real available
  header at the eligible cutoff, preserving all receipt, gas, checkpoint and
  reverted-prefix assertions. The repeated focused command passed:
  **414 Composer, 41 EVM, 207 protocol, 16 primitives, 7 control-RPC tests**.
  The single ignored Composer test is its existing explicit profiling utility.

Fresh process recordings, complete signer/conformance replay, remaining
workspace checks and final review are still pending. These progress results
are not a completion claim or release-artifact evidence.

- Pinned core Foundry validation passed **778 tests**, with its three
  environment-dependent fork tests skipped. The required exclusions were
  `script/*`, `test_Doc_OriginalValue_SeedInSetup`, and
  `test_PostCost_Incremental`; these exclusions are not coverage.
- Tooling initially failed because several subprocesses use `/tmp` directly,
  where the host quota was exhausted. A first private `/tmp` mount lacked the
  device mount and failed on `/dev/null`. Both failure logs are retained. With
  an owned private `/tmp` and normal `/dev`, **418 tests completed successfully,
  nine skipped**: one real Docker payload image, two real Kurtosis probes, one
  native-archive relocation, three real runtime Docker builds, and two Gitleaks
  tests. These opt-in/tool availability skips are not coverage.
- The first fresh source-based bidirectional process test failed before the
  workload, while admitting an ordinary signed contract deployment. Composer's
  DA reservation still constructed the removed `0x45` history encoding, which
  canonical payload decoding correctly rejected. The reserve now uses a
  maximal `0x46` update. A focused regression covers its actual production
  calculation for all 1–64-hash counts and chain-ID encoding boundaries. The
  original process logs and datadir remain under the local evidence directory.
  The new production-reserve regression passed; process success after this
  correction remains pending.
- The optional standalone monitoring indexer was a further `0x45`-only
  receipt consumer. It now classifies `0x46` history gas as fee-free as well,
  preserving strict zero-price and complete-receipt checks. Its six focused
  fee tests passed, including mixed receipt kinds and checkpoint resumption.
  Standalone indexer Clippy passed with warnings denied.

## Requirement coverage under review

| Requirement | Owning implementation | Regression / remaining gate |
| --- | --- | --- |
| Derive complete groups; never clean between load/user, roots or callbacks | `eez-protocol::system_tx::ExecutionGroupState`, `eez-evm::block`, Composer `local/build` | Production EVM group tests and Composer fork/restore test passed; nested combined process gate pending. |
| Include cleanup in the final transaction delta and every permitted checkpoint | `eez-evm::balances`, both canonical and incremental executors | Independent manually calculated signer state-root oracle and cold follower comparison pending. |
| Preserve funding, failures, discarded previews, normal fees and donations | Same executor paths and existing committed-table funding | Four EVM group regressions and eight real-core funding cases passed. Partial-consumption process replay pending. |
| Optional canonical `0x46`, exact 1–64 hashes, no source identity | `eez-primitives::history`, shared enum/receipt/storage adapters, ordinary payload codec | Primitive tests passed; ordinary-column tests passed (16 unit, two integration). Fresh RPC/blob process evidence pending. |
| Fixed phase and delayed origin; omission inherits PREVRANDAO | `eez-protocol::l1_history`, `eez-evm::history`/`block`, L1 history contract | Omission/resumption, malformed placement and inherited-randomness tests passed; imported-history reorg and multipart processes pending. |
| One compatible producer/follower/offline validation contract | Composer/payload/pending producers, deriver, driver witness, independent signer | Shared tests passed; fresh captured offline transcripts, full signer tests and source-based combined process gate pending. |
| Authenticate changed execution rules without changing prove.v2/core | Fixed version 2 in canonical profile and portable validation identity | Solidity/Rust profile vectors and contract suites passed; refreshed portable execution corpus pending. |
| Preserve application getter behavior and new-network deployment boundary | History ABI, reference collector, generated runtime/genesis/profile, operator runbook | Collector and history contract tests, genesis regeneration/check passed. No deployment authorized or performed. |

## Complete PR acceptance and final-review matrix

This is the current acceptance matrix. The withdrawn core allocator and its
SI-1–SI-11 allocation gates are historical, not runtime requirements. The accepted
additional-field replacement is the [prefix-bound source contract](../../proposals/native-system-transactions.md#source-identity-simplification)
recorded in review `ebb31ca1`. Core contracts, their ABI/public-input algorithm
and gitlink remain unchanged. The intermediate #105/#108 commit is
`19843a670085fc7b012cb7b395088de26a56d5fd`; its process evidence must be renewed
for the final source-identity bytes.

| Accepted requirement | Implementation / owning contract | Executed evidence and remaining acceptance gate |
| --- | --- | --- |
| Upgradeable FeeVault, initialized genesis proxy, separate upgrade/treasury ownership, safe withdrawals and recipient changes | `contracts/src/L2/FeeVault.sol`, genesis renderer, `fee-vault-upgrades.md` | 165 contract tests and generated artifact check at unchanged Solidity/genesis code. Final-source treasury process passed: real credits, recipient rotation, implementation upgrade, withdrawal, offline settlement and cold follower replay. |
| #107 collector handles an ahead cursor with `current()`; strict getter/unexpected failures remain visible; no pause | `contracts/src/L1BlockHashCollector.sol` | Collector tests passed against actual history runtime, including omission/resumption. It remains a separately deployable application; no live collector was changed. |
| #105 complete independently funded groups, cleanup before checkpoints, discard/revert behavior | `ExecutionGroupState`, `clear_protocol_balances`, full and incremental executors | EVM 41, Composer 416, driver 74 passed. Eight real-core funding tests model cleanup with `vm.deal`; production EVM tests and the signer's independently assembled trie/checkpoint oracle validate the hook. Nested/multi-root and partial/restart source processes passed. |
| #108 optional nonempty `0x46`, fixed genesis phase, inherited PREVRANDAO on omission/empty blocks, fee-free typed receipts | Primitive codec, history contract/schedule/executor, all producers/consumers, indexer | Primitive 16, affected EVM/Composer/driver tests and 20 independent prefix replay tests passed. Earlier indexer six tests/Clippy and contract/genesis checks cover unchanged component code. History present/omitted combined source processes passed. |
| Delayed/late/missing L1, both anchor modes, bounded catch-up, ordinary L2 progress | History preparation, offline context, multipart/reorg scenarios | Final-source catch-up: 2,010 L2 blocks, 32 ordinary transactions, five cold offline parts (4 historical/1 actual-parent), 403 imported L1 headers; old source age 314 rejected by manager. Imported-history reorg preserved 20 ordinary transactions. This is not a four-day wall-clock benchmark. |
| Preserve RX-PROVER-01/02 observation-prefix and recursive-event rejection | Signer validated raw boundaries and outbound authorizer | All 329 signer tests passed, including prior observation/event regressions. Standalone seccomp-denied process passed six requests: 2 correct signatures and 4 earlier-state rejections. |
| Exact literal-string ABI source formula, original mutable rows including failed gaps and static-only outbound host | `native_source`, `ReconstructionActions`, portable encoding corpus | Protocol 209; manual ABI/Foundry cross-check of 18 encodings and 9 relationships; original-row/failed/static/append-stability tests. Static-callback, repeated-call and ordered inbound process cases passed. L1 static rows do not count. |
| Raw pre-group B[i], sequential construction without cyclic/folded roots or repeated earlier execution | Composer prefix cache/estimation/final rebuild, shared assembler, signer checkpoint lookup | Production-prefix regression checks row gaps, speculative discard, count rejection, no repeated transaction execution and finish equivalence. Exact raw-prefix/source rejection and real trie/process checks passed. |
| Same original bytes on partial reproof after an intervening same-block post; no sourceHash-based replay placement | Follower authenticated entry/final-root placement, exact byte compare, original selected rows | Deriver 46 and enhanced three-progressive-post process passed, with independent ABI oracle, stale-proof rejection and exact cold-follower transaction hashes. Boxed final test wrapper also passed (50.98 seconds). |
| v2 unchanged; no L1/L2 RPC for proving; independent context/root/bytes/effects/public-input checks | Signer, RPC and C#/Nethermind `IMPLEMENTING.md` | 3 fresh process captures, 329 signer/7 RPC/34 client tests, 29 real TCP transcripts, independent PI/rolling/KZG oracles passed. No new protobuf field or selectable profile. Actual C# execution is an external integration gate. |
| One fresh-network validation contract, unchanged core and dependencies | Version 2 bound by canonical on-chain profile/portable identity | Core pin stays clean at `84a5b520...`; core ABI/public-input and Cargo/schema files unchanged. Generated genesis/runtime and profile/identity vectors agree. Final shipping-artifact CI remains pending. |
| Queue ownership, closed retries and fee policy preserved | Existing admission/reconciliation; local-prefix failure has no user attribution | Partial/restart/native-reorg processes and Composer suites passed. Unsigned nested quotes, missing-refund continuation and mixed partial inclusion passed on Ethereum and Gnosis. |
| Scoped cleanup; no legacy production paths | Old source domains/positions/precomputed identities and old history encoding removed; docs/corpus migrated | Production callers traced. Unsigned fee probes retain a nonpublishable zero identity; Ethereum signed legacy transactions, Reth witness `Legacy`, and v1 native envelope/vector names remain current formats. Historical audits/superseded proposals are evidence, not runtime modes. Fee trace `source_hash` still identifies the signed user transaction. |
| Exact source/artifacts, final CI and independent correctness review | This audit and `source-index-validation.json` | Local source/runtime/harness/log identities retained; 27 distinct source-process scenarios passed. Full final-head release/Kurtosis CI and separate review remain required. No merge/deployment/restart authorized or performed. |

The final review must trace production callers, not infer cleanliness from a
build or a text search. Any retained item resembling an older implementation
must have a concrete current purpose. Findings require scoped fixes and renewed
affected checks; final source/core changes invalidate earlier candidate-specific
evidence. No PR-readiness conclusion has been made.

The second bidirectional capture run built the harness in 11m35s, then built the
standalone signer from source. Bidirectional execution, signing and posted-proof
assertions completed, but the new recorder failed on an unavailable debug RPC
(`Method not found`). This is **not a passing process result**. Capture now opts
into the existing test-only debug API and reports the specific failed RPC. The
first and second logs, captures, datadirs and source/artifact hashes are retained.

## Source-index scope correction

On 2026-10-27 the owner explicitly required that `eez-core-protocol` contracts
never be modified. No allocator implementation had started: the core submodule
remains clean at `84a5b5207880f192a6a5896cbfe0bac5249db47f`, with no new core
branch, ABI/public-input change or dependency PR. The implementation phase for
the proposed core allocator is stopped. A Composer-only counter or batch-local
index is not an authorized substitute for the block-wide requirement. Existing
source identities stay unchanged while a compatible design is evaluated.

The subsequent reassessment in review commit `141efbdf` selects no core or
RollupX-wrapper allocator. Existing source identity is an input identifier, and
current production authorization does not require an L1-allocated number. A
local original-stream ordinal would be a different scope from the selected
block-wide direction and has not been substituted. The pending SI design gates
do not block the separately authorized #105/#108 implementation or authorize
new contract state.

The third source-based bidirectional run passed: **1 test, 186 filtered**, in
293.03 seconds after source builds. It exercised real Composer execution,
network-denied offline signing, L1 settlement and bidirectional state assertions.
Fresh raw blocks, receipts, witness and deployed profile were captured; canonical
`L2ExecutionPerformed` events supply the independent accepted checkpoint pins.
The full-block fixture has `[load, signed user, inbound]`; the separate history
fixture contains the dedicated `0x46` update. Original failures remain retained.
The other two semantic captures and replay/conformance checks are still pending.

## Current-source replay milestone

All three native captures passed with signer networking denied: bidirectional
(293.03 seconds including its signer build), caught-revert lookup (324.71 seconds
including a feature-driven signer rebuild), and changing static/write with
follower replay (40.47 seconds after the build was warm). The strict inventory
utility selected these complete cold requests, all starting at L2 block 1:

| Fixture | Terminal block | Request Keccak |
| --- | --- | --- |
| Bidirectional | 285 | `e9f2e86ff19b888d1bc0a3e04ac7471ca43ad05b0d8c74b9d24c0932451bd888` |
| Caught-revert lookup | 305 | `363363fa3be7e8430ca2fea5932e45a58df82673eff88eb2f71ad1b890aa105e` |
| Changing static/write | 30 | `bdb1465374a44305b22bfc44d0c7dc5cb0201b4843202e44272d90ace27d7f7d` |

After regenerating the six observation requests and all 29 portable transcript
cases, `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc
-p eez-prover-client -- --test-threads=2` passed: **329 signer, seven control-RPC,
34 prover-client tests**. The signer's three ignored tests are explicit fixture
regeneration/inventory utilities, separately executed here. Empty doc-test
selections add no behavioral coverage. The Python manifest checker also passed.
This includes all RX-PROVER-01/02 signing/event regressions, complete cold replay,
all 29 portable TCP cases, and the independently assembled group-cleanup trie
root/checkpoint oracle; none is inferred from a build alone.

Extended process tests pin the current source-built signer copied to the owned
`verified-runtime/` directory, with Composer, follower and harness SHA256 values
checked before every scenario. This avoids repeated Cargo feature rebuilds while
retaining exact source-artifact identity; it is not a CI release-artifact claim.
The initial three captures built their signer from source with stale/prebuilt
selectors cleared. Completed extended scenarios so far include two Composers and
two keyless followers (including cold replay), scheduled-history static/lookup
settlement, and bidirectional settlement with history in Sync. The remaining
serial cases and final checks are still running.

The first prefix-example command omitted its required `chain-spec` feature and
ran no tests. That invocation is retained; the corrected command explicitly
selects the feature and remains a separate gate.

## Next authorized source-identity phase

Review commit `ebb31ca1` replaces the withdrawn allocation proposal with the
[source-identity simplification](../../proposals/native-system-transactions.md#source-identity-simplification):
one dynamic-string ABI domain, rollup, authenticated L1 parent, original
batch-local mutable-row index and the independently reconstructed pre-group
prefix hash. The owner authorized this additional-field fallback. No allocator,
core/ABI/public-input change, gitlink change or new RPC is allowed. This phase
starts after the running #105/#108 milestone; existing source bytes are unchanged
in the results above. It requires its own implementation, fresh captures and
renewed affected validation before final PR readiness.

## Completed #105/#108 milestone

All 14 scenarios in `combined-process-results.tsv` passed with nonzero exact
selection, one serial test each, zero failures and zero ignored tests. They
covered two Composers and two keyless followers (one cold), scheduled-history
static and lookup observations, history inside bidirectional Sync, multi-root
outbound calls and nested callbacks, first/middle/last partial consumption,
restart after partial consumption, same-L1-block checkpoint reproof, mixed
execution directions and imported-history reorg recovery. The expired-source
scenario executed 2,025 L2 blocks, 32 ordinary transactions and five catch-up
parts (four historical anchors and one actual-parent anchor), imported 406 L1
headers and proved sources 316 blocks old with cold network-denied signers and
public follower replay. This exercises the unavailable-L1-`blockhash` boundary;
it does not claim a four-day load benchmark or production deployment.

The separate source-process history-observation signing regression passed
(1 selected, 186 filtered), preserving correct-prefix acceptance and both
incorrect earlier-state rejections. The independent prefix example passed all
20 tests with its required `chain-spec` feature, including exact canonical
L1-accepted checkpoint pins; the earlier feature-omission failure is retained.
The final runtime-adapter/oracle command passed: blob codec 21 plus one Ethereum
KZG compatibility oracle, deriver 46, driver 65, L1 200, node-common 15, protocol
207, causal evidence 10, public-input vectors 3 and rolling-hash vectors 2.

The checked-in [validation summary](validation.json) records the commands,
nonzero selections, local log digests and exact runtime artifact SHA256 values.
The logs/datadirs and binaries themselves remain local-only. The fixtures and
portable transcripts checked into this milestone allow independent replay.

Still required after the approved source-identity implementation: refreshed
source captures and portable transcripts for its changed native bytes, affected
unit/process checks, workspace Clippy with warnings denied, final shipping/CI
matrix, scoped no-legacy cleanup and a fresh independent correctness review.
Passing this intermediate milestone does not make the complete PR ready.

## Prefix-bound source implementation progress

The production paths now derive original mutable settlement rows once from the
complete semantic stream. Native plans contain call/value/chain fields only;
source identity and remaining gas are supplied at the actual pre-group position.
Composer's existing prefix cache supports sequential resolution and a separate
fresh final rebuild. The follower uses the pinned assembler and executed state
without reexecuting preceding groups at each identity, and authenticates append
versus already-applied placement using the settlement entry/final checkpoint.
The signer uses its independently replayed raw ordinary/group-prefix hashes,
never folded root carriers or a request-supplied index. Exact byte/effect checks
remain mandatory.

Current shared suites passed: **416 Composer, 46 deriver, 74 driver, 41 EVM,
16 primitives and 209 protocol tests**. One existing Composer profiling utility
remains explicitly ignored. First failures are retained: missing API docs/imports,
opaque Reth builder access during migration, and four driver witness tests whose
shared fixture still expected an empty scheduled history transaction. The fixture
now includes a produced eligible L1 header and the complete suite passes; no
assertion or execution check was removed. The source-identity process harness and
signer fixtures are being rebuilt. Intermediate frozen transcripts remain
historical until recaptured; they are not claimed as current-source evidence.

The fresh bidirectional, caught-revert and changing-static captures passed with
the current source runtimes and network-denied signers. Their complete cold
requests cover 440, 45 and 30 L2 blocks respectively. The existing inventory
utility checked the selected semantics before installation; all 29 portable
transcripts and the six observation requests were regenerated, and the Python
manifest/schema check passed. Eleven focused signer DA tests passed, including
wrong original row, wrong raw prefix, wrong gas and whole-group reconstruction.

The first enhanced same-block process failed a new test expectation that a
no-progress immediate posting would succeed. Pinned core explicitly rejects an
all-failed leading run with `AllImmediateL2TxsFailed`; its implementation and
specification were retained. The corrected scenario creates two conditional
checkpoint reproofs and three successful postings, advancing exactly one
original user group at a time, with an intervening accepted posting and an
independently rejected stale proof. No production validation changed to satisfy
this scenario. Its first log/datadir and binary identities are retained; the
corrected process passed (51.40 seconds, one selected/186 filtered); it is
not a retry-only flake.


The first complete source-index signer run passed 324 tests but rejected five
synthetic service cases. Their test transactions used a zero pre-group prefix,
whereas the controlled validated evidence explicitly pinned `0x04…04`. The
three fixture constructors now use that same declared original prefix. All 81
focused service tests then passed (two explicit fixture utilities ignored).
Production validation and source-artifact bytes did not change for this fix;
the original full-suite failure log remains retained.


The complete refreshed signer/control/client rerun passed **329/7/34 tests**.
All 29 TCP conformance transcripts and the four audited history-observation
signing cases passed. The three ignored signer entries are explicit generation
and inventory utilities, already executed separately. The first workspace Clippy
run found the enhanced same-block test's 18 KB future; its test wrapper now boxes
the future instead of suppressing the lint. No production behavior changed.


The renewed prefix example initially passed 19 of 20 tests; its explicit captured
height assertion still named the prior recording (285/283). Foundry RLP decoding
confirmed the new captures are blocks 440/438. Updating those fixture expectations
preserved all independently recorded L1 checkpoint checks; all 20 tests then
passed. Workspace Clippy subsequently passed with warnings denied. These are
fixture/lint corrections with retained first failures, not retry-only passes.


All **17 selected combined source-process scenarios passed** with the frozen
final production runtimes: whole/nested groups, inherited and imported history,
source omission, same-block original-byte reproof, partial restart, native/history
reorg, repeated calls, ordered inbound roots and multipart catch-up. The renewed
catch-up covered 2,010 L2 blocks and 32 ordinary transactions in five separately
proven/settled parts (four historical, one actual-parent), importing 403 L1
headers. Its first source was 314 blocks old and the manager rejected that expired
anchor before the valid recent-anchor flow. This proves the inaccessible-blockhash
and segmented replay mechanism, not a four-day wall-clock load benchmark.

Independent public-input (3), rolling-hash (2), causal-evidence (10), blob codec
(21), Ethereum KZG (1), and payload codec (16 unit plus 2 integration) tests passed.
Signer all-target/all-feature Clippy and warnings-denied documentation also
passed. Seven further fee/treasury/static-only/observation process scenarios and
the boxed same-block test-wrapper rerun are selected; these are still pending.


## Final local validation and review

The complete implementation is `d37d2e86481ca18153f23ebf859a9b48566431a7`, with
unchanged core `84a5b5207880f192a6a5896cbfe0bac5249db47f`. All seven additional
process scenarios passed: unsigned nested quoting, service after missing L1
refunds, mixed partial inclusion on Ethereum and Gnosis, treasury
upgrade/withdrawal/cold replay, static callbacks and the standalone audited
history-observation signing boundary. The enhanced same-block test also passed
after its wrapper was boxed. Together with the three captures, **27 distinct
source-process scenarios passed**; the extra wrapper rerun is not counted as
another distinct scenario. One earlier failed test expectation remains recorded.

[Source-validation evidence](source-index-validation.json) records exact binaries,
fixture/log hashes, selected tests, first failures and limits. The process build
uses the existing E2E/signer package union and one Cargo job. An accidental
E2E-only rebuild selected a different dependency feature union; only that owned
build was interrupted, and its log is retained. The completed union rebuild took
1m49s; the harness path was read from Cargo's output and its digest checked before
each test. No result is claimed from the interrupted build. The local node
package optimization/debug overrides address this host's memory limit; they are
not shipping release settings or changed repository configuration.

The fresh correctness read traced complete-group funding and native failure,
provisional delta/cursor rollback, account deletion and FeeVault credits, history
omission and canonical bounded decoding, original-row mapping before filtering,
raw-prefix sequencing, final reconstruction, checkpoint reproof, follower
placement, independent signer authorization and public-input binding. No further
runtime correction was identified. Stale synthetic prefixes/captured heights and
the test-future lint were corrected without weakening any assertion or production
gate; a tautological source-hash assertion was removed while retaining the manual
ABI oracle and actual reproof process. The core, wire schema and dependency pin
were checked again. Historical superseded design records remain clearly marked;
no prior history/source algorithm is selectable in production.

The independent reviewer has been given the exact candidate, complete requirement
matrix and owning specifications. This implementation read does not substitute
for that review. Final-head CI, shipping source/artifact provenance and any review
findings remain acceptance gates. No live deployment or migration was performed.
