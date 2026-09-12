# Partial consumption implementation and verification

Historical implementation and validation report. The
[current scope and validation](default-policy.md) supersede the journal and
opt-in policy described below. Partial consumption is always enabled and
submitted attempts are tracked in memory.

Worktree: `/tmp/eez-partial-consumption`

Branch: `fix/partial-consumption`

Implementation base: `40134955dc3f698ea65571ba82b6521637bf1191` (`main`)

Protocol: `3c0cdf4955941a5c23639346932b55b33babebbd`

The original upstream investigation and ordered P0–P6 plan are in [README.md](README.md).
Its earlier baseline is retained as historical evidence, not the implementation base.

## Changes and decisions

| Plan | Implementation |
| --- | --- |
| P0 | Reproduced height-based reservation loss before replacing the ledger. Deterministic tests force observer-first, deriver-first and recovery-entered orders, stale callbacks, and duplicate resolution. The historical diagnostic now explicitly reads its pinned pre-fix revision. |
| P1 | `eez-protocol::settlement_manifest` retains exact signed identities, source chain/nonce, FIFO order, retry counters, resolved semantic forests, whole outbound groups, mutable/static source-table positions and physical checkpoint endpoints. It validates complete ownership and group coverage using existing materializers. No consensus/DA field was added. |
| P2 | The deriver publishes batch-specific canonical L1 identity, calldata hash, applied virtual slice, replay selection and exact L2 endpoint before advancing the scheduling cursor. `ScanComplete` gates ownership until the whole canonical block is processed. Both height-only completion shortcuts were removed. Empty outcomes are explicit, including zero-block payloads with no applied effects; applied effects without block metadata fail closed. |
| P3 | Observation records inclusion/exclusion/uncertainty but cannot complete users. Reconciliation checks canonical source receipts and hash-pinned nonces; outbound nonces come from the deriver's canonical L2 endpoint. Mutable inbound consumption is scoped to its batch window, call hash and queue index. Included reverts and caught failures terminate spent source bytes; eligible omissions requeue once. |
| P4 | A checksummed, exclusively locked, versioned journal is atomically replaced and fsynced before dispatch or ownership transfer. It preserves raw users, manifests, attempts and actual targets across restart, including retry composition. Reorgs invalidate by L1 identity; old attempts cannot reclaim newer ownership. Finality audits retain records on uncertainty. Partial mode requires durable storage. |
| P5 | `atomic` remains the default. `allow-inbound-reverts` allows only actual signed L1 users after the required postBatch. Chain/signature/duplicate checks protect membership; sidecars and timestamp pins remain intact. Partial and user-bearing submissions require a bundle relay. No dropping policy was introduced. |
| P6 | Real contract/signer/native-blob omission tests, a valid same-block resumption fixture, a fresh follower, an immutable-builder Kurtosis profile/canary, CI selection and operator documentation. Integration results are recorded below. |

The journal intentionally covers submitted users and their subsequent recovery
lifecycle. It does not add durability to ingress that has never been submitted.
On disk failure the node stops ownership mutation/dispatch; it does not continue
with an in-memory substitute. Recovered users retain the existing retry budget,
fee-replacement rules, funding deferral and sender/direction nonce isolation.

Source completion is distinct from successful execution of every proposed call.
Static-only successful receipts are recorded as `SourceCompleted`; successful
sources missing required mutable effects are
`SourceCompletedWithMissingEffects`. Both spent nonces are terminal. A missing
receipt with a canonically advanced nonce is `SourceNonceSpent`, without claiming
to know the replacement's identity. Infrastructure failures do not consume a
user's retry budget.

## Protocol and builder sources verified

- Pinned [core specification](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md),
  sections B.1, D.1, G.4–G.5 and H.1/H.3/H.6–H.8; the pinned static-entry
  definitions, EEZ/EEZL2 implementations and interfaces.
- [EEZ.sol](../../../eez-core-protocol/src/EEZ.sol): `postAndVerifyBatch` verifies
  the complete proofs atomically, resets verified rollups' queues, attempts each
  immediate entry separately, emits `L2TxSkipped` on caught failures, and reverts
  the entire post if every immediate fails. `_executeEntry` rolls root updates,
  logs, value and queue state back on a failed frame. `_findMatchingEntry` scans
  forward against call identity and live roots.
- [Local carrier construction](../../../crates/eez-protocol/src/settlement.rs),
  `settlement_root_carriers`, assigns failed proven effects the preceding
  carrier's exact commitment. It does not authorize dynamically skipped,
  root-changing entries to be absorbed into that carrier.
- [Blob format](../../../eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md) and the current payload
  codec: protocol-native authenticated blobs retain their current self-contained
  block/group representation. Local attempt metadata is not follower input.
- [Flashbots bundle RPC](https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint):
  `revertingTxHashes` permits EVM reverts. Invalid-transaction omission is a
  separate builder capability.
- rbuilder image
  `ethpandaops/reth-rbuilder@sha256:916ba84da2690e3e368f57da5b0c73bfd919b452de8b95f64895846e0f3a8308`,
  source commit `6037fa728b13bf1806e16fff2586414216f6b8fa`.
  [Order execution](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/building/order_commit.rs),
  [bundle tests](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/building/testing/bundle_tests/mod.rs),
  and [configuration](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/docs/CONFIG.md)
  were inspected. The running `mgp-ordering` and `mp-ordering` configurations
  had `discard_txs = true`. The real canary verified omission of an invalid
  allowed nonce and inclusion of a later reverted source transaction.

The embedded native test relay controls inclusion and ignores revert allowlists.
It validates the node's accounting/replay but does not stand in for builder
permission semantics. The Kurtosis canary uses the actual pinned builder.

## Verification record

All Cargo commands used the lockfile and offline dependency cache. Executable
tests used Rust 1.96 with the repository's optimized test profile, debug symbols
disabled and incremental compilation disabled. Clippy used the repository's CI
toolchain, Rust 1.94.0. No dependency versions changed.

| Check | Actual result |
| --- | --- |
| Original optimistic ledger plus new partial-loss regression | Existing 8 tests passed; new regression failed before the replacement. |
| Focused core tests after scanner/source-read review | 367 passed: composer 70, deriver 45, L1 87, protocol 165, including finality cleanup, reorg reservations and resumed authorization. |
| Test-framework unit tests (`--features alloy-eips/arbitrary`, matching workspace feature unification) | 8 passed, including the independent weighted event oracle. |
| CI Clippy `--workspace --all-targets -- -D warnings` | Passed on Rust 1.94.0. |
| `cargo fmt --all --check`; `git diff --check` | Passed, including the final staged diff. |
| CI Python scripts | 178 tests, 174 passed and 4 explicitly skipped; no failures after updating the native target inventory. |
| Chiado blob configuration; changed Python/shell syntax | Passed. |
| `cargo deny --offline check advisories bans sources` | Passed. |
| `cargo audit --no-fetch` | Passed against the cached 1,243-advisory database, with 12 existing allowed warnings. This was not an online advisory refresh. |
| Full pinned protocol Forge suite | 410 passed; no failures or skips. |
| Full task contract Forge suite | 19 passed; no failures or skips. |
| Final native partial-consumption suite | All 4 passed in 374.98 s: first/middle/last omissions and positive same-block resumption, using final node/follower binaries and real proof/blob data. |
| Real-builder happy wave | Passed: four cross-chain operations, two posts, proof verification and continued safe-head progress. |
| Final real-builder state-chaining suite | All 9 scenarios passed, including both directions, caught destination failures, poison recovery, mixed directions and repeated calls in one source transaction. |
| Final real-builder nonce-replacement canary | Passed with final node and signer binary hashes verified; canonical evidence below. |
| Existing devnet clean-restart recovery script | Passed: prior settled L2 hash and both correlation directions preserved; current safe-head/registry convergence verified. RPC recovered in 10 s, correlations in 24 s. |
| Final native chained-state suite | All 5 passed in 419.05 s. |
| Final native cross-chain suite | All 22 passed: focused lookup regression in 111.09 s and the remaining 21 in 1660.06 s. |
| Final native negative resumption | 1 passed in 75.27 s; malformed-sidecar rejection and recovery retained. |
| Final native L1 reorg regressions | All 3 passed in 165.82 s: blob correlations, follower recovery and two-composer recovery. |

Verification logs are retained in the worktree's ignored `target/` directory.
The initial native omission run failed because the Beacon fixture encountered a
test control file in its blob directory; controls now live in a separate
subdirectory. The initial happy Kurtosis check expected the obsolete observer
boolean; both affected devnet scripts now check the authoritative deriver safe-head
event and passed on rerun. Neither failure was treated as a pass.

The first valid resumption run **failed** after real L1 prefix and suffix
execution. It exposed a scanner defect: `L2TxSkipped` was treated as a proven
carrier outcome regardless of its root. This selected all outbound users for
the first post while L1 committed only the first user's prefix. Attribution now
requires equal commitments before expanding a carrier; root-changing skips
remain boundaries. A second run then exposed an authorization scope defect:
the resumed table was checked against all events in the rebuilt block, including
the already validated prefix. The gate now checks the appended transaction
receipts, also selecting that suffix on idempotent replay. It still rejects
missing, extra and duplicate calls; an invalid receipt boundary fails closed.
All 45 deriver unit tests passed after this correction. The valid fixture then
passed with both corrections: prefix and resumed posts committed in one L1 block;
all three users completed once with zero retries; a fresh follower matched the
full commitment. The final broader run also includes the later reservation fix.

The final review also corrected two failure paths: a receipt ahead of the
completed scan is inconclusive, and a journal failure after candidate
classification retains only surviving candidates instead of resurrecting the
original drain's discarded users. Applied payloads without block metadata
are rejected; conclusively empty outcomes are published even for zero blocks.
Reorg invalidation now reopens journal ownership and restores pool reservations
under one lock. The resolver checks canonical generation under that same lock,
so a stale in-progress decision cannot erase an already observed reorg. A final
cleanup correction removes orphaned historical records after all their users
have transferred to newer attempts (or reached local terminal outcomes); reopened
canonical users remain ineligible. This prevents retaining superseded blob
envelopes forever after a reorg. Its ownership regression and all core tests pass.

The broader cross-chain run exposed a third attribution defect in the caught
inbound-revert case. An authenticated inbound lookup was counted as a producing
L2 effect. The stricter replay bounds rejected the mismatch rather than silently
clamping it. Scanner projection now uses the existing exact lookup definition
(proxy identity, empty destination trace, zero-value no-op root, and recomputed
rolling seed). It preserves event positions with zero physical weight. A
successful lookup may emit a no-op log or lose it when its source frame reverts;
matching permits either while retaining order and multiplicity for producing
carriers. Tests cover omitted/persisted lookup logs before and after a physical
effect, prefix and resumed settlement, malformed masks, excess logs, and gaps
between producing carriers. All 366 core tests and CI Clippy pass after this fix.
The next native run reached the correct L2 safe hash but failed the independent
test framework's event oracle, which retained the old physical-step assumption.
The oracle now enumerates valid weighted fixture ranges independently of the
production scanner, permits optional lookup logs, and rejects inflated counts,
missing physical events, and dynamically skipped root-changing checkpoints.

A final transport review found that the new source-recovery calls used an
unbounded default provider. A real stalled-header regression failed before the
correction. Canonical source/target providers now follow the existing L1 reader's
bounded HTTP-client construction with the submitter's 30-second timeout policy.
Both stalled headers and stalled response bodies return an inconclusive provider
error. Client construction no longer substitutes an unbounded dispatch client.
All 367 core tests and workspace Clippy pass with this correction.

One core-suite invocation lacked permission to open local fixture sockets and
failed four existing RPC tests with `PermissionDenied`. It passed on rerun with local
socket access; that environmental failure is not counted as successful testing.

### Final real-builder canary evidence

Private enclave: `eez-partial-consumption`; existing `eez-devnet` was not changed.
After the private checks completed, the task enclave was stopped with its data
retained. Native integration tests run independently of that enclave.
The node was restarted with its journal preserved (generation 676 before restart,
678 at the verification read). Docker reassigned host ports, so readiness was
verified after rediscovering the RPC endpoint. A clean restart is not a kill-9
crash-injection test.

- L1 post: `0x6b212010ffba4d7db9d76b96cb58d416ac666f00b18508ba63d7f7f52ba2bb1f`.
- L1 block 728: `0xcd886079daedb16f1ebdd665c53c92dbf615407d72e2e51afb4331e8bdbe81f5`.
- L2 height 4362 partial commitment:
  `0xc0d9681995284618ae93815725e45d5bd18b793b62f6b30c1ba630e92b7d3916`.
- Proposed full commitment:
  `0x2abaa8301d560575f07caf4b90c94ad485ce40c1f4f52218e15d1a4cb340171c`.
- User outcomes: `[Committed, SourceNonceSpent, SourceReverted]`; retry counts
  `[0, 0, 0]`.
- Healthy subsequent source:
  `0x3ad8c3f7635f7820e19496714e683df340d3c229dc17baf9fe1200305966271b`.
- Safe head advanced to 4368,
  `0xf3427e220bcf8c065705e97b0decc8c90b629b4376255187c13ebfa85fefa107`.

The [machine-readable evidence](validation-evidence.json) records all user and
replacement hashes, immutable builder/source identity, generated builder config
hash, local image IDs and SHA-256 hashes of the running node and signer binaries.
They match the binaries used by the final native suites. The task containers were
updated with those exact binaries and restarted; local task images were rebuilt.
Their original container base-image IDs are recorded separately. These private
validation images use bundled host runtime libraries; the production Docker image
build was not part of this check.

## Rollout and limits

Drain pre-upgrade in-memory attempts. Enable partial submission only with a
durable datadir and a builder whose semantics have been validated. Preserve the
journal when restarting or setting the policy back to `atomic`; disabling new
partial requests must not disable reconciliation of existing attempts. Do not
delete a damaged journal to resume dispatch without an ownership audit.

The canary is an isolated private devnet run, not a public-chain deployment or
validation of arbitrary relay images. No upstream comments or PRs were posted.
Missing canonical source data remains inconclusive and intentionally keeps
ownership reserved. Resending an expired proved postBatch is never recovery;
eligible users are recomposed against fresh state with fresh proof and DA.

Source-state reads are sequential. Code review identified a liveness constraint:
if reads consistently span a new canonical generation, recovery can remain
pending despite successful individual RPC calls. Size
`EEZ_MAX_USER_TXS_PER_BUNDLE` (default 50) to the RPC latency and monitor the
settlement backlog. The private validation did not load-test the maximum batch
size through a high-latency remote RPC. Ownership remains reserved in this case.

All 35 final native tests passed. Their node, follower and signer binary hashes
match the completed private-builder canary and restart check. The final
machine-readable evidence includes every suite's exit status and elapsed time.

## Final critical review

The complete task diff was reviewed against
`40134955dc3f698ea65571ba82b6521637bf1191`, including every added file. The changes
are confined to P0–P6 implementation, regression coverage, CI selection and
operator/audit documentation. The protocol submodule and nested dependency
revisions are unchanged; the original worktree's tracked source is untouched.

The review/test cycle corrected dynamic-skip and lookup attribution, resumed
authorization scope, journal failure recovery, reorg reservation races, finality
history cleanup and unbounded source HTTP requests, as detailed above. The
independent test oracle was corrected without relaxing physical-effect checks.
No further ownership, protocol or authorization defect was found in the final
review. The sequential-RPC liveness constraint remains an explicit operational
limitation; it was not load-tested on a high-latency remote endpoint.

## Acceptance coverage

This maps requirements to executable checks; final native results are recorded
in the verification table above. Unit crash tests reopen the real journal at
ownership boundaries; they are not claims of process-kill testing at every
boundary.

| Scenario | Coverage |
| --- | --- |
| Full consumption | Native bidirectional and repeated-wave tests; exact-root real-builder happy wave. |
| First/middle/last failure | Three `partial_consumption` omission fixtures, each after real proving; exact prefix, dispositions, retries and healthy traffic. |
| Anchor only / nothing applied | First-source omission; scanner empty/no-op attribution; canonical-head publication and evidence-gating tests. |
| Inbound dropped | Omission fixtures retain only the reusable source, FIFO and one attempt charge; durable ownership unit tests. |
| Inbound included/reverted | Native omissions and real-builder canary verify reverted receipts terminate without a retry. |
| Successful source after caught failure | `included_revert_and_caught_failure_spend_nonce_without_retry`; native caught-destination-result tests in both directions. |
| Nonce replaced/invalid | Real pinned-builder canary replaces the middle nonce after proving and verifies the original's terminal disposition and subsequent progress. |
| Outbound skipped | Scanner first/middle/last vectors and valid native prefix/resumption using unchanged proof/blob calldata. |
| Multi-root outbound | Manifest whole-group tests, `replay_selection_requires_whole_outbound_groups_and_bounded_effects`, native multiple-proxy-call test. |
| Multi-action inbound | Manifest FIFO mutable/static/lookup tests; native repeated-wave and nested-call fixtures. |
| Expected/caught destination revert | Protocol carrier and scanner equal-commitment regression tests; native caught-revert tests. |
| Mixed directions / same EOA | Manifest FIFO test, held-pool direction/sender isolation, native mixed-direction and same-sender chains. |
| Ordinary Sync-tail users | Existing `tail_truncation_boundary_must_skip_the_unconsumed_entry_too` regression. This composer emits cross-chain/system transactions in rich Sync blocks; retained ordinary-tail compatibility remains subject to the exact L1 commitment check. |
| Observer/deriver/recovery ordering | Barrier-controlled ledger tests and recovered-tail idempotence; runtime first/middle/last omissions preserve the prefix. |
| Stale observer after replacement | `duplicate_resolution_does_not_release_a_new_attempt_or_charge_again` and exact attempt identity tests. |
| Same-block batches / mid-chain resume | Exact source-window unit test; positive valid-blob resumption with two real posts, system nonce reconstruction and a clean follower; existing malformed-sidecar negative test retained. |
| Competing same-height batch | No height-based completion; canonical-head same-height evidence tests; native second-poster resumption resolves users by canonical L2 inclusion. |
| Duplicates / lag / RPC failures | Duplicate publication/resolution tests; canonical-source mock RPC errors/reorgs/future receipts; complete-scan generation barrier. |
| Partial then reorg / same-height replacement | `reorg_at_same_l2_height_reopens_only_canonical_owned_users` and `reorg_restores_reservations_and_rejects_a_stale_ownership_decision`. |
| Restart at ownership boundaries | `durable_recovery_survives_dispatch_transfer_composition_and_new_attempt`, stale-recomposition/finality tests, corruption/locking/write-error tests; final devnet restart preserves the journal. |
| Native blob + clean follower | All omission fixtures and the valid resumption fixture start a follower using only L1 and authenticated blob availability. |
