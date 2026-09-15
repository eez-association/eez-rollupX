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
| `EEZ_L1_CHAIN_CONFIG` (signer and follower) | Absent legacy profile | `mainnet`, `chiado`, or a reviewed Ethereum-format genesis path. Enabled signing requires the selected L1 profile to validate its target context. Historical followers use the containing block's fork. |

Margins use checked `ceil(x * (10000 + margin) / 10000)` arithmetic. Fee ceilings
are per-unit prices, not currency budgets or maximum transaction costs. A
required price above its ceiling keeps work pending as `fee_cap_blocked`.

With `EEZ_MAX_USER_TXS_PER_BUNDLE=auto`, the composer selects a finite FIFO
prefix using preparation measurements from previous attempts. It starts with
one group after restart and grows as measured time permits, without a fixed
transaction-count ceiling. It reserves time for final block construction,
effect-prefix commitments, encoding, KZG and witnesses before the existing
proof cutoff. A smaller time budget or a capacity cut reduces the next
selection. This also prevents outbound-first execution from indefinitely
bypassing an older inbound transaction.
After canonical reconciliation identifies an excluded or partially consumed
submission, automatic admission reduces its next selection too. Source
dispositions, nonce rules, and actual-submission attempt accounting remain
authoritative; a relay error alone never proves that nothing executed.

Complete groups must fit L2 block gas, the active L1 transaction and whole-bundle
gas budgets, and the remaining blob allowance after companion transactions.
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

Policy-driven historical batches select the earliest future L1 slot that leaves
the configured proof-time budget and submission slack. Timely reconciliation can
therefore lead to posts in consecutive L1 blocks; later preparation selects a
later slot. Builder lead controls when submission opens, not a mandatory block
gap. Selection is logged as `eez.submitter.policy.target_selected`, including the
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
