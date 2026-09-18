# L1 priority-fee feedback loop

## Incident evidence

Chiado transaction
`0x304de0437211910afb658ef77dc6424bd9655f58b8716325d08aa18a701158b4`
was included in block `23106030` with:

- block base fee: 7 wei;
- block gas use: 4,394,322 of 59,765,405 (about 7.35%);
- transaction gas use: 4,373,322;
- `maxPriorityFeePerGas`: 197,813,594,902 wei;
- `maxFeePerGas`: 197,813,594,916 wei;
- effective gas price: 197,813,594,909 wei; and
- total execution charge: 865,102,546,514,617,698 wei.

The prior observed tips were 114,475,460,012, 137,370,552,015,
164,844,662,418, and 197,813,594,902 wei. Checked ceiling arithmetic reproduces
each transition exactly as `ceil(previous * 1.2)`.

The deployed fee settings had a 1-gwei priority floor, a 20% priority margin,
a 500-gwei execution-price ceiling, and no independent priority ceiling. The
old parser therefore used the 500-gwei execution ceiling as the priority
ceiling. The 7-wei base fee did not protect the account: EIP-1559 charged the
explicitly signed priority fee because the fee cap allowed it.

## Exact runtime path

`PostingRuntime::quote` in `crates/eez-composer/src/composer/posting.rs` uses the
embedded L1 provider. With no fixed override, it calls
`eth_maxPriorityFeePerGas`, builds a `FeeSample`, and passes that to
`PostingFeeConfig::quote` in `crates/eez-l1/src/posting_fees.rs`. The resulting
priority price is written into the EIP-4844 poster transaction. The observed
fee cap is also explained exactly: twice the 7-wei base fee plus the selected
197,813,594,902-wei tip is 197,813,594,916 wei.

The correction also removes an unused, lenient second read of
`EEZ_L1_POSTBATCH_PRIORITY_FEE` from node startup. `PostingFeeConfig::parse` is
now the single typed parser and `PostingRuntime` is the single consumer.

The pinned Reth RPC implementation routes `eth_maxPriorityFeePerGas` to
`GasPriceOracle::suggest_tip_cap`. Its default oracle examines 20 populated
blocks, takes up to the three lowest effective tips per block, selects the 60th
percentile, ignores only transactions signed by the block beneficiary, and
caps its own result at 500 gwei. Empty blocks contribute zero and extend the
history scan. It has no poster-address exclusion, so this composer's own posts
are eligible samples and can dominate a low-activity chain.

## Correction and soundness argument

Dynamic mode now selects
`clamp(eth_maxPriorityFeePerGas, priority_min, priority_max)` directly. It
requires an explicit `EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE`; this value never
inherits the execution-price ceiling. The former percentage-margin setting is
removed. A fixed priority override remains exact and safely becomes its own
maximum when no separate maximum is configured.

For a sparse chain dominated by the preceding poster transaction, model the
oracle as `estimate[n] = selected[n-1]`. If that value is within the bounds,
the new rule gives `selected[n] = selected[n-1]`; there is no factor greater
than one and therefore no recursive growth. Below the floor it reaches the
floor once. Above the ceiling it reaches the ceiling once. At all times the
selected tip is no greater than the independent priority ceiling.

This rule remains responsive to legitimate conditions. A changing external
estimate passes through unchanged while it is within the bounds. The minimum
expresses an actual relay requirement, and the maximum bounds estimator spikes.
The fixed mode remains available when the operator intentionally wants one
known price.

This is a bounded adaptation policy rather than perfect demand attribution. If
the poster dominates Reth's sample after an external spike, its own bounded
posts can keep the estimate at that in-range value until other samples replace
them. They cannot raise it further. At the Chiado 1–2 gwei bounds, the observed
4,373,322-gas transaction's tip is limited to about 0.0044–0.0087 xDAI instead
of roughly 0.865 xDAI. Fixed 1-gwei mode removes even that bounded persistence
when an operator prefers minimum spend over fee-market adaptation.

Pinned Reth also contains an Optimism-oriented internal oracle that returns a
minimum tip while the latest block has spare capacity and raises it only when
the block is full. That method is not exposed through the standard L1 RPC used
by the composer. Reimplementing it or a poster-excluding multi-block oracle in
the composer would add chain-history reads, caching, and a second oracle whose
behavior could drift from the node. The bounded raw estimate removes the proven
positive-feedback term without that extra state and remains restart-safe.

## Chiado migration

The existing binary already accepts the safe values, so operators can stop the
feedback before upgrading:

```dotenv
EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE=1000000000
EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE=2000000000
# EEZ_L1_POSTBATCH_PRIORITY_FEE must remain unset for dynamic mode.
```

Apply the configuration through the normal rendered deployment environment and
restart the composer only after reconciling any already-submitted bundle. Verify
that `eez.composer.postbatch.fee_quote` reports `priority_source` as
`network_estimator`, `priority_max` as 2 gwei, and every `priority_fee` at or
below that bound. Then deploy the corrected binary. It rejects a missing
dynamic priority ceiling. Delete the former priority-margin assignment from the
deployment environment; neither the new parser nor its telemetry carries it.

If dynamic estimation is not desired, set
`EEZ_L1_POSTBATCH_PRIORITY_FEE=1000000000`; that mode signs exactly 1 gwei and
does not call the network estimator.

## Removed stale comparison

The receipt test binary still contained two comparisons against the removed
optional-policy posting mode. Their baseline started the now-mandatory interval
policy with its one-day/one-million-block test defaults and then waited for a
legacy per-Sync post that can no longer occur. The tests timed out by
construction and did not exercise fee selection. Their historical evidence is
retained under `docs/audits`; the obsolete runtime comparison and its helper
machinery were removed. Current canonical receipt coverage is provided by the
bounded-priority Ethereum/Gnosis cases and the independent settlement/cost
suites.

## Requirement coverage

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Deterministically reproduce self-feedback | `posting_fees` retains the checked arithmetic helper for an isolated regression vector | `observed_percentage_uplift_reproduces_the_self_feedback_loop` checks the three exact 20% transitions |
| Confirm the deployed estimator/configuration path | `PostingRuntime::quote` and the pinned Reth oracle are traced above; the unused duplicate node setting was removed | Native receipt tests match canonical-parent quote events to the signed transaction and receipt |
| Prevent recursive uplift | Dynamic quote uses the raw estimate with no multiplier; nonzero legacy margin fails startup | `sustained_self_posting_does_not_amplify_estimator_samples` models 20 self-dominant rounds |
| Enforce an independent priority ceiling | Dynamic startup requires `EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE`; fixed mode safely defaults the maximum to its exact override | `dynamic_estimator_requires_an_independent_priority_ceiling`, invalid/empty/boundary configuration cases, and composer startup tests |
| Preserve external adjustment and relay minimums | Dynamic estimates clamp to the explicit floor and ceiling on every fresh quote | `sparse_samples_use_the_floor_and_spikes_use_the_priority_ceiling` and `external_estimator_changes_are_followed_within_independent_bounds` |
| Preserve fixed mode | Fixed override bypasses the estimator and remains exact | `fixed_priority_override_is_exact_and_does_not_query_the_estimator` |
| Exercise the signed protocol path | Composer event and transaction use the same bounded quote; canonical settlement/cost logic is unchanged | Ethereum and Gnosis `*_receipts_use_bounded_network_priority` native tests |
| Ship safe configuration and migration | Generic, Chiado, and Kurtosis examples use independent bounds; Compose/Kurtosis transport validates them | Python static profile tests and `scripts/check-settlement-policy-config.py` |
