# Conditional slot production

| Field | Value |
| --- | --- |
| Design status | Implementation draft in PR #79. The user authorized replacing the temporary dual scheduler with one position-based scheduler and an optional sub-block delay. Validation is in progress; not accepted for production rollout. |
| Date | 2026-10-09 |
| Source baseline | `eduadiez/rollupX`, branch `feat/composer-wallet-gas-estimation`, `6b1e281a418f845605744c09a6bc9e8782eb6b61`. Existing unrelated Compose edits and operator artifacts are excluded. |
| Protocol baseline | `855fe0602484750861b4c31502a1a91355d33ed6`; initialized, without protocol edits. |
| Implementation base | `main` at `4c4fec08d8a713497e1c8fc23bc0cf6fe0273e0f`, protocol gitlink `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. Pinned checkout initialized and affected contracts rechecked; no protocol edits. The three original design reviews predate this base. |
| Related work | [Settlement contract](../configurable-settlement-policy.md), [RPC contract](../../crates/eez-control-rpc/SPEC.md), [skipped-slot recovery](skipped-slot-speculative-retry.md), [review record](../audits/2026-10-09-conditional-slot-production/README.md). |

## Problem and recommendation

The current driver reserves a Future suffix and builds the remaining interval
early even when Composer subsequently decides not to post. Future blocks already
use the ordinary transaction pool. Producing several blocks back-to-back gives
reactive applications little opportunity to submit between them; they can look
empty without any explicit rule excluding transactions.

The recommended first release fixes this timing problem and simplifies the
preparation trigger. It keeps every on-grid boundary empty unless the existing
cross-chain composition path puts its authorized actions there. When no
preparation is needed, all blocks are produced at their normal wall-clock times,
including the empty boundary. When preparation is needed, the remaining suffix
is built promptly using the existing settlement pipeline.

This deliberately leaves one empty block per idle interval. It does **not** yet
deliver ordinary transactions in non-settling sync positions. That extension is
separate because the current admission and historical splitting paths depend on
those empty endpoints. The user's relaxed requirement permits proposing this
trade-off, but it must not be reported as implementing the entire original idea.

No contract, native DA format, signer acceptance profile, queue durability,
genesis grid, block timestamps, or L1 target-binding change is proposed.

## Requirements and acceptance

These are proposed requirements, not a replacement for accepted specifications.

| ID | Required behavior | Acceptance check |
| --- | --- | --- |
| R1 | A slot with no preparation reason runs at the genesis-defined cadence, without reserving a Future suffix. | With future settlement thresholds and an empty cross-chain queue, inject a transaction after each ordinary block; verify the last two ordinary positions also include transactions submitted before their build starts. Record receipt times, not just header timestamps. |
| R2 | One explicit position plus an optional delay selects the preparation cutoff. | With `K=6`, `P=3`, commit the third ordinary block at nominal `+6s`; with delay 250 ms decide at or after `+6.25s`, then build positions 4 and 5 and compose position 6 if preparation is selected. Timestamps remain `+8s`, `+10s`, `+12s`. |
| R3 | Empty queue is not synonymous with no settlement. | Independently exercise pure-L2 mode/interval, general threshold, fullness, finite drain, window limit, and cross-chain opportunity. |
| R4 | Late arrivals cannot reopen an ordinary decision while its authenticated context remains valid. | Requests arriving after the cutoff remain owned for the next eligible opportunity; no eviction or retry charge for waiting. Canonical generation/branch changes invalidate the decision and permit fresh bounded recovery/policy evaluation. |
| R5 | Preparation is an opportunity, not permission to post. | Final Composer policy, admission, exact L1 parent/target, gas, blob, fee, proof, and exclusive deadline checks still control dispatch. |
| R6 | A non-posting or failed boundary remains an empty usable endpoint. | Policy deferral, failed preparation, expired target, and catch-up never fall back to a pool-filled terminal; historical splitting still works. |
| R7 | Recovery remains active in ordinary mode. | Failed attempts, reorgs, same-parent missing slots, four-replacement exhaustion and late-original inclusion preserve single ownership, canonical reconciliation and seed derivation. |
| R8 | Ordinary production never speculates ahead of wall time merely to reach the cutoff. | Delayed, duplicated, reordered and dropped triggers cannot duplicate commits or produce future ordinary-mode blocks. Settlement-mode bursts are the explicit exception. |
| R9 | Configuration reaches every supported runtime consistently. | Test absent, empty, malformed, zero, out-of-range and conflicting settings; runtime, Compose and Kurtosis agree on the resolved position. |
| R10 | Deployment claims require process and performance evidence. | Run the acceptance matrix below on the actual candidate, with independent follower replay and documented baseline/candidate identities. |

## Scheduling design

### Position and timing

Let `K` be the immutable number of L2 blocks per L1 interval. Number positions
`1..K` after the preceding on-grid boundary; position `K` is the next boundary.
The illustrative block numbers 100..106 in the user diagram are relative labels,
not a change to the chain's genesis-aligned modulo rule.

Require `EEZ_SETTLEMENT_START_AFTER_BLOCK=P`, an integer `1 <= P < K`.
`EEZ_SETTLEMENT_START_DELAY_MS` defaults to zero and must be a nonnegative integer
strictly below one L2 interval. For Ethereum's 12-second/2-second schedule, P=3
and delay=250 means decide at nominal `+6.25s`, after the block at `+6s` commits.
Resolve ticks and the delayed trigger against the genesis timestamp grid, not
process startup, notification arrival or the actual completion time of a late
block. A late cutoff does not add another full delay or reset the submission
deadline. Block P still builds at its normal time. The delay allows reactive
submissions before planning, but does not promise admission or inclusion.

The sequencer owns one in-memory decision keyed by exact target (including its
L1 parent hash), reconciled generation and the cutoff block's hash. It does not
create a second scheduler or durable reservations. Blocking policy reads use one
bounded worker permit, retained until worker exit even if its caller times out. The order is:
`recover_failed`, `prepare_next_slot` maintenance, authenticate the resulting
parent/context, commit through the cutoff block when due, then decide through
the sequencer's serialized path. A performed timed-out replacement requires a
fresh preparation opportunity, still subject to final admission and deadline.
The cutoff event and ordinary ticks use the same decision helper after the
committed P prefix. Ticks retry a cutoff that lacked reconciled evidence. A
planner error or timeout selects paced fallback with existing Empty settlement
maintenance; only a successful not-due result suppresses posting at K. Either
ordering commits P once and obtains one valid decision. A new canonical generation or local
branch replacement invalidates the old decision; all prepared blocks still pass
the existing locked commit checks. Normal appends descending from the cutoff
hash do not invalidate it. Catch-up remains bounded and uses the existing
structural-only rules; do not insert cross-chain work into past catch-up slots.

If the L1 head/trigger is late, a caught-up interval gets at most one decision
when it can actually run. Build only missing positions. Start preparation only
while the original deadline leaves the finalization/delivery reserve and a
positive proof opportunity; otherwise continue the ordinary/empty path. Do not
reconstruct a historical queue snapshot or reset the deadline. If behind, use
bounded structural catch-up and defer cross-chain work to an eligible interval.

### Two production paths

1. **Ordinary:** continue one due block at a time through position `K`. Positions
   below `K` use the pool. Position `K` uses the existing empty-block construction
   without posting. Advance to the next interval without requiring a settlement
   attempt. Existing readiness, canonical-context, depth and outage bounds still
   apply; this is not a promise to sequence indefinitely without L1 evidence.
   Advance ordinary ticks through `K` only while a known target or one of the
   bounded (at most four) replacement targets exists; with no target, hold as
   today. Test this explicitly in G2; do not add a free-running L1-independent
   production path. Publish an already armed replacement to ticks once its
   preceding boundary time arrives, without waiting for its cutoff. Finish an
   existing valid ordinary decision's boundary before using the next target.
2. **Preparation selected:** promptly build any remaining ordinary positions,
   using the pool and existing DA admission, then invoke the existing Composer
   settlement path for position `K`. Do not wait for fresh pool arrivals between
   suffix blocks. Actual settlement terminals receive no new ordinary-pool
   behavior. If final admission declines or fails, retain the existing empty
   terminal/recovery outcome; do not switch back mid-attempt.

If Composer returns `EndTick`, a later tick at or after the boundary's timestamp
may finish the missing boundary through the existing Empty mode. It must rerun
readiness, recovery and context checks and verify the boundary is still the next
block. It may not pull from the pool or bypass an unavailable ancestor/DA check.
An error is not proof that an empty candidate is safe; retry under fresh evidence.

Run `recover_failed` and the necessary `prepare_next_slot` maintenance even when
the slot will take the ordinary path. Maintain the common missing-slot bound and
the submission gate, not an independent counter in the new path. Neither a
receipt nor relay acceptance establishes canonical progress.

### Deciding whether to prepare

Expose a side-effect-free Composer planning query over reconciled policy facts.
Factor shared trigger arithmetic out of the existing policy module; do not call
its state-mutating evaluator with a fabricated future block hash. The planner
records a bounded reason set and returns a preparation hint, not a settlement
attempt or drain owner. The caller binds the result to target/generation context;
the real built candidate remains authoritative.

At the cutoff, consider included ordinary activity, thresholds at the target
timestamp/height, finite uncovered drain work, fullness, effective signer window,
pending settlement maintenance, and available cross-chain work. A nonempty queue
may justify one bounded admission opportunity; it is not evidence of executable
cross-chain urgency. Do not duplicate cross-chain simulation in the planner.

The cutoff is the actual serialized decision following position `P`, normally
at its scheduled time plus the configured delay; the planner observes queue occupancy then. The existing
finite admission snapshot is still taken by Composer after a preparation burst.
Work observed after the decision cannot change a successful ordinary decision for the interval. This
also specifies the late-head case without requiring durable arrival history.

Conservative false positives are acceptable: a slot may burst and end empty
without posting. A false negative caused by genuinely new work after the cutoff
waits until the next eligible cutoff. Record this scheduling latency explicitly
in the settlement contract, including the first late ordinary inclusion in
`always` mode and newly eligible cross-chain work. For an on-time steady chain,
the next decision is one interval later and targets the following boundary;
this is not a bounded inclusion guarantee during overload, rejection or outage.
GENERAL and WINDOW_LIMIT are computable from the target height; future activity
and eligibility are not. Do not assert that fullness is the only rule that can
become due after the cutoff. Resource admission continues
to bound ordinary blocks meanwhile; preserve the empty endpoint even if a
threshold becomes due late. Missing canonical evidence never authorizes a post
or bypasses existing sequencing-readiness checks.

### Keep indispensable time limits

Position replaces the calculation of the preparation trigger and Future count.
It does not replace wall-clock deadlines. Keep the existing absolute target
minus submission margin, finalization/delivery reserves, measured admission
estimates, and proof-request cancellation/ownership rules. The block-count knob
cannot guarantee that a heavily loaded signer finishes in time.

For the first release, `EEZ_PROOF_TIME_MS` remains a proof-estimator seed, not a
second trigger knob. Keep `EEZ_SUBMISSION_SLACK_MS`.
Do not simultaneously redesign the estimator or invent a new universal seed.
Reject any `EEZ_PREPARATION_TIME_MS`, including an empty value, with an actionable
migration error. Missing or invalid P fails startup; no legacy scheduler or
implicit conversion of the old timing budget remains. An absent delay means
zero; an empty delay is invalid. Log position, delay, deadline margin and proof seed.

Bound the preparation-hint wait by the earlier of the next future ordinary grid
tick and remaining deadline headroom. At an on-time delayed trigger this leaves
at most `L2 - delay` for planning, so a slow query cannot consume the next ordinary
block's interval merely because delay is nonzero. If already late, use the next
future tick rather than adding a full interval or rejecting planning solely for
a missed tick. This does not undo existing lateness or guarantee commit latency.
A late query immediately before a tick can receive only a few milliseconds and
fall back safely; already-overdue blocks may wait up to one more L2 interval.
A timed-out blocking worker still retains its single permit until it exits.

Audit every consumer of `proof_window_open`, `future_count`, late-entry gates and
replacement cutoffs: they must use the resolved schedule consistently. Selecting
a later position must not leave an old proof-start check that rejects every
attempt before Composer can use its shared deadline. For scheduled targets,
replace the sequencer's `now + proof_seed > deadline` pre-gate with the exclusive
deadline gate and existing shared pipeline admission. Preserve the independent
constraints of non-exact-target internal composition paths.

At startup, conservatively require `proof_seed + submission_slack + initial
post-proof reserve + delay < (K - P) * L2_interval`, leaving positive preparation time.
The existing initial post-proof reserve is 150 ms and already includes the
delivery allowance; do not add the 25 ms delivery reserve again. Validate this
in Composer configuration/node wiring, where those budgets belong, rather than
making the role-neutral driver depend on Composer. Use the same existing reserve
constant, not a duplicated literal. This is a configuration sanity check, not
a completion guarantee or a new stage timeout.

Derive the trigger offset and replacement timing from `P * L2_interval + delay`, and the
burst layout from `future_count = K - P - 1`. `preparation_time()` and its nominal
telemetry remain the derived remaining planning allocation, not the offset
itself. Preparation, proving and submission keep the shared absolute deadline.

## Why not fill skipped sync blocks in the same release?

At this baseline, ordinary admission in
[`sequencing.rs`](../../crates/eez-composer/src/composer/sequencing.rs) reserves a
whole `K`-block interval and assumes the preceding grid boundary is empty or has
an existing optimistic settlement owner. Historical splitting selects empty
on-grid terminals. Filling a skipped boundary joins two previously independently
postable intervals; simply removing `compose_empty` is therefore unsafe.

A later extension could carry the admission budget from the last actual empty
endpoint, allowing a filled boundary only when an empty continuation through the
next boundary is reserved. It would have to cover native stream bytes, logical
block limits, request/witness quotas, and demonstrated proof-window headroom;
blob bytes alone do not establish those properties. Startup, reorg reconstruction,
independent payload jobs and downgrade behavior must share the same rule.

That follow-on needs a shared, branch-bound boundary plan for both the sequencer
and payload jobs; a nonempty boundary cannot reset segment accounting. Its
minimum invariant would reserve the native bytes and block window for the whole
segment through the next empty boundary. Check anchor-only L1 gas under the
resolved blob allowance too. Ordinary admission must distinguish a conclusive
capacity result (which can choose the already reserved empty boundary) from
missing/stale evidence (which cannot authorize a commit).

Publish a tolerant-reader release before enabling such production. The reader
must understand historical nonempty intermediate boundaries without an ephemeral
production plan. Bound/cache ancestry work, test cold-cache and signer restart,
and test nonempty intermediate boundaries through ordinary preparation, final
proof and follower replay. The tolerant reader becomes the rollback floor until
all affected history is settled or the speculative suffix is explicitly discarded.

The baseline only shrinks historical candidates on block/blob/gas over-budget
results, not on proof-time or witness-quota errors. Retaining empty boundaries
does not fix those existing liveness limitations. Removing them nevertheless
enlarges the minimum possible recovery prefix and removes options for future
recovery changes; an equal maximum batch span is not an equivalent guarantee.

Do not implement this as an automatic second phase hidden inside the first
release. Revisit it after measuring the remaining one-block inclusion gap. No
new checkpoint-frequency knob, speculative proof worker, or terminal-profile
extension is proposed now. If ordinary transactions at skipped grid boundaries
are required for the first release, this narrower recommendation must be
reconsidered explicitly rather than silently weakening recovery invariants.

## Implementation sequence

1. **Timing/configuration:** add the resolved position mode in driver timing;
   cover boundaries and configuration transport before changing production.
2. **Shared planning query:** extract pure due-reason calculations and add the
   reconciled planning interface, with no ownership side effects. Retain final
   policy evaluation and all admission checks.
3. **Conditional sequencing:** align ticks, serialize the cutoff decision,
   support ordinary continuation through the empty boundary, and reuse the
   existing burst/compose/recovery paths. Keep all fixed-interval DA assumptions.
4. **Contract/docs/observability:** update settlement policy and timing docs,
   RPC failure/retry wording if affected, supported example/config consumers,
   and operator rollout guidance. Log decision reason, actual commit time,
   advertised timestamp, target identity, deadline miss and canonical outcome.
5. **Validation and staged enablement:** run the checks below, compare matched
   workloads, then enable the explicit setting only in an authorized rollout.

Each implementation chunk carries focused tests; these are reviewable increments,
not permission to deploy an incomplete scheduling path.

## Compatibility and recovery

The proposed first release preserves wire data, on-grid empty anchor-only
terminals, exact hash commitments, signer identity and historical derivation.
Mixed-version signer/follower compatibility is expected from the unchanged
contract but must be demonstrated by process replay, not inferred from compilation.
There is no protocol-pin or database migration in scope.

The first dual-scheduler candidate is frozen at `9ca75baed975d4fd9aef560ea359ac6a800dd3af`
(candidate A). Its [CI run](https://github.com/eduadiez/rollupX/actions/runs/37964363878)
passed. A subsequently failed the heavier ordinary-transaction workload with a
nonce gap; see the [retained failure](../audits/2026-10-09-conditional-slot-production/stress-failure.md).
It is not a known-good rollback for that workload. The single-scheduler candidate
and its recovery repair need their own CI and live validation, recorded in the
[merge-readiness audit](../audits/2026-10-09-conditional-slot-production/final-validation.md).
Compare A with explicit P against B at the same P and zero delay before measuring
a nonzero delay separately. Keep policy, workload and hardware comparable; do not
run competing builds during timing measurements. A's success does not validate B.

Before starting B, preserve operator files, remove the old preparation setting
from all effective sources and explicitly select P. Retain the proof seed and
submission margin; start with delay zero. Keep launcher/env/config-file precedence
unchanged. New follower binaries share this parser and also require P, even though
P/delay do not alter historical replay. Existing followers need no wire migration.
There is no configuration switch back to the legacy scheduler in B.
Rollback requires a verified compatible prior binary and its matching configuration;
A contains an earlier recovery fix, but not the displaced-batch sender-cap repair;
do not recommend it as a production fallback for this failure. A also does not
implement the delay setting. Do not downgrade onto state
whose storage compatibility has not been established. Canonical recovery still
runs; restarting can discard speculative blocks and loses in-memory queued
requests/retry counters. Clients may need to resubmit. A database backup does not
provide durable queue recovery.

## Validation and release gates

Release gates are G1 (configuration migration and timing), G2 (sequencer ordering and
fallbacks), G3 (policy/late-work semantics), G4 (process recovery and replay), and
G5 (matched load and the remaining empty-boundary gap). All five need recorded
evidence for the candidate; agreement on this design does not satisfy them.

- **Driver unit tests:** `K=2`, Ethereum `K=6`, Gnosis `K=5`; all valid `P` values;
  required/missing/invalid/overflow/obsolete inputs; zero and sub-block delay,
  exact interval rejection and headroom exhaustion; genesis offsets; clock/tick/cutoff
  ordering; delayed heads; no duplicate decision, early queue read or ordinary commit.
- **Composer unit tests:** every due reason and late-threshold case; planner has
  no drain/ownership side effects; conservative queue hints still use validated
  admission; no policy-disabled behavior is invented (policy is mandatory here).
- **Process tests:** continuous ordinary arrivals across late positions; actual
  commit-time cadence; pure-L2 and cross-chain posting; empty failed/skipped
  terminals; tight blob/gas limits and historical splitting; delayed prover and
  late arrivals; partial consumption, restart, reorg, missing L1 slots, four
  replacements, exhaustion and late-original inclusion.
- **Independent replay:** follower reproduces endpoint hashes and state/seed
  behavior for alternating ordinary and settlement intervals on both supported
  network profiles. Compare mixed-version components where supported.
- **Configuration/quality:** affected Rust package tests, locked workspace Clippy,
  formatting, Chiado configuration tests and settlement-config checker, with
  Kurtosis transport checks. Missing P must fail; absent delay must remain distinct
  from an empty string through Compose interpolation. Obsolete inputs must not be
  silently dropped. Trace `RollupTiming::from_env`,
  `ComposerSettings`, node wiring, both Chiado Compose files, the Kurtosis node
  Compose file and `testing/kurtosis/main.star`; preserve existing operator edits.
  Use the repository's Rust/Foundry versions and CI artifact rules. Require
  nonzero selected tests and retain first failures.
- **Matched performance comparison:** use this baseline and the candidate with
  identical policy, contracts, hardware, pool-arrival workload and signer state.
  Measure per-position transaction inclusion, actual inter-block intervals,
  inclusion latency percentiles, preparation/proof duration and target misses.
  A reactive arb workload supplements, not replaces, continuous-arrival tests.
  Record idle boundaries separately; the first release still leaves them empty.

Do not call the implementation production-ready until semantic tests, independent
replay and representative load/deadline measurements pass. This document records
a design and planned gates, not executed runtime validation or deployment.

Focused process selections are modules of the existing `node_e2e` target:
`settlement_policy::`, `settlement_policy_gas::`, `settlement_policy_cost::`,
`dynamic_admission::`, `partial_consumption::`, `queue_liveness::`, `queue_reorg::`,
and the affected `cross_chain::`/`chained_interstate::` cases. Run serially with
retained diagnostics following root `AGENTS.md`; the full release/Kurtosis matrix
remains CI-owned. Add deterministic scheduling regressions to the existing driver
tests and cadence/recovery process cases to the relevant modules, rather than
creating a separate test framework.
