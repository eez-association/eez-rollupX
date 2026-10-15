# L1-origin fee collection and builder payout validation

Status: implementation and validation in progress. L1 collection has not been
enabled on the existing Chiado deployment. This record does not supersede the
completed [L2 deployment](../2026-10-15-simplified-fees/README.md).

## Identity and accepted scope

Baseline: `eez-association/eez-rollupX`, `feat/compact-fee-quotes`,
`a2da0488ec70dafe2ee8d4956be37fc27d8197b1`; initialized, clean protocol
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. The separate operator checkout,
configuration, datadirs and existing networks were preserved. The implementation
follows the [authorized L1 increment](../../proposals/cross-chain-fee-recovery.md#l1-collection-increment-authorized-on-2026-10-15)
and [refund profile](../../configurable-settlement-policy.md#l1-builder-refund-profile).
Revenue may remain on its source chain. No second user payment/signature,
protocol change, genesis migration or fee conversion was introduced.

The initial profile settles one whole L1 source group, including nested calls,
per native post. It uses an externally funded user tip, a fixed percentage refund
and a dedicated EOA recipient. Both ordinary transaction encodings share the
compact quote API and internal gas discovery. The quote targets the configured
profit; signed admission and final native dispatch require costs plus risk.
Canonical receipt accounting independently observes the builder's payment.

## Findings and first failures

The first two process tests successfully quoted and admitted legacy and EIP-1559
nested sources, but their user receipts reverted. The embedded development
`eth_sendBundle` adapter forwards transactions into a normal priority-sorted
mempool. The higher user tip can therefore place the source before `postBatch`.
It also ignores refund fields. Those tests could not establish successful ordered
settlement or payment; their logs and datadirs were retained. The adapter's
misleading ordering comments were corrected, without changing its behavior.

The process cases now assert this failure explicitly, verify the serialized
refund terms and required payer, and require the canonical missing-payment
observation to pause new L1 quotes. Their initial replay check sampled `latest`
while ordinary posts were deliberately delayed; it was corrected to compare the
settled `safe` prefix. Passing accounting or a bundle acknowledgement alone is
never recorded as a successful refund.

Review also added boundaries preventing a later native post's payment from being
attributed to an earlier post, and stops new L1 funding if one canonical payment
would be credited twice. Reorgs retract previous observations. These are
in-memory operational records, not durable queue or financial recovery.

## Validation evidence

The final arithmetic/configuration/attribution unit run passed 357 Composer,
193 L1 and 88 node tests; one pre-existing Composer timing-profile test remained
ignored. It includes mock RPC cases for missing, ambiguous, wrong-recipient,
wrong-payer, reverted, delegated/contract-cost, later-post and reorged payments.
SDK tests and 19 Chiado configuration tests passed. Initial Clippy style/import
failures were fixed without relaxing lint settings; the final check and process
and real-builder results are pending.

The controlled builder is
`ethpandaops/reth-rbuilder@sha256:916ba84da2690e3e368f57da5b0c73bfd919b452de8b95f64895846e0f3a8308`,
whose source label identifies `flashbots/rbuilder` revision
`6037fa728b13bf1806e16fff2586414216f6b8fa`. Its
[pinned parser](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder-primitives/src/serialize.rs)
accepts one selected refund transaction and defaults to an immediate payment.
This source inspection establishes API behavior, not a received payment or
support by every external builder.

Large diagnostics are local-only in
`artifacts/l1-fee-refunds-20261015` in the operator workspace. They are not
available to another checkout or uploaded as public artifacts. No private keys,
operator URLs, bulk logs or databases belong in this record. A sanitized result
summary and artifact identities will be added after validation.

## Remaining acceptance work

Complete the corrected process regressions and controlled real-builder test for
ordered native blobs, both transaction formats, nested calls, Safe creation,
actual recipient balance increase and independent follower replay. Then qualify
the actual Chiado builder before enabling this profile there. Public provider
documentation and private-network success do not establish Chiado support.
No production throughput, full protocol stress matrix, Falcon workload,
durable financial accounting or broad builder coverage is claimed.
