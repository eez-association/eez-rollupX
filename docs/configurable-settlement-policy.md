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
| `EEZ_L1_POSTBATCH_GAS_MARGIN_BPS` | 2000 | Nonnegative integer margin on pre-refund execution gas (at least the calldata floor). The final ordered bundle is checked again at the selected gas limit. |
| `EEZ_L1_CHAIN_CONFIG` (signer and follower) | Required for signer; optional for follower | `mainnet`, `chiado`, or a reviewed Ethereum-format genesis path. Every proof request supplies a matching L1 chain ID and exact target timestamp. Historical followers use the containing block's fork. |

Execution, blob, and gas margins use checked
`ceil(x * (10000 + margin) / 10000)` arithmetic. Fee ceilings are per-unit
prices, not currency budgets or maximum transaction costs. A
required price above its ceiling keeps work pending as `fee_cap_blocked`.

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
of one), capped at the configured proof seed, with a 25 ms floor. With no proof
observations the configured seed applies. This is a scheduling estimate, not a
proof completion guarantee; the shared deadline remains authoritative. Each proof
request records the estimate and its `configured_seed`, `covering_sample` or
`scaled_sample` basis. Finalization only uses covering observations and reserves
at least 150 ms, including 25 ms for delivery. These samples are in
memory and are discarded on restart; they are not durable queue state.

A nominal preparation overrun or infrastructure error does not halve selection.
A full proving-window failure can reduce the next selection using admitted work,
and a resource cut that bypasses older eligible FIFO work retains the fairness
backoff. A successful FIFO prefix removes a redundant count cap. If a stale
estimate blocks every group, the next opportunity makes a bounded probe under
all resource/deadline checks. Canonically observed, charged exclusions or partial
consumption in produced blocks retain submission backoff. A strict timestamp gap
between consecutive canonical headers proves the old target was never produced;
that target cannot train capacity even if some of its sources completed in a
replacement. Source dispositions and replacement ownership still reconcile
independently. `admission.skipped_slot` records this distinction.

The bounded deferred-entry gas model can discount six fixed storage words that
are already nonzero in the exact canonical parent. It discounts 15,000 gas per
observed word; dynamic result storage, unknown shapes and speculative writes earn
no such discount. The existing shape and 128-entry limits remain. Final full-ABI
projection and exact signed-bundle native execution still validate gas and every
intended effect. This is not a receipt-ratio discount.

The retained `eez.composer.admission.v1` event family records each candidate,
source gas decision, summary (including no-cut batches), proof/preflight/delivery
timing and final signed hash. See [admission monitoring](../monitoring/indexer/README.md#admission-history)
for estimate/receipt joins, missing-event reporting and CSV export. Version
`shared-deadline-1` identifies the scheduling behavior. Deploy the additive
collector/schema/dashboard before the new Composer; old collectors ignore the
new family and old histories retain null estimates. Rollback can leave those
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

At protocol `9d13218`, bounded simple deferred entries reserve gas according
to their return-data storage: 230,000 for 0–31 bytes, 250,000 for 32 bytes,
and 270,000 for 33–64 bytes, provided the corresponding retained queue mapping
slot is empty or contains only one root update, no nested/callback rows, and
at most 64 result bytes. The Composer reads every potentially discounted slot
from the canonical L1 parent, including inactive slots beyond the current
queue length. An unknown, oversized, or unreadable slot keeps the 370,000
reserve. One successful simple call through a proxy registered
at the canonical parent reserves 70,000, plus its measured L1 target execution.
New proxies and complex,
nested, reverting or larger entries retain the existing 370,000 reserve.
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
