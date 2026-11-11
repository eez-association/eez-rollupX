# Late-original settlement status correction

Date: 2026-11-10. Investigation and PR validation; no deployment of this fix.
Baseline: `7ea2a50edbd3304f80718304ab4f4f871b22026c` in
`eez-association/eez-rollupX`. Protocol pin unchanged:
`84a5b5207880f192a6a5896cbfe0bac5249db47f`.
The deployed node image used for read-only diagnosis was
`ghcr.io/eez-association/eez-rollupx-node@sha256:71755407f30d1c3865abd5bef9d99b6fe58d32fb4ee5a17abe3e945cc37372b1`.

## Findings

[Sanitized evidence](evidence.json) records the source, delivery, both batches
and UTC log sequence. The canonical batch has a successful L1 receipt at index
zero; the source succeeds immediately after it. The settlement lookup maps the
successful L2 delivery block to that batch. Composer and an independent follower
both return the successful delivery receipt. The reported replacement batch has
no receipt. This incident is incorrect ownership/status correlation, not missing
L2 execution. These checks do not constitute a general prover soundness audit.

The original was already dispatched when a deadline allowed recomposition.
Ownership moved to generation 66 before generation 65's inclusion was observed.
`resolve` ignored the original's validated completion because it no longer owned
the source. The replacement's missing-effect outcome and candidate references
then survived into finality-pruned status history.

The initial correction distinguished complete effects in the exact attempted
batch from source completion elsewhere. It allowed only the former to select an
earlier owner in the same uninterrupted deadline-replacement sequence. It
incorrectly started new sequences for ordinary retries as well as explicit
readmission; the adversarial correction below supersedes that rule. Stale failures do not transfer ownership,
charge the newer owner, or override success. Reorg recovery and finality pruning
use the successful attempt and its existing manifest. Retained records remain
eligible for reconciliation even after all their sources completed elsewhere.
See the [accepted replacement design](../../proposals/skipped-slot-speculative-retry.md#november-10-correction-late-original-completion)
and [RPC contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).

## Initial candidate validation

Rust 1.95.0, LLVM 22, Foundry 1.7.1; Cargo dependencies locked. Baseline unit
regression `late_original_commit_reports_settled_references_after_replacement`
failed with actual `SourceCompletedWithMissingEffects`, expected `Committed`
(1 selected, 0 passed). It uses the same original/replacement sequence as the
incident and checks the terminal references through finality pruning.

The initial candidate Composer suite passed **428 tests**, with one existing
manual profiling test ignored. The new partial-application/reorg regression
first exposed a dependency on a scheduling flag reset by reorg; using the
retained sequence boundary fixes that case. Both first failures are preserved
locally. The baseline test executable was not retained; its source revision and
failure output are recorded. The exact candidate runtime/test executable hashes
are in [evidence.json](evidence.json), compiled from
`5c4832c5e708527612bb8a0697b145aa04f172d7`; subsequent record updates do not change
runtime source. The hashes matched across both process invocations.

| Check | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-composer --lib` | 428 passed; one existing manual profile ignored |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed on the initial candidate code |
| `cargo +1.95.0 fmt --all --check`; `git diff --check` | Passed |
| `forge build --root contracts` | Passed with existing compiler/lint warnings |
| `dynamic_admission::late_original_settlement_replaces_the_timeout_branch_without_losing_users` | Passed |
| `dynamic_admission::late_original_after_multiple_replacements_retains_users_and_follower_parity` | Passed |
| `partial_consumption::partial_prefix_keeps_unconsumed_users_and_receipt_costs` | Passed |
| `queue_reorg::native_l1_reorg_recovers_confirmed_and_unresolved_source_generations_in_nonce_order` | Passed |

The two delayed-original tests ran via
`cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- dynamic_admission::late_original --test-threads=1`.
The partial-consumption and reorg tests ran serially with `--exact` and both full
names above against the same source-built `node_e2e` executable. Each invocation
selected and passed two tests, with no ignored tests. Prebuilt runtime overrides
were unset; the harness built all three real runtime roles with `--locked`.
These process checks execute semantic assertions and independent follower replay,
including successful receipt-backed batch references for the late originals.

The first process invocation failed before execution while writing genesis
(`Disk quota exceeded` in sandbox temporary storage). A host rerun passed the
four-replacement test, but its `/tmp` output truncated and the invocation exited
101; the remaining outcome was unavailable. The final Clippy rerun's output was
also truncated. A direct 16 KiB write confirmed `/tmp` returned errno 122 while
the task directory succeeded. Moving **both** temporary files and output logs to
task-owned storage produced complete passing results for both process tests and
Clippy. The incomplete run is retained as inconclusive infrastructure evidence,
not counted as a passing run. No observed semantic failure was hidden by retries.

Raw incident RPC responses, process logs/datadirs and build/test logs are retained
locally only; they are not published. Small sanitized incident evidence and the
final log/executable digests are committed. Full workspace runtime tests, the
release-artifact matrix and Kurtosis were not run locally; CI owns those lanes.
Their pending/pass/fail status is available on
[PR 131](https://github.com/eez-association/eez-rollupX/pull/131).

## Compatibility and limits

Composer-only change. No protocol pin, contract, signer, blob, database or RPC
shape changes. `basis: attempt` and `complete` retain their candidate semantics.
No new RPC endpoint or durable queue is introduced. Rechecking unresolved
historical attempts can add repeated canonical RPC work. Ownership adds one
in-memory generation boundary per retained source. No performance benchmark was
run. Restart still loses pending/status history, so upgrading cannot reconstruct
old process-local terminal records. Older Composers retain this bug; rollback
restores it. The live deployment has not been changed by this PR work.

## Adversarial review and correction

Claude Opus 5.5 (`claude-opus-5-5`, maximum effort) reviewed head
`c979251cf504f68238e2f1437831105917bb27c5` read-only. Five targeted ledger
regressions failed at that head: a replacement winning after the original was
orphaned, a pre-dispatch replacement failure followed by an automatic retry,
local eviction before late application, finality before original evidence, and
reopening ownerless historical evidence after a reorg. The forward-winner case
passed with pre-PR production code at the baseline above, demonstrating an
introduced regression. PR #131 was returned to draft before implementation.
The first review's raw report and baseline reproductions are retained locally
under task-owned `artifacts/late-original/claude-review` and the separate
`late-original-adversarial` diagnostic worktree; they are not public artifacts.

The correction uses one admission boundary through all automatic retries.
Explicit readmission alone severs that boundary. Exact canonical effect evidence
can select either an older or newer retained attempt; source-only dispositions
and stale failures cannot transfer ownership. Reorgs reopen all invalidated
retained audits without scheduling ownerless history. Selection and mutation use
the same finality predicate: terminal ownership survives while another attempt
from its admission has unresolved evidence for that source. Completion metrics
are deduplicated across owner changes within an admission, but invalidation and
reobservation after a reorg still count again.

All five failing regressions now pass. The replacement-winner regression covers
both reconciliation orders and a previously reconciled, ownerless replacement.
An additional regression covers completion metrics across canonical-owner
correction and reorg. The complete Composer unit suite passes 434 tests, with
one existing manual profiling test ignored. These ledger tests explicitly inject
canonical outcomes; they do not independently establish L1 execution evidence.
A real retained replacement winning on a competing L1 fork remains a process
coverage limitation, rather than being inferred from the ledger tests. The
fixture prebuilds an empty competing payload; it does not select the retained
replacement for that fork.

No new RPC endpoint, configuration, or persistent state was added. Finality
now scans retained related history before pruning an owner. Persistent missing
evidence can retain history longer, as required for correct ownership; no
performance benchmark was run. Restart and rollback limits above still apply.

The first new inbound process test froze the canonical reader, but the
submission observer still saw the real receipt and correctly prevented a
replacement. It failed its required replacement barrier, before exercising the
intended transition. That output and datadir are preserved as fixture failure
evidence, not a production regression or a passing retry. The revised test holds
a real Engine API payload until ownership transfers, then imports it. The
controlled fixture forwards source companions through the standard EL pool;
this is non-atomic and block order is not assumed. The test verifies successful
source/post receipts, their exact same-block ordering, native delivery, state
and independent follower convergence. Existing outbound cases use the same
relay mode with one transaction. Relay unit tests cover companions, early error
and empty rejection in addition to the existing hold/drop/DA barriers.

### Corrected-candidate validation

Production corrections are in `ded7dff7`; the controlled inbound fixture is in
`2e92af22`; `d17227fd` corrects the test's native receipt decoder. On the first
controlled run, the source and post succeeded, ownership returned to the original,
and the independent follower replayed the delivery. The test then failed because
Alloy's Ethereum-only receipt decoder rejects EEZ type `0x45`. The final test
checks the native RPC receipt fields directly, without weakening the status,
identity, state or follower assertions. Both failures are preserved; neither is
counted as a passing retry on unchanged test code.

| Corrected check | Result |
| --- | --- |
| Composer library suite | 434 passed; one existing manual profiling test ignored |
| Workspace Clippy, all targets | Passed |
| Clippy for final native-receipt test amendment | Passed |
| Format and whitespace checks; edited-document relative paths | Passed; 104 relative paths resolved |
| Controlled relay tests | 21 passed |
| Both outbound late-original process tests | 2 passed |
| Partial-consumption and general reorg process tests | 2 passed |
| Corrected inbound late-original process test | 1 passed |

The five process cases used the source-built `node_e2e` target and real Composer,
signer and follower binaries with all prebuilt overrides unset. The four existing
cases passed in one five-case invocation whose inbound case failed at the native
receipt decoder; the corrected inbound case passed separately. These are semantic
process assertions and independent replay, not throughput-only checks. The exact
replacement winning on a competing fork remains ledger-only coverage.

The full tooling invocation selected 433 tests but encountered this host's `/tmp`
quota even with `TMPDIR`, because some shell scripts pass absolute templates to
`mktemp`. A task-local PATH wrapper relocated only those explicit templates to the
writable task directory. That rerun passed 421 tests, failed three resource tests,
and skipped nine opt-in/unavailable-tool tests. The three failures came from the
wrapper's `/usr/bin/env python3` selecting those tests' mocked Python. Changing
only the local wrapper to `/usr/bin/python3` made all seven resource tests pass.
Thus all 424 applicable tooling tests passed across those runs, not in a single
clean full-suite invocation. No repository workaround was added. Exclusions:
one real Docker payload probe, two real Kurtosis resource probes, one native
archive relocation probe, three real runtime Docker builds and two Gitleaks
checks (Gitleaks was unavailable locally). CI still owns the full release and
Kurtosis matrix.

### Independent re-review

Claude Opus 5.5 at maximum effort re-reviewed the production fixes and their
callers. It confirmed F1–F5 fixed and found no introduced High or Medium defect.
The broad review started from `2e92af22`; production Rust remained unchanged.
The native receipt amendment was made while that read-only review was running,
so a separate focused review checked its exact diff against the final test.
That review found no blocking defect and recommended accepting the amendment.
These were source-only reviews, not executed validation or a proof of soundness.
Claude could not confirm the protocol checkout through its restricted toolset;
the pinned checkout was independently verified before local validation.

The broad review requested that the inbound process test wait for the exact
replacement's reconciliation before its final assertions, rather than stopping
at the first `Committed` response. `99652ce7` ties the barrier to the original's
timeout, verifies the actual replacement dispatch and non-null new generation,
waits for its reconciliation, then checks the full original reference set. That
stronger process test passed with real L1 payload execution and follower replay.

One Low, pre-existing readmission limitation is deliberately tracked in the
[RPC contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission):
explicit readmission after local disposition severs the old admission, even when
an old dispatched attempt still has unresolved evidence. Its later canonical
success cannot transfer ownership to the new admission. Gating readmission on
that evidence would change admission policy; this correction preserves the
existing explicit-readmission contract and regression. The review allowed
tracking this limitation rather than changing that policy here. Other hypotheses
and pre-existing observations in the local review report were not treated as
verified new defects or silently folded into this fix.

A final focused Opus 5.5/max review found no blocker in the reconciliation
barrier or readmission documentation. It verified the event/state ordering and
whole-reference equality, and identified an inaccurate comment: the slot path
can reconcile the replacement first even though the recovery loop visits
attempts in generation order. The comment was corrected without changing test
behavior. The process test accepts either valid ordering after both dispositions;
the ledger regression deterministically covers both orders. The stronger test
passed and its focused Clippy check passed before this comment-only correction.
Formatting and whitespace checks passed afterwards. Review transcripts and raw
runtime evidence remain local-only; their report/log digests are retained in
[evidence.json](evidence.json). No claim of complete prover soundness or of exact
replacement-winner/finality process coverage is made.
