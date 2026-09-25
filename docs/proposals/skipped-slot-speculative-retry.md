# Replace an unresolved batch at the next preparation deadline

Status: design accepted by the user on 2026-09-23; initial implementation locally validated and deployed to the existing Chiado network. See the [validation and deployment record](../audits/2026-09-23-timeout-replacement/README.md). The later stress run exposed an uncovered historical-prefix fallback; its correction passes the targeted process regressions and is now deployed to the existing Chiado node. See the [historical-prefix audit](../audits/2026-09-23-historical-target/README.md).

Baseline: fork `1832f970884ca9460646a4cde87cca70eaffb3e1`, protocol
`3c0cdf4955941a5c23639346932b55b33babebbd`. Replaces the earlier draft that
proposed maintaining two active branches. Historical investigation results remain
in the [fourfold-run audit](../audits/2026-09-23-chiado-staged-performance-4x/README.md).

## Behavior

After sending a batch, wait until the next slot's preparation deadline. If the
expected L1 block remains absent, replace the speculative L2 suffix and prepare
a fresh batch for that next slot. Retain the old submission as history. If L1
later settles the original, the existing canonical deriver replaces the local
speculative branch with the settled execution.

There is one active L2 branch. A deadline is permission to rebuild, not proof of
failure or canonical consumption. No consensus-client request is needed.

The scheduler permits one extra opportunity per observed L1 head, at the normal
preparation deadline (2.8 seconds after the missing slot under the Chiado
500 ms preparation / 1,000 ms proof / 700 ms submission-slack preset). The
execution block number stays unchanged; the timestamp advances one slot. A new
head replaces this timer with the ordinary schedule. Consecutive missing slots
without any new head exhaust this single replacement; observation continues.
This bounds work and retained submissions during disconnection.

## Invariants and implementation

- Before replacing, require the latest execution head to remain the parent of
  the original target, and canonical derivation to have completed through that
  same hash. Recheck under the engine reconciliation lock.
- Replace the old cross-chain Sync with an empty Sync by switching branches,
  preserving Reth's reorg notifications so ordinary descendant transactions
  return to the pool. Rebuild toward the new terminal Sync with complete user
  groups. The proven range still starts at the canonical settlement cursor.
- Atomically return still-owned users to the held pool, preserving signed bytes,
  source nonce order, attempt counters and reservations. Keep the original
  manifest, target, envelope and observer. Old results cannot remove newer owners.
- Build a new proof and bundle at the new exact timestamp, and reuse the old
  poster nonce. Final ordered execution, source nonces, fee checks, timestamp
  pins and target-height checks still apply. A later original inclusion may
  invalidate this preparation; canonical replay remains authoritative.
- The policy submission gate admits either the ordinary next-slot timestamp or
  one additional slot on the current parent. It rejects misaligned/larger gaps
  and rejects a target height that has already been produced. This deliberately
  admits speculation without pretending to have proof that the slot was missed.
- Reconcile both submission records against canonical execution. Partial source
  consumption remains supported. Timeouts do not charge user failure budgets;
  canonical evidence decides source completion and eligible retries.

## Historical-prefix correction

The live oversized-history case reached `emit_historical_chunk`, which selected
a fresh target instead of retaining the exact replacement target. This could
advance the execution height even though the original slot produced no block.
Pass the already selected target through both historical-prefix callers. Select
a future target only when the caller has no exact target, then bind every prefix
candidate and its submission to that one target. Keep the existing proof cutoff
and canonical-parent checks; an expired opportunity remains a deferral.

The regression must force blob overflow, capture an actual historical-prefix
bundle, omit its target slot, and require a replacement at exactly +1 slot and
the same execution height. Verify ordinary transfers execute once and the
independent follower reaches the settled prefix. Preserve the baseline failure
and repeat the same test against the correction. Existing delayed-original and
ordinary skipped-slot regressions remain required.

## Compatibility and operations

No protocol pin, contract, wire format, signer profile, genesis or database
migration changes. Mixed-version followers continue replaying the same canonical
blob format. Upgrade the Composer to enable this scheduling behavior; older
Composers retain the extra-slot wait. Pending ownership remains in memory.
Restart/rollback reconciles canonical history, discards speculative state and
can require clients to resubmit unsettled requests. Preserve existing network
bindings, images and datadirs during the upgrade; follow the
[deployment runbook](../runbooks/chiado-deployment.md#recovery-and-upgrades).

## Acceptance

- Skipped-slot process test: replacement targets exactly +1 slot at the same L1
  height; all source nonces settle once with unchanged group selection.
- Delayed-original process test: hold an actual Engine API payload until the
  Composer has dispatched its replacement, then release the original. Verify
  L2 reorg, user outcomes, state, nonce ordering and independent follower parity.
- Ledger tests: late original success/failure cannot erase newer ownership;
  failed replacement preparation leaves original users reconcilable; L1 reorg
  reopens the right owner; timeout does not charge attempts.
- Scheduler and submission tests: one extra timer, same execution height,
  correct timestamp/deadline, rejection of larger gaps and already-built targets.
- Run affected settlement, partial-consumption, queue/reorg and resumption suites,
  formatting and Clippy before deployment. Record actual results and omissions
  in the implementation audit; design acceptance is not validation evidence.
