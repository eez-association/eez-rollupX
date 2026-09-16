# Configurable Settlement Policy and Multi-Blob Packing

| Field | Value |
| --- | --- |
| Status | Implemented; validated in native tests, Kurtosis, and public Chiado runs. See [implementation coverage](configurable-settlement-policy-implementation.md) for revision-specific evidence and limitations. |
| Date | 2026-09-12 |
| Source baseline | `40134955dc3f698ea65571ba82b6521637bf1191` |
| Networks | Ethereum mainnet and Chiado; the same policy implementation with network-specific timing and limits |
| Scope | Settlement decisions, activity tracking, multi-blob packing, configuration, existing recovery integration, and validation |
| Encoding | Existing payload and native blob formats; compression and byte-string format changes are excluded |
| Review | [Requirement coverage and implementation interpretations](configurable-settlement-policy-implementation.md) |
| Accounting baseline | Partial-settlement reconciliation is included in implementation baseline `71a8f52` (PR #16); consume it without duplicating that work |

The requirements in section 1 are confirmed by the user. The remaining sections
define the implementation contract. **MUST**, **MUST NOT**, **SHOULD**, and **MAY**
are normative; the linked coverage ledger records implementation and validation.

The implementation has three main parts: typed configuration, a small policy
module, and packing/integration in the existing composer. The normal flow is:

1. Run existing recovery and respect the unresolved-attempt gate.
2. Read the canonical anchor, pending activity, estimated bytes, and eligible
   cross-chain work; evaluate the rules together.
3. If nothing is due, produce the normal empty Sync block without posting.
4. If due, pack the complete candidate into one multi-blob batch. Only actual
   blob/gas overflow invokes selection of a valid smaller prefix.
5. Use the existing ECDSA proof and submission paths; update the policy from
   validated settlement outcomes and continue any in-memory drain.

The detailed sections below define integration checks and acceptance evidence.
Version 1 adds four settlement-trigger settings and volatile policy state, and
updates the existing gas/fee configuration to reflect the active network. It
reuses the existing prover, recovery/accounting interface, witness store, and
chain profiles. It adds no policy database, background proof service, hot reload,
price-timing optimizer, automatic prover tuning, or storage/backpressure redesign.

## 1. Confirmed product rules

1. At least one cross-chain transaction requires settlement at the next eligible
   opportunity. An empty scheduled Sync block does not independently trigger
   this rule.
2. Pure L2 settlement has two selectable modes:
   - `always`: non-empty L2 blocks during the slot require settlement, including
     when the terminal Sync block is empty;
   - `interval`: settle when there are unposted pure L2 transactions and the
     configured duration has elapsed since the latest L1 settlement.
3. A configurable general rule, such as every 150 unposted L2 blocks, requires
   consolidation even when every block is empty.
4. The general rule applies independently of the pure L2 rule. A longer pure L2
   interval cannot postpone a settlement already required by the general rule.
5. A settlement MUST use multiple blobs in one batch when necessary. Split only
   when the complete candidate exceeds the effective blob allowance or L1
   execution-gas budget; the more restrictive constraint wins. Do not split at
   each individual blob boundary. Prover capacity tuning remains manual and is
   not an automatic splitting policy in this implementation.
6. Ethereum and Chiado MUST both work. Policy thresholds and operational limits
   must be configurable; network rules must not be replaced with universal
   Ethereum-specific values.
7. Encoding and compression changes are not part of this implementation.
8. A configurable blob-fullness threshold is an independent early-settlement
   trigger. Its denominator is the total usable capacity of **all blobs allowed
   in one batch**. If ten blobs are allowed, 10% means one full blob's worth of
   encoded bytes. Fullness cannot postpone any other due rule and is not a cap
   on how much an already-due batch may contain.
9. Fullness is a soft target based on estimated usage. A 10% target does not
   require exactly 10% utilization or prohibit posting at 12% or 15%. Actual
   network blob/gas limits remain hard constraints, checked on the final batch.
10. Pending policy intent may be lost on restart. Keep it in memory during a
    running process; after restart, reevaluate the canonical backlog using the
    latest validated L1 settlement and the current configuration. An unfinished
    drain may wait for the normal rules again.
11. Reuse the existing fast ECDSA proof path, scheduling, timeouts, and retries.
    This feature adds no historical-proof worker or separate proof timeout.

## 2. Terminology and boundaries

- **Scheduled Sync block:** an L2 block at a scheduler Sync boundary. It may be
  empty. Its existence alone is not a settlement reason.
- **Cross-chain work:** an executable cross-chain transaction group selected for
  composition, including all associated user and system transactions, or valid
  work that needs a finite historical predecessor first. That predecessor
  inherits the group's urgency before full-batch admission succeeds.
- **Pure L2 activity:** an included ordinary L2 transaction without cross-chain
  effects. A reverted included transaction counts as activity; a mempool
  submission that has not entered a block does not.
- **Settlement anchor:** the latest canonical L1 event that the deriver has
  validated as advancing this rollup's commitment, together with its L2 height
  and hash and its containing L1 block number, hash, and timestamp.
- **Eligible opportunity:** a target L1 slot for which the existing parent,
  timestamp, proof, relay, and in-flight checks allow submission. This is not
  synonymous with every L2 block or an unconditional inclusion guarantee.
- **Batch:** one `postBatch` transaction, with one or more native blobs.
- **L1 bundle:** the ordered `postBatch` plus its required companion L1 source
  transactions. Inbound transactions execute in this bundle; outbound source
  transactions execute on L2 and are carried in DA. A batch and a bundle have
  different execution-gas budgets.
- **Drain target:** an L2 endpoint that a due settlement must cover. Reaching an
  intermediate endpoint during capacity-driven splitting does not complete it
  during the running process. The target is held in memory and may be discarded
  on restart as described in section 3.3.

“Latest L1 settlement” means the canonical, validated commitment advancement
above, not RPC acceptance, a successful receipt without applied state, the
observer's provisional verdict, or Ethereum finality. Existing finality rules
remain unchanged. A reorg can invalidate an anchor.

This feature changes L1 posting decisions. It MUST preserve the configured L2
timestamp grid, Sync placement, exact block reconstruction, and cross-chain
atomicity. Reuse the existing ECDSA proof flow and preserve the existing Chiado
timing profile. This feature does not introduce a strict L2 admission cap at the
fullness percentage or a new partial-settlement implementation. Section 14
records these scope clarifications.

## 3. State and configuration

### 3.1 Policy configuration

Proposed configuration, shown with illustrative values rather than production
defaults:

```yaml
settlement:
  pure_l2:
    mode: interval
    interval: 15m
  general:
    max_unsettled_l2_blocks: 150
  blobs:
    fullness_threshold_bps: 1000  # 10% of the effective total blob allowance
```

The initial implementation SHOULD expose equivalent typed settings through the
repository's existing startup configuration mechanism. A new YAML loader or
runtime administration API is not required. Proposed environment names are:

| Setting | Proposed environment variable | Validation |
| --- | --- | --- |
| Pure L2 mode | `EEZ_SETTLEMENT_PURE_L2_MODE` | Required; exactly `always` or `interval` |
| Pure L2 interval | `EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS` | Required and positive in `interval` mode; if supplied in `always` mode, validate it and report it as inactive |
| General block threshold | `EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS` | Required, positive, checked arithmetic |
| Aggregate blob-fullness threshold | `EEZ_SETTLEMENT_BLOB_FULLNESS_BPS` | `off` or an integer from 1 to 10,000 inclusive; 100 basis points = 1%; unset means disabled |

Policy values MUST be parsed once into per-rollup configuration. Invalid values
MUST fail startup with an actionable error; they MUST NOT silently select an
unrelated fallback. Deployment profiles supply explicit values. The example
150-block threshold is not a hard-coded algorithm constant.

Profiles SHOULD explicitly select a fullness threshold or `off`. Zero, negative
values, fractions, and values above 10,000 are invalid; use whole basis points
for fractional percentages, for example 1250 for 12.5%. No production default
percentage is selected by this spec. Disabling fullness leaves the other rules
active. Startup-only configuration is sufficient; hot reload is out of scope.

The implementation MUST distinguish policy settings from:

- active network/fork rules;
- prover, memory, request, and concurrency guards;
- transaction gas and fee settings;
- the amount of work allowed per sequencer catch-up tick.

Existing configured fee controls remain available as explicit overrides. Remove
the current implicit 10-gwei postBatch priority-fee fallback from production;
that value comes from test transaction-ordering assumptions. Use network fee
estimation by default, with validated per-network margins and caps as specified
in section 6.3.1. Production profiles state their fee policy explicitly; test
ordering overrides belong in test profiles. Fee settings are operational
configuration, separate from the four settlement-trigger settings.

Validate compatibility with the existing `EEZ_MAX_SPECULATIVE_DEPTH` setting.
That setting limits how far L2 production runs ahead of the confirmed cursor;
it is not a settlement trigger. For version 1, require it to be zero (disabled)
or at least `N + K - 1`, using checked arithmetic, where `N` is the general block
threshold and `K` comes from the existing timing profile. The additional `K - 1`
allows the threshold to round forward to a valid Sync boundary, including after
an anchor off the usual grid. Reject incompatible settings at startup with the
required minimum; do not silently change the policy or disable the guard.

This is a conservative configuration check that keeps steady production able
to reach the general trigger. The current node default is 64, while the existing
Chiado and Kurtosis profiles explicitly use zero. Preserve those explicit
profile settings. Catch-up production bounds remain unchanged; a guard-induced
pause must not be treated as a new reason to post an otherwise not-due batch.

### 3.2 Per-rollup state

The decision engine needs:

```text
anchor = (l2_height, l2_hash, l1_number, l1_hash, l1_timestamp, settlement_identity)
candidate = (terminal_l2_height, terminal_l2_hash, target_l1_timestamp)
pending_activity = canonical transaction-bearing blocks above anchor.l2_height
pending_da_size = estimated encoded byte accounting keyed by canonical range and codec version
cross_chain_work = admissible groups and prerequisite settlement dependencies
due_work = in-memory anchor generation, reasons, and required L2 endpoints/hashes
attempt = existing accounting/submission state, including input hashes, held transactions,
          ordered L1 bundle, nonce ownership, target, and lifecycle phase
network_limits = resolved rules for the target L1 fork
```

During a running process, retain pending activity and due intent through skipped
slots and failed submissions, and reconcile them on L1/L2 reorg. On restart,
discard policy intent and rebuild activity and size accounting from canonical
chain data. Canonical blocks and validated L1 effects establish what actually
settled; cache entries and cursors cannot override them. Cache entries MUST be
keyed by block hashes and invalidated on reorg. Existing transaction ownership
and attempt recovery remain the responsibility of the integrated accounting
and submission paths.

Before the first settlement, the registered/genesis rollup commitment and its
canonical L1 registration block provide the initial anchor. If that evidence
cannot be resolved, report bootstrap readiness failure rather than starting a
fresh timer from process startup.

### 3.3 In-memory policy state and restart

Keep drain endpoints, trigger reasons, and derived policy caches in memory. No
policy journal, durable policy revision, or storage migration is required.

After restart, recover the latest validated L1 settlement through the existing
chain database/checkpoint, rebuild the pending canonical range, and evaluate
the normal rules with the current startup configuration. The timer still starts
at that settlement's L1 timestamp; process startup does not become a new anchor.
If a rule is already due, attempt at the next eligible opportunity.

For example, if a range needs three batches and the process restarts after the
first settles, the remaining range may wait until a normal rule becomes due
again. Remembering its old drain target is unnecessary. This delay is accepted.
During an uninterrupted process, keep the target until covered so intermediate
batches do not make the remainder wait for another interval.

Use the accounting/recovery interface supplied by the other worktree for held
transactions, source outcomes, attempt identity, and nonce ownership. Discarding
policy intent does not change that existing recovery. An attempt unresolved at
restart is reconciled through the existing path before another submission.
The feature adds no transaction-ownership journal or recovery protocol.

Startup-only configuration governs the rebuilt backlog immediately, including
an increased interval or disabled fullness setting. Preserve the existing
one-in-flight gate, adoption of valid batches posted by other composers, and
posting-account nonce ownership. External settlement updates the canonical
anchor, completes covered in-memory intent, and invalidates stale candidates.

## 4. Decision rules

At each scheduler opportunity, use one consistent anchor/candidate snapshot.
Let:

```text
C = last validated settled L2 height
H = candidate terminal Sync height
A_ms = containing L1 timestamp of the latest anchor, checked-converted to milliseconds
T_ms = candidate target L1 slot timestamp, checked-converted to milliseconds
N = configured general block threshold
D_ms = configured pure L2 interval in milliseconds
P = unposted pure L2 activity exists in (C, H]
X = executable cross-chain work or its valid finite historical predecessor is pending
B = effective number of blobs allowed in one batch at the target fork
U = usable stream bytes per blob from the active codec
E_est = estimated encoded, unpadded stream bytes for the complete candidate range (C, H]
F = configured fullness threshold in basis points, when enabled

general_due       = H > C and H - C >= N
pure_always_due   = mode == always   and P
pure_interval_due = mode == interval and P and T_ms >= A_ms + D_ms
cross_chain_due   = X
blob_fullness_due = fullness_enabled and H > C and E_est * 10_000 >= F * B * U

due = cross_chain_due or general_due or pure_always_due
      or pure_interval_due or blob_fullness_due
      or in_memory_due_work_not_yet_covered
```

The final term retains a finite drain during the current process only. It is
empty after restart; the other predicates are evaluated from the recovered
anchor and current backlog as specified in section 3.3.

Arithmetic MUST use checked, consistently typed units. Duration comparison
uses target L1 time so proof preparation may start before a deadline without
submitting to a target earlier than it. Do not derive a duration by multiplying
an L1 block count by a universal 12 seconds: skipped slots and Chiado differ.
Anchor identity includes the L1 block and attributed settlement position, not
just a height. Process duplicate evidence once. Source-transaction completion
without a new L2 commitment does not reset the L2 settlement timer or block
count; it is recorded separately. A valid commitment change at the same L2
height may update the anchor hash/time, but does not complete an unrelated drain.

The OR relationship is intentional. Empty-block consolidation does not wait
for the pure L2 timer, and cross-chain work does not wait for any other threshold.
Several simultaneous reasons produce one candidate covering the pending range,
not one transaction per reason.

In `always` mode, checking only the terminal Sync block is incorrect. The
implementation MUST include activity in intervening L2 blocks. Once a non-empty
slot makes settlement due, subsequent empty slots do not erase that obligation.
After restart, any still-pending pure L2 activity makes `always` due again.

A general-rule, timed-rule, or fullness-rule drain captures its required endpoint
when it becomes due. Repeated evaluation against the same unresolved anchor MUST NOT
extend that endpoint on every empty tick. Otherwise one slow confirmation could
turn periodic consolidation into continuous empty posting. New `always`-mode
activity and cross-chain work may record additional obligations. A fresh
candidate may include newer blocks, but opportunistic inclusion is distinct
from extending the obligation.

When validated settlement covers the captured canonical endpoint, clear that intent
and evaluate remaining activity against the new anchor. Pure transactions that
arrived after the prepared batch are governed by the new timer unless another
rule independently requires them. An unfinished capacity-driven drain retains its
original endpoint despite intermediate anchor advancements.
Do not open a replacement general/timed/fullness drain just because an
intermediate prefix created a new anchor while the original drain is unfinished.
These intent-retention rules apply during an uninterrupted process; restart
discards the target and reevaluates the normal predicates.

Coverage means the expected hash is in the validated canonical settled history
and associated transaction obligations are accounted for; `cursor >= height`
alone is insufficient. If a partial application replaces a block at the same
height, reconcile the replacement and requeue eligible unconsumed transactions
under new identities before releasing the old intent.

If no new range or valid recovery work exists, do not post a zero-progress batch.
Same-height commitment recovery is handled by reconciliation and MUST NOT be
discarded solely because `H == C`. If no rule is due,
produce the scheduled L2 block through the existing no-emission path. Do not
invoke witness collection, proving, KZG generation, signing, or relay submission
solely because an empty Sync boundary was reached.

Thresholds mean “attempt at the first eligible opportunity once due,” not
“guaranteed included exactly at N blocks or D milliseconds.” When N is not a
multiple of the configured L2-blocks-per-L1-slot ratio, retain N unchanged and
evaluate it at the next eligible boundary. Expose lateness and the reason for
any additional delay.

### 4.1 Aggregate fullness accounting

Estimated fullness is `E_est / (B * U)`, not `ceil(E_est / U) / B` and not the
utilization of just the blobs currently allocated to the candidate. Its sizing
model includes existing block
encoding, entries, groups, semantic messages, and stream framing through the
close marker. It excludes unused trailing padding and the field-element bytes
reserved by the codec. Actual zero-valued payload bytes still count; counting
nonzero bytes would be incorrect.

The current codec exposes `DATA_BYTES_PER_BLOB = 4096 * 31 = 126,976`. The policy
MUST obtain this value from the codec, not duplicate it. Under this format:

| Effective allowance | Threshold | Nominal estimated byte target |
| --- | --- | --- |
| 10 blobs | 10% / 1000 bps | 126,976 bytes: one full usable blob |
| 10 blobs | 80% / 8000 bps | 1,015,808 bytes: eight full usable blobs |
| 6 blobs | 10% / 1000 bps | 76,186 bytes: rounded up to the first qualifying byte |

These are hypothetical allowance inputs, not claims about either network's
current limits. Use checked wide-integer arithmetic for both products in the
predicate; equality of the estimate triggers settlement. Equivalently, the byte target is
`ceil(F * B * U / 10_000)`. A partially occupied first blob does not automatically
count as one full blob. For example, with ten allowed blobs and a 10% threshold,
126,975 estimated bytes are below target even though one physical blob is
allocated. This is an accounting example, not a one-byte precision requirement
for when a real transaction lands. Headers of empty L2 blocks count, so an idle backlog can trigger this
rule before the general block threshold.

Estimate the complete candidate before selecting a smaller prefix; an oversized
backlog must not appear below threshold merely because it was truncated for
packing. Crossing the threshold by a large block still makes settlement due.
At 100%, a candidate that jumps from below capacity to above capacity is due
and uses the splitting rules in section 6. The threshold need not be hit exactly.

Use cheap incremental sizing, preferably measured/cached block bytes plus
framing and semantic overhead. An exact size function is allowed but not
required on every tick. Do not use an unexplained fixed average block size as
the only accounting input when actual encoded transaction sizes are available.
Report the estimate, final encoded bytes, and their difference so estimation
drift is visible. No extra percentage-tolerance or packing cap is implied.

The target is flexible in both estimation and timing. If usage is estimated at
9% at one opportunity and 12% at the next, settle the 12% candidate if its actual
encoding and gas fit. A valid final size below the estimate also need not cancel
an already triggered attempt. Do not delay other due rules to approach the
percentage or reject ordinary L2 transactions merely for crossing that target.

Fullness triggers a settlement attempt at the next eligible boundary. Pack all
due work that fits the actual budgets; do not intentionally stop at the fullness
percentage or add padding to reach it. An immediate cross-chain settlement may
therefore use much less than the threshold. Exact final encoding enforces the
real maximum blob allowance; the soft fullness estimate does not replace that
hard check.

Resolve `B` from the target fork and explicit supported posting caps, not from
the block fee target, observed competing transactions, or a prover request cap.
A gas constraint is checked independently; do not convert a gas estimate into
an invented number of available blobs. Recompute the threshold when the active
allowance changes. A lower allowance can make pending work due immediately;
a later increase during the same process MUST NOT cancel a previously captured
canonical drain intent.
After that intent is covered, evaluate the remaining range with the new anchor
and allowance. After restart, use the current allowance without restoring old
intent. A reorg can remove the underlying obligation as in section 8.

### 4.2 Slot modes and the existing proof path

Here, "historical" means L2 blocks already produced since the last settlement.
The user confirmed that the existing ECDSA proofs are fast. Reuse the current
proof path, scheduling, timing budgets, retries, and validation. This feature
requires no new background worker, separate historical-proof timeout, or
architecture for long-running proofs. Prover resource sizing remains manual.

The decision engine must run independently of whether the current Sync block
can carry fresh cross-chain work:

| Slot mode | Fresh cross-chain composition | Already-due historical consolidation |
| --- | --- | --- |
| Steady, target deadline available | Allowed after admission checks | Allowed |
| Catch-up at a past/capped terminal | Held for a steady opportunity | Allowed against a fresh future L1 target |
| Late/empty Sync | Held for a steady opportunity | Allowed against a fresh future L1 target |
| Previous attempt unresolved | No overlapping attempt | Retain intent; no overlapping attempt |

The user authorized a latency follow-up on 2026-09-15 after a measured Chiado
bridge wave. Completing catch-up MUST NOT discard an otherwise eligible future
opportunity: if the terminal equals the requested height and timestamp and still
satisfies the existing deadline and speculative guard, classify that terminal
as steady and use its normal Future suffix. Intermediate, capped, past or
misaligned terminals remain empty; canonical parent checks and single-attempt
ownership remain mandatory. This refines slot classification without changing
the Sync grid, catch-up production bound, or settled derivation.

This follow-up also permits optional operational `EEZ_PREPARATION_TIME_MS`:
positive integer milliseconds, unset by default. Unset MUST preserve the existing
rounded trigger. Supplied MUST reserve preparation + proof + relay slack with
millisecond precision, while retaining whole-second L2 timestamps. Their sum
MUST fit `(K - 1)` L2 blocks. Invalid/empty/zero/overflowing values MUST fail
startup. The Future suffix must cover the combined budget. This setting is
startup-only and does not add a worker, automatic proof tuning, or permission
to miss existing proof/submission deadlines. Operators size it from measured
workloads, including multi-blob historical batches.

Automatic cross-chain admission MUST distinguish per-blob KZG preparation from
transaction-dependent work. After one cold-start sample it considers the finite
FIFO snapshot rather than imposing a doubling ramp from the previous small
post. Before accepting each complete group it checks remaining measured time,
including any additional physical blob, alongside the existing gas/DA limits.
Effect-prefix commitments MAY be computed incrementally and reused after the
canonical transaction/position checks, so their repeated execution is charged
to elapsed admission time instead of extrapolating the entire finishing tail
quadratically. The finishing allowance MAY use the currently measured admission
work and the previous finishing sample, keeping physical-blob KZG separate;
growing a candidate must not multiply a small prior post's fixed overhead.
The allowance MAY use a measured relationship between accepted-group work and
non-KZG finishing time, retaining the previous finishing observation as a floor
with the existing proportional reduction for a smaller candidate. Rejected,
source-deferred, rebuilt or inconsistently measured work MUST NOT train a cheaper
reserve. An untouched capacity-deferred suffix contributes no measured work and
does not invalidate a clean accepted prefix. An explicitly time-refused group
MAY have its matching measurement cancelled before canonical append, preserving
an otherwise clean accepted prefix. Its rejected duration and prospective blob
count MUST NOT enter that prefix's sample, and final accepted/measured group
counts MUST still agree. Unknown refusals, source-state deferrals and rebuilt
candidates retain the full-pass fallback. Record the relevant measured phases
and calibration eligibility; no scenario-specific counts, fixed timing ratio
or test identity governs admission.
Exclusion/incomplete-preparation backoff remains mandatory.
An uncharged exclusion whose exact target is proven absent between consecutive
canonical L1 timestamps MAY retain the existing admission estimate when no post
or settlement was applied and all sources remain retryable. Bind that proof to the completed
canonical scan. A relay label, uncertain outcome, partial consumption or missing
canonical evidence MUST NOT grant this exception.
A successful capacity-limited FIFO prefix need not cap the next selection;
cuts that bypass older eligible work MUST retain shrinking FIFO lookahead to
prevent starvation under outbound-first execution. Nonce order, source ownership,
and final exact validation remain authoritative; local deferral does not consume retry attempts.
A performance estimate MUST NOT permanently block all opportunities to learn:
if it prevents admission of even the first group, discard that estimate and
allow a bounded one-group probe next time, subject to the same hard guards.
Source-state waits and transport failures must retain their existing retry
semantics rather than resetting performance estimates indiscriminately.

Do not remove the old 300-block branch and accidentally put all historical
settlement behind the late-slot early return. A chronically late sequencer must
still be able to drain a due, valid historical range.

Prepare the selected valid endpoint through the existing witness, ECDSA proof,
and sidecar flow. Preserve its target/deadline checks and revalidate the anchor,
hashes, limits, gas, fees, and nonce before dispatch. Historical submission uses
the existing future-target path and respects any interval rule's earliest
permitted target timestamp. Fresh cross-chain observations must be recomposed
if their slot is missed; stale candidates cannot be dispatched after a reorg.
Retain the existing one-in-flight and recovery behavior. A proof timeout remains
an ordinary resource/error outcome, not a new capacity-driven split rule.

Represent policy skip, prepared empty Sync, stale parent, and build failure
explicitly at the composer/sequencer interface. In the current API, returning
`None` invokes a mempool-driven Sync fallback. A policy skip MUST NOT use that
fallback and accidentally create a transaction-bearing terminal that cannot
be proved by the anchor-only historical path.

The outcome handling must be explicit:

| Outcome | Sequencer action |
| --- | --- |
| Not due, unresolved attempt, or locally deferred posting; empty Sync built successfully | Commit that empty Sync on the configured grid |
| Due batch composed and Sync built successfully | Use the existing submission/commit integration |
| Parent changed during composition | Discard the stale candidate; use existing recovery and rebuild from the current head |
| Required empty Sync cannot be built | Report the build error and end this tick; the next trigger uses existing catch-up behavior |

The last two outcomes do not fall through to a pool-driven Sync block. This
defines handling at the existing composer boundary; no new scheduling service
is required.

## 5. Cross-chain composition and submission

1. Run existing failed-batch recovery before making a new decision.
2. Preserve the one-in-flight gate. A due reason can remain pending while a
   previous attempt is unresolved.
3. Use cheap pool/activity checks to identify possible work. Validate and admit
   cross-chain groups using existing dependency, nonce, simulation, and gas
   checks before treating them as executable Sync content.
4. If all proposed cross-chain transactions are invalid or deferred, reevaluate
   the other due reasons. A rejected mempool item alone must not create an
   indefinite stream of empty `postBatch` transactions.
5. When admitted cross-chain work requires earlier unsettled history, include
   that history in the same batch if both blob and gas budgets allow it.
6. If an actual capacity constraint requires predecessor batches, settle valid
   historical prefixes first while retaining the cross-chain intent. Recompose the eventual
   cross-chain Sync block against a fresh L1 target; do not reuse a proof or
   timestamp pin from a missed slot.
7. Preserve all atomic groups. Never split a user/system pair, its callbacks,
   nonce dependencies, or its committed root sequence merely to fill a blob.

“Always” never authorizes violating transaction validity, timestamp alignment,
canonical parent checks, proof validity, or resource guards.

Admission must distinguish these outcomes instead of treating every failure as
an empty pool:

- **Executable:** contributes cross-chain urgency and may be composed now.
- **Needs a historical predecessor:** valid work has a concrete earlier range
  that must settle first. Latch that finite predecessor and the group identity
  even if composing the complete batch would exceed capacity.
- **Temporarily unavailable:** for example, insufficient funds or an unresolved
  external prerequisite. Keep its ownership and retry according to existing
  rules, but do not manufacture empty predecessor posts that cannot help it.
- **Capacity-deferred:** other complete groups were admitted first. Keep the
  deferred group pending for a later opportunity without charging a failure.
- **Permanently invalid or individually unpostable:** record an explicit
  disposition and apply nonce-chain rules; never silently forget the user.

Reevaluate other due rules even if every cross-chain group is deferred. A
policy skip, local capacity deferral, or proof work cancelled by a changed
generation is not a failed transaction attempt. Preserve the existing handling
of transport errors, skipped slots, target reorgs, and actual transaction errors.

The batch packing rule concerns the selected range and complete admitted groups.
It does not require draining an unbounded live mempool in one opportunity.
Existing group-admission limits, including `EEZ_MAX_USER_TXS_PER_BUNDLE`, must be
explicit validated startup settings and observable as admission limits; they
must not be mislabeled as exhaustion of blob capacity or split committed history.

## 6. Multi-blob packing and batch splitting

### 6.1 Effective blob allowance

Resolve the following separately for the target chain and fork:

```text
effective_blob_limit = min(
    network_max_blobs_per_transaction,
    network_max_blobs_per_block - required_companion_blob_count,
    supported_transport_limit,
    explicitly_configured_operational_limit_if_any
)
```

The subtraction reserves only blobs in required transactions of our own bundle,
not speculative competition from other users. For ordinary historical/pure-L2
posting the companion count is zero. Count companion blobs when the supported
transaction path allows them; reject unsupported companion transaction formats
at admission. An exhausted or invalid allowance prevents candidate dispatch;
do not divide by zero or treat unknown capacity as unlimited. Cross-chain work
is already due independently of any change in its fullness denominator.

The per-block fee-market **target** is not a transaction capacity limit. Nor does
the maximum block capacity guarantee that a builder has space available for us.
Temporary competition for block space is a retry/inclusion issue, not permission
to invent a smaller persistent blob limit.

The source and value of every limiting term MUST be observable. Do not silently
apply a one-blob operational cap. Explicit operator caps must be named and
reported rather than mistaken for network limits.

### 6.2 Packing algorithm

For already-due work:

1. Select the complete pending range and valid terminal composition that would
   satisfy the drain target.
2. Encode it using the existing payload and semantic-message formats. Count
   exact encoded bytes and blobs, including metadata and framing.
3. Check the complete candidate against both the effective blob allowance and
   the L1 gas budget in section 6.3. If both fit, use one `postBatch` carrying the
   required blob count. A one-blob boundary or the configured fullness trigger
   is never a reason to split otherwise fitting data.
4. Only if one of those budgets is exceeded, choose the largest admissible
   prefix ending at a valid settlement boundary. Rebuild endpoint-specific
   metadata and recheck both exact encoded size and gas. Record `blob_limit`,
   `l1_tx_gas_budget`, `l1_bundle_gas_budget`, or combined reasons, including the source of the effective limit. If no
   progressing prefix is admissible, report the blocking constraint instead
   of repeatedly posting empty predecessor batches.
5. Submit the prefix, await canonical commitment advancement, and continue the
   remaining drain at eligible opportunities. Each validated intermediate
   settlement updates the anchor; the in-memory target keeps the remaining
   range due during the running process. A process restart follows section 3.3.
6. Stop draining once its required endpoint is covered; do not create a stream
   of empty follow-up batches solely because L2 continues producing blocks.

Keep candidate sizing and conservative gas pricing free of submission side
effects. Perform those checks before KZG sidecar generation, witness collection,
proof generation, or signing for that candidate. Generate those artifacts only
for a selected fitting candidate, then run the final validation in section 6.3.
Distinguish `fits`, `over_blob_or_gas_budget`, and `unavailable_or_invalid` in
the planner: missing data, a proof error, or an unavailable network rule is not
permission to shrink the batch. Reuse existing encoding helpers for exact size;
this does not require a different encoding or a general optimization framework.

Historical anchor-only batches currently require an empty, scheduler-aligned
terminal block. The planner MUST preserve that condition unless a separately
validated protocol change removes it. It must not truncate arbitrary bytes or
assume every L2 height is a valid boundary.

A single unsplittable atomic group that exceeds the permitted blob or gas budget
cannot be repaired by batch splitting. Retain/report or reject it according to
the existing transaction-admission rules with a precise reason.

### 6.3 L1 execution-gas budget

L1 execution gas and blob capacity are independent constraints. In this fork,
`EEZ.postAndVerifyBatch` can execute outbound L1 calls through cross-chain
proxies. The target's work can increase substantially without increasing the
call input or blob references. Gas can therefore bind before blob capacity for
cross-chain workloads. Section 13 records the code inspection and local probe.
This is not a prediction that idle or pure-L2-only batches hit gas first.

Resolve a gas budget from the minimum of the active network per-transaction
limit, the validated target block gas budget, and any explicit operator posting
cap. Treat a network limit as absent only when the validated fork rules say it
is absent. Ethereum's [EIP-7825](https://eips.ethereum.org/EIPS/eip-7825) cap is
16,777,216 where active; do not apply it to Chiado without resolving Chiado's
own active rules. Blob gas is accounted for separately under
[EIP-4844](https://eips.ethereum.org/EIPS/eip-4844); adding blobs does not provide
extra execution gas.

Gas assessment MUST include transaction intrinsic gas, active calldata pricing
and floor rules, proof verification, entry processing, first-use proxy creation,
target execution, and required call-forwarding headroom. Conservative estimates
and configurable safety margins must be distinguishable from protocol limits
in diagnostics. Existing measured gas pins may remain versioned implementation
parameters with regression coverage; they are not universal consensus rules.

Use conservative preflight accounting before proving, then simulate the final
ordered L1 bundle against the applicable target state before dispatch. A simulation
or gas estimator reporting outer-call success is insufficient: verify that the
intended entries/root advances actually complete, because an immediate entry
can be skipped while `postBatch` returns successfully. Revalidate gas and blob
limits if the candidate, target state, or target fork changes.

Check every companion transaction's per-transaction gas rules and whether the
entire ordered bundle fits the block's gas accounting. Checking `postBatch`
alone misses gas consumed by inbound user transactions and their deferred calls.
Do not confuse gas used, declared gas limits, or gas remaining for later
transactions. A conservative bound may reject a candidate early; final ordered
simulation must model the actual target fork and required outcomes. Account for
all supplied blob sidecars, sender balances, fees, and nonce dependencies too.
Admission MAY use source simulation gas plus the existing protocol-work
projections instead of summing companion declarations. Such simulation bypasses
source proxy consumption and MUST be labelled as an estimate. Preserve declared
headroom for every ordered companion, including when postBatch's projected cost
grows. Final native bundle execution, with actual cumulative gas and complete
effects, remains the authority for dispatch.

For pure L2/empty consolidation the L1 bundle normally contains only `postBatch`.
For inbound cross-chain work it can be `[postBatch, inbound_tx_1, inbound_tx_2]`.
Each is a separate L1 transaction and all must fit together in the intended
block. Outbound target calls are different: they execute inside `postBatch` and
their gas belongs to that transaction's existing gas check. This requirement
does not create a new submission mechanism; the current composer already builds
these ordered bundles. An explicit user-count cap is an additional operator
ceiling, not a substitute for companion gas accounting. Final capacity checks
must account for actual admitted work rather than assume every user costs the
same amount of gas.

Use the existing local execution and submission mechanisms to validate each
supported network's ordered bundle. Extend their budget/outcome reporting where
needed; this does not require a new remote simulation API if the local path
provides the same checks. An isolated `eth_estimateGas(postBatch)` or a relay acceptance response does not
prove that the dependent bundle executes completely. For deferred inbound
effects, inspect the companion source transactions and attributed state changes
over the bundle's L1 block; their effects may be absent from the `postBatch`
receipt itself. Do not assume companions fit solely because the first
transaction passed its individual gas check.

Where possible, control gas use during cross-chain admission by admitting fewer
complete groups. Keep deferred groups pending and urgent. Do not cut apart an
already formed atomic Sync group to make it fit, or assume publishing less
historical DA will solve an oversized target call. If one atomic group alone
cannot fit, use explicit admission failure/defer behavior, not endless splitting.

### 6.3.1 Posting fees and cost on the active network

Reducing unnecessary posts and improving packing MUST be accompanied by realistic
fee selection and cost reporting for the L1 network receiving `postBatch`.
Ethereum and Chiado use their own chain state, active fork rules, relay needs,
and native currency units. No universal gwei price or per-post currency cost is
assumed.

1. **Read current fees from the selected L1.** Use a standard client/RPC fee
   estimator for priority fees and the active network's execution and blob base
   fees. Reuse the existing provider facilities. Resolve fee data against the
   submission target's canonical chain; refresh when the target or fee sample
   becomes stale. Missing fee data is an explicit retryable error, not a zero
   price or a fallback to another network's defaults.
2. **Make operational margins and caps explicit.** Validate execution/blob fee
   headroom, priority-fee bounds, maximum execution/blob fee prices, and gas-limit
   safety margin in the existing startup fee configuration. The current
   `EEZ_L1_POSTBATCH_PRIORITY_FEE` remains an optional explicit fixed override;
   an invalid override fails startup. An unset override uses network estimation.
   Profiles state any relay-required minimum. Replace implicit fixed multipliers
   and the smoke-test 10-gwei fallback with documented profile values. This is
   ordinary fee estimation, with no custom bidding or price-prediction service.
3. **Size transaction gas for the actual candidate.** Use target-state execution
   estimates and a validated safety margin to select the declared gas limit,
   within the resolved hard limits and operator cap. Check that intended effects
   complete at that limit. Do not use the maximum permitted transaction gas as
   the assumed consumption of every post. Existing gas pins are conservative
   planning inputs, validated for the relevant contract/proof version and fork;
   final execution and receipt measurements establish actual usage.
4. **Separate expected spend, fee ceilings, and balance reservation.** For the
   supported EIP-1559/blob transaction path, calculate in checked native-unit
   arithmetic:

   ```text
   estimated_post_cost = estimated_execution_gas * estimated_effective_gas_price
                         + blob_count * protocol_blob_gas_per_blob * estimated_blob_base_fee
   actual_post_cost    = receipt.gasUsed * receipt.effectiveGasPrice
                         + receipt.blobGasUsed * receipt.blobGasPrice
   ```

   Receipt fields or equivalent canonical transaction/block data supply the
   actual values. The fee-cap balance reservation is a separate upper bound,
   not the expected or paid cost. Unused execution gas is not charged as used
   gas. Blob charging counts complete physical blobs, including unused payload
   space; obtain the billing constant from the active protocol definition,
   separately from the codec's usable bytes. These distinctions follow
   [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559) and
   [EIP-4844 gas accounting](https://eips.ethereum.org/EIPS/eip-4844#gas-accounting).
5. **Measure the correct payer and outcome.** Report the poster's `postBatch`
   spend separately from companion transactions' spend and the full bundle
   total. Record execution and blob components, estimate versus actual, and
   costs per settled L2 block and included L2 transaction where the denominator
   is nonzero. Included reverted/partially applied attempts still have receipt
   costs; a never-included attempt has no on-chain transaction fee. Attribute
   costs using canonical receipts and correct the metrics after reorg.

For version 1, define the operational fee inputs precisely:

| Input | Unit and behavior |
| --- | --- |
| Fixed priority-fee override | Optional integer native base units per execution gas; use it when supplied and valid; use the standard network estimator only when it is absent |
| Priority-fee minimum/maximum | Integer native base units per execution gas; minimum includes any configured relay requirement; minimum must not exceed maximum |
| Network priority-fee margin | Nonnegative integer basis points, default zero; apply `ceil(estimate * (10_000 + margin_bps) / 10_000)` before minimum/maximum bounds. A fixed override remains exact and bypasses this estimator-only margin. Recompute from the current network estimate for each new attempt. |
| Execution/blob fee-price ceilings | Separate positive integers in native base units per respective gas unit; these are per-unit ceilings, not per-batch or monthly currency budgets |
| Execution/blob fee headroom and gas-limit safety margin | Separate nonnegative integer basis points added to the relevant base value; `x` becomes `ceil(x * (10_000 + margin_bps) / 10_000)` using checked arithmetic |

An explicit fixed tip outside the configured bounds fails startup. Automatic
estimates use those bounds; a final quote that cannot satisfy the active base
fees and configured minimum tip within the ceilings is `fee_cap_blocked`.
Do not clip a required gas limit below a validated complete-execution value.
Numeric profile values and environment-name wiring belong in the implementation
configuration table; they must be documented and tested before release.

Obtain a new fee quote for each new submission attempt. Bind it to the chain ID,
canonical fee-sample block hash/number, and intended target. Before dispatch,
refresh/revalidate if the canonical head, target, or candidate has changed;
invalidate a quote whose sample was reorged out. Read existing head information
for that check; no periodic fee-polling worker or new cache lifetime setting is
required. Ordinary not-due ticks do not request a fee quote.

Automatic repricing applies only to the poster-owned `postBatch`. Preserve the
signed bytes and fee caps of companion user transactions. An underpriced
companion is deferred through existing admission/retry handling; the composer
does not increase a user's authorized fees. An unresolved sent attempt remains
under existing nonce/attempt ownership and is not repriced in parallel.

Changing the final transaction's gas limit, fee fields, or target invalidates
its previous final simulation. Validate the complete ordered bundle again. If
observed execution or proof-bound inputs change, use the existing composition
and ECDSA proof path again. A lower gas limit that returns outer-call success
but skips intended effects is a failed preflight, not a cheaper successful post:
increase the limit within the permitted budget and recheck, or leave the work
pending with the appropriate error. Do not dispatch the incomplete candidate.

Fee selection does not add a settlement trigger or defer already-due work to
wait for a predicted cheaper period. If configured fee ceilings cannot support
a valid submission, keep the work due, report `fee_cap_blocked`, and retry under
the existing submission rules. Do not override spending caps or classify high
prices as exhaustion of blob/gas capacity. Refresh/re-sign only through the
existing attempt and nonce ownership path. Required bundle ordering remains
the responsibility of the existing atomic submission mechanism; production
must not depend on an arbitrary high tip copied from a smoke test.

### 6.4 Catch-up and manually managed prover limits

`MAX_BLOCKS_PER_BATCH` currently defaults to 300 and can trigger historical
splitting before blobs fill. That behavior MUST NOT remain an independent
posting/splitting rule under the confirmed policy. Decouple it from the
sequencer's catch-up production bound.

Remove `EEZ_MAX_BLOCKS_PER_BATCH` from the updated deployment profiles. Reject
that obsolete setting when starting with the new policy, with an actionable
migration message. Do not reinterpret it as the general posting threshold:
one was a maximum batch size and the other is a trigger. Keep catch-up production
and manually configured signer guards separate.

The signer separately defaults to 512 blocks per request and also limits request
bytes, witness items, and processing time. These are real resource guards.
Removing the 300-block split does not authorize disabling them. Operators will
size and coordinate prover limits manually for the chosen deployment. Automatic
prover-capacity discovery, tuning, and splitting on prover limits are out of
scope. Existing resource failures must remain visible and preserve due intent;
do not silently replace the user's policy or repeatedly submit a known invalid
request. A generic proof timeout does not prove a batch is permanently too large.

### 6.5 Soft posting target and actual overflow

The configured percentage is the point at which posting becomes worthwhile,
not an L2 block-size or transaction-admission limit. With ten allowed blobs and
a 10% target, crossing from approximately one blob's worth of data to 1.2 blobs
is acceptable: that is about 12% of the allowance and fits comfortably in one
two-blob transaction. Do not split it into two batches just to approach 10%.

The actual limit is the complete permitted blob allowance, together with the
gas constraints. Validate those with exact final encoding. If an accumulated
range exceeds them, split at the existing valid boundaries and retain the
remaining drain intent. The normal headroom between a low soft target and full
capacity makes an exact percentage cap unnecessary.

The original proposal excluded a sequencing byte budget. The user explicitly
extended that scope on 2026-09-14 after the Chiado interval-capacity incident.
When the policy is enabled, ordinary sequencing MUST prevent newly produced
intervals from exceeding the hard DA allowance:

- The composer resolves the receiving network's active transaction and block
  blob maxima, the existing operator cap, and known future fork reductions.
  Use the codec's usable bytes and framing; do not use the soft percentage,
  fee-market target, transaction count, or a network-specific constant.
- Before accepting an ordinary transaction into a block, account for all exact
  encoded blocks since the preceding usable boundary and reserve conservative
  space for every remaining empty block through the next Sync terminal. Use
  the scheduler target's residue relative to L2 genesis, not height modulo K
  equal to zero; preserve genesis offsets and whole-slot catch-up.
  Unknown execution-derived header values must have a safe upper bound. Check
  the final block's actual encoding before returning a built payload.
- Ordinary overflow stays in the existing mempool. Preserve nonce dependencies
  and continue considering other eligible senders. Transactions that cannot fit
  even in a fresh interval must receive an explicit admission error. Missing
  rules or unavailable ancestry must fail closed, without treating temporary
  lookup failures as permanently invalid transactions.
- Catch-up and reorg rebuilding MUST preserve every intermediate scheduled
  boundary as an empty Sync block, unless it is an already-owned composed Sync.
  Derivation of already settled blocks remains unchanged. Admission and payload
  building must consult the same composer allowance resolver.
- Fitting multi-blob history must not repeatedly miss the same preparation
  deadline. For anchor-only posting, reserve observed encoding, KZG, and witness
  preparation time in addition to the existing proof budget when selecting a
  fresh L1 target. Keep the measured high-water mark in memory per rollup; it
  may reset on restart. Do not shrink fitting batches because preparation is
  slow, relax proof/submission deadlines, or retarget composed cross-chain effects.
- These guards apply independently of the pure-L2 settlement mode or the
  cross-chain count setting. They do not make a batch due, change timestamps,
  compress data, alter wire formats, or replace final gas/blob validation.
  Policy-disabled nodes and followers keep their previous behavior.

Already accepted history can still be unpostable after manual configuration
changes or upgrades from an unguarded node. Report `unpostable_range` explicitly;
never truncate accepted data or submit an invalid batch. Repairing such history
or resetting the live network is outside this prevention change.

Changing an operational blob cap or activating a lower fork limit must account
for accepted history. Reject a local cap reduction that makes the retained range
unpostable, and revalidate the plan under target-fork rules. The soft percentage
must never be used as a substitute for the actual network allowance.

## 7. Ethereum and Chiado support

The policy engine operates in L2 heights and typed durations. It MUST NOT branch
on magic chain IDs to substitute policy rules or assume Ethereum's slot length.

A network adapter/profile supplies:

- expected chain identity and fork activation schedule;
- L1 slot timing and existing relay submission windows;
- active block and transaction blob limits;
- active per-transaction gas rules and observed block gas capacity;
- native currency metadata, fee estimator support, and explicit operational
  fee margins/caps and any required relay minimum;
- supported blob-sidecar format and transport behavior.

Use one shared representation of resolved chain/fork limits. Pass its outgoing
allowance through the composer and blob transport; the signer and follower must
validate against the corresponding chain/fork rules. The existing shared
`MAX_BLOBS_PER_TRANSACTION = 6` checks occur in all of these paths. Updating
only the composer would leave another component rejecting a permitted larger
batch. Replace those universal checks together and test a profile fixture with
an allowance above six end to end. This fixture does not assert a current
production limit. An outgoing operator cap applies only to posting.

Use validated client configuration and supported chain profiles. Where
available, [`eth_config`](https://eips.ethereum.org/EIPS/eip-7910) exposes current
and scheduled fork configuration, including the block-level blob schedule.
It is not by itself a complete source of transaction, timing, or local prover
limits. Do not make support for this RPC mandatory when a supported client
offers a validated configuration fallback.

Resolve outgoing rules for the **target** fork, revalidate before dispatch, and
refresh on fork transitions. Historical blob admission and replay MUST use the
rules at the containing historical L1 block, not today's posting policy.
An operator's outgoing blob cap MUST NOT reject otherwise valid historical DA.

Unknown or inconsistent network rules block submission with a clear diagnostic;
they do not select Ethereum defaults. Numeric protocol constants remain in
versioned protocol definitions. “Avoid hard-coded values” does not mean making
consensus constants arbitrary operator settings.

Current reference sources include the
[Ethereum mainnet configuration](https://raw.githubusercontent.com/ethereum/consensus-specs/master/configs/mainnet.yaml)
and [Chiado configuration](https://raw.githubusercontent.com/gnosischain/configs/main/chiado/config.yaml).
Their mutable URLs are discovery references; implementation tests must pin
reviewed fixtures and record their revisions. Do not embed today's blob counts
as permanent policy constants.

The existing [Chiado deployment profile](../../docker-compose.chiado-node.yml)
already sets 5-second L1 slots and 1-second L2 blocks. Its defaults reserve
1,500 ms for proving and 700 ms for submission slack, satisfying the current
scheduler's `K = 5` timing constraints. Preserve this profile and its genesis
grid; there is no new Chiado cadence decision or scheduler redesign in scope.
The earlier review incorrectly treated Ethereum Kurtosis defaults as a missing
Chiado configuration.

Reuse the existing Gnosis L1 adapter, native blob/sidecar configuration, atomic
relay wiring, and startup checks. Extend validation to the new settlement policy
on both supported profiles. Document the client/fork and relay capabilities used
by the tests; a 5-second Ethereum devnet is useful timing coverage but not the
same as exercising the existing Chiado stack. Multi-tx cross-chain bundles must
preserve the submitter's restriction on individually broadcast fallback.

## 8. Settlement completion, restart, and reorg

- Update the anchor only after canonical commitment advancement is validated.
  Failed, dropped, rejected, or no-state-progress submissions do not reset it.
- Every successful settlement, including an empty consolidation or a
  cross-chain settlement, updates the anchor used by the pure L2 timer.
- Clear pending activity only for the L2 range actually covered. Preserve later
  activity and, during the running process, any incompletely drained endpoint.
- A receipt may contain only partial execution. Advance only to the range the
  deriver validates as applied; do not clear the full submitted range or release
  unconsumed cross-chain groups merely because the transaction succeeded.
- On restart, discard in-memory drain intent, recover the canonical settlement
  anchor and pending activity, and reevaluate the current rules. A remaining
  range may wait again; its timer uses the latest settlement, never restart time.
- On L1 reorg, restore the latest still-canonical anchor, its original L1
  timestamp, and uncovered L2 activity. Invalidate stale prepared batches.
- On L2 reorg, discard activity records for removed hashes and rebuild them from
  the replacement canonical blocks. Preserve an overdue obligation only for
  work that still exists or is requeued by valid recovery.
- Preserve existing held-pool nonce reservations and rollback ownership. A
  policy decision or intermediate cursor must not incorrectly release a whole
  cross-chain group that has not settled.

### 8.1 Use the separately fixed transaction accounting

The user is fixing partial-settlement accounting in another worktree and has
instructed this spec to assume that fix is available. It is not an unresolved
blocker or a duplicate implementation task for this feature.

Consume that implementation's canonical settlement evidence, per-transaction
dispositions, actual endpoint hashes, nonce ownership, and recovery outcomes.
Attach the policy's remaining drain intent to those outcomes. Tests here verify
that policy integration respects partial application, same-height replacements,
lookup-only completion, and valid caught reverts. They do not reimplement the
accounting layer or introduce a separate relay partial-inclusion policy.

### 8.2 Reuse existing witness retention

The volatile policy intent in section 3.3 is separate from the chain, witness,
and transaction data used by the existing settlement and recovery paths.

Retain unsettled canonical block bodies, required witnesses or replayable
parent state, accepted raw transactions, and unresolved attempt data. Do not
prune them by the pure L2 interval, general block threshold, or signer block
count. Retain data needed for non-finalized settlement recovery; the existing
witness store's finalized-L2 purge floor is relevant and must be preserved.

Use the existing persistent witness store, canonical-hash validation, and
on-demand replay fallback. Verify that an accumulated batch can obtain its
older witnesses through those paths. If required inputs are unavailable,
report the existing data/proof error and preserve pending work; do not mark the
range settled or treat missing data as a blob-capacity failure.

This feature does not add a state-pinning service, storage quota policy, or
disk-failure backpressure mechanism. Existing witness-capture failures remain
separate storage work. The policy's acceptance criteria cover
retained-backlog settlement and unchanged retention/recovery behavior; they do
not claim to fix arbitrary pre-existing storage failures.

## 9. Efficiency requirements

- Maintain an incremental index of transaction-bearing canonical L2 blocks and
  invalidate it by hash on reorg. Avoid scanning the entire backlog every slot.
- Check cheap activity/age predicates and cached byte accounting before witness,
  proof, and KZG work. Fullness needs sizing even when the time/block predicates
  are false, but does not justify proving each idle tick. Cross-chain admission
  may require simulation; ordinary idle ticks should not. Fetch fee estimates
  only for work selected for submission; not-due ticks do not add fee RPC calls.
- Reuse immutable block RLP and measured encoded lengths while their hashes and
  encoding version remain valid. Price a candidate before collecting witnesses.
- Avoid repeatedly walking and encoding the full history for each smaller
  boundary. Reuse cached prefix accounting and validate the final exact encoding.
  Do not assume endpoint-specific payload size is monotonic without establishing
  that property for the selected encoding and candidate class.
- Reuse codec stream lengths or a cheap incremental estimate without changing
  encoded bytes. Account for RLP/semantic/framing overhead and validate actual
  encoding at final packing. Track estimation error; counting padded sidecar
  bytes is not an acceptable estimate of meaningful usage. Include limits,
  codec version, anchor, and terminal hashes in candidate cache keys.
- Bundle simultaneous trigger reasons into one settlement. Pack multi-blob data
  without a default one-batch-per-blob policy.
- Keep proof execution, memory, and network concurrency bounded and measured.
  Larger DA capacity does not make signer execution free.

## 10. Examples and acceptance cases

The durations below are illustrative arithmetic, not shared network constants.
Unless stated otherwise, assume no earlier captured drain or in-flight attempt.

| Situation | Expected result |
| --- | --- |
| Empty scheduled Sync; no pending activity; general and fullness thresholds not reached | No settlement |
| At least one admissible cross-chain transaction; all timers below threshold | Settlement due at the next eligible opportunity |
| `always`; an earlier L2 block in this slot has a pure transaction; terminal Sync empty | Settlement due |
| `always`; submission missed; next slot is empty | Previous due activity is retried, not forgotten |
| Idle general-rule batch stays in flight through several empty slots, then succeeds | Clear the covered intent; no follow-up batch unless the fresh general threshold or another rule is due |
| Pure interval batch succeeds after newer pure transactions arrive | Apply the new settlement timer to uncovered activity; do not inherit an expired old timer for already-completed work |
| `interval`; pure transactions pending; interval, general, and fullness thresholds not reached | No settlement yet |
| `interval`; pure transactions pending; interval elapsed from latest L1 settlement | Settlement due |
| `interval`; interval elapsed but no transactions and general threshold not reached | No settlement from the pure rule |
| All blocks empty; general block threshold reached | Consolidation due |
| Two-second L2 cadence; N=150; pure interval=15 minutes; fullness off; no cross-chain work | General rule becomes due around five minutes, before the pure interval |
| One-second L2 cadence; N=150; pure interval=15 minutes; fullness off; no cross-chain work | Same policy becomes due around two minutes thirty seconds |
| N is crossed between eligible Sync boundaries | Attempt at the next eligible boundary; report actual age |
| Nonzero speculative-depth limit is below `N + K - 1` | Reject startup with the required minimum; do not add an early-settlement trigger |
| Speculative depth is zero or satisfies the compatibility bound | Preserve the configured guard; the general trigger is reachable on the existing Sync grid |
| Obsolete `EEZ_MAX_BLOCKS_PER_BATCH` remains configured | Reject startup with migration guidance; do not retain a hidden 300-block split |
| Pure transaction arrives after the interval already elapsed; general threshold has not fired | Due at the next eligible opportunity; do not grant a fresh full interval |
| A cross-chain or empty consolidation succeeds before the pure timer expires | Timer anchor advances to that settlement |
| Due data needs three blobs; target network and transaction permit three | One batch with three blobs |
| Fixture permits more than six blobs and the fitting candidate needs them | Composer, transport, signer, and follower all accept the same permitted count |
| Same data needs three blobs; effective target allowance is two | Blob-driven split at valid boundaries; drain the remainder |
| Ten blobs allowed; threshold=10%; estimated usage=126,976 bytes | Fullness makes settlement due; nominally one full usable blob is 10% of the allowance |
| Ten blobs allowed; threshold=10%; estimate jumps from 9% to 12% | Settle the fitting candidate; exceeding the soft target is allowed |
| Six blobs allowed; threshold=10%; estimate=76,185 versus 76,186 bytes | Below target versus due in the integer predicate; actual inclusion need not hit this byte count |
| Fullness becomes due with data needing several blobs but fitting both budgets | One batch with all required blobs; do not split at the trigger percentage |
| Fullness disabled; other rules not due | Wait; fullness does not create an implicit posting rule |
| All blocks empty; fullness threshold reached before the general block threshold | Consolidation due from fullness |
| Allowance changes while work is pending in the same process | Recompute using target rules; preserve already due canonical intent |
| Small cross-chain batch fits the blobs but exceeds the L1 gas budget | Limit admission or split only at valid boundaries; an indivisible oversized group is deferred/rejected explicitly |
| Simulation or receipt succeeds while an intended entry is skipped | Do not classify the full range/group as settled |
| More than 300 blocks fit in the permitted blobs and validated resource budget | One batch; the old 300-block split does not apply |
| Capacity-driven first batch succeeds; later due data remains; process stays running | Continue the drain without waiting a new interval or fullness threshold |
| Failure or observer verdict without validated commitment advancement | Do not reset settlement age or clear activity |
| Restart with unsettled activity | Recover the canonical anchor and activity; evaluate current rules without restoring old policy intent |
| Restart without a newer settlement; interval already elapsed and pure activity remains | Settlement is still due; process startup does not reset the timer |
| Restart after an intermediate batch settles; none of the current rules is due | The old drain target is lost; wait until a normal rule becomes due |
| Reorg makes previously settled activity pending | Restore the canonical anchor and activity; reconcile in-memory intent and reevaluate |
| Ordinary traffic would exceed the hard allowance before its first valid split boundary | Defer transactions and preserve the empty boundary; reject individually unpostable transactions. Previously accepted incompatible history still reports an explicit error |
| `postBatch` fits its gas budget but required inbound transactions make the bundle too large | Reduce complete admitted groups; do not dispatch the oversized bundle |
| Same batch runs against two profiles with different fee markets or fork gas rules | Estimate and price it from the selected network; no shared currency-price assumption |
| Production priority-fee override is unset | Use the active network's fee estimator; do not fall back to the smoke-test 10-gwei tip |
| Estimated gas is well below the permitted transaction maximum | Declare sufficient estimated gas plus validated margin; report expected usage and balance reservation separately |
| Partially filled physical blob is posted | Charge/report its full protocol blob-gas cost; fullness percentage is not a billing discount |
| Fees move before an unsent candidate is dispatched | Refresh and revalidate within configured caps through existing attempt ownership |
| Fee quote's sample is reorged out, or the canonical head/target changes | Invalidate or refresh the quote and final validation before dispatch |
| A fixed priority-fee override is invalid or outside configured bounds | Fail startup; do not silently use the default or estimator |
| Companion user transaction is underpriced | Preserve its signed bytes and authorized fees; use existing admission/retry handling |
| Lower gas limit produces outer-call success but skips an intended entry | Fail preflight; increase gas within the allowed budget and recheck, or preserve pending work with an error |
| Fee or gas changes affect simulated execution or proof-bound inputs | Revalidate the ordered bundle and recompose/reprove as required through the existing path |
| Fee estimator is unavailable or configured ceilings are insufficient | Keep settlement due; report the fee error/block and retry without overriding caps |
| An included attempt reverts or applies only a prefix | Record its actual receipt cost independently of whether the entire intended range settled |
| Deferred inbound effects appear in a companion transaction, outside the `postBatch` receipt | Attribute and validate the complete bundle's effects before resolving ownership |
| Cursor reaches the proposed height but a different partial block hash is applied | Account for the applied prefix; recover the unconsumed tail without releasing it as completed |
| Restart after fullness became due; new allowance would make it below threshold | Use the new allowance; wait unless a current rule makes settlement due |
| Restart with a longer interval or fullness disabled | Apply current startup settings to the backlog; no durable policy revision is needed |
| Crash after bundle send and before response persistence | Use existing attempt/nonce reconciliation before permitting another submission |
| A due historical batch needs an ECDSA proof | Use the existing proof path, timing, retries, and validation |
| Every steady trigger is late but historical work is due | Historical settlement can target a fresh L1 opportunity; it is not hidden behind the late-slot return |
| Policy skips posting while the ordinary mempool is non-empty | Build the defined empty Sync; do not enter a mempool-driven fallback |
| The empty Sync build fails during a policy skip | Report the error and end the tick; no pool-driven Sync fallback |
| A valid cross-chain group needs earlier history, but other triggers are below threshold | Its finite predecessor inherits urgency and can settle first |
| A delayed batch needs older witnesses | Read retained canonical witnesses or use the existing replay fallback |
| Required witness data remains unavailable | Report the error and preserve pending work; do not shrink or classify the batch as settled |

## 11. Implementation plan

1. **Turn the resolved policy into fixtures.** Pin the trigger truth table,
   exact sizing examples, and reviewed network/fork rules before integration.
   Use the accounting fix from the other worktree and preserve the existing
   Chiado profile. Section 14 records the resolved review clarifications.
2. **Add typed policy and network-limit inputs.** Validate startup configuration;
   separate trigger thresholds, blob/gas allowance, catch-up limits, and prover
   guards. Check speculative-depth compatibility, reject the obsolete batch-size
   setting, and update deployment examples for both networks. Replace universal
   blob-limit checks across composer, transport, signer, and follower together.
   Extend existing fee configuration for network estimation, explicit overrides,
   margins, and caps; remove implicit production smoke-test pricing.
3. **Implement a pure decision engine, activity index, and byte accounting.**
   Inputs are a stable snapshot; outputs include all due reasons and the required
   endpoint. Cover the truth table, boundary arithmetic, and recovery invariants
   independently of RPC and proving. Keep policy intent in memory and test
   fresh evaluation from the canonical anchor and current settings after restart.
4. **Integrate at the composer boundary.** Preserve the configured L2 grid,
   validated recovery, cross-chain admission, and timestamp targeting. Replace
   unconditional empty-post paths with explicit due reasons and typed outcomes.
   Keep the existing ECDSA proof path, scheduling, timeouts, and retries.
   Ensure due historical work remains reachable in catch-up and late-slot modes.
   Define the error path for an empty Sync build without a mempool fallback.
5. **Make packing multi-blob-first.** Use exact existing encoding, active network
   blob/gas limits, valid historical boundaries, and in-memory drain intent.
   Remove the unrelated 300-block split trigger and document manual signer sizing.
   Validate complete L1 bundles; allow overshoot of the soft fullness target
   while enforcing the actual encoded blob/gas limits.
   Select gas and fees for the actual candidate and target network, and record
   estimated cost separately from fee-cap reservations and actual receipt cost.
6. **Integrate confirmation and reorg state.** Anchor changes must come from the
   deriver's canonical application path. Rebuild pending activity and reevaluate
   the current policy after restart using the existing recovery integration.
7. **Add observability and run the validation matrix.** Check real receipts,
   applied roots, native blob evidence, and follower replay on both profiles.
8. **Roll out explicit profiles.** Begin with local networks and staging;
   production values and activation are reviewed separately. Reverting policy
   configuration must not require a data-format migration.

Likely code touchpoints:

| Area | Current location |
| --- | --- |
| Decision engine and activity tracking | New module in `crates/eez-composer/src/`, integrated with `composer.rs` |
| Unconditional minimal posts and historical splitting | `crates/eez-composer/src/composer.rs` |
| Timing and no-emission L2 block production | `crates/eez-driver/src/timing.rs`, `slot.rs`, `sequencer.rs` |
| Policy wiring and network fee configuration | `crates/eez-node/src/lib.rs`, `crates/eez-composer/src/composer.rs` |
| Submission targeting and outcomes | `crates/eez-l1/src/config.rs`, `submitter.rs` |
| Outgoing and historical blob limits | `crates/eez-blob-codec/src/transport.rs`, `crates/eez-l1/src/blob_source.rs`, `crates/eez-proof-signer/src/settlement/blob_da.rs` |
| Exact unpadded stream sizing | `crates/eez-blob-codec/src/lib.rs`, existing composer payload encoding |
| Manual signer sizing documentation | `crates/eez-proof-signer/src/config.rs`, deployment profiles |
| Confirmed cursor, rollback, pending ownership | `crates/eez-composer/src/optimistic.rs`, `crates/eez-deriver/src/deriver.rs` |
| Volatile drain intent and existing recovery integration | New policy module and the accounting/recovery interface supplied by the other worktree |
| Existing witness retention integration checks | `crates/eez-node/src/witness_source.rs` and the integration harness |
| Integration profiles and evidence | `testing/kurtosis/`, existing Rust/Python integration harnesses |

## 12. Validation and operational evidence

Unit/property checks MUST cover the section 10 cases, threshold equality,
checked arithmetic, simultaneous reasons, activity outside the terminal block,
invalid configuration, and unchanged decisions for equivalent inputs across
different network timing profiles.
Include the speculative-depth compatibility bound with non-aligned anchors,
and obsolete-setting migration. These checks preserve the normal policy rules.

Fullness-predicate checks MUST include estimated equality and one byte below/above, thresholds
of 1 and 10,000 bps, disabled mode, rounding with six blobs at 10%, the user's
ten-blob example, real zero-valued data versus padding, metadata-size changes,
multiple blobs, overshoot beyond total capacity, and allowance changes in both
directions. Compare estimated and actual stream lengths across representative
ranges and invalidate stale estimates on reorg. Do not require the actual posted
batch to hit the target exactly. Include safe estimate overrun and underrun,
9%-to-12% crossing of a 10% target, and exact final rejection/splitting when
actual encoded data exceeds the hard allowance.

Integration checks MUST establish:

- unchanged reconstruction, configured timestamp grid, and ordinary transaction
  admission when only L1 emission is suppressed;
- no posting-related witness/proof/fee-estimation/submission work for an ordinary
  not-due idle opportunity; ordinary block witness capture remains unchanged;
- successful pure L2 `always`, pure L2 `interval`, idle consolidation, and
  cross-chain paths;
- successful native multi-blob posting and independent reconstruction of the
  entire settled L2 range;
- splitting only for an authorized reason, valid atomic boundaries, and complete
  draining across multiple successful submissions during a running process;
- separate cases where blob capacity binds first and where L1 gas binds first;
  low-byte expensive target calls, new proxies, indivisible oversized groups,
  and successful outer calls with incomplete intended execution;
- network-specific fee estimation and explicit overrides, changing execution
  and blob base fees, fee caps and estimator failures, gas-limit margin validity,
  quote invalidation, preserved companion signatures, complete execution after
  gas/fee changes, and estimated versus receipt-measured costs for both networks;
- dropped bundles, skipped slots, temporary RPC/prover failures, underfunding,
  and L1/L2 reorg with correct pending activity, in-memory intent, and anchor;
- restart with discarded policy intent, current-rule reevaluation, and unchanged
  transaction/attempt recovery; retain the actual settlement timestamp;
- changing active fork limits and validating historical data under its own rules;
- maximum-size transaction data and visible, manually recoverable prover
  resource failures without automatic prover-driven splitting;
- both Ethereum-like and Chiado-like local network profiles, followed by Chiado
  staging validation before production activation.

The second-review cases in section 10 are mandatory acceptance cases too.
Include restart before any settlement and after an intermediate split settles,
changed startup configuration, and an unresolved submission handled by the
existing recovery path. Check same-height reorg/resume with different hashes,
lookup-only inbound completion, stale candidates, companion-transaction gas
and blobs, and per-account nonce ownership. Exercise the existing ECDSA proof
path and the late-slot path with due history below the old 300-block threshold.
No policy-journal crash matrix or artificial long-running-proof test is required.

Test actual hard-cap overflow, a range with no admissible fitting prefix, and a
cap reduction with an existing backlog. Verify explicit errors and preserved
data/ownership. Do not turn the soft target into a rejection threshold for L2
transactions or claim it eliminates every pre-existing oversized-unit case.

Bind the network matrix to reviewed timing values, client versions, and RPC/relay
capabilities. A 5-second Ethereum test chain alone is insufficient Chiado evidence.
Include an accumulated range whose older witnesses come from the existing
persistent store, canonical-hash checks after reorg, and an unavailable-input
case that preserves pending work. No storage/backpressure redesign is required.

Kurtosis checks must inspect actual `postBatch` receipts and canonical applied
roots, not merely composer logs. Capture blobs, count transactions/batches,
verify KZG/semantic evidence, and independently replay the covered blocks.
Short test intervals are explicit profile values, not production constants.

For cost validation, replay equivalent idle and transaction-bearing workloads
through the baseline and new policy with the same controlled network fee
fixtures and measure the same covered L2 range. Compare poster spend, bundle
spend, batch count, execution gas, and physical blob gas. Establish savings from
fewer unnecessary posts, consolidation, and removal of excessive implicit tips;
do not promise a fixed savings percentage across live fee markets or claim that
lowering a declared gas limit alone lowers gas actually consumed. Confirm the
accounting formulas against canonical receipts on both supported profiles.

Metrics/events SHOULD expose: effective policy and limiting network rule,
pending range, pending transaction count, last settlement time, due reasons,
overdue duration/blocks, in-flight state, exact unpadded bytes, actual blob count,
effective allowed blob count, fullness threshold and denominator, estimated and
observed execution gas, gas budget and its source, drain progress, split reason,
failed admission/resource limit, execution/blob fee samples and caps, fee source,
estimated post cost, fee-cap balance reservation, actual execution/blob cost,
poster versus companion/bundle spend, and proof/encoding latency. Drain progress
is in memory and may reset on restart. Distinguish posted
blob utilization from backlog fullness against the total allowance; an oversized
backlog can exceed 100%. Emit structured changes rather than repeated idle warnings.

## 13. Resolved decisions and supporting evidence

The user confirmed that the more restrictive actual blob/gas constraint wins,
that prover limits will be managed manually, and that fullness is measured
against all blobs permitted in one batch. These trigger and packing decisions
remain resolved. Section 14 records subsequent scope clarifications. Production
profile values remain deployment choices.

### 13.1 Why gas cannot be inferred from blob occupancy

At the recorded baseline, `EEZ.sol::_processNCalls` executes outbound targets
through source proxies. The composer separately accounts for probed target gas
in `projected_postbatch_gas` and checks the encoded batch gas budget separately
from its blob-count check. The codec packs DA into blobs; it does not turn unused
blob capacity into an execution-gas allowance.

A local Foundry probe on 2026-09-12 uses the actual EEZ contract and ECDSA proof
system with a fixed `postBatch` input and one fixed mocked blob hash. Only the
target's pre-existing L1 state changes, selecting one versus 800 fresh storage
writes for the same `ping()` call. The test passed with these results:

| Measurement | Small target | Expensive target |
| --- | --- | --- |
| Identical `postBatch` calldata | 2,500 bytes | 2,500 bytes |
| Identical mocked blob references | 1 | 1 |
| Measured caller-frame execution gas | 154,857 | 17,857,501 |

The expensive case exceeds 16,777,216 before adding transaction intrinsic and
calldata charges. A second call with only 16,777,216 gas forwarded returned
success but did not reach the intended final root: the expensive immediate
entry was skipped. This supports the requirement to inspect applied state,
not just outer-call success, during simulation and confirmation.

The source, command, tool version, measurement, and reproduction helper are
recorded with the local evidence in
`artifacts/settlement-policy-2026-09-12/`. This is a contract-level counterexample,
not a native-DA/proof-signer or network inclusion test: the blob hash is supplied
by a Foundry cheatcode, and no valid KZG sidecar is asserted by this experiment.
ABI calldata length must not be reported as native blob payload length.

Before release, the integration checks in section 12 must exercise the complete
native encoding, admission, signing, submission, and applied-root verification
paths under both network profiles. This document and local probe do not by
themselves certify an implementation as production-ready.

Transient timeouts, low fees, builder non-selection, and the old 300-block cap
must not be mislabeled as exhaustion of protocol blob capacity or L1 gas.

## 14. Review clarifications and scope

- **Partial accounting:** assume the other worktree's fix is available. Integrate
  with it; do not list reimplementing the fix as work for this feature.
- **Fullness:** a flexible estimated trigger across the total allowed blob
  capacity. Crossing 10% does not stop transaction inclusion or cap a batch at
  10%. Exact final encoding still enforces the real allowance.
- **Chiado:** the compatible 5-second L1 / 1-second L2 profile already exists.
  Preserve its timing, Gnosis adapter, blob transport, and relay configuration.
  The earlier open cadence question was based on an incomplete configuration
  review and is withdrawn.
- **Bundle gas:** inbound L1 source transactions accompany `postBatch`; their
  combined block gas matters. Pure-L2 consolidation usually has no companions.
- **Posting cost:** fee estimation, transaction gas sizing, and actual cost
  reporting must reflect the receiving network. Remove implicit smoke-test
  pricing from production, keep margins/caps configurable, and validate savings
  with comparable workloads. The settlement triggers remain unchanged.
- **Restart:** keep drain intent in memory and allow it to be lost on restart.
  Reevaluate the remaining canonical range from the latest L1 settlement using
  current settings; waiting for normal thresholds again is accepted. Reuse
  existing transaction and submission recovery without new policy persistence.
- **Historical proofs:** keep the existing fast ECDSA proof path, scheduling,
  timing, retries, and validation. The proposed background worker, separate
  historical-proof timeout, and artificial slow-proof requirements are removed.
- **Storage:** reuse and test existing witness retention and replay. New storage
  quotas, state pinning, and disk-failure backpressure are outside this feature.

No further product decision is required to start implementation. Production
profiles still need explicit choices for the four policy settings and existing
operational limits; illustrative values in this document are not automatic
production defaults. Implementation is complete only after the acceptance
matrix passes on the supported Ethereum and Chiado paths.

The first implementation change must publish the complete configuration table
(names, units, validation, defaults or required profile values, and migration)
and deterministic local profiles. Production spending ceilings remain explicit
deployment choices. Routine API and estimator-library choices can be resolved
in implementation review; they do not require a new policy design or service.

The [implementation coverage](configurable-settlement-policy-implementation.md)
records the resolved interpretations, requirement mapping, and revision-specific
validation. Production profile selection and activation remain explicit
deployment steps; completed public Chiado runs have separate audit evidence.
