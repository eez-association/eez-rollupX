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
No new network calls or durable queue are introduced. Ownership adds one
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
The controlled-consensus process fixture supports single-transaction outbound
bundles; a real retained replacement winning on a competing L1 fork remains a
process-coverage limitation, rather than being inferred from the ledger tests.

No new network requests, configuration, or persistent state were added. Finality
now scans retained related history before pruning an owner. Persistent missing
evidence can retain history longer, as required for correct ownership; no
performance benchmark was run. Restart and rollback limits above still apply.
