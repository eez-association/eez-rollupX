# Configurable settlement policy

Implemented and validated on Ethereum and native Gnosis profiles, including
Kurtosis and the [live Chiado deployment](audits/2026-09-13-chiado-settlement-policy/README.md). The [proposal](proposals/configurable-settlement-policy.md)
defines the requirements; the
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
| `EEZ_MAX_SPECULATIVE_DEPTH` | Existing default 64; Compose profiles default to 0 | Must be 0 (unbounded), or at least `N + K - 1`, using the existing timing grid's `K`. |
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
| `EEZ_L1_CHAIN_CONFIG` (signer and follower) | Required for signer; optional for follower | `mainnet`, `chiado`, or a reviewed Ethereum-format genesis path. Every proof request supplies a matching L1 chain ID and exact target timestamp. Historical followers use the containing block's fork. |

Execution, blob, and gas margins use checked
`ceil(x * (10000 + margin) / 10000)` arithmetic. Fee ceilings are per-unit
prices, not currency budgets or maximum transaction costs. A
required price above its ceiling keeps work pending as `fee_cap_blocked`.

The gas-margin default is now 500 basis points (5%). Explicit values, including
zero and the former 2000 default, override it; empty or malformed values are
errors. Compose forwards the operator value to the node and leaves an absent
setting to the runtime. Shipped Kurtosis profiles explicitly use 500, and its
Starlark transport preserves supplied strings for runtime validation. Shell
environment takes precedence over `.env`. Signers
do not consume this operator margin. For mixed versions, explicitly set 500 to
keep older Composers consistent. This changes future gas planning/signing only;
no wire, database or follower-replay migration is required. Rollback restores
the older default if the setting is absent, so preserve explicit operator values.

Requests without L1 posting context are rejected.
The batch, DA and signature formats remain unchanged.

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
stage limit. Preparation can
run beyond its allocation and continue proving the same accepted candidate.
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
the largest ratio across those three dimensions. From `strict-fifo-7`, a sample's
scaling term is superseded only by an observation that took at least as long and
supplies at least as much of every requested dimension, strictly more of one.
Observed counts are clipped to the request only for this comparison: excess
history in an old sparse sample cannot prevent learning from a wider measured
batch with sufficient history. Incomparable observations, including greater
witness/blob coverage or a more expensive sparse proof, retain their scaling
terms. This avoids multiplying fixed overhead from a one-group observation after
larger proofs have been measured. All observations still contribute to the
slowest measured proof floor. Scaling uses a denominator of one for zero observed
counts. The prediction is capped at the greater of the configured proof seed and
the slowest observed proof with headroom, with a 25 ms floor. A larger request never scales
below an already observed slower proof. With no proof
observations the configured seed applies. This is a scheduling estimate, not a
proof completion guarantee; the shared deadline remains authoritative. Each proof
request records the estimate and its `configured_seed`, `covering_sample` or
`scaled_sample` basis. Finalization only uses covering observations and reserves
at least 150 ms, including 25 ms for delivery. These samples are in
memory and are discarded on restart; they are not durable queue state.

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
`strict-fifo-7` adds the request-relative proof scaling described above;
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

Estimator `candidate-framing-9` replaces the fixed 600,000 admission reserve
with a bound on each candidate's calldata. It counts the native single-rollup
ECDSA envelope/anchor, up to 128 proof bytes, and the target fork/operator's
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
Estimator `candidate-framing-10` checks both inbound possibilities before a
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
uses the existing gas fields. Upgrading or downgrading Composer changes future
admission and in-memory retry behavior only; mixed-version signers/followers
retain the same protocol contract and historical replay. Preserve monitoring
history and checkpoints on rollback. A restart still requires clients to
reconcile and potentially resubmit unsettled work.

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

The 160,000 base already includes the leading anchor. Queue replacement now
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
`submission_backoff`, `pipeline_deadline_backoff`, `preparation_probe_limit`,
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
p90 of p90 described below). On Ethereum mainnet, a 7-day replay
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
and minimal settlement blocks. Historical replacement preserves the existing
header rather than substituting the current setting. This changes newly built
checkpoint hashes for a nonzero recipient, not the block/DA format: existing
history remains replayable using its recorded environment. No protocol,
database, or signer migration is needed. Followers retain upstream external
reinsertion policy; they do not sequence displaced ordinary work. The adapter
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
limits at the intended target timestamp. The signer independently verifies that
context against its configured L1 profile; followers resolve historical limits
at each containing L1 block. The Chiado and Kurtosis Compose profiles pass the
corresponding L1 profile to the signer.

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
optional [shadow pricing policy](#shadow-l2-fee-pricing), realization,
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

## Shadow L2 fee pricing

`EEZ_L2_FEE_SHADOW_POLICY` optionally prices the already prepared candidate using
explicit operator assumptions. It never changes transaction fees, admission,
dispatch, execution, queue ownership or financial reservations. An absent value
keeps existing unpriced accounting. Empty or invalid values fail startup; there
are no implicit monetary defaults. Read once by `ComposerSettings`, with process
environment before `.env` before `deployments.env`; restart is required to
change it. There is no CLI override. Chiado and external Kurtosis Compose pass
it only to the Composer. Kurtosis `eez.settlement_env` accepts its JSON string
and `EEZ_L2_FEE_RECIPIENT`; neither reaches the proof signer or follower.

The value is one JSON object, at most 8192 bytes, with all the following fields
required and unknown fields rejected:

| Field | Type and meaning |
| --- | --- |
| `version` | Integer `1`. |
| `l1_chain_id`, `l2_chain_id`, `rollup_id` | Positive u64 integers matching the actual Composer wiring. |
| `beneficiary` | Nonzero 20-byte address equal to `EEZ_L2_FEE_RECIPIENT`; rechecked against the measured block. Configuring it asserts expected control but does not verify possession of a key. |
| `valid_until_l1_timestamp` | Positive u64 Unix seconds; the exact candidate target must be at or before this timestamp. Expiry makes pricing unavailable, without blocking execution. |
| `l1_per_l2_numerator`, `l1_per_l2_denominator` | Positive U256 decimal strings: conservative L1 native base units realizable per L2 native base unit, after proportional haircuts. No inferred parity or live price lookup. |
| `proving_cost_l1`, `operations_cost_l1`, `treasury_cost_l1` | Nonnegative U256 decimal strings charged once per complete post. Explicit zero is allowed. Treasury cost includes the allocated fixed cost of realizing/sweeping L2 income. These are configured allowances, not measured compute expenses. |
| `risk_bps`, `profit_bps` | Integers from 0 to 10000, each independently applied to posting plus the three fixed costs. |
| `excluded_senders` | Array of at most 64 distinct 20-byte operator addresses. The current L1 poster and beneficiary are always excluded too. An empty list is explicit; senders outside it are only assumed external. |

Amounts always carry their chain's base-unit denomination; token symbols do not
define a conversion. The policy identifier is Keccak-256 of the parsed object's
JSON serialization, and the complete assumptions accompany each comparison.
It identifies assumptions, not an authorization or a treasury price certificate.

For example, the following **synthetic fixture** illustrates JSON quoting and
explicit units, not production prices or an instruction to enable this policy:

```bash
export EEZ_L2_FEE_SHADOW_POLICY='{"version":1,"l1_chain_id":1337,"l2_chain_id":1,"rollup_id":1,"beneficiary":"0x4444444444444444444444444444444444444444","valid_until_l1_timestamp":2000000000,"l1_per_l2_numerator":"2","l1_per_l2_denominator":"3","proving_cost_l1":"7","operations_cost_l1":"5","treasury_cost_l1":"3","risk_bps":1000,"profit_bps":500,"excluded_senders":[]}'
```

Let `P` be final native L1 post execution plus physical blob cost, `F` the sum of
the three fixed costs, and `n/d` the configured conversion:

```text
subtotal_l1 = P + F
required_l1 = subtotal_l1 + ceil(subtotal_l1 × risk_bps / 10000)
                         + ceil(subtotal_l1 × profit_bps / 10000)
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

## Unsigned L2 fee quotes

When `EEZ_L2_FEE_SHADOW_POLICY` is configured, the L2 Composer front accepts
`eez_quoteL2ToL1Fee` with JSON-RPC positional params `[intent]`, and retains
`POST /eez/fees/l2-to-l1` with the intent as its body. Both use the same quote
engine and worker budget. They return advisory ordinary legacy or type-2 wallet
fields before signing. A quote does not enqueue work, reserve
funds, request a proof, sign, submit, or change admission rules. The L1 front
does not serve quotes. The service waits for the existing startup readiness
gate. Missing policy returns `404 quotes_disabled`; there is no default tariff.

The intent is exactly this object, with JSON-RPC hex quantities and explicit
fields (addresses and calldata below are illustrative):

```json
{
  "type": "0x2",
  "chainId": "0x1",
  "from": "0x1111111111111111111111111111111111111111",
  "to": "0x2222222222222222222222222222222222222222",
  "nonce": "0x0",
  "gas": "0x124f80",
  "value": "0x0",
  "data": "0x1234"
}
```

All fields are mandatory; unknown fields, supplied fees, creation, access lists,
authorizations, blob sources and reserved senders are rejected. The source
type is `0x0` for an EIP-155-protected legacy transaction or `0x2` for EIP-1559;
the service preserves that type in execution, DA sizing and returned fields.
Replay-unprotected signed legacy transactions are rejected. The source
must use the node's L2 chain ID and produce a supported outbound settlement
group. Values remain subject to normal source balance checks and the pinned
L1 rollup escrow. Quotes do not fund either account. The application supplies
a gas limit between 21,000 and 16,777,216 (the active EIP-7825 transaction cap)
and must keep it unchanged when signing. This bound applies to both quotes and
signed tariff admission. Upgrade Composer and every SDK copy together: older
versions reject limits above 5,000,000 even when native execution permits them.
This endpoint does not solve gas-limit discovery for arbitrary cross-chain calls.

The backend selects an exact L1 parent/next-block target and its normal posting
fee sample. Its validated settlement scan must have reached that same L1 parent.
It reads an L2 parent and validated settlement endpoint, pins the
simulation environments and cold state reads, and uses private overlay channels.
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
its 20% risk allowance once.

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
Intermediate native history requires reconciliation and causes an unavailable
quote. The quote charges this single intent the complete estimated post and
retained history; it assumes no future batch participants.

The [shadow pricing arithmetic](#shadow-l2-fee-pricing) adds service costs,
risk/profit and conversion. Starting at a zero priority fee, the backend raises
the tip to `ceil(required_L2 / native_source_gas)`, then repeats composition,
native replay and sizing with those actual fee fields. It returns only when the
replayed gross tip covers the estimate. At most four simulations are allowed;
execution depending on fees may fail or not converge. `maxFeePerGas` is the
draft base fee plus this tip, without future base-fee headroom. Legacy quotes
return that sum as `gasPrice`, with no EIP-1559 fee fields. On execution, only
`gasUsed * (gasPrice - actualBaseFee)` counts as legacy priority revenue; the
burned base fee is excluded. A base-fee increase can invalidate coverage.

Successful JSON responses contain:

| Field | Meaning |
| --- | --- |
| `version: 1`, `mode: advisory` | Application response schema and guarantee level |
| `intent`, `intentHash` | Normalized eight request fields; Keccak-256 of their compact, lexicographically key-sorted JSON |
| `transaction` | The same intent plus `gasPrice` for legacy, or `maxPriorityFeePerGas` and `maxFeePerGas` for type 2; all wallet quantities in hex |
| `issuedAt`, `expiresAt` | Unix seconds; expires at the earlier of ten seconds from issuance and the selected L1 target timestamp |
| `context` | Chain/rollup IDs, L1 parent and target, L2 parent and draft timestamp, settled L2 endpoint |
| `policy` | Policy ID and all conversion, cost, exclusion and validity assumptions |
| `estimate` | Source/native L2 gas, measured conditional L1/target gas, DA bytes/blobs, distinct L1 execution/blob/subtotal/risk/profit amounts, required L1/L2 amounts and gross L2 tip |

Monetary amounts in `estimate` are decimal strings. Gas/counts/context heights
are integers. `estimate.basis` is
`isolated_native_l2_and_l1_settlement`, with `estimate.proof` set to
`metered_assumed_valid`; `qualifiedRecoveryL1` is null.
L1 figures are estimates, not receipts. Neither the intent hash nor response
authenticates a quote or guarantees inclusion. Applications must trust their
chosen endpoint and reviewed policy, preserve the original intent, and impose
their own maximum fee.

The SDK's wallet preflight compares chain ID, pending nonce and pending base fee
by numeric value. Wallet middleware may return non-negative safe JavaScript
integers or zero-padded hex strings; these observations are accepted without
changing the quoted transaction. Hex observations are bounded to 256 bits;
unsafe numbers and malformed or missing values fail before submission with the
affected field identified. Intent and quote quantities remain canonical hex.

Work is limited to one request per node, with no queue; additional requests get
`429 quote_busy`. The whole request has a ten-second deadline. Input is limited
to 32 KiB, calldata to 12 KiB, retained history to 256 blocks, encoded DA to 2 MiB,
cross-chain target frames to 32 across all iterations, and nesting depth to eight.
Active network/operator gas and blob limits and the negotiated emission window
still apply. Expensive work uses Reth's blocking executor. Disconnect, timeout
or shutdown sets cancellation; checks between frames/history/replay steps stop
further work. A running EVM frame cannot be interrupted, and retains the sole
worker permit until it exits. Frames also have existing EVM gas limits.

Malformed intent returns `400 invalid_intent`; unsupported execution or exhausted
work bounds return `422 unsupported_intent` / `quote_limit`; a changed/expired
context returns `409 stale_context`; missing state, reconciliation, escrow or
pricing returns `503 quote_unavailable`; HTTP timeout returns `504 quote_timeout`.
Errors have `{ "error": { "code": "...", "message": "..." } }`, except the
front's existing plain-text 413 body-limit response. For JSON-RPC, malformed
params/intents use `-32602`; other quote failures use `-32004` with the same
stable error object in `error.data`. Successful `result` contains the unchanged
quote response. The RPC method is absent (`-32601`) from the L1 front and regular
node/follower RPCs. A policy-disabled L2 front reports `quotes_disabled`.
JSON-RPC IDs, mixed batches and notifications follow the front's existing
semantics; batch members execute sequentially under the shared worker bound.
The 32 KiB intent bound also applies through JSON-RPC, alongside the front's
overall request-body limit. Requote after a retryable
context/availability change; never silently submit with a default fee.

The dependency-free [JavaScript helper](../sdk/l2-fees.mjs) fetches and validates
the response and returns ordinary wallet fields. `maxTotalFee` is a mandatory
application ceiling in L2 base units, covering `gas * maxFeePerGas` or legacy
`gas * gasPrice` (excluding
the intent's separately specified value). `expectedPolicyId` optionally pins
the reviewed policy. `sendQuotedTransaction` checks chain, authorized account,
pending nonce/base fee and expiry before calling `eth_sendTransaction` once. It requests
no second signature, permit or payment. The wallet's broadcast RPC must already
be the EEZ route; selecting a quote transport does not change it. Supply either
a direct Composer `rpc(method, params)` function or the existing HTTP quote URL
to `requestL2FeeQuote`; not both. Injected wallets need not forward custom RPCs.

```js
import { requestL2FeeQuote, sendQuotedTransaction } from './sdk/l2-fees.mjs';
const args = { intent, maxTotalFee: 10n ** 17n }; // App-selected L2 fee ceiling.
const quote = await requestL2FeeQuote({ ...args, rpc: composerRpc });
const hash = await sendQuotedTransaction({ ...args, quote, wallet });
```

Changing the gas limit, nonce, calldata, value or fee fields invalidates the
estimate. Later state/base-fee changes, wallet edits and time spent approving
can invalidate it too. The quote supplies no inclusion deadline guarantee,
liquid L1 reserve, treasury realization or durable financial
ledger. Final native bundle execution remains mandatory for settlement.
The SDK checks expiry before opening the wallet, not after a signature already
broadcast by it. Its JSON numeric chain/context validation supports safe
JavaScript integers only.

The RPC method and legacy profile are additive application APIs: standard
wallet RPCs, protocol commitments, genesis,
historical decoding and follower replay are unchanged. Deploy a compatible
Composer/front and configure its reviewed policy before enabling the app helper;
an older front lacks the custom quote method and legacy profile, and a
policy-disabled front cannot provide quotes. Upgrade the node before clients
select the new method or legacy type; roll those clients back before restoring
an older node. Existing type-2 HTTP clients continue working. Rollback disables quote
availability; existing signed/accepted transactions retain their normal behavior.
Applications must stop the quote flow on missing/unsupported responses. No
database migration is required. SDK checks run with `node sdk/l2-fees.test.mjs`;
the source-based process regression is the `fee_quotes::` module.

The measured-settlement correction changes `estimate.basis` while retaining
response version 1 and wallet transaction fields. Upgrade the SDK/dashboard
first: it accepts both the previous projection basis and the new measured basis.
Then upgrade Composer, including signed ingress and cumulative prefix checks.
An older SDK rejects the new basis; an older Composer can reject the lower fees
quoted by the new implementation. Drain accepted requests before handover and
requote after a rollback. Protocol, signing RPC, genesis, historical decoding and
follower replay are unchanged; there is no persistent-data migration.

### Opt-in L2 tariff admission

`EEZ_L2_FEE_ENFORCE_TARIFF=true` requires a complete, runtime-matching
`EEZ_L2_FEE_SHADOW_POLICY`. Absent or `false` preserves existing admission;
empty and other values fail startup. This is a testnet tariff, separate from
the production strict-funding design's durable reserves and treasury realization.
Chiado, Ethereum and Kurtosis configuration pass it to the Composer only.

For a newly submitted outbound source, the L2 front recovers the signed legacy or type-2
transaction and replays its exact fees using the quote engine's bounded isolated
state. Its effective priority payment must cover the whole measured settlement
tariff. The comparison uses native receipt gas, never the user's gas limit, and
accounts for clipping by `maxFeePerGas - baseFee`, or the legacy difference
`gasPrice - baseFee`. Unsupported quote profiles
are rejected before queue admission. A tariff failure returns JSON-RPC code
`-32003` with `data.code = "l2_tip_underfunded"`; availability and profile errors
retain the quote engine's stable `data.code`. Rejection neither consumes a nonce
nor acquires queue ownership. Already-owned identical transactions remain
idempotent. Ordinary L2 and L1-origin admission retain their existing rules.

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
batches. While this L2-only tariff is enabled, L1-origin users are settled in
separate batches under the existing FIFO ordering, so unfunded L1 work cannot
invalidate a paid outbound batch. Nested calls within one source remain whole.
Quoting a future nonce whose
predecessor has not executed is outside the isolated-intent profile.

Immediately before dispatch, an attempt containing outbound users must also
cover its complete newly priced L1 post and blob tariff using actual eligible
L2 native receipts. Poster, beneficiary and explicitly excluded senders cannot
finance this check. Failure defers the unsent attempt without charging a user
retry or declaring canonical completion. Accepted work remains subject to
state, price and availability changes; this is not an inclusion guarantee.

The dashboard enables the fee flow with `EEZ_UI_L2_FEES_ENABLED=true` and caps
the wallet's maximum execution fee with `EEZ_UI_L2_FEE_MAX_TOTAL_WEI` (default
`100000000000000000` L2 wei). It quotes before the single ordinary wallet
signature and preserves explicit fees for injected wallets and the disposable
local test signer. Its `/composer/l2/eez/fees/l2-to-l1` proxy must reach the
Composer's application endpoint for existing clients. Updated dashboards call
`eez_quoteL2ToL1Fee` directly on the L2 Composer RPC and default to legacy quotes
when the selected provider identifies itself as Rabby, whose custom-network
path uses `gasPrice`. Other providers default to type 2; an explicit supported
transaction type is preserved. Quote failures stop submission rather than
falling back to a generic wallet tip.

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
Version 7 changes only Composer's in-memory proof timing estimate. It retains
version 6's FIFO behavior, existing event fields and all actual deadline gates.
Rollback restores the older extrapolation; no queue, database or wire migration
is required, and older collectors continue to accept the version string.
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
