# Compact, direction-aware fee quotes

Status: L2 API, compact response cleanup and simplified pricing/availability
implemented, deployed and validated on 2026-10-15. The
[latest validation record](../audits/2026-10-15-simplified-fees/README.md) identifies
the exact Docker images, Kurtosis checks and live Chiado handover; the
[earlier API record](../audits/2026-10-15-compact-fee-quotes/README.md) preserves
the preceding results. The [L1-origin collection increment](cross-chain-fee-recovery.md#l1-collection-increment-authorized-on-2026-10-15)
is implemented behind an opt-in policy and
[validated on the configured Chiado builder route](../audits/2026-10-15-l1-fee-refunds/README.md).
The deployed L1 and L2 fronts share this API; additional builder providers still
require separate payout qualification.
The normative API contract is in
[the settlement policy](../configurable-settlement-policy.md#compact-fee-quotes).

## Scope and invariants

Add `eez_quoteFee` and `eez_debugQuoteFee` on Composer fronts. The L2 front
resolves omitted gas and nonce and returns independently simulated legacy and
EIP-1559 options. The L1 front uses the same contract when its refund policy is
configured and reports `quotes_disabled` otherwise. Builder eligibility, refund
recipient, funding source and actual recovery must be qualified on each network.
An API name alone does not establish builder payment.

Reuse the existing gas-discovery algorithm with the fee engine's exact L1
posting target and a shared pinned L2 parent. Both encodings run under that
context and are validated again at their actual gas limit and effective price.
A reorged or excessively old context invalidates the entire response; normal
head advancement does not. Gas discovery verifies a
measured requirement plus headroom; it does not promise a binary-search minimum.
Contracts observing `gasleft()` can require the verified high limit. Contracts
observing `GASPRICE` can fail after fees are selected; report the failed variant.

Preserve measured L1 settlement pricing, separate L1/L2 meters, whole nested
execution, ordinary-L2 settlement subsidy and final signed native validation.
Gas-limit headroom is a wallet reservation, not charged consumption. No new
payment, authorization, protocol, signer, genesis or storage format is introduced.

Each request owns a worker through preparation, gas discovery, both pricing
searches and final replays. There is no nested acquisition of the ordinary estimator's worker pool.
Cancellation/deadline expiry does not release the fee worker permit while a
blocking frame is still running. `eth_estimateGas` keeps its own existing limits.

## API and migration decisions

Use optional `gas`, `nonce` and `chainId`, with canonical hex quantities. Default
to both types unless a type or explicit `transactionTypes` selection is supplied.
Reject conflicting selections. Preserve supplied gas/nonce, and refuse queued
nonce dependencies instead of simulating a different transaction. Per-type
execution errors are explicit; shared-context/infrastructure errors fail the
whole request. Compact output includes only wallet fields, transaction request
bindings, expiry, source network/nonce and estimated source fees. Tariff enforcement mode
is diagnostic metadata, exposed only as `diagnostics.recoveryMode`. The unsigned
input is a transaction request, and `requestHash` binds its normalized fields
without claiming to be a signed Ethereum transaction hash.

`eez_debugQuoteFee` runs the identical engine and adds detailed diagnostics.
It is not an execution trace. Separate calls can select different heads; compact
and debug outputs are equivalent only for the same transaction request and context.

The user explicitly chose a breaking API replacement: remove
`eez_quoteL2ToL1Fee`, the HTTP quote route and old SDK helpers. Migrate the
reference dashboard and test drivers to the new API in the same change. Upgrade
server and clients together; no compatibility fallback or unquoted send is
retained. The SDK verifies transaction request bindings using the application's
existing Ethereum hash function.

## Validation criteria

Exercise compact and debug routing, one/both types, omitted and explicit fields,
canonical quantities, wrong chains/nonces, ordinary and nested execution,
fee-sensitive rejection, gas-sensitive replay, insufficient balance, cancellation
and permit retention. Compare source gas and measured L1 fees against receipts,
reject underfunded signed transactions, and independently replay settled blocks.
Retain signed-fee and Safe regressions and verify the old method is absent.
Recorded Falcon gas limits are parser coverage only; qualification of actual
Falcon signature workloads is separate.

The full protocol staged/stress matrix and live deployment are separate evidence;
unit or process success alone must not be described as either.

## Source balance diagnostics (accepted correction, 2026-10-18)

The [rollback-and-call-again investigation](../audits/2026-10-18-revert-fee-quote-report/README.md)
found that gas discovery and zero-tip execution succeeded, but selecting the
settlement tip exceeded the sender's balance. The source simulator retained a
typed balance condition for retry-aware callers; the quote path discarded it.
The user requested correcting this Composer error and adding regressions.

Preserve the typed balance failure through gas discovery, priced simulation and
compact/debug projection. Report `insufficient_funds` with the source chain,
available and required balances, and shortfall. Keep normal EVM balance checks,
settlement tariff enforcement, immutable quote state and queue ownership intact.
Returning a quote before funding or changing the tariff is outside this fix.

Acceptance: reproduce the priced-pass failure before the fix, check both legacy
and EIP-1559 options with omitted and supplied gas, verify that compact/debug
errors retain the same amounts, and show that funding the same sender makes
the caught-revert call quotable. Check that true execution reverts and underpaid
settlement tips retain their distinct errors. The owning
[API contract](../configurable-settlement-policy.md#compact-fee-quotes) describes
the additive error fields and mixed-version behavior.

## Simplified pricing and availability (accepted scope, 2026-10-15)

The operator requested implementation, testing and deployment of the following
refinement. Its implementation and deployment are recorded in the
[simplified fee audit](../audits/2026-10-15-simplified-fees/README.md).

Use the existing policy fields without adding configuration layers. `risk_bps`
is the mandatory reserve above measured posting, blobs and configured service
costs; `profit_bps` is the desired additional markup in a quote. Admission,
composition and final dispatch require costs plus the reserve, not the target
profit. The trial configuration moves its 2000 bps from risk to profit: quote
120% of estimated cost, accept less surplus while still covering refreshed cost.
Operators may retain a nonzero mandatory reserve. Conversion remains an explicit
operator assumption, and ordinary L2 settlement remains subsidized.

Quotes use the latest completed, canonical L1/L2 scan pair, even while the next
scan or an unrelated settlement is pending. Extend the L2 parent through ordinary
history only; stop before unresolved native effects. Every request keeps one
immutable context for gas discovery, both envelopes and priced native replay.
Normal head advancement does not invalidate an estimate. Reorged or excessively
old inputs still fail. A quote has a short wall-clock lifetime independent of the
simulated L1 slot; final settlement still selects and validates its exact target.
Dependencies on unresolved transactions are not guessed or silently executed.

Allow a small bounded pool of quote workers and a bounded waiting queue, with
separate signed-admission capacity. Preserve cancellation, shutdown and permit
ownership through blocking execution. No unbounded background work or financial
reservation is introduced. Standard gas estimation keeps its execution contract;
the relaxed snapshot rule belongs to advisory fee quotes and fee admission only.

Validation must cover a 20% quote followed by lower-margin signed settlement,
rejection below costs, the mandatory reserve, clipped effective tips, nested
calls, both envelopes, independent replay, concurrent requests, ongoing
settlement, advancing heads, reorg invalidation and expired context. Record
first-attempt availability and timing rather than hiding failures with retries.

Changing profit from a hard requirement to a quote target is a behavioral
upgrade: older Composers can reject lower-margin signed transactions. Coordinate
policy and Composer rollout/rollback, drain accepted work, preserve datadirs and
the existing compact API client upgrade. No protocol/storage migration applies.

At the simplified-pricing baseline, L1→L2 remained `unsupported_direction`:
the submitter had no refund path and the deployed Chiado route had no
demonstrated payout. The later
[L1 collection increment](cross-chain-fee-recovery.md#l1-collection-increment-authorized-on-2026-10-15)
adds an opt-in refund path and uses `quotes_disabled` while it is unconfigured.
Enable it only after qualifying the native-blob bundle, selected payer, payout
deductions, recipient and canonical receipt on the receiving builder route.
Requested refunds and the poster's own tip are not external revenue.
