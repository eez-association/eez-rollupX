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

The correction distinguishes complete effects in the exact attempted batch from
source completion elsewhere. Only the former may select an earlier owner in the
same uninterrupted deadline-replacement sequence. Explicit readmission and
ordinary retries start new sequences. Stale failures do not transfer ownership,
charge the newer owner, or override success. Reorg recovery and finality pruning
use the successful attempt and its existing manifest. Retained records remain
eligible for reconciliation even after all their sources completed elsewhere.
See the [accepted replacement design](../../proposals/skipped-slot-speculative-retry.md#november-10-correction-late-original-completion)
and [RPC contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).

## Validation

Rust 1.95.0, LLVM 22, Foundry 1.7.1; Cargo dependencies locked. Baseline unit
regression `late_original_commit_reports_settled_references_after_replacement`
failed with actual `SourceCompletedWithMissingEffects`, expected `Committed`
(1 selected, 0 passed). It uses the same original/replacement sequence as the
incident and checks the terminal references through finality pruning.

The initial candidate Composer suite passed 426 tests, with one existing manual
profiling test ignored. Additional sequence/reorg tests and process assertions
were then added; final validation is pending. Integration contracts built with
Foundry 1.7.1 (existing compiler/lint warnings). Formatting and whitespace checks
pass. Full process and workspace Clippy checks are running; these are not yet
claimed as passing.
Raw incident RPC responses/logs and build/test logs are retained locally only;
they are not required to run the committed regressions and are not published.

## Compatibility and limits

Composer-only change. No protocol pin, contract, signer, blob, database or RPC
shape changes. `basis: attempt` and `complete` retain their candidate semantics.
No new network calls or durable queue are introduced. Ownership adds one
in-memory generation boundary per retained source. No performance benchmark was
run. Restart still loses pending/status history, so upgrading cannot reconstruct
old process-local terminal records. Older Composers retain this bug; rollback
restores it. The live deployment has not been changed by this PR work.
