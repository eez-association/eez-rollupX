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

## Reuse priced native validation (2026-10-21)

Status: implementation authorized by the user; local correctness and repeated
timing checks passed ([validation record](../audits/2026-10-21-quote-replay-reuse/README.md)).
Release CI and live deployment are outside that evidence. Baseline:
RollupX `1ab1e2e3386a8832c7b18224e1eed82fa7c0b074`, pinned protocol
`abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`.

The L2 quote already composes and natively replays each candidate price before
measuring L1 settlement. Attach the existing effect inspector to that native
source replay and its callback deliveries, then verify the outbound event
hashes and source return data. Price its receipt gas directly. The compact
projection can then omit its separate final composition/setup/source pass.
Ordinary L2 calls also retain the comparison between composition and native
return data.

| ID | Requirement | Acceptance |
| --- | --- | --- |
| O1 | Validate native call identities, outcomes, event hashes, source output and callback order at the returned gas and fees | Nested process tests, gas/fee-sensitive rejection and priced replay trace assertions |
| O2 | Preserve actual source/native/settlement gas pricing, margin and admission floors | Fee receipt round trips, underpayment rejection and independent follower replay |
| O3 | Keep source execution independent across fee iterations, formats, requests and snapshots; bind any L1 reuse to one envelope and identical replay inputs | Both formats, explicit/omitted gas, fee-dependent changes, exact settlement identity and existing context tests |
| O4 | Reduce duplicate work without weakening the estimator or L1 settlement replay | Nested L2 fixture: 19→16→14 runs with omitted gas, 13→10→8 with explicit gas across the two optimizations; same-workload baseline/candidate timings |

Gas discovery and settlement gas-limit verification remain unchanged: `gasleft()`
and `GASPRICE` can change behavior, so zero-price composition cannot be reused
as a priced result. No speculative cross-request cache or new concurrency is
introduced. The existing worker retains ownership, limits and cancellation.

This changes no quote fields, fee policy, contracts, stored data or historical
replay. Debug traces report the executions actually performed: successful L2
quotes no longer include a separate `final_validation` phase. Effect failures
are attributed to `native_replay` within `fee_simulation`. Viewers must not
require a particular phase count. Mixed versions and rollback need no migration;
older Composers perform the extra pass. Live deployment is outside this change.

Measure repeated serial quotes for identical nested fixtures, formats and gas
selection on the baseline and candidate, using the same runtime build profile.
Retain run counts and elapsed milliseconds; shared-host timing noise prevents
inferring production throughput from these measurements.

### Identical settlement replay reuse (authorized follow-up, 2026-10-21)

Baseline: the first optimization at `09934699357fc8da2c5bf80c9554edff1d1df3bf`.
Local unit, native fee/Safe, viewer and two-round timing checks passed; the
[follow-up record](../audits/2026-10-21-quote-replay-reuse/settlement-reuse.md)
identifies exact executables, measurements and remaining CI/deployment limits.
Keep source composition and native validation at every candidate fee, but reuse
a successful L1 gas measurement within that envelope's pricing loop when all
L1 replay inputs match exactly. The estimator owns immutable references to the
posting runtime and quote, so the state hash, target environment and gas policy
cannot change during reuse. Compare the complete prepared posting transaction
(including gas, fees and blobs), verifier calldata, settlement endpoint, rollup
and expected callback identities/outcomes. Funding and context checks still run;
the follow-up below reuses validated snapshot preparation. Recompute payment
from the measured gas.

This narrows the original O3 restriction: only identical successful L1 replays may be reused
between fee iterations; source execution, different formats, requests and
snapshots remain independent. A changed nested call, result, calldata or blob
count requires replay. Failures are not cached. Retain both the generous and
selected-limit probes on a miss, including the verified high-limit fallback
for gas-sensitive settlement. Signed admission and final posting remain fully
validated. No shared cache, concurrency, configuration or storage is introduced.

Acceptance: demonstrate the same nested quote goes from 16 to 14 recorded runs
with gas discovery (10 to 8 with supplied gas), retains the final priced source
validation and identical cost arithmetic, and does not reuse changed
fee-dependent settlement inputs. Test callback-expectation changes separately
from calldata equality and retain failed-probe behavior. Measure repeated
baseline/candidate latency with the same fixture and build profile. Debug
diagnostics count reused L1 measurements; traces contain only executions that
actually ran. Run counts remain diagnostic, not an API guarantee.

### Immutable preparation reuse (authorized follow-up, 2026-10-21)

Baseline: `ec9a22a04bd590ae183c1de80df508dd542443a8`, including the two
optimizations above. Local unit/native checks and 1,920 quote arithmetic checks
passed; the [validation record](../audits/2026-10-21-quote-replay-reuse/immutable-preparation.md)
reports mixed latency results and regressions. This is a reduction in repeated
preparation work, not an established general latency improvement.
Within one envelope's estimator, load the registry, validate deployment code,
fetch the verification key and read the poster account once at the pinned L1
state. Keep this preparation lazy: ordinary L2 execution does not acquire a
settlement dependency. A different rollup or expected settled root must repeat
the lookup and validation; failed preparation is not cached.

Rebuild the batch, blob hashes, proof inputs, callback expectations and posting
transaction for every pricing iteration. In particular, a cached poster balance
does not authorize reusing a blob reservation or affordable gas limit: recompute
those from the current blob count. Preserve source/native execution, exact L1
replay identity, fee arithmetic and final signed validation. There is no cache
sharing between requests, formats or L1 snapshots, and no API, configuration,
storage, deployment or rollback change.

Acceptance: regress preparation identity and failures, run nested and Safe
fee/receipt/replay tests, and compare repeated nested-callback and nested-Safe
quotes on the baseline and candidate using identical harnesses/build profiles.
Report timing noise and unchanged gas/payment arithmetic. These removed setup
reads were not recorded as trace runs; the existing trace run count need not
decrease. Release/PBS validation remains separate from native process tests.

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

## Execution failure diagnostics (accepted correction, 2026-10-20)

### Affordable posting probes and partial failure exports (accepted, 2026-10-20)

The operator authorized fixing both priced routes after the
[mainnet investigation](../audits/2026-10-20-mainnet-quote-reservation/README.md)
found that their maximum-gas probes could exceed the poster's balance even when
the actual settlement was affordable. Use final posting's existing
balance-minus-blob-reservation calculation for the initial probe, retain native
execution and the validated final gas margin, and distinguish genuine poster
funding failures from source-wallet failures. Preserve partial debug traces
when a blocking calculation fails after capture begins. Infrastructure failures
still invalidate every quote option; provider credentials must not enter exports.

Acceptance includes reproducing the old failure, nested calls in both
directions, legacy/EIP-1559 and explicit/omitted gas, blob/cap arithmetic,
unfunded-posting errors, unchanged canonical state, RPC error projection, viewer
imports and existing fee/settlement regressions. This correction does not change
fee policy, queue ownership, consensus or signing authorization. The
[API contract](../configurable-settlement-policy.md#compact-fee-quotes) owns the
additive error and trace fields and mixed-version behavior.

### Earlier execution-revert correction

The [issue #93](https://github.com/eez-association/eez-rollupX/issues/93) review
confirmed that quote error conversion discarded estimator reverts, effect
mismatches and unsupported-execution reasons. An isolated process regression
against `51ef1ba08d5c506c8909bc8be2e16e8d25c0a7a8` reproduced the missing
revert details from a contract with a known `Error(string)` payload.
This does not establish the cause of the reported GPv2 transaction failure.

Preserve the typed execution cause and quote pass until debug projection.
Expose bounded revert bytes and messages only in diagnostic failed options.
Keep the compact response, operational retry categories, funding diagnostics,
execution rules and final signed validation unchanged. The
[API contract](../configurable-settlement-policy.md#compact-fee-quotes) defines
the fields, bounds and mixed-version behavior. This is not an execution trace;
it does not promise identification of the failing nested contract.

Acceptance: repeat the baseline process check with both transaction types,
supplied and omitted gas, and a contract that reverts only after pricing.
Assert unchanged canonical nonce/state. Test empty, custom, malformed and
oversized revert payloads, UTF-8 truncation, effect mismatches, unsupported
execution and provider-error redaction. Retain fee-enabled nested CREATE2
coverage with native settlement and independent follower replay. The original
GPv2 route still requires its exact failing transaction to reproduce.

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
