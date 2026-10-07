# Cross-chain queue liveness and nonce recovery

Status: implemented, validated locally and on Chiado, and deployed on
2026-09-15.
The [implementation audit](../audits/2026-09-15-cross-chain-queue-liveness/README.md)
maps requirements to code and tests. The prototype evidence below describes
the earlier investigation, not the current implementation's acceptance status.

This proposal supplements [configurable settlement policy](configurable-settlement-policy.md),
especially its admission outcomes and ownership requirements in §5/§6/§8. It does
not change settlement triggers, consensus, contract interfaces, DA encoding,
proof formats, or the accepted restart behavior. Pending in-memory requests may
still need resubmission after a restart.

## Problem and evidence

The [Chiado investigation](../audits/2026-09-15-chiado-nonce-gap/README.md) found a
composer repeatedly selecting nonce 14,517 while its sender requires 14,514.
The queue continued growing while unrelated ordinary history settled on L1.

Further experiments against `71be1a5` identify separate faults:

| Fault | Deterministic reproduction | Consequence |
| --- | --- | --- |
| Admission uses the highest reservation | Admit nonce 3 against source nonce 3, then lower the observed source nonce to 0 | Nonce 4 is accepted; the missing nonce 0 is rejected |
| Recovery ignores predecessors already queued | Reopen an older attempt after a partial completion, then recover a newer attempt from the same sender | Queue order becomes `[1, 0, 2]`; nonce 0 exists but nonce 1 blocks it |
| A terminal accounting record overrides explicit admission | Exhaust a request's attempt budget, explicitly resubmit its signed transaction, then reconcile the old attempt again | Admission returns success, but the accepted request disappears from the queue |
| The same ownership problem exists for replacements | A previously terminal transaction replaces a cheaper queued transaction at its nonce | An old callback removes the accepted replacement |
| Sender conditions are treated as slot failures | Source nonce, fee, or balance is unavailable | The complete drain is restored and automatic selection can remain at one |

There is also a reproducible path through the older deployed error handler:
after evicting a previously submitted prefix and an unsubmitted successor, an
unguarded restoration of the original drain reintroduces both. A later
reconciliation removes the terminal prefix but retains the untracked successor.
The experiment produces exactly `[14517]` from `[14514, 14515, 14516, 14517]`.
The PR's existing `requeue_composing` guard prevents that particular path.

These are proven code paths, not proof of the original live event sequence.
The node logs covering that event have rotated away. Both an absent predecessor
and a predecessor stranded later in the queue must be handled safely.

## Required invariants

1. For one `(rollup, sender, direction)`, queued transactions are ordered by
   nonce, including after recovery, replacement, and explicit resubmission.
   Inbound and outbound nonce chains remain independent.
2. A new successor cannot extend a known hole in the observed reservation
   chain. The first missing predecessor can be submitted and placed before its
   successors. Active exact duplicates and valid replacements remain supported.
3. Every accepted request remains owned until canonical reconciliation, an
   explicit terminal disposition, replacement, or the already accepted loss of
   in-memory state on restart. Old callbacks cannot erase a new admission.
4. Pending/provisional nonce observations are admission hints. They do not
   prove execution or authorize dropping requests after a rollback.
5. Source execution uses the actual pinned source state and the accepted
   prefix of this composition. No future nonce is executed before its
   predecessor, and no transaction/system pair or nonce dependency is split.
6. A blocked source chain cannot starve independent eligible senders or the
   opposite direction. Global provider, proof, and canonical-state failures
   retain their existing conservative recovery behavior.
7. Deferral for nonce, fees, balance, or escrow does not count as a failed
   settlement attempt or establish cross-chain posting urgency.
8. Reconciliation, selection, and reported status refer to explicit hashes and
   attempt generations. A same-height replacement is not the same state.

## 1. Repair admission and recovery in HeldPool

Keep `HeldPool` and its existing mutex authoritative for queue membership,
hash deduplication, nonce reservations, and ownership transfer. Do not add a
second queue or a transaction-ownership journal.

Replace `max(reserved) + 1` with the first unreserved nonce starting at the
observed source nonce:

```text
expected = observed_source_nonce
while expected is reserved in this sender/direction's queued or in-flight set:
    expected = checked_add(expected, 1)
```

Only membership is queried, so hash-map iteration order cannot affect the
result. Overflow must remain explicit. Reservations below the observed nonce
do not contribute; they are not deleted by this calculation. A sparse
reservation at `u64::MAX` must not prevent repair of an earlier hole.

Preserve the front's current support for ordinary source-mempool predecessors.
Do not replace its `pending` observation with an L1-finalized or L2-safe nonce:
that would reject valid existing submission flows or unnecessarily delay them.
If that observation later decreases, admission uses the lower observation to
find the first hole, and composition reevaluates readiness from its own source
state. Do not rely on an atomic RPC-read-plus-admission operation: that cannot
be guaranteed across a source-chain rollback. Tolerate the race safely.

Apply one checked queue-insertion policy at every insertion path:

- New admission normally appends. A gap repair is inserted before the first
  queued successor in the same sender/direction chain.
- Recovery normally takes front priority, but is inserted after any queued
  predecessor in that chain. It must never push nonce N+1 ahead of queued N.
- A valid replacement keeps the replaced queue position. Both fee-bump checks,
  active hash deduplication, and the prohibition on replacing an in-flight
  nonce remain unchanged.
- Existing relative order among unaffected queued requests is preserved. Nonce
  dependencies take precedence over literal global FIFO. Recovery arriving
  from different attempt generations cannot override that rule.

Exercise the invariant after every mutation, rather than relying on a later
sorting pass to make an invalid queue executable. Avoid sorting globally by
address: that would introduce unrelated sender priority.

### Explicit resubmission is a new admission

After envelope/nonce/replacement validation succeeds, distinguish explicit
ingress from internal recovery:

- A locally terminal old owner (`canonical: None`) may be retired atomically
  when the user explicitly resubmits that transaction. Keep the old attempt
  record and generation for callback/finality bookkeeping; detach its ownership
  binding for this hash. The newly queued request is owned by the pool until a
  new attempt takes it, following the existing model.
- Do this for both insertion and replacement through the same helper. Testing
  only insertion missed the replacement form of the bug.
- Do not clear a canonical terminal record or unresolved owner using an ingress
  nonce hint. If the nonce view conflicts with such a record, return an explicit
  retryable reconciliation error until the existing canonical recovery path
  establishes the outcome. Never acknowledge and then silently discard it.
- Old observers must still match the exact attempt generation and post hash.
  Neither `push_front_batch` nor an old observer may perform explicit
  readmission. Keep all existing `requeue_composing` guards.

This is not an unconditional reset of retries. Active duplicate submissions
keep their current attempts and owner. An explicitly accepted new admission
after a local terminal disposition follows the existing new-request policy.

### Excluded target slots use a separate bound

Status: implemented on branch `fix/excluded-slot-attempt-budget`; unit-tested
only. Not yet observed on a live network.

An `excluded` observation means the target block was built without the bundle:
by the proposer without MEV-Boost, by a builder that never received it, or by a
configured builder that left it out. No postBatch gas was spent. On Ethereum
mainnet on 2026-10-06, roughly a fifth of slots were built by validators or
builders outside the configured set.

The same day, outbound user `0x38b6…` (nonce 28) was evicted with
`AttemptsExhausted` after six charges on the shared counter:
- Three landed postBatches (L1 26132475–26132477) skipped its entry with
  `RollingHashMismatch`, plus `RootMismatch` for the dependent entries. The call
  read `slot0()` of the Uniswap V3 USDC/WETH 0.01% pool, and in each block the
  builder placed the bundle after at least one swap on that pool. In 26132600 no
  swap preceded it and the resubmitted call succeeded.
- Three excluded targets followed (26132478–26132480). They were built locally or
  by a builder outside the configured set, so no configured builder could have
  included the bundle.

The nonce gap then removed `0x293f…` (nonce 29), whose nonce stayed unspent. With
separate bounds the source would have stood at 3/6 settlement failures and 3/12
excluded slots, and remained eligible.

Reconciliation therefore distinguishes the charge. A landed postBatch that leaves
the user incomplete charges `attempts`, bounded by six (automatic) or three
(numeric), because each retry spends L1 gas. An excluded target charges
`excluded_slots`, bounded by `MAX_EXCLUDED_SLOT_ATTEMPTS = 12`, in both modes.
Either bound exhausts the user and creates the usual dependent nonce gap. The
prover/relay rejection bound and the per-rollup admission streak and backoff are
unchanged. Both counters stay in memory and are lost on restart.

Alternatives not taken:
- Raising the shared bound. A source that lands but can never complete would
  pay for more postBatches before eviction.
- Not charging exclusions at all. A bundle that every builder keeps rejecting
  would then never be evicted.
- Classifying the winning builder and charging only reachable misses. That is
  more precise, but the classification is heuristic: `extraData`, fee
  recipient and relay data are not authenticated. It is left for later.

Acceptance: a unit test shows excluded slots beyond the settlement bound neither
evict nor charge `attempts`, the twelfth excluded slot evicts, and a landed
failure still evicts at the settlement bound without touching `excluded_slots`.

## 2. Defer source conditions without aborting independent work

The existing `Unavailable(String)` combines source transaction conditions with
unavailable chain infrastructure. Introduce a structured source-condition error
at the existing EVM validation match in
`simulate_source_tx_on_with_retries`; do not parse error strings or duplicate
Revm's validity rules in the scheduler.

| Condition | Required handling |
| --- | --- |
| Source nonce too high | Keep the source and its same-chain successors owned; wait for the missing predecessor |
| Source fee cap below the pinned base fee | Defer that source chain; reevaluate at a new opportunity or replacement |
| Insufficient source balance | Defer that source chain; another sender can still be processed |
| Insufficient current rollup escrow | Defer the withdrawal; do not call it permanently invalid merely because this drain has no inbound deposit |
| Provider/database/transport unavailable or canonical generation changed | Preserve the existing slot-wide abort/recovery path |
| Malformed or otherwise permanently invalid composition | Keep the existing explicit terminal disposition and nonce-suffix handling |

The enabled policy's existing source-condition handling is the compatibility
boundary for the new structured error. Keep disabled-policy behavior explicitly
covered; do not broaden legacy error semantics incidentally.

Continue using the two canonical execution phases: outbound pairs first, then
inbound deliveries. A withdrawal cannot spend escrow supplied by an inbound
transaction later in the same physical bundle. Let that deposit settle, then
reconsider the withdrawal at a later opportunity. Read escrow from the pinned
L1 state, not an unrelated moving head.

A failed source validation happens in the candidate's fork. Discard that fork,
retain the already accepted prefix, defer the affected source chain, and
continue with independent candidates. An unavailable database or uncertain
shared state must still abort the slot. Never reuse effects from a failed fork.

### Selection must reach eligible work when its limit is one

Changing the exception handler alone is insufficient: `pop_n(1)` would never
look past the blocked head.

Use a finite snapshot of the existing queue and an eligibility-aware claim
operation under the same pool lock. Claim only a transaction whose hash and
nonce reservation still match the snapshot; a concurrent replacement requires
refreshing that candidate. Preserve existing attempt ownership while claimed.

For each opportunity:

1. Consider source-chain heads in stable queue order. Reuse the pinned source
   states and the accepted prefix for nonce/fee/balance readiness.
2. Skip successors of a blocked chain without simulating each one. Continue
   examining independent source chains while the preparation deadline permits.
3. Count admitted complete groups against the automatic selection estimate or
   an explicitly configured numeric limit. Blocked candidates do not consume
   those positions; with a limit of one, a blocked A must not hide ready B.
4. Revalidate readiness against the current state; no stale ready flag
   authorizes execution. The September 27 strict FIFO correction supersedes
   discovery rotation: an unclassified older source cannot be bypassed merely
   because discovery exhausted the deadline. Classified source-condition waits
   still allow independent eligible work.
5. Retain the gas, blob, whole-group, preparation, proof, and exact-target checks
   already implemented. Eligibility does not authorize exceeding any of them.

When an inbound candidate is deferred for a source condition, an outbound
candidate can become eligible after the outbound phase has already ended.
Preserve canonical phase ordering and reevaluate it at the next opportunity in
stable queue order. Capacity deferrals differ: they stop younger admission across
both directions. A canonical outbound-first pass that omitted an older eligible
inbound must be rebuilt before submission. The [strict FIFO correction](cross-chain-admission-throughput.md#september-27-correction-strict-fifo-admission)
defines the bounded repair and retained queue boundary. An unfinished discovery
scan cannot authorize promoting a later chain ahead of an eligible source.

Do not add a fixed count of users to examine or a per-chain timing constant.
Reuse the actual `RollupTiming`/target deadline. A transient source condition
alone must not train `AutomaticAdmission` to halve the whole network's capacity;
measured resource/time cuts and failed submission outcomes retain their backoff.

Preserve `SettlementPolicy` as the single authority for posting decisions.
Blocked requests contribute `CrossChainWork::None`. Only executable groups or
an existing validated finite historical predecessor contribute urgency. Pure
L2 activity, fullness, and the general consolidation rule still apply normally.

## 3. Report queue state and ownership decisions

Expose a read-only composer status through the existing L2 JSON-RPC wiring,
available over its configured HTTP and WebSocket transports. Report:

- Queued, in-flight, ready, and blocked counts, with source-state reference and
  timestamp. Readiness that has not been evaluated is unknown, not ready.
- Oldest pending age and the last canonical cross-chain completion; report
  unknown after restart until evidence exists rather than inventing a time.
  Report service age separately so an old source wait or idle period does not
  make newly executable work appear stalled.
- For a requested transaction or source chain: hash, direction, nonce, owner
  generation/lifecycle, blocking reason, and required versus observed nonce.
- Explicit local terminal and replacement outcomes while their existing
  records are retained. Do not expose signing keys or raw signed envelopes.

Keep this a bounded, non-mutating view of existing state. Do not return the
entire queue by default or perform source RPC calls while holding its mutex.
Record structured transitions with transaction identity, attempt generation,
canonical reference, and reason, including resubmission and recovery ordering.
Do not log every unchanged blocked request on every slot.

### Local terminal observations

The PR139 follow-up extends the existing telemetry with the most recent 1,024
local evictions/replacements per pool. Retain only hash, sender, direction,
nonce, reason, observation time, and optional replacement hash; never retain
the envelope. Simulation rejections add a separately bounded diagnostic
([simulation rejection diagnostics](#simulation-rejection-diagnostics)). This
is a bounded diagnostic cache, not an ownership journal, admission limit, or
source of recovery decisions.

Live pool and settlement-ledger records take precedence. Successful explicit
readmission clears that hash's local terminal observation; rejected admission
must not clear it. A delayed recovery callback cannot replay or reinstate an
observation. Existing authoritative generation checks remain unchanged.
Telemetry retention also clears superseded local observations when authoritative
ownership appears, so they cannot resurface after the ledger prunes that owner.

`eez_getCrossChainTransaction` may now return `lifecycle: terminal` for an
unsubmitted eviction, with `ownerGeneration: null` and `ownership.canonical:
null`. Local observations additionally expose `observedAtMs` and, on replacement,
`replacementHash` in `ownership`. This does not assert canonical consumption.
Older nodes return null once such records leave their pool; clients must treat
null as unknown. Old clients may ignore the additional fields. Oldest local
observations expire by count, and all are lost on restart. No migration,
configuration change, contract change, or mixed-node execution change is needed.

Finality pruning feeds the same cache. When the finality audit removes an
attempt from the settlement ledger, each released terminal user (`Committed`,
`AttemptsExhausted`, `NonceGap`, source outcomes) is recorded with its reason
and, for canonical outcomes, `ownership.canonical`. Previously such a request
answered null about one L1 finality period after its outcome, although it had
settled or been evicted. Recording happens under the pool lock with the pruning,
so a read never observes the gap. The entries share the same count bound and
restart loss, and remain observations: they never authorize execution,
admission or recovery.

### Simulation rejection diagnostics

A source whose composition fails deterministically is evicted before any
settlement attempt, with its higher nonces (`NonceGap`). Two causes qualify:
- the source transaction reverts in simulation (`source transaction does not
  execute successfully`);
- the composition error is itself deterministic (for example a source that
  makes no cross-chain call, or an unsupported call shape).

Such a root reports `ownership.reason: "SimulationFailed"` instead of
`PoolDiscarded`. This holds whether it was still queued, in flight, or a
previously submitted ledger user being recomposed. `PoolDiscarded` remains for
replacement, the other composition cuts (shape, budget, validation) and
recovery loss.

`ownership.rejection` carries a bounded diagnostic:

| Field | Content |
| --- | --- |
| `error` | Display form of the composition error that caused the eviction |
| `source` | `{gasUsed, output, outputLength}` when the source simulation ran, else null; `output` is the revert data, truncated to 512 bytes |
| `calls` | Up to 16 recorded cross-chain calls in preorder: `{depth, sourceRollupId, targetRollupId, caller, target, selector, static, success, gasUsed, returnData, returnLength}`. `target` is the original address the proxy represents, `returnData` is truncated to 512 bytes, and `success`/`gasUsed` are null for a call that had not returned |
| `omittedCalls` | Recorded calls beyond the bound |

With nested calls, the failing leg is the deepest call with `success: false`.
Its `returnData` is the target's revert data, which an application may not
propagate to the source output.

The node keeps the 256 most recent rejections per pool, separately from the
removal records, because a previously submitted user's status is answered from
the settlement ledger. A successful readmission of the same hash clears its
rejection, and a rejection is shown only with a `SimulationFailed` reason.
The diagnostics are observations derived from local simulation of the user's
own transaction. They contain no signed envelope and never authorize
execution, admission or recovery. They are lost on restart.

The Composer also logs each intercepted call's return or revert data
(`return_len`, `return_data` truncated to 256 bytes) on its `cross-chain proxy
call detected` line.

Compatibility: both fields are additive. A client that matches `PoolDiscarded`
to detect any pre-settlement eviction must also accept `SimulationFailed`; one
that treats every reason other than `Committed`/`SourceCompleted` as a failure
needs no change. Older nodes keep reporting `PoolDiscarded` without
`rejection`. No storage, wire, configuration or contract change is involved,
and a rollback only removes the new reason and field.

The observatory currently computes `healthy` from L1/L2 RPC health and head
freshness in `eez-rollup-ui/network-observatory/app/server.py`. Add an explicit
cross-chain processing status so fresh heads cannot produce an unqualified
healthy indication while ready work is stalled. Missing queue telemetry is
unknown. An expected wait for funds/predecessors must be distinguishable from
ready work that receives no service. Ordinary inactivity is not a stall.

## Components and implementation order

| Change | Existing components | Required validation |
| --- | --- | --- |
| First-hole admission, ordered insertion, explicit readmission | `crates/eez-composer/src/held_pool.rs`, `optimistic.rs`; admission errors in `crates/eez-node/src/ingress.rs` | Rollback, replacement, generation, duplicate callback, overflow, and directional nonce tests |
| Typed source deferral and eligible selection | `crates/eez-protocol/src/error.rs`, `crates/eez-composer/src/local/client.rs`, `composer.rs`, `composer/admission.rs` | Source-only versus slot-wide failure tests; limit-one independent progress; whole-group budget tests |
| Queue/status observations | Existing composer/pool accounting and `eez-node-common`/node RPC installation | Status agrees with accepted, deferred, replaced, terminal, and canonical outcomes; repeated events are idempotent |
| Monitor integration | Separate `eez-rollup-ui` observatory and its existing tests | Fresh heads with a blocked queue; missing telemetry; intentional inactivity; recovery to working status |
| System behavior | Existing `eez-node-e2e`, `eez-testkit`, native relay and reorg controls | Ethereum and Gnosis profiles, canonical receipts, and independent follower parity |

Implement the pool/ownership fixes first, then sender isolation, then expose
their actual state to the monitor. Do not present the pool prototype alone as
the complete liveness correction.

## Acceptance tests

Use existing fixtures, including `hold_next_bundle`, `release_bundle`, partial
consumption controls, and `Anvil::reorg`; no parallel integration framework.

1. **Recovery across generations:** confirm N, submit N+1 in a later attempt,
   then orphan N while the later attempt is unresolved. Reconcile both in
   different orders. Queue and canonical execution must remain N, N+1, N+2.
2. **Admission racing rollback:** admit a successor while its predecessors are
   provisional. Exclude that attempt and roll back. Preserve or explicitly
   account for the predecessors; permit gap repair and reject new successors
   past the first missing reservation.
3. **Resubmission and replacement:** exhaust or locally discard an attempt;
   explicitly readmit the same signed request, including as a replacement.
   Delayed old callbacks and finality pruning cannot remove or reclaim it.
   Canonical/unresolved owners cannot be reset with stale ingress data.
4. **Independent progress:** while A is nonce-blocked, B must settle with
   automatic and explicit selection limits of one. Repeat with insufficient
   balance, fee cap, and escrow, and with the two source directions swapped.
   Repair/fund A and verify it resumes in order, without duplicate effects.
   Exercise the phase-boundary case above: B receives first discovery priority
   next opportunity when its canonical phase has already ended.
5. **Long blocked suffix:** thousands of A successors cannot hide B or require
   thousands of simulations per opportunity. Verify bounded discovery and
   progress across deadlines and newly arriving traffic.
6. **Preserve posting rules:** a blocked-only queue produces no extra posts
   before another policy rule is due. Pure traffic still triggers its configured
   mode, and idle consolidation still fires at the general threshold.
7. **Canonical and resource safety:** partial settlement, several events/posts
   in one L1 block, same-height replacement, exclusion, proof failure, stale
   source reads, gas limits, and multiple blobs retain existing invariants.
8. **Compatibility:** active duplicates, the 10% replacement rules, in-flight
   reservations, ordinary mempool predecessors, disabled policy, fixed limits,
   source-chain separation, and restart/resubmission remain covered.

Run the affected unit/component tests and the existing dynamic-admission,
partial-consumption, settlement-policy, and cross-chain native suites on both
Ethereum and Gnosis profiles. Verify canonical source receipts exactly once
and match an independent follower's safe block hash/state root. Then validate
a bounded blocked-sender/healthy-sender scenario on Chiado with its actual
timing and resource profile. Keep the PR in draft until that validation passes.

## Prototype evidence and limits

Six diagnostic tests were added temporarily to the real composer test module.
The final pool prototype passes **149 composer tests**, including all **1,440**
permutations of six recovered nonces across inbound and outbound queues. The
experiments use existing pool and accounting implementations, not a parallel
model. The generation test exercises ledger invalidation and reconciliation;
it does not replace the required native L1-reorg test.

Against the original pool code, five of those six diagnostic tests fail. The
sixth confirms both the old unguarded-restoration failure and the effectiveness
of the current guard. After restoring all production sources and removing the
temporary tests, the original **143 composer tests pass**, and `crates/` matches
the tested base commit. See the audit for the exact comparison commands.

Local source patches, logs, and identities are retained under
`artifacts/chiado-nonce-investigation-20260915/deep-analysis/`. Production source
changes used for those experiments were restored before implementing the final
correction. The prototype
does not implement sender isolation, canonical-conflict admission handling,
ordinary-pool integration changes, RPC health, or native acceptance tests.
No live Chiado state was modified during this deeper analysis.
