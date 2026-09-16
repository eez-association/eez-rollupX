# Configurable settlement policy

Implemented and validated on Ethereum and native Gnosis profiles, including
Kurtosis and the [live Chiado deployment](audits/2026-09-13-chiado-settlement-policy/README.md). The [proposal](proposals/configurable-settlement-policy.md)
defines the requirements; the
[coverage ledger](proposals/configurable-settlement-policy-implementation.md)
records implementation, tests, evidence, and deployment limitations.

The policy changes when a batch is posted. L2 execution, the Sync grid, ECDSA
validation, canonical derivation, and transaction ownership keep their existing
roles. Only validated L1 commitment advancement resets the settlement clock.

When enabled, a validated cross-chain group is due at the next eligible L1
opportunity. An empty Sync is not cross-chain activity. Included ordinary L2
transactions, including reverted transactions, use the selected pure-L2 rule.
The general block threshold consolidates empty history too. All due rules are
combined into one candidate. A pending drain has a finite endpoint; empty ticks
do not extend it indefinitely.

## Configuration

Settings are read once before node launch. Existing environment/config-file
precedence is unchanged. Invalid values fail startup. An omitted value differs
from an empty string. Restart is required to change settings.

| Setting | Default | Enabled-policy meaning / validation |
| --- | --- | --- |
| `EEZ_SETTLEMENT_POLICY_ENABLED` | `false` | Opt in with `true`; disabled mode preserves legacy behavior. Policy-only settings supplied while disabled are errors. |
| `EEZ_SETTLEMENT_PURE_L2_MODE` | Required when enabled | Exactly `always` or `interval`. |
| `EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS` | Required for `interval`; absent for `always` | Positive integer milliseconds. A value supplied with `always` is validated but inactive. Measured from the containing L1 timestamp of the latest canonical commitment. |
| `EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS` | Required when enabled | Positive integer `N`, a posting trigger rather than a packing limit. |
| `EEZ_SETTLEMENT_BLOB_FULLNESS_BPS` | Off | `off`, or an integer from 1 through 10000. Soft threshold over meaningful unpadded bytes across **all** permitted blobs. |
| `EEZ_MAX_BLOBS_PER_BATCH` | No operator cap | Optional positive integer, additionally bounded by active network transaction/block and transport limits. Applies only to outgoing posts. |
| `EEZ_MAX_POSTBATCH_GAS` | No operator cap when enabled | Optional positive integer; actual budget also respects the target fork's transaction rule and validated block budget. Legacy default remains 16,777,216 with its existing validation. |
| `EEZ_MAX_USER_TXS_PER_BUNDLE` | 50; Chiado Compose default 12 | Positive integer for the existing count limit, or `auto` with the policy enabled for adaptive resource/time admission. Parsed once; restart to change. |
| `EEZ_PREPARATION_TIME_MS` | Unset | Optional positive integer milliseconds. Reserve preparation independently of proof and relay slack, with an exact millisecond trigger. The combined budget must fit `(K - 1)` L2 blocks. Unset retains the previous rounded trigger. Operational timing setting, independent of policy enablement. |
| `EEZ_MAX_SPECULATIVE_DEPTH` | Existing default 64; Compose profiles default to 0 | Enabled policy requires 0 (unbounded), or at least `N + K - 1`, using the existing timing grid's `K`. |
| `EEZ_MAX_BLOCKS_PER_BATCH` | Legacy default 300 | Must be absent when enabled. No automatic 300-block split. |
| `EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS` | Required when enabled | Positive integer execution-price ceiling in native base units per gas. |
| `EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS` | Required when enabled | Positive integer blob-price ceiling in native base units per blob gas. |
| `EEZ_L1_POSTBATCH_PRIORITY_FEE` | Network estimator when enabled | Optional nonnegative integer override within configured bounds. Legacy mode retains its prior default. |
| `EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE` | 0 | Nonnegative integer; include any actual relay minimum. |
| `EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE` | Execution-price ceiling | Must be at least the minimum and no greater than the execution-price ceiling. |
| `EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS` | 0 | Nonnegative integer basis points added to the network-estimated priority fee, rounded up before applying minimum/maximum bounds. Fixed overrides remain exact. |
| `EEZ_L1_POSTBATCH_EXECUTION_HEADROOM_BPS` | 10000 | Nonnegative integer margin added to sampled execution base fee before adding priority fee; bounded by the ceiling. |
| `EEZ_L1_POSTBATCH_BLOB_HEADROOM_BPS` | 10000 | Nonnegative integer margin added to the active network's blob base fee; bounded by the ceiling. |
| `EEZ_L1_POSTBATCH_GAS_MARGIN_BPS` | 2000 | Nonnegative integer execution-gas margin. The final ordered bundle is checked again at the selected gas limit. |
| `EEZ_L1_CHAIN_CONFIG` (signer and follower) | Required for signer; optional for follower | `mainnet`, `chiado`, or a reviewed Ethereum-format genesis path. Every proof request supplies a matching L1 chain ID and exact target timestamp. Historical followers use the containing block's fork. |

Margins use checked `ceil(x * (10000 + margin) / 10000)` arithmetic. Fee ceilings
are per-unit prices, not currency budgets or maximum transaction costs. A
required price above its ceiling keeps work pending as `fee_cap_blocked`.

Upgrade composer and signer together: requests without L1 posting context are
rejected, including when the configurable scheduling policy is disabled.
The batch, DA and signature formats remain unchanged.

Signed execution fee caps are checked against the pinned L1 target and L2 draft
base fees before allocating the finite selection window. An underpriced nonce
and its successors stay queued; earlier nonces and independent source chains
remain eligible. Fees are reevaluated each opportunity. Balance and nonce checks,
full simulation, and final bundle validation still apply.

With `EEZ_MAX_USER_TXS_PER_BUNDLE=auto`, the composer selects a finite FIFO
prefix using preparation measurements from previous attempts. It starts with
one group after restart. Once a sample exists, it considers the queued snapshot
without an artificial doubling ramp and stops incrementally as measured time,
gas or DA permits. It computes and caches each effect-prefix commitment during
admission, charging that repeated execution to elapsed time, and reserves time
for final block construction, encoding, KZG and witnesses before the existing
proof cutoff. KZG is measured separately and scales with physical blobs. The
composer adds 10% of the actual preparation window to calibrated finishing
predictions for variation and the handoff to proving. Without a calibrated
relationship, it takes the larger of a full admission pass and the observed
non-KZG finishing cost plus that allowance, then adds KZG. This preserves the
conservative fallback without counting its headroom twice after a small post.
Group-start and pre-append checks both use that reserve.
The proof-time budget and absolute cutoff are unchanged. The finishing estimate
uses the observed relationship between accepted-group work and non-KZG finishing
time, with the previous finishing sample as a floor.
Only complete, consistently measured accepted groups train that relationship.
If the finish-time check refuses the next group before it is appended, discard
that group's measurement and retain the valid accepted prefix. Its rejected
work and prospective extra blob never reduce the reserve. Source deferrals,
unknown or inconsistent measurements, failed appends and rebuilt candidates
retain the conservative full-admission-pass reserve. An untouched FIFO suffix
contributes no measurement.
A smaller candidate may scale the prior sample down; growth uses the current
measured work without multiplying a small post's fixed overhead.
The `admission.prepared` event reports admission, finishing and blob timings,
the available window and its `jitter_reserve_ms`, accepted-group work, and whether
that sample qualified for calibration.
Crossing a blob boundary rechecks the preparation allowance before accepting
the complete group. Incomplete preparation, failed submission, or a capacity
cut that bypasses older eligible FIFO work reduces the next selection. A
failed preparation halves the actual admitted workload, or the final prepared
group count after a rebuild, instead of halving a potentially much larger queued
snapshot. Without an admitted group, the existing lookahead/probe backoff applies.
A successful FIFO prefix does not impose a redundant count cap on the next slot.
The backoff still prevents outbound-first execution from indefinitely bypassing
an older inbound transaction.
If the time estimate prevents even the first group from being admitted, the
next opportunity makes a bounded one-group probe and learns again. A stale
slow sample cannot permanently freeze admission. This discards only the
performance estimate; all resource/deadline checks still apply. Source-state
waits and transport failures keep their existing retry behavior.
After canonical reconciliation identifies an excluded or partially consumed
submission, automatic admission reduces its next selection too. Source
dispositions, nonce rules, and actual-submission attempt accounting remain
authoritative; a relay error alone never proves that nothing executed.
A strict timestamp gap between consecutive canonical L1 headers can establish
that the target slot did not exist. When that evidence matches the completed
canonical scan, the observer conclusively reports uncharged exclusion, no post
or settlement was applied, and every source can retry, admission retains its
existing estimate. `admission.skipped_slot` records this decision. Actual exclusions,
partial consumption and uncertain or unavailable evidence retain backoff.

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

With the policy enabled, bounded simple deferred entries reserve gas according
to their return-data storage: 230,000 for 0–31 bytes, 250,000 for 32 bytes,
and 270,000 for 33–64 bytes. One successful simple call through a proxy registered
at the canonical parent reserves 70,000, plus its measured L1 target execution.
New proxies and complex,
nested, reverting or larger entries retain the existing 370,000 reserve.
The measured simple family permits up to 64 result bytes; an immediate call
also permits at most 256 input bytes and requires its entry result to be empty.
Calldata, the fork-specific calldata floor, companion execution and the
configured gas margin remain separate. These versioned reserves describe the
tested native ECDSA batch family; they are not network gas limits.

The 160,000 base already includes the leading anchor. Its lower estimate is
used only with a recognized previous queue: clearing each stored no-call entry
adds 55,000, 60,000 or 65,000 for those respective result sizes, even when its
consumption cursor has reached the end. Mixed queues sum each stored entry's
reserve; the result size comes from canonical storage. Unknown or
static prior queues retain the legacy projection. Queue shape and existing
proxies are read from pinned canonical state on each attempt, including after
a restart. Speculative proxy creation never earns the lower reserve.
The [result-size reserve audit](audits/2026-09-16-chiado-result-reserves/README.md)
records the measured classes, native replay tests and public Chiado comparison.
Discounts cover at most 128 total entries, including the anchor; a larger
candidate restores the old per-entry reserve rather than imposing a transaction
count cap. A first source blocked only by previous-queue cleanup remains held
until independently due consolidation clears that queue. This does not create
an additional settlement trigger.
`admission.gas_projection` records the initial reserve and previous-queue cost;
`gas_budget_cut` also reports the applied entry discount. Final ordered native
execution remains mandatory before submission.
The incremental blob estimate includes derivation metadata, semantic messages,
and the existing RLP integer-list encoding of block bytes. Unknown execution
header fields are conservatively bounded; the final actual encoding and ordered
bundle simulation remain authoritative. A single group still gets the exact
capacity check. Earlier L2 history uses the existing historical-drain rules.
Capacity/time deferral retains ownership and does not charge a failed attempt.

Automatic admission is a capacity estimate, not a guarantee that every attempt
will make its target: expensive transactions, RPC delays, and missed L1 slots
still use the existing retry/reconciliation paths. Ordinary L2 transactions use the interval DA guard described below. Numeric and unset settings keep
the previous count behavior; `auto` with disabled settlement policy is rejected.

With `EEZ_SETTLEMENT_POLICY_ENABLED=true`, ordinary L2 blocks also receive an
automatic hard data budget. The composer uses the receiving network's blob
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
post. Policy-disabled nodes and independent followers keep their existing paths.
No additional configuration or recovery journal is required. This guard prevents
new oversized intervals; it cannot rewrite an already accepted oversized suffix.

Anchor-only posting also measures encoding, sidecar, and witness preparation.
Target selection reserves the largest observed preparation time in this process
plus the configured proof time, using the selected network's slot duration.
This lets large fitting batches use a later eligible slot when necessary.
Cross-chain effects retain their bound execution slot and all existing deadline
checks remain active.

For example, this opt-in policy settles ordinary activity every 15 minutes,
consolidates every 150 blocks, and can post earlier at 10% aggregate fullness:

```dotenv
EEZ_SETTLEMENT_POLICY_ENABLED=true
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

### Following network fees with a percentage margin

With the policy enabled and both price ceilings configured, this example pays
the receiving network's estimated tip plus 20%, subject to the configured bounds:

```dotenv
# Leave EEZ_L1_POSTBATCH_PRIORITY_FEE unset to use the network estimator.
EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE=0
EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS=2000
```

`100` basis points is 1%; `2000` is 20%; `10000` is 100%. The default is `0`
(no uplift), and margins above 100% are allowed. Values must fit an unsigned
64-bit integer; empty, negative, fractional, malformed, or larger values fail
startup. Supplying this setting when the policy is disabled also fails startup.

Each new attempt uses a fresh network estimate. For example, an estimated tip
of 0.01 gwei becomes 0.012 gwei with a 20% margin. The minimum/maximum bounds
apply **after** the percentage: a 1-gwei minimum would still select 1 gwei in
this example. A fixed `EEZ_L1_POSTBATCH_PRIORITY_FEE` bypasses the estimator and
its margin, while all supplied settings are still validated. Missing estimates,
arithmetic overflow, or insufficient fee ceilings preserve due work for retry.

Execution and blob base fees already follow the selected network and fork.
Their existing `*_HEADROOM_BPS` settings raise the maximum permitted prices;
they do not multiply the base fees actually charged. The priority margin raises
the selected tip and its estimated cost. The gas-limit margin provides execution
headroom; actual charges still use receipt gas consumption. Fee-quote
events include the raw estimated priority fee, margin, bounds, and selected tip.

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
example, a 5-second/1-second profile with preparation 500 ms, proof 500 ms and
slack 700 ms triggers at 3.3 seconds after its anchor. This is an operator example,
not a default or a latency guarantee. Size proof and preparation for large
historical batches as well as small bridges. The setting requires a node restart;
all absolute proof and submission deadlines remain enforced.

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
work. No new timing setting is required. Explicit Sync targets and policy-disabled
legacy targeting retain their existing behavior.

Enabled composer nodes wake the existing L1 scanner on embedded canonical-head
notifications, so settlement evidence need not wait for the two-second fallback
poll. The scan still verifies canonical hashes, complete event ordering and
native blob data before reconciliation. Notifications coalesce, and missed or
closed notifications retain periodic recovery; finality refreshes keep their
existing timer cadence. Followers and policy-disabled nodes retain periodic
polling. This adds no polling or proof-time configuration.

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

## Migration

1. Leave the policy disabled to retain existing posting behavior.
2. Select the mode, general threshold, and both price ceilings to enable it.
3. Remove `EEZ_MAX_BLOCKS_PER_BATCH`; adjust speculative depth for `N + K - 1`.
4. Configure the same reviewed L1 profile for signer/follower deployment, retain
   native sidecar and relay settings, and size manual prover limits for the
   intended backlog. Prover errors do not authorize automatic splitting.
5. Inspect decision, allowance, fee-quote, preflight, dispatch, and canonical
   settlement diagnostics during the profile's integration checks.

Packing format is unchanged. Multi-blob batches are preferred; splitting uses
valid historical boundaries only when actual blob or gas capacity requires it.
