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
embedded L1 provider. The incident binary called
`eth_maxPriorityFeePerGas`, built a `FeeSample`, and passed that to
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

Dynamic mode now requests one gas-weighted p90 reward for each of the latest 20
canonical blocks with `eth_feeHistory`, selects the p90 of those samples, then
clamps it between the optional relay minimum and the independent priority
ceiling. The history must end at the canonical head used for the quote. The
ceiling never inherits the execution-price ceiling, the former percentage
margin is removed, and a fixed override remains exact.

An isolated poster transaction occupies only one of 20 samples and cannot move
the outer p90. On a sparse chain, empty blocks contribute zero and the optional
relay floor applies. If frequent poster transactions occupy enough samples to
affect the percentile, their already-paid tips can create a bounded plateau,
but no factor greater than one is applied and recursive growth is impossible.

This rule remains responsive to legitimate conditions. Sustained changes in
gas-weighted block rewards change the selected percentile, while isolated
spikes are rejected and the maximum bounds sustained extremes. The minimum
defaults to zero and expresses only an actual relay requirement. Fixed mode
remains available when the operator intentionally wants one known price.

This is a bounded historical estimate rather than an inclusion guarantee. A
builder auction does not expose its future clearing price. The exact block and
timestamp pin ensures a losing bundle expires instead of landing late. At the
Chiado 1–2 gwei bounds, the observed 4,373,322-gas transaction's tip is limited
to about 0.0044–0.0087 xDAI instead of roughly 0.865 xDAI.

Pinned Reth also contains an Optimism-oriented internal oracle, but its ordinary
`eth_maxPriorityFeePerGas` path samples up to the three cheapest transactions
per block. Standard `eth_feeHistory` already supplies gas-weighted reward
percentiles, so the composer needs only one stateless RPC request and a small
deterministic order statistic. It adds no cache or restart state.

## Ethereum calibration snapshot

On 2026-09-18, a public mainnet `eth_feeHistory` response covered blocks
26,002,715 through 26,003,738 with reward percentiles 50, 75, 90, 95 and 99
(raw-response SHA-256
`a9c9bad01267f897a48ce35bf0d88ed09bc073564965ca6c66508bd9f15c0935`).
Replaying each block as an unknown next block, the p90 of the preceding 20
per-block p90 values, clamped to 0.1–3 gwei, equalled or exceeded that block's
p90 reward in 90.8% of observations. The selected mean was 2.103 gwei, the
median was 2 gwei, and the 3-gwei ceiling bound 0.1% of observations.

This is a historical competitiveness proxy, not proof of future inclusion or
of a private builder auction's clearing price. It supports 3 gwei as an initial
Ethereum ceiling while preserving an explicit operator bound. Production
inclusion results must still be monitored per relay and builder path.

The corresponding Chiado sample covered blocks 23,113,330 through 23,114,353
(raw-response SHA-256
`fabed69973316d8570e41fc31e7163ae070af4eb312a0e7a4868f42fdd17f74f`).
Its latest 20-block window contained 19 zero p90 rewards and one 1-gwei reward,
so the dynamic statistic was zero before applying bounds. This validates the
Chiado-specific 1-gwei floor as active policy rather than a redundant setting;
the independent 2-gwei ceiling remains the maximum signed tip.

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
`fee_history_p90_of_p90`, `priority_max` as 2 gwei, and every `priority_fee` at or
below that bound. Then deploy the corrected binary. It rejects a missing
dynamic priority ceiling. Delete the former priority-margin assignment from the
deployment environment; neither the new parser nor its telemetry carries it.

If dynamic estimation is not desired, set
`EEZ_L1_POSTBATCH_PRIORITY_FEE=1000000000`; that mode signs exactly 1 gwei and
does not call fee history.

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
| Prevent recursive uplift | Dynamic quote uses stateless p90-of-p90 history with no multiplier; the former margin setting is absent | `fee_history_selects_recent_p90_of_p90_without_chasing_one_spike`; `sustained_self_posting_does_not_amplify_estimator_samples` |
| Enforce an independent priority ceiling | Dynamic startup requires `EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE`; fixed mode safely defaults the maximum to its exact override | `dynamic_fee_history_requires_an_independent_priority_ceiling`, invalid/empty/boundary configuration cases, and composer startup tests |
| Preserve external adjustment and relay minimums | Fresh canonical history is reduced deterministically, then clamped to the optional floor and required ceiling | `sparse_samples_use_the_floor_and_spikes_use_the_priority_ceiling` and `external_fee_history_changes_are_followed_within_independent_bounds` |
| Preserve fixed mode | Fixed override bypasses the estimator and remains exact | `fixed_priority_override_is_exact_and_does_not_query_the_estimator` |
| Exercise the signed protocol path | Composer event and transaction use the same bounded quote; canonical settlement/cost logic is unchanged | Ethereum and Gnosis `*_receipts_use_bounded_fee_history_priority` native tests |
| Ship safe configuration and migration | Generic, Chiado, and Kurtosis examples use independent bounds; Compose/Kurtosis transport validates them | Python static profile tests and `scripts/check-settlement-policy-config.py` |
