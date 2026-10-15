# Simplified fee pricing and advisory availability

Status: implementation and validation in progress; this record does not yet
establish deployment or production readiness. The operator authorized local
Docker builds, isolated tests and a Chiado upgrade. Dashboard publication remains
deferred. The accepted scope is in the
[compact quote proposal](../../proposals/compact-fee-quotes.md#simplified-pricing-and-availability-accepted-scope-2026-10-15).

Source baseline: `83f12825d831d6ce60a8f967b6c1be89a0b6ce1e`, branch
`feat/compact-fee-quotes`, PR #91 in `eez-association/eez-rollupX`. Protocol:
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, unchanged and initialized.
The separate operator checkout/configuration is preserved.

## Reproduced baseline

The new cost-floor unit regression fails on the original implementation:
`l2_tip_underfunded` rejects a payment above all configured costs when it misses
the target profit. This establishes the target/floor coupling independently of
live fee changes.

The deployed `a9313f9ad4eb54e79d628172381866accb3f7da7` runtime received three
read-only bursts of eight identical internally estimated type-2 requests through
its local Composer front. There were 3 successful quotes and 21 `quote_busy`
responses. No transaction was signed or submitted. This bounded baseline measures
first-attempt availability, not maximum sustainable throughput.

[baseline.json](baseline.json) preserves identities, counts and hashes. Detailed
logs and requests are local-only under the ignored
`artifacts/simplified-fees-20261015/`; they are unavailable to another checkout.

## Implementation and initial checks

`profit_bps` becomes a quote target. Signed admission, composed prefixes and final
native dispatch require measured costs plus `risk_bps`. Debug/observation output
exposes both amounts. Canonical completed scan pairs and ordinary L2 history
remain available for advisory estimation during pending settlement; unresolved
native suffixes are excluded. Exact submission/proof targeting and queue
ownership rules are unchanged. Quote concurrency and waiting capacity are
bounded independently of signed admission, including cancellation/shutdown.

An initial candidate passed 353 Composer tests (one existing ignored timing
profile), 189 L1 tests and 88 node tests. SDK tests and all 18 Chiado configuration
checks passed; mandatory policy rendering passed for Chiado, Ethereum and
Kurtosis. Subsequent source changes require final validation. The first local
node test run could not bind a listener inside the sandbox; its retained failure
was followed by the permitted loopback run. The first Clippy run found a
collapsible conditional, corrected without relaxing lint settings. These are
distinct from a flaky runtime test.

## L1 collection boundary

The existing submitter has no requested-refund fields or receipt-backed recovery
qualification for the deployed Chiado builder. The official
[Titan API](https://docs.titanbuilder.xyz/api/eth_sendbundle) and
[Beaverbuild API](https://beaverbuild.org/docs.html), rechecked on 2026-10-15,
document selected-source percentage refunds with a separate recipient. Their
deductions and selection rules require provider-specific verification; docs do
not establish a successful native-blob payout on this deployment. Prior
[research](../../proposals/cross-chain-fee-recovery.md#common-funded-single-source-profile)
already distinguishes a funded user refund from returning the poster's own tip.

Keep L1 `eez_quoteFee` explicitly unsupported until that collection path is
implemented and qualified. L1 gas estimation and existing subsidized inbound
execution remain available. A nominal quote alone would neither recover the
user's L1 tip nor satisfy the operator's cost floor.

## Remaining evidence

Final unit/lint checks, lower-margin legacy/type-2 process settlement, concurrent
quoting during pending settlement, Docker image identities, Kurtosis semantic
and independent replay checks, and the coordinated live handover remain to be
recorded. No durable queue recovery, actual Falcon signature workload, real wallet
extension coverage or majority-builder refund qualification is claimed.
