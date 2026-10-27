# Complete-group cleanup and optional history updates

| Field | Value |
| --- | --- |
| Status | #105/#108 local execution, process and conformance milestone passed; approved source-identity phase and final-head checks pending |
| Scope | PR #103: existing upgradeable FeeVault and #107 reference collector; accepted #105 complete-group cleanup and #108 dedicated optional history; new networks only. Prefix-bound source identity is the next authorized phase; no core or allocator changes |
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

This matrix distinguishes accepted behavior from implementation and evidence.
The source-index design recorded in review commit
`2788499d14e8da202c09c0a057b5ac438fd0c62c` is a historical design direction.
The owner subsequently prohibited all core-contract, ABI and public-input
changes. That overrides the earlier allocator authorization: its implementation
is stopped, the protocol pin remains unchanged, and existing production source
identities remain in place. No core edits, branch or dependency PR were made.
The SI rows below record the unresolved design requirements, not work currently
authorized for this PR; a compatible design requires separate resolution. The
final #105/#108 candidate still requires fresh correctness review and final-head CI.

| Accepted requirement | Implementation / owning contract | Executed evidence and remaining acceptance gate |
| --- | --- | --- |
| Upgradeable FeeVault, initialized genesis proxy, separate upgrade/treasury ownership, safe withdrawals and recipient changes | `contracts/src/L2/FeeVault.sol`, genesis renderer and `docs/proposals/fee-vault-upgrades.md` | Existing contract suite passed on the current pin; final generated genesis, upgrade/storage and fee-credit process checks must match the final pin. |
| #107 application handles an ahead cursor using `current()`; strict getter and unexpected failures remain visible; no production pause | `contracts/src/L1BlockHashCollector.sol` and its contract tests | Collector contract tests passed, including omitted history and resumed imports. This reference collector does not change network scheduling. |
| #105 independently funded whole groups, cleanup before checkpoints, identical full/incremental/stateless execution | `ExecutionGroupState`, executor balance deltas, Composer snapshot/restore | Production EVM and Composer regressions passed. Independent signer roots, nested process, partial settlement, live/cold follower and fee-credit checks pending. |
| #108 only canonical nonempty `0x46`, immutable genesis schedule, omission inherits history/PREVRANDAO including empty blocks | Primitive codec, shared history schedule, EVM, Composer, deriver, signer | Focused shared/contract tests passed. Fresh source captures and combined replay remain pending. Old `0x45` history is rejected, not a compatibility mode. |
| Delayed/late/missing L1 facts, both anchor modes and bounded multipart catch-up; ordinary L2 keeps progressing | History preparation/execution, offline context validation, source process scenarios | Unit boundary tests passed; multipart expired-anchor, reorg, pending, omission and source process evidence pending. |
| Every native consumer recognizes both native kinds; pure ingress and DA reservations remain valid | Payload codec, production sequencing reserve, receipts, fee tests and optional indexer | Reserve regression, payload 16+2 tests, indexer six tests and Clippy passed. Initial source-process admission failure retained; corrected process replay pending. |
| RX-PROVER-01/02 exact observation prefix and recursive event partitioning remain enforced | Signer validated boundaries and outbound authorization | Earlier audit evidence is historical. Updated history recordings and positive/rejection signing-gate/process tests pending for the final semantics. |
| Source-identity direction and SI-1 through SI-11 | Pending design in the accepted source-index amendment; existing production identity formulas remain active | No allocator, core change or local-index substitute is selected. These gates do not block #105/#108. The old proposal's allocation/ABI design is withdrawn, not implemented or counted as coverage. |
| Offline prover has no L1/L2 RPC and does not trust Composer claims or broaden its supported profile | Signer admission/execution/effect/public-input boundaries and portable C#/Nethermind contract | Source signer network-denial capture is in progress. Final portable vectors and malformed-input signing rejection remain pending. |
| One fresh-network profile, no obsolete runtime, codec or config path; no incidental dependency change | Owning native/history/core/RPC/signer specs and all callers | Final scoped cleanup/dead-code review pending, including generated files, fixtures and runtime config consumers. Negative obsolete-format tests and clearly dated evidence remain intentional. |
| Queue ownership, closed retries and partial canonical consumption unchanged | Settlement policy, Composer/deriver reconciliation | Targeted partial/reorg/restart processes pending; a proof, receipt or relay acceptance alone must not finish retained work. |
| Exact tested candidate/artifacts, final-head CI and independent final correctness review | This audit and PR description | Pending. Earlier failures/logs stay retained; skips/mocks are explicitly not live/process coverage. No merge or deployment is authorized. |

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
