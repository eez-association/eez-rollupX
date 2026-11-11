# Post-stress bug review — 2026-10-09

Read-only implementation review; no deployment, restart, transaction resubmission,
production-code edit, or PR mutation performed.

## Identity

- PR #79 base: `4c4fec08d8a713497e1c8fc23bc0cf6fe0273e0f`.
- Candidate A head: `9ca75baed975d4fd9aef560ea359ac6a800dd3af`;
  deployed CI merge: `c3061cb1883eee253890d8310e5393deeb7b4335`.
- Candidate B head: `a4536050b6cecf80b247344a08348277ba098f92`;
  CI merge: `729a85310b595f53902fdab38c11b412bc659d55`.
  B head and CI merge share tree `82467f2aa15d0e73a9afc188303b56cb694a9a55`.
- Protocol: `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`.
- Reth: `3d592ece6de8c4559987416a544fc215fd6d6921` (v2.7.0).

## Findings

1. **Release blocker: ordinary transaction recovery is incomplete.**
   `discard_stale_seed_suffix` causes a real branch replacement but relies on
   Reth pool maintenance to retain displaced transactions. Reth reinserts them
   as external and ignores the insertion results (`maintain.rs:427–434`). Its
   default external per-sender capacity is 16. Candidate A and B have identical
   deriver recovery code. This explains the live 20-to-16 recovery pattern and
   resulting nonce gap; see [stress evidence](stress-failure.md).
2. **CI synchronization migration:** 38 of 39 failed tests wait for the old
   `eez.composer.settlement.decision` event with `due=false`. The new idle path
   uses the planner and skips that preparation path. These failures do not
   establish 38 runtime faults. Replace barriers while preserving the intended
   slot phase; a cutoff event is not automatically a boundary event.
3. **Lost nonzero-coinbase regression coverage:** the remaining test expects a
   historical empty boundary carrying the configured recipient. The new
   `Ordinary` path calls `build_empty_slot_block`, which supplies zero. The
   fixture fails before testing replacement. Resolve the header contract and
   preserve the nonzero-header regression; do not merely remove its assertion.
   No settlement correctness failure is established by this assertion alone.

## Diagnostic evidence and limits

A local diagnostic used the actual pinned Reth pool with mocked validation.
It first inserted N local transactions, removed them, then reinserted them with
external or local origin. Results:

| N | External accepted/rejected | Local accepted/rejected |
| --- | --- | --- |
| 16 | 16 / 0 | 16 / 0 |
| 17 | 16 / 1 | 17 / 0 |
| 20 | 16 / 4 | 20 / 0 |

All rejections were `SpammerExceededCapacity`. All six assertions passed.
This proves the pool-limit mechanism, not an end-to-end branch replacement.
The old live block body was unavailable, so its complete transaction list was
not independently recovered. A multi-transaction process regression remains
required before a fix can be accepted.

Local-only diagnostic source/binary and eight CI job logs are retained under
`/tmp/rollupx-pr79-ci-review-y7axfhr9/`; these are not available in another checkout.
The diagnostic was linked using Rust 1.95.0 against matching existing cached
Reth/Alloy/Tokio libraries. An initial Cargo attempt was stopped because its
feature selection triggered a broad dependency rebuild; it is not a passing check.

[CI run 37979169850](https://github.com/eduadiez/rollupX/actions/runs/37979169850)
completed with failure: native E2E shards total 93 passed / 39 failed; build,
lint, unit and Kurtosis lanes passed. No new full E2E suite or live recovery
was executed during this review. Candidate B was not deployed.
