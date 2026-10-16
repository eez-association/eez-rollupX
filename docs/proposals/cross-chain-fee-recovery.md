# Cross-chain fee recovery

The public quote API is now specified in [compact fee quotes](../configurable-settlement-policy.md#compact-fee-quotes).
The earlier method/HTTP migration described below is historical and is removed
by the compact API replacement. Both transaction encodings remain supported.

| Field | Value |
| --- | --- |
| Design status | L2 tariff and opt-in L1 collection trials implemented, validated and deployed on the configured Chiado route. Production financial reservations and broader collection profiles remain draft. No core protocol or genesis change. |
| Date | 2026-10-10 |
| Source baseline | `eduadiez/rollupX`, `main`, `fa916b13254a36423cad130080dde367bb18a7d6`; existing operator/worktree changes excluded |
| Protocol baseline | Initialized, clean `eez-core-protocol` at `855fe0602484750861b4c31502a1a91355d33ed6` |
| Owning contracts | [Settlement policy](../configurable-settlement-policy.md), [RPC contract](../../crates/eez-control-rpc/SPEC.md), [signer profile](../../crates/eez-proof-signer/SPEC.md), [core specification](../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md) |
| Research evidence | [Source trace, references, review and limitations](../audits/2026-10-10-cross-chain-fee-research/README.md) |

## Recommendation

### Stress recovery and batching corrections authorized on 2026-10-16

Recover the 231 retained test sources with the operator-approved RPC pause and
same-image restart, preserving the stopped datadir, source identities and any
explicit replacement envelopes. Canonical source evidence, not admission or a
posting receipt alone, determines recovery. The original failed run stays failed.

The follow-up implementation aligns quote history with the active posting window
(reserving its terminal block), while retaining deadlines, byte bounds, canonical
ancestry and exclusion of unsettled native effects. Native execution failures in
final preflight must cause smaller whole-group retries instead of replaying the
unchanged candidate forever. Size reduction retains ownership without charging
users; an indivisible source that repeatedly fails native validation needs a
bounded explicit disposition, but only a failed native companion execution bound
to that exact signed L1 hash can charge its retry counter. Posting/root failures
cannot attribute blame to a singleton. Infrastructure failures must not train
this backoff.
The source gas estimate needs headroom for shared-table scans, but headroom alone
cannot replace final ordered validation or adaptive batching.

Explicit future L1 nonces may replay up to eight contiguous, owned signed
predecessors under the same quote deadline and work bounds. Replay must retain
real nonces, balances, fee fields and nested effects on both chains in disposable
state; changing a nonce in otherwise canonical state is not sufficient. Each
predecessor uses a conditional native post at the sampled L1 target. Final
settlement rebuilds and validates the actual shared batch. Missing bytes, gaps,
replacements during replay, and unsupported L2 predecessor profiles fail closed.

Acceptance requires the original failure mechanisms as regressions, full protocol
semantic checks including consecutive source nonces, nested/reverting calls and
Falcon Safe workloads, exact quote fees, canonical settlement, native DA checks,
and independent follower replay. Record any unsupported profile explicitly.
Protocol, signer, contracts, genesis and persistent data formats remain unchanged;
drain the in-memory queue before upgrade or rollback. This section records the
authorized implementation scope, not a claim that validation has passed.

### L1 batching and observation-only recovery authorized on 2026-10-16

The operator accepts losses when one user's refund does not cover a shared L1
batch. Remove the single-source scheduling restriction: include whole eligible
L1 source groups up to the existing gas, blob, timing and configured admission
bounds. Select the largest recoverable **total** priority payment using final
native receipts, not the highest per-gas price. Request one refund and retain
the selected payer as mandatory; preserve source order and nonce dependencies.
Do not reject or shrink a final batch because that refund is below its cost.
Standalone quotes still target the configured profit and signed ingress still
checks the standalone cost/risk floor. L2 fee enforcement is unchanged.

Missing, ambiguous and insufficient refunds are observations, never a reason
to pause quotes, admission or settlement. Reject duplicate accounting credits,
keep canonical consumption/reorg rules, and expose costs, received refunds,
shortfalls and observation errors in Prometheus/Grafana. Do not credit projected
payments as cash. In-memory attribution remains acceptable for this iteration;
monitoring resets on process restart and is not a durable financial ledger.
No automatic alert, provider switch or new configuration mode is introduced.

Acceptance: a real builder settles multiple independent nested L1 sources in
one post, refunds the largest measured payer, and repeats after an undercovered
batch. Process tests show quoting and dispatch continue after missing refunds.
Unit tests cover selection below the batch floor, excluded senders, bounded
labels, duplicate observations and reorg corrections. Existing execution,
partial-consumption, L2 fee and policy-disabled regressions remain applicable.

This authorization supersedes the single-source, final L1 coverage and
payment-failure stop requirements of the preceding increment below. It changes
Composer policy and observability only; protocol, signer and replay formats
remain unchanged. Older Composers restore the one-source restriction and stop
behavior, so drain accepted work before upgrading or rolling back.

The [October 16 protocol/Falcon stress follow-up](../audits/2026-10-16-chiado-batching-stress/README.md)
confirmed multi-source posts and largest-payer refunds, and passed all 48 Falcon
sources. It did **not** pass the full protocol matrix: isolated gas estimates
failed in shared candidates, queued future nonces were unsupported, and the
256-block history bound blocked replacement admission after the resulting stall.
The report preserves that failure and records the later authorized recovery of
all 231 sources. Recovery alone does not validate the fixes or establish general
workload or production readiness; a fresh matrix remains required.

### L1 collection increment authorized on 2026-10-15

Implement the funded single-source refund profile after the
[L2 fee load test](../audits/2026-10-15-chiado-fee-load/README.md). Revenue may stay
on its source chain; sweeping or converting the L1/L2 balances is not a release
prerequisite. Keep both denominations explicit and never count one chain's
balance twice. L1 and L2 fee-funded candidates remain separate in this increment. The first
implementation deliberately settles one complete L1 source group per funded post;
shared L1 batching requires a later measured prefix-funding check. This is an
implementation scope limit, not a change to the proposed future batching policy.

The L1 front reuses the compact quote contract and internal gas estimation.
Price complete native posting/DA plus the configured service costs, risk and
profit; divide the required refund by the selected L1 source's measured receipt
gas and the configured refund percentage. Deduct an EOA payout's gas cost before
crediting recovery. Iterate at the actual fee fields and retain final signed
native validation. The quote targets profit; signed admission and dispatch may
consume that profit while preserving the configured cost/risk floor.

Use an opt-in, immediate EOA refund profile with explicit percentage and
recipient. Every receiving relay must be qualified for the same profile; no
refund-dependent request goes to an unqualified fallback. Select one external
user by recoverable total priority payment, preserve source order, and remove
that payer from the optional-revert list. Keep all other partial-consumption,
nonce, ownership and reconciliation rules. A return of the poster's own money
is not revenue. Do not request sponsorship or depend on discretionary rebates.

Record the selected source and requested payout with the posting identity, then
reconcile canonical L1 payments separately from L2 tips. Missing, ambiguous or
wrong-recipient payments remain unqualified. Refund observation must not change
whether a source was canonically consumed or authorize resending its nonce.
Requested refunds are projected revenue, not received cash or an on-chain
zero-loss guarantee. Existing in-memory ownership/recovery limits remain.

Acceptance requires arithmetic/configuration and request-encoding regressions,
internal quotes for both formats, nested/Safe execution, cost-floor rejection,
mandatory refund payer handling, canonical payment evidence and preserved
policy-disabled behavior. A controlled builder test is distinct from qualifying
an external provider; enable a live network only after its native-blob payout is
verified. The [October 15 qualification](../audits/2026-10-15-l1-fee-refunds/README.md)
records successful isolated-builder and configured-Chiado-route checks and the
live deployment. It does not qualify additional builder providers.

Introduce a transaction-specific **cross-chain service quote**, backed by a
Composer admission and final dispatch check. The quote pays for the poster's
L1 execution, blob DA, a share of settlement overhead, proving/operations, and
bounded execution risk. Source-chain gas remains a separate user expense.

For L2-to-L1 v1, use the transaction-specific **dApp/SDK tip calculation**
clarified by the user on 2026-10-14. The application obtains the complete
source group's cost estimate, converts it into ordinary signed priority-fee
fields, and submits one normal wallet transaction. A Composer-backed application
service can supply the estimate over HTTP; `eez_quoteCrossChainFee` is optional.
Standard wallet fee RPCs alone do not supply arbitrary nested L1 cost data.
Keep the existing execution rules and enforce funding in Composer admission
and final dispatch. The network-wide tip alternative and native settlement-gas
design remain alternatives, not the chosen v1 direction. The later operational
authorization and bounded Chiado tariff release are recorded in the fifth
increment. That release does not accept the remaining production financial
reservation design.

The user's L1 constraint is **one ordinary signed application transaction, with
the service contribution carried by its existing gas-fee fields**. The quote
requires no signature. There is no additional customer payment, deposit,
approval, permit, delegation or fee authorization. An operator-authenticated
builder request and a builder's funding/refund transaction are backend work;
neither creates another user action. This constraint supersedes the earlier
prepaid-credit recommendation.

Use L2 priority fees for L2-origin requests. For L1-origin requests, calculate
one required net payment for the candidate, then admit only builders whose
qualified payment mechanism covers it. The broadest documented common payment
profile is a funded bundle with a refund from one real user's transaction.
Use it for one independently settled request group, or a shared batch only
when that user's existing quoted tip covers the entire batch. The user accepts
higher quoted ordinary tips and optional later rebates as a launch trade-off.
This makes the common refund profile the proposed initial L1 collection mode;
uneven cost allocation or higher fees are not, by themselves, a launch blocker.
It still does not collect small contributions from every user.

For efficient shared-cost batching, prefer refunds based on all contributing
user transactions; value sponsorship is an optional adapter where qualified.
The [common-payment comparison](#common-payment-profile-and-efficient-batching)
separates these modes and the provider changes needed to extend pooled refunds.
No current evidence establishes efficient pooled collection at every major
builder. Treat variable BuilderNet
redistributions as realized upside, with **zero contribution to the admission
budget**. An L1 tip does not pay the poster until the builder transfers value
back or funds an otherwise poster-funded expense.

Start with accounting and shadow quotes, then enforce priced admission for new
work. Do not promise literal zero loss from tips alone: the present protocol
supports partial consumption, L2 receipts can be speculative, and a builder can
include a paid post while omitting its revenue-bearing companions. For bounded
operation, reserve operator-funded risk capital. This bounds exposure; it does
not turn a builder promise into an on-chain zero-loss guarantee. If a route
cannot cover the service cost, defer or reject new work rather than introducing
an extra user payment as a fallback.

Verification follow-up: the [local sponsorship experiment](../audits/2026-10-10-cross-chain-fee-research/README.md#sponsorship-verification)
establishes funding mechanics with the pinned EEZ contract and a mock proof,
including residual-balance batches and a funded native post with a separate
underfunded operator payout. The harness supplies the funding; deployed builder
acceptance remains unverified. Current Composer finalization requires actual
parent-state affordability and simulates without a builder funding prefix.
Sponsorship needs explicit composition/preflight integration; disabling balance
validation is not a solution. The common refund profile keeps submitted senders
funded and avoids that funding-prefix requirement, but still needs refund API,
admission and reconciliation integration.

Cover Ethereum and Chiado/Gnosis independently. An Ethereum refund API does not
establish Chiado support. Gas denominations, redeemability of the L2 native
asset, and the cost of moving revenue to L1 belong in the network profile.

## First iteration: shared pricing with two collection routes

Consolidated design assessment, 2026-10-14. The preferred approach is suitable
for a bounded first implementation through the integrated dApp/SDK. Execution
rules and the ordinary user signature remain unchanged. The design is coherent;
production prices, wallet behavior and actual L1 builder payments still need
validation before launch. The broad-builder-coverage objective remains an unmet
qualification milestone, not a requirement waived by calling this v1.

Implementation priority is now L2-to-L1 fee recovery, with its own staged release
criteria. L1-to-L2 refund collection follows later. The two-direction design below
remains the target; L1 builder qualification does not block accounting, shadow
quotes or a separately qualified L2-only fee release.

| Direction | Collection for v1 | Funding rule |
| --- | --- | --- |
| L1 to L2 | Fund the post, submit the ordered post/users bundle privately through qualified builders, and request a fixed-percentage refund from one external user's priority payment to the treasury | The selected source's net refund, after provider deductions, covers the complete candidate's required contribution. Select by recoverable payment within valid execution order, not tip rate alone. |
| L2 to L1 | The dApp/SDK sets a transaction-specific ordinary tip; the controlled L2 beneficiary collects the included users' effective priority payments | Conservatively realizable qualifying L2 tips cover the complete candidate's required contribution. The treasury maintains liquid L1 funds for posting. |

Implement one shared path:

1. The application supplies the intended transaction to a Composer-backed cost
   estimator. Price its complete supported source group, including nested work,
   actual encoded DA and conservative standalone posting/service overhead. Keep
   user-funded execution and bridged principal out of operator expense.
2. Convert the required contribution into normal signed fee fields through the
   direction's collection formula. L1 pricing includes the builder's refund
   fraction and deductions; L2 pricing uses native source receipt gas. An HTTP
   application service is sufficient; a new wallet-facing JSON-RPC method is not
   required. The user sees the expected fee and signs one ordinary transaction.
3. Send the signed transaction through the integrated admission/submission path.
   In particular, L1 recovery depends on the Composer's qualified private bundle
   route; public-mempool broadcasting or standalone builder submission is not
   an equivalent payment path. Test the actual wallet transport as well as its
   handling of supplied fee fields.
4. Enforce funding at admission and again against the fully materialized batch
   at dispatch. Preserve exact target, whole source groups and signed limits.
   Reject underfunded new work; retain accepted work under the existing ownership
   contract if a subsequent price change blocks dispatch. Do not assume future
   users will arrive to fund an accepted standalone quote.
5. Reconcile canonical source consumption, poster receipts and treasury income.
   Use durable attempt/payment records to prevent duplicate revenue or released
   exposure across partial consumption, replacements, restarts and reorgs.

Start with standalone-funded groups and permit additional groups only when the
finished batch remains covered. For mixed-direction candidates, count shared
posting cost once and each qualified payment once. A simple bounded operating
tariff and risk allowance can precede fine-grained proving-cost allocation.
Supported nested execution still needs complete cost attribution; arbitrary
new protocol shapes or exact future-cost prediction are not launch promises.

The L2-first release needs estimator/SDK integration, Composer admission and
dispatch policy, and receipt-based treasury accounting. Fixed-refund builder
adapters additionally gate the later L1 collection release. Sponsorship
combinations, pooled L1 refunds, automatic customer
rebates, fair allocation among all users and native settlement-gas accounting
are later improvements, not prerequisites. Optional upside cannot make an
otherwise unfunded candidate eligible.

Launch evidence must establish three things: the integrated wallet flow supplies
the quoted fees and required submission route with one signature; actual
refunds and L2 treasury receipts reconcile to full batch costs on representative
supported workloads; and bounded failure funding plus replay/reconciliation
remain correct under partial consumption, stale prices and retries. For L1,
qualify the native blob post, exact target, source selector, net payout and
replacement behavior at every receiving builder, then measure the qualified
set's production coverage. API documentation or ACKs alone do not pass that gate.
Keep reserves for unpaid preparation, paid failed posts and already owed work;
the policy aims at sustainable pricing with bounded downside, not zero loss on
every possible execution outcome.

### Implementation readiness — 2026-10-14

The selected design is ready for staged implementation, beginning with accounting
and shadow quotes. It is not yet a complete wire/storage contract or evidence
that either paid route is ready to enable. Preserve the draft status until the
implementation scope is accepted; this assessment does not authorize deployment.

The following details must be specified with the first affected implementation,
before another component depends on them:

| Boundary | Contract still to make concrete |
| --- | --- |
| Dashboard/SDK quote and status | Versioned request/response and structured error shapes, units, intent/context binding, expiry and funding-blocked status. Use the selected application-service integration; a new wallet RPC is optional. Dashboard owns wallet setup and submission routing, with integration tests. |
| Durable financial reservations | Storage schema and version, atomic reservation/write ordering before preparation or submission, idempotent event identities, reorg reversal and exact release/recovery rules. A crash or unavailable ledger cannot authorize new unreserved spending in enforced mode; accepted-work ownership stays with the existing queue contract. |
| Operator policy | Concrete setting names, bounds, defaults and precedence for quote/work limits, markup, treasury/asset conversion and aggregate/group risk budgets. Absent policy remains accounting-only; enforced mode requires valid funding and qualified collection. |
| L1 adapter | Exact request/response mapping and enabled capability profile for each route, selected-source semantics, payout attribution and failure handling. Real native-blob inclusion/payment tests determine eligibility; an API ACK does not. |

These complete implementation boundaries around the chosen fee mechanism; they
do not require a new execution fee rule or another customer authorization.
Define their acceptance tests before enabling the dependent behavior. Existing
source-cost observations and fee-cap eligibility are useful starting points,
not an already implemented cross-chain revenue gate or durable finance ledger.

Calibration and external qualification remain separate launch requirements:
measured estimator error and preparation cost, funded loss/working-capital
budgets, verified realization of L2 revenue, receipt-backed L1 refunds, and a
qualified builder set meeting an explicit coverage target and observation
window. No numeric production tariff, reserve size or achieved majority coverage
is implied by the design review. Shared L1 batches retain their ordered-payer
qualification gate. The first code slice need not wait for these deployment
facts, but enforced operation does.

### First development increment: L2 receipt accounting

The first implementation changes the optional standalone settlement indexer's
[L2 fee collector](../../monitoring/indexer/src/l2_fees.rs). It records native
type `0x76` gas at zero price, ordinary receipt gas and fees, and gross priority
payments grouped by the actual block beneficiary. All receipts still participate
in inclusion and cumulative/block-gas validation; native prices must be zero.
Reverted ordinary transactions remain fee-paying. Unsupported or incomplete
evidence stays unavailable rather than becoming zero revenue.

Enrichment version 6 requeues older records; fee-checkpoint version 1 requires a
fresh bounded scan before exposing native gas and beneficiary attribution.
Canonical revalidation, finalized caching and restart/reorg behavior remain in
the existing collector. The [indexer contract](../../monitoring/indexer/README.md#accounting-boundaries)
and fee-accounting documentation describe compatibility and rollback limits.

This is part of stage 0, not completion of its full economics criterion. Gross
tips still include operator-funded transactions and beneficiaries whose control
is unverified. L1 redemption, failed/prepared-attempt cost reconciliation, shadow
quotes, durable financial reservations and admission/dispatch enforcement remain
to implement. The observer does not authorize posting or own requests. No node
execution, fee fields, protocol commitments or deployment settings change in
this increment.

### Second development increment: Composer shadow fee evidence

This slice pairs final L1 preflight with native L2 terminal-block receipts,
using the existing execution paths. It observes complete prepared candidates;
it is not yet an unsigned-intent estimator or a quote a wallet can sign against.
Admission, signed user bytes, execution, proof validation and request ownership
remain unchanged. No tariff or conversion rate is inferred from token names.

| ID | Observation contract | Acceptance check |
| --- | --- | --- |
| SF1 | Meter ordinary L2 fees from cumulative native receipts and the final block's base fee; retain native system gas at zero price. Match every ordinary source to the whole-group settlement manifest. | Receipt/prefix equivalence, fee-cap clipping, reverted fees, native callbacks, missing/duplicate source and mismatched endpoint regressions. |
| SF2 | Price only the poster's complete final L1 transaction and its physical blobs using the bound target fee quote; keep signed-cap balance reservation separate. | Companions cannot inflate poster cost; exact transaction/parent checks and arithmetic boundaries; process comparison with canonical receipts. |
| SF3 | Preserve chain units, beneficiary, signed source identities, exact L2 block and L1 target. Gross tips are speculative and unqualified. Historical ordinary fees, realization, proving, operations, risk and profit are not silently zero. | Separate unavailable/unpriced values; bounded per-source records; no funding verdict or required tip until a pricing policy exists. |
| SF4 | Accounting failure changes observability only. | Inconsistent inputs produce an unavailable observation; existing submission, reconciliation and source semantics continue in process tests. |

The additive `fee_observation` version 1 object belongs to existing `preflight`
admission and `repriced` posting records. Per-source `source_preflight` records
identify their execution chain and basis; L2 records now contain charged native
receipt gas, signed limit, sender, base-fee burn and gross priority payment.
Pre-refund required gas stays unavailable for those receipt-only observations.
The aggregate L1 scope is the complete post, including shared historical DA;
the L2 scope is only the terminal Sync block. No per-user allocation of shared
cost, canonical revenue, cross-chain subtraction or profitability is claimed.

These are bounded additions to existing event kinds. Current collectors preserve
the JSON details without a schema migration; historical missing fields remain
unknown. Older Composers omit them on rollback. There is no protocol, RPC,
genesis, queue-storage or configuration migration, and no deployment is part of
this increment. Signed wallet quotes, qualified income, durable reservations and
funding enforcement remain later increments.

### Third development increment: shadow pricing policy

Authorized for implementation on 2026-10-14; validation is recorded in the
[research audit](../audits/2026-10-10-cross-chain-fee-research/README.md). This
increment calculates economics for an already prepared candidate. It does not
provide an unsigned-intent estimator, wallet quote or funding enforcement.

An optional `EEZ_L2_FEE_SHADOW_POLICY` JSON object supplies explicit network,
beneficiary, conversion and service-cost assumptions. Absent policy retains
unpriced accounting; empty, incomplete or invalid configuration fails startup.
The runtime snapshot is immutable until restart. Costs are L1 native base units;
the conversion is a positive rational number of L1 base units per L2 base unit,
already net of any proportional realization haircut. No parity is inferred.

| ID | Contract | Acceptance check |
| --- | --- | --- |
| SP1 | Bind the policy to the actual L1/L2 chain IDs, rollup and configured nonzero beneficiary. Bound its validity by the exact L1 target timestamp. | Absent/empty/malformed/unknown settings, conflicting identity, expiry and configuration propagation tests. |
| SP2 | Add explicit per-post proving, operations and treasury costs to complete final L1 posting/blob cost. Calculate risk and profit allowances independently on that subtotal, rounding up. Convert required payment up and projected recovery down with checked integer arithmetic. | Independent integer examples, rounding and overflow boundaries; receipts establish posting cost. |
| SP3 | Count only terminal-source effective tips to the expected beneficiary. Exclude the poster, beneficiary and explicitly listed operator senders. Exclusions cannot prove that remaining senders are externally funded. | Fee-cap clipping, native zero-fee gas, excluded senders and wrong-beneficiary cases. Base-fee burns and historical ordinary fees never count as income. |
| SP4 | Compare full candidate cost with projected L2 recovery. Report a candidate-specific minimum tip only for one eligible L2 source and no L1 companions; shared/mixed candidates have no standalone per-user tip. | Multi-source/mixed cases, lower charged gas, zero eligible gas and nested source process tests. |
| SP5 | Pricing failures affect diagnostics only. Persist assumptions and policy identity with the existing events; qualified revenue remains unknown. | Underfunded candidates still execute in process tests; missing/expired pricing never authorizes admission, spending, eviction or release. |

The minimum tip divides the required L2 payment by native receipt gas for the
source, rounding up. This is a measurement for the exact current signed bytes
and execution context. Changed fees can change execution; a future pre-signing
quote must simulate those fields and final dispatch must validate again. Single
source pricing includes the complete post's shared/history cost; it never
assumes another customer will arrive. Aggregate comparisons do not allocate
cost among users or guarantee recovery under partial consumption.

The owning [operator contract](../configurable-settlement-policy.md#shadow-l2-fee-pricing)
defines the JSON fields, compatibility and output. Production tariff calibration,
verified treasury realization, canonical financial reconciliation and durable
reservations remain prerequisites for enforcement.

### Fourth development increment: unsigned advisory quotes

Authorized by the user's continuation after shadow pricing. Add an application
HTTP endpoint on the existing L2 front and a dependency-free JavaScript client.
The endpoint accepts an unsigned type-2 contract-call intent and returns ordinary
wallet transaction fields. This is an advisory estimate; it creates no queue
entry, reservation, proof, signature or submission. The existing shadow policy
is the explicit opt-in and supplies the conversion/service assumptions.

The initial profile requires an explicit sender, chain ID, nonce, gas limit,
destination, value and calldata. Access lists, authorization lists, blob sources
and contract creation are outside this quote profile. Unsupported execution
must fail explicitly. Keep the gas limit fixed while pricing: changing it or
the fee fields can change execution. The current source gas must come from a
native L2 replay of the complete generated group, never the intercepted
composition gas or the signed limit.

| ID | Contract | Acceptance check |
| --- | --- | --- |
| UQ1 | Pin source state and L1 target/fees; preserve the complete supported nested group and use isolated composition overlays. | Unsigned/signed equivalence, nested process case, no sender nonce/queue/state change from quoting. |
| UQ2 | Report native source gas separately from estimated complete L1 posting work, encoded DA, fixed costs and conversion. Include retained ordinary history; refuse unavailable/unsupported history. | Independent receipt comparisons and DA/gas boundary cases; no fabricated native L1 receipt or treasury income. |
| UQ3 | Re-simulate proposed fee fields in a bounded iteration and return only a covered estimate for those exact fields. | Fee-dependent execution, rounding, nonconvergence, wrong chain and malformed inputs. |
| UQ4 | Bound input, frames, history, iterations and concurrency; blocking work owns its permit until exit even after timeout/disconnect. | Controlled worker lifecycle, busy/timeout errors and isolated local process tests. |
| UQ5 | Bind the response to the intent/context/policy, expose expiry and standard type-2 fields; the SDK validates them before an ordinary wallet submission. | Client mismatch/expiry/chain tests and one-signature process flow. No wallet-specific routing claim. |

The owning operator contract records the HTTP schema, bounds and compatibility.
L1 posting remains a conservative projection until final signed-bundle preflight.
An advisory quote cannot replace the later strict admission/dispatch gate,
treasury qualification or durable financial ledger.

### Fifth development increment: L2 tariff integration and Chiado trial

Authorized on 2026-10-14: complete the application flow, build local Docker
images, validate in an isolated Kurtosis network, then upgrade the existing
Chiado trial after the validation gates pass. The release is based on the
running recovery revision `3f5bc03fcf9383c6b425dbe16d859395c28ede59`, retaining
protocol `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, recovery fixes, wallet gas
estimation and Composer-only RPC routing. The earlier fee checkout is preserved.

The trial enforces a configured **L2 tariff**. This is distinct from the later
production financial-reservation contract: configured conversion remains an
assumption, and a Chiado test cannot qualify asset realization or zero loss.
Durable financial reservations and qualified revenue remain explicit production
requirements, rather than inferred properties of transaction-tip checks.

- `EEZ_L2_FEE_ENFORCE_TARIFF` is absent/false by default and requires the complete
  existing pricing policy when true. Empty or invalid values fail startup.
- Before accepting a new outbound transaction, recover its signer and replay
  its exact signed fee fields and fixed gas limit. Reject a projected payment
  shortfall before queue ownership transfers. A quote ID or prior HTTP request
  is not required. Existing accepted duplicates retain their ownership.
- Check native source receipts and the complete final L1 posting cost again
  before dispatch. Unavailable pricing or insufficient coverage defers accepted
  work without charging a user retry or releasing its nonce. Normal canonical
  reconciliation remains authoritative. Unsupported source profiles fail closed.
- Recheck cumulative native payments against the growing outbound prefix before proof
  work. A price or state change defers only that source's nonce chain and restores
  the accepted prefix, allowing independent sources to proceed. Shared-batch
  discounts and future-nonce quotes are outside this initial profile. With the
  gate enabled, settle L1-origin users in separate batches under existing FIFO
  ordering; their uncollected fees cannot underfund an outbound candidate.
- The application obtains a quote immediately before its ordinary transaction
  request, preserves all quoted fields for both injected and local signers, and
  applies a visible fee ceiling. Wallet edits or approval delays still require
  final admission validation. No second signature or payment is introduced.
- L1-origin and ordinary L2 traffic retain their existing admission behavior;
  this incremental L2 trial does not activate the proposed L1 paid route.

Acceptance requires nonzero negative tests for underpricing and cap clipping,
positive receipt-backed payment and canonical settlement, supported nested and
multi-source traffic, ordinary/L1 regression checks, restart/reconciliation and
independent replay. Run the exact candidate images in Kurtosis, retain image and
binary identities, and preserve stopped live databases/configuration before
the existing-rollup handover. Do not redeploy contracts or regenerate genesis.

Completed evidence is recorded in the
[L2 integration and Chiado trial audit](../audits/2026-10-14-l2-fee-integration/README.md).
The deployed trial uses 1:1 base-unit conversion, a 20% risk allowance and zero
fixed proving, operations, treasury or profit charges. Those are test assumptions,
not qualified production prices. The audit distinguishes exact-image private
network tests, actual public-network settlement, independent replay and mocked
wallet tests. L1-origin fee collection and durable financial reservations remain
outside this release.

### Accepted correction: full source transaction gas range

Gas-profile correction authorized on 2026-10-15: admit source limits up to the
active EIP-7825 maximum, 16,777,216, in quotes, signed tariff admission and the
SDK. The former application-only 5,000,000 cap excluded recorded Falcon Safe
transactions with gas limits of 6,050,247 and 6,037,754. Keep the other simulation
work bounds, fee calculation and final native validation. This changes no
protocol, storage or genesis rules. Older servers/SDK copies still reject these
higher limits; upgrade every client copy and Composer. Rollback requires draining
accepted work and restoring the older client limit. The user authorized this
small correction's expedited deployment before the broader post-deploy tests.

### Accepted correction: measured L1 settlement pricing

Authorized on 2026-10-15 after the bridge fee reconciliation: replace the generic
admission gas reserve in L2 quotes and tariff admission with isolated execution
of the complete outbound L1 posting. Charge its receipt gas and physical blob
cost, then apply the existing service/conversion policy and 20% trial risk
allowance once. Gas-limit headroom remains a capacity setting, not a user fee.
Ordinary L2 settlement subsidies and final signed-bundle validation are unchanged.

Unsigned intents cannot have an authenticated settlement proof or final signed
L2 block hashes. The estimate therefore uses private root/blob/signature
placeholders, executes the recognized deployed verifier's full ECDSA path, and
assumes acceptance only for the exact independently computed proof-call digest.
This assumption is restricted to disposable estimation state and recognized
deployment bytecode; it produces no signature, proof, queue ownership or posting
artifact. Unknown deployment code and mismatched, skipped or reverted effects
fail closed. The later signer and final native preflight retain every real check.
The response identifies this estimation basis and proof assumption explicitly.

The same measured pricing must reach unsigned quotes, signed ingress and the
growing multi-user prefix check. Keep whole nested groups, state pinning,
work/cancellation bounds, exact wallet fields and separate L1/L2 meters. Do not
reduce the admission reserves to conceal the quote error. Proof/root placeholder
byte differences and later state/price changes remain estimate uncertainty;
there is no automatic rebate in this iteration.

Acceptance: reproduce the old quote, compare new quotes with actual complete
posting receipts (including blobs), verify the policy margin arithmetic, reject
underpayment, and cover legacy/type-2, nested calls, first-time proxy creation,
repeated bridges and multi-user batching. Validate real candidate Docker images
in a fresh Kurtosis enclave with semantic checks and independent replay before
the authorized Chiado handover. Upgrade the SDK/dashboard to accept the new
basis before Composer; retain acceptance of older advisory quotes during rollout.
No protocol, genesis, signing RPC or persistent storage migration is required.
Rollback to older Composer restores its excessive tariff and can reject newer
lower-fee transactions; drain accepted work before handover.

Implemented and deployed on 2026-10-15. The
[measured settlement audit](../audits/2026-10-15-measured-l1-fee-quotes/README.md)
records receipt comparisons, candidate Docker tests, independent replay and the
Chiado handover. The repeated bridge's public quote fell from 0.0013997604 to
0.0003768348 xDAI; the separate live nested-call canary collected 1.2000945 times
its actual complete posting cost. These are bounded trial observations, not a
guarantee of identical gas or prices for future transactions or shared batches.

The follow-up [Safe fee review](../audits/2026-10-14-safe-fee-recovery/README.md)
checks actual Safe 1.4.1 factory and nested initializer execution. Deployment on
L1 from L2 belongs to the enforced outbound path; deployment entirely on L2
retains ordinary gas rules and has no enforced posting/blob contribution. This
scope boundary must not be described as universal L2 fee recovery.

Accepted scope clarification on 2026-10-15: settlement of ordinary L2 traffic
may be operator-subsidized in this iteration. These users still pay normal L2
execution fees; no mandatory settlement tip or surcharge is required. The
ordinary-L2 behavior above is therefore accepted, not an unmet acceptance
criterion. L2-to-L1 transactions, including nested calls, must still satisfy
the outbound tariff. This clarification does not change the existing
whole-post pricing or final dispatch checks.

### Sixth development increment: legacy transactions and JSON-RPC quotes

Requested on 2026-10-15: support legacy wallet transactions and expose the quote
through the Composer JSON-RPC interface. Implemented and locally validated;
see the [audit](../audits/2026-10-15-legacy-fee-quotes/README.md) for evidence and
remaining live-wallet/deployment limits. No protocol or fee-collection rule
changes are required.

- Add `eez_quoteL2ToL1Fee` on the L2 Composer front with positional params
  `[intent]`, sharing the HTTP endpoint's engine, response schema, readiness,
  work limits and cancellation. Retain the existing HTTP route for clients.
- Admit explicit type `0x0` intents and EIP-155-protected signed legacy sources.
  Keep type `0x2` support. Legacy quotes return `gasPrice`, calculated from the
  simulated L2 base fee plus the required per-gas contribution; only the
  difference from the actual base fee counts as collected priority revenue.
  Preserve the actual envelope type in simulation and DA sizing. Reject
  replay-unprotected legacy sources and unsupported fee-field combinations.
- Extend the SDK and dashboard to request a legacy quote for Rabby's custom
  network path. A dApp calls the custom quote method directly on the Composer;
  the wallet still receives only standard transaction RPCs. Preserve the fee
  ceiling, exact intent and one-signature flow for either transaction type.
- Retain signed admission, cumulative composition and final receipt checks.
  A higher base fee reduces a legacy transaction's contribution; recheck it
  rather than counting the whole `gasPrice` or the gas limit as revenue.

Acceptance: reproduce the current legacy admission rejection, then exercise
legacy and type-2 quotes/submission, nested canonical settlement and independent
replay; reject underfunding and replay-unprotected sources; check RPC transport,
IDs, errors, disabled/front-only placement and shared limits; test SDK and
dashboard fee preservation. Real wallet-extension behavior requires separate
validation from mocked wallet requests. Upgrade the node before clients select
legacy quotes or the new RPC; rollback clients before an older node. Historical
transaction decoding and follower replay already support legacy transactions;
no database or genesis migration is involved.

### Wallet transport and accepted-work behavior

For the proposed integrated wallet flow, the wallet's source-chain submission
RPC must be the corresponding EEZ front or another explicitly qualified route
into it. The dApp calls the wallet's ordinary `eth_sendTransaction`; the wallet
signs locally and sends the resulting `eth_sendRawTransaction` to that route.
Supplying an HTTP quote URL or changing an SDK's read provider does not reroute
the wallet's broadcast. Do not assume generic wallets export signed bytes via
`eth_signTransaction`. One-time network/RPC configuration is a setup prerequisite,
not a second fee authorization; its actual support remains a per-wallet launch
test. The [MetaMask send flow](https://docs.metamask.io/metamask-connect/evm/guides/send-transactions/)
establishes the wallet request shape, not EEZ transport compatibility.

The existing [front](../../crates/eez-node/src/ingress.rs) intercepts raw
submission but forwards other ordinary methods. Retain its current routing:
calls classified as cross-chain are held, ordinary transactions and creations
are forwarded upstream. Today an inconclusive classification enters the held
admission path rather than leaking a suspected cross-chain transaction. Preserve
that behavior when pricing is disabled/accounting-only. In the proposed enforced
mode, reject a new unclassifiable request with a diagnostic before ownership is
accepted, because its service funding cannot be validated; never broadcast it
as ordinary work. Previously accepted requests retain their owner. This is an
explicit versioned admission-policy change, not a claim about current runtime
behavior. An ordinary successor at
nonce `n+1` can remain pending upstream behind held source `n`; the front does
not take ownership of that ordinary transaction or privately compose it.

Use the existing [HeldPool replacement rule](../../crates/eez-composer/src/held_pool.rs):
duplicate hashes are idempotent, a queued same-nonce cross-chain source can be
replaced only after both signed fees satisfy the current bump rule and the new
funding check, and an in-flight nonce cannot be replaced through that path.
Ordinary same-nonce cancellations/transfers follow the ordinary forwarding path;
receipt of them is not canonical cancellation of the held source. Preserve old
attempt/payment history and release financial exposure only with its required
evidence, even if local queued ownership is explicitly replaced.

For the minimal v1 SDK, allow one unsettled application submission per sender
and source chain, display its held/blocked status, and offer explicit ordinary
replacement only where the ownership state permits it. This is a client flow
restriction, not a new protocol nonce rule or ownership of other applications'
transactions. Read actual source nonce/status again after any external conflict;
do not claim an upstream-only pending nonce includes privately held reservations.
General multi-pending wallet support needs a reservation-aware nonce view and
separate qualification. A held status is not a fabricated canonical receipt.
Test wallet fee edits, private routing, resend, ordinary successors and queued/
in-flight replacement/cancellation. Detection of a source in the public mempool
is a route-risk signal and can stop new speculative spending; it is not canonical
nonce consumption and does not by itself release request ownership.

Distinguish quote validity from signed-transaction lifetime. The quote has a
source/L1 context, a price envelope, a short admission validity window and a
bounded preparation budget. Admission checks signed bytes against current costs
and reserves permitted preparation/failure exposure before expensive work. V1
does not underwrite a new candidate's projected service-revenue shortfall:
customer recovery must still pass the economic dispatch test. Operator capital
funds bounded preparation, realized failures and already-required settlement;
it is not service revenue. A reserve also cannot make an ordinary transaction
eligible when its signed fee cap is below the source base fee or its gas limit
is insufficient.

Accepted work retains the pinned ownership rules. A lapsed quote, exhausted
financial budget or maximum service waiting window stops further unfunded
preparation/dispatch and exposes a funding-blocked status; it does not authorize
dropping the source or treating its nonce as spent. Existing attempts and their
financial reservations still reconcile canonically. Signed Ethereum transactions
do not gain an expiry from our quote, and deleting a local copy cannot revoke
copies elsewhere. An optional ordinary replacement/cancellation requires another
wallet signature. Therefore v1 promises one ordinary signature for the normal
funded path, not guaranteed completion by a deadline with no possible fee
replacement. No separate service-payment signature or debit is introduced.

An accepted but unposted L1 inbound request is owned pending demand, not by
that fact a canonical obligation to post: it may remain funding-blocked. Work
already required by the settlement policy/canonical history has its own funded
backlog obligation. Retain ownership of both, but do not conflate their reserve
budgets or let the price of new demand stop a post already owed.

Qualification starts with one complete source group per candidate in each
direction, including supported nested work. Shared L1 batches remain an optional
v1 capability only after ordered tests demonstrate the selected payer's actual
receipt gas and net refund still cover the batch. Earlier sources can deploy a
proxy or change state and reduce the payer's later gas; EVM access warmth does
not carry between separate transactions. Unselected L1 tips remain builder
income and must be disclosed in the quote before signing as user expense, never
treasury revenue. Proxy creation/state changes that reduce the selected payer's
gas are named shared-batch qualification cases. This
preserves the user's accepted conservative batching trade-off without making
multi-user qualification a prerequisite for the first single-group trial.

## What the current fork establishes

| Path | Who pays and receives | Consequence |
| --- | --- | --- |
| L1-origin source transaction, included after the post | User pays its L1 receipt charge; execution priority payment goes to the L1 block beneficiary | Its gas estimate/tip is not poster revenue |
| L1 `postAndVerifyBatch` transaction | Poster pays execution plus native blob charges | Includes protocol framing/verification/storage and any work executed inside that transaction |
| L2-origin ordinary signed transaction | User pays ordinary L2 execution fees; tip goes to the L2 header beneficiary | Collectable only when the beneficiary belongs to the fee treasury and the transaction is canonically retained |
| Native L2 system transactions, type `0x76` | Metered execution with zero gas price, no priority fee | Incoming L2 work does not automatically produce user fee revenue; budget its compute/DA costs explicitly |
| Bridged call value and pooled system inventory | Protocol value accounting | Principal transfers are neither service revenue nor operating expense |

The [fronts](../../crates/eez-node/src/ingress.rs) currently forward ordinary
`eth_*` methods and intercept raw transaction submission. `gate_and_hold`
checks source affordability and nonce continuity; it has no cross-chain
cost-recovery test. `HeldTx` retaining signed fee caps does not enforce a
minimum service payment. In this baseline there is no transaction-specific
cross-chain price RPC. Availability of `eth_estimateGas` on both fronts must
not be interpreted as an estimate of the poster's total bill. If the deployed
estimator is from another revision, identify it and validate that integration
before implementation.

Composition already records source and target execution work. However,
[`compose_crosschain`](../../crates/eez-composer/src/composer.rs) explicitly
distinguishes intercepted source simulation from a canonical receipt estimate.
The [inspector](../../crates/eez-evm-inspector/src/inspector.rs) returns a
synthetic proxy outcome. Price from the complete materialized execution,
including the actual tables, transaction order, storage state and blob payload;
do not add two endpoint estimates and assume that equals a receipt charge.

[`EEZ_L2_FEE_RECIPIENT`](../../crates/eez-composer/src/composer/config.rs) already
sets Sync-block coinbase; [node wiring](../../crates/eez-node/src/lib.rs) also
applies it to Live blocks. The parser's absent value is zero; the Chiado Compose
profile requires an explicit value and permits explicit zero, as documented in
the [deployment runbook](../runbooks/chiado-deployment.md) and
[configuration checks](../../scripts/ci/tests/test_chiado_config.py). Verify actual
header beneficiaries and treasury control before counting tips. This research
does not assert the operator's deployed value.

The [posting fee selector](../../crates/eez-l1/src/posting_fees.rs) already prices
the poster's transaction with bounded target-chain fee history and separate
execution/blob caps. Its priority setting is an **expense**, not the user's
service-price setting. Preserve the [feedback-loop correction](../audits/2026-09-18-priority-fee-feedback-loop.md).

[`CostReport::amounts`](../../crates/eez-composer/src/composer/costs.rs) separates
poster and companion charges. Use `post_total_cost` for the current poster
transaction, not `bundle_total_cost`: companions are normally user-funded.
The [indexer](../../monitoring/indexer/README.md) has canonical L1 cost
and L2 tip/burn reporting, but `L2 tips − L1 total` alone is not net business profit
or liquid L1 cash. It does not establish treasury ownership, redemption,
refund income, proving cost, or outstanding liability coverage.

At the research baseline, [`block_fees`](../../monitoring/indexer/src/l2_fees.rs)
subtracted base fee from every receipt's effective price without a native-type
branch. The [native envelope](../../crates/eez-primitives/src/lib.rs) returns zero
effective price, so a native receipt in a positive-base-fee block took the
indexer's `L2 effective price below base fee` error path. The
[first increment](#first-development-increment-l2-receipt-accounting) reproduced
and fixed that failure: native receipts contribute gas with zero monetary
burn/tip, while ordinary below-base prices still fail validation. The
[regression](../../monitoring/indexer/tests/l2_fees.rs) now covers the mixed
ordinary/native case. This is a local fix and reproduction, not a live diagnosis
or evidence that the deployed dashboard was upgraded.

## Requirements and acceptance

These are proposed requirements; accepted protocol and queue contracts remain
authoritative until an implementation is approved.

| ID | Required behavior | Acceptance check |
| --- | --- | --- |
| F1 | Separate payer, chain, asset and beneficiary for every cost/revenue item | Reconcile receipts, treasury balances, transfers and refunds without counting user-funded gas or bridged principal twice |
| F2 | Charge the whole authorized source group and its real settlement contribution | Nested/static/reverted/multi-root cases; no missing table, blob, framing or compute cost |
| F3 | Enforce effective payment, not an advertised tip cap | High priority cap/low max fee, lower actual gas, stale quote and price-spike cases cannot pass with insufficient coverage |
| F4 | Preserve canonical ownership and partial-consumption semantics | Omitted sources retain ownership; no duplicate financial claim across retry, replacement, restart or reorg |
| F5 | Dispatch has an explicit funding source for both normal cost and uncovered downside | No post-only/partial outcome creates an unreserved poster liability |
| F6 | Refund-dependent traffic only uses verified compatible routes | Missing, ignored, delayed, reduced, wrong-recipient and wrong-transaction refunds fail eligibility or consume an explicit reserve |
| F7 | Respect the ordinary transaction's signed fee limits | No change to signed user bytes, value or caps; quotes expire; over-cap work waits or needs an ordinary replacement transaction |
| F8 | Underpriced new demand cannot stop settlement already owed | Independent eligible work progresses; mandatory backlog has its own funded reserve |
| F9 | Financial reconciliation survives loss of the volatile request queue | Rebuild exposure and builder receivables from durable attempt records and canonical evidence; never recognize one payment twice |
| F10 | Recovery works after conversion and operating costs | Show contribution margin, full operating margin and L1 liquidity separately, including periods with no refunds |
| F11 | Preserve the required L1 wallet flow | One normal application transaction; no separate customer payment or authorization; builder authentication uses operator keys |

## Pricing model

All calculations use checked integer arithmetic in native base units, rounded
up for required payments. Display gwei only at the UI boundary. Work from the
same L1 parent/target used for proving and submission, and the actual L2 draft.

For an ordinary EIP-1559 source transaction:

```text
effective_tip = min(maxPriorityFeePerGas, maxFeePerGas - source_base_fee)
collected_tip = receipt_gas_used * effective_tip
```

A negative fee headroom makes the transaction ineligible. Legacy/access-list
transactions use `gasPrice - source_base_fee` under their existing admission
rules. A larger gas limit does not itself increase payment; unused gas is not
charged. Neither a larger `maxFeePerGas` nor a larger base fee guarantees more
treasury income. These distinctions follow [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559).

For batch `B`, define:

```text
C_chain(B) = sum over poster-funded L1 transactions j of
             (receipt_gas_j * effective_gas_price_j
              + blob_gas_j * blob_gas_price_j)

C_service(B) = C_chain(B) + C_compute(B) + C_treasury(B) + C_risk(B)
Q(B)         = ceil(C_service(B) * (1 + markup))

R(B) = spendable value of treasury-owned canonical L2 user tips
       + received builder refunds attributable to B
       + received builder sponsorship attributable to B
```

`C_compute` covers measured composition, witness/proving and service overhead;
separate fixed operating costs for full-business reporting. `C_treasury` covers
redemption/bridging, conversion, payout transactions and liquidity carrying
cost. `C_risk` is a calibrated allowance for failed postings and unpaid
preparation, not permission for unlimited retries. Markup is a cost markup,
not a percentage of revenue. No markup or rebate percentage is prescribed
without measurement.

For forecasts use final ordered simulation gas and target prices; for accounts
use canonical receipt costs. Keep **gas needed to complete** distinct from
receipt gas after refunds and any calldata floor. Never subtract EVM storage
refunds a second time from receipt gas. Blob charges cover complete blobs,
including unused capacity, and are separate from execution gas. Obtain fork
pricing from the active network rules rather than freezing old blob limits.
[EIP-4844](https://eips.ethereum.org/EIPS/eip-4844) describes the separate blob
charge, including payment on a reverted blob transaction.

### Estimate nested source groups from complete execution

This subsection records the 2026-10-11 design follow-up. Its full native-post
forecast remains a proposal; the [initial advisory quote profile](#fourth-development-increment-unsigned-advisory-quotes)
implements native L2 replay with projected L1 posting cost. The pricing unit is the complete ordinary
source transaction and all its cross-chain effects. It can contain several
sequential root calls, nested callbacks, static lookups and caught reverts.
Keep that entire group intact, with its source identity and nonce/root
dependencies. Nesting depth alone is not a cost metric.

For an L1-origin flow `L1 A -> L2 B -> L1 C -> L2 D`, the pinned execution model
puts the callback C inside the user's L1 companion transaction. The L2 work B
and D belongs to its materialized L2 delivery; PostBatch separately publishes
and verifies the required data. The callback is not a second independently
paying L1 transaction. For an L2-origin flow, L1 application work can instead
execute inside PostBatch and is then poster-funded. See the pinned
[nested examples and no-splitting rule](../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md).
Multiple sequential roots can require multiple delivery units; do not infer
one delivery transaction for every complete source group.

Maintain three separate outputs:

| Output | Purpose | Basis |
| --- | --- | --- |
| Required gas limit | Allow each actual transaction to finish | Pre-refund execution and forwarding headroom, validated at the final limit |
| Expected charge | Forecast the payer's monetary spend | Native simulated receipt gas, effective execution price and physical blob charge |
| Service contribution | Recover operator costs and margin | Allocated poster cost, off-chain resources, treasury cost and bounded risk |

An enclosing native transaction's gas already contains its nested execution.
Do not sum inclusive parent/child trace totals or opcode gas deltas, which can
include forwarded gas. Attribute frames for explanation, but reconcile to the
whole native transaction result. Conversely, Composer's intercepted simulation
returns cross-chain call gas to its caller: its source meter omits real queue,
proxy and hash work. Its separate L1-target meter is useful composition evidence,
not a replacement for materialized receipt gas. These are different meters,
not interchangeable implementations of the same sum.

Use the following estimation sequence:

1. **Compose the whole group in one state context.** Reuse the production
   cross-chain simulator and overlay/re-entry handling, preserving caller,
   value, access list, static flags, fee fields, ordering and revert scopes.
   Bind the estimate to the source-chain state, L1 parent/target, L2 draft and
   assumed block environment. Retain every attempted supported frame, including
   caught failures. Unsupported or ambiguous protocol/signer shapes must not
   receive an unconditional quote.
2. **Materialize the real payload.** Build execution/static tables, root updates,
   native deliveries, PostBatch calldata and the actual native blob encoding.
   Count encoded calls, return/revert data and framing, not just user calldata.
   The pinned encoding carries 126,976 useful bytes per physical blob; charge
   the encoder's whole blob count, including padding and the next-blob boundary.
   A nesting multiplier or a calldata-byte average cannot capture these steps.
3. **Measure the ordered native execution.** The authoritative L1 forecast is
   the exact signed `[PostBatch, source A, source B, ...]` candidate with real
   proof bytes, fees and gas limits. Validate the corresponding complete L2
   execution too. This captures actual queue scans, proxy creation, storage
   transitions, within-transaction access warmth, calldata floors, refunds and
   callbacks in the transaction that pays for them. Warm access state does not
   carry between separate Ethereum transactions. Rebuild when ordering or state
   changes; a valid simulation is conditional on its environment, not a promise
   about an arbitrary builder prefix or future block.
4. **Convert resources to money and allocate.** Compute operator-funded L1 gas
   and blobs at target prices. Budget measured composition/witness/proving work
   and service overhead separately; native zero-price L2 gas is useful resource
   evidence, not an L1 fee. Use the deployed proof system's real workload model
   or invoice. Neither one proof per nested call nor a universal conversion
   from EVM gas to proving cost is justified. Keep bridged principal separate.
5. **Reconcile after inclusion.** Match native predictions to canonical receipt
   charges and actual off-chain spend. Keep successful, failed and partially
   consumed attempts distinct. Reorg-aware ownership and cumulative risk
   accounting still apply; lower eventual success cost does not erase an
   earlier paid post or free preparation work.

The selected gas limit must itself pass execution: the
[EIP-150 forwarding rule](https://eips.ethereum.org/EIPS/eip-150) can require
headroom above consumed gas. Unused headroom is not an extra fee. Account for
the active [calldata floor](https://eips.ethereum.org/EIPS/eip-7623) and gas
refunds through the native executor, rather than applying another refund to
its receipt result. Static lookup alternatives deserve explicit coverage:
the [pinned static specification](../../eez-core-protocol/docs/STATIC_ENTRY.md)
requires gas for every attempted candidate and its callbacks, even when only
one result is selected. Reverting effects do not make that work free.

The early wallet quote cannot generally use the final signed, proved candidate.
Use composition measurements, encoded sizes and calibrated conservative protocol
and proof allowances for that stage, identifying the basis as provisional.
Prefer a conservative sparse-batch quote initially; apply batching discounts
only to funded work already available under the allocation policy below.
After signing and proving, rerun native execution and the economic coverage
test before dispatch. Repricing fee fields can change balances, `GASPRICE` and
execution, so iterate within a bounded simulation budget. If signed caps no
longer cover the candidate, defer or request an ordinary replacement rather
than changing signed bytes. A replacement is another ordinary wallet signature,
not a separate service-payment authorization; it is an exception to the normal
one-signature path, never an automatic amendment of an accepted source.

The source's standard fee payment is the collection vehicle, not an additional
line item alongside an identical service debit. For L1 refund collection,
first calculate the operator contribution, then invert the qualified builder
route's payout rule to suggest an ordinary effective tip. Use predicted native
source receipt gas, never its declared limit or summed remote gas, as the
payment basis; recheck realizable revenue for the final candidate. A large
admission reserve can be conservative for block capacity yet overstate revenue
if multiplied by a signed tip. Maintain a separate downside reserve.

Current implementation already supplies
[composition L1-target measurements](../../crates/eez-composer/src/composer.rs),
[final ordered native gas results](../../crates/eez-composer/src/composer/posting.rs),
[separate expected cost and balance reservation](../../crates/eez-l1/src/posting_fees.rs)
and [poster/companion receipt separation](../../crates/eez-composer/src/composer/costs.rs).
The [admission models](../../crates/eez-composer/src/composer/gas.rs) use bounded
shape evidence and fall back to a companion's signed limit for other shapes;
that fallback is not a measured bill. The first implementation milestone is
shadow per-group quotes and batch-cost reconciliation using these existing
meters, followed by admission enforcement once measured errors and collection
coverage justify it. Compare shallow/deep and multi-root groups, caught reverts,
static alternatives, fresh/reused proxies, sparse/full batches and blob
boundaries. Derive allowances from these measurements, without claiming old
audit samples validate the current pin.

### Allocate costs without relying on future arrivals

Price whole source-transaction groups, retaining all nested/replay dependencies.
For each selected group allocate: directly attributable poster execution;
encoded DA contribution; a share of shared verification/framing/storage and
unused blob space; compute; treasury cost; and risk. Normalize rounded shares
so they cover the complete candidate cost, not just its marginal additions.
An L1 callback inside a user-funded companion is user gas; the same work inside
the poster's transaction is poster gas. Execution location decides attribution.

First identify the post that would already be due without new cross-chain work
at this target. Assign its fixed costs to the funded ordinary/backlog budget.
Assign new groups their incremental execution, DA, compute and risk, including
any extra blob or post they cause. If no post was otherwise due, the selected
groups fund its shared fixed cost. Keep one allocation ledger whose shares sum
to the whole cost; do not also bill the backlog for the same overhead. The
baseline must use the actual trigger/state, not a hypothetical profitable post.

For an initial quote, assume the group must fund its assigned posting overhead
without hypothetical future users. Any later discount using co-admitted payers
requires qualified projected customer recovery to cover the full candidate;
the operator reserve covers failure exposure after dispatch, not a known
pre-dispatch revenue shortfall.
A cheaper batch tier can wait within a disclosed deadline until enough funded
work exists. A priority tier covers the sparse batch.
Byte weights use the native encoding; blob-boundary rounding is included.
Removing any candidate requires recomputing coverage and deferring/requoting
if the remaining customer recovery is insufficient. Signed tip fields cannot
be changed in either direction by the Composer. A lower shared allocation does
not automatically lower a signed tip payment: offer a batch discount before
signing, or a separately specified funded rebate. The initial tip-only product
discloses an estimated fee and signed spending limits, without promising a fixed
monetary charge or a post-assembly discount.
Marginal-only pricing of a partly filled blob does not fund the next
blob or the fixed post.

Assign ordinary L2 history its share and record the funding already collected
for it. A new cross-chain user must not unknowingly pay the entire accumulated
ordinary backlog. Conversely, the same historical tips cannot fund two future
posts. A mandatory settlement of already accepted history may be loss-making;
fund it from the backlog reserve and stop accepting additional underfunded work.
Measure the idle policy cost floor separately: number of obligatory postings
over a representative interval multiplied by their measured minimum cost.
Neither a high tip on an occasional user nor stopping new admission eliminates
the existing settlement obligation.

### Two dispatch tests

1. **Economic coverage:** projected realizable revenue from this exact candidate
   covers its new-demand share of `Q(B)`. With no already-required backlog, that
   is all of `Q(B)`. In a mixed backlog/new candidate, fix the actual already-due
   baseline first: new groups' qualified recovery covers their allocated shares,
   and only the pre-identified backlog budget covers the backlog share. The
   allocations cover the entire candidate once; new incremental execution,
   blobs and service costs cannot be hidden in that baseline. Exclude
   discretionary future refunds and unidentified assets.
   V1 reserve draws cannot make this predicate pass: they are capital, not
   customer revenue. Already-required ordinary/backlog work uses its separately
   allocated funding policy; it is not a new underpriced cross-chain candidate.
2. **Downside funding:** unencumbered operator risk reserve covers the spend that
   can remain after partial/failed inclusion. The poster's liquid L1 balance
   covers maximum upfront amounts of its signed transactions and other still-live
   attempts. A sponsored route may instead supply the verified upfront shortfall
   before execution; it still requires separate bounds for unsupported costs and
   failure exposure.
   Reserve every group's frozen share of bounded failure exposure, as well as
   the global total, before the corresponding work/dispatch; discovering a
   depleted group budget after a loss is too late to authorize that expenditure.

A profitable forecast is not a worst-case guarantee. Include the post-only
outcome where applicable. Existing skipped-slot replacements can overlap in
submission history: financial reservations stay allocated while a late original
can still consume them, following canonical reconciliation. Timeout is not a
financial release event. Never invent a cancellation guarantee from a relay ACK.

Maintain per-group **cumulative** attributed spend and outstanding exposure
across attempts, in addition to the global limits. L2 tip revenue from the same
signed source can be recognized only once. Before another posting, its uncovered
cost must fit the group's remaining allocated risk budget and the global reserve;
otherwise retain/defer/requote. Entries being published is distinct from source
completion. Infrastructure costs do not authorize an extra debit to a customer;
one eventual source tip cannot fund repeated posts without limit. The existing
retry-count limit remains an additional bound.

Calibrate preparation and retry allowances for the actual qualified route set.
Measure attempted targets, landed bundles, time to inclusion and compute cost
per preparation. A builder's observed block share is not our bundle's landing
probability: its retained bid and candidate compatibility also matter. Repeated
proofs on missed exact targets consume compute even when no L1 transaction is
included. A stationary independent-attempt model may use `1 / p` as a planning
approximation, but v1 admission uses a finite monetary/work budget, not that
expectation as a guarantee. Reserve priced preparation work before starting it;
charge actual compute to the financial budget independently of whether the
existing protocol retry counter is chargeable. Exhaustion blocks new spending
without changing the closed retry-status list or discarding accepted sources.

For a shared attempt with no validated fault attribution, freeze cost-allocation
weights before work starts. Use the groups' attributed service-cost shares,
including a separately funded ordinary/backlog share when present; use exact
integer apportionment with a stable FIFO tie-break so allocations sum to the
observed shared cost. Record each allocation against its group's cumulative
operator-funded preparation/failure budget and the global budget. This is
financial participation accounting, not blame, a protocol retry-counter charge,
or another customer debit. Weights cannot be changed after seeing who failed.
Missing allocation data charges the global unassigned-loss budget and blocks
reuse of the affected reservations until reconciled; it never records zero cost.

After an inconclusive multi-group failure, the next funded attempt may use a
smaller eligible prefix or independently eligible single-source groups while
preserving whole nested/replay groups and all nonce/state dependencies. This
selection can limit correlated exposure; it does not authorize eviction, imply
fault, reset spent budgets, or bypass the economic gate. Dependent groups that
cannot safely separate remain deferred together. Single-group qualification
does not require this shared-attempt allocation machinery to be advertised as
already tested.

## L2 origin: collect a transaction-specific priority fee

Let `q_i` be the group's required contribution in L1 units; `x` the conservatively
realizable L1 value per L2 native unit; and `g_i` its ordinary source transaction's
predicted receipt gas in the **final materialized block**. The suggested fee is:

```text
required_tip_i = ceil(q_i / (x * g_i))
suggested_maxPriorityFeePerGas = max(required_tip_i, ordinary_priority_floor)
required_maxFeePerGas >= bounded_L2_base_fee + suggested_maxPriorityFeePerGas
```

The formula is a quote, not a proof of future gas consumption. Do not divide by
the gas limit, a remote-chain estimate, or native system transaction gas.
Re-execute with the selected signed fee fields: `GASPRICE`, balances, beneficiary
changes, and state-dependent paths can affect the result. Revalidate the actual
`g_i * effective_tip_i` for the final candidate; if rebuilding changes coverage,
reprice or defer. Bound quote/simulation iterations and resources; oscillating
or unsupported traces do not receive an unconditional quote.

For a conservative guarantee over a defined outcome set, use a proved lower
bound on collectable source gas and applicable effective tip, or require prepaid
coverage of the shortfall. An estimate multiplied by a safety percentage is
not a lower bound. Native replay groups being indivisible helps match L2 fees
to retained work, but does not pay for every failed L1 post or skipped entry.

Choose an operator-controlled nonzero `EEZ_L2_FEE_RECIPIENT`; account for each
canonical block's actual beneficiary. Exclude tips from operator self-transfers
or sponsored transactions from external customer revenue. High recovery tips
must not automatically drive the ordinary L2 gas oracle upward; use the
transaction-specific quote and separately measure any effect on wallet defaults.

This is a Composer pricing policy using ordinary transaction fees, not a new
consensus gas rule. Standard `eth_estimateGas` keeps returning execution gas.
No artificial gas burning or gas-limit inflation is needed. Raising/reassigning
the L2 base fee would change the current execution/replay contract and is not
part of the first release.

### Low-traffic L2-to-L1 options — 2026-10-14

The user selected a simpler L1-to-L2 starting point: one selected user's fixed
percentage refund funds the complete batch when its net payout is sufficient.
For L2-to-L1, direct ordinary priority-fee collection is simpler still. A
controlled L2 beneficiary receives every included ordinary user's effective
priority payment; no L1 builder refund selector or percentage is needed for
that revenue. L1 builders still receive the poster's ordinary L1 fees.

At the inspected fork, ordinary L2 transactions delegate to Ethereum execution
in [`EezEvm::transact_raw`](../../crates/eez-evm/src/evm.rs). The beneficiary is
configured through
[`EEZ_L2_FEE_RECIPIENT`](../../crates/eez-composer/src/composer/config.rs) and
passed to both Live and Sync production. An absent bare-process setting uses
the zero address; the Chiado Compose example requires an explicit setting.
This inspection does not establish which beneficiary a running deployment uses.
Base-fee burns and native zero-price system execution are not customer revenue.

Keep collection and pricing separate:

| Option | Collection and pricing | Scope and trade-off |
| --- | --- | --- |
| Ordinary tips with a batch coverage gate | Collect all external user tips in the selected L2 execution; admit the batch when their conservatively realizable value covers its complete cost and margin | Recommended initial mode; no new transaction format, contract debit or user signature. Precise per-user shares can wait. |
| Transaction-specific outbound quote | Use the same tip collection, with a suggested source tip derived from the complete source group's expected contribution | Better targeting for sparse batches and heterogeneous nested work; adds quote lifecycle and pre-signing estimation. |
| Protocol-level settlement fee | Define a native fee component collected during ordinary L2 execution, potentially with a treasury/vault recipient | Future execution-rule change. Requires deterministic metering, fee bounds, replay/prover support, wallet estimation, activation and historical compatibility. |

For v1, combine the first two rows: ordinary L2 tip collection with a batch
coverage gate, and transaction-specific pricing supplied through the dApp/SDK.
The user clarified this preferred client-integration direction on 2026-10-14;
the protocol-level fee remains exploratory future work.

The first two options share the existing fee mechanism. A quote can accompany
the initial batch gate without implementing fair allocation among all users:
start with a conservative standalone contribution and later offer funded
batch discounts. A fixed per-transaction contribution is viable only for a
bounded workload class whose complete cost fits that amount. A single fixed
priority rate is not a general L1-cost estimator: the same L2 gas can drive
very different L1 execution, data and nested work.

For the initial gate, evaluate:

```text
tip_i = min(maxPriorityFeePerGas_i, maxFeePerGas_i - L2_base_fee)
predicted_L2_revenue = sum(source_receipt_gas_i * tip_i)
conservative_L1_value(predicted_L2_revenue)
    >= operator_batch_cost + risk_allowance + profit
```

Reject fee-ineligible transactions before applying the formula. Use the native
L2 draft's ordinary source receipts, not cross-chain interception meters,
declared gas limits or the L1 target's gas. Account for the actual beneficiary.
Do not select only the largest payer: summing already available qualifying
tips requires no extra builder support. Under the user's simplified policy,
one large payer may cover other groups, but adding any group must still pass
the complete cost and capacity checks. Preserve whole nested groups and source
dependencies. Predictive coverage is not a guarantee of all future outcomes.

Charge the batch once in mixed-direction operation: sum qualifying L2 tip
revenue and the qualified single-source L1 refund against one shared cost
ledger. Historical L2 tips may contribute only through an identifiable,
unspent backlog allocation. Count no source fee twice and no operator-funded
treasury transaction as external revenue. A speculative L2 receipt is not
settled income; recognize only retained canonical execution and reconcile
partial consumption, replacements and reorgs under the existing ownership
contract.

For example, suppose a standalone group's required contribution, including
its assigned operating/risk allowance and profit, is 0.003 L1 ETH. If its
native source receipt is predicted to use 100,000 L2 gas and collected L2 ETH
has conservatively realizable 1:1 L1 value after accounted treasury costs,
the required effective priority rate is 30 gwei. Its signed execution fee cap
must also accommodate the L2 base fee. This is illustrative arithmetic, not a
measured price. A lower final receipt charge can reduce revenue, so the final
candidate must pass the gate again.

The existing fronts forward ordinary `eth_*` estimation and do not provide
the proposed service quote. A wallet/dapp must apply an outbound-specific
fee recommendation before the normal transaction signature when its default
tip is insufficient. A global `eth_maxPriorityFeePerGas` response cannot
express each transaction's remote work. Keep standard gas-limit estimation
separate; retain the signed bytes after submission.

L2 tips accrue on L2, while the L1 poster needs liquid L1 funds. Verify the
deployment's asset backing/redemption, budget the treasury sweep and its delay,
and reserve L1 working capital before posting. A native token's label alone
does not establish 1:1 redemption. These requirements remain even though
builder refund support is no longer a prerequisite for collecting L2 tips.

Protocol fee components are an established design category: the
[OP Stack fee documentation](https://docs.optimism.io/op-stack/transactions/fees)
describes L1 data and operator fees alongside execution fees. That implementation
is not present in this fork, and its data-size pricing alone would not meter
RollupX's arbitrary L1 application execution. The current fork's base-fee rule
is fixed in the [signer profile](../../crates/eez-proof-signer/SPEC.md) and ordinary
execution burns base fees. Raising the base fee does not redirect that money
to the poster. A future fee-rule change needs a separate accepted design;
it is not an alternative operator configuration for the initial policy.

## L1 origin: collect through builders without another user payment

### Distinguish three different refunds

| Mechanism | Economic meaning | Admission treatment |
| --- | --- | --- |
| EVM storage gas refund | Reduces charged execution gas subject to fork rules | Already included in receipt accounting |
| Fixed builder rebate | Returns an agreed portion of selected user priority/coinbase payment | Eligible only after route semantics, recipient, timing and downside funding are validated |
| Variable builder redistribution or backrun surplus | Revenue depends on builder outcome and orderflow value | Zero forecast contribution; recognize verified receipts once |

BuilderNet documents fixed `refundPercent`/`refundRecipient`, a
`refundTxHashes` selection limited to one transaction (default: last), optional
delayed payout, and a distinct redistribution identity. It also documents refund
queries and recipient delegation. These are separate from authenticating a
bundle. [BuilderNet API](https://buildernet.org/docs/api)

Its redistribution rule depends on orderflow contribution and retained block
value; public-mempool transactions do not qualify under the documented rule.
It supplies no fixed minimum return for our batch. [BuilderNet refunds](https://buildernet.org/docs/refunds)
Flashbots Protect separately documents exclusivity effects and its own recipient
defaults; do not apply those defaults to our `eth_sendBundle` integration.
[Protect refunds](https://docs.flashbots.net/flashbots-protect/gas-fee-refunds)

Titan documents a selected-transaction rebate and automatic payout, with small
refunds able to make a bundle uneconomic. Its page mentions `refundIndex`, while
its hash example includes `refund_tx_hashes`. Subsequent
[public API probes](../audits/2026-10-10-cross-chain-fee-research/sponsorship/titan-refund-cardinality-probes.json)
resolve the cardinality question: both mainnet and Hoodi reject two camel-case
`refundTxHashes` with `bundle 'refund_tx_hashes' count cannot exceed 1`.
The camel-case field is type-checked; a malformed snake-case field had no
observable effect in these controls. Actual source eligibility and payout
deductions still require an inclusion test. The displayed arithmetic mixes
gas-price and total-payment units; calculate from integer wei ourselves.
Its displayed request does not establish support for our exact `maxTimestamp`
constraint. [Titan API](https://docs.titanbuilder.xyz/api/eth_sendbundle)

These sources establish possible Ethereum integrations. No reviewed source
establishes fixed rebates on the configured Chiado/Gnosis builder. Capability
is **unverified**, not inferred from accepting the method name.

### Fixed rebate pricing

For one eligible selected source transaction, let `r` be the verified rebate
fraction, `H` a conservative bound on payout deductions, and `q_i` its service
contribution. Let `W_user` be the verified rebate base funded by that customer,
excluding poster-funded or unrelated payments. In general the external revenue
is `r * W_user - H`. The following specialization requires a route whose base
is exactly that source's priority payment and which deducts `H` after `r`:

```text
net_rebate = r * source_receipt_gas * effective_L1_tip - H
required_effective_L1_tip = ceil((q_i + H) / (r * source_receipt_gas))
```

Adapt the equation to the provider's verified deduction order. The user also
pays the L1 base fee; that payment is not rebated to our treasury. A rebate of
the poster's own priority payment only reduces an expense: increasing that bid
to get some back does not create external revenue.
Report that recovered poster payment as an expense rebate, separately from
customer revenue. In the net-margin equation include it once against gross
cost, never also subtract it from cost. Reject refund-based pricing when the
provider cannot establish the selected customer-funded base; marginal-value
redistribution cannot substitute for that base.

Independently check builder economics after the rebate: remaining user tips
and poster tips must make the entire bundle
competitive, including its payout transaction. A fixed rebate does not make
blockspace free. Choose the rebate fraction by testing cost and inclusion
tradeoffs; an arbitrary 99% can remove the inclusion incentive.

The present [`bundle_request`](../../crates/eez-l1/src/submitter.rs) contains
transactions, exact block/timestamp constraints and optional reverting hashes;
it does not request fixed rebates. Existing `EEZ_L1_BUNDLE_SIGNING_KEY` supplies
authentication only. A future adapter must name the refundable user transaction
explicitly and send proceeds to the designated L1 treasury. Avoid accidental
rebates from the final unrelated companion or from the post itself.

BuilderNet's documented one-selected-transaction restriction prevents summing
all users' tips as that route's rebate base. The separate Titan public API
probes now establish the same one-hash limit on its tested endpoints.
A single-source rebate can fund a shared batch only if that
source's refund alone covers the required cost or the remainder is independently
funded. The common compatibility mode below permits that concentrated charge
only within the payer's disclosed ordinary transaction quote. It is not the
shared-cost mode; giving every user a high tip does not pool those refunds.
Qualify the multi-user alternatives below before accepting pooled tip revenue.
Do not reorder dependency-bearing sources, repeat the post, or split replay
groups to work around an API. Measure lost batching efficiency if choosing
one independently posted user per bundle.

Current relay fan-out sends identical payloads and one ACK starts observation.
For rebate-funded work, initially use only a homogeneous, verified eligible
relay set. Do not fall back to an unqualified relay or public mempool. Supporting
different provider request formats later requires an explicit change to the
fan-out contract, preserving signed transactions, order and exact target.
Track each relay's bundle identity and payout semantics. Defaulting an unknown
capability to supported is unsafe even if the relay silently ignores fields.

`revertingTxHashes` semantics differ by provider, and the fork deliberately
supports partial consumption. A refund payer that can be omitted cannot fund
unconditional post costs. Disabling omission can improve the builder contract,
but it does not add an on-chain atomic payment guarantee. Builder nonpayment,
reorg and partial-inclusion exposure therefore require an explicit operator
reserve. Delayed rebates remain receivables until paid.

### Multi-user L1 batches and Titan qualification

Keep one shared post followed by its ordered source companions. Several paying
users need one aggregate payment to the treasury, not separate payouts to each
user. The selection of the revenue base is distinct from the payout recipient.
One `refundRecipient` therefore does not prevent batching; a rebate calculated
from only one user's tip does prevent counting the others as poster revenue.

| Option | How multiple users fund the shared post | Qualification or tradeoff |
| --- | --- | --- |
| Pooled fixed rebate | Builder returns an agreed share of the sum of selected customer tips to one treasury | Preferred tip-based shape where supported. Titan's tested mainnet/Hoodi `eth_sendBundle` validators explicitly reject more than one refund hash |
| Titan sponsored bundle | Builder prepends funding for the post sender and recovers its outlay from the complete bundle | Candidate for gas recovery; compatibility with our blob post, balances and exact target remains untested |
| Sponsor plus a selected-source rebate | Aggregate bundle tips fund posting gas; one selected source funds a rebate for remaining service costs | Unverified combination; must cover the non-gas quote and leave positive builder return without counting a tip twice |
| One selected source for the entire batch | The selected user's rebate alone pays for all users' post | Proposed launch route under the subsequently accepted higher ordinary-tip quotes; does not pool the other users' tips |

Prepaid credit, separate payment transactions and additional account/permit
authorizations are excluded by the user's wallet constraint. Raw EOA signatures
cannot simply be merged into a new aggregate transaction.

Titan's current API links its sponsored-bundle explanation: an insufficiently
funded sender can receive a prepended ETH transfer when the bundle repays the
builder. A funded sender proceeds without that transfer. This is published
behavior, not validation of our workload. [Titan sponsorship explanation](https://titanbuilder.substack.com/p/titan-tech-teatime-1)

Proposed application: `[builder funding, post, user_1, ..., user_n]`. This could
recover posting gas from aggregate tips without multiple refund selectors.
It does not automatically pay our proving/operations margin or reimburse an
already funded poster. Model actual top-ups, remaining balances and subsequent
posts; testing one empty account is insufficient. Keep bridged principal
separate. Verify execution and blob upfront requirements, fee caps, post value,
and validity after the prepended funding transaction. Never lower gas caps or
weaken signer checks to induce sponsorship. Count a received sponsor transfer
against gross poster expense once, not as both revenue and an avoided expense.
Builder acceptance still depends on its net balance after funding and any
rebates; the same user tip cannot fully fund both.

The local experiment passed seven checks: zero balance, execution-only funding
and a one-wei reserve shortfall are rejected; a wrong-slot post reverts and
still pays blob fees; explicit sponsor funding works for an initial batch and
one using the residual balance; a fully funded poster pays its own costs without
a top-up. The positive cases execute the pinned registry, a physical EIP-7594
blob, and three synthetic L1 companions in the same block. The mock verifier
checks public-input equality, not an L2 execution proof. Funding was supplied by
our local harness, not Titan or Quasar. This narrows the unknowns without
qualifying a production route.

The source blocker is concrete: [`PostingRuntime::finalize`](../../crates/eez-composer/src/composer/posting.rs)
reads the poster balance at the selected parent, subtracts the full blob fee-cap
reserve and caps executable gas by the remaining funds. It then executes
`[post, companions...]` against that parent state. Its caller returns before
dispatch on a finalization error. With an adequately funded poster, the
documented automatic sponsorship trigger no longer applies. A production mode
must model the permitted funding prefix consistently through composition and
final execution, retain full gas/blob affordability checks, and preserve the
post's identity and source ordering. If an application reads balances affected
by the prefix, that state change must agree with the proven execution.

Public `eth_callBundle` and `mev_simBundle` probes returned `-32601` from Titan
mainnet, Titan Hoodi and Quasar mainnet. Their documented stats APIs inspect
bundles after submission; these results do not provide a dry-run sponsorship
test. Before qualifying a
route, obtain actual builder-created funding and receipt evidence for the
native-blob workload. The test also does not demonstrate recurring recovery of
proving/operations costs; gas sponsorship remains distinct from that revenue.

A sponsor-plus-refund experiment can test whether a selected source also funds
the remaining compute/operations quote. This is a proposed combination, not
documented interoperability. Require the selected refund to cover that remainder
and total tips to cover sponsorship, payout overhead and the builder's retained
bid. A larger batch can have ample total tips but no individual source large
enough for the required single-source rebate. Do not silently move everybody's
cost to the last user to make the request fit.

There is also a separate API candidate: Titan announced `mev_sendBundle`
support in 2023. Its current supported profile has not been established here.
The MEV-Share specification has nested bundles and refund allocations, but
`refundConfig` distributes a refund; it is not a list of transactions whose
tips are automatically summed. A concrete aggregation request needs verified
refund-base semantics and target constraints. Flashbots' current endpoint also
documents a one-backrun-transaction limit, so generic nesting is not evidence
that this multi-user shape works through that endpoint or directly at Titan.
[Titan support statement](https://github.com/flashbots/dowg/pull/1)
[MEV-Share specification](https://github.com/flashbots/mev-share/blob/main/specs/bundles/v0.1.md)
[Flashbots endpoint](https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint)

Follow-up empty-parameter `mev_sendBundle` probes returned `-32601 Method not
found` at both public Titan and Quasar mainnet endpoints, despite the older
Titan announcement and Quasar's current schema page. This documentation/endpoint
disagreement remains unresolved; no valid bundle or authenticated alternative
was tested. It is not evidence that every provider route lacks this capability.

### Sponsorship and refund portability

Rechecked public documentation on **2026-10-11**. Automatic builder-funded
sponsorship plus a fixed refund is **not an established common capability
across the major builders**. Titan documents gas/value sponsorship and refunds;
Quasar documents gas sponsorship and refunds. Their simultaneous behavior on our
native-blob workload, especially using the sponsored post itself as the refund
source, remains unverified. The other reviewed providers document refunds but
do not establish automatic sponsorship. A community BuilderNet proposal is not
an accepted provider contract. See the dated
[support matrix and source URLs](../audits/2026-10-10-cross-chain-fee-research/sponsorship/combined-support-2026-10-11.json).

Keep three distinct funding profiles:

1. **Builder funds the post; refund a real user.** Candidate for Titan/Quasar.
   Aggregate external user payments must repay the builder's actual funding,
   payout overhead and retained bid, in addition to the selected refund. The
   refund can pay remaining operator expense and profit once sponsored posting
   is accounted for exactly once.
2. **Builder funds a high-tip post; refund that post to the treasury.** The
   conditional construction below can route aggregate user value through one
   selected source. It additionally requires sponsored-source eligibility and
   suitable gross/net accounting. It is not equivalent to refunding an
   operator-funded post's own tip.
3. **Operator funds the post; refund a real user.** The broad documented fallback.
   The selected refund must cover all operator-funded posting and non-gas costs
   plus margin. A prepended operator funding transaction, if used, is ordinary
   bundle execution, not automatic builder funding and not a new user payment.

Choose and validate the funding profile before dispatch. An underfunded post
cannot simply be sent unchanged to endpoints without established sponsorship.
Prefunding the poster in parent state also removes Titan's documented shortfall
trigger for that same post. Sending different provider-specific funding prefixes
requires explicit composition/finalization support and separately validated
cost/refund budgets; current identical-body fan-out does not implement this.

Qualify the combination with actual builder-created funding and payout, native
blob affordability, exact target/consumption evidence and at least two included
batches using the residual poster balance. No new funded test was performed in
this recheck. The earlier local harness supplied both builder transfers itself.

### Sponsored post tip as the one refund source

There is a second way to combine sponsorship and refunds: use the sponsored
post itself as the single refund source. The intended sequence is:

```text
builder funds poster -> post with quoted priority fee -> ordered users
                     -> builder pays post-tip refund to separate treasury
```

Users still pay only their ordinary signed transaction fees. If the builder
funds the post's execution and blob shortfall, permits that sponsored post as
the selected source, and refunds its gross priority payment, aggregate user
tips can fund both posting and our treasury receipt. This could avoid needing
several refund-source hashes or a separate operator value payment. It is a
conditional economic construction, not verified builder support.

[Titan](https://docs.titanbuilder.xyz/api/eth_sendbundle) and
[Quasar](https://docs.quasar.win/api/eth_sendbundle) document sponsorship and
refunds separately, with a maximum `refundPercent` of **99**, not 100. Neither
reviewed page explicitly defines their composition or the eligibility of a
sponsored transaction's priority payment. Quasar explicitly accepts one source
hash. Titan's subsequent API probes establish that camel-case `refundTxHashes`
is recognized and limited to one hash on both tested public endpoints, despite
the documentation's inconsistent selector spelling. A request selecting the
post's hash is therefore the concrete candidate; actual eligibility of that
sponsored post and the resulting payout remain unverified. Leaving selection
at the last-transaction default would select a user, not the post.

Let `U` be external user tips, `P` the post's actual priority payment, `F` the
builder funding transfer, `R` the treasury refund, `C` the post's execution-base
and blob fees, and `H` the builder's funding/refund transfer costs:

```text
operator balance change = F + R - (C + P)
builder balance change  = U + P - F - R - H

at a stable standing reserve, F = C + P:
operator balance change = R
builder balance change  = U - C - R - H
```

Thus the post's high tip is an accounting route to a requested payout; it is
not additional external income. The users collectively fund `C + R + H` and
the builder's retained bid. `R` can cover proving, operations, risk allowance
and profit because posting is already sponsored. Counting sponsored posting
again in `R` as an unpaid expense would overstate the required receipt. Initial
funding may also leave a working balance at the poster; track that separately
from recurring service income. Actual fee-cap shortfalls and changing receipt
gas require the full equations, not only this steady-state simplification.

The [local result](../audits/2026-10-10-cross-chain-fee-research/sponsorship/tip-refund-local-results.json)
contains 22 passing cases: the previous 15 plus seven controls for this
combination. In the repeat-batch case, three synthetic users pay `0.00509385 ETH`
in tips, the post consumes `0.001387386000131072 ETH` including its high tip,
and the manually supplied sponsor transfer covers that amount. Applying 99%
after a 21,000-gas refund deduction returns `0.0012873168 ETH` to the separate
treasury. The builder retains `0.003698467199868928 ETH` before its validator
bid. Subtracting an illustrative `0.0002 ETH` of non-gas expense leaves
`0.0010873168 ETH`. These are synthetic values, not a production quote or a
measured proving cost. Both builder transfers were signed by the harness.

The controls establish important accounting limits:

- Without sponsorship, refunding our own post tip reduces combined operator
  assets by `0.000100054800131072 ETH` before non-gas costs in this fixture.
- With the refund sent back to the poster, its next funding shortfall shrinks.
  The repeat batch has zero combined operator balance growth and no treasury
  receipt. A separate treasury, or an explicitly accounted withdrawal, is
  necessary to extract recurring income under this shortfall-funding policy.
- A counterfactual policy deducting the sponsor transfer from the selected
  post's refund base produces a zero refund. This tests policy sensitivity;
  it is not an observed Titan or Quasar rule. A
  [BuilderNet community proposal](https://collective.flashbots.net/t/proposal-builder-funded-gas-sponsorship-for-bundles/5820)
  also discusses netting sponsorship before refundable value, but is not an
  accepted specification and does not determine either provider's formula.
- An oversized signed gas reserve makes the first funding transfer unprofitable
  for the builder even though the transactions execute locally. Gas affordability
  and the builder's complete balance change must both pass; an EVM success does
  not establish admission or auction competitiveness.

This does not yet fit every major builder. Titan and Quasar are candidates
because both features are documented; the earlier observed 73.59% combined
share is neither verified combined-feature coverage nor a future inclusion
guarantee. A decisive provider test needs builder-created funding and a paid
refund on the actual selected native-blob post, followed by a second batch using
its residual balance. It must resolve source selection, gross/net accounting,
both transfer deductions, exact target and atomicity. The current Composer
finalization gate still rejects an underfunded post without a modeled funding
prefix. No such runtime integration, valid public bundle, or live payout has
been executed. Retain the accepted funded single-user-source launch proposal
while qualifying this potentially more efficient collection route separately.

### One aggregate operator payout

The Composer can calculate a single desired receipt for the whole batch:
posting plus blobs, proving/operations, risk allowance and profit margin.
The builder need only validate its own remaining economics. No per-user cost
allocation or separate recipient is required at the builder. The unresolved
constraint is how to express that receipt through an accepted payment mechanism.
The reviewed refund APIs expose percentages of eligible transaction payments;
they do not document an arbitrary absolute `refundAmount`. A larger percentage
cannot expand a one-transaction refund base. On a verified multi-source route,
select all contributing source hashes, derive the percentage from their total,
round within the API's bounds and check the net receipt after payout deductions.

There is a distinct **fixed-payment candidate with local execution evidence but
unverified builder acceptance**: Titan explicitly
documents sponsorship of transaction value as well as gas. We could include an
operator-signed transaction transferring the desired amount from a dedicated
underfunded operator account to the treasury. Keep the native post conventionally
funded initially, so the only requested sponsorship is for this payout account.
If Titan funds that value transfer, it would recover the outlay from the entire
bundle's user tips; the treasury receipt could cover both posting and margin.
Users still sign only their ordinary application transactions. This adds a
backend transaction and does not assume an undocumented refund-amount field.
[Titan sponsorship terms](https://docs.titanbuilder.xyz/api/eth_sendbundle)

Provider compatibility is an inference from the documented value-funding rule,
**not verified provider acceptance**. Quasar's published rule explicitly mentions gas, so
do not infer value funding there. The pinned
[`postAndVerifyBatch`](../../eez-core-protocol/src/EEZ.sol) is nonpayable: attaching
the desired receipt as its `value` would revert. A separate operator payout
requires explicit composition and final-simulation support, exact target and
source ordering, and reconciliation of the payout account's existing/residual
balance. No operator-funded value, funding transfer or treasury receipt may be
counted twice as external revenue. Qualify the permitted payout purpose, value
limits, atomicity and repeat-batch economics before using it in quotes. The
earlier Hoodi gas-only trial does not test this candidate.

The expanded [local experiment](../audits/2026-10-10-cross-chain-fee-research/sponsorship/value-local-results.json)
passed fifteen checks, including the original seven gas-only controls. A
conventionally funded native-blob post, three users and a separate `0.0024 ETH`
operator payout execute successfully when our harness supplies real funding
for the payout sender. The second batch retains the first batch's balance and
still yields a positive combined operator balance change after posting/payout
fees. This is synthetic L1 execution with a mock proof; the harness supplies
the funding, not Titan. It does not establish production cost or margin levels.

New controls reject a gas-only payout reserve and a one-wei shortfall, and show
that adding value directly to the nonpayable post reverts while the same post
with zero value succeeds at the same parent. A prefunded payout sends ETH to
the treasury but reduces total operator assets by on-chain costs: it is not
external revenue. An excessive payout is EVM-valid with our supplied funding
but makes the builder lose money; a profitable builder would have to decline
that candidate. No real builder admission decision was exercised.

This refines the gas-only conclusion: accepted sponsorship of an explicit
operator value transfer could support recurring profit. For admission, compute
the required external funding after accounting for the payout account's balance,
then reconcile all operator accounts and expenses. Keep the payout compulsory
in a strict bundle; without sufficient funding it cannot execute. That is a
useful funding check, not a consensus guarantee of whole-bundle atomicity or
builder compliance. The plain EOA payment is not independently bound to the
post's target, so production expiry/replay and attempt ownership need validation.

Prepared a separate [Hoodi value-payout trial](../audits/2026-10-10-cross-chain-fee-research/sponsorship/hoodi-value-trial.json)
with a `0.06` test-ETH cap. Its three cases exercise an empty payout sender,
its residual balance, and a prefunded self-payment control, while the native
poster stays funded. It is not executed because there is no funded disposable
test setup. Actual Titan acceptance, Quasar value support, broad builder
compatibility and production integration remain open. No public transaction or
production change was made.

For a verified pooled rebate with the same deduction order as the earlier
equation, the final candidate must satisfy:

```text
T = sum(source_receipt_gas_i * effective_L1_tip_i)
net_batch_rebate = r * T - H_batch
net_batch_rebate >= Q(batch)
```

Only selected external customer payments enter `T`; exclude poster payments.
Allocate `Q(batch)` into contributions `q_i` using attributable work and an
explicit share of common post/blob/operations costs. Allocate the single payout
cost into `h_i`, with `sum(h_i) = H_batch`. Suggested individual tips are then
`ceil((q_i + h_i) / (r * credited_source_gas_i))`. Use conservative credited gas
and revalidate the complete fee-bearing execution before dispatch. A common
flat tip is not fair when users have different remote costs or source gas.

Illustrative equal-work batch: ten users each pay a 4 gwei effective tip on
100,000 charged source gas. Total external tips are `0.004 ETH`. An 80% pooled
rebate, less one `0.0002 ETH` payout deduction, yields `0.003 ETH` to cover the
batch service quote. Users separately pay source base fees. If only one source
is selected, the same deduction model yields just `0.00012 ETH`; the other
nine tips are not poster revenue. These are hypothetical terms and arithmetic,
not a Titan payout prediction or a current gas quote.

Offer immediate execution priced for the funded candidate, or a batching tier
that waits for enough signed contributions until a stated expiry. Do not quote
a ten-user overhead share and dispatch one user without reserved coverage.
If a contributor becomes invalid, recompose and rerun both coverage and
downside checks. Already signed priority fees cannot be lowered by the poster;
returning realized batching savings needs an explicit funded refund mechanism
or an ordinary replacement transaction. For the proposed wallet flow, a later
discount must be an operator-funded refund that needs no user action; do not
introduce customer credit or an extra signature. Preserve whole replay groups
and nonce dependencies.

The next bounded integration experiment should establish:

1. On a route that supports multiple sources, whether a three-user bundle with
   distinct tips can return their combined priority payments to one treasury;
   compare one and several sources and measure actual payout. Titan's tested
   public `eth_sendBundle` endpoints already reject several hashes, so this
   experiment there requires a provider extension or another qualified route.
2. Whether sponsorship works for the actual native-blob post at the same exact
   L1 target, with underfunded, funded and residual-balance senders. Reconcile
   funding, charged execution/blob fees, principal, rebates and builder return.
   Separately test simultaneous sponsorship and a selected-source rebate before
   assuming this can cover compute/operations as well as posting gas.
3. Behavior when a contributing source fails, is permitted to revert/drop, or
   disappears during replacement. No omitted payer may back unconditional
   cost without separately reserved coverage; reorgs and late originals must
   not double-credit revenue or release live exposure.
4. A provider adapter that preserves signed bytes, order and both target
   constraints. Unknown refund fields must fail eligibility rather than be
   silently accepted as evidence of recovery.

For the later efficient cost-sharing mode, prefer a qualified pooled rebate;
evaluate sponsorship as a separate gas-funding mode. The funded single-source
refund below remains the v1 route. The broader compatibility study identifies
the remaining builder integrations. Do not split
a shared post into repeated `[post, user_i]` bundles and assume a builder will
deduplicate the post while
preserving all payment conditions. No such behavior was established.

### Broad builder compatibility

The user requires a solution usable with a big majority of builders. Proposed
qualification threshold: at least 90% of recent observed MEV-Boost block
production, targeting 95% or more. Weight coverage by unique blocks, rather
than endpoint count; this is a proposed threshold, not an accepted numerical
requirement. BuilderNet regions and a forwarding Flashbots endpoint do not
create additional independent builder share.

The [dated coverage snapshot](../audits/2026-10-10-cross-chain-fee-research/builder-coverage.json)
contains the global seven-day builder table fetched from
[Relayscan](https://www.relayscan.io/overview?t=7d), published at
2026-10-10 09:26:35 UTC: 46,023 observed blocks. Its `extra_data` attribution
was not independently authenticated against builder keys and canonical hashes.
This sample measures delivered MEV-Boost blocks, not all L1 slots, and does not
predict whether our particular bundle lands. Coverage figures below are
documentation candidates; none has passed the proposed live workload checks.

| Builder / primary documentation | Observed 7d share | Multiple user tips in one fixed refund | Automatic builder-funded gas sponsorship |
| --- | ---: | --- | --- |
| [Titan](https://docs.titanbuilder.xyz/api/eth_sendbundle) | 52.40% | Mainnet/Hoodi probes explicitly reject more than 1 hash | Documented, conditional on insufficient funds and builder recovery |
| [Quasar](https://docs.quasar.win/api/eth_sendbundle) | 21.19% | `refundTxHashes` explicitly max 1 | Documented, conditional on insufficient funds and builder recovery |
| [BuilderNet](https://buildernet.org/docs/api) | 16.20% | Explicitly max 1 | No deployed support established from reviewed sources |
| [Eureka](https://docs.eurekalabs.xyz/order-flow/api/eth_sendbundle.md) | 4.57% | Documented selection of 1–16 unique hashes, selected coinbase-profit basis | Not established |
| [Bombora](https://bombora.build/docs/eth-sendBundle/) | 3.16% | Explicit combined profit of selected transactions | Not established |
| [BTCS Builder+](https://www.btcs.com/builder-docs/) | 1.49% | Hash array documented; cardinality and aggregation require confirmation | Not established |
| [Ultra Sound builder](https://github.com/ultrasoundmoney/builder-docs/blob/main/bundle-rpc.md) | 0.23% | Explicitly max 1 | Not established |
| [Beaverbuild](https://beaverbuild.org/docs.html) | 0.11% | More than one hash explicitly rejected | Not established |

Quasar's general description of bundle rewards must be read with its specific
one-hash constraint. A July 2026 community
[BuilderNet sponsorship proposal](https://collective.flashbots.net/t/proposal-builder-funded-gas-sponsorship-for-bundles/5820)
does not establish acceptance or deployment. No current rsync/Loki support is
credited: their primary documentation could not be retrieved, and registration
lists are not evidence of a deployed payment contract. Missing documentation
does not prove a feature is absent.

The data defines the integration gap under the user's wallet constraint:

- The two clearly documented pooled-refund candidates cover **7.74%**.
- Titan plus Quasar, the documented sponsorship candidates, cover **73.59%**.
- Combining these modes across the four builders reaches **81.33%**, before
  accounting for their different funding, margin and submission requirements.
  This is a candidate union, not a validated shared implementation.
- The top five builders cover **97.53%** and the top six **99.02%**. All six
  document ordinary bundle submission. That alone does not transfer pooled user
  tips to the poster and therefore does not qualify them for this product.

The following payment flow describes the pooled/sponsored expansion studied
here. The funded single-source v1 flow is defined in
[First iteration](#first-iteration-shared-pricing-with-two-collection-routes)
and [Common funded single-source profile](#common-funded-single-source-profile):

1. Return a transaction-specific gas quote, including the priority fee needed
   for that user's allocated share of the post, blobs and service overhead.
   The quote is a read-only call. The user signs only the ordinary application
   transaction with its normal fee caps.
2. Assemble one `[post, user_1, ..., user_n]` candidate from complete replay
   groups. Recompute actual effective tips and costs for the exact target;
   dispatch only when qualified recovery covers the service quote and the
   operator reserve covers failure exposure.
3. For each eligible builder, request either a pooled fixed refund to the
   poster's treasury or a verified sponsorship arrangement covering the same
   economics. The operator signs any required bundle authentication. Preserve
   original user bytes, order, native blobs and both target constraints.
4. Require every endpoint receiving that candidate to satisfy the recovery
   check independently. An incompatible builder winning inclusion cannot be
   offset by sending the same batch to a compatible builder as well.
5. Reconcile canonical posting, source consumption, sponsor funding and refund
   receipts. Count each transfer once. Actual user charges remain the ordinary
   transaction's gas charges; there is no later customer debit.

For that pooled expansion, the minimum builder integration contract is: an explicit set of contributing
source hashes; a verified formula for their combined priority payments; a fixed
refund fraction or agreed net payment; a treasury recipient; bounded deductions;
and inclusion, omission, replacement and payout rules for the exact candidate.
This is our qualification requirement, not a new field already supported by all
builders. With refund-only adapters, require `min_b(net_rebate_b) >= Q(batch)`
over every receiving builder `b`; after the rebate, each builder also needs a
competitive retained bid. Adapters can differ without changing the wallet flow.

Funding modes also affect eligibility. A funded poster that works for a pooled
refund route does not trigger automatic shortfall sponsorship. Do not count
the 81.33% union as simultaneous compatible fan-out of one pre-funded post.
Qualify the actual balances and signed transactions of each mode before combining
their coverage, and fund remaining compute/operations separately within the
builder recovery arrangement, not with another user payment.

For broad initial compatibility, qualify the single-source profile below across
the major builders. For efficient multi-user batching, seek multi-source support
from Titan, Quasar and BuilderNet through a provider extension or another
qualified route. Eureka/Bombora provide documented aggregation candidates against
which to define the payment test. Adding a qualified
BuilderNet route to the four-builder candidate union would bring the dated
sample to 97.53%; neither this arithmetic nor standard bundle compatibility
establishes actual support. Keep the majority-coverage milestone unmet until
end-to-end payment evidence exists. A proposal, SDK field, or successful RPC ACK
cannot substitute for that evidence. No provider was contacted in this study.

Before claiming the coverage target, qualify the actual blob post, target
height/timestamp, bundle-size limits, replacements, omitted/reverted sources
and fee settlement at each endpoint. Repeat the share calculation over aligned
recent windows with authenticated builder identities and canonical blocks;
deduplicate relay deliveries. Track receipt-backed compatibility separately
from declared support. One seven-day table is sufficient to prioritize work,
not to establish stable coverage or a posting deadline guarantee.

### Common payment profile and efficient batching

The required receipt is builder-independent:

```text
Q = posting + blobs + proving + operations + risk allowance + target profit
```

This is our pricing calculation, not a standardized absolute-payout RPC field.
We compute `Q`; the builder need only assess its remaining economics and honor
the agreed transfer. Sponsorship is not necessary when a sufficient refund is
available. The [dated comparison and arithmetic](../audits/2026-10-10-cross-chain-fee-research/README.md#common-payment-compatibility-analysis)
cover the current major builders and preserve the distinction between documented
capability, static arithmetic and actual payment verification.

| Candidate | Ordinary user transaction only | Compatibility established by documentation | Main trade-off |
| --- | --- | --- | --- |
| Funded bundle, refund from one real user | Yes | Single-source refunds appear in the APIs of Titan, Quasar, BuilderNet, Eureka, Bombora, BTCS, Ultra Sound and Beaver; exact workload remains unqualified | That source alone must cover `Q`; one whole request group per post loses shared fixed-cost savings |
| Shared post, refund from all contributing users | Yes | Explicit at Eureka (up to 16 selected sources) and Bombora; not a common API profile yet | Preferred cost-sharing model, requires wider provider support |
| Sponsored post, refund from its priority payment | Yes | Titan and Quasar document both features separately; composition and sponsored-source eligibility remain unverified | Local recurring treasury receipt works under gross-tip refund assumptions; needs separate recipient and complete funding/refund accounting |
| Funded post plus builder-funded operator value payout | Yes | Titan documents gas and value sponsorship; exact payout purpose remains unqualified | More runtime/account-lifecycle work; not documented across the major builders |
| Nested MEV-Share refunds | Yes in principle | Format exists; deployed endpoint/profile compatibility is unresolved | Not a portable fallback to the reviewed `eth_sendBundle` APIs |
| Separate bundles sharing one post | Yes in principle | No common contract for joint admission, duplicate handling and all required refunds established | A subset can leave the full post cost unpaid |
| Explicit fixed batch payout agreed with builders | Yes | Proposed commercial/API integration, not a deployed common field | Best expression of `Q`, but requires provider adoption |

#### Common funded single-source profile

Submit a funded native-blob post and its ordered source companions in one
strict bundle. Select exactly one external user transaction for the refund;
use the final source transaction as the common default where the valid execution
order permits it. Set a nonzero treasury `refundRecipient` and a supported
integer `refundPercent`. Omit source selection only when the verified default
is that intended final user. Titan's tested validator recognizes explicit
camel-case `refundTxHashes` with at most one hash, but payout from that selected
source is not yet verified. Each builder still needs qualification for native blobs, exact
target, omitted fields, payout deductions and replacements.

For the often documented deduction order, with `T_a` the selected user's actual
priority payment and `H_b` builder `b`'s payout cost:

```text
net_refund_b = floor(p_b * T_a / 100) - H_b
required_anchor_tip_b = ceil(100 * (Q + H_b) / p_b)
```

Use the actual provider formula: Titan's example deducts transfer fees before
applying its percentage, while Bombora explicitly deducts them afterwards.
Choose a common percentage against the worst qualified route or separate
verified request adapters. Require every receiving builder's net refund to
cover `Q` and leave enough retained revenue for inclusion. Do not assume that
a positive retained balance guarantees a competitive bid.

The refundable source must be externally user-funded. A refund of a large
poster-funded tip moves our own money back at a loss. If the operator pays
`X` and receives `floor(p * X / 100) - H`, its net is at most
`-(1 - p/100) * X - H`, before posting expenses; other users' tips remain with
the builder. Increasing `gasLimit` or fee caps does not create refundable
payments beyond actual charged gas and effective priority fees.

There are two valid uses of this common profile:

1. Independently price and settle a whole request group whose selected user
   transaction funds its full service cost. This preserves the wallet flow,
   but pays fixed proof/post/blob overhead more often. It is a separate
   composition, not ten copies of one shared proof/post in different bundles.
2. Keep a shared batch only when a selected user's already signed, quoted
   payment covers all its costs and margin. Add other groups only while that
   check remains satisfied. This concentrates the charge on that payer; do not
   silently bill an arbitrary user for the rest of the batch or call it shared
   cost allocation. Valid execution/nonce order determines eligible sources.

If neither condition holds, that exact candidate is ineligible for a
single-source route. Routing cannot raise a signed tip. A cheaper pooled quote
must not silently become the expensive compatibility mode after signature.
Existing accepted work retains its queue ownership; a new pricing mode does
not authorize eviction or retroactive collection.

#### Economic comparison

The [integer model](../audits/2026-10-10-cross-chain-fee-research/common-payment-analysis.json)
uses ten equal-work users, a `0.006 ETH` required batch receipt, 90% refunds and
an illustrative `0.0001 ETH` payout cost. The numbers are not production prices
or a prediction of any builder's exact refund. Source base fees are additional.

| Arrangement | Total user tips, approximately | Net received by us | Consequence |
| --- | ---: | ---: | --- |
| Equal quoted shares, pooled refund | 0.006778 ETH | 0.006 ETH | Cost target met with shared contributions |
| Same shared post, one concentrated payer | 0.006958 ETH | 0.006 ETH | One user pays 0.006778 ETH; nine others pay 0.00002 ETH each |
| Equal 0.001 ETH tips, single-source refund | 0.010000 ETH | 0.0008 ETH | More aggregate tips still do not recover the required payment |
| Charge all ten users the full anchor amount | 0.067778 ETH | 0.006 ETH | Expensive; unselected tips stay with the builder |
| Ten independently posted request groups | 0.037778 ETH | 0.033 ETH | Same aggregate target profit, repeated fixed service/payout costs |

The last row assumes `0.003 ETH` fixed service cost per post, `0.00025 ETH`
marginal cost per user and `0.0005 ETH` total target profit, held constant across
the comparison. It does not assert those costs for EEZ or concurrent inclusion
of ten independently proven batches. Separate settlements each pay their own
payout overhead; same-block payout coalescing is not assumed.

#### Higher quoted tips and optional user rebates

The user's subsequent preference accepts higher ordinary transaction fees with
optional later rebates. Use the funded single-source refund as the proposed
launch profile, while keeping efficient pooled refunds as an improvement.
Keep one shared post whenever a valid final candidate is covered by a selected
user's tip. Uniform conservative quotes are also permitted by this preference;
they may let any eligible user fund the refund, but the unselected tips remain
builder revenue. Source ordering still determines which payer can be selected.

Quote the expected higher fee and its signed bounds before submission. An
individual standalone quote is not automatically sufficient for an arbitrarily
large batch: after adding each complete request group, recheck the whole batch's
cost against its actual selected refundable payment. Respect existing signed
caps and retain accepted requests under the normal queue ownership rules.
Builder-specific multi-source refunds may improve recovery on the same user
transactions, but the common route must be independently funded without them.

Distinguish a shared settlement from a refund based on multiple user sources.
The former can use the common single-source profile; the latter remains a
separate provider capability. Higher source tips satisfy the first profile's
economics only when the selected source is sufficient. They do not remove the
other qualification requirements or establish support at every builder.

Optional customer rebates require no additional user transaction or signature:
the operator can send ETH to the original source sender. Compute any rebate
from actual externally received funds, not aggregate tips paid to the builder:

```text
customer_rebate_principal + rebate_transfer_cost
    <= max(0, settled_external_receipts
              - actual_attributed_service_costs
              - retained_profit
              - remaining_risk_reserve)
```

The builder's payout deduction is already excluded from settled net receipts;
do not charge it twice. If the net receipt equals the required cost, margin and
reserve, there is no rebate budget. If costs are lower than reserved, or a
qualified route returns additional value, some surplus can be returned. Rebating
all users' unrecovered tips would require another explicit funding source.
Do not count a fee cap as a collected amount or advertise an optional rebate as
guaranteed recovery of the higher quoted fee.

Record rebate eligibility only after the source's canonical inclusion and the
corresponding payment are reconciled under the settlement policy. A receipt,
pending builder receivable or bundle ACK is insufficient by itself. Keep
durable attribution to the consumed source and settlement attempt, account for
reorgs and transfer costs, and ensure retries cannot pay the same rebate twice.
No user rebate execution is implemented by this research recommendation.

#### Recommended integration target

Use one quote/admission calculation with explicit collection profiles. Qualify
the common funded single-source refund first for broad endpoint compatibility;
use the accepted higher-tip profile with optional funded rebates for launch.
Use pooled refunds to improve multi-user pricing and recoverable surplus.
Add value sponsorship or a refund from a sponsored post only where its complete
funding and recurring-payout behavior is verified; neither is a universal
prerequisite.

The smallest apparent extension for builders with the one-source limit is to
accept several unique, included `refundTxHashes` and calculate one refund from
their combined external payments. This already matches Eureka/Bombora's
documented shape. Public rbuilder at revision
`6037fa728b13bf1806e16fff2586414216f6b8fa` contains accumulation of refundable
transaction profit, which suggests reuse may be possible; it does not establish
deployment or prove that removing an API limit is sufficient. Providers must
define admission, simulation, deduplication, omission and payout accounting.

For a verified aggregate formula, choose an allowed integer percentage that
meets `Q` after deductions, rather than demanding a new absolute amount field.
Titan, Quasar and BuilderNet require a provider extension or another qualified
route for multi-source collection; an agreed absolute net payout is an
alternative provider integration. Native
blob transport, bundle-size limits, exact target, source ordering and canonical
reconciliation remain necessary in either case. Do not extend production
fan-out merely because these fields are accepted or silently ignored.

MEV-Share nesting and repeated overlapping bundles remain research alternatives.
The Flashbots public method documents one backrun transaction, and earlier
empty-parameter Titan/Quasar probes returned method-not-found. Those facts do
not establish every private endpoint's behavior. Multiple refund destinations
or one coalesced transfer also do not widen a one-source refund base. In
overlapping bundles, duplicate nonces cannot execute twice; allowing a post to
be dropped still does not require all the other bundles or refunds to land.
The pinned protocol's same-block queues and supported partial consumption
prevent assuming that publishing the table guarantees all its paying users.

#### Separate user refunds alongside one shared post

The one-source restriction is per bundle, not an established limit of one
refund-paying bundle per block. The screenshot supplied by the user describes
a valid documented mechanism: a bundle containing one real user transaction
can request its refund to our treasury. This requires no extra on-chain payment
or authorization from that user. [Titan API](https://docs.titanbuilder.xyz/api/eth_sendbundle),
[Beaver API](https://beaverbuild.org/docs.html).

A concrete candidate avoids repeating the same post in several bundles:

```text
bundle A: [postBatch, user A] -> refund from user A to treasury
bundle B: [user B]           -> refund from user B to treasury
bundle C: [user C]           -> refund from user C to treasury
```

Each label must preserve its complete replay group and source dependencies.
The first bundle must be valid on its own; subsequent bundles consume the
same posted generation at the same exact target. The unresolved provider
behavior is admission/simulation of dependent bundles and their ordered
composition into one block. A builder may reject a standalone user bundle
against parent state because its required post has not run. Documentation of
one-transaction refunds alone does not establish this state-dependent behavior
or require all the bundles to be selected. No live composition test was run.

The user's accepted higher-quote policy supplies a conservative funding option:
require A's net refund alone to cover the entire candidate's incurred posting,
blob, proving and operating costs plus the target margin. B and C then offer
additional revenue if included and paid; they are not needed to fund already
incurred fixed costs. This still requires actual payout qualification and the
usual failure reserve. It is a concentrated initial charge, not low per-user
shared pricing. Any later rebates use received surplus under the existing
rebate policy. Per-bundle payout overhead must be priced unless coalescing is
verified for that provider.

**Deeper qualification:** Titan's [bundle tracing](https://docs.titanbuilder.xyz/bundle-tracing)
documents top-of-block simulation and failure for a required transaction revert
or nonpositive net builder payment. It does not promise to retry an initially
failing child after another bundle creates its state. The public `eth_sendBundle`
fields above expose no general parent-bundle dependency. This is a documented
admission concern, not evidence that every Titan sorting algorithm permanently
discards every dependent bundle.

The reviewed public rbuilder snapshot gives a concrete implementation example:

- At revision `6037fa728b13bf1806e16fff2586414216f6b8fa`,
  [`sim_worker.rs`](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/live_builder/simulation/sim_worker.rs)
  opens parent-block state and forwards successful order simulations. A child
  that fails without a simulated parent does not reach normal builder intake.
- [`SimTree`](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/building/sim.rs)
  discovers nonce predecessors, not arbitrary contract-state dependencies. Its
  predecessor index admits results only when `nonces_after.len() == 1`.
  `[postBatch, user A]` changes at least the poster and A's nonces. Appending an
  operator nonce transaction to B therefore does not provide a general fix in
  this snapshot. Independent users with already-ready nonces supply no nonce
  edge that would identify the required post or earlier user.
- The ordering builder's `failed_order_retries` applies to an inserted order
  whose profit fell below its simulated value; it does not establish retries
  for children that never passed admission. A later block-building pass cannot
  repair an order absent from its input.

These are source observations, not a production rbuilder execution test or a
claim about Titan's private implementation. Source URLs/hashes and limitations
are retained in the [dependency evidence](../audits/2026-10-10-cross-chain-fee-research/README.md#dependent-bundle-admission-ordering-and-pricing).

The pinned protocol adds dependencies between users themselves. Six isolated
Foundry checks with **real nonempty entries and the core mock proof system**
passed: a child before posting fails; a child whose `currentRoot` requires A
still fails after posting until A executes; A alone can consume only a prefix;
the child expires next block; another same-block post replaces it; and a later
entry with matching roots can skip an earlier entry. These unit calls do not
exercise signed transactions, native blobs, Composer, L2 replay or a builder.
They establish why completion must follow the actual dependency graph and
canonical consumption, rather than assuming either all users or every arbitrary
subset can execute. An application that catches a failed proxy call also needs
semantic reconciliation; a successful outer receipt alone is insufficient.

**Repeated droppable prefixes do not guarantee multiple refunds.** Consider:

```text
bundle A: [post, A]       refund from A; every transaction mandatory
bundle B: [post, A, B]    refund from B; post and A droppable
bundle C: [post, A, B, C] refund from C; post, A and B droppable
```

This makes each bundle self-contained for initial simulation. If C is chosen
first, it executes all three users. A and B then have spent nonces, so the
remaining bundles cannot collect their selected refunds. If A, B, C are chosen
in that order and consumed prefixes can be dropped, three refunds are possible.
The declared refund belongs to its bundle execution, not permanently to a raw
transaction wherever another bundle executes it.

There can also be an incentive to choose C first. In the illustrative arithmetic
model, each user tips `0.003 ETH`, each refund requests 90%, and each payout costs
`0.0001 ETH`. Three paid refunds deliver `0.0078 ETH` to the treasury and leave
`0.0009 ETH` to the builder before its bid. Executing C alone delivers only
`0.0026 ETH` and leaves `0.0063 ETH` to the builder. This is a counterexample to
assured refund stacking, not a prediction of a provider's chosen ordering.
All six abstract order permutations were evaluated in the retained model.
In particular, never count A's full-cost refund while also offering its raw
transaction through an overlapping bundle that pays only B or C's small refund.

**The economic admission rule is outcome-based.** For every allowed successful
posting outcome `S`, require:

```text
sum(actual qualifying net refunds in S) >= incurred operator cost(S) + margin
```

Cost includes the full proof preparation and native-blob post already incurred,
even when only A consumes its entries. With a strict A bundle and optional
children, charging A for this full amount is sufficient for those included
outcomes, provided its refund is actually paid and no offered variant bypasses
that payment. If sharing cost across many small refunds instead, require joint
selection/payment or budget the missing contributions explicitly.

For illustration, take `0.0055 ETH` of operator cost and `0.0005 ETH` of margin,
so required net payment `Q = 0.006 ETH`. Using the conservative example formula
`R = floor(0.90 * T) - 0.0001 ETH`, the required selected priority reward is
`T >= 0.006777777777777778 ETH`. `T` is the user's total effective priority
payment, not its gas limit or max-fee authorization. For ordinary EIP-1559
transactions without direct coinbase transfers, price it from actual execution
gas and `min(maxPriorityFeePerGas, maxFeePerGas - baseFee)`, with an appropriate
lower bound under the admitted state changes. Source execution base fees remain
an additional user expense. [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559).

Titan's published example applies the percentage after transfer cost; Beaver's
subtracts that cost after applying the percentage. `p*T-H` is conservative
relative to `p*(T-H)` for `0 < p < 1`, given the same verified reward and bounded
transfer cost. This does not establish another provider's exact deductions.
Retain a provider-specific formula and integer rounding. Refunds are capped
below 100% in the reviewed Titan/Beaver APIs; sufficient net builder value is
still necessary for competitive inclusion.

The retained ten-user model tests all ten dependent prefixes, and all 512
subsets containing A for a separately assumed independent-child workload.
Equal `0.0006 ETH` net contributions meet the target only when all ten pay;
A alone yields a `0.0049 ETH` operating loss. A `0.006 ETH` net contribution
from A meets the `0.0005 ETH` margin floor for every modeled included subset.
Ten separately charged payout costs also raise the full-inclusion gross-tip
requirement to approximately `0.00777778 ETH`, versus `0.00677778 ETH` for one
pooled payout. These are illustrative costs, not live gas measurements.

No-inclusion outcomes still consume preparation resources. Include historical
failed-attempt costs in quotes and retain a finite risk budget; the included
outcome floor alone does not guarantee profit on every proving attempt.
Count later user rebates only from received surplus after this reserve.

**Implementation preference:** qualify one strict bundle
`[postBatch, A, B, C]`, selecting A's adequately quoted transaction as the refund
source, before fragmenting the candidate. That preserves one-post multi-user
execution without depending on discovery of separate bundles. It remains a
builder-enforced bundle policy, not an on-chain obligation to reimburse the
poster. Pooled-source refunds improve allocation where qualified; separate
refund bundles remain a provider-specific optimization requiring ordered
admission and payout evidence. The existing production submitter has no refund
fields or multi-bundle ownership integration, so this is not a configuration-only
change.

For any separate-bundle implementation, use distinct replacement identities
for siblings and one local candidate generation tying their exact parent,
timestamp, post, proof and signed sources together. Do not release the attempt
on its first ACK. Reconcile each consumed user and actual treasury payment,
including partial outcomes, replacement history and reorgs. Verify where refund
transactions execute: the reviewed rbuilder code can combine eligible simple
recipient payouts at block end, while other cases pay immediately. A
refund recipient whose balance/code can affect later application execution
requires explicit simulation coverage. Coalescing is an optimization, not an
assumed provider guarantee or a substitute for source aggregation.

If instead every user pays only a fraction of the fixed cost, the sum of all
requested refunds cannot be treated as assured revenue when only a subset may
land. Joint inclusion/payment then needs separate provider support or an
explicit operator subsidy policy. This makes the independent-bundle route a
valid candidate with a conditional cost floor, not a proven portable solution
and not an impossibility implied by the one-hash limit. A refund from an
operator-funded post remains different: it returns our own fee spending and
does not itself create an external user payment.

There is no verified current configuration that simultaneously provides
ordinary tip-only user transactions, evenly allocated fees in one shared post,
and collection through every major builder. The proposed path preserves the
user flow and exposes the economic trade-off, while seeking provider support
for the efficient common profile. This is a draft recommendation, not a claim
of live qualification or a request to change the user's payment constraint.

### Contract settlement of multiple L1 users

The proposed two-transaction shape is feasible for compatible application
requests or smart-account operations:

```text
transaction 1: native-blob postAndVerifyBatch(...)
transaction 2: Settlement.execute(signedOperations[])
```

It is not a transparent replacement for independent, already-signed L1 EOA
transactions. A contract can verify signatures, but it cannot invoke an ordinary
raw transaction as a new top-level Ethereum transaction, debit that EOA's gas
balance, or assign the EOA as the caller of an arbitrary target. A normal call
from the settlement contract makes that contract the target's `msg.sender`.
`delegatecall` does not provide arbitrary sender impersonation. Transaction
boundaries, `tx.origin`, nonce handling and transient state also differ.
[Solidity call semantics](https://docs.soliditylang.org/en/latest/introduction-to-smart-contracts.html),
[EIP-1559 transaction accounting](https://eips.ethereum.org/EIPS/eip-1559).

The fork has a concrete additional constraint: the
[proxy](../../eez-core-protocol/src/base/CrossChainProxy.sol) passes its actual
caller as `sourceAddress`, and the
[core call identity](../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md#c-action-hash-computation)
commits to that address. A generic wrapper therefore changes the source
identity wherever it replaces the caller seen by the proxy. The
[Composer](../../crates/eez-composer/src/composer.rs) currently submits the exact
held L1 inbound transactions after the post. Supporting settlement operations
would require composition, proof/execution validation and ownership/replay
rules for their actual call graph, not repackaging an existing proof and raw
transactions. No such runtime extension is established by this research.

| User/application model | Can share one outer settlement transaction? | Remaining requirement |
| --- | --- | --- |
| Current arbitrary L1 EOA transactions | No transparent conversion | Neither caller authority nor per-user gas payment transfers into a wrapper |
| Existing compatible smart accounts / ERC-4337 operations | Yes in principle | Signed operations, account validation and authorized fee reimbursement; new integration in this fork |
| EIP-7702 delegated accounts | Yes with suitable delegated code | A valid delegation must already exist or be authorized; ordinary transaction signatures do not install it |
| Application-specific signed orders / meta-transactions | Yes for applications built to accept them | Recipient/order verification and existing authority to settle assets and collect fees |

[ERC-4337](https://eips.ethereum.org/EIPS/eip-4337) explicitly defines a single
`handleOps` transaction and a beneficiary paid collected operation fees.
[EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) requires signed delegation
authorizations. [ERC-2771](https://eips.ethereum.org/EIPS/eip-2771) requires a
recipient that recognizes its trusted forwarder. Already-configured accounts
or applications may preserve one signature per operation without another
per-operation approval; that does not establish compatibility for arbitrary
existing EOAs under the user's no-new-authorization constraint.

One outer settlement transaction also has one protocol gas payer. Raw user
fee fields carried inside its calldata do not become separately charged tips.
If the operator funds the outer tip, refunding it remains an operator expense
unless users reimburse it through an authorized mechanism. Where operation
fees can already be collected, paying the treasury directly can cover posting
and service costs without depending on builder refunds. This is a conditional
account/application design, not a fee-collection mechanism already present here.

L2-origin settlement is different: the pinned
[execution model](../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md#delivery-units-and-execution-transactions)
already permits multiple L2Tx entries within one L1 post or deferred execution.
Its meta hook permits contract-driven calls but grants no EOA authority.
Neither facility converts L1-origin user transactions into one fee source.

### Alternatives excluded by the user constraint

Prepaid service credit, a separate payment transaction, a fee permit, an escrow
deposit or an account-aggregation authorization are outside this proposal's L1
wallet flow. They are not an automatic fallback when a builder lacks refunds.
They remain part of the research history, not implementation prerequisites.

Nor can an arbitrary existing signed EOA transaction be silently wrapped to
redirect its tip: fee fields pay the L1 beneficiary, the sender signature binds
the application transaction, and a wrapper can change caller semantics. The
current signer rejects sender binding; a raw post proof does not bind the source
transaction or guarantee consumption. These facts prevent treating a generic
wrapper as an already-supported substitute for the required builder integration.

## Native L2 settlement fees through standard wallet RPCs

Exploratory design, 2026-10-14. The user requested this alternative to a custom
quote API. It is a proposed L2 execution-profile change, not accepted protocol
behavior or an implemented configuration. Preserve ordinary signed user
transactions and their spending limits; additional setup remains operator work.

### Charge money through a real settlement-gas component

Let `F` be a deterministic settlement tariff in L2 native base units for one
complete outbound source group. It covers priced L1 application work, posting
overhead, native DA, proving/operations, treasury costs, risk and profit under
an explicit parameter version. It is a price before inclusion, not an exact
invoice for future L1 receipts. Arbitrum documents this distinction between
known-at-execution pricing and unknown eventual batch cost in its
[fee design](https://docs.arbitrum.io/how-arbitrum-works/deep-dives/gas-and-fees).

One concrete candidate for preserving the standard wallet payment identity is:

```text
B = L2 base fee
P = min(maxFeePerGas, B + maxPriorityFeePerGas)   # require ordinary eligibility
S = 0 if F == 0 else ceil(F / P)                 # require P > 0 when F > 0
G = ordinary local receipt gas, including its own refund/floor rules

receipt_gas_used = G + S
user_fee        = (G + S) * P
settlement_vault_credit = S * P
ordinary_base_burn     = G * B
ordinary_tip_credit    = G * (P - B)
```

All arithmetic is checked integer arithmetic. `S * P` covers `F` with less
than `P` units of rounding excess when `F > 0`. Local-only transactions have
`F = 0`. This particular design sends the entire
settlement component, including its nominal priority portion, to the settlement
treasury. It changes the present beneficiary rule and must be specified as
such. Keep transaction `GASPRICE = P`; it is not the larger L1 gas price.
Do not also debit `F` separately: that would charge the same service twice.

Reserve `S` gas units before application execution; the EVM can use only the
remaining signed limit. Preserve the original source transaction and full
upfront affordability check, including its value. Reducing a temporary TxEnv
limit without implementing the corresponding debit/refund rules is insufficient.
`S + local_required_gas` must fit the signed limit; local required gas includes
pre-refund needs and forwarding headroom, not just final receipt gas. Insufficient
allowance rejects admission or fails according to the specified execution rule;
it never permits a debit above `gasLimit * maxFeePerGas` for execution fees.

Compute ordinary storage refunds and calldata floors on the local component,
then add `S`. Settlement gas must neither increase the storage-refund allowance
nor disappear under a calldata floor. Define vault credit at transaction
finalization, outside application revert scopes, with no recipient EVM call.
An included supported transaction can consume resources despite caught inner
reverts; simulated or discarded transactions create no canonical credit.
This proposal does not broaden the current signer's source-failure profile.

For illustration, `F = 0.003 ETH`, `P = 1 gwei` and `G = 100,000` produce
`S = 3,000,000`, a receipt charge of 3,100,000 gas and a fee of 0.0031 ETH.
The treasury receives 0.003 ETH; the remaining 0.0001 ETH follows ordinary
execution burn/tip rules. This is a model example, not measured production cost.

Use a dedicated spendable L2 treasury account. The current
[block executor](../../crates/eez-evm/src/block.rs) burns the balances of EEZL2
and SYSTEM_ADDRESS at the next block start; neither is a fee vault. A withdrawal
contract is optional account infrastructure, but redemption authorization,
canonical accounting, sweep costs and L1 working capital remain necessary.

### Price the complete nested group, with authenticated inputs

For `L2 A -> L1 B -> L2 C -> L1 D`, ordinary local gas covers A, C and their
actual L2 protocol execution. The settlement tariff covers the L1 work of B
and D once, plus posting/data/service costs. A native L1 trace of B may include
D; adding its inclusive total to D's total would double charge. Composer's
intercepted action meters have different semantics and must not be substituted
for native receipt gas. Preserve all sequential roots of the source, supported
caught failures, rollback spans and static work. Existing unsupported static
or failed frames with descendants remain unsupported.

Define an owned resource record for each original signed source, scoped by
chain, source hash, candidate generation and position. Its complete tree and
encoded data, rather than a flat list of external addresses or nesting depth,
determine the tariff. Track non-overlapping L1 work, protocol/table work and
encoded DA including return/revert bytes. A snapshot/revert can roll back state
without erasing consumed gas. Charge attempted supported execution, and count
the source group once, rather than applying a fixed posting charge at every
cross-chain hop. Blob-boundary and sparse-batch allowances belong to the
tariff/batch coverage model; fractional byte pricing alone is insufficient.
For the initial low-traffic policy, price the source as potentially the only
funded group in a batch. Do not assume future users will cover a shared posting
or blob charge; pooling savings can be a later pricing-policy refinement.

Separate two possible authorities for the resource record:

1. **Bounded, deterministic tariff.** Derive byte counts, supported shape counts
   and published work-class allowances from validated group semantics and an
   authenticated price schedule. Independent execution/reconstruction can verify
   the charge. Use a final complete-batch cost check and restrict the workload
   class to what its tariff and gas limits cover. Structural counts alone do not
   measure arbitrary L1 application complexity. Reject or defer work outside the
   funded class; do not silently credit a Composer measurement as consensus fact.
2. **Independently checked L1 metering.** Add L1 execution evidence and validation
   at the authenticated parent, preserving the relevant prefix, chain rules,
   caller/gas/fee environment, nesting and revert semantics. Recompute the defined
   remote-work measurement independently before accepting its resource record.
   This is a larger signer/prover capability than the current L2 Stateless replay.
   A fee field or a hash of a Composer report is not independent verification.

Within this future native-fee alternative, the first is the recommended bounded
prototype. Neither is a v1 prerequisite. The second is required before
claiming that native fees track arbitrary state-dependent L1 execution closely.
Neither predicts an arbitrary builder prefix or future inclusion perfectly.
The [current signer contract](../../crates/eez-proof-signer/SPEC.md#11-what-the-signature-does-not-establish)
does not establish future L1 target execution and cannot be described as
already certifying an L1 gas invoice.

Price inputs must be immutable for the candidate and derivable historically:
an authenticated L1 reference with execution/blob fee inputs or bounded tariffs,
L2 fee context, asset conversion rule, schedule version and authorized parameter
updates. Do not use the latest RPC response or wall-clock proof time during
consensus replay. Proving/operations/profit are published tariff terms, not
arbitrary Composer invoices. Existing L1 context authentication can help bind
inputs; it does not authorize new price or remote-gas fields automatically.

Avoid deriving `F` from the exact eventual PostBatch receipt or its proof bytes.
The fee changes L2 balances/receipts and therefore committed block hashes and
proof inputs, creating a circular dependency. Use the specified resource tariff
with conservative shared-overhead allowances; validate the finished L1 candidate
for economic coverage separately. Reserving `S` also changes available gas and
balances during execution: compose with that actual rule, iterate only within a
bounded budget, and reject unstable/unsupported outcomes instead of accepting
an earlier uncharged simulation.

### Carry the fee context and preserve partial settlement

The current outbound action is `[native loadExecutionTable, original user tx]`.
A candidate integration point is a versioned extension of that authenticated
setup, binding the fee context to the next exact source group. The user signs
no second transaction. The final encoding is an open design item: if it changes
the load ABI or native carrier, update its Solidity/Rust producers, decoders,
authorization and reconstruction together. Do not repurpose the existing
zero-valued call-gas field as a fee: it participates in call identity and the
current signer explicitly rejects nonzero values.

Whatever carrier is selected, fee inputs must be committed through the native
DA and validated derivation path. Current direct V0 contains no fee side table;
an out-of-band Composer field would not make follower replay deterministic.
Reject missing, duplicate, reordered, unused, forged or differently attributed
fee records. Independent validation must bind both record contents and the
authority of their price/resource inputs before signing.

Credit each user's charge inside that user's L2 transaction boundary, so the
existing block-hash prefix commitments retain fees only for retained groups.
Never move all fee credits to block end: intermediate canonical prefixes must
have the right balances and receipts. Native incoming/setup transactions remain
zero-price and do not become another customer payer. Reconcile source fees once
across replacement, resubmission, partial consumption and reorg. A paid failed
L1 post can still lack retained L2 fee revenue; the operator reserve funds that
exposure. An eventual successful source fee cannot pay unlimited failed posts.

### Resolve monetary gas versus execution capacity

At low L2 prices the settlement gas equivalent can be large: 0.003 ETH costs
3,000,000 units at 1 gwei but 300,000,000 at 0.01 gwei. The current profile fixes
30,000,000 block gas and uses Osaka's 16,777,216 transaction cap. Receipts,
header gas, native remaining-gas budgets, base-fee evolution and prefix hashes
currently share those counters. Merely adding `S` can make a cheap-to-execute
transaction unadmittable or crowd out block capacity.

For the bounded prototype, count billed settlement gas against the existing
limits and reject estimates/candidates that cannot fit. Test realistic L1/L2
price ratios before claiming this is commercially usable. Define behavior for
zero effective price; nonzero service cannot be converted at `P = 0`.

A production design may require separate execution and fee-unit budgets,
possibly a fee-price floor. That is a further execution-profile decision, not
just an internal counter: specify transaction/header gas limits, cumulative
receipt gas, the base-fee update inputs, native system budgets, gas-used ratios
and independent validation. Never leave a receipt/header above the currently
validated limit or let settlement units grant unbounded EVM computation.
Changing a fee-price floor also changes ordinary transaction economics and
requires explicit activation and wallet tests. No such change is accepted here.

### Standard RPC behavior and implementation scope

`eth_estimateGas(tx)` must run the same fee-aware composition and materialization
with the requested sender/value/calldata/nonce and fee assumptions, then return
`S + local_required_gas` with justified headroom. It cannot just add an arbitrary
number to an ordinary source-chain estimate. Resolve omitted fee fields from a
documented candidate context; if the wallet changes prices or the L1 context
changes, recompute. Keep `eth_feeHistory` truthful and price suggestions
network-wide. Simulation is read-only and bounded; no proving broadcast or user
signature is needed for an estimate. The existing fronts currently forward
these methods, so this is new backend behavior behind standard RPC names.

| Area | Required change |
| --- | --- |
| Shared rules and specification | Tariff, resource authority, integer conversion, fee recipient, refund/failure rules, activation and gas-budget semantics in `eez-protocol` and the owning specifications |
| L2 execution | Fee reservation, sender debit, treasury credit and receipts in `eez-evm`; preserve L1 simulation rules and native zero-price behavior |
| RPC/fronts | Fee-aware `eth_estimateGas`, consistent `eth_call`/pending simulation and receipts through `eez-node-common` and the Composer front |
| Composer | Whole-source resource attribution, fee-aware composition, authenticated fee context, final L1 cost check and bounded preparation |
| Signer/prover RPC | Independent fee-policy/input validation, revised replay/receipt/checkpoint expectations; L1 metering evidence only if explicitly implemented |
| DA, core setup and derivation | Versioned canonical carrier if necessary; update `eez-payload-codec`, native semantic reconstruction, system-transaction builders, signer and deriver/follower together; any changed load ABI requires the pinned core change too |
| Treasury/indexer | Separate local burns, ordinary tips and settlement credits; keep reorg-aware recognition, withdrawals and L1 liquidity accounting |

Required acceptance evidence includes fee/supply conservation and signed spending
bounds; zero-price/rounding/overflow cases; local storage refunds and calldata
floors; low-price gas-budget saturation; nested and multi-root work without
double charging; supported failed/static work; forged or unattributed resource
records; independent signer/follower replay; partial prefixes and reorg/restart;
actual MetaMask/Rabby signing and displayed fee behavior; and batch receipt
cost reconciliation. An arithmetic example is not evidence of these properties
in a running node.

Deploy as a coordinated versioned activation. Preserve old-block execution and
old DA decoding; old clients must not silently interpret the new profile.
Update signer, Composer and follower/deriver before activation, including any
necessary contract/storage and witness changes. Existing-network activation and
fresh genesis need separate procedures; changing a genesis file cannot upgrade
an existing chain. After new fee-bearing state is canonical, rollback requires
a compatible forward recovery plan, not merely an old binary. No deployment,
native-fee implementation or activation is authorized by this exploratory text.

## Quotes, admission and ownership

For transaction-specific tip pricing under the current execution rules, give
the dApp/SDK a quote before its normal wallet signing request. A versioned
`eez_quoteCrossChainFee` method, advertised through `eez_composerInfo`, is one
possible interface, not a protocol requirement or an existing endpoint. An
application backend or SDK can provide equivalent pricing without introducing
a wallet-facing RPC method. Such a client integration still needs to set the
standard signed fee fields. The proposed detailed quote returns:

- Chain/deployment identity, direction, quote ID/version, intent hash, source
  and L1 parent hashes, L2 draft context, expiry and pricing assumptions.
- Source gas limit recommendation and expected receipt gas separately from
  remote execution, framing, DA bytes/blob share and compute allowance.
- Required service contribution, asset/recipient, collection method, suggested
  priority fee and required fee-cap headroom; user source cost and expected
  total cost separately.
- Ordinary transaction fee caps/exposure, batching assumptions and conditional
  builder dependence. V1 does not quote reserve-funded new-demand discounts.

The user's 2026-10-14 question about standard wallet endpoints exposes a separate
choice: unmodified-wallet automatic pricing versus application-specific quote
integration. Under the standard
[fee RPC definitions](https://github.com/ethereum/execution-apis/blob/main/src/eth/fee_market.yaml),
`eth_gasPrice` and `eth_maxPriorityFeePerGas` have no transaction parameter;
`eth_feeHistory` takes a block range and reward percentiles. They can supply
network-wide recommendations, not the remote-work price of a particular call.
[`eth_estimateGas`](https://github.com/ethereum/execution-apis/blob/main/src/eth/execute.yaml)
does receive the transaction, but returns one gas quantity, not suggested fee
fields. In the present execution model, inflating that quantity alone increases
the limit; unused gas does not become revenue. Errors containing a suggested
tip likewise require client logic to interpret them. Do not infer a reliable
per-transaction association between separate, possibly cached or concurrent
wallet RPC requests.

Standard wallet RPCs can support transaction-specific total fees if the L2
actually meters and collects a settlement component in execution and includes
it in gas estimation. [Arbitrum's estimation model](https://docs.arbitrum.io/arbitrum-essentials/how-to-estimate-gas)
is an example of expressing parent-chain data charges in child-chain gas units;
its [fee accounting](https://docs.arbitrum.io/how-arbitrum-works/deep-dives/gas-and-fees)
also collects those funds. That supports the interface approach, not reuse of
its data-only formula for RollupX's nested L1 application work. Here it would
require a new deterministic fee rule, explicit treasury credit rather than
ordinary base-fee burning, signed spending bounds, receipt/refund semantics,
independent proof/replay validation and an activation/compatibility design.
It is the future protocol-fee option above, not an RPC-only implementation.
MetaMask/Rabby behavior would need actual wallet tests before claiming the
complete user flow works without application changes.

The current-rule alternative using only ordinary wallet fee suggestions is a
network-wide tip policy plus the batch coverage gate. It sacrifices precise
transaction-specific pricing and can overcharge cheaper L2-only operations
while still requiring deferral for expensive outbound work. It remains an
option for the low-traffic product, rather than a universal cost guarantee.

Bound parsing, simulation, concurrent work and quote lifetime. Reuse Reth's
task executor and existing request lifecycle rules; expensive execution stays
off async workers. An unauthenticated quote is not a reservation or guarantee.
Rate-limit costly free simulations and use bounded client/service quotas;
fund unpaid preparation from the service budget rather than monetizing retry
counters retroactively.

Keep `eth_sendRawTransaction` for ordinary signed transactions. In tip mode the
signed caps are the payment authorization; recompute eligibility from signed
bytes and current state, even if the client skipped the quote. No separate
signed quote, fee permit or payment-submission method is needed. An
underpriced initial submission receives a structured error with required
payment and context, without entering the owned queue.
In enforced mode, an inbound source without a qualified, sufficiently funded
builder collection route is rejected before acceptance. The strict v1 policy
does not use operator subsidies to admit underfunded new demand. Any future
subsidy mode needs a separate policy with explicit per-client and aggregate
funding; it is outside this v1. Builder gas sponsorship paid for by a user's tip
is a different, also deferred collection adapter. Enabling L2 enforcement must
also apply this inbound policy. Honor previously accepted inbound work from its
migration reserve.

Recheck coverage at final composition/dispatch. If accepted work becomes
underpriced, retain it as blocked under the existing ownership model and expose
the reason/requote requirement. Extend the existing per-sender/direction
`fee_blocked` selection pattern in a documented policy version; do not use a
resource cut that stops the entire younger FIFO suffix. Sender successors wait,
independent eligible sources remain selectable. Batch-occupancy shortfall is a
separate funding failure: defer/requote the unfunded cohort without declaring
an individual source invalid. In v1, a reserve cannot substitute for the missing
projected customer recovery in this check.
Sources with independently sufficient funding can proceed in eligible FIFO
order; bound repeated assembly work. Do not turn a pricing deferral into a failed
settlement attempt or introduce fee bidding as a replacement for FIFO.
No fee policy may bypass the normal gas, blob, time, proof or final execution
checks. Already-required settlement keeps its reserve rather than waiting
forever for profitable new traffic.

State-dependent L1 work may invalidate a composed batch after simulation. Keep
its financial exposure bounded without inventing new failure attribution. A
trace at a later parent or an infrastructure timeout cannot establish that a
particular source caused the submitted attempt's canonical failure. Apply only
the existing validated request-local attribution and explicit disposition rules;
otherwise preserve ownership, use a funded deferral/recomposition path, and let
independent eligible work proceed. Record preparation costs even when a failure
does not charge the protocol retry counter. L2 source tips become recognizable
only with the validated canonical source group/endpoint, not merely a Live or
optimistic receipt. Do not use that same future fee to fund repeated posts.

Recheck source nonce and affordability immediately before dispatch, while
recognizing that a sender can still spend them elsewhere before inclusion. A
canonical conflicting nonce terminates that signed request under the existing
ownership rules; it does not establish delivery or refund entitlement. After a
conclusive conflict, require funded coverage for further speculative service
from that client until its exposure is reconciled. Do not infer malicious
intent from the conflict. Per-sender controls alone are Sybil-vulnerable: global
exposure limits and pre-reserved operator preparation/failure budgets bound
authorized spending. Those reserves cannot cover a known new-demand revenue
shortfall.
Never lower other users' financial coverage on an unreserved promise that this
source will remain available.

Financial state follows `quoted → risk-reserved → dispatched → reconciled`, with
separate paid/unpaid/reorged builder receivables and operator reserve releases.
`HeldPool` remains the sole request owner. A durable accounting ledger stores
exposure and payment evidence, not customer balances or a second transaction
scheduler. Keys include
source identity and generation, quote, attempt, target parent, post hash,
canonical inclusion and payment evidence. Reorgs reverse canonical revenue and
receivables before reassessment. Duplicate acknowledgements and refund API
records cannot recognize revenue twice. Old attempts cannot release newer
reservations.

Refund accounting also needs payment identity distinct from attempt identity.
Identify an observed transfer by chain, canonical block, payout transaction and
transfer position; bind one or more attributed attempt allocations to it. A
coalesced payout's allocations must not exceed its verified amount, and every
allocation must have qualified provider attribution plus canonical payment
evidence. Provider refund-query records alone do not establish receipt of money.
Keep expected, observed, canonical and finalized/reorged states distinct, with
idempotent reversal/reallocation. Use a dedicated treasury per network and record
the qualified route; separate route recipients are optional where useful.
Unmatched deposits, operator funding and internal sweeps cannot become customer
refund revenue. Credit unidentified payments zero toward candidate revenue
until attribution is established. Delayed payouts remain unspendable receivables
until received; a route without sufficient reconciliation evidence is ineligible.

The [RPC settlement contract](../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission)
defines inbound/outbound completion, static/reverted lookups and whole replay
groups. Use it directly: a post receipt, matching height, or optimistic source
receipt is insufficient. On restart, queue contents may be lost and clients
may need to resubmit; funds/claims must reconcile independently. Uncertain
chain evidence freezes reserve release/revenue recognition rather than guessing.

## Treasury and controls

Record amounts with chain ID and asset identity. Ethereum L1 spending is ETH;
Gnosis spending is xDAI and Chiado uses testnet xDAI. [Gnosis network reference](https://docs.gnosischain.com/about/networks/)
The L2 native token's display name is not an exchange rate. Set `x = 1` only
after verifying backing, redemption route and costs for that deployment;
otherwise use a bounded conversion quote/haircut or do not credit it as L1
coverage. Keep settlement principal and treasury withdrawals separately booked.
Use a configured set of controlled treasury beneficiaries and revalidate it
against each block; a restart/configuration change can redirect fees without
changing consensus. Alert and stop crediting an unexpected beneficiary.
L2 base-fee burns and any residual backing are not accessible treasury revenue;
assign them zero value for recovery under the current protocol.

Sweeping L2 fees is itself outbound work. Include its L1 execution, DA and
redemption delay in `C_treasury`, amortized over the funded sweep. Book its tip
and transferred principal as internal movements, with no new external revenue
and no recursive service markup. Treasury work still passes normal protocol
validation and uses an explicit funded operating allocation.

Size working capital from observed posting spend over the collection/redemption
delay, maximum simultaneous signed exposure, reorg/retry allowance and already
owed backlog. Reserve before signing and release only after canonical
reconciliation. An expected builder refund is not freely spendable cash.
Stop accepting newly unfunded work when cash/reserve limits bind; honor funded
commitments. The current retry counter's word “charge” means an attempt count,
not permission to deduct money.

Initially expose a small typed policy: accounting-only versus enforced
admission, explicit collection method/treasury per chain, quote expiry,
calibrated cost markup, and aggregate financial exposure/reserve ceilings.
Default an absent new policy setting to accounting-only behavior and keep it
separate from `EEZ_L1_POSTBATCH_*` network-price caps. Refuse enforced
mode with zero/uncontrolled beneficiary, unknown conversion, unfunded reserve
or unverified refund route. Map future settings through node/CLI/environment,
Compose and Kurtosis with one documented precedence; test absent, empty,
malformed and conflicting inputs. Do not silently reuse an existing gas cap as
a currency budget.

## Worked examples

These use invented prices/workloads to illustrate the arithmetic, not a
production recommendation or a current network quote.

**L2 collection.** Ten users share a post using 1,200,000 L1 gas at 12 gwei and
one blob charged 131,072 blob gas at 1 gwei. Posting cost is
`0.014531072 ETH`. A combined illustrative 20% allowance produces
`0.0174372864 ETH` required revenue. With 80,000 actual L2 receipt gas per
user and verified 1:1 realization, each needs a **21.796608 gwei effective
L2 tip**, paying `0.00174372864 ETH` in tips. The user's L2 base fee is additional.
At 1 gwei the ten tips produce only `0.0008 ETH`. If only one user remains and
the post cost stays unchanged, that user needs 217.96608 gwei or must wait for
funded batching. If actual gas falls to 40,000, the original quote collects half
as much; final validation must catch that.

**L1 rebate.** A user owes `0.001 ETH` of service contribution, uses 100,000 L1
gas, and has a verified 80% rebate route deducting `0.00021 ETH` for payout.
Required effective tip is **15.125 gwei**: gross tip `0.0015125 ETH`, net
rebate `0.001 ETH`, builder-retained user tip `0.0003025 ETH`. Source base fee
and the post's builder incentive remain separate. Without that rebate the
poster receives zero of the user tip. At 50% realized rebate this price is
insufficient; using expected averages cannot enforce the quoted recovery.

The [September fee incident](../audits/2026-09-18-priority-fee-feedback-loop.md)
also illustrates why a copied chain-wide tip is unsuitable: for its historical
4,373,322-gas post, a 1–2 gwei poster tip alone costs roughly
0.0044–0.0087 xDAI. That dated transaction does not calibrate today's workload,
blob charge or user gas denominator.

## Rollout and validation

| Stage | Deliverable and affected components | Exit criteria |
| --- | --- | --- |
| 0. Establish economics | Extend existing Composer cost events/indexer reporting with payer/beneficiary attribution, canonical external tips, refunds, asset conversion, compute and reserves | A representative complete posting window reconciles receipts and treasury flows; no missing data reported as zero |
| 1. Shadow quotes | Quote service and immutable pricing context in `eez-node` fronts, scheduling/cost model in `eez-composer`, role-neutral money types where shared | Compare quotes with final simulation and canonical spend by workload, direction and batch occupancy; no changed charging |
| 2a. Prepare L2 enforcement | Transaction-specific L2 tip checks and final revenue/exposure gate; reject unfunded new inbound requests under the strict v1 policy | Adversarial and process checks pass with zero assumed builder redistributions; unfunded new inbound rejected, old obligations funded; combined activation waits for L1 qualification |
| 2b. L1 builder qualification | Qualify the common funded single-source refund using the actual native blob post, customer tips and operator balances; specify adapters and durable payment attribution | Receipt-backed recovery of posting plus non-gas costs; preserved exact target and user bytes; no duplicate revenue across restart/reorg; declared support reported separately from proven coverage |
| 3. Enforce L1 tips | Quote the selected collection mode and check normal transaction fee fields; dispatch through qualified routes with bounded operator exposure | One user transaction and no additional authorization; disclosed cost allocation, actual builder recovery and the mode's coverage target demonstrated across multiple batches |
| 4. Expand coverage and optimize | Add qualified builders; investigate pooled refunds and sponsorship separately without changing the wallet flow | Each new route independently passes the same economic and semantic checks; measured market coverage and posting latency improve; optional funding modes pass their own tests |

Implement the L2 track first; its accounting, quote and enforcement milestones
have their own acceptance criteria. The later L1 track qualifies refund collection
independently. Advertise and activate the combined paid service only after both
directions pass on the chosen network. A separately qualified L2-only release
can precede that milestone, with unfunded new inbound demand rejected and accepted
old work covered by its migration reserve. Reserve amounts, price windows and fee allowances require
calibration before enforcement, while the accounting/estimation implementation
can proceed without pretending those deployment facts are already known.

A protocol fee with separate execution, data and service components is a useful
comparison for L2: OP Stack explicitly separates execution, L1 data and operator
fees. It does not redirect ordinary Ethereum L1 tips to our poster. Its fee
formula and execution changes are not transferable to this fork without design
and replay work. [OP fee documentation](https://docs.optimism.io/op-stack/transactions/fees)

Calibrate over representative full workload cycles, including sparse intervals,
blob boundaries, failed posts and fee spikes. Use historical replay followed by
shadow operation; report sample counts, p50/p95/p99 forecast error and confidence
limits, never just average profitability. Choose allowance and exposure caps
from an explicit tolerated-loss budget. Require nonnegative total contribution
without discretionary refunds; separately report full operating margin and
liquid L1 runway. A finite successful sample is not a zero-loss proof.

An isolated [Titan Hoodi trial manifest](../audits/2026-10-10-cross-chain-fee-research/sponsorship/hoodi-trial.json)
is prepared, not executed: at most 0.06 test ETH for optional fixture deployment
and three batches covering empty, residual and funded poster balances. It can
test provider funding before changing Composer, using the existing mock verifier
and synthetic L1 users. Only a real builder-created transfer plus canonical
receipts can pass that mechanism check. Production cross-chain execution,
mainnet support and other builders remain separate qualification requirements.
The funded test-wallet/deployment configuration is still missing.

The [2026-10-11 Hoodi coverage check](../audits/2026-10-10-cross-chain-fee-research/README.md#hoodi-builder-coverage--2026-10-11)
found published, responding bundle APIs for Titan and Ultra Sound among the eight
reviewed providers. A parser response does not establish inclusion: Ultra Sound
appeared in seven-day delivery statistics but not the latest 24-hour table, and
the sampled relay records did not match Titan's published builder keys. BTCS keys
did match recent Hoodi deliveries, but no public Hoodi submission endpoint was
found. Public Hoodi routes for Quasar, BuilderNet, Beaverbuild, Eureka and Bombora
also remain undiscovered. Confirm builder inclusion availability before any funded
trial; Hoodi alone currently cannot qualify majority coverage or the pooled-refund
path. These are discovery and observation limits, not claims that private test
environments do not exist.

The implementation acceptance matrix must include:

- Tiny source/large remote execution; large DA/small execution; nested callbacks;
  static-only and reverting lookups; source reverts after remote simulation;
  expensive first proxy deployment; empty/sparse and multi-blob batches.
- Native zero-price receipts in positive-base-fee blocks, preserving gas totals
  while recording zero monetary fees; effective-tip truncation by `maxFeePerGas`;
  legacy fees; zero or changing
  beneficiary; same transaction on a different chain; stale/reorged quote;
  altered fees changing execution; lower receipt gas and EVM refunds.
- Omitted refund payer, skipped immediate entry, post-only/reverted post,
  partial consumption, late original inclusion after replacement, sender nonce
  conflicts and four consecutive speculative replacements/exhaustion.
- Fake/missing/delayed/wrong-recipient refund, refund selector defaulting to the
  wrong transaction, payout below its gas cost, unsupported fields silently
  ignored, and incompatible fan-out winning inclusion.
- Repeated submission/ACK/reconciliation, reorg reversing fees, restart losing
  queue state, duplicate refund records, expired quotes and reservation-release
  races. No duplicate revenue or financially unowned attempt.
- A single ordinary user transaction with no quote signature, payment permit or
  deposit; operator bundle authentication must not involve the user's key.
- Treasury conversion unavailable, fee withdrawal delay, self-funded tips,
  depleted working capital, no refunds, and mandatory ordinary backlog that
  cannot wait for new profitable work.

Sponsorship absent/present, residual-balance behavior, native blob funding and
combined sponsorship/refund tests belong to the later funding-mode expansion.
They are not v1 acceptance requirements. Successful gas funding alone cannot
pass that expansion's full service-cost check.

Use Composer/node unit tests and indexer tests for money arithmetic, attribution
and idempotence. Extend `settlement_policy`, `settlement_policy_cost`,
`dynamic_admission`, `cross_chain`,
`partial_consumption`, `queue_liveness`, `queue_reorg` and `chained_interstate`
process cases as appropriate, with nonzero test selection, semantic assertions
and independent follower replay. Signer/RPC tests apply when their boundary
changes; any later protocol extension needs its own full contract/vector/genesis
checks and is outside the L1 collection design here.
Native receipt tests do not validate a real PBS auction or refund payment.
Live Chiado/Ethereum trials need separate operational authorization and actual
route support; no live procedure was executed for this proposal.

## Compatibility and recovery

The proposed L1 rollout preserves transaction encoding, core ABI, DA and
historical replay.
Old clients can still submit sufficiently paying standard transactions, but an
enforcing Composer can reject previously accepted underpriced new work. Expose
capability and explicit errors; a global `eth_maxPriorityFeePerGas` suggestion
cannot encode a transaction-specific remote cost. Wallet/application integration
must apply the transaction-specific suggested fee before the normal signing
step; changing `eth_estimateGas` alone does not collect a service contribution.
Mixed Composer versions can differ in admission policy; route clients explicitly
and coordinate operator treasury reservations.

Roll out observability, then clients/quotes, then enforcement. Before switching,
account for existing accepted work under its old terms and allocate a finite
migration reserve; do not retroactively increase its charge. Reconciliation
remains enabled in either mode. Disabling enforcement stops new price checks
but does not erase builder receivables, expire live attempts or release
encumbered operator funds.
Persist financial schema/version and support recovery independently of node
databases; restoring the node database does not restore volatile user requests.

Builder adapters require capability/version gates and must preserve exact target
and submission ownership. A future fee debit or escrow commitment in execution
would be a separate hard compatibility boundary, outside the user's current
request. An old signer must reject a new unsupported profile. Never enable sender
binding, alter native system gas fees or reinterpret `etherDelta` as a service
charge to bypass the collection gap.

## Decisions and remaining evidence

Adopt the shared cost model, zero discretionary-refund credit, L2 tip collection,
and L1 builder recovery funded only through ordinary user tips as the proposed
direction. Before setting production prices, obtain: the deployed estimator
revision; a sanitized complete
cost/revenue sample; controlled treasury/asset redemption facts; and each target
builder's verified refund and exact-slot behavior. Pick latency/batching tiers,
operating markup and tolerated exposure from those measurements. Establish a
qualified builder set meeting the coverage target without adding a customer
payment or authorization. That capability remains an open integration milestone;
the current public documentation does not establish it as achieved.

Research and source tracing are complete to the scope recorded in the audit.
Production implementation, full Composer/signer/follower validation, actual
builder-payment validation and public-network deployment remain future work.
The local funding experiment above does not qualify those steps as completed.
