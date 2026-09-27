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
| `EEZ_PREPARATION_TIME_MS` | Unset | Optional positive integer milliseconds. Planning allocation used with proof and slack for the exact millisecond trigger; cross-chain preparation and proving share the final submission deadline. The combined budget must fit `(K - 1)` L2 blocks. Unset retains the rounded trigger. |
| `EEZ_MAX_SPECULATIVE_DEPTH` | Existing default 64; Compose profiles default to 0 | Must be 0 (unbounded), or at least `N + K - 1`, using the existing timing grid's `K`. |
| `EEZ_MAX_BLOCKS_PER_BATCH` | Removed | Must be absent. The payload ceiling and negotiated signer limit bound a post. |
| `EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS` | Required | Positive integer execution-price ceiling in native base units per gas. |
| `EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS` | Required | Positive integer blob-price ceiling in native base units per blob gas. |
| `EEZ_L1_POSTBATCH_PRIORITY_FEE` | Fee-history estimator | Optional nonnegative exact override within configured bounds. When no separate priority maximum is supplied, this fixed value is also its safe maximum. |
| `EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE` | 0 | Nonnegative integer; include any actual relay minimum. |
| `EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE` | Required with fee-history estimation; fixed override otherwise | Independent nonnegative priority-price ceiling. Must be at least the minimum and no greater than the execution-price ceiling. It never inherits the execution-price ceiling. |
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

For exact-target cross-chain candidates, the exclusive deadline is target time
minus submission slack. `EEZ_PREPARATION_TIME_MS` and `EEZ_PROOF_TIME_MS` control
scheduling estimates; they are not separate hard stage limits. Preparation can
run beyond its allocation and continue proving the same accepted candidate.
Proving uses the remaining deadline less finalization/delivery reserve; retries,
streaming and cache repair cannot reset that window. A fast preparation can
leave more time for a slow proof. A true window failure keeps source ownership
and produces an empty local Sync when required, without an unproved or substitute
empty L1 posting. Late scheduler entry uses this same cross-chain policy.
Ordinary-only exact-target posting and `NextBlock` keep their prior timing rules.

Preparation learns consistently measured group work and finishing/KZG cost.
Only complete measured groups train the finishing relationship; failed appends,
rebuilds and source-state waits retain conservative estimates. The calibrated
finishing reserve includes 10% of the available window for variation. A rejected
next group does not invalidate the accepted prefix's clean sample. Proof and
finalization retain at most 32 observations per rollup and stage, with 25%
headroom. Proof samples covering the requested group, physical blob and witness
block counts are used directly. Otherwise, observed durations scale upward by
the largest ratio across those three dimensions (zero counts use a denominator
of one), capped at the greater of the configured proof seed and the slowest
observed proof with headroom, with a 25 ms floor. A larger request never scales
below an already observed slower proof. With no proof
observations the configured seed applies. This is a scheduling estimate, not a
proof completion guarantee; the shared deadline remains authoritative. Each proof
request records the estimate and its `configured_seed`, `covering_sample` or
`scaled_sample` basis. Finalization only uses covering observations and reserves
at least 150 ms, including 25 ms for delivery. These samples are in
memory and are discarded on restart; they are not durable queue state.

A nominal preparation overrun or infrastructure error does not halve selection.
A full proving-window failure can reduce the next selection using admitted work,
while a resource cut does not impose a FIFO count backoff. FIFO remains a
preference within the canonical outbound-before-inbound phases and per-source
nonce chains. A nonfitting gas/blob group is retained with its nonce successors;
discovery continues through independent groups in the finite snapshot while the
shared pipeline deadline permits it. Earlier capacity deferrals also remain owned
if a later provider failure aborts the candidate. If a stale
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
failures per source, allowing a smaller retry after the fifth exclusion. Numeric
admission retains its three-failure limit. Prover/immediate relay rejection also
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
`confirmed-failure-backoff-4` identifies five-failure submission tolerance; it
retains the capacity minimum checks of `packing-minimum-check-3` and the FIFO
preference of version 2. Historical `shared-deadline-1` events retain their former
FIFO count semantics. Version 3 checks a minimum contribution before source
execution: 70,000 per outbound entry or 65,000 per inbound entry, zero calldata/
target gas and zero new companion consumption. It uses the same budget arithmetic,
including discount revocation, previously accepted companions and the new inbound
source's signed start requirement. L2 checks retain the existing declared/system
gas requirement. These are lower bounds on the admission model, not measurements
of actual execution. A miss retains the source without charging an attempt and
blocks its nonce dependants, while independent sources remain eligible. Empty
candidates still run normal simulation/classification. Cheap checks do not train
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
retry or ownership rules. `fifo_fairness_limit` is historical only; new candidates
record `fifo_bypassed` as a diagnostic without restricting selection. The summary's
`capacity_deferrals` counts each individually rejected group by reason, even when
other work fits later or a deadline ends discovery. Each rejected source retains
its own outcome reason. New source records additionally
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

Dynamic mode requests the gas-weighted p90 reward for each of the latest 20
canonical blocks with `eth_feeHistory`, selects the p90 of those 20 samples,
and clamps it to an optional relay floor and an independent priority ceiling.
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

One poster transaction among a 20-block window cannot raise the outer p90, so
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

Ethereum and Chiado retain their existing slot and submission-window settings.

An optional `EEZ_PREPARATION_TIME_MS` replaces the implicit rounding remainder
with a measured preparation allowance. The trigger occurs at the next L1 slot
minus preparation, proof and submission slack. Block timestamps and the Sync
grid remain unchanged; the Future suffix covers the combined budget. For
example, a 5-second/1-second profile with preparation 500 ms, proof 750 ms and
slack 700 ms triggers at 3.05 seconds after its anchor. This is the packaged
Chiado preset, not a latency guarantee. Size proof and preparation for large
historical batches as well as small bridges. The setting requires a node restart;
the applicable proof policy and absolute submission deadline remain enforced.
If the expected L1 block is still absent at the following preparation deadline,
the scheduler permits one replacement opportunity on the same parent, with the
same target execution height and a timestamp one slot later. The Composer
rebuilds its speculative suffix and retains the old submission as history;
canonical replay resolves late original inclusion. This does not classify the
slot as skipped or charge user retries. No new timer setting is introduced.
One additional opportunity per observed L1 head bounds work during an outage;
a second consecutive missing slot waits for a new head. See
[settlement ownership](../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).
On the 5-second/1-second Chiado grid, this preset reserves two Future blocks
per interval rather than one with the former 500 ms proof budget.

After catch-up, the sequencer rechecks the terminal. Only a terminal exactly at
the requested future height and timestamp, with deadline headroom and no
speculative-cap pause, may use steady composition in that same opportunity.
Intermediate, capped, past and misaligned boundaries remain empty. This avoids
wasting a usable future slot after rebuilding a missed interval.

Policy-driven historical batches select the earliest future L1 slot that leaves
the measured preparation reserve, configured proof-time budget and submission
slack. The reserve is the largest of the last four anchor-only preparations for
that rollup, plus 10% for variation. New slow measurements increase it immediately;
four fresh measurements replace an old peak, so a transient slowdown does not
force later batches to skip an otherwise usable slot for the process lifetime.
Timely reconciliation can therefore lead to posts in consecutive L1 blocks;
later preparation selects a later slot. Builder lead controls when submission
opens, not a mandatory block gap. Selection is logged as
`eez.submitter.policy.target_selected`, including the
observed head, timing budget, and chosen offset. Final timestamp, canonical-head,
fee, and deadline checks still apply, and missed opportunities preserve pending
work. No new timing setting is required. Once an exact target has been selected,
capacity splitting into historical prefixes preserves that execution height and
timestamp for every candidate and its submission. In particular, a skipped-slot
replacement keeps the same execution height and advances only the timestamp.
Only an unpinned catch-up opportunity selects a fresh target in the historical
helper; expired proof/submission deadlines still defer the candidate.

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

Restart discards the policy's in-memory intent and cached summaries. Canonical
anchors and pending activity are rebuilt under the new configuration. Restart
time never becomes a settlement timestamp. Existing nonce and source-outcome
reconciliation remain authoritative.

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
hash nor an acknowledgement proves inclusion. Historical readers use null for
missing fields. No RPC, storage or protocol migration is needed; upgrade the
Composer and dashboard independently. Rolling back removes the discount and new
observations without changing historical decoding or follower replay.
