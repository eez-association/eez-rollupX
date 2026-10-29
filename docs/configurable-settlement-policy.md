# Configurable settlement policy

For the fresh-network history profile, ordinary production has its own local clock.
L1 RPC, proof and settlement delays cannot gate that clock. The speculative-depth
and four same-parent replacement limits apply to settlement preparation only;
retained ordinary prefixes settle independently in bounded parts. History-only
Sync blocks are ordinary checkpoints despite having a nonempty transaction root.
See the [current execution proposal](proposals/l1-origin-and-offline-proving.md).


This policy runs within the [fresh offline-history profile](proposals/l1-origin-and-offline-proving.md).
Current implementation and validation are recorded in the
[feature audit](audits/2026-10-22-l1-history-offline-proving/README.md).
Earlier Ethereum/Gnosis, Kurtosis and
[Chiado deployment results](audits/2026-09-13-chiado-settlement-policy/README.md)
establish the preceding policy implementation, not deployment of this new profile.
The [policy proposal](proposals/configurable-settlement-policy.md) defines the requirements; the
[coverage ledger](proposals/configurable-settlement-policy-implementation.md)
records implementation, tests, evidence, and deployment limitations.

The policy changes when a batch is posted. L2 execution, the Sync grid, ECDSA
validation, canonical derivation, and transaction ownership keep their existing
roles. Only validated L1 commitment advancement resets the settlement clock.

Under this policy, a validated cross-chain group is due at the next eligible L1
opportunity. An empty Sync is not cross-chain activity. Included ordinary L2
transactions, including reverted transactions, use the selected pure-L2 rule.
The general block threshold consolidates empty history too. All due rules are
combined into one candidate. A pending drain has a finite endpoint; empty ticks
do not extend it indefinitely.

## Configuration

Settings are read once before node launch. Existing environment/config-file
precedence is unchanged. Invalid values fail startup. An omitted value differs
from an empty string. Restart is required to change settings.

| Setting | Default | Meaning / validation |
| --- | --- | --- |
| `EEZ_SETTLEMENT_PURE_L2_MODE` | Required | Exactly `always` or `interval`. |
| `EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS` | Required for `interval`; absent for `always` | Positive integer milliseconds. A value supplied with `always` is validated but inactive. Measured from the containing L1 timestamp of the latest canonical commitment. |
| `EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS` | Required | Positive integer `N`, a posting trigger rather than a packing limit. |
| `EEZ_SETTLEMENT_BLOB_FULLNESS_BPS` | Off | `off`, or an integer from 1 through 10000. Soft threshold over meaningful unpadded bytes across **all** permitted blobs. |
| `EEZ_MAX_BLOBS_PER_BATCH` | No operator cap | Optional positive integer, additionally bounded by active network transaction/block and transport limits. Applies only to outgoing posts. |
| `EEZ_MAX_POSTBATCH_GAS` | No operator cap | Optional positive integer; actual budget also respects the target fork's transaction rule and validated block budget. |
| `EEZ_MAX_USER_TXS_PER_BUNDLE` | 50; Chiado env preset `auto` | Positive integer for the existing count limit, or `auto` for adaptive resource/time admission. Compose retains a numeric interpolation fallback of 12, but startup still requires the mandatory policy settings. Parsed once; restart to change. |
| `EEZ_PREPARATION_TIME_MS` | Removed | Must be absent, including from inherited environment and Compose inputs. Even an empty value fails startup. Choose P explicitly; no automatic migration of the old millisecond budget. |
| `EEZ_SETTLEMENT_START_AFTER_BLOCK` | Required | Conditional production after position P, `1 <= P < K`. Proof seed + slack + delay + the shared 150 ms post-proof reserve must be strictly less than `(K-P) * L2`. See [conditional slot production](#conditional-slot-production). |
| `EEZ_SETTLEMENT_START_DELAY_MS` | 0 | Optional nonnegative integer milliseconds, strictly less than one L2 interval. Delays the preparation decision from P's scheduled time, not its actual completion time. Empty is invalid. |
| `EEZ_MAX_SPECULATIVE_DEPTH` | Default 64; Compose profiles default to 0 | Caps speculative cross-chain preparation; ordinary production and retained ordinary settlement continue. Existing validation requires 0 or at least `N + K - 1`. |
| `EEZ_MAX_BLOCKS_PER_BATCH` | Removed | Must be absent. The payload ceiling and negotiated signer limit bound a post. |
| `EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS` | Required | Positive integer execution-price ceiling in native base units per gas. |
| `EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS` | Required | Positive integer blob-price ceiling in native base units per blob gas. |
| `EEZ_L1_POSTBATCH_PRIORITY_FEE` | Fee-history estimator | Optional nonnegative exact override within configured bounds. When no separate priority maximum is supplied, this fixed value is also its safe maximum. |
| `EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE` | 90 | Integer from 1 to 99 used by the fee-history estimator: each recent block's gas-weighted reward at this percentile, then this percentile of those samples. 90 bids above most recent transactions; 50 bids near the typical recent tip. Ignored with a fixed override. |
| `EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE` | 0 | Nonnegative integer; include any actual relay minimum. |
| `EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE` | Required with fee-history estimation; fixed override otherwise | Independent nonnegative priority-price ceiling. Must be at least the minimum and no greater than the execution-price ceiling. It never inherits the execution-price ceiling. |
| `EEZ_L1_POSTBATCH_CROSS_CHAIN_PRIORITY_PERCENTILE` | Unset: base percentile | Optional integer from 1 to 99 for bundles that execute cross-chain calls on L1 ([cross-chain priority](#cross-chain-priority)). Setting only the cross-chain maximum keeps the base percentile. Rejected with a fixed override. |
| `EEZ_L1_POSTBATCH_CROSS_CHAIN_MAX_PRIORITY_FEE` | Unset: base maximum | Optional priority ceiling for those bundles. Must be at least the shared minimum and no greater than the execution-price ceiling. Setting only the cross-chain percentile keeps the base maximum. Rejected with a fixed override. |
| `EEZ_L1_POSTBATCH_TARGET` | Unset: `EEZ_REGISTRY_ADDRESS` | Optional nonzero address used only as the postBatch transaction's destination ([postBatch target](#postbatch-target)). Boot waits until it has code and its `eez()` returns the registry; a different registry is a configuration error. |
| `EEZ_L1_POSTBATCH_EXECUTION_HEADROOM_BPS` | 10000 | Nonnegative integer margin added to sampled execution base fee before adding priority fee; bounded by the ceiling. |
| `EEZ_L1_POSTBATCH_BLOB_HEADROOM_BPS` | 10000 | Nonnegative integer margin added to the active network's blob base fee; bounded by the ceiling. |
| `EEZ_L1_POSTBATCH_GAS_MARGIN_BPS` | 500 | Nonnegative integer margin on pre-refund execution gas (at least the calldata floor). The final ordered bundle is checked again at the selected gas limit. |
| `EEZ_L1_CHAIN_CONFIG` (signer and follower) | Required for both | Supported public-network preset or a reviewed complete Ethereum-format genesis path. The registered verification key binds chain identity and configured L1 rules. Actual-parent proofs carry an exact posting context; historical proofs carry an explicit anchor and no posting context. Followers also check the actual inclusion block's rules. |

Execution, blob, and gas margins use checked
`ceil(x * (10000 + margin) / 10000)` arithmetic. Fee ceilings are per-unit
prices, not currency budgets or maximum transaction costs. A
required price above its ceiling keeps work pending as `fee_cap_blocked`.

The gas-margin default is 500 basis points (5%). Explicit values, including
zero, override it; empty or malformed values are
errors. Compose forwards the operator value to the node and leaves an absent
setting to the runtime. Shipped Kurtosis profiles explicitly use 500, and its
Starlark transport preserves supplied strings for runtime validation. Shell
environment takes precedence over `.env`. Signers
do not consume this operator margin. Changing the margin affects future gas
planning/signing; it does not change the network's fixed execution profile.

Actual-parent requests require the exact L1 posting context. Historical-mode
ordinary catch-up requires that field to be absent and binds its explicit
header anchor instead. Historical ordinary catch-up may cross a known compatible
L1 fork; actual-parent mode remains mandatory for cross-chain actions. The attempt's operational parent and submission cutoff
remain distinct from that proof anchor; retry and replacement accounting are
unchanged. See the [offline profile](proposals/l1-origin-and-offline-proving.md)
for its context commitment and fresh-genesis boundary. Native batch/DA encoding
and ECDSA signature encoding remain unchanged.

Signed execution fee caps are checked against the pinned L1 target and L2 draft
base fees before allocating the finite selection window. An underpriced nonce
and its successors stay queued; earlier nonces and independent source chains
remain eligible. Fees are reevaluated each opportunity. Balance and nonce checks,
full simulation, and final bundle validation still apply.

With `EEZ_MAX_USER_TXS_PER_BUNDLE=auto`, the composer considers a finite FIFO
queue snapshot from the first candidate. There is no mandatory one-source
calibration post. Group-start and pre-append checks reserve the remaining
build/encoding/KZG/witness work, proving and finalization. Whole source groups,
nonce dependencies, gas, blob and L2 bounds still apply. The cold finishing seed
is the larger of elapsed admission work and 100 ms, plus 250 ms per physical
blob; these are planning estimates, not consensus limits. See the
[validation record](audits/2026-09-26-admission-throughput/README.md) for measured
profiles and remaining limits.

When native tariff validation defers a source and its speculative effects have
been removed, its measured simulation/rollback interval is spent deadline time,
not work that the final accepted batch must repeat. Automatic admission excludes
only that bracketed interval from predicted finishing and next-group costs.
The absolute deadline still charges all elapsed work. Unknown or mismatched
brackets keep the conservative estimate, and source-deferred candidates cannot
train the calibrated finishing ratio. A rebuilt candidate starts fresh timing
accounting. This is an in-memory estimate correction: no configuration, queue,
protocol or follower migration is required. Older binaries retain conservative
overestimation on rollback; normal in-memory queue restart limits still apply.

Canonical effect-prefix hashing retains one candidate-local execution
checkpoint. Reuse requires the same raw transaction prefix, parent and block
environment. A changed or shorter prefix discards it; any construction error
invalidates it. The checkpoint contains pre-finalization state, receipts and
gas accounting. Each prefix is finalized separately through Reth, preventing
post-block changes from reaching the next transaction's pre-state. Canonical
append, the final full block rebuild, witness replay and signer validation are
independent. This changes local preparation work only: no protocol, ABI,
configuration, persistent storage or follower replay migration is required.

Blob admission uses a bounded encoded-size estimate before KZG work. The final
assembled batch still requires its real blob bytes, commitments and propagation
proofs. `prefix_hash_ms`, `prefix_hash_executed_tx_count`, `blob_preparation_ms`
and `witness_preparation_ms` expose preparation subintervals in retained admission
events. Existing collectors accept these optional details; historical values
remain absent. They are included in total preparation time, not added to it.

If the uncalibrated proof allocation would reject even the first group forever,
one complete group may probe the available window. This retains the finishing
and finalization reserves plus a 25 ms minimum proof opportunity, and proving
still uses the original shared deadline. The summary records `cold_timing_probe`.
This fallback applies only when the normal estimate refuses that first group;
it does not cap cold batches that already fit the complete estimate.

For all exact-target candidates, the exclusive deadline is target time
minus submission slack. P plus delay controls the preparation trigger and
`EEZ_PROOF_TIME_MS` seeds the proof estimate; neither creates a separate hard
stage limit. A planning snapshot invalidated by ordinary progress or a refreshed
canonical scan is recalculated within the same target window, with the attempt
count bounded by the number of positions in the Sync interval. Each attempt
rereads the parent and canonical generation; a changed L1 target stops that
opportunity. Retries retain the original monotonic cutoff and run in the single
preparation worker, leaving the ordinary clock independent. Stale Live/Future
work replans before dispatch, including when context changes between two
preparatory commits. A stale prebuilt Sync does not restart submission: the
original attempt retains ownership. Preparation can run
beyond its allocation and continue proving the same accepted candidate.
Proving uses the remaining deadline less finalization/delivery reserve; retries,
streaming and cache repair cannot reset that window. A fast preparation can
leave more time for a slow proof. A true window failure keeps source ownership
and produces an empty local Sync when required, without an unproved or substitute
empty L1 posting. Late scheduler entry uses this same cross-chain policy.
Ordinary-only posting uses the same complete deadline. Its target is the
observed canonical head's immediate child: the proof binds that parent's hash,
so timing estimates cannot reserve an unknown future parent. Missing slots may
advance only the timestamp within the shared replacement bound. A preparation
timeout retains history and retries against fresh canonical evidence; it never
changes a proof's target or treats a failed estimate as a capacity limit.

Preparation learns consistently measured group work and finishing/KZG cost.
Only complete measured groups train the finishing relationship; failed appends,
rebuilds and source-state waits retain conservative estimates. The calibrated
finishing reserve includes 10% of the available window for variation. A rejected
next group does not invalidate the accepted prefix's clean sample. Proof and
finalization retain at most 32 observations per rollup and stage, with 25%
headroom. Proof samples covering the requested group, physical blob and witness
block counts are used directly. Otherwise, observed durations scale upward by
the largest ratio across those three dimensions. From `strict-fifo-8`, a sample's
scaling term is superseded by an observation that supplies at least as much of
every requested dimension, strictly more of one, even if that wider proof was
faster. A slower sparse proof remains a duration floor rather than a per-group
cost once wider work has been measured.
Observed counts are clipped to the request only for this comparison: excess
history in an old sparse sample cannot prevent learning from a wider measured
batch with sufficient history. Incomparable observations, including greater
required witness/blob coverage, retain their scaling terms. This avoids
multiplying fixed overhead from a one-group observation after larger proofs
have been measured. Every observation still contributes to the slowest measured
proof floor during extrapolation. Scaling uses a denominator of one for zero observed
counts. The prediction is capped at the greater of the configured proof seed and
the slowest observed proof with headroom, with a 25 ms floor. A larger request never scales
below an already observed slower proof. With no proof
observations the configured seed applies. Counts do not bound arbitrary gas,
calldata, proof shape or transport delay. This is a scheduling estimate, not a
proof completion guarantee; the shared deadline remains authoritative. Each proof
request records the estimate and its `configured_seed`, `covering_sample` or
`scaled_sample` basis. Finalization only uses covering observations and reserves
at least 150 ms, including 25 ms for delivery. These samples are in
memory and are discarded on restart; they are not durable queue state.

When an admission estimate refuses a group,
`eez.composer.admission.pipeline_refused` records the existing candidate ID and
the exact remaining, next-group, finishing, proof and post-proof durations in
nanoseconds. `before_group` includes the next group's estimated execution;
`finish_group` has already spent that execution time and records zero for it.
These observations explain the comparison without changing its reserves.

A nominal preparation overrun or infrastructure error does not halve selection.
A full proving-window failure can reduce the next selection using admitted work,
while a resource cut does not impose a FIFO count backoff. Admission is strict
FIFO across batches among eligible requests, in both directions. A gas/blob/L2
capacity refusal stops younger work; a cheaper independent source cannot bypass
it. Within one batch, execution retains the signer's canonical outbound-first
order. If that execution order leaves an older eligible inbound out after
accepting younger outbounds, discard the speculative candidate and rebuild the
FIFO prefix including the first omitted source before proving. Start its width
at the number of complete groups just simulated, rather than always stopping at
the first omitted source; differing source costs may require further cuts. All
effects are recomputed against the same parent and target, under the original
shared deadline. Each rebuild strictly shortens the snapshot; a concurrently enlarged
prefix waits for a fresh opportunity. Fitting mixed snapshots still use one batch.

When rebuilding cannot finish in time, the held pool remembers the retry prefix's
first and last sender/direction/nonce identities. The last identity bounds the
next snapshot. Repeated attempts
cannot submit younger sources without the older eligible requests. Replacements inherit
their positions. Canonical resolution of the first source clears the boundary,
so an unconsumed suffix can share the next batch with younger work, subject to
the same FIFO validation. A receipt, relay acceptance, or partial consumption
that leaves the first source unresolved does not establish this progress.
Removal or consumption of the last source, or a classified source condition
blocking that source's nonce chain, also clears the boundary. This is an
in-memory scheduling boundary, not a learned transaction count or a settlement
failure.

Discarded preparation remains elapsed time against the same deadline. A rebuild
resets the candidate's blob/group observations and predicts future work using
only the replacement's elapsed preparation. Its finishing sample likewise excludes
discarded work. Do not divide the old pass's elapsed time by the replacement's
smaller source count or reserve KZG for blobs discarded with that pass. Rebuild
samples retain the conservative uncalibrated finishing estimate; final execution
and deadline checks remain authoritative.

Queue snapshots no longer promote a younger chain
merely because it was not visited. Nonce, fee, balance and escrow waits remain
eligibility exceptions; independent eligible sources can proceed. Discarded
simulations retain ownership without charging attempts. If a stale
estimate blocks every group, the next opportunity makes a bounded probe under
all resource/deadline checks. Canonically observed, charged exclusions or partial
consumption in produced blocks increment an in-memory failure streak per rollup.
Automatic admission preserves selection on failures one through four; the fifth
and later consecutive qualifying failures halve the next submitted-group allowance
(minimum one). Complete canonical settlement of a nonempty source batch resets
this streak and any submission limit. Preparation success and relay acknowledgement
do not reset it. Skipped targets and uncertain observations do not change it.
Real pipeline deadline limits still apply immediately and are not cleared by
canonical success. A strict timestamp gap
between consecutive canonical headers proves the old target was never produced;
that target cannot train capacity even if some of its sources completed in a
replacement. Source dispositions and replacement ownership still reconcile
independently. `admission.skipped_slot` records this distinction.

A completed native execution/effect rejection before dispatch has a separate
response: mark the never-dispatched attempt `preflight_rejected`, reconcile
canonical source ownership, and halve its whole-group selection allowance
(minimum one). This `native_preflight_backoff` also bounds numeric admission.
Multi-group reductions do not charge source attempts. Retain the learned bound
across preparation and successful posts until the queue drains; preparation
success does not prove the real shared-table execution fits. An indivisible
L1 source whose exact signed hash is identified by a failed native companion
execution uses the existing cumulative three-attempt execution bound, with
dependent nonce disposition unchanged. An unattributed root/effect mismatch or
failed poster cannot charge a singleton's retry counter.
Transport, proof deadlines, missing balances and uncertain chain observations
do not authorize this reduction or penalty. Final native validation always runs.

This is in-memory scheduling behavior within the fixed offline profile.
Drain accepted work before a same-profile runtime replacement; restart does not
preserve its ownership or learned limits. Changing scheduling settings does not
migrate contracts, genesis or follower replay rules.

Automatic submitted-attempt reconciliation permits six cumulative charged
failures per source, allowing a smaller retry after the fifth failure. Numeric
admission retains its three-failure limit. These count landed postBatches that
left the source incomplete, which spend L1 gas. Excluded target slots, whose block
was built without the bundle and spent no gas, count separately up to twelve per
source in both modes (`MAX_EXCLUDED_SLOT_ATTEMPTS`); either bound exhausts the
source and requires resubmission. The automatic admission streak and its backoff
still record both kinds of failure. Prover/immediate relay rejection also
retains its three-failure bound on the same cumulative source episode counter;
switching failure paths never resets a source's counter. Only eligible unspent
sources retry; spent nonces, partial consumption and dependent nonce eviction
retain their existing dispositions. This is separate from the per-rollup
consecutive admission streak. Both counters are in memory and lost on restart.

The legacy deferred-entry gas model can discount six fixed storage words that
are already nonzero in the exact canonical parent. It discounts 15,000 gas per
observed word; dynamic result storage, unknown shapes and speculative writes earn
no such discount. The existing shape and 128-entry limits remain. Final full-ABI
projection and exact signed-bundle native execution still validate gas and every
intended effect. This is not a receipt-ratio discount.

Estimator `companion-input-8` retains the `companion-storage-6` posting model
(storage inventory introduced in `companion-storage-4`), which inventories
bounded retained callback and result storage.
For supported deferred entries it reserves 35,000 per entry, 8,000 per
callback/result row, 3,000 per zero-to-zero storage word, and 6,000 per other
written word. A nonzero new word whose canonical-parent value is zero or unknown
adds 17,000, retaining the previous 23,000 fresh-write allowance. Old words
outside the new layout add 3,000 if zero or 6,000 if nonzero for truncation;
refunds are never credited. Version 5 charged 8,000 for every word plus 15,000
for fresh nonzero words.
The lower of this measured reserve and the previous reserve is used.
New words absent from the old inventory pay the fresh allowance. Roots and rolling
hashes are priced as nonzero before stitching. The current and old layouts are
bounded to one root update, four total callbacks, four nested result rows,
256-byte callback input and 256-byte results (64 in `deferred-storage-3`). Current
entries/calls are own-rollup and zero-value. Posting can store reverted entries,
static/revert-span/explicit-gas call flags, and reverted/static nested rows within
these bounds. Packed flags count as a nonzero word even with a zero source address.
Nested rolling hashes reserve a nonzero word before stitching; the position key
alone does not reveal the nested call mode. Other unmeasured shapes keep the
previous estimate. Parent inventory
reads are bounded before narrowing lengths or walking arrays, including inactive
mapping slots. No writes in a speculative candidate earn reuse credit.

These storage prices cover posting. Companion execution adds measured L1 source
gas, separately measured L1 callback application gas (including reverted work),
and bounded protocol overhead. Intercepted callbacks return their gas to the
source caller, so source gas alone omits that work. Version 7 reserves 70,000 per
root entry, 35,000 per callback, 25,000 per nested result row, 25,000 per nonzero
revert span marker, and 2,300 per 32-byte input/result word. Version 8 prices
validated nested input lengths with a four-word minimum (see the refinement
below); version 7 charged every nested input at its 256-byte bound. Queue scans cost 2,600 per preceding
deferred entry or 12,500 for a matching identity requiring root checks. These
terms are checked against cold-frame fixtures at the pinned protocol revision;
they are bounded admission estimates, not universal EVM gas guarantees.

The measured family permits one to four sequential root entries per source,
up to four callbacks and four nested result rows per entry, 256-byte root/callback
inputs and root results, 64-byte successful callback/nested results, own-rollup
zero-value mutable calls and implicit gas forwarding. The bounded forest covers
root L2 calls, L1 callbacks, L2 reentries and leaf L1 callbacks. Reverted root
results and bounded callback revert spans are supported; deeper recursion,
static calls, explicit callback gas and unsupported outcomes use the full signed
companion limit. Every callback proxy must already be registered in the canonical
parent. These are admission profiles, not an extension of the signer profile.

The combined prediction is capped at the signed limit: an uncertain prediction
cannot establish source invalidity. A successful-looking root does not prove
that the source advanced the cursor; an outer revert can undo it. The full
accepted prefix, including preceding roots of this same source, is charged for
each scan. Final ordered native execution still checks actual consumption,
required gas, signed limits and complete effects. Receipt ratios do not replace
these checks.

The existing 128-total-entry limit, full-calldata projection and final
ordered native validation still apply. No protocol pin, ABI, signer profile,
configuration, runtime database or historical follower decoding changes are required.

The retained `eez.composer.admission.v1` event family records each candidate,
source gas decision, summary (including no-cut batches), proof/preflight/delivery
timing and final signed hash. See [admission monitoring](../monitoring/indexer/README.md#admission-history)
for estimate/receipt joins, missing-event reporting and CSV export. Version
`strict-fifo-8` replaces sparse scaling after a faster wider proof while retaining
the measured slow-duration floor; `strict-fifo-7` introduced request-relative
scaling but required the wider observation to take at least as long.
`strict-fifo-6` releases a retained repair boundary on canonical head progress;
`strict-fifo-5` introduced FIFO prefix repair alongside the five-failure submission tolerance
introduced by `confirmed-failure-backoff-4`. It retains the capacity minimum
checks of `packing-minimum-check-3` and supersedes version 2's FIFO preference.
Historical `shared-deadline-1` events retain their former FIFO count semantics. Version 3 checks a minimum contribution before source
execution: 70,000 per outbound entry or 65,000 per inbound entry, zero calldata/
target gas and zero new companion consumption. It uses the same budget arithmetic,
including discount revocation, previously accepted companions and the new inbound
source's signed start requirement. L2 checks retain the existing declared/system
gas requirement. These are lower bounds on the admission model, not measurements
of actual execution. A miss retains the source without charging an attempt and
retains the complete younger suffix under `strict-fifo-5`; earlier policies
allowed independent younger sources to pass it. Empty candidates still run
normal simulation/classification. Cheap checks do not train
execution timing samples. No execution estimate is cached across changed state.

Estimator `candidate-framing-11` bounds each candidate's calldata. It counts the
native single-rollup ECDSA envelope/anchor, its mandatory accepted-root pin,
up to 128 proof bytes, and the target fork/operator's
maximum permitted blob indices. Each source adds its encoded entries plus
allowance for one root update per mutable entry, one pin per static entry,
position/rolling hashes and dynamic offsets. Actual payload bytes remain in
native blobs. The envelope is sized once, with per-source work only; accepted
prefixes are not re-encoded and KZG is not run for these estimates.

Admission compares the complete bound (including the active EIP-7623 floor)
against the margin-adjusted transaction budget and the original ordered
companion start requirements. Static pools revoke the same execution discounts
as final batch sizing, including the cheaper anchor. Unsupported entry shapes
retain general execution pins; metadata costs grow with their actual arrays.
It checks both inbound possibilities before a
cheap rejection: the minimum mutable entry and a static-only group with zero
mutable entries that restores general execution pins. Empty calls are rejected
by composition, so every valid source belongs to one of these possibilities.
Both must fail before simulation can be skipped. The inbound L2 lower bound is
zero because static-only lookups need no L2 delivery transaction. This preserves static-only work
without pricing it as a free addition to a discounted mutable batch. The checks
are lower bounds, not full execution or calldata estimates. Final native execution at signed gas limits,
the shared deadline and whole-group handling remain unchanged.

`projected_gas` retains the incremental execution/original-calldata projection.
New summary fields separate `framing_reserve_gas`, `full_estimate_gas`,
`calldata_bound_gas`, floor activation, general execution pricing and configured
margin. `planning_cap` now bounds the full estimate, rather than subtracting a
fixed reserve in advance. Source events expose framing and full totals before
and after the gas gate. The after-values are provisional: a later blob or timing
gate can still defer the source. Use its source outcome to establish acceptance;
rejected groups leave the accumulated ledger unchanged.
Cheap-check events add `minimum_mutable_entry_gas` and
`minimum_static_general_fallback`; `minimum_entry_gas` retains the zero inbound
minimum for compatibility. Historical estimator versions keep their old meanings. Existing collectors can
retain the additive fields without a database migration; update the dashboard
before or after Composer and preserve historical events/checkpoints on rollback.

Version 4 candidate `started` records add nullable `submission_failure_streak`
and `submission_backoff_threshold` details, displayed in Grafana's preparation
table. Numeric admission leaves these null. Existing collectors accept these
details without a schema change; historical candidates remain null. Version 7
uses the existing gas fields. These admission-only revisions do not change the
protocol contract. Runtime replacement still requires the same offline network
profile and matching prover validation identity; an older proving profile is
incompatible. Preserve monitoring history and checkpoints on rollback. A restart
still requires clients to reconcile and potentially resubmit unsettled work.

Deploy the additive collector/schema/dashboard before the new Composer. Collectors
that understand admission events but predate `source_capacity_check` reject that
unknown kind; old histories retain null bounds and counters. Rollback can leave those
additive tables and checkpoints in place. No ABI, storage migration, protocol
pin or signer-profile change is required.

Complete groups must fit L2 block gas, the active L1 transaction and whole-bundle
gas budgets, and the remaining blob allowance after companion transactions.
The L1 preflight combines the existing protocol gas projections with measured
source simulation gas. It reserves each companion's declared gas against its
estimated preceding consumption, instead of adding all declared gas limits.
Source simulation bypasses proxy consumption, so this is a planning estimate:
the final ordered native simulation checks actual cumulative gas, each next
transaction's declared limit, and complete effects before anything is dispatched.
In `gas_budget_cut` diagnostics, `companion_gas` is the source simulation
estimate; `postbatch.preflight` reports the final `companion_gas_used` separately.
Protocol-work projections remain conservative and can stop admission while
receipt-measured gas still has headroom. A `gas_budget_cut` planning event is
not a measurement that a merged candidate exhausted consensus capacity. See
the [mixed-workload packing audit](audits/2026-09-15-chiado-settlement-review/packing-improvement.md)
for the remaining distinction and the validated improvement.

At protocol `9d13218`, the legacy simple deferred-entry reserve is priced according
to their return-data storage: 230,000 for 0–31 bytes, 250,000 for 32 bytes,
and 270,000 for 33–64 bytes, provided the corresponding retained queue mapping
slot is empty or contains only one root update, no nested/callback rows, and
at most 64 result bytes. The Composer reads every potentially discounted slot
from the canonical L1 parent, including inactive slots beyond the current
queue length. An unknown, oversized, or unreadable slot keeps the 370,000
reserve. One successful simple call through a proxy registered
at the canonical parent reserves 70,000, plus its measured L1 target execution.
The `bounded-calls-2` estimator also prices a successful immediate entry with
multiple calls or nested results at 70,000 for its first L1 call, 40,000 per
additional L1 call (including callbacks), and 35,000 per nested result row.
This family permits at most four L1 calls and four result rows; all calls use
registered canonical-parent proxies, zero value, implicit gas forwarding,
mutable successful execution, at most 256 input bytes and 64 result bytes.
The measured topology is top-level L1 calls, L2 reentries and leaf L1 callbacks;
deeper recursion, forced reverts and static rows retain the general reserve.
Call/result evidence stays bound to the accepted occurrence through root
stitching and final ABI sizing. These immediate rows use transient storage;
deferred queue entries use the separate storage models above.
New proxies and unmeasured immediate shapes retain the existing
370,000 reserve. Actual target execution is added separately in every case.
The measured simple family permits up to 64 result bytes; an immediate call
also permits at most 256 input bytes and requires its entry result to be empty.
Calldata, the fork-specific calldata floor, companion execution and the
configured gas margin remain separate. These versioned reserves describe the
tested native ECDSA batch family; they are not network gas limits.

The conservative 190,000 base covers the leading anchor, original Core manager
context and registry root assertion. Its contract regression covers up to 64 blob
references; the calldata bound also includes the mandatory root pin. Incremental
and final sizing retain the same discounts for exactly one nonzero own-rollup
pin; missing, foreign, zero or duplicate pins keep conservative reserves and still
face the independent settlement validation. Queue replacement
resets active bounds without deleting old mapping values, so there is no
per-entry cleanup reserve. A later post can overwrite those retained values;
the slot check above determines whether the measured return-size reserve is
applicable. Existing proxies are read from pinned canonical state on each
attempt, including after a restart. Speculative proxy creation never earns
the lower reserve.
The [result-size reserve audit](audits/2026-09-16-chiado-result-reserves/README.md)
records the measured classes, native replay tests and public Chiado comparison.
Discounts cover at most 128 total entries, including the anchor; a larger
candidate restores the full per-entry reserve rather than imposing a transaction
count cap. The `admission.gas_projection` event retains its
`prior_cleanup_gas` compatibility field, now zero with this protocol pin;
`gas_budget_cut` also reports the applied entry discount. Final ordered native
execution remains mandatory before submission.

Admission telemetry keeps the finite snapshot (`selected`) separate from the
effective `selection_limit` and its originating reason. Summary reasons identify
gas/blob/time cuts, `configured_count_limit`,
`submission_backoff`, `native_preflight_backoff`, `pipeline_deadline_backoff`, `preparation_probe_limit`,
`snapshot_exhausted`, or `eligible_sources_exhausted`. A resource cut takes
precedence if it occurs before reaching the count cap. Snapshot exhaustion does
not describe arrivals after the snapshot. These observations do not change the
retry or ownership rules. `fifo_fairness_limit` is historical only. Policy
`strict-fifo-5` uses `fifo_prefix_rebuild` for a discarded non-FIFO simulation and
`fifo_prefix_limit` when its queue boundary restricts the next opportunity.
`fifo_prefix_len` identifies the repair prefix; `fifo_trigger` retains the gas or
pipeline refusal that caused it. `fifo_bypassed=true` may describe a discarded
candidate, which must never reach proving/submission; its replacement is linked
by `rebuild_of`. Successful candidates have `fifo_bypassed=false`. The summary's
`capacity_deferrals` counts each individually rejected group by reason, even when
older work fits in the later execution phase or a deadline ends discovery. Each
examined rejected source retains its own outcome reason. Younger sources beyond
the FIFO cutoff remain unexamined and have no simulated gas estimate;
`not_examined_count` and `selection_filtered_count` expose that suffix. New source records additionally
separate entry reserve, L1 call count and nested result count from target gas.

The new fields are additive within the existing admission event family.
Upgrade the monitoring views/dashboard before Composer. Old events retain
unknown selection limits/causes; no historical backfill is inferred. There is
no configuration, consensus/storage encoding, follower-replay or signer-profile
change. Runtime rollback retains the normal in-memory queue recovery boundary.
Keep the upgraded indexer/views on runtime rollback: an older indexer's startup
DDL cannot replace a view while dropping its newly appended columns.
The incremental blob estimate includes derivation metadata, semantic messages,
and V0 RLP byte-string wrappers for full blocks and entries. Ordinary blocks use
compact counts, sparse overrides and transaction byte strings. Unknown execution
header fields are conservatively bounded; the final actual encoding and ordered
bundle simulation remain authoritative. A single group still gets the exact
capacity check. Earlier L2 history uses the existing historical-drain rules.
Capacity/time deferral retains ownership and does not charge a failed attempt.

Automatic admission is a capacity estimate, not a guarantee that every attempt
will make its target: expensive transactions, RPC delays, and missed L1 slots
still use the existing retry/reconciliation paths. Ordinary L2 transactions use the interval DA guard described below. Numeric and unset settings keep the count behavior described above.

Ordinary L2 blocks receive an automatic hard data budget. The composer uses the receiving network's blob
maxima and `EEZ_MAX_BLOBS_PER_BATCH`, when set. It subtracts the exact data already
sequenced in the current interval and reserves room for empty blocks through
its next usable Sync boundary. The soft fullness percentage does not constrain
transaction admission. Announced future reductions are considered because
accepted data may settle later.

Transactions that fit a fresh interval but exceed its remaining space stay in
the mempool for a later block; nonce dependencies remain intact. An individually
unpostable transaction receives a descriptive RPC error. Catch-up preserves
empty intermediate boundaries. Neither this budget nor reaching a boundary
triggers settlement by itself: the configured policy still determines when to
post. Independent followers do not run this admission path. No additional
recovery journal is required. This guard prevents
new oversized intervals; it cannot rewrite an already accepted oversized suffix.

Anchor-only posting also measures encoding, sidecar, and witness preparation.
Target selection reserves the largest observed preparation time in this process
plus the configured proof time, using the selected network's slot duration.
This lets large fitting batches use a later eligible slot when necessary.
Cross-chain effects retain their bound execution slot and all existing deadline
checks remain active.

For example, this policy settles ordinary activity every 15 minutes,
consolidates every 150 blocks, and can post earlier at 10% aggregate fullness:

```dotenv
EEZ_SETTLEMENT_PURE_L2_MODE=interval
EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS=900000
EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS=150
EEZ_SETTLEMENT_BLOB_FULLNESS_BPS=1000
EEZ_MAX_SPECULATIVE_DEPTH=0
# Set both positive price ceilings for the chosen network and operator budget.
# EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS=...
# EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS=...
```

For production estimation, remove a copied smoke-test priority override unless
it is intentional. Configure relay minimums explicitly. Companion user
transactions retain their original signed bytes and authorized fees.

### Bounded next-block priority estimation

Dynamic mode requests the gas-weighted reward at percentile P for each of the
latest 20 canonical blocks with `eth_feeHistory`, selects the P-th percentile of
those 20 samples, and clamps it to an optional relay floor and an independent
priority ceiling. P is `EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE` (default 90, the
p90 of p90 described below).

With `EEZ_FEES_L1_TO_L2_ENABLED=true`, use filtered canonical receipt payments
instead of the unfiltered `eth_feeHistory` reward. Exclude transactions sent by
the configured poster and transactions calling the configured registry,
including calls whose logs were erased by a caught revert. Receipts emitting
`BatchPosted`, `ExecutionConsumed` or `CrossChainCallExecuted` from that registry
identify service activity without an additional trace.
These transactions carry our posting/recovery payments; their gross tips are
not an independent market bid. Match the registry address as well as the event
topic. Retain unrelated transactions in the same block, use their charged
receipt gas to weight the per-block percentile, and keep the same 20-block
selection and configured bounds. An empty filtered block contributes zero.
This removes service-price feedback without suppressing external competition.

Read blocks and `eth_getBlockReceipts` by hash along the sampled head's ancestry.
When a paid transaction is not already identified as service activity, obtain
`debug_traceBlockByHash` with `callTracer`; validate transaction coverage and
inspect nested frames, including reverted ones. An unrelated transaction still
contributes its tip, even when another transaction in its block uses the service.
Validate receipt count, transaction identities/order, block identity, cumulative
gas, total gas and effective prices before accepting each sample. Missing or
inconsistent evidence returns unavailable; it never falls back to unfiltered
pricing or bypasses the final cost floor. The node's submission RPC must support
hash-bound `eth_getBlockReceipts` and `debug_traceBlockByHash` with `callTracer`.
Trace RPCs have a four-second server timeout and traversal is bounded by block
execution gas, including the maximum gas refund. One async mutex shares a
64-block cache across
posting and quote requests; only validated samples enter it, keyed by block hash.
The cache contains no queue ownership and is reconstructed after restart.
The complete filtered read has a five-second timeout, including cache-lock
waiting; cancellation releases the guard without leaving a background sampler.
Fixed priority overrides bypass sampling. Without L1 refund recovery the original
fee-history path remains unchanged. Fee selection logs identify
`priority_source=filtered_receipts` and `excluded_priority_transactions`.

On Ethereum mainnet, a 7-day replay of the original unfiltered estimator
(2026-09-25 to 2026-10-02) of P=50 within 0.05–2 gwei averaged about 0.375 gwei:
- it was at or above each next block's 25th-percentile tip in 97% of blocks, and
  at or above its median tip in 59%;
- the default P=90 within the same bounds averaged about 1.95 gwei (99% and 95%).

These measure competitiveness against the block's own transactions, not
builder inclusion.
This targets prompt inclusion without a multiplier or mutable feedback state:

```dotenv
# Leave EEZ_L1_POSTBATCH_PRIORITY_FEE unset to use fee history.
# EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE=0
EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE=3000000000
```

Chiado uses a network-specific 1–2 gwei band:

```dotenv
EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE=1000000000
EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE=2000000000
```

#### Cross-chain priority

A bundle whose users execute cross-chain calls on L1 (immediate outbound
entries inside postBatch, or inbound L1-origin transactions) can depend on L1
state that earlier transactions in the target block change. The Composer
simulates against the parent state. A builder can place the bundle after a
transaction that changes that state, for example a swap on a pool whose
`slot0()` the call reads. The call then fails on L1 even though the postBatch
lands: the entry is skipped with `RollingHashMismatch`, and its dependents with
`RootMismatch`. Each such landing is charged as an incomplete settlement and
still pays for its gas.

Setting either `EEZ_L1_POSTBATCH_CROSS_CHAIN_*` variable gives these bundles
their own percentile and ceiling. Every other bundle keeps the base settings,
and the shared minimum applies to both. Selection follows the bundle's contents:
- composition prices a nonempty held-pool drain, which is always cross-chain work,
  at the cross-chain profile;
- final pricing selects the profile again from the surviving users' semantic
  transactions;
- a postBatch with none of them, such as an anchor-only or ordinary L2 batch,
  uses the base profile.

Final validation is unchanged. The exact signed bundle is re-executed at the
final fees, so an outcome that depends on the gas price is never dispatched
with a stale expectation.

A higher bid ranks the bundle above more of the block's transactions; it does
not guarantee a position. Searchers often pay builders directly instead of
through the tip, and builders choose their own ordering.

Evidence from Ethereum mainnet, 2026-10-06:
- 54 postBatches from deployment to L1 26133707. 25 carried cross-chain calls,
  and 16 of those landed with the call skipped.
- All 16 followed at least one swap on the Uniswap V3 USDC/WETH 0.01% pool in
  the same block, which the call read. The 3 successful calls in those retry
  bursts followed no such swap.
- The base P50 bid was 0.06–0.56 gwei. The best-paying swap ahead of each
  skipped post paid 0.10–40 gwei per gas, through the tip or by direct transfer
  to the builder.
- Recomputed with this estimator, P90 limited to cross-chain bundles would have
  outbid that swap in 9 of the 16 cases at a 2 gwei ceiling, 11 at 3.5 gwei,
  and 12 with no ceiling.
- Over those 54 posts, total spend would have risen from 0.0062 ETH to at most
  0.0176 ETH at 3.5 gwei. This upper bound assumes no retry was avoided.
- The other bundles keep P50.

The Ethereum example therefore uses:

```dotenv
EEZ_L1_POSTBATCH_CROSS_CHAIN_PRIORITY_PERCENTILE=90
EEZ_L1_POSTBATCH_CROSS_CHAIN_MAX_PRIORITY_FEE=3500000000
```

Unset, both profiles are identical and behaviour is unchanged. Node images
built before these variables existed ignore them and price every bundle with
the base settings, so they can be added to `.env` before or after upgrading,
and removing them is a complete rollback. They affect only local pricing, not
wire formats, storage or the signer. A fixed
`EEZ_L1_POSTBATCH_PRIORITY_FEE` prices every bundle exactly, so it cannot be
combined with these variables. Applications that read volatile L1 state remain
better served by values a same-block transaction cannot change, such as a TWAP
that ends before the current block.

The minimum is intentional on Chiado. A 2026-09-18 sample of 1,024 canonical
blocks ended with 19 zero-reward blocks and one 1-gwei block in its latest
20-block window, so the unbounded market statistic was zero. The 1-gwei floor
preserves the validated builder incentive in that sparse case, while the
2-gwei ceiling limits the amount actually paid as priority fee. These are the
Chiado deployment preset, not parser defaults: the parser minimum remains zero
and dynamic mode always requires an explicit maximum.

Each attempt samples immediately before signing and is bound to the same
canonical head as the quote. Empty blocks contribute zero, so a minimum is not
needed unless the selected relay imposes one. Isolated spikes do not control
the result, sustained market changes do, and the independent ceiling bounds
the signed tip. Missing, malformed or stale history preserves due work for
retry. A fixed `EEZ_L1_POSTBATCH_PRIORITY_FEE` bypasses fee history and remains
exact.

One poster transaction among a 20-block window cannot raise the outer
percentile (for any P below 100), so
an isolated post cannot feed its own price back into the next quote. On a chain
where the poster occupies several recent blocks its paid tip can contribute to
the market sample, but there is still no uplift and therefore no recursive
growth. The ceiling remains authoritative.

The tip is also the builder payment for an EIP-4844 poster transaction. Blob gas
has no separate priority fee: its target-block base fee is burned and is covered
independently by `EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS` plus blob headroom.
The priority estimate therefore improves builder competitiveness for blob
bundles without using blob base fee as a tip signal.

Execution and blob base fees already follow the selected network and fork.
Their existing `*_HEADROOM_BPS` settings raise the maximum permitted prices;
they do not multiply the base fees actually charged. The gas-limit margin
is applied to pre-refund execution gas, respecting the calldata floor. Storage
refunds reduce the receipt charge but cannot supply gas during execution. Actual
fee estimates and charges still use receipt gas consumption after refunds.
The complete bundle is revalidated at its final signed limit; the validated
larger-limit fallback remains available for additional gas-forwarding requirements.
Fee-quote events include the fee-history estimate, bounds, selected tip, and
the independent execution/blob fee caps.

Use a relay minimum only when the selected relay needs it. The 1-gwei minimum
in the local Kurtosis PBS profiles funds their builder payout; it is not a
mainnet default. Do not copy that fixture minimum into a production profile
without validating its requirement. These settings are read at startup, so
changing them requires a node restart with a binary supporting this option;
no contract redeployment is needed. The same rules apply to Ethereum and Chiado
in their respective native fee units.

## Network profiles and execution

### Conditional slot production

**Implementation draft; not yet approved for production rollout.** Set
`EEZ_SETTLEMENT_START_AFTER_BLOCK=P` to choose the ordinary block after which
settlement preparation may begin. Positions count from the preceding immutable
sync boundary, so K=6, L2=2 seconds, P=3 means the block at +6 seconds. Ordinary
ticks use the genesis time grid. Remove `EEZ_PREPARATION_TIME_MS` entirely first;
empty is invalid, not absence. P is required and must be an integer in `1..K` (excluding K).
`EEZ_SETTLEMENT_START_DELAY_MS` is optional, defaults to zero, and must be less
than one L2 interval. With P=3 and delay=250 on the 12s/2s grid, P commits at
nominal +6s and planning starts at or after +6.25s. Ordinary ticks still produce
P on time; the queue is observed after the delayed trigger. If P commits late,
do not wait another 250 ms. The original target deadline and block timestamps
remain unchanged. More arrival time does not guarantee admission or inclusion.
Startup requires positive preparation time after subtracting the delay and reserving the configured proof
seed, submission slack and the existing 150 ms post-proof reserve. The latter
already includes delivery and is counted once. Proof and submission controls
remain enforced; P plus delay replaces only the preparation trigger.

After recovery/ownership maintenance and the committed cutoff prefix, Composer
returns a read-only preparation hint using the canonical settlement anchor,
projected boundary height/time, known activity/fullness, window pressure, retained
drain/predecessor work and held cross-chain queue occupancy. Occupancy is not proof
that a request is eligible. Final policy, fees, admission, proofs and submission
still decide whether to post. No fake future block hash is supplied to policy.
Planning does not create/extend a drain or consume queued requests. A blocking
optimistic owner selects paced recovery/Empty maintenance instead; final selection
still takes its finite request snapshot after the burst. Its database
work runs on a blocking worker with a single shared permit retained until exit,
including after the waiting cutoff times out. The wait is capped at the next
future ordinary grid tick or remaining deadline headroom, whichever is earlier.
An on-time P-plus-delay decision therefore waits at most `L2 - delay`; a late
entry uses the next future tick and never resets the settlement deadline.

If preparation is due, the existing pipeline rapidly builds the remaining
ordinary blocks using the pool, then composes the sync block. Otherwise it keeps
ordinary blocks paced through the end of the interval. **Every sync boundary
still uses the existing empty checkpoint path** unless cross-chain composition
supplies its authorized native actions. This release does not put pool
transactions in skipped sync blocks. A later ordinary tick may finish a missing
empty boundary after `EndTick`, subject to recovery and normal commit checks.

Only a successful not-due hint selects the non-posting `Ordinary` checkpoint
mode. Planner errors/timeouts keep paced production but retain the existing
`Empty` policy/recovery opportunity at the boundary, so unavailable planning
cannot deadlock settlement at the speculative-depth cap. Incomplete or stale
canonical context is retried from ticks; it is never treated as not-due evidence.

The cutoff decision belongs to one exact known L1 parent, target, reconciled
generation and local branch. Late requests or newly crossed policy thresholds do
not reopen an ordinary decision; they wait for the next eligible cutoff. Stale or
unavailable evidence does not authorize a burst or bypass empty-block validation.
A new canonical scan generation invalidates the hint even on a same-branch L1
extension; fresh recovery may reevaluate settlement at the missing boundary.
The existing bounded catch-up, speculative-depth limits and maximum four skipped
slot replacements still apply; there is no L1-independent sequencing mode.
Armed bounded replacements supply the next paced prefix after its preceding
boundary time, even while the next L1 head is delayed; an unfinished valid prior
decision retains its boundary semantics. Ticks catch up only already-due blocks
within the existing work bound. `eez.slot.configuration`, `eez.composer.slot.plan`
and `eez.sequencer.slot.decision` record configuration, hints and choices.

There is one scheduler: unset P fails startup rather than selecting legacy timing.
Compose transports absent, empty and explicit values without silently choosing
precedence; shell values override `.env`. Kurtosis carries P and delay through
`settlement_env`. The removed preparation input remains visible to validation so
an obsolete operator setting cannot silently disappear.
Composer and new follower binaries share the timing parser: both require explicit
P and reject the removed preparation setting. Followers do not schedule or use
the delay to reinterpret history, but their launch environment must migrate too.
There is no protocol pin, wire, ABI, database or signer configuration migration.
Existing follower replay and mixed-version
signer/follower compatibility use unchanged block formats and empty checkpoints;
the process/replay release gate still must verify this. Migrate configuration
together with the Composer upgrade: remove the preparation setting, explicitly
choose P and initially use zero delay. There is no runtime legacy-mode switch.
For rollback, use a verified compatible prior binary with its matching timing
configuration; remove the delay for versions that do not support it. Candidate A
at `9ca75ba` supports P but has no delay setting and still has the displaced-batch
sender-cap bug demonstrated by the
[failed load test](audits/2026-10-09-conditional-slot-production/stress-failure.md).
It is not a known-good rollback for that workload.
Preserve original state and establish storage compatibility before a downgrade.
Normal restart may discard speculative history and in-memory pending requests;
clients may need to resubmit. Never run two Composers on shared state.

Live settlement can change the authenticated execution seed of already-produced
unsafe descendants. Reconciliation replaces the first stale child with an
executed empty sibling, letting Reth requeue ordinary transactions from the
removed suffix. The Composer pool's maintenance-only adapter revalidates these
displaced transactions with local origin: Reth's default external reinsertion
would otherwise apply its 16-transaction sender cap to an already accepted
suffix. RPC/network ingress keeps the unwrapped pool and its existing limits.
System envelopes and Composer-owned cross-chain sources remain excluded by
the same validator. Reinsertion failures are logged, not counted as successful
recovery. Explicit `--txpool.nolocals` settings still disable local
exemptions; this repair does not override operator admission policy.
Reth does not retain the original origin in block data. Recovery therefore
selects **all** local policies: the RPC fee cap, local propagation setting,
eviction exemptions, and ordinary Reth local-transaction backup when enabled.
An externally received transaction above the local fee cap can be rejected on
recovery. Minimum-tip, execution, DA and replacement checks still apply. A
validation error, including temporarily unavailable DA limits, is logged without
an automatic retry; clients must reconcile and resubmit if necessary. This is
not a durable guarantee for every accepted transaction. Reth's pool backup does
not persist Composer request ownership or its retry counters. Startup backup
reinsertion is also best-effort and can run before DA admission is ready; it is
not a durable ordinary-transaction journal. Reconcile actual canonical receipts
and pending hashes, not the upstream backup log's attempted-insertion count.
Those transactions must execute again under the new context;
their earlier receipts are not final. The replacement does not advance safe or
finalized state. This recovery repair is enabled for every Composer, needs no
wire/storage migration, and remains compatible with existing followers. Rolling
back the binary restores the old ancestor-only rewind and its transaction-loss
risk; it does not recover transactions already lost from an in-memory pool.

Empty local checkpoints use `EEZ_L2_FEE_RECIPIENT`, just like ordinary production
and minimal settlement blocks, including empty replacements for a nonempty Sync
after an observation deadline or canonical exclusion. An already empty Sync
keeps its existing header and descendants; retained-history reconstruction uses
the recorded beneficiary rather than substituting the current setting. This
changes newly built checkpoint hashes for a nonzero recipient, not the block/DA
format: existing history, including zero-beneficiary recovery blocks, remains
replayable using its recorded environment. Within the same execution profile,
only the Composer's construction policy changes; signers and followers need no
upgrade, and no protocol, genesis or database migration is needed. Rolling back
this correction restores zero beneficiaries for future recovery replacements
without rewriting settled history. Followers retain upstream external reinsertion
policy; they do not sequence displaced ordinary work. The adapter
adds no worker, queue, or durable recovery state.

See the [proposal and acceptance gates](proposals/conditional-slot-production.md)
and [implementation evidence](audits/2026-10-09-conditional-slot-production/README.md).

### Shared target handling

Ethereum and Chiado retain their existing slot and submission-window settings.

The trigger occurs at the preceding boundary plus P L2 intervals and the optional
delay. Block timestamps and the Sync grid remain unchanged; the potential Future
suffix contains `K-P-1` ordinary blocks. The Chiado preset P=2, delay=0 triggers
at +2 seconds on its 5-second/1-second grid, leaving 3 seconds before the target,
including proof seed, submission slack and post-proof reserve. This is not a
latency guarantee. Budget for large historical batches as well as small bridges.
Changing timing requires a node restart; the shared absolute deadline remains enforced.
If the expected L1 block is still absent at the following preparation deadline,
the scheduler permits up to four replacement opportunities on the same parent.
Each retains the target execution height and advances its timestamp one slot.
The Composer rebuilds its speculative suffix and retains all old submissions as history;
canonical replay resolves late original inclusion. This does not classify the
slot as skipped or charge user retries. No new timer setting is introduced.
The shared scheduler/submission bound limits an outage to five target opportunities
per observed L1 head, including the original. Exhaustion logs
`eez.slot.l1_anchored.replacement_limit` and waits for a new head. Older Composers
retain one replacement; this extension needs only a Composer upgrade and has no
wire, signer, follower or storage migration. See
[settlement ownership](../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).
On the 5-second/1-second Chiado grid, this preset permits two Future blocks
when preparation is selected; otherwise those positions remain paced.

After catch-up, the sequencer rechecks the terminal. Only a terminal exactly at
the requested future height and timestamp, with deadline headroom and no
speculative-cap pause, may use steady composition in that same opportunity.
Intermediate, capped, past and misaligned boundaries remain empty. This avoids
wasting a usable future slot after rebuilding a missed interval.

Policy-driven historical batches target the immediate child of the observed
canonical L1 head. Native proofs bind that parent's hash; choosing a later
execution height from a timing estimate would require an unknown parent. The
first unexpired slot is selected, with the same bounded timestamp replacements
used by the scheduler when slots are missing. Builder lead controls when
submission opens, not a mandatory block gap. Selection is logged as
`eez.submitter.policy.target_selected`, including the observed head, target height
and `slot_offset`. The former preparation-reserve history and `block_offset`
field are removed. Actual encoding/KZG/witness preparation duration remains
available in `eez.composer.settlement.preparation_time`.

Preparation, proving and submission use one complete target deadline, including
ordinary-only windows. A slow observation cannot permanently move every attempt
to an unknown future parent. Final timestamp, canonical-head, fee, provenance
and deadline checks still apply, and missed opportunities preserve pending work.
No new timing setting is required. Once an exact target has been selected,
capacity splitting into historical prefixes preserves its execution height and
timestamp for every candidate and submission. Only an unpinned or expired
opportunity selects a fresh target before preparing a new proof.

Composer nodes wake the existing L1 scanner on embedded canonical-head
notifications, so settlement evidence need not wait for the two-second fallback
poll. The scan still verifies canonical hashes, complete event ordering and
native blob data before reconciliation. Notifications coalesce, and missed or
closed notifications retain periodic recovery; finality refreshes keep their
existing timer cadence. Followers retain periodic polling. This adds no polling
or proof-time configuration.

The composer uses the embedded node's actual fork configuration and executor,
including Gnosis fee collection and fork transitions. It resolves outgoing
limits at the intended target timestamp. The signer uses its fixed local L1
profile at the exact actual-parent target. Historical proofs have no inclusion
timestamp and use the largest configured blob allowance as an admission bound.
L1 execution and followers enforce the actual containing block's rules. Known
compatible forks do not invalidate historical proofs; incompatible program changes
require coordinated on-chain key revocation/replacement. The Chiado and
Kurtosis Compose profiles supply the corresponding local L1 profile to the signer.

Final preflight executes `[postBatch, inbound companions...]` in order against
local canonical L1 state. It checks active intrinsic/floor/transaction/block gas,
sidecars, balances, fees, nonces, applied roots, skipped immediate entries, and
actual cross-chain lookup outcomes. Missing or inconsistent local state defers
submission. A successful outer call alone is insufficient.

Fees are refreshed at the existing submission window. Only an unsent poster
envelope may be repriced, and its complete bundle is simulated again. Dispatch
rechecks network identity, canonical fee-sample identity, and target. Existing
attempt ownership governs sent transactions.

Final preflight also emits [shadow fee observations](proposals/cross-chain-fee-recovery.md#second-development-increment-composer-shadow-fee-evidence)
in the existing posting/admission event details. These pair the complete
postBatch's measured gas and physical blob cost with gross ordinary tips from
the final L2 terminal-block receipts, keeping native system gas separate at
zero price. The L2 source set is checked against the settlement manifest. Costs
and tips retain their own chain units and exact candidate context. Without the
optional [fee cost model](#cross-chain-fee-configuration), realization,
proving/operations, risk and profit remain unpriced. Missing or inconsistent
accounting is reported as unavailable and does not alter dispatch, scheduling,
accepted-work ownership, retries or canonical reconciliation. These observations
have no admission authority and require no protocol or
storage migration. They do not constitute a user quote or recovered revenue.

Restart discards the policy's in-memory intent and cached summaries. Canonical
anchors and pending activity are rebuilt under the new configuration. Restart
time never becomes a settlement timestamp. Existing nonce and source-outcome
reconciliation remain authoritative.

L1 startup preflight checks chain identity, deployment height, and finality of
the immutable genesis reference. It does not require deployment-era receipts on
every restart. Before L1-active tasks start, the deriver authenticates that
reference and validates the persistent correlation tail against canonical L1
and local L2 hashes. Recovery scans the selected checkpoint's containing L1
block **inclusively**; without a valid checkpoint it falls back toward deployment.
Required logs, transaction bodies, headers and native blob data must still be
available for the selected recovery range. Missing history fails recovery; it
does not authorize skipping blocks or declaring settlement complete.

Each correlation retains the accepted endpoint's executed history state and
header. Same-height partial consumption can replace its Reth state; recovery
uses the retained record and rechecks its canonical L1 occurrence and origin.
The fresh-network index format is `eez-batch-correlation-recorded-history-v1`;
older formats are rejected, without an in-place migration. Ordinary production
persists the exact parent witness locally before advancing, so rapid catch-up
cannot outrun witness capture into pruned trie state. This does not wait for the
prover or an L1 response, but local storage failures still fail closed.

See the [recovery and rollback requirements](runbooks/ethereum-node.md#recovery-with-a-pruned-l1).

Recovery can retain validated ordinary L2 history present in the reopened chain
database. A completed graceful shutdown lets the engine flush its recent blocks
and state; a crash or forced kill can lose an unflushed suffix. An execution
receipt alone is not a crash-durability acknowledgement. Settled history is
recoverable from canonical L1 and available blob data; unpublished history that
was never persisted has no such recovery source. Clients must reconcile receipts
and nonces before resubmitting unsettled transactions. The policy does not make
in-memory request ownership or retry counters durable.

Composer links the embedded L1 runtime's graceful shutdown and critical-task
monitor to the CLI while retaining independent L1/L2 worker pools. Both engines
are given the existing CLI shutdown budget. The pinned Reth shutdown
[backport](../crates/vendor/reth/README.md) delays termination acknowledgement
until engine-owned persistence and prewarming workers release their resources.
Exceeding the shutdown budget can still race process exit; forced shutdown and
persistence errors can leave an unflushed suffix. Neither a termination
notification nor a successful process exit proves that
every pending request is recoverable.

See the [migration validation record](audits/2026-09-26-reth-27429/README.md#retained-failures-and-limits)
for tested crash/restart behavior, retained failures and recovery limits.

Reth persistence settings are local storage choices, not consensus parameters.
The v2.6 port retains upstream production L2 defaults. The short persistence
window for isolated native-test L1 miners exists because those miners have no
peers from which to recover lost history. Operational policy-change tests must
use graceful shutdown; deliberate crash tests must establish any required durable
fixture state before killing the process.

<a id="shadow-l2-fee-pricing"></a>

## Cross-chain fee configuration

Fee collection is independently enabled with `EEZ_FEES_L1_TO_L2_ENABLED=true`
and `EEZ_FEES_L2_TO_L1_ENABLED=true`. Both default to false. Only literal `true`
and `false` are accepted; empty and malformed flags fail startup. Supplying
costs or refund terms alone never enables collection. A complete cost model
with L2 collection disabled still permits advisory L2 quotes. L1 quotes require
L1 collection to be enabled; this profile has no advisory-only L1 mode.

`ComposerSettings` reads an immutable startup snapshot. The native launcher
loads process environment before `.env` before `deployments.env`; restart is
required for changes, with no CLI override. Direct Compose interpolation gives
shell variables precedence over `.env`; explicit service environment overlays
can override the rendered values. The existing `chiado-up.sh` wrapper first
sources `.env` and later `deployments.env`, replacing inherited values at those
steps. Keep operator fee settings in `.env` or deliberate service overrides,
and deployment bindings in `deployments.env`. Inspect the effective node
environment for the actual launch path before deployment.
Chiado, Ethereum and external Kurtosis Compose forward the settings only to the
node. Kurtosis `eez.settlement_env` accepts the same names as quoted strings;
the signer and follower do not receive them. Its UI fee flag follows the L2
collection switch. Unknown `settlement_env` names are rejected; arbitrary unknown
process environment variables are not a supported configuration interface.

The shared cost model requires **eight settings** below. All absent means no
model; supplying any cost setting requires the complete set. Monetary assumptions
have no implicit defaults, including zero costs, parity conversion and profit. All
numbers are unsigned decimal integers; signs, whitespace, fractions, hex and
overflow are rejected. The exclusion list is the only cost field accepting an
empty value.

| Setting | Meaning and bound |
| --- | --- |
| `EEZ_FEES_L1_PER_L2_NUMERATOR`, `EEZ_FEES_L1_PER_L2_DENOMINATOR` | Positive U256 values: conservative L1 native base units realizable per L2 native base unit, after proportional haircuts. No inferred parity or live price lookup. |
| `EEZ_FEES_PROVING_COST_L1`, `EEZ_FEES_OPERATIONS_COST_L1`, `EEZ_FEES_TREASURY_COST_L1` | Nonnegative U256 native L1 base units charged once per complete post. Explicit zero is allowed. These are configured allowances; treasury includes the fixed realization/sweep cost. |
| `EEZ_FEES_RISK_BPS`, `EEZ_FEES_PROFIT_BPS` | Integers 0..10000, independently applied to posting plus the three fixed costs. Risk is mandatory; profit is the additional quote target and may absorb later cost changes. |
| `EEZ_FEES_EXCLUDED_SENDERS` | Comma-separated distinct 20-byte addresses, at most 64 and 4096 bytes. Empty means none; surrounding address whitespace is allowed. The poster and L2 beneficiary are always excluded too. |

`EEZ_FEES_QUOTE_BASE_FEE_PERCENT` is optional and independent of the cost model:
an integer 100..1000, default 150, giving the percentage of the sampled source
base fee placed in an unsigned type-2 quote's `maxFeePerGas` before the tip. It
bounds only what the wallet signs; charged fees, tip requirements and admission
are unchanged. Absent uses the default; empty or out-of-range values fail startup.

The model derives chain IDs, the owned L2 rollup and its beneficiary from runtime
wiring. The shipping node owns one L2 rollup; a configured fee model rejects zero
chain IDs, zero/multiple owned L2 rollups, and a zero `EEZ_L2_FEE_RECIPIENT`.
There are no separate fee-specific chain or rollup ID settings. Measured chain
and beneficiary identities remain checked before using receipts as revenue.

L1 collection also requires `EEZ_FEES_L1_REFUND_RECIPIENT` (nonzero plain EOA)
and `EEZ_FEES_L1_REFUND_PERCENT` (integer **1..99**). Either supplied term requires
the other, even when L1 collection is disabled. Both may be prepared while the
flag is false without requesting refunds or enabling L1 quotes. The percentage
is fixed for that startup snapshot and shared across configured builder relays;
it is not the profit margin and is not dynamically selected per transaction.
Every relay still requires qualification for the [refund profile](#l1-builder-refund-profile).

Amounts always carry their chain's base-unit denomination; token symbols do not
define conversion. Debug output retains the normalized `assumptions` object with
runtime-derived identities, and its identifier is Keccak-256 of that object's
JSON serialization. Equivalent normalized assumptions retain the identifier.
It is not an authorization or treasury price certificate. The historical
observation labels `mode: shadow` and `pricing_status: shadow_priced` remain
readable; they describe projected revenue, not configuration or enforcement modes.

This **synthetic fixture** illustrates scalar syntax and explicit assumptions,
not production prices or an instruction to enable collection:

```bash
export EEZ_FEES_L1_TO_L2_ENABLED=false
export EEZ_FEES_L2_TO_L1_ENABLED=false
export EEZ_L2_FEE_RECIPIENT=0x4444444444444444444444444444444444444444
export EEZ_FEES_L1_PER_L2_NUMERATOR=2
export EEZ_FEES_L1_PER_L2_DENOMINATOR=3
export EEZ_FEES_PROVING_COST_L1=7
export EEZ_FEES_OPERATIONS_COST_L1=5
export EEZ_FEES_TREASURY_COST_L1=3
export EEZ_FEES_RISK_BPS=1000
export EEZ_FEES_PROFIT_BPS=500
export EEZ_FEES_EXCLUDED_SENDERS=
```

### Removed fee-policy expiration

Fee policies have no calendar cutoff. `EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP` has
been removed; its presence, including an empty value, fails startup with migration
instructions. Gas-price sampling, individual quote freshness, source/context
checks, signed funding admission and final native settlement validation still
apply. No conversion or cost assumption is refreshed automatically.

New debug/observation output omits `assumptions.valid_until_l1_timestamp` entirely.
Removing that key changes the policy identifier even when all monetary assumptions
are unchanged. Historical records retain their original identifiers and timestamp
values (a number, or `null` from builds with optional expiration); readers spanning
versions must also accept an absent field. The compact quote response and its
30-second per-quote expiry are unchanged.

Remove the obsolete setting from shell, `.env`, `deployments.env` and effective
service overrides before starting this Composer. Recreate it with the updated
environment after draining accepted work. Older Composers that require a timestamp
reject the non-expiring configuration; restore a valid explicit timestamp before
rolling back to them. Configuration is still read once at startup. There is no
protocol, signer-wire, genesis, storage or historical follower-replay migration.

### Fee configuration migration

The former `EEZ_L2_FEE_SHADOW_POLICY`, `EEZ_L2_FEE_ENFORCE_TARIFF`,
`EEZ_L1_FEE_REFUND_POLICY` and `EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP` are removed.
Their presence, including an empty value,
fails startup with migration instructions. Mixed old/new settings are rejected;
there is no compatibility parser. Compose and Kurtosis forward obsolete names
only so the runtime can reject them rather than silently disable fees.

Map each retained cost JSON field to its uppercase `EEZ_FEES_` name above. Drop
the old `valid_until_l1_timestamp` field as described above. Join the
excluded address array with commas; an empty array becomes an explicit empty
value. Remove `version`, chain IDs, rollup ID and `beneficiary` after verifying
they match the runtime bindings and `EEZ_L2_FEE_RECIPIENT`. Map the old L2 switch
to `EEZ_FEES_L2_TO_L1_ENABLED`. If the old refund policy was configured, set
`EEZ_FEES_L1_TO_L2_ENABLED=true` and copy its recipient and percentage into the
two scalar terms. Preserve monetary assumptions and exclusions; renaming
settings does not authorize changing fee economics.

Upgrade configuration and Composer together after draining accepted work. Remove
old names from shell, `.env`, `deployments.env` and every effective Compose
overlay; assigning an empty string does not remove a setting. Check the rendered
node configuration without exposing secrets. **Old binaries ignore the new
names**, so restore both the old binary and old configuration on rollback.
There is no mixed-version compatibility guarantee for these settings. No
protocol, genesis, signer-wire, persistent storage or follower-replay changes
are required. The compact RPC schema and refund arithmetic are unchanged; the
debug assumptions lose only the expiration field described above.
In-memory pending work and refund attribution still require the existing
[upgrade procedure](runbooks/chiado-deployment.md#recovery-and-upgrades).

### Fee pricing arithmetic

Let `P` be final native L1 post execution plus physical blob cost, `F` the sum of
the three fixed costs, and `n/d` the configured conversion:

```text
subtotal_l1 = P + F
minimum_l1 = subtotal_l1 + ceil(subtotal_l1 × risk_bps / 10000)
required_l1 = minimum_l1 + ceil(subtotal_l1 × profit_bps / 10000)
minimum_l2 = ceil(minimum_l1 × d / n)
required_l2 = ceil(required_l1 × d / n)
projected_recovery_l1 = floor(eligible_effective_L2_tips × n / d)
```

Arithmetic uses checked U256 totals and U512 intermediate products. Overflow
makes pricing unavailable. Base-fee burns, native system gas, historical L2 fees
and excluded senders cannot supply projected recovery. The complete L1 post
includes shared historical DA and all nested work, even for a single source.
No expense paid by an L1 companion or bridged principal is counted as poster
cost. No L1 builder refund is assumed as income.

`fee_observation` version 2 adds `pricing` with the assumptions, required amounts,
eligible/excluded tips, projected recovery, and mutually exclusive nonnegative
`shortfall_l1`/`surplus_l1`. Successful calculation sets `pricing_status` to
`shadow_priced`. `pricing.comparison` (`covered` or `shortfall`) is hypothetical
coverage of the complete candidate under those assumptions, with no liquidity
or partial-consumption guarantee. `qualified_recovery_l1` remains null.
`minimum_service_payment_l1` / `minimum_service_payment_l2` and `cost_coverage`
report the separate hard floor; `comparison` still compares against the quote
target. A target shortfall can therefore coexist with sufficient cost coverage.

Only a single eligible L2 source with no L1 companions gets
`required_tip_per_gas = ceil(required_l2 / source_receipt_gas)` and
`minimum_max_fee_per_gas_at_current_base = current_L2_base + required_tip`.
Both are L2 base units per gas. The latter has **no future base-fee headroom**.
These are diagnostic minimums for current signed bytes and execution context,
not wallet-ready quotes; changing fees can change execution. Shared/mixed
candidates use `tip_status: standalone_estimate_required` and a null tip.
Zero source gas, excluded sources and values exceeding u128 signed fee fields
have explicit statuses and null tip fields. Native system gas and the signed
gas limit never enter the payment denominator.

Absent policy uses `pricing.status: not_configured`; failed/expired inputs use
`unavailable` with a reason. Original measurements remain visible. Existing
event collectors preserve this nested JSON without a schema migration. Version
1 historical observations remain unpriced; missing observations stay unknown.
Upgrade readers before enabling the optional policy, and handle both versions.
Rollback omits version 2 pricing; it changes no protocol, wire commitments,
genesis, follower replay or durable queue state. Existing admission and posting
events remain version 1. This feature makes no durable financial reservation.

## Compact fee quotes

`eez_quoteFee` and `eez_debugQuoteFee` use positional params `[transaction, options?]`
on the Composer fronts only. `/composer/l2` selects L2-origin pricing;
`/composer/l1` selects the optional [L1 refund profile](#l1-builder-refund-profile).
An unconfigured direction returns `-32004` with `error.data.code = "quotes_disabled"`.
Regular node/follower transports do not expose either method. These are project-specific JSON-RPC
methods, not Ethereum standard methods.

```json
{
  "jsonrpc": "2.0", "id": 1, "method": "eez_quoteFee",
  "params": [
    { "from": "0x1111111111111111111111111111111111111111",
      "to": "0x2222222222222222222222222222222222222222", "data": "0x1234" },
    { "transactionTypes": ["0x0", "0x2"] }
  ]
}
```

`from` and `to` are required; creation is unsupported. `value` defaults to `0x0`
and `data` to `0x`. `chainId` defaults to the front's source network; a mismatch
is rejected. An omitted `nonce` starts from the pinned source state. On L1,
Composer advances across the sender's contiguous owned pending sources to quote
the first available nonce, replaying those sources before estimation. Explicit
nonces are never changed; an already-owned requested nonce is rejected. Both
automatic and explicit future L1 quotes can replay up to eight contiguous, owned
predecessors from that sender; gaps, missing signed
bytes and unsupported predecessors fail explicitly. Each predecessor executes
its signed fields and complete nested effects through a conditional native
post/source sequence at the same target, retaining both chains' disposable
state. This sequence is an estimate, not a promise of the final shared batch
layout. Replacement or release during quoting invalidates the prefix. L2
future-nonce quotes and ordinary-mempool predecessors remain unsupported.
With tariff enforcement enabled, signed calls behind an owned predecessor go
through this stateful admission before ordinary-mempool forwarding. An isolated
trace can revert before reaching dispatch when the predecessor has not executed.
Unsupported successors, including ordinary calls behind privately held work,
must wait for that predecessor to become canonical. Policy-disabled routing is
unchanged.
This read does not reserve a nonce; clients must recheck the wallet's pending
nonce before signing. The SDK's automatic wallet-send guard remains
conservative; explicit pipelining can use the quoted envelope after checking
the caller's owned nonce sequence.

Older Composers default an omitted L1 nonce to canonical state and can reject it
as already pending. Clients quoting prepared transactions should continue to
send their prepared nonce; that profile works before and after this change.
Automatic selection requires the updated Composer, without a signer, follower,
protocol, genesis or storage migration. Rolling back restores the older default;
use explicit prepared nonces for bursts. The response schema is unchanged, and
concurrent quotes can still return the same unreserved nonce.

Before nonce selection and after calculation (including errors), L1 quotes compare
the sender's nonce in the completed snapshot with current hash-bound L1 state.
If it changed, the response is `stale_context` (`-32004`) with
`details.reason = "source_nonce_changed"`, `sampledBlock`, `currentBlock`,
`sampledNonce` and `currentNonce`. Block numbers and nonces are hex quantities.
An already-mined ordinary predecessor must not be reported as a permanent queue
gap, and an omitted nonce must not return a spent nonce. Unrelated head advancement
remains valid within the existing context-age bound. A true future gap still fails.

Only this condition detected **before execution** is refreshed internally: at most
eight refreshes, 250 ms apart, rebuild the complete prepared L1/L2 context within
the original ten-second deadline and held worker/capacity permits. If state changes
after execution starts, return the same retryable diagnostic without resetting the
execution/frame budget. Persistent pre-execution lag also returns that diagnostic;
the overall request deadline may instead return `quote_timeout`. No signed bytes
or pending ownership change. This Composer-only correction changes no protocol,
signer wire, genesis, database or follower replay. Mixed versions retain the same
successful schema; old Composer versions retain the nonce/snapshot bug.

`gas` is optional. Its explicit value is preserved and must fit 21,000..16,777,216
as well as native transaction/block capacity. When absent, the existing internal
cross-chain estimator discovers and verifies it, including nested calls. The
estimator and fee variants share one pinned L1/L2 context and exact posting target.
The provisional gas search is followed by pricing and effect validation at the
final gas and price. Gas/fee-sensitive code may require the high bound or fail
explicitly; the API does not promise a minimum gas limit or universal convergence.
The initial L1 gas candidate reserves 20% plus the call stipend for shared-table
scans; L2 retains 5% plus the stipend. This sizes the gas limit, separately from
the fee policy's profit margin. Only consumed gas is priced. Final native
preflight may still need to reduce the shared batch.

`transactionTypes` defaults to `["0x0", "0x2"]`. Supply one type to reduce work.
Alternatively `transaction.type` selects one type; combining it with conflicting or
multiple `transactionTypes` is invalid. Duplicates, empty selections, unknown
options/transaction fields, explicit nulls, fee fields, access lists and authorizations
are rejected. Input quantities must be canonical hex strings.

The compact result is:

```json
{
  "version": 1,
  "chainId": "0x1",
  "nonce": "0x0",
  "expiresAt": "0x6b000000",
  "options": [
    { "type": "0x0", "gas": "0x186a0", "gasPrice": "0xf",
      "requestHash": "0x…", "estimatedFee": "0x4ce78" },
    { "type": "0x2", "gas": "0x186a0", "maxPriorityFeePerGas": "0x5",
      "maxFeePerGas": "0x14", "requestHash": "0x…", "estimatedFee": "0x4ce78" }
  ]
}
```

Values above are illustrative. Apart from numeric schema `version`, all compact
quantities, including expiry Unix seconds, use canonical hex. Fees are in source
native base units. `estimatedFee` is charged source gas times the quoted effective
price; the separate maximum reservation is `gas * gasPrice` or `gas * maxFeePerGas`.
A legacy `gasPrice` is the sampled source base fee plus the quoted tip and is
charged in full. A type-2 `maxFeePerGas` is `EEZ_FEES_QUOTE_BASE_FEE_PERCENT`
(default 150) percent of the sampled base fee, rounded up, plus
`maxPriorityFeePerGas`: the base fee can rise by that margin before admission and
inclusion without clipping the tip, while the charged price stays base fee plus
tip.
The headroom does not raise the tip: admission still recomputes the required tip
from current prices.
Each option's `requestHash` is Keccak-256 of the normalized eight-field transaction
request's UTF-8 JSON, with lexicographically sorted keys, no whitespace, lowercase
addresses, calldata and minimal lowercase hex quantities. It includes resolved type/gas/nonce,
chain, sender, destination, value and calldata. It does not authenticate the server
or authorize execution, and is not an Ethereum transaction hash. Changing the
transaction or its fees requires a fresh quote.

Quotes are advisory estimates, not inclusion or treasury-realization guarantees.
Ordinary L2 calls use zero settlement contribution; they still pay normal L2
execution fees. A missing fee policy or
unknown L1 direction never becomes a zero-cost cross-chain quote.

Every requested type has one ordered entry. A variant that cannot execute returns
`{"type":"0x2","error":{"code":"unsupported_transaction","message":"…"}}` instead
of wallet fields. Invalid input for a variant uses `invalid_transaction`. An
insufficient source balance uses `insufficient_funds`, with `error.details`:

```json
{
  "reason": "source_balance",
  "sourceChainId": "0xdd6d9",
  "availableBalance": "0x64",
  "requiredBalance": "0xc8",
  "shortfall": "0x64"
}
```

These amounts are source-chain native base units from the failing validation.
Priced execution requires the transaction value plus the maximum gas reservation
(`gas * maxFeePerGas`, or `gas * gasPrice` for legacy). This is not an estimated
charge. If gas discovery stops because the balance cannot cover the value alone,
`requiredBalance` is that value; fees have not yet been calculated. The same
details appear in compact and debug failed options, and in `error.data.details`
for signed tariff admission. This error is distinct from `l2_tip_underfunded` or
`l1_tip_underfunded`, which compare the offered settlement contribution to the
tariff. Gas estimation also preserves the source balance failure as JSON-RPC
`-32000` with `error.data.code = "insufficient_funds"` and these details.

No variant may be signed when it contains an error. Shared unavailability, stale state, expiry
or exhausted work fails the whole RPC with the existing quote error envelope;
there are no partial successful options after those errors. Invalid request syntax,
chain/nonce or selection fails with `-32602`. Transient errors never trigger a
silent unquoted send.

Both priced directions bound the initial posting-gas probe by the poster's
balance at the sampled L1 state, after reserving physical blobs at their fee
cap. They use the same affordability calculation as final signed posting.
The network/operator gas limit remains an upper bound; it is not a required
reservation for a smaller settlement. Complete native execution, the gas margin
and replay at the selected limit remain mandatory. Native balance checks are
not disabled. Pending L1 predecessors use the replayed account balance.

If the poster cannot fund the probe, the whole quote fails with `-32004` and
`error.data.code = "poster_insufficient_funds"`. Its `details` contain
`reason: "poster_balance"`, `scope: "poster"`, the posting address, L1 `chainId`,
`availableBalance`, `blobFeeReservation`, `maxAffordableGas`, `gasBudget`, and
the sampled L1 block identity. Quantities are canonical hex. This is an operator
funding condition, separate from source balance or insufficient source tips.
The affordable gas bound is not a measured gas requirement or a top-up quote;
the response does not invent an exact shortfall when execution could not finish.

The debug method accepts identical params and returns the compact result plus
`diagnostics.recoveryMode` and `diagnostics.options`. `recoveryMode` is
`enforced_tariff` when signed ingress and final dispatch enforce the configured
L2 tariff, otherwise `advisory`; the L1 profile uses `builder_refund`. This metadata appears only under `diagnostics`; it
is absent from `eez_quoteFee` and is not needed to construct a wallet transaction.
`diagnostics.options` contains the detailed calculation result (or error) for each type,
including policy, contexts, measured costs and replay iterations. Ordinary-L2
diagnostics use `estimate.basis = "ordinary_l2"` and zero `requiredL2Tip`; they do
not invent L1 cost components. Debug is a projection of the same calculation,
not a trace by default. Separate calls can select different contexts and need not be equal.

Only `eez_debugQuoteFee` additionally accepts `options.trace = "calls"`:

```json
[
  {"from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","data":"0x"},
  {"transactionTypes":["0x2"],"trace":"calls"}
]
```

This adds `diagnostics.trace`, a versioned `eez-quote-call-trace` export from
the actual blocking quote calculation. It retains calls on failed execution
variants as well as successful quotes. Omission preserves the existing debug
response; other trace values, including null and false, are invalid. The compact
method rejects the trace option before acquiring a worker.
Traced quotes must be submitted individually. A JSON-RPC batch containing a
debug quote with a `trace` option is rejected with `-32600` before any member
executes, so batch response accumulation cannot multiply the capture budget.

Each run records its transaction format, phase (`gas_estimation`,
`fee_simulation`, or `final_validation` in older exports), purpose, actual transaction fields and
call frames. Composition links nested chain executions to their suspended
callers using explicit parent IDs. Native source, system and settlement replays
are separate runs. Conditional setup and settlement simulate proof results;
their traces do not establish proof verification. Context records the L1 parent
and posting target, L2 parent/environment and resolved nonce. Pending
predecessors establish this state before capture; their transaction bodies are
not exported as predecessor runs. Internal state-provider reads and hash-only
prefix construction are outside the call trace.

L2 pricing validates call identities, outcomes, source return data and outbound
hashes during the native source replay and any callback deliveries. It uses that validated
replay's receipt gas, so no separate final composition/source pass is needed.
Both transaction formats still execute independently at their actual gas and
fee fields; gas-sensitive discovery and L1 settlement limit checks remain.
L2-origin gas discovery constructs its native block prefix once per format,
including pre-execution changes and any replayed predecessor transactions.
Composition and native probes execute on independent forks of that prefix;
nonce, writes, transaction position and consumed gas never carry between probes.
The high-limit, selected-limit and conditional fallback probes remain, as do
their nested action/output comparisons. No preparation spans quote requests.
Within one format's pricing loop, an identical successful L1 settlement gas
measurement may be reused. The complete posting transaction (including gas,
fee caps and blobs), proof-call data, settlement endpoint and nested callback
expectations must match under the same immutable L1 state and target environment.
Registry/deployment validation, verification-key lookup and poster-account reads
are prepared lazily once per format at the pinned L1 state. A different rollup
or expected settled root repeats that validation; failed preparation is not
cached. Blob reservations, affordable gas, batch/proof inputs and callback
expectations are recalculated for each candidate. Source composition and native
validation always run at the new fees. Changed replay inputs and failed
measurements require replay. This is local to the quote, with no reuse between formats,
requests or snapshots. `diagnostics.options[].estimate.l1SettlementReuses`
counts reused measurements for cross-chain L2 options. The trace retains the
original L1 executions; it does not synthesize duplicate runs for reuse.
Effect mismatches appear at `native_replay` during `fee_simulation`.
Run counts and phases are diagnostic observations, not fixed API guarantees.
These optimizations change no compact quote fields, policy or persisted state.
Older Composers perform the extra passes and omit the optional debug reuse
counter; mixed versions, historical replay and rollback need no migration or
coordinated signer/follower upgrade.

Capture is limited to 128 runs, 4096 frames and 1 MiB of raw input/output across
the request, with at most 4096 bytes per captured field. Lengths and truncation
flags preserve the distinction between empty and omitted bytes. Capture limits
do not stop execution or change a fee decision. Incomplete EVM runs retain
unfinished frames. RPC-level context, provider or deadline failures can prevent
the calculation from completing. When the blocking calculation returns an error
after capture began, the error remains a whole-request failure and retains
`error.data.trace`. That export has `partial: true` and a bounded `failure`
containing the safe error code, message and available diagnostic details. No
successful wallet options are returned with that failure. Provider error text
and credentials remain private. Failures before capture begins, RPC timeout,
cancellation, shutdown and worker panic can still return no trace; no durable
trace ID or server storage is provided.

This correction is a binary/API upgrade with no protocol, storage or genesis
migration. The added funding code and optional trace fields retain existing
JSON-RPC error semantics and trace schema version 1. Upgrade Composer to fix
both quote directions; older SDKs must continue rejecting error responses.
Upgrade the viewer to import error envelopes directly, or extract
`error.data.trace` for an older viewer. Rolling back restores the maximum-probe
failure and generic diagnostics; it requires no chain-state rollback.

Frames preserve CALL, STATICCALL, DELEGATECALL, CALLCODE, CREATE and CREATE2,
caller, target/storage and code addresses, value, static context, return bytes,
EVM outcome and inclusive frame gas. Gas excludes transaction-level overhead
and is not additive across nested frames, chains or simulation runs. An inner
revert can be caught, and a successful child's effects can be unwound by a
later ancestor revert; neither its status nor a gas-probe failure alone
establishes the quote outcome.

The dashboard's Debug view renders these exports. Viewer assets and browser tests
are maintained in the dashboard repository; RollupX owns the trace RPC and its
backend tests. Tracing does not affect contracts, signer RPC, storage, genesis,
historical replay, follower behavior or fee enforcement.

The version 1 export has the following fields:

| Field | Meaning |
| --- | --- |
| `format`, `version` | `eez-quote-call-trace`, `1` |
| `request` | Original caller-supplied transaction |
| `context` | L1/L2 chain IDs, pinned parents, L1 target, L2 timestamp, resolved nonce and pending predecessor count |
| `runs[]` | Ordered executions, each with an ID, quoted type, phase, pass, purpose, chain ID, actual transaction, `complete` and `frames[]` |
| `truncated`, `omittedFrames`, `omittedRuns`, `limits` | Explicit capture limits and omissions |
| `gasAccounting` | `inclusive_frame_execution_gas` |

Run IDs are zero-based and contiguous. Frame IDs are zero-based within each run;
`parentId` is null for a root, otherwise an earlier frame in that run. `pass`
increments when composition begins within a phase/type, and its subsequent native
replays carry the same pass. The quoted `type` and actual `transaction.type` can
differ for synthetic/system executions. `crossChain` identifies a different
chain from the parent; `executionRoot` identifies a new EVM instance. `from` is
the EVM caller, which DELEGATECALL preserves; use `parentId` to identify the frame
that issued the call.

`input` and `output` are `{hex, length, truncated}` objects. `length` is the
original byte count; `hex` may hold only a prefix. A frame that did not complete
has `status: "incomplete"` and may lack output/result/gas-used fields. Run
`complete` means all observed frames ended; it does not override truncation or
imply success. Context and IDs use JSON integers where bounded; chain IDs,
transaction quantities, values and gas use hex strings.

To capture a response, send a single request to the Composer front for its source
chain (`/composer/l1` or `/composer/l2`). Save the raw HTTP response to retain
`error.data.trace` on whole-request failures:

```bash
COMPOSER_RPC='https://YOUR_HOST/composer/l2'
TX='{"from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","data":"0x","value":"0x0"}'
jq -n --argjson tx "$TX" \
  '{jsonrpc:"2.0",id:1,method:"eez_debugQuoteFee",params:[$tx,{transactionTypes:["0x2"],trace:"calls"}]}' |
  curl --silent --show-error --max-time 60 "$COMPOSER_RPC" \
    -H 'content-type: application/json' --data-binary @- \
    --output quote-trace.json
```

For a failed execution variant, only `diagnostics.options[].error.details`
adds the available execution cause:

```json
{
  "reason": "execution_reverted",
  "stage": "gas_estimation",
  "message": "order expired",
  "messageTruncated": false,
  "revertData": "0x…",
  "revertDataLength": 100,
  "revertDataTruncated": false
}
```

The example bytes and length are illustrative. `reason` is `execution_reverted`,
`effects_mismatch` or `unsupported_execution`. `stage` identifies the quote pass:
`gas_estimation`, `fee_simulation`, `source_execution`, `native_replay` or
`final_validation`; it does not identify a nested call's address or depth.
The latter two reasons retain the estimator's explanation without revert-data
fields. Reverts retain at most 4096 bytes as hex, the original byte length as a
JSON number, and an explicit truncation flag. Complete payloads within that bound
are decoded as standard Solidity `Error(string)` or `Panic(uint256)` when valid.
Custom, empty, malformed or oversized payloads use `execution reverted` and
retain the bounded raw bytes; no custom ABI or failing contract is inferred.
Messages are capped at 512 UTF-8 bytes, without splitting a character.
Backend/provider failures remain operational errors, without provider details.

The top-level `options` remain the compact projection, including inside debug
responses: execution failures keep `unsupported_transaction` and no execution
details or signable wallet fields. Funding diagnostics described above remain
available in both projections. Debug formatting does not change quote execution,
retry categories, fee enforcement, source nonces or queue ownership.

Both methods share the bounded quote worker pool. Signed tariff checks have
separate capacity; both lanes use a ten-second request budget. Per type, at most three gas discovery probes, four
pricing iterations and one final validation probe run
(maximum sixteen across both types). Each probe may compose and replay nested
execution. Pricing retains the aggregate 32-target-frame/eight-depth limit;
L1 pending-prefix replay adds at most eight predecessor passes under the same
frame, DA and deadline bounds. It does not acquire another worker or mutate
queue ownership, canonical state or the original signed envelopes.
discovery/final validation reuse the estimator's 64-recorded-action bound and
shared deadline. Existing calldata, DA/history, gas and blob limits remain active.
No ordinary estimator worker is acquired. Cancellation is cooperative; a running
blocking frame retains its permit until it exits, including after RPC timeout.

The dependency-free [SDK](../sdk/l2-fees.mjs) adds `requestFeeQuote`,
`feeQuoteTransaction` and `sendFeeQuotedTransaction`. Supply an expected source
`chainId`, selected type, `maxTotalFee` bigint and a `keccak256(utf8String)` function
from the app's existing Ethereum library. Pass the unsigned input as the SDK's
`transaction` argument. For viem, use
`text => keccak256(toHex(text))`. The helper verifies the resolved transaction binding,
expiry, selected type and maximum fee, and copies only approved wallet fields into
the original transaction. The send helper rechecks wallet account, network and pending
nonce, then requests one ordinary transaction signature. Wallet approval delays,
base-fee changes and wallet edits can invalidate the quote; native admission is
still authoritative. Compact output does not pin a policy ID; applications that
require policy verification must use/debug-check the diagnostic response.

The API replacement is intentionally breaking: `eez_quoteL2ToL1Fee`, the
`/eez/fees/l2-to-l1` HTTP route and the old SDK helpers are removed. Upgrade
Composer and applications together; old callers fail instead of falling back.
Coordinate rollback of both. This changes no storage, contracts, genesis,
historical decoding, signer RPC or follower replay, and needs no data migration.
The draft also renames `intentHash` to `requestHash`, the SDK input argument
from `intent` to `transaction`, detailed input from `intent` to `request`, and
input errors to `invalid_transaction` / `unsupported_transaction`.
`recoveryMode` moves from the compact result to `diagnostics.recoveryMode`.
No aliases are retained; the normalized eight-field binding bytes are unchanged.
Keep final signed native settlement validation enabled.

The source-balance diagnostic is an additive error-contract correction. Older
Composers can report the same failure as `unsupported_transaction`,
`invalid_transaction` or `quote_unavailable` without balance details. Clients
must reject any failed option, including an unknown error code; they may display
the new details when available. Successful quote fields and request bindings are
unchanged. Composer can be upgraded or rolled back independently for this
correction; no protocol, signer, storage, configuration or historical-replay
migration is needed. Rollback loses the improved diagnostics, not fee enforcement.

Execution diagnostics are also additive and need only a Composer upgrade.
Clients must tolerate absent details from an older Composer and treat displayed
contract messages as untrusted text. Successful options and bindings are
unchanged. Rolling back removes execution details; it does not change historical
decoding, follower replay, signer behavior, configuration or storage.

## Unsigned L2 fee quotes

Both methods use the following measured pricing contract. The diagnostic endpoint
exposes its full evidence; the compact endpoint projects wallet-ready fields.

The backend selects the last completely scanned canonical L1/L2 pair and prices
a hypothetical next L1 block at that parent. A later scan or submitted attempt
may remain pending. It extends the L2 parent through retained ordinary blocks,
stopping before any unresolved native effects. Thus independent requests remain
quotable without treating speculative cross-chain state as settled. L2 time/seed
derivation uses only anchors available at the sampled L1 block. Simulation
environments and cold state reads stay pinned in private overlay channels.
Its draft next L2 block is a hypothetical source context, not a prediction of
the eventual inclusion slot. The complete supported group is composed and
replayed as native loads, the unsigned source, and callback deliveries.
Normal source nonce, balance and fee checks remain enabled. Only native source
receipt gas divides the service charge; native system gas is reported separately.

L1 gas is measured by executing the complete outbound `postAndVerifyBatch`
under the native L1 configuration at the selected parent/target, including
nested calls, proxy creation, storage refunds and the applicable calldata floor.
It prices receipt gas, separately from the gas-limit headroom used to make the
posting executable. Conservative admission reserves remain capacity limits and
are not charged as estimated consumption. Physical blob costs are added before
the configured service costs and risk/profit allowance; the Chiado trial applies
its 20% target markup once (zero mandatory risk reserve, 2000 profit bps).

Unsigned execution cannot produce an authenticated proof or final signed L2
block hashes. Disposable settlement estimation uses distinct full-width root
and versioned-blob-hash placeholders and a 65-byte low-s signature placeholder.
It recognizes the pinned registry, rollup, wrapper and ECDSA verifier bytecode,
computes the exact verification digest for that private batch, executes the real
verifier to retain its gas, and assumes a valid result only for the single exact
registry verification call. Reverted/malformed verification, unknown bytecode,
skipped entries, incomplete root advancement or mismatched nested effects fail
closed. No signing key is used, no proof is requested, and no simulated artifact
can enter canonical execution or submission. Final signed native execution and
independent signer validation remain mandatory. Actual proof/root bytes and
future state can change receipt costs slightly; this remains an advisory estimate.

Blob sizing includes exact retained ordinary transaction bytes and metadata
plus the complete semantic stream. A maximum-width source signature placeholder
is used only for encoding size; it is never authenticated, proved or submitted.
Unresolved native history is excluded by choosing the preceding L2 parent. A
transaction depending on that excluded state (including a nonce predecessor or
a newly deployed contract) still needs reconciliation; its dependencies are not
guessed. The quote charges this single transaction the complete estimated post
and retained ordinary history; it assumes no future batch participants.

The [fee pricing arithmetic](#fee-pricing-arithmetic) adds service costs,
risk/profit and conversion. Starting at a zero priority fee, the backend raises
the tip to `ceil(required_L2 / native_source_gas)`, then repeats composition,
native replay and sizing with those actual fee fields. It returns only when the
replayed gross tip covers the estimate. At most four simulations are allowed;
execution depending on fees may fail or not converge. `maxFeePerGas` is the
draft base fee plus this tip, without future base-fee headroom. Legacy quotes
return that sum as `gasPrice`, with no EIP-1559 fee fields. On execution, only
`gasUsed * (gasPrice - actualBaseFee)` counts as legacy priority revenue; the
burned base fee is excluded. A base-fee increase can invalidate coverage.

Each successful `diagnostics.options` entry contains:

| Field | Meaning |
| --- | --- |
| `version: 1`, `mode: advisory` | Application response schema and guarantee level |
| `request`, `requestHash` | Normalized eight request fields; Keccak-256 of their compact, lexicographically key-sorted JSON |
| `transaction` | The normalized request plus `gasPrice` for legacy, or `maxPriorityFeePerGas` and `maxFeePerGas` for type 2; all wallet quantities in hex |
| `issuedAt`, `expiresAt` | Unix seconds; expires thirty seconds after issuance, independently of the hypothetical L1 target slot |
| `context` | Chain/rollup IDs, L1 parent and target, L2 parent and draft timestamp, settled L2 endpoint |
| `policy` | Policy ID and all conversion, cost, exclusion and validity assumptions |
| `estimate` | Source/native L2 gas, measured conditional L1/target gas, DA bytes/blobs, distinct L1 execution/blob/subtotal/risk/profit amounts, required L1/L2 amounts and gross L2 tip |

Monetary amounts in `estimate` are decimal strings. Gas/counts/context heights
are integers. `estimate.basis` is
`isolated_native_l2_and_l1_settlement`, with `estimate.proof` set to
`metered_assumed_valid`; `qualifiedRecoveryL1` is null.
L1 figures are estimates, not receipts. Neither the request hash nor response
authenticates a quote or guarantees inclusion. Applications must trust their
chosen endpoint and reviewed policy, preserve the original transaction, and impose
their own maximum fee.

The SDK compares wallet chain ID and pending nonce numerically, accepting
safe non-negative integers or bounded zero-padded hex observations. Transaction and
quote quantities remain canonical. Wallet observations cannot change signed
transaction fields. The pending nonce check detects ordinary-mempool predecessors
that isolated quote execution does not simulate.

Each Composer direction has four workers for compact/debug quotes with space for sixteen waiting requests.
Signed admission has one reserved worker and space for eight waiting requests,
so quote traffic cannot occupy its capacity. Each lane returns `quote_busy` only
when its total capacity is full. The ten-second request deadline includes queue
wait and network preparation; the simulated frame budget also remains bounded. Input is limited
to 160 KiB of serialized transaction/options JSON, calldata to 64 KiB (65,536
decoded bytes, including the selector and any ABI encoding), retained ordinary
history to the active emission window minus one terminal block, encoded DA to 2 MiB,
cross-chain target frames to 32 across all iterations, and nesting depth to eight.
Active network/operator gas and blob limits and the negotiated emission window
still apply. Expensive work uses Reth's blocking executor. Disconnect, timeout
or shutdown sets cancellation; checks between frames/history/replay steps stop
further work. A running EVM frame cannot be interrupted, and retains its
worker and capacity permits until it exits. Frames also have existing EVM gas limits.
History walks forward from the validated settlement endpoint and stops at the
first unsettled native block. A long speculative suffix does not invalidate an
otherwise usable bounded ordinary prefix. An ordinary prefix beyond the posting
window still needs canonical settlement advancement.

Malformed params/transactions use JSON-RPC `-32602`; other shared quote failures
use `-32004`, with `{code, message}` in `error.data`. Stable application codes
include `invalid_transaction`, `unsupported_transaction`, `quote_limit`, `stale_context`,
`insufficient_funds`, `poster_insufficient_funds`, `quotes_disabled`, `quote_unavailable`, `quote_busy` and `quote_timeout`.
Per-variant execution/input errors are represented as described above; shared
errors never return a usable option. IDs, mixed batches and notifications follow
the front's existing semantics. Members execute sequentially under the shared
worker bound. The 160 KiB transaction/options bound applies alongside the front's
overall transport-body limit. Requote after genuine availability/context failures. Ordinary head advancement
alone does not discard a quote. Before and after simulation, both sampled hashes
must remain canonical. Neither chain may have advanced more than sixty seconds
beyond its sampled parent, and the L1 fee sample must be at most sixty seconds
old by the server clock. Reorged context, stale samples and startup without a
completed scan remain explicit failures.

Changing gas, nonce, calldata, value or fee fields invalidates the estimate.
Later state/base-fee changes, wallet edits and approval delays can invalidate it
too. Quotes provide no inclusion guarantee, liquid L1 reserve, treasury realization
or durable financial ledger. SDK checks run with `node sdk/l2-fees.test.mjs`;
source-based process regressions are selected with `fee_quotes::`.

#### Calldata limit compatibility

The 64 KiB bound applies to compact/debug quotes, tariff-enabled signed admission
and the SDK, for both legacy and type-2 transactions and both source directions.
The serialized-input allowance accommodates hex encoding (128 KiB for maximum
calldata) plus transaction fields and options. This is a source transaction limit;
it does not increase the gas, DA, frame, nesting, worker or deadline budgets.
A large source and its forwarded call can require multiple blobs, which the
quote must price under the active network limits. Final native settlement
validation remains required.

Upgrade every Composer front serving an application before enabling larger
payloads in that application's copied SDK and clients. Older SDKs and Composers
still reject calldata above 12 KiB; mixed front versions cannot consistently
quote or admit the expanded range. The wire format, signing RPC, protocol contracts,
genesis and database format are unchanged. Existing signers and followers
already decode these lengths; historical replay does not use the quote parser.
No migration or protocol pin change is required.

Before rollback, stop accepting larger requests and drain accepted work to
canonical settlement. Older Composers restore the smaller quote/admission
profile; retained or resubmitted large requests may then be rejected.
Queue ownership is in memory, so restart is not durable pending-work recovery.
This compatibility procedure does not establish a live deployment.

### Opt-in L2 tariff admission

`EEZ_FEES_L2_TO_L1_ENABLED=true` requires the complete, runtime-bound
[cost model](#cross-chain-fee-configuration). Absent or `false` preserves existing
admission; empty and other values fail startup. This is a testnet tariff, separate
from the production strict-funding design's durable reserves and treasury
realization. Chiado, Ethereum and Kurtosis pass it to the Composer only.

For a newly submitted outbound source, the L2 front recovers the signed legacy or type-2
transaction and replays its exact fees using the quote engine's bounded isolated
state. Its effective priority payment must cover the whole measured settlement
cost plus the mandatory risk reserve. The desired profit markup is not an
admission requirement. The comparison uses native receipt gas, never the user's gas limit, and
accounts for clipping by `maxFeePerGas - baseFee`, or the legacy difference
`gasPrice - baseFee`. Unsupported quote profiles
are rejected before queue admission. A tariff failure returns JSON-RPC code
`-32003` with `data.code = "l2_tip_underfunded"`; availability and profile errors
retain the quote engine's stable `data.code`. Rejection neither consumes a nonce
nor acquires queue ownership. Already-owned identical transactions remain
idempotent. Ordinary L2 admission retains its existing rules; the independent
L1 refund profile below controls L1-origin work.

The destination matters for factory deployments. Creating a Safe on L1 through
its L2 cross-chain proxy is an outbound source, including the factory's CREATE2
and initializer calls. Creating the Safe entirely on L2 is ordinary traffic and
has no minimum settlement contribution under this flag. Under the scope accepted
on 2026-10-15, ordinary L2 users pay normal execution fees and their settlement
may be operator-subsidized; no additional settlement tip is required. This does
not exempt L2-to-L1 work, including nested calls, from the outbound tariff or
change its whole-post pricing. See the
[Safe fee validation](audits/2026-10-14-safe-fee-recovery/README.md) and the
`safe_fee_quotes::` process regressions for this distinction.

Composition checks the growing outbound prefix against the same complete
conditional L1 execution estimate using actual native receipt fees. A changed
state, reduced charged gas or insufficient effective tip defers the source and
its nonce successors, restores the accepted prefix, and continues independent
sources. Admission quotes charge each source for a complete standalone
post; composition can pool their actual receipts. Quotes do not discount shared
batches. Fee-enabled candidates may include both directions under the same FIFO,
nonce, whole-group, gas/blob and deadline rules. The last accepted outbound-prefix
simulation supplies the cost of posting that group separately, including retained
ordinary history and its physical blob count. L1 additions cannot fund its L2
tariff. Nested calls within one source remain whole.
Quoting a future L2 nonce whose predecessor has not executed is outside this
outbound profile. The compact L1 route has the bounded owned-prefix replay
described above.

Immediately before dispatch, an outbound-only attempt must still cover its
complete newly priced L1 post, blobs, configured service costs and risk reserve
using actual eligible L2 native receipts. For a mixed attempt, compare those same
receipts with the standalone outbound cost and allowances. Reuse that reference
only when its ordered source hashes and entire L1 fee quote match final preflight:
chain, parent, target, fee caps and effective execution/blob prices. Missing or
mismatched evidence defers the unsent attempt for recomposition without a user
retry charge. A rejected outbound addition cannot replace the accepted reference.
This is the existing native counterfactual estimate, not an actual standalone
payment; the complete signed mixed bundle always undergoes final native execution.
The preflight event's `l2_tariff` identifies the cost basis, gas, blobs, required L2
payment and additional mixed posting cost. Existing `fee_observation` describes
the whole post and is not a per-direction coverage assertion. A batch may earn less than the
quoted profit target; with risk zero it may settle at cost. Poster, beneficiary and explicitly excluded senders cannot
finance this check. Failure defers the unsent attempt without charging a user
retry or declaring canonical completion. Accepted work remains subject to
state, price and availability changes; this is not an inclusion guarantee.

The dashboard enables the fee flow with `EEZ_UI_L2_FEES_ENABLED=true` and caps
the wallet's maximum execution fee with `EEZ_UI_L2_FEE_MAX_TOTAL_WEI` (default
`100000000000000000` L2 wei). It quotes before the single ordinary wallet
signature and preserves quoted fees for injected wallets and the disposable
local test signer. It calls `eez_quoteFee` directly on `/composer/l2`, including
internal gas estimation, and defaults to legacy quotes
when the selected provider identifies itself as Rabby, whose custom-network
path uses `gasPrice`. Other providers default to type 2; an explicit supported
transaction type is preserved. Quote failures stop submission rather than
falling back to a generic wallet tip.

The simplified trial uses `risk_bps: 0` and `profit_bps: 2000`. Moving the
existing 2000 bps risk allowance to profit preserves the quoted target but
relaxes the hard floor. Retaining a nonzero risk reserve is supported; do not
change it implicitly. Older Composers enforce both fields as a hard tariff, so
rollback can defer signed transactions that the new policy accepted. Drain or
account for accepted work before policy changes and preserve both configurations.
The compact response shape is unchanged; the added minimum figures are debug
and observation fields. Historical observations may omit them. This enforces a
refreshed simulated cost floor under configured conversion/service assumptions,
not a guarantee of realized profit or immunity to price and execution changes.

No protocol, genesis, wire encoding or storage migration is involved. Upgrade
the matching node/signer runtime and quote-capable dashboard before enabling
the tariff. Existing queued outbound work is subject to final dispatch checks,
so drain or explicitly account for it before changing the policy. A restart
retains the existing in-memory queue limitations and may require resubmission.
Disabling the gate or rolling back restores earlier admission behavior; it does
not undo transactions or collected tips. Historical follower replay is unchanged.

## Deployment checklist

1. Select the mode, general threshold, and both price ceilings.
2. Keep `EEZ_MAX_BLOCKS_PER_BATCH` absent; adjust speculative depth for `N + K - 1`.
3. Configure the same reviewed L1 profile for signer/follower deployment, retain
   native sidecar and relay settings, and size manual prover limits for the
   intended backlog. Prover errors do not authorize automatic splitting.
4. Inspect decision, allowance, fee-quote, preflight, dispatch, and canonical
   settlement diagnostics during the profile's integration checks.

Packing format is unchanged. Multi-blob batches are preferred; splitting uses
valid historical boundaries only when actual blob or gas capacity requires it.

### Nested-input companion refinement

Estimator `companion-input-8` keeps version 7's queue, callback, row and rollback
reserves. After validating the bounded action forest, each nested input pays
2,300 gas per rounded 32-byte word, with a minimum of four words and maximum of
eight. The minimum covers measured repeated expected-table copying; empty input
does not make that work free. Previously every nested input paid eight words.
Unsupported shapes and inconsistent action evidence retain the signed gas limit.
The discount is an admission prediction; ordered native execution still checks
the complete signed bundle against the same target. Posting gas is unchanged.

Delivery events now add optional relay bundle hash, exact target/parent,
transaction count and remaining shared deadline after acknowledgement. A relay
hash is accepted only as a 32-byte correlation identifier; absent or malformed
identifiers stay null without changing acknowledgement semantics. Neither a
hash nor an acknowledgement proves inclusion. With several relays, see
[builder relay fan-out](#builder-relay-fan-out) for the per-relay `builders` field. Historical readers use null for
missing fields. No RPC, storage or protocol migration is needed; upgrade the
Composer and dashboard independently. Rolling back removes the discount and new
observations without changing historical decoding or follower replay.

### FIFO policy compatibility

FIFO prefix repair changes Composer scheduling only. The protocol pin, wire
profile, historical decoding, follower replay, signer validation and operator
settings are unchanged. Existing collectors accept the new policy/reason strings
and optional detail fields; historical reasons remain valid. Upgrade the Composer
to enforce this policy; older Composers retain relaxed admission. No storage
migration is required. Restart loses pending in-memory queue state and its FIFO
boundary under the existing recovery contract; clients may need to resubmit.
FIFO constrains the proposed source set. L1 partial consumption can still omit
inbound companions from that same proposal; canonical reconciliation retains
and retries eligible omitted sources under the existing contract. Rollback
from version 6 to version 5 retains strict FIFO but can strand a partially
consumed repair prefix in a small batch; earlier policies restore relaxed
scheduling. Version 6 needs only a Composer upgrade; signers, followers and
collectors can run mixed versions under the unchanged wire/event schema.
Reorg reconciliation restores source ownership and order; a cleared scheduling
boundary does not authorize execution, eviction, or canonical completion.
Versions 7 and 8 change only Composer's in-memory proof timing estimate. Version
8 also supersedes a sparse scaling term after a faster wider observation, while
retaining the slow-duration floor during extrapolation. It preserves version 6's
FIFO behavior, existing event fields and all actual deadline gates. Only Composer
needs upgrading; signers, followers and collectors can run mixed versions under
the unchanged wire/event schema. Timing samples disappear on restart, alongside
the pending-queue restart limitations above. Rollback from 8 to 7 restores the
duration-ordered extrapolation and its possible sparse-proof width plateau; no
queue, database, historical replay or wire migration is required.
The separate main-branch Reth
existing-database compatibility limits still apply to deployment.

### PostBatch target

By default the poster transaction calls `postAndVerifyBatch` on the registry.
`EEZ_L1_POSTBATCH_TARGET` sends it to another contract instead. The intended
use is a wrapper such as `EEZPostBatcher` (a fixture copy is in
`contracts/src/EEZPostBatcher.sol`): it forwards the batch to the registry and
then reverts with `LastImmediateRootMismatch` unless the registry's root
equals the last immediate entry's `newRoot`.

**Why use it.** Without the wrapper, a failed immediate entry is caught
(`L2TxSkipped`), the postBatch still lands, and its gas is spent although the
user stays unsettled. With the wrapper the poster transaction reverts instead.
The poster is never in `revertingTxHashes`, so a builder cannot include the
bundle in that position: the slot is observed as excluded, which spends no
postBatch gas and uses the separate excluded-slot budget. The trade-off is
that the whole batch, including other users and the rollup's root advance,
waits for a slot in which every immediate entry completes.

**Contract requirements.**
- The target must expose `postAndVerifyBatch` with the registry's exact
  selector and calldata. The Composer sends the registry ABI unchanged, and
  derivation finds each post through the registry's `BatchPosted` log and
  decodes the transaction input with that ABI, whatever its destination.
  Calling a different entry point, such as `EEZPostBatcher.postBatch(batch,
  bool)`, would make the post undecodable.
- `eez()` must return `EEZ_REGISTRY_ADDRESS`. Boot treats a target without
  code as not yet ready and retries; a target forwarding to another registry
  is reported as a configuration error and readiness stays closed.
- The registry sees the target as `msg.sender`. The supported proof profile
  always sets `bindMsgSenderInPublicInput = false`, so proofs do not change.
  If an immediate prefix ever contained meta-hook entries, the registry would
  call the target's `executeMetaCrossChainTransactions()` rather than revert
  for a codeless sender; the Composer does not produce such batches.

**Unchanged.** Every registry read (state, proxies, events, gas projection)
still uses `EEZ_REGISTRY_ADDRESS`. Final preflight executes the exact signed
poster through the target and still requires the registry's calls, logs and
final root, so a target that does not forward the batch is never dispatched.
The wrapper's extra call and root read are included in the measured gas
limit, but not in the static admission projection, which may under-reserve
by those few thousand gas.

**Compatibility.** Unset, the destination and behaviour are unchanged. Older
node images ignore the variable and post to the registry. Followers at this
revision derive posts made through a conforming target, because the L1 scan
decodes the transaction input by selector, not by destination; verify that an
older follower's scan does the same before mixing versions. Removing the
variable is a complete rollback.

### Builder relay fan-out

`EEZ_L1_BUILDER_RPC_URL` lists one or more `eth_sendBundle` relays as absolute
HTTP(S) URLs separated by commas; surrounding whitespace is ignored and a
literal comma inside a URL must be written as `%2C`. A single URL is the
previous configuration and behaves as before. Startup rejects an absent or
empty value, an empty entry (including a trailing comma), a malformed or
non-HTTP(S) entry, duplicate entries after URL normalization, and more than 16
relays. `EEZ_L1_BUILDER_RPC_URLS` is not a setting: its presence fails startup
so a misspelled list cannot be ignored. URLs may contain credentials, so
configuration errors, logs, delivery events and captures identify a relay only
by position and host (`builder[1] (relay.example)`).

Each dispatch serializes the timestamp-pinned request once (same transactions,
target block, `minTimestamp`/`maxTimestamp` and `revertingTxHashes`) and sends
the identical bytes to every relay concurrently. Each relay is bounded by the
HTTP client timeout and, with dynamic admission, the shared delivery deadline.
After the first acknowledgement, relays still pending at the latest-safe
submission boundary are abandoned instead of delaying observation. The dispatch
fails only when no relay acknowledges: a lone relay keeps its exact previous
error; if every relay answered `-32601` the error is `BundleRpcUnsupported`; if
every other failure was transport the error is a provider (transport) error;
otherwise it is a submission error. Classification uses error variants, never
error text.

A fan-out is one delivery attempt. Dispatch is recorded once before any relay is
contacted, so retry and episode accounting, the replacement bound and user
failure charges are unchanged by the relay count. Acknowledgement by one or
several relays never implies inclusion: the existing canonical observation
alone decides inclusion or exclusion at the pinned target.

The delivery event keeps its fields with unchanged single-relay meaning:
`delivery_ms` and `ack_remaining_ms` refer to the first acknowledgement (or the
end of the fan-out when none arrived) and `relay_bundle_hash` is the first
acknowledging relay's identifier. A new `builders` array records each relay's
`index`, `host`, `outcome` (`acknowledged`, `rejected`, `unsupported`,
`transport_error`, `deadline_exceeded` or `abandoned_after_acknowledgement`),
`relay_bundle_hash` and `elapsed_ms`; relay error text is not retained there.
Bundle capture metadata is version 2: `builderRpcUrl` is replaced by the same
per-relay outcomes, and replaying a version 2 capture requires an explicit relay
URL (`testing/kurtosis/scripts/replay-bundle.sh CAPTURE URL`).

Startup scripts probe every listed relay with an empty bundle
([`scripts/builder-relays.sh`](../scripts/builder-relays.sh)). At least one relay
must be reachable and serve `eth_sendBundle`; other failing relays only warn,
because one acknowledging relay suffices per dispatch. The Composer itself does
not probe relays at startup.

Compatibility: single-URL configurations need no change. The change is
Composer-only; signers, followers, protocol encoding and storage are unchanged.
Older Composers parse the whole value as one URL: depending on its spelling a
list is rejected at startup or silently becomes one wrong endpoint. Set a list
only after upgrading and return to a single URL before rolling back.
Collectors that ignore unknown fields keep reading delivery events; version 1
capture readers must handle version 2 metadata.

## L1 builder refund profile

`EEZ_FEES_L1_TO_L2_ENABLED=true` enables L1 quotes, signed admission and refund
selection together, independently of L2 collection. It requires the complete
[shared cost model](#cross-chain-fee-configuration), `EEZ_FEES_L1_REFUND_RECIPIENT`
and `EEZ_FEES_L1_REFUND_PERCENT`. Preparing those terms while the flag is absent
or false leaves collection disabled. L1 calculations use L1-denominated costs
directly; the L2 conversion assumption remains independent.

The operator must first qualify **every** configured builder relay for the same
immediate native-blob refund profile. Generic `eth_sendBundle` support or an ACK
is insufficient. The recipient must be a dedicated plain EOA; contract and
EIP-7702 delegated recipients are unsupported. This initial profile observes a
single successful 21,000-gas transfer from the canonical block beneficiary after
the selected source and before a subsequent native post. Delayed payouts, pooled payouts, multiple matching transfers,
a different payer address, public-mempool fallback and sponsorship are unsupported.
A recipient shared across unrelated revenue routes cannot establish attribution.

The L1 front returns the same compact wallet fields, with independently simulated
legacy and EIP-1559 options. It estimates omitted gas, composes nested calls,
constructs the native L2 delivery commitments, and executes a disposable native
blob post followed by the source. The known verifier executes with real gas;
only its final, exact verification boolean is privately assumed valid. Actual
posting still requires the independent signer and final signed native validation.
Ordinary L1 transactions do not use this cross-chain pricing profile; use their
normal Ethereum estimation path. Ordinary L2 settlement remains subsidized as
specified above.

Let `C` be measured posting execution plus physical blob cost and configured
proving/operations/treasury costs; `R` the configured risk amount; `P` the target
profit; `g` the charged source receipt gas; and `p` the refund percentage.
The payout allowance `H` is 21,000 times the sampled posting effective gas price.
The quote finds fee fields satisfying:

```text
required net refund = C + R + P
required gross tip = ceil((required net refund + H) * 100 / p)
priority fee per gas = ceil(required gross tip / g)
projected net refund = max(0, floor(g * effective priority fee * p / 100) - H)
```

Deducting all payout gas after applying the percentage is conservative for
providers that deduct it before the percentage. Fee-dependent execution is
replayed at the proposed fields within the same bounded quote worker. Debug
includes measured L1/source gas, DA, percentage, payout allowance, cost/risk/profit
and `projectedRefundL1`. `qualifiedRecoveryL1` remains null in a quote.

Signed admission requires the standalone `C + R`; profit is a quote target
and may absorb drift. It uses effective priority fees and charged receipt gas,
not gas limits or fee caps. Underpayment uses `data.code = "l1_tip_underfunded"`.
Admission does not reserve a quote or change transaction encoding.

Funding rejections retain `l1_tip_underfunded` / `l2_tip_underfunded` and add
`error.data.details` on `eth_sendRawTransaction`: `reason=settlement_fee`,
`scope=source`, `paymentBasis=gross_priority_payment`, `paymentChainId`,
`requiredPayment`, `offeredPayment`, `shortfall`, `sourceGasUsed`,
`sourceBaseFeePerGas`, L1 priority/execution/blob prices, and `l1Sample` /
`l2Parent` number/hash references. Amounts are native base units on the payment
chain; numeric fields are canonical hex quantities. For L1 the required gross
payment includes the refund percentage and payout-cost allowance. An execution
cap below the source base fee instead returns `source_fee_cap_below_base_fee`,
with `reason=source_base_fee`, source chain ID, cap and sampled base fee. Compact
successful quotes are unchanged; debug estimates also expose the selected
`l1PriorityFeePerGas`.

Queue status distinguishes `settlement_fee` (the same measured fields, with
`scope=candidate_prefix` for aggregate prefix funding) and `settlement_pricing`
(a calculation is unavailable) from the existing EVM `fee` reason. Prefix
offered/required amounts include previously selected sources; `sourceGasUsed`
identifies the source being evaluated, not total prefix gas. Each condition
retains the source and its nonce successors without charging an attempt. Logs
retain structured funding details, and RPC admission logs identify the signed
transaction hash so quotes and rejections can be correlated.

These diagnostic fields/reasons are additive; status consumers must allow
unknown reasons and absent details in historical records. There is no storage,
contract, signer-wire or follower-replay change. Upgrade Composer after draining
accepted work. Old runtimes keep unfiltered pricing and less specific diagnostics;
rollback can raise the required cost for outstanding lower-priced transactions.
The existing in-memory queue and stopped-database rollback boundaries apply.

L1-funded posts pack multiple complete source requests, including their nested
calls, up to the existing gas/blob/timing and configured admission bounds. They
may share a post with an independently funded outbound group under existing
FIFO/nonce ownership and canonical outbound-first execution. The L1 route owns
the extra posting cost above the standalone outbound reference; its refund is
never counted toward the L2 funding check. Gross post/refund observations still
describe the entire post and do not claim to be a net per-direction profit ledger.
Following the operator's accepted subsidy policy, the final batch need not cover
its cost: do not shrink or reject it because the largest refund is below `C + R`.
The dispatch selector chooses
one external payer by recoverable total payment, never by gas price alone; it
excludes the poster, recipient and configured excluded senders. The exact request
contains `refundPercent`, `refundRecipient`, and one `refundTxHashes` element.
That payer is required: it is removed from `revertingTxHashes`. An absent eligible
external payer is still an error; internal money cannot be labeled user revenue.
Other existing
partial-consumption and reconciliation behavior is unchanged.

The final post hash binds immutable refund terms in the in-memory cost ledger.
Canonical accounting checks the selected source, same-block payment, actual cost
floor and uniqueness. Repeated observations are idempotent; reorgs retract their
previous cost/payment reports. `eez.composer.postbatch.actual_cost` adds
`refund_status`, `refund_hash`, `refund_value`, `refund_source`, and
`refund_minimum`. A requested refund is never cash. A missing, ambiguous or
underfunded immediate payout does **not** stop quotes, admission or dispatch.
Underfunded means received revenue is below the refreshed cost/risk floor; with
subsidized batching it need not mean the builder violated its refund terms.
Duplicate payment attribution is rejected without suspending the network.
The [refund metrics](../monitoring/README.md#l1-refund-observations) expose received
revenue, posting cost, shortfall, status and observation errors for operator
follow-up. They never authorize execution or source resubmission.

There is no protocol, genesis, protobuf, DA or historical replay migration.
Upgrade Composer before using its L1 fee API; older Composers neither enforce the
L1 tariff nor request these refunds. Keep L1 collection disabled during a mixed-version
rollout, drain outstanding work, qualify the actual builder path, then enable it.
Disabling it or rolling back restores subsidized L1 admission. The financial
plans are **in memory**, like accepted requests: a restart can lose unsettled
requests and refund attribution. Monitoring totals reset at restart; retained
posting logs and canonical receipts remain the evidence for historical checks.
Node database persistence does not recover that ledger. Do not use projected or
unobserved L1 revenue as received cash. There is no on-chain payment guarantee.

The batching/observation-only increment changes no API or stored encoding.
Drain accepted work before replacing Composer. An older Composer restores the
one-source restriction, final funding floor and automatic pause after a bad
refund; upgrading the signer/follower is not required for this policy change.
