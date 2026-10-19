# Cross-chain fee recovery research

| Field | Value |
| --- | --- |
| Date and status | 2026-10-10 UTC; source/document research, local sponsorship verification and isolated nonempty-entry dependency checks below; proposal remains draft |
| Scope | Poster cost recovery through user fees, L2 tips, L1 builder rebates, admission, treasury and recovery |
| Source | `eduadiez/rollupX`, branch `main`, `fa916b13254a36423cad130080dde367bb18a7d6` |
| Protocol | Initialized clean `eez-core-protocol` at `855fe0602484750861b4c31502a1a91355d33ed6`; no pin change |
| Runtime executables | Initial research: none. Follow-up: Foundry `1.7.1` and pinned EEZ bytecode in a disposable local Anvil; no production node or public-chain deployment |
| Environment | Local repository inspection, public primary documentation and unauthenticated simulation-method probes; isolated Anvil chain `31337`, Osaka, EIP-7594; earlier Claude Code `2.1.296` text review |
| Related record | [Draft fee proposal](../../proposals/cross-chain-fee-recovery.md) |

## Findings

The fork has a posting-cost estimator and fee controls, but no user-price gate
that requires a cross-chain request to pay its attributable poster costs.
Raising a source-chain gas estimate does not collect an additional service fee.
L2 ordinary tips can fund a controlled treasury; L1 user tips need a separate
transfer/refund mechanism to reach the poster.

The later [Titan public API test](#titan-public-api-refund-source-limit) resolves
one previously open question: its tested mainnet and Hoodi validators reject
more than one `refundTxHashes` entry. Earlier unresolved-cardinality statements
below describe the evidence available before that test.

The [dependent-bundle follow-up](#dependent-bundle-admission-ordering-and-pricing)
adds six passing mock-proof contract checks, exact-revision public builder
source review, and an inclusion/payment arithmetic model. It establishes
protocol dependencies and pricing counterexamples, not live multi-bundle
admission or refund payment.

Recommend final-candidate cost coverage plus a separate downside/liquidity
reserve, with whole-group attribution and canonical reconciliation. L2 tips
are a comparatively small policy extension. Fixed L1 rebates need verified
builder integration; variable refunds cannot support a minimum-revenue promise.
The user's subsequent constraint excludes additional customer payments and
authorizations. The current proposal therefore uses only ordinary transaction
fees, with builder integration responsible for L1 recovery. The earlier prepaid
credit recommendation is superseded. Trustless fee settlement would need a
further protocol design and is not a prerequisite of this wallet flow.

The user reports working estimates on both endpoints. At this exact baseline,
the fronts forward `eth_estimateGas`; no complete transaction-specific service
quote is present. This limits what the checkout demonstrates and does not
disprove an estimator deployed from another revision.

## Source evidence

| Question | Production path and relevant existing tests | Finding and evidence type |
| --- | --- | --- |
| What do the two fronts validate? | [`ingress.rs`](../../../crates/eez-node/src/ingress.rs), `handle`, `gate_and_hold`, `upfront_cost`; source-chain/fee/nonce admission tests in the same file | Static: source affordability and replacement checks; no recovery contribution gate; other ordinary `eth_*` methods forwarded |
| Are source simulation and receipt gas interchangeable? | [`compose_crosschain`](../../../crates/eez-composer/src/composer.rs), [`SessionInspector`](../../../crates/eez-evm-inspector/src/inspector.rs) | Static: intercepted source gas explicitly differs from canonical receipt estimation; proxy result uses synthetic remaining gas |
| Who pays native L2 work? | [`eez-evm/evm.rs`](../../../crates/eez-evm/src/evm.rs), [`native fee fields`](../../../crates/eez-primitives/src/lib.rs), [signer §3 native profile](../../../crates/eez-proof-signer/SPEC.md); `native_execution_rejects_noncanonical_gas_fields_without_mutating_state` and `ordinary_transactions_still_pay_fees_and_outbound_eth_stays_at_the_system_address` in [`eez-evm/tests.rs`](../../../crates/eez-evm/src/tests.rs) | Static: native type `0x76` has zero gas price; ordinary user transactions pay standard fees; tests read, not run |
| Can tips reach the operator? | [`ComposerSettings`](../../../crates/eez-composer/src/composer/config.rs), [`node wiring`](../../../crates/eez-node/src/lib.rs), [public Chiado example](../../../.env.chiado.example) | Static: shared Live/Sync beneficiary exists; absent parser default zero, Compose explicit setting; deployed value not inspected |
| What is the poster's bill? | [`posting.rs`](../../../crates/eez-composer/src/composer/posting.rs), [`posting_fees.rs`](../../../crates/eez-l1/src/posting_fees.rs), [`CostReport::amounts`](../../../crates/eez-composer/src/composer/costs.rs) | Static: final ordered execution, separate execution/blob costs and user companions; cost ledger is observability, not charging |
| Does current receipt coverage prove refund economics? | [`settlement_policy_cost.rs`](../../../crates/eez-node-e2e/tests/settlement_policy_cost.rs), `ethereum_receipts_use_bounded_fee_history_priority` and `gnosis_receipts_use_bounded_fee_history_priority` | Static: native receipt tests explicitly use no PBS bid; not builder refund validation; tests not run |
| What refund support exists? | [`SubmitterConfig`](../../../crates/eez-l1/src/config.rs), [`bundle_request`](../../../crates/eez-l1/src/submitter.rs), `bundle_requests_carry_the_signature_only_when_a_key_is_configured` | Static: optional authentication and exact target pinning, no fixed refund request fields; identical-body fan-out |
| Are all posted users completed? | [core caveats](../../../eez-core-protocol/docs/CAVEATS.md), [entry contract](../../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md), [static contract](../../../eez-core-protocol/docs/STATIC_ENTRY.md), [RPC §8](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission) | Normative: partial consumption and lookups; completion needs canonical evidence, not a post receipt |
| Can a wrapper freely bind a proof to its payer? | [signer §§1 and 7](../../../crates/eez-proof-signer/SPEC.md), [signer contribution rules](../../../crates/eez-proof-signer/CONTRIBUTING.md) | Normative: current profile rejects sender binding; generic wrapper/escrow protection is not established |
| Is the existing indexer a profit ledger? | [`l1_fees.rs`](../../../monitoring/indexer/src/l1_fees.rs), [`l2_fees.rs`](../../../monitoring/indexer/src/l2_fees.rs), [indexer semantics](../../../monitoring/indexer/README.md) | Static: useful receipt accounting, not complete treasury/operations/refund profit |
| Are native zero-price receipts handled by that fee sum? | `block_fees` in [`l2_fees.rs`](../../../monitoring/indexer/src/l2_fees.rs), native `effective_gas_price` in [`primitives`](../../../crates/eez-primitives/src/lib.rs), [`fee fixtures`](../../../monitoring/indexer/tests/l2_fees.rs) | Static incompatibility: unconditional `price.checked_sub(base)` errors when a native zero-price receipt meets positive base fee; fixture coverage omits native receipt types. Not reproduced against a running indexer |

The source was traced at this fork and pin, not upstream `main`. Existing
tests establish intended coverage only; this research did not execute them.
The native receipt accounting mismatch is included as a stage-0 prerequisite,
not silently fixed during fee-design research.

## External primary sources

Consulted on 2026-10-10; policies can change. The proposal distinguishes these
published claims from our design recommendations and unverified deployment
capabilities.

| Source | Use and limits |
| --- | --- |
| [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559) | Effective priority payment versus fee cap, actual gas and base fee; not a service-payment contract |
| [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844) | Separate blob cost; use active fork parameters for pricing rather than old numeric limits |
| [BuilderNet API](https://buildernet.org/docs/api) | Fixed rebate fields, selection, recipients, authentication and reconciliation interfaces; no proof the configured relay implements them |
| [BuilderNet refund rule](https://buildernet.org/docs/refunds) | Variable orderflow redistribution; no guaranteed amount |
| [Flashbots Protect refunds](https://docs.flashbots.net/flashbots-protect/gas-fee-refunds) | Exclusivity and recipient differences between products; do not import Protect defaults into bundle submission |
| [Titan bundle API](https://docs.titanbuilder.xyz/api/eth_sendbundle) | Fixed rebate and sponsorship candidates; selector cardinality, payout and exact timestamp semantics require testing; displayed example has unit ambiguity |
| [Titan sponsorship explanation](https://titanbuilder.substack.com/p/titan-tech-teatime-1) | Linked by current API; funding is conditional on insufficient sender balance and builder recovery, not a general poster reimbursement |
| [Titan MEV-Share registration](https://github.com/flashbots/dowg/pull/1), [MEV-Share specification](https://github.com/flashbots/mev-share/blob/main/specs/bundles/v0.1.md), [Flashbots endpoint](https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint) | Historical Titan API support, generic refund/nesting semantics and current Flashbots backrun restriction; no verified Titan multi-user request |
| [Quasar bundle API](https://docs.quasar.win/api/eth_sendbundle), [Beaverbuild API](https://beaverbuild.org/docs.html), [Ultra Sound API](https://github.com/ultrasoundmoney/builder-docs/blob/main/bundle-rpc.md) | Explicit single-selected-transaction limits; Quasar also documents sponsorship |
| [Eureka bundle API](https://docs.eurekalabs.xyz/order-flow/api/eth_sendbundle.md), [Bombora bundle API](https://bombora.build/docs/eth-sendBundle/), [BTCS API](https://www.btcs.com/builder-docs/) | Eureka documents 1–16 refund sources; Bombora explicitly sums sources; BTCS array alone leaves limits/aggregation unqualified |
| [BuilderNet sponsorship proposal](https://collective.flashbots.net/t/proposal-builder-funded-gas-sponsorship-for-bundles/5820) | July 2026 community request, not evidence of an accepted or deployed feature |
| [Relayscan global builder table](https://www.relayscan.io/overview?t=7d) | Dated seven-day observed market sample; source labels not independently authenticated, not all L1 slots or workload inclusion probability |
| [Flashbots bundle pricing](https://docs.flashbots.net/flashbots-auction/advanced/bundle-pricing) | Builder competitiveness after allocating refunds; inclusion remains an auction outcome |
| [Gnosis networks](https://docs.gnosischain.com/about/networks/) | Chain IDs and native currency; no evidence of a particular builder's refund capability |
| [OP transaction fees](https://docs.optimism.io/op-stack/transactions/fees) | Comparison for explicit execution/data/operator fee separation; not a compatible implementation for this fork |

The former Flashbots Auction gas-fee-refund URL returned a fetch error; it was
not used as evidence. Current BuilderNet and Protect pages supplied the relevant
claims. No builder submission, refund-recipient mutation, payment, treasury
query or authenticated operator endpoint was used.

## Multi-user L1 follow-up

At the user's request, revisited Titan's public documentation on 2026-10-10.
The single-selected-source limit previously established for BuilderNet is not
evidence of Titan's accepted list length. Titan's selector descriptions do not
establish a pooled refund of several customers' tips. The proposal now separates
pooled rebates, conditional gas sponsorship, explicit credit, application/account
aggregation and a commercial batch payer. It adds batch pricing and a focused
provider qualification experiment. These remain design candidates.

Sponsorship is a potentially useful new route, but a funded production poster
does not automatically receive reimbursement. Residual balances, native blob
upfront requirements, principal and actual expense must reconcile over multiple
batches. The older MEV-Share support announcement also does not establish the
current request shape needed for our workload.

Titan's documentation index and a candidate `mev_sendBundle` documentation page
could not be fetched. Attempts to fetch rsync's primary documentation also
failed; no rsync capability is asserted from secondary search results. No live
bundle or testnet transaction was submitted. The two Claude review passes below
preceded this follow-up and did not review its new candidate mechanisms.

Repeated documentation whitespace, local link/anchor, pinned-path and fence
checks after this follow-up: 51 local links and 32 unique paths, including the
two new research records. Integer arithmetic confirms the illustrative
`0.004 ETH` aggregate tip, `0.003 ETH` pooled payout, and `0.00012 ETH`
single-selected-source payout. Runtime and builder-payment checks remain unrun.

## Majority-coverage follow-up

The user then required broad builder compatibility. Rechecked primary APIs and
downloaded the public seven-day Relayscan table directly because the browser
result contained an older cached snapshot. The downloaded page reports
2026-10-10 09:26:35 UTC, latest slot 15,400,031. Extracted the global builder
table once, excluding per-relay child tables and the separate relay-share
table. Its 15 rows sum to 46,023 blocks. Recomputed percentages from integer
counts and checked them against the published rounded percentages.

Retained the small structured [coverage evidence](builder-coverage.json),
including source URL, timestamp, attribution limits, counts, candidate unions
and source HTML/extracted-table SHA-256 digests. Full public HTML remains
local-only at `/tmp/rollupx-builder-coverage-relayscan7d.html`; the source is
dynamic, so the digest identifies this download rather than promising future
retrieval of identical HTML. No bulk page dump is added to the repository.

The explicit pooled-refund candidates Eureka/Bombora cover 7.7374%; documented
sponsorship candidates Titan/Quasar cover 73.5893%. Their union covers 81.3267%,
with no live common implementation established. The top five ordinary-bundle
candidates cover 97.5252%, and adding BTCS yields 99.0201%. This evidence moves
the recommendation at that stage toward separately authorized customer service
fees and funded ordinary bundles. The later user constraint below supersedes
that recommendation. The proposed 90% floor/95% target is a design choice, not a
user-approved numerical requirement or achieved live coverage.

The public MEVBoost dashboard was also fetched, timestamped 2026-10-10 06:00:00
UTC. Its per-pubkey `14d_total` table summed to 106,615, exceeding the 100,800
slots in an exact fourteen-day interval and differing from its displayed flow
chart's 92,315 blocks. This study did not resolve that window/counting mismatch
and did not use those table totals to substantiate coverage. Parsing that
flow chart also encountered null link values; no incomplete aggregation was
reported as a validated market share. Relayscan source attribution still needs
independent canonical/block-key validation before operational qualification.

The browser could not retrieve Eureka's method page; a direct public HTTPS
download succeeded and established its explicit refund-source bound. Public
rsync DNS resolution failed even outside the sandbox; Loki and Bob method
documentation could not be established. These are retrieval/qualification gaps,
not proofs that their services or features are absent. No authenticated builder
request, transaction submission, provider message or wallet operation occurred.

An additional Claude review was attempted after preparing a local prompt, but
automatic approval review rejected process creation: it considered exporting
the repository proposal and potentially linked source/spec content to Claude
broader than the external-transfer authorization it recognized. No additional
Claude review ran or result was received. Work continued through public-source
research and local analysis; the earlier two successful passes do not validate
this follow-up. No retry or indirect export was used.

Validation passed for this revision: whitespace checks of all three research
files; 53 local links, 33 existing paths with pinned-source/anchor checks and
balanced fences; JSON parsing, 15 unique builder rows and all six recomputed
coverage unions. Runtime suites and live payment qualification were not run
because this change contains research documents and public-data evidence only.

## User constraint: no extra payment or authorization

The user subsequently required no extra customer payment or authorization.
Revised the proposal throughout: one ordinary signed L1 application transaction
carries the service contribution in its existing fee fields; the quote needs no
signature; builder authentication uses operator keys. Removed prepaid custody,
payment permits and escrow from the active recommendation and rollout. Earlier
references to these options above and in the Claude review describe the research
history, not the current product decision.

Reopened Titan, Quasar and BuilderNet's primary API pages. The documented
selection/sponsorship limitations remain. The proposal now specifies each
receiving builder's required batch recovery economics, with pooled refunds
preferred and sponsorship separately qualified. It explicitly leaves majority
coverage unmet: a funded poster does not trigger shortfall sponsorship, and gas
funding alone does not recover compute/operations. Combining sponsorship with a
selected-source rebate is a proposed test, not established provider support.
No user is silently assigned everyone else's posting cost to work around a
single-source refund limit.

Added acceptance checks for the one-transaction wallet flow, actual blob
funding, residual balances and any combined sponsor/refund arrangement. Retained
canonical per-request reconciliation and bounded operator-funded risk; normal
gas caps do not authorize another debit for failed posting attempts. Provider
qualification remains future work. No provider was contacted, no live bundle
was submitted, and no further Claude review was attempted.

Validation passed after this revision: whitespace checks, balanced fences,
53 local links and 33 paths with pinned-source/anchor checks, unchanged fork
and clean protocol identity, all 15 builder rows and six coverage unions, and
the pooled-versus-single-source rebate arithmetic. No runtime or live builder
tests were run; only the proposal and this research record changed.

## Sponsorship verification

The user requested verification that the builder can fund `postBatch` from
several users' normal tips. The result is **local funding mechanics verified;
production integration and deployed builder behavior not verified**. Runtime
source, protocol and operator configuration were left unchanged.

### Source and public API findings

The current [`PostingRuntime::finalize`](../../../crates/eez-composer/src/composer/posting.rs)
reads the poster balance at the canonical fee-sample parent, subtracts the
blob-cap reserve and derives affordable execution gas from the remainder.
An underfunded post fails this gate or subsequent exact execution. The
[`observe_bundle_outcome` caller](../../../crates/eez-composer/src/composer.rs)
records `final_pricing_failed` and returns before dispatch. Its final simulator
starts with the post, without a sponsor prefix or funding state. This is a
static source finding, not a reproduced production Composer process failure.

Titan and Quasar document automatic shortfall funding subject to positive
bundle economics. A funded poster does not trigger that shortfall. Their public
documentation does not establish how the automatic implementation handles our
native-blob reserves, exact proof target and recurring non-gas costs.
[Titan API](https://docs.titanbuilder.xyz/api/eth_sendbundle),
[Quasar API](https://docs.quasar.win/api/eth_sendbundle).

On 2026-10-10 at 11:21 UTC, bounded unauthenticated `eth_callBundle` probes with
`params: []` returned HTTP 200 and JSON-RPC `-32601 Method not found` at all
three tested endpoints: Titan mainnet, Titan Hoodi and Quasar mainnet. No signed
transaction was sent. This establishes the response to these method probes,
not the absence of private simulation services or sponsorship support. Retained
requests/responses are in [public-probes.json](sponsorship/public-probes.json).
The documented [Titan tracing API](https://docs.titanbuilder.xyz/bundle-tracing.md)
and [Quasar tracing API](https://docs.quasar.win/bundle-tracing.md) report status
after bundle submission; an ACK or `SimulationPass` is not a funding receipt.

The web reader could not retrieve the documentation indexes; a direct public
HTTPS fetch succeeded. A sandboxed curl attempt first failed DNS resolution.
The successful indexes also exposed a current direct
[Quasar `mev_sendBundle` schema](https://docs.quasar.win/api/mev_sendbundle.md),
which was fetched and read. It documents nesting and refund-allocation fields;
it does not by itself validate our desired aggregate payment or sponsorship
semantics. Do not infer that its refund fields mean the same thing as the
single-source `eth_sendBundle` selector.

### Local experiment and limits

Added the [reproduction runner](sponsorship/verify-local.py) and
[test-only rollup/verifier](sponsorship/Probe.sol) because no existing fixture
exercises the funding prefix plus physical blob and residual-balance accounting.
The runner creates and stops its own loopback Anvil on an unused port, uses only
public disposable fixture keys, and retains diagnostics in a unique `/tmp`
directory. It reads no operator environment file, wallet or deployment.

The real pinned EEZ contract is deployed locally. Its artifact's compiler
`0.8.34`, optimizer runs `1`, `viaIR`, and all ten source-content hashes are
checked. The fixture binds the real blob hash, parent hash and timestamp, but
its verifier checks public-input equality **without validating an L2 execution
proof**. Execution tables are empty. Three synthetic L1 companions require the
registry's same-block verification marker. The payload is a real type-3
transaction with an EIP-7594 sidecar; its data is not a production semantic DA
stream. This is not a Composer/signer/follower E2E test.

The runner constructs the sponsor transfer itself. For underfunded posts,
Anvil's public txpool admission ignores the pending incoming transfer, so the
fixture temporarily supplies the cap balance for admission, restores the
original balance **before mining**, and asserts the exact mined transaction
order, receipt status and final balances. This fixture step is not a proposed
production balance-check bypass. The actual block execution includes the real
funding transfer ahead of the post.

The final run selected and passed seven cases:

| Case | Observed result |
| --- | --- |
| Zero-balance blob sender, no sponsor | RPC rejects with `Insufficient funds for gas * price + value` |
| Execution-cap funding without blob-cap funding | Same insufficient-funds rejection |
| One wei less than the full execution/blob reserve | Same insufficient-funds rejection |
| Fully funded post in the wrong slot | Post reverts; 131,072 blob gas is still charged |
| Sponsor funding, post, three users | All five transactions succeed in order; poster and builder balances reconcile |
| Next sponsored batch with residual poster balance | All five transactions succeed; top-up is only the new cap shortfall |
| Already-funded poster, post, three users | All four transactions succeed; no top-up and the poster pays its own bill |

In this deliberately synthetic workload, the first funding amount was
`0.001131072 ETH`, versus an actual post charge of approximately
`0.000083166 ETH`. User tips totaled `0.00594885 ETH`; the simulated builder
retained `0.004796778 ETH` after the transfer and its funding-transaction fee.
The remaining ETH stayed in the poster's balance. These numbers demonstrate
fee-cap liquidity and conservation, not a production quote, benchmark or agreed
builder payout. A successful gas-funding test does not establish recurring
proving/operations revenue.

The checked-in [local-results.json](sponsorship/local-results.json) contains
all seven results, source/protocol/executable identities, fixture hashes and
transaction hashes. Raw receipts and Anvil logs remain local-only in
`/tmp/rollupx-sponsorship-1t_4acqe/`; no retrievability from another checkout is
promised. The runner and small result record are retained for reproduction.

The [initial harness failure](sponsorship/initial-harness-failure.json) occurred
before any case ran: this Foundry CLI wraps decoded transaction JSON in a JSON
string, causing a Python type error. Corrected the parser; the next run passed
four initial cases. Added the reserve-boundary and wrong-slot controls, formatted
the fixture, and the final seven-case run passed. These were changed-harness
runs, not repeated identical tests used to hide a flaky result. The initial
diagnostics remain at `/tmp/rollupx-sponsorship-r8uc1_12/`, and the intermediate
four-case run at `/tmp/rollupx-sponsorship-gwnk2lam/`.

### Reproduction, checks and remaining gate

From the exact fork/protocol baseline above, with this research directory
present, Python 3.11 or later and Foundry 1.7.1 installed:

```bash
forge build --root eez-core-protocol
python3 docs/audits/2026-10-10-cross-chain-fee-research/sponsorship/verify-local.py
```

The protocol build succeeded with Solidity 0.8.34 in 297.51 seconds, with
existing compiler/lint warnings. The isolated fixture compiled successfully.
The final local runner exited 0 with seven intended checks executed. Full
protocol unit suites, production Rust/process suites, independent follower
replay, actual L2 semantics and live builder payments were not run; the local
test does not establish that coverage.

Final record checks passed for all eight research files: whitespace, 62 local
links and 38 referenced paths at the recorded pins, balanced code fences,
shell-example/Python syntax, Solidity fixture formatting, recorded fixture
SHA-256 values, seven result cases, fee conservation and three public method
responses. Existing operator edits and the clean protocol checkout were preserved.

Before a live builder trial, implement an explicit sponsored posting mode whose
composition/proof assumptions and final preflight account for the funding
prefix, retaining all protocol validation. Then prepare an isolated deployment,
disposable funded user wallets and a bounded test budget on a supported network;
obtain builder-created funding and canonical receipts, including residual-balance,
blob-cap, wrong-slot and omitted-payer cases. No provider message, public-chain
transaction, operator wallet funding or production service restart was performed.
This work does not qualify the earlier market-share candidates as live coverage.

### Public verification follow-up and prepared Hoodi trial

At the user's next verification request, checked the remaining public simulation
method and public test-network readiness. At 12:00 UTC on 2026-10-10,
`mev_simBundle` with empty parameters returned HTTP 200 and JSON-RPC
`-32601 Method not found` at Titan mainnet, Titan Hoodi and Quasar mainnet.
Together with the earlier `eth_callBundle` probes, this leaves no verified
public simulation route for automatic funding. These are method probes, not
valid bundle simulations. The requests and responses are retained in
[public-followup.json](sponsorship/public-followup.json).

The [official Titan endpoint documentation](https://docs.titanbuilder.xyz/general.md)
lists Hoodi. The public chain RPC returned chain ID `560048` and block
`3789137`, timestamp 12:01:00 UTC, with execution base fee `937876124` wei/gas.
A separate latest blob-fee call returned `879392855` wei/blob-gas. These reads
are a preparation snapshot, not an atomic fee quote for a signed target.
The five sampled deliveries from Titan's Hoodi **relay** used no key in the
[published Titan builder key list](https://docs.titanbuilder.xyz/builder-public-keys.md).
This does not prove Titan is inactive: it leaves testnet builder identity and
inclusion availability unestablished. A relay can deliver other builders'
blocks. Retained [chain-readiness.json](sponsorship/chain-readiness.json)
records the public requests, responses and documentation digest.

No existing Hoodi deployment was found in the searched runbooks, deployment,
testing, script and example-configuration paths. No Quasar testnet endpoint
was found in its current official index/API documentation; this is a discovery
gap, not proof that no private test endpoint exists. Asked the user for a
disposable funded Hoodi wallet/deployment configuration reference, or a choice
to prepare a new trial; no answer was available during this preparation.

Also attempted a read-only search for existing funding examples in forty
mainnet blocks, `26161856` through `26161895`. Every full-block request returned
HTTP 403. **Zero blocks were inspected**, so no positive or negative sponsorship
conclusion follows. The [failure record](sponsorship/historical-read-failure.json)
retains the attempted range and digest of the local-only detailed responses.
No alternative credentials or authenticated endpoint were used.

**Correction to the preceding implementation gate:** an isolated builder trial
can run before adding production sponsored posting. Reuse the pinned EEZ and
the existing mock verifier to construct a standalone bundle, then observe what
the builder actually includes. Composer still needs funding-aware composition
and final preflight before a production route can use this result. The mock
test cannot establish actual L2 proof semantics or independent replay.

Prepared a concrete [Hoodi trial manifest](sponsorship/hoodi-trial.json), with
three cases: initially empty poster, subsequent batch retaining its residual
balance, and fully funded poster as a control. Each submits one native-blob
post and three ordinary synthetic user transactions. We do **not** supply the
sponsor transfer in this trial; the test needs an actual builder-created
transfer, canonical successful receipts and reconciled balances. No refund
fields are included, so this trial isolates sponsorship. Non-inclusion is
inconclusive, not evidence of rejection or support.

The proposed ceiling is **0.06 Hoodi test ETH**, including disposable-user
funding and optional deployment of both test contracts. Conservative allocations
total `0.054230144` test ETH: two deployments capped at 8,000,000 gas each and
2 gwei, registration, nine user transactions capped at 100,000 gas and 22 gwei,
the funded-control poster's execution/blob reserve, and four funding-transfer
fees. User/account prefunding and its later gas expenditure are not counted
twice. Actual unused funds remain in the disposable accounts. Stop rather than
increase caps if fresh target fees, available funds or bundle economics fail.
The manifest bounds submissions to eight target attempts per case, three
included batches total, with canonical nonce/receipt reconciliation between
attempts. Public local-fixture private keys must not be reused on Hoodi.

Preparation used no private key, public-chain submission, wallet funding,
deployment, service change or builder message. The remaining input is an
appropriate funded test setup and, if one must be created, authorization for
that bounded deployment. A Titan Hoodi success would still leave Titan mainnet,
Quasar, broad builder coverage and production proof integration unqualified.

Follow-up checks passed for twelve research files: whitespace, 67 relative
documentation links, JSON/Python syntax, exact budget/reserve arithmetic, three
new method-error responses, and unchanged local fixture hashes/seven-case
record. No runtime source changed; runtime suites and live inclusion were not
rerun or represented as new coverage.

### Aggregate payout and explicit value follow-up

The user challenged the need for builder-side refund calculation and asked to
request one aggregate receipt. Clarified that the operator can price the batch
itself, including profit; the public API's permitted payment base and amount
encoding are the outstanding constraints. Re-read Titan, Quasar, BuilderNet and
Bombora APIs. Bombora explicitly sums selected anchors. Quasar and BuilderNet
document one selected hash; Titan documents a percentage of a selected payment.
No arbitrary absolute refund-amount field was established in these APIs.

The [follow-up evidence](sponsorship/refund-aggregation-probes.json) retains two
empty-parameter `mev_sendBundle` probes: both public Titan and Quasar mainnet
endpoints returned HTTP 200 / `-32601 Method not found`. Quasar's current
`mev_sendBundle` schema was downloaded again successfully. The mismatch with
that page and Titan's earlier support announcement remains unresolved. These
malformed probes submitted no transaction or valid bundle and do not qualify
or exclude authenticated/private routes.

Inspected public `flashbots/rbuilder` at exact revision
`6037fa728b13bf1806e16fff2586414216f6b8fa`. Its
[`commit_bundle_no_rollback`](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/building/order_commit.rs#L824)
sums positive builder-balance changes for transactions admitted by
`is_tx_refundable`, then applies the refund percentage. This is static source
evidence only; it does not establish public API cardinality, a provider's deployed
revision, or acceptance of a MEV-Share multi-user request. The source digest and
local-only download location are retained with the probes.

Added a candidate to the proposal because Titan's current documentation
explicitly includes **transaction value** in its sponsorship. An operator-signed
treasury payment, funded by the builder from combined user tips, could encode a
fixed payout including margin. This would add a backend transaction without
another user authorization. Keep the native post normally funded to isolate the
value-funding question. Quasar only explicitly promises gas funding; no common
implementation is inferred. Read the pinned multi-prover posting flow and
confirmed `EEZ.sol:371` declares `postAndVerifyBatch` without `payable`; a service
payment cannot simply be attached as native value to that call.

This corrects the overly broad reading that sponsorship could never support
profit. The gas-only experiment establishes no recurring service margin; the
new **explicit value-payment candidate is untested**, including policy/value
limits, target/atomicity and residual-balance economics. It is not covered by
the prepared Hoodi gas-only manifest. No runtime changes, public funding,
transactions or provider communications occurred.

### Explicit operator-value verification

On the user's request to check the new fixed-payment candidate, re-established
the same fork/protocol identities and clean protocol checkout. Re-read the
pinned posting flow, relevant core execution rules and caveats before extending
the isolated experiment. Titan's current API and linked sponsorship article
both explicitly mention funding transaction value as well as gas; Quasar's
current API explicitly mentions gas only. These documents still do not establish
acceptance of an arbitrary operator service payout or a shared majority route.

Extended the existing [runner](sponsorship/verify-local.py), retaining the original
seven checks and adding eight value-payment checks. No production Rust, protocol
Solidity, fixture contract, operator wallet or runtime configuration changed.
The old gas-only runner is preserved locally at
`/tmp/rollupx-value-sponsorship-20261010/verify-local-gas-only.py`, SHA-256
`3ddc9dae96627f8efa302af6a3847400c598ec873411ab24b0cc956a430178e4`.
The earlier `local-results.json` remains historical evidence with that older
runner hash. Use the new [value-local-results.json](sponsorship/value-local-results.json)
for the current runner's fixture hashes and all fifteen checks.

The run used a new owned loopback Anvil, chain `31337`, Foundry 1.7.1 and the
previously built pinned EEZ artifact after rechecking its source hashes and
compiler metadata. It deployed a freshly compiled unchanged mock fixture.
The first run of the expanded harness passed all fifteen selected checks and
exited 0. It retained receipts/logs at `/tmp/rollupx-sponsorship-0ooax8k1/` and
stopped its own Anvil. The archived full EEZ build was not rerun because its
checked inputs and artifact were unchanged. No identical-test retry occurred.

The new positive shape is:

```text
harness builder funds payout sender -> prefunded native-blob post
  -> three users -> operator value transfer to treasury
```

The payout sender, poster and treasury are distinct operator accounts. Only the
harness's builder supplies external value in the sponsored cases. The same
txpool-only admission workaround described above restores the payout sender's
original balance before mining. All mined transfers are real EVM execution;
the sender is not secretly prefunded at execution. User signatures remain
ordinary fixture transactions. Execution/static tables are empty and proof
verification is still mocked; no actual L2 prover/signer/follower ran.

| New case | Result |
| --- | --- |
| Empty payout sender, no funding | Rejected for insufficient gas/value funds |
| Sender has gas-cap reserve but lacks payment value | Same rejection |
| Sender is one wei below value plus gas-cap reserve | Same rejection |
| Service value attached to pinned post | Reverts; paired zero-value call succeeds at the identical parent; both pay blob gas |
| Empty payout sender, explicit funding, post and three users | Six ordered successful transactions; treasury receives requested value and both operator/builder balance changes are positive |
| Next batch retains payout sender balance | Six ordered successes; top-up decreases, all balances and fee burn reconcile |
| Prefunded payout sender without sponsorship | Treasury receives the same value, but combined operator assets decrease by gas/blob charges |
| Oversized operator payout with explicit funding | EVM execution succeeds locally but builder balance decreases; this demonstrates failed economics, not an observed builder rejection |

The requested local payout was `0.0024 ETH`. In the **second** sponsored batch,
external funding was `0.002421 ETH`, the payout cost `0.000021 ETH`, and the
post cost `0.000066066000131072 ETH`. The payout sender kept its previous
`0.000021 ETH` residue, so combined operator assets increased by
`0.002333933999868928 ETH`. After an explicitly **assumed**, unmeasured
`0.0002 ETH` non-gas cost, illustrative contribution is
`0.002133933999868928 ETH`. The builder retained `0.00265185 ETH` before any
validator bid. The initial batch also increased payout-account working capital;
that one-time reserve change is not presented as recurring margin.

The prefunded control still paid `0.0024 ETH` to the treasury, but operator
assets fell by `0.000087054000131072 ETH`. A `0.01 ETH` payout instead made the
builder lose `0.00494815 ETH`. These accounting controls prevent counting our
own transfers as revenue and prevent pricing from operator receipts alone.
Prices, gas and the non-gas allowance are synthetic, not production quotations.

#### Public endpoint and on-chain checks

Retained [value-public-evidence.json](sponsorship/value-public-evidence.json).
Empty-parameter `eth_sendBundle` controls reached parameter validation: Titan
mainnet/Hoodi returned code `-32700` with an embedded `InvalidParams` error;
Quasar returned `-32602 Invalid parameters`. These differ from the earlier
`-32601` simulation/MEV-Share responses. No valid bundle was submitted and these
method controls establish no funding policy.

Public full-block/receipt reads now succeeded; the earlier forty HTTP 403
responses remain recorded failures. Fetched fifty transactions from Titan's
published fee-recipient address via the public explorer, then nine qualifying
recent blocks containing early/mid-block payments (bounded to sixteen).
Twenty-two builder-address payments were observed. Two payments preceded later
transactions from their inferred forwarded recipients. Both later transactions
had **zero native value**, and both recipients also had an earlier outgoing
transaction in the same block. Parent balances therefore do not establish
their balances at the funding point. No complete state trace or authenticated
builder-key attribution was performed. This sample establishes no positive or
negative conclusion about our explicit value-payout pattern.

#### Remaining qualification

Prepared [hoodi-value-trial.json](sponsorship/hoodi-value-trial.json) specifically
for the value question; the preceding gas-only manifest does not test it.
It uses a funded native poster and a separate payout sender, three included
batches, and at most eight targets per case. Conservative deployment, user,
poster and control allocations total `0.058338432` Hoodi test ETH beneath a
`0.06` cap. Its proposed payout is `0.0015` test ETH because the live manifest's
user tip caps differ from the local fixture. Fresh target simulation and positive
builder economics are required; no fixed inclusion or price claim is made.
The user has no funded test setup, so deployment and actual submissions remain
unexecuted. No production-funded substitute or provider message was used.

The required builder confirmation or controlled test must establish value-funding
limits and permitted purpose, funding of a later non-post sender, same-block
funding/payout atomicity, recurrent balance handling, native-blob bundle support
and actual canonical payments. An ACK or profitable local simulation is
insufficient. Composer's current final simulator uses canonical parent state
without the funding prefix; even with a funded native poster, an underfunded
appended payout needs explicit simulation/ownership support. Its first-transaction
blob assumptions prohibit blindly prepending funding to the existing raw vector.
No validation bypass is proposed. Runtime suites, actual L2 semantic validation,
independent replay and live builder-payment checks remain unrun.

Final follow-up checks passed for sixteen research files: whitespace, 75 relative
documentation links, JSON/Python/shell-example syntax, balanced fences, Solidity
fixture formatting, all fifteen intended local results, current and historical
runner identities, consolidated balance equations, three method controls, the
nine-block public sample and the prepared trial's exact budget. Operator edits
and the clean pinned protocol checkout were preserved.

## Independent review and iteration

The user invited iteration with Claude. Two read-only CLI review passes were
requested with only `Read`, `Grep` and `Glob` available, customizations/MCP
disabled and no saved session. The first sandboxed attempt failed with
`EAI_AGAIN`; a separate short availability attempt timed out. Network access
was then granted through the environment's approval mechanism.

The first completed review used the CLI-reported model `claude-opus-5-5`,
55 turns, about 288 seconds. It independently identified the missing recovery
gate, L1/L2 payer split, zero native fees, zero-beneficiary risk and liquidity
gap. It also offered tentative suggestions requiring correction:

- A low-cost/marginal-only fee omits shared post/blob costs and post-only losses.
- A trailing settlement transaction that skips an empty vault cannot secure
  repayment, and a builder promise is not on-chain atomic enforcement.
- An omitted/replaced user can leave a paid post behind; bundle rejection is
  not the only failure outcome supported by this fork.
- Fixed same-block rebates differ from variable redistributions. Refund APIs
  can provide attribution, but a received payment still needs verification.
- A native asset's backing does not by itself establish liquid 1:1 revenue.

The second pass reviewed the actual draft using the same reported model,
21 turns, about 183 seconds. It confirmed that the main distinctions were
resolved and identified the following refinements, now incorporated:

- Specify sender/direction fee blocking separately from FIFO resource cuts and
  batch-occupancy funding failures; signed fees cannot be repriced in place.
- Allocate already-required posting overhead to its existing backlog funding,
  avoiding a second charge to new cross-chain groups; measure the idle cost floor.
- Bound cumulative financial exposure across attempts, independently of retry
  counters and without counting one source's eventual tips more than once.
- Account for canonical nonce conflicts and disappearing co-admitted payers;
  reserve discount shortfalls and apply global limits as well as client controls.
- Condition the fixed-rebate formula on the provider's verified customer-funded
  base; report recovery of the poster's own tip separately.
- Revalidate controlled beneficiaries and price internal treasury sweeps without
  treating their principal/tips or burned-fee backing as external income.
- Split L2 enforcement from the more substantial prepaid-credit work; reject
  unfunded new inbound requests unless explicit reserved sponsorship applies.

It also requested an evidence record and committed references for Compose
behavior. This audit supplies that record; the proposal now links the committed
runbook/tests and the baseline `git show` check is recorded below. The additional
native-receipt indexer finding was independently identified during source tracing
and added as an accounting prerequisite, not claimed as a Claude-validated test.
Model feedback is treated as review input, not as protocol authority or executed
validation. Unverified statements from the review are not adopted as facts.

Only sanitized findings are included here. Raw prompts/responses remain
local-only under `/tmp` and are not available to another checkout:

| Evidence | SHA-256 |
| --- | --- |
| First review prompt | `498e65ea41de59222b7116e30b494df0f6b58fe6d789ae5bb955262ae8c60a48` |
| Initial network-failure JSON | `d33ad2cf81ef71054288b001e8794818124a6b593530b2337e0c736cb3963c0e` |
| First successful review JSON | `959b7db2014f12aae113aeb6df4bfbd944bfb76fd4fb7c27f45f653c38b97b4d` |
| Second review prompt | `f0e7ce7c5151f1719b9eea3621ceabd920e04914e1904f3cbde397fa176d9bff` |
| Second successful review JSON | `331d7105331a728e28acb12e4d52f1fcc23d0148fc63fe315b8dbd1b11224389` |

## Initial research checks and limits

Baseline commands ran from the repository root: `git status --short`,
`git remote -v`, `git rev-parse HEAD`, `git ls-tree HEAD eez-core-protocol`,
`git submodule status --recursive`, and `git -C eez-core-protocol status --short`.
The pin was already initialized and clean. Existing Compose modifications,
other untracked proposals/audits, worktrees and operator backup state were left
in place; none form part of the proposed fee change.

Selected source/spec reads and `rg` searches covered the production paths above.
`git show HEAD:docker-compose.chiado-node.yml` confirmed the committed fee
recipient/authentication wiring separately from the operator's edited file.
The Claude CLI was invoked in print/plan mode with safe mode, strict MCP
configuration, read-only tools and no session persistence. The successful
network-enabled research invocation had a 300-second process deadline;
the follow-up had a 240-second deadline.

Python integer arithmetic checked the proposal examples:

| Case | Computed result |
| --- | --- |
| 1,200,000 execution gas at 12 gwei plus 131,072 blob gas at 1 gwei | `14,531,072,000,000,000` wei |
| Illustrative 20% total allowance | `17,437,286,400,000,000` wei |
| Ten L2 sources each using 80,000 charged gas | Required effective tip `21,796,608,000` wei/gas |
| 100,000 L1 gas, 80% rebate, `10^15` wei contribution, `210,000,000,000,000` wei payout deduction | Required effective tip `15,125,000,000` wei/gas; net receipt `10^15` wei |

Documentation checks passed: `git diff --check`; whitespace checks of both new
files using `git diff --no-index --check /dev/null <file>` (exit 1 denotes the
new-file difference, with no whitespace diagnostics); and an inline Python
link/anchor validator covering 51 local links and 30 unique existing paths.
Existing paths were also checked with `git cat-file -e` at the exact fork or
protocol revision above. Code fences are balanced. There are no shell examples
in the new documents requiring `bash -n`.

Runtime tests, Solidity builds, process
semantic assertions, independent replay, benchmarks and live builder/treasury
tests were not run: only research documents were changed. The proposal lists
those future acceptance checks explicitly. No bug-fix baseline/candidate
comparison or deployed profitability claim is made.

## Remaining evidence before implementation

Identify the estimator's deployed revision; obtain complete sanitized receipt,
cost and external-revenue samples; verify treasury ownership and redemption;
reproduce the indexer's native-receipt accounting gap; and establish sufficient
builder coverage for inbound recovery without extra user payments or
authorizations, including non-gas costs. Fixed margins,
refund ratios, reserve sizes and production tip levels remain uncalibrated.
The proposal's formulas and illustrative numbers are not live prices.

## Common-payment compatibility analysis

Follow-up on 2026-10-10 to the request for the best solution across builders.
The source remains `fa916b13254a36423cad130080dde367bb18a7d6` with clean protocol
gitlink `855fe0602484750861b4c31502a1a91355d33ed6`. Rechecked git status, remotes,
HEAD, gitlink and recursive submodules before editing research records. Existing
Compose changes, operator configuration, other audits and worktrees were
preserved. No runtime or protocol implementation was changed.

The [source manifest](common-payment-sources.json) records eight provider
profiles and ten public document reads beginning at 15:43:33 UTC. Nine raw
downloads succeeded. The direct BuilderNet request failed with network
unreachable; its official API page was successfully read with the browser tool.
Successful raw bodies and source-read metadata remain local-only under
`/tmp/rollupx-builder-compatibility-20261010/`; URLs, timestamps and SHA-256
digests are retained, not a promise of durable copies of the original pages.
No valid or malformed builder submission was made in this follow-up.

The broadest documented common payment shape is a funded `eth_sendBundle`
with a percentage refund from one real user's transaction to our treasury.
Titan, Quasar, BuilderNet, Eureka, Bombora, BTCS, Ultra Sound and Beaver all
describe single-source/default-final-transaction refunds. The last-user default
avoids depending on Titan's ambiguous selector fields, but requires an execution
order whose final transaction is the intended payer. This is a documentation
finding, not a qualification of blobs, exact targets, payout math or inclusion.

That common shape funds a shared post only when the one selected customer's
payment covers its entire required receipt. It also supports the design of
separately priced, independently posted request groups, at the cost of repeated
fixed overhead. Neither approach is evenly shared pooled collection. Existing
signed tips cannot be enlarged by choosing a different builder. Payer selection
must preserve complete replay groups, sender nonce dependencies and the pinned
execution order; a compatibility fallback cannot silently change the quote.

The desired net receipt can remain identical across providers. The unresolved
integration is how each pays it. Among reviewed sources, Eureka and Bombora
explicitly support several contributing transaction hashes. Quasar and
BuilderNet cap that array at one. Titan's gas-and-value sponsorship remains a
different candidate; an absolute-payout field is not established as a common
API. Recommend the single-source mode for qualification of broad compatibility
and pooled refunds for efficient batching, seeking multi-source support from
the providers that lack it. No provider was contacted and no support agreement
is claimed.

Reviewed the following alternatives and their limits:

- A large operator-funded refundable transaction loses its unrecovered tip
  and transfer fees. It cannot convert other users' tips into external income.
- Separate copies of `[post, user_i]` have a shared nonce conflict. Allowing
  a previously executed post to be omitted does not make the other bundles'
  admission or refunds compulsory; merging same-recipient payouts does not
  supply joint inclusion. No generic profitable shared-batch construction was
  established from the reviewed APIs.
- MEV-Share defines nesting and refund allocation, but the current Flashbots
  public RPC documents one backrun transaction. Earlier Titan/Quasar public
  method-not-found evidence remains a qualification gap, not proof of every
  private endpoint's behavior. No new success is inferred from the schema.
- Fixed builder-to-operator payment by explicit agreement would express our
  desired amount directly. It still requires provider integration; a contract
  cannot unilaterally debit the block beneficiary's priority-fee receipts.
- A wrapper, account delegation or an extra user fee/payment is not an
  automatic fallback under the user's transaction-only fee constraint.

Re-read the pinned core execution/lifecycle rules, execution entries, static
lookup rules, caveats and signer profile. Same-block table expiry, replacement
and forward-scan partial consumption prevent treating a post receipt as proof
that all contributing users executed. The signer does not currently admit
sender binding. Inspected the existing submitter's `bundle_request`: it has
exact block/timestamp constraints and reverting hashes, but no refund request
fields. This follow-up adds no binding, fee gate or builder adapter.

Re-read the earlier downloaded rbuilder source at exact public revision
`6037fa728b13bf1806e16fff2586414216f6b8fa`, including refundable-profit
accumulation and payout creation. That code suggests a possible reuse point
for providers implementing multi-source selection; it does not establish their
deployed revision, the public admission schema, or that lifting one array
limit is sufficient. The existing [source record](sponsorship/refund-aggregation-probes.json)
retains its digest and provenance.

The [integer model](common-payment-analysis.json) checks the economic trade-off
using explicit synthetic assumptions: ten users, 90% refund, `0.006 ETH` required
batch receipt and `0.0001 ETH` per independent payout. It records five shared-post
arrangements, a repeated-post cost comparison and an operator-funded negative
control. Seven check groups passed: pooled versus single-source coverage,
maximum-percentage failure for an insufficient source, minimal rounded quotes,
tip conservation, equal total profit assumptions and the negative self-funded
payment. These are arithmetic checks, not builder or EVM simulation.

The comparison holds a `0.0005 ETH` aggregate target profit constant. With
`0.003 ETH` fixed service cost per post and `0.00025 ETH` per-user marginal cost,
ten separate posts require `0.033 ETH` in net receipts rather than the shared
batch's `0.006 ETH`. Approximate user tips are `0.037778 ETH` versus
`0.006778 ETH` under the stated payout model. Actual proof/blob/gas costs must
be measured before using this relationship for a production quote.

No additional Claude review was attempted: the earlier automatic-review
rejection remains recorded above. Prior successful Claude reviews did not
review this new comparison. The previous fifteen local sponsorship cases were
not rerun or claimed as new coverage. Public-chain funding, live payouts,
production signing, semantic E2E and independent replay remain unverified.

Validation passed for all eighteen research files: whitespace and applicable
JSON/Python syntax, balanced Markdown fences, 81 relative links/heading anchors,
nine retrieved source digests, eight explicitly unqualified provider profiles
and five recomputed shared-post financial vectors. The seven arithmetic check
groups passed when generating the model. The prior local runner's hash is
unchanged, `git diff --check` passed, and the protocol worktree remains clean.
No shell examples or runtime source were edited, so shell execution, Rust,
Solidity and process suites were not required or run for this follow-up.

Evidence SHA-256:

- `common-payment-analysis.json`:
  `367adffc85d7abc01986cf502546f2e8328b5be88cdc06544696fb6f3a666386`.
- `common-payment-sources.json`:
  `f6086c3a1f006f9d97565bb22711d5d53e135322ee5ed16063f42aa96c931d49`.

### Higher tips and later user rebates

The user subsequently accepted higher quoted fees and optional later rebates
as a practical trade-off. Updated the draft to propose the funded single-source
refund as the initial L1 collection mode, retaining a shared post whenever its
selected user payment covers the full candidate. Uneven allocation and higher
fees are no longer treated as launch blockers in themselves. This preference
does not establish deployed builder support or turn a shared post into a refund
based on all its users' tips.

Re-read the current official Titan, Quasar, BuilderNet and Bombora API pages.
No capability change was established: selected-source refunds are documented;
Quasar and BuilderNet still specify at most one source hash. Sending ETH rebates
from the operator to original included senders would require no further user
transaction or signature, but must be paid from settled external revenue after
actual costs, retained profit, reserves and rebate transfer costs.

The new [rebate arithmetic record](later-user-rebates.json) extends the earlier
synthetic high-quote example without modifying its historical model or digest.
The ten users collectively pay about `0.067778 ETH` in tips, but only `0.006 ETH`
is refunded to us when one source is selected. With `0.0055 ETH` of costs and
`0.0005 ETH` retained profit, no user rebate can be funded from that receipt.
With actual costs of `0.005 ETH`, the same retained profit, `0.0001 ETH` risk
reserve and `0.0001 ETH` transfer cost, a `0.0003 ETH` user rebate is affordable.
The unselected tips are never counted as operator cash. These remain invented
prices and accounting checks, not live transactions or production fee quotes.

Five arithmetic checks passed across two cases, covering the surplus bound,
uncharged transfer gas when no rebate is made, zero surplus, positive surplus
and separation of user-paid tips from operator receipts. No builder submission,
wallet operation, service change or source implementation occurred.

## Sponsored post tip and treasury refund

Follow-up completed on 2026-10-10 at approximately 16:51 UTC. The user asked to
check whether the post itself can be the refund source when its gas is builder
sponsored. Re-read the official
[Titan](https://docs.titanbuilder.xyz/api/eth_sendbundle) and
[Quasar](https://docs.quasar.win/api/eth_sendbundle) API pages. Both separately
document sponsorship and percentage refunds capped at 99%. Neither reviewed
page explicitly guarantees a refund calculated from a sponsored transaction's
gross tip. Titan's first-transaction selection remains ambiguous; Quasar's
single-hash selector is explicit. These are documentation findings, not live
acceptance or payout evidence.

The July 2026
[BuilderNet sponsorship proposal](https://collective.flashbots.net/t/proposal-builder-funded-gas-sponsorship-for-bundles/5820)
discusses deducting sponsor capital before computing refundable value. It is a
community proposal, not an accepted specification, and does not establish Titan
or Quasar accounting. It motivates a separate counterfactual local control;
the control does not claim to implement that proposal's exact formula. No new
builder RPC probe, valid bundle submission, provider contact or public-chain
transaction occurred. The earlier lack of a funded disposable test setup still
prevents receipt-backed provider qualification.

Extended the existing [runner](sponsorship/verify-local.py) with seven cases,
preserving its previous fifteen. The earlier fifteen-case runner was copied
before editing to the local-only
`/tmp/rollupx-sponsored-tip-20261010/verify-local-before-tip.py`, with SHA-256
`f05453dd74568bab6e26f6d819822093e9debce735dc2b7a7a362f40dcf49839`.
Historical result files and their hashes were not replaced. The new
[tip-refund-local-results.json](sponsorship/tip-refund-local-results.json)
records all 22 cases and the updated runner hash.

Executed from the repository root:

```text
python3 docs/audits/2026-10-10-cross-chain-fee-research/sponsorship/verify-local.py
```

The first execution of this extension passed all 22 intended cases, exit 0.
It created and stopped only its own local Anvil on an unused loopback port,
chain 31337, Osaka, with public disposable fixture keys. It rebuilt the small
mock fixture offline using Solidity 0.8.34, optimizer runs 1 and `via_ir`.
Foundry, Cast and Anvil were version 1.7.1; executable hashes are in the result.
The pinned EEZ artifact was reused only after all ten metadata source hashes
matched the source files. Source and protocol identity remain the exact
revisions recorded above. Raw receipts, the build log and the node log are
local-only at `/tmp/rollupx-sponsorship-gzx7_x0x/`; the compact result is retained
in this audit. No shared service, wallet, deployment or operator state was used.

The new cases manually sign a funding prefix and, when nonzero, a refund suffix
from the local builder account. Between them execute the actual pinned
native-blob post and three synthetic L1 companions. The mock proof validates
public-input equality, not L2 execution. A snapshot preflight determines actual
post gas for the refund and is fully reverted. The pre-existing Anvil txpool
workaround temporarily supplies a balance only to admit a pending sponsored
post; all balances are restored before the actual block is mined. The final
block's funding is exclusively the signed transfer. Assertions check restored
balances/parent, ordered successful receipts, the native blob, and conservation
across all tracked accounts after execution-base and blob burns.

The principal assumptions are a 1-gwei execution base fee, 20-gwei post tip,
100,000 signed post gas limit, 1-gwei blob fee cap, three 50-gwei user tips and
a 99% refund after a 21,000-gas transfer deduction. This uses Titan's displayed
deduction order as a hypothetical formula, not as a verified combined policy.
The actual local blob base fee is 1 wei. Production refund transfers can have
different overhead and ordering. The non-gas cost of `0.0002 ETH` is illustrative.

| New case | Observed result in ETH | Meaning and limit |
| --- | --- | --- |
| Sponsored post, refund to treasury, first batch | Funding `0.002231072`; treasury `0.0012873168`; poster working balance increases `0.000843685999868928` | Initial reserve is separate from recurring income |
| Same route, residual balance retained | Funding `0.001387386000131072`; treasury `0.0012873168`; poster change zero; builder change `0.003698467199868928` | Recurring receipt works under the manually supplied gross-tip policy; builder bid not charged |
| Sponsorship deducted from selected-source refund base | Treasury and combined operator change zero | Counterfactual policy sensitivity, not observed provider behavior |
| Prefunded post, no sponsorship | Combined operator change `-0.000100054800131072` | Refunding our own funded tip is a net expense |
| Refund back to poster, first batch | Poster grows `0.002131002799868928`; treasury zero | One-time balance buildup does not demonstrate recurring service income |
| Refund back to poster, repeat batch | Funding `0.000100069200131072`; combined operator change zero | The refund reduces the next funding shortfall |
| 500,000 signed gas reserve, otherwise same policy | Builder change `-0.0055452212` | EVM-valid with manual funding but economically inadmissible to a profitable builder |

In the separate-treasury repeat case, external users pay `0.00509385 ETH` in
tips. The post's actual tip is `0.00132132 ETH`; total posting cost including
that tip is `0.001387386000131072 ETH`. The received `0.0012873168 ETH` less
assumed non-gas expense leaves `0.0010873168 ETH`. The complete balance equation
is `U + P - F - R - H` for the builder, so the high post tip is not counted as
new external revenue. The first batch's standing reserve and the negative
controls are not reported as sustainable profits. No fee caps are changed on
user-signed transactions.

This verifies the local mechanics and conditional economics only. Actual
builder-created sponsorship and refund, native-blob eligibility, selection of
the post, refund-base netting, auction inclusion and exact-target behavior still
need a provider trial. The current Composer finalization balance gate requires
an explicit funding-aware design before this could reach the submitter. No
runtime implementation, production build, semantic Composer/L2 E2E or independent
replay was changed or run. The proposal keeps the user's accepted high-tip,
funded single-source launch profile and adds this separately qualified candidate.
No further Claude invocation was made; its earlier reviews do not cover this
extension.

Validation passed for all twenty research files: whitespace, applicable JSON
and Python syntax, balanced Markdown fences, 85 relative links/heading anchors,
all 22 selected local results, fixture/artifact hashes and the preserved
historical evidence digests. `git diff --check` passed and the protocol worktree
remains clean. No shell examples or runtime source were edited; Rust, full core
tests and process suites were not needed for this local research extension.

Evidence SHA-256:

- Current `verify-local.py`:
  `50de3a0dc38c37d23ed2e70df92890e353a3b2a17020b4d7f27c88eb7455a88d`.
- `tip-refund-local-results.json`:
  `2f9dc201e41620ad33f63b89658f37bd42fb7c1caf7b4289e549d74b9b497525`.
- Unchanged `Probe.sol`:
  `4a5e5d9029005702c42ef8b4bb778e0ea814d909612de6cdf22ce8fa55d84756`.
- Reused EEZ artifact:
  `d3a0d29d34b7c1e67a7f7828aaa13e4aa6204fadb0f2a69e10e92b212398dd96`.

## Titan public API refund-source limit

On 2026-10-10 at 22:02 Europe/Madrid (20:02 UTC), the user asked whether we
could query Titan's API to resolve its `Option<Vec<String>>` documentation.
The [retained requests and responses](sponsorship/titan-refund-cardinality-probes.json)
establish a new finding: **both tested public mainnet and Hoodi endpoints reject
two refund hashes**. Each returns HTTP 200 with this JSON-RPC error:

```json
{
  "code": -32600,
  "message": "bundle 'refund_tx_hashes' count cannot exceed 1"
}
```

This supersedes the earlier unresolved-cardinality finding in this audit and
the historical source manifests. An optional vector in the displayed Rust
type does not remove the deployed validator's one-element limit. The proposal
now treats ordinary multi-source `eth_sendBundle` collection at these Titan
endpoints as rejected, rather than merely undocumented. It does not rule out
a provider extension or a separately qualified private/alternative API.

Endpoints came from Titan's [official general documentation](https://docs.titanbuilder.xyz/):
`https://rpc.titanbuilder.xyz` and `https://rpc-hoodi.titanbuilder.xyz`.
For each, sent one `eth_callBundle` request and five `eth_sendBundle` validation
controls using curl with 5-second connection and 15-second total deadlines.
Every bundle contained the invalid raw transaction byte `0x00`, targeted the
already-past block `0x1`, and had no replacement identifier. Thus no submitted
transaction could execute or spend funds. These were actual malformed
submission requests, not live signed bundles and not a builder simulation.
No wallets, private inputs, signatures or operator configuration were used.

| Control, on both endpoints | Mainnet response | Hoodi response |
| --- | --- | --- |
| `eth_callBundle` with explicit object parameters | `-32601 Method not found` | Same |
| No refund-source field | `-32602 rlp offset overflow` | Bundle hash ACK |
| `refundTxHashes` with one 32-byte hash | `-32602 rlp offset overflow` | Different bundle hash ACK |
| `refundTxHashes` with two distinct 32-byte hashes | `-32600`, count cannot exceed 1 | Same exact error |
| `refundTxHashes: 42` | `-32700`, nested invalid-params message expecting a sequence | Same |
| `refund_tx_hashes: 42` | Same RLP error as the baseline | Same bundle hash as the baseline |

The controls show that camel-case `refundTxHashes` is recognized and type-
checked, and that its length check can run before transaction decoding. A
malformed snake-case field had no observable effect, consistent with an ignored
unknown field. It is not an alternative way to request several sources. The
Hoodi ACK for an invalid transaction and past target independently demonstrates
why an ACK alone cannot establish execution, source eligibility or payout.
The one-hash response also does not prove a refund was paid from that source.

The first direct urllib request for the documentation index returned HTTP 403.
The earlier retained public index identified the
[bundle-tracing page](https://docs.titanbuilder.xyz/bundle-tracing.md), which
curl retrieved successfully. That page documents `titan_getBundleStats` at
`https://stats.titanbuilder.xyz`, with traces available approximately five
minutes after submission. This is an after-submission status query, not a
pre-submission payout simulator. No stats query or delayed polling was needed
to establish the explicit cardinality rejection.

The new record contains thirteen entries: one public documentation read and
twelve RPC responses. Checked all response codes, the two identical limit
errors, scalar type rejection, differing one-source Hoodi hash, matching
snake-case/baseline Hoodi hash, and the non-execution guards on every submission.
All checks passed. This is live API-validation evidence only: no inclusion,
native-blob transport, sponsorship, refund amount or real payment was tested.
The preceding 22-case local test and historical evidence files remain unchanged;
no runtime, protocol, process suite or production deployment was changed or run.

Raw response bodies remain local-only at `/tmp/rollupx-titan-query-ais4ohig/`.
The complete RPC requests/responses are retained in the linked JSON. Its SHA-256
is `013bcdc1fe9596d1d9dc3f6c6c4476a608de5eb4231addb242a5d33654cb3237`.
The retrieved tracing page's digest is
`8587db08a903fbb5476c122c7a9c79ecbd048874915f0fe7b6ec4ada6c487c72`.

Final documentation checks passed: all 21 research files' whitespace and
applicable JSON/Python syntax; balanced Markdown fences and 88 relative
links/heading anchors; five current/historical evidence digests;
`git diff --check`; and a clean protocol worktree. No shell examples were added.

## Contract settlement of L1 user transactions

The subsequent user question proposed one native post plus one contract
transaction settling every user. Rechecked the repository HEAD, remotes,
protocol gitlink, recursive submodules and clean protocol worktree; identity
remains the revisions recorded above, with unrelated changes preserved.
This follow-up is source/document research, with no implementation or live
operation. The new proposal section distinguishes compatible signed operations
from unchanged signed L1 EOA transactions.

Read the pinned core proxy/call-hash and execution lifecycle sections, execution
entry delivery units, static entry routing, caveats and signer profile before
following their implementation. `CrossChainProxy._fallback` forwards its actual
`msg.sender`; `EEZ.executeCrossChainCall` hashes that source address and permits
entry only through an authorized proxy. The Composer's production bundle at
`composer.rs` around line 5220 appends exact inbound raw transactions after the
native post; outbound user transactions are L2-signed and are not appended to
the L1 bundle. Posting simulation executes the exact ordered transaction bytes.

The core meta hook can issue contract-origin calls, and its existing
`test_MetaHook_ImmediateStaticLookupAndTransientReentrant` constructs the source
hash using the hook caller's address. It does not establish authority to act
as arbitrary EOAs. Sender binding in the core is a poster-public-input feature;
the current signer rejects that optional profile, and it would not itself grant
users' account authority. These implementations/tests were inspected, not run.

Reviewed primary Solidity call semantics and EIP-1559, ERC-2771, ERC-4337 and
EIP-7702, linked in the proposal. A normal contract call changes the immediate
caller; signature recovery cannot independently debit an undelegated EOA or
execute its raw transaction as a nested top-level transaction. ERC-4337 supplies
the desired shared outer-transaction shape for compatible accounts and signed
operations. Delegated accounts and application-specific orders offer other
conditional paths; permissions and fee reimbursement must already exist or be
explicitly authorized. A single user signature per operation can be possible
without establishing unchanged-EOA compatibility.

The fee distinction is material: a settlement transaction has one outer gas
payer, so merely embedding users' ordinary fee fields does not collect them.
An operator-funded tip refund still returns the operator's own funds. With a
valid operation-fee mechanism, direct treasury reimbursement could instead
remove the dependency on builder refunds. This remains a design alternative;
no contract, account delegation, fee authority or signer profile was changed.
No tests or deployments were necessary for these prose-only findings, and no
new Claude invocation or builder API request occurred.

## One-transaction refunds and a shared post

The user supplied a screenshot describing Titan/Beaver single-transaction
bundles with a configurable refund recipient and asked why that is invalid.
Re-read the official Titan API/general pages, Beaver API and Flashbots Protect
gas-refund page. The screenshot's fixed-refund mechanism is supported by the
reviewed Titan/Beaver documentation. The observed Titan limit applies to the
number of selected hashes in one bundle; it does not prove a one-bundle or
one-refund limit for an entire block. Protect's variable overpayment refunds
remain separate from a requested fixed percentage.

The proposal now explicitly retains a candidate with one strict
`[postBatch, user A]` bundle and additional `[user B]`, `[user C]` bundles, each
requesting its own user-funded refund to the same treasury. This avoids a
duplicated post transaction and changes no user's signed transaction bytes.
Whole replay groups, source dependencies and the same exact L1 target remain
required. Successful composition of these state-dependent bundles by a real
builder is not established by its refund-recipient field or the earlier
malformed-request probes.

The cost condition is explicit: if A alone pays the complete incurred candidate
cost plus margin, missing B/C revenue does not create that fixed-cost deficit.
This is compatible with the user's accepted conservative higher quotes, subject
to the same payout qualification and risk reserve as the original single-source
proposal. Lower shared quotes that require all refunds still need joint
inclusion/payment support or an explicitly budgeted subsidy. Thus the screenshot
is valid as a fee-collection mechanism; it does not alone establish the shared
post's minimum payment. No contract aggregation, extra user payment or account
authorization is required by this candidate. No valid bundle, payment, wallet
operation, runtime change or new test was performed in this follow-up.

## Dependent-bundle admission, ordering and pricing

The user requested deeper analysis of separate one-source refunds sharing one
post. Baseline and clean protocol pin remain those above. Runtime and protocol
source were not changed. Evidence is retained in
[`dependent-refunds/evidence.json`](dependent-refunds/evidence.json), with
artifact and executable hashes, scope, first setup failure and actual selection.
The evidence was captured at `2026-10-10T20:54:16Z`.

### Builder admission and overlap findings

Re-read the official Titan bundle API and tracing docs, Beaver API, Quasar API
and tracing, BuilderNet API, Bombora API and EIP-1559. Titan's
[tracing documentation](https://docs.titanbuilder.xyz/bundle-tracing) explicitly
classifies required reverts and nonpositive builder payment during top-of-block
simulation. This supports an admission concern for child bundles that require
an earlier post/user. It does not document whether every sorting algorithm
retries those failures. A method ACK, even with a bundle hash, does not resolve
this. No new public `eth_sendBundle` probe was made in this follow-up.

Fetched public rbuilder files at exact revision
`6037fa728b13bf1806e16fff2586414216f6b8fa`. The retained
[source manifest](dependent-refunds/builder-sources.json) provides retrievable
raw URLs and SHA-256 hashes; local snapshots are under
`/tmp/rollupx-dependent-refunds-20261010/`, and the previously fetched
`order_commit.rs` is under `/tmp/rollupx-refund-aggregation-20261010/`.
The initial sandbox fetch could not resolve the public host; a read-only
network escalation retrieved the sources. No repository content or private
data was transmitted. Relevant static paths:

| Path at the recorded rbuilder revision | Finding |
| --- | --- |
| `live_builder/simulation/sim_worker.rs`, parent provider and `OrderSimResult` match | Each simulation starts from the target parent with any explicitly discovered predecessor orders. Only successful simulations are sent downstream. |
| `building/sim.rs`, `get_order_nonce_state`, `process_simulation_task_result`, `simulate_order_using_fork` | Dependencies are inferred from account nonces. Only successful results changing exactly one account nonce enter the predecessor lookup (`nonces_after.len() == 1`). A carrier containing a distinct poster and user is not such a predecessor. Successful predecessors are committed before the child, then the simulation is rolled back. Arbitrary contract-state dependencies are not discovered here. |
| `live_builder/simulation/simulation_job.rs`, `process_new_simulations` | Successful, uncancelled simulations enter builder intake and the nonce dependency tree; this is upstream of order sorting. |
| `building/builders/ordering_builder.rs`, `fill_orders` | The retry branch handles `LowerInsertedValue`, adjusting an already simulated order's priority; it is not a retry mechanism for missing failed child simulations. |
| `building/block_orders/prioritized_order_store.rs` | Block assembly tracks nonce readiness; it cannot make an order appear if initial simulation never produced it. |
| `building/order_commit.rs`, `commit_bundle_no_rollback` | Explicitly optional transaction failures can be dropped, selected executed transaction rewards enter refund accounting, and an empty resulting bundle fails. Eligible simple-recipient payouts can be combined at block end; other payouts may execute immediately. |

This source review did not build or execute rbuilder and does not identify any
provider's deployed revision. It is not Titan source. The nonce detail rules
out treating an added operator nonce transaction as a verified general fix
in this snapshot, while leaving provider-specific dependency support open.

The overlapping-prefix calculation assumes `[post,A]`, `[post,A,B]` and
`[post,A,B,C]`, with only earlier prefixes droppable and one mandatory selected
refund source per bundle. Evaluating all six visit orders produces three, two
or one refunds. C-first consumes every user through C's bundle, so A/B cannot
later execute their mandatory selected transactions. For the recorded
`0.003 ETH` per-user reward, 90% percentage and `0.0001 ETH` per-payout cost,
builder payment before its bid is `0.0063 ETH` with C alone versus `0.0009 ETH`
with three refunds. This is an abstract accounting counterexample, not a live
algorithm result or inclusion-probability estimate.

### Pinned protocol experiment

Read the pinned core execution-entry, lifecycle, same-block, root consistency
and caveat contracts before inspecting the matching Solidity paths. Also
reviewed settlement policy, RPC ownership and the queue-liveness proposal's
implemented status. The new research fixture
[`Dependency.t.sol`](dependent-refunds/Dependency.t.sol) imports the pinned
core's `Base` fixture and executes real **nonempty** queued entries through an
authorized proxy, with distinct pranked user addresses and a mock proof system.

All six intended tests passed:

| Scenario | Asserted result |
| --- | --- |
| B before the post | Exact `ExecutionNotInCurrentBlock(rollupId)` error; root remains unchanged |
| Post, then B whose root depends on A | Exact `ExecutionNotFound`; queue cursor remains zero; A followed by B succeeds and reaches the final root/cursor |
| Post and A only | First root reached, queue length two and cursor one; posting/first consumption do not complete the batch |
| A lands but B is deferred to the next block | B fails with the exact same-block error; root stays at A |
| Another post replaces the queue after A | B fails with the exact missing-entry error and the old queue is unavailable |
| A later entry already matches the current root | B can skip A; the cursor advances past both and A then fails |

Raw test outcomes are retained in
[`protocol-results.json`](dependent-refunds/protocol-results.json).
Foundry `1.7.1`, Solidity `0.8.34+commit.80d5c536`, optimizer runs 1, `via_ir`,
and Osaka were used in an isolated temporary project. Verified all **52**
compiler metadata source hashes against the actual imported files. The compiled
fixture artifact hash is in the evidence record. Generated artifacts stayed
in `/tmp`; the protocol checkout remains clean.

The first compile failed before test execution because the temporary project
omitted the protocol's OpenZeppelin remappings. The retained
[first failure](dependent-refunds/initial-compile-failure.txt) shows this setup
error. Copying all pinned remappings corrected it without changing dependency
versions or test behavior. A separate source-hash check initially exceeded the
OS argument-size limit; supplying source bytes through stdin completed it.
These are corrected harness/check setup errors, not a retry-only test pass.

Reproduce from the repository root after initializing the recorded clean pin:

```bash
research_dir=$(mktemp -d /tmp/rollupx-dependent-refund-check.XXXXXX)
python3 - "$research_dir" <<'PY'
import json
from pathlib import Path
import sys

root = Path.cwd()
protocol = root / "eez-core-protocol"
work = Path(sys.argv[1])
(work / "test").mkdir()
fixture = root / "docs/audits/2026-10-10-cross-chain-fee-research/dependent-refunds/Dependency.t.sol"
(work / "test/Dependency.t.sol").write_bytes(fixture.read_bytes())
remappings = ["protocol/=" + str(protocol) + "/"]
for line in (protocol / "remappings.txt").read_text().splitlines():
    if line.strip():
        prefix, relative = line.split("=", 1)
        remappings.append(prefix + "=" + str(protocol / relative) + "/")
config = (
    '[profile.default]\nsrc="src"\ntest="test"\nsolc="0.8.34"\n'
    'optimizer=true\noptimizer_runs=1\nvia_ir=true\nevm_version="osaka"\n'
    + "allow_paths=" + json.dumps([str(protocol)]) + "\n"
    + "remappings=" + json.dumps(remappings) + "\n"
)
(work / "foundry.toml").write_text(config)
PY
forge test --root "$research_dir" --offline \
  --no-match-path 'script/*' \
  --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental' \
  --match-contract DependentRefundProtocolTest --json > "$research_dir/results.json"
```

The explicit core-test exclusions are retained. This selects only the six
research tests; the full core suite was not run. Pranked calls do not establish
raw-transaction boundaries, native blob transport, fee/refund accounting,
cryptographic proof correctness, Composer/signing behavior, independent L2
replay or real builder admission. The earlier native-blob sponsorship experiments
remain separate evidence and were not rerun.

### Pricing and integration decision

[`economics.json`](dependent-refunds/economics.json) records integer arithmetic
and an exhaustive abstract inclusion model. Inputs are illustrative:
`0.0055 ETH` operator cost, `0.0005 ETH` target margin, 90% refund and
`0.0001 ETH` per-payout cost, using `floor(p*T)-H`.

- Ten equal `0.0006 ETH` net contributions meet the margin only on full
  inclusion. A alone leaves `-0.0049 ETH` profit.
- A full-cost `0.006 ETH` net contribution from A meets the margin in all ten
  modeled prefixes and all 512 subsets containing A for a separately assumed
  independent-child workload. The latter are not claimed to be all reachable
  states of one pinned-protocol root chain.
- At full inclusion, ten separate payout charges require approximately
  `0.00777778 ETH` gross tips, versus `0.00677778 ETH` for one pooled payout.
  Coalescing must be qualified before using the lower overhead; individual
  simulation may still require each requested refund to cover its own cost.

The safe first integration remains one strict multi-user bundle with a selected
real user whose quoted tip covers the full candidate, including profit and risk
reserve. This preserves the original signed user transactions and their ordinary
fee fields. It uses the broad documented one-source refund interface without
requiring the builder to discover sibling dependencies. Separate refunds can
improve revenue recovery only after ordered admission and actual payouts are
qualified. Repeated prefixes must never bypass the full-cost payer's refund.

The source tree's current identical-body fan-out and single submitted attempt
observer do not implement this design. A later implementation must preserve
whole replay groups, one exact proof/parent/target, per-user canonical ownership,
sibling replacement identities, actual payment attribution and rollback/reorg
behavior. Added refund transactions may affect later application observations;
their placement and recipient behavior belong in final simulation qualification.

### Remaining provider acceptance checks

No funded test network was available and no funded public transaction was sent.
These are prepared acceptance criteria, not executed tests or a request to
change the user's payment constraints:

1. Establish a successful native-blob strict `[post,A,B]` control with one selected
   user refund to a distinct treasury. Verify canonical source consumption and
   the actual net payout; API acceptance is insufficient.
2. Submit `[post,A]` and `[B]` for the same exact target, with both arrival orders
   and a B whose root genuinely requires A. Capture simulation status, final
   order and refund evidence. Repeat with multiple independent users. A passing
   isolated transaction transfer does not cover this state-dependent workload.
3. Withhold or invalidate a child, and verify the allowed partial outcome and
   full-cost first refund. Check that the unconsumed user is retained for normal
   reconciliation, not reported complete by the post or first bundle ACK.
4. Compare overlapping prefixes and record selected refund sources, dropped
   transactions, and actual payout totals. Do not infer multiple payments merely
   because every user transaction appears in the final block.
5. Test distinct sibling replacement/cancellation identities and stale target
   handling. Simulate queue replacement/reorg reconciliation in an owned local
   environment; do not induce public-chain disruptions.
6. Establish payout timing/coalescing, deductions, effective gas rewards, and
   positive competitive net builder value. Preserve receipts/traces, bundle
   identifiers and canonical block identities. Price delayed or discretionary
   redistribution separately from the requested fixed refund.

Runtime implementation, full Composer/signer/E2E suites, production deployment,
live rbuilder execution and real provider payout qualification remain unrun.
Documentation validation passed: 100 local Markdown links/anchors resolved,
balanced fences and clean whitespace in both owning documents, syntax checking
of the new reproduction shell example, all new JSON records parsed, retained
artifact hashes matched, the research Solidity fixture passed `forge fmt --check`,
and `git diff --check` passed. No additional runtime test rerun was needed after
the documentation-only additions.

## Sponsorship plus refund coverage recheck — 2026-10-11

The user asked whether the two features can be combined at every major builder.
Rechecked public primary APIs for Titan, Quasar, BuilderNet, Beaverbuild,
Eureka, Bombora, BTCS Builder+ and Ultra Sound, together with Titan's linked
sponsorship explanation and the July 8, 2026 BuilderNet community proposal.
The checkout remains at `fa916b13254a36423cad130080dde367bb18a7d6`, with protocol
gitlink `855fe0602484750861b4c31502a1a91355d33ed6`; unrelated operator changes
were preserved. No production code or protocol source was changed.

The [dated support record](sponsorship/combined-support-2026-10-11.json) captures
the source URLs and distinctions:

| Provider | Published automatic funding | Requested fixed refund | Combined workload evidence |
| --- | --- | --- | --- |
| Titan | Gas and transaction value, conditional on a funding shortfall and builder recovery | Documented | Candidate; actual native-blob funding and selected refund together unverified |
| Quasar | Gas, conditional on a funding shortfall and builder recovery; transaction value not promised by that wording | Documented | Candidate; actual combination unverified |
| BuilderNet | Deployment not established; community proposal does not establish acceptance | Documented | Unverified |
| Beaverbuild, Ultra Sound | Not established in reviewed documentation | Documented | Unverified |
| Eureka, Bombora | Not established in reviewed documentation | Pooled selected-source refunds documented | Unverified |
| BTCS Builder+ | Not established in reviewed documentation | Refund fields documented; source limits remain unqualified | Unverified |

Absence of a sponsorship statement does not prove provider rejection. Equally,
two individually documented features do not establish the accounting and
eligibility of a combined sponsored-post refund. No new market snapshot was
taken; the earlier Titan/Quasar 73.59% sum remains a dated candidate-coverage
figure, not verified combined support or inclusion probability.

Eureka's page could not be retrieved by the browser tool. A read-only HTTPS
fallback fetched the Markdown successfully, with SHA-256
`e697b5f0b6ae997945241994e4fbbbe3a51913a02bde9b39572174ba46c10565`.
Its source URL, status and size are retained in the record; the raw snapshot is
local-only at `/tmp/rollupx-sponsor-refund-20261011/eureka-eth-sendbundle.md`.
The browser's transient failure is not evidence of a provider outage or feature
removal. No valid bundle, credentials, user data or repository content was sent
to any provider.

The proposal now distinguishes (1) builder sponsorship plus a refund from a
real user, (2) builder sponsorship plus a refund from the post's own priority
payment, and (3) operator funding plus a real-user refund as the broad fallback.
The second construction needs explicit sponsored-source eligibility and a
gross/net refund rule; subtracting funding from the selected post's refundable
value can eliminate its payout. It can be economically valid under the earlier
local gross-tip assumptions, so an operator-funded self-refund limitation must
not be generalized to this externally funded candidate.

Actual native-blob funding, payout deductions, a separate treasury, residual
poster balances across repeated batches, positive builder economics and exact
canonical consumption remain qualification requirements. Sponsorship triggered
by insufficient funds is not reimbursement for a poster already funded at the
parent. Provider-specific funding prefixes need explicit final simulation and
admission integration; identical-body fan-out does not make them interchangeable.

No new local EVM, Composer, signer or live builder test was run: the changes
record documentation and accounting distinctions already covered by the earlier
local experiment. That experiment manually signed the builder's transfers and
does not qualify deployed combined support. Validation for this follow-up is
documentation links/whitespace and structured-record checks.

## Hoodi builder coverage — 2026-10-11

The [Hoodi coverage record](hoodi-builder-coverage-2026-10-11.json) separates
published submission endpoints, responding API parsers, observed block delivery
and the still-unverified fee-recovery workload. Its SHA-256 is
`cbe2a13a942dec1c0ae2beab3f74f814e3f2866d681e1245968a9abe48147b08`.
This follow-up used public documentation and unauthenticated reads only, plus
three invalid `eth_sendBundle` requests with `params: []`. No signed transaction,
valid bundle, deployment, funding or service operation was performed.

| Builder | Published Hoodi submission route | Evidence and limit |
| --- | --- | --- |
| [Titan](https://docs.titanbuilder.xyz/general.md) | `https://rpc-hoodi.titanbuilder.xyz` | HTTP 200 with an invalid-parameter parser error. This proves a responding API, not inclusion or sponsorship/refund execution |
| [Ultra Sound](https://github.com/ultrasoundmoney/builder-docs#hoodi) | `https://builder-rpc-eu-hoodi.ultrasound.money`, `https://builder-rpc-us-hoodi.ultrasound.money` | Both returned HTTP 200 and JSON-RPC `-32602` for the empty-parameter probe; published Hoodi keys also match sampled relay deliveries |
| [BTCS Builder+](https://www.btcs.com/builder-docs/) | No public Hoodi-specific endpoint found | Published builder keys match recent Hoodi delivery records; its documented general endpoint is not thereby qualified for Hoodi |
| [Quasar](https://docs.quasar.win/getting-started.md) | No public Hoodi endpoint found | Reviewed current index and endpoint documentation |
| [BuilderNet](https://buildernet.org/docs/send-orderflow) | No public Hoodi endpoint found | Reviewed published orderflow endpoint list; a browser read succeeded despite a separate direct-fetch network failure |
| [Beaverbuild](https://beaverbuild.org/) | No public Hoodi endpoint found | Reviewed homepage and API documentation |
| [Eureka](https://docs.eurekalabs.xyz/readme.md) | No public Hoodi endpoint found | Reviewed current index, overview, orderflow API and integration guide |
| [Bombora](https://bombora.build/) | No public Hoodi endpoint found | Reviewed homepage and bundle API documentation |

Not finding a public endpoint does not prove that a provider has no private
test environment. The [Flashbots bundle endpoint list](https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint)
documents mainnet and Sepolia. Its responding Hoodi **MEV-Boost relay** is a
different interface and cannot be substituted as a documented searcher bundle
endpoint. The same distinction applies to Titan's relay and builder.

Directly retrieved [seven-day](https://relayscan-hoodi.flashbots.net/overview?t=7d)
and [24-hour](https://relayscan-hoodi.flashbots.net/overview?t=24h) Relayscan
snapshots both report an update at **2026-10-11 15:24:24 UTC**, slot `4126572`.
The browser search cache returned older snapshots; those were not used for
these figures. The global builder tables contain **8,540** and **1,031** observed
delivered MEV-Boost blocks respectively, not all Hoodi blocks. Attribution in
these tables uses unauthenticated `extra_data` labels:

- `builder.ultrasound.money`: **1,338 / 8,540 (15.67%)** over seven days,
  absent from the 24-hour builder table. API reachability therefore does not
  establish a timely inclusion path for a trial.
- `Builder+ www.btcs.com/builder`: **27 / 8,540 (0.32%)** over seven days and
  **6 / 1,031 (0.58%)** over 24 hours.
- `hoodi two shoes`: **650 / 1,031 (63.05%)** over 24 hours. Its operator was
  not established; do not attribute it to Titan, Flashbots or BuilderNet.
- Titan, Quasar, BuilderNet, Beaverbuild, Eureka and Bombora are not identified
  by name in these tables. That is not proof of inactivity under other keys or
  labels, and the shares cannot be transferred to mainnet or refund coverage.

For stronger attribution, queried the latest 100 delivery records from each of
the Ultra Sound, Titan, Aestus and Flashbots Hoodi relays. Deduplication by block
hash leaves **362** records. Published keys match **24 Ultra Sound** records and
**3 BTCS** records; none match Titan's published key list. These samples cover
different intervals and are not a market-share estimate. The retained examples
are Ultra Sound block `3781841` and BTCS block `3796396`. Independent canonical
hash checks failed: PublicNode and ethPandaOps both returned HTTP 403 for both
historical block reads. Delivery and key attribution remain relay-reported;
canonical hashes and relay signatures were not independently verified.

The public chain-ID read returned `560048`. Raw document/statistics/relay
responses are local-only under `/tmp/rollupx-hoodi-coverage-20261011`; the checked-in
record retains source URLs, timestamps, digests, extracted statistics, parser
responses, attribution examples and failed canonical reads. The source commit
and protocol gitlink remain the baseline above; existing workspace edits and
untracked protocol content were preserved.

**Qualification consequence:** Hoodi currently provides two discovered public
builder candidates from the eight-provider comparison, with uncertain immediate
inclusion availability. It does not establish majority-builder fee recovery.
Titan is a documented sponsorship test candidate; Ultra Sound provides a second
fixed-refund API candidate. Public Hoodi routes for the pooled-refund candidates
Eureka and Bombora remain missing. Establish actual inclusion availability before
spending a prepared trial's test funds, then qualify native blobs and actual
refunds separately. No funded trial or runtime test ran in this follow-up.

## Nested transaction cost estimation — 2026-10-11

The follow-up asks how to estimate a complete user transaction when its
cross-chain calls nest. The resulting draft design is in
[Estimate nested source groups from complete execution](../../proposals/cross-chain-fee-recovery.md#estimate-nested-source-groups-from-complete-execution).
Research is static at fork `fa916b13254a36423cad130080dde367bb18a7d6` and
initialized protocol `855fe0602484750861b4c31502a1a91355d33ed6`. Existing
operator configuration, untracked records and the submodule's untracked lock
were preserved. No production code, protocol pin or deployed service changed.

Read the pinned core, execution-entry, static-call, caveat and native-blob
specifications before tracing these production paths:

| Evidence | Finding |
| --- | --- |
| `EXECUTION_ENTRY_SPEC.md`, nested examples and no-splitting rule | A reentrant callback stays inside its host transaction; a source can own multiple sequential roots. User-funded L1 callbacks and poster-funded L1 immediate work have different payers. |
| `STATIC_ENTRY.md`, builder/prover procedure | Every attempted lookup candidate and its callbacks consumes work. A top-level L1-to-L2 static read need not create an L2 delivery/root transition. |
| `docs/blobs/BLOB_FORMAT_SPEC.md`, context stack and physical encoding | Source transactions own nested calls, returns and revert brackets; actual encoded data occupies whole blobs of 126,976 useful bytes each. |
| [`compose_crosschain` and `l1_target_gas`](../../../crates/eez-composer/src/composer.rs), [`SessionInspector`](../../../crates/eez-evm-inspector/src/inspector.rs) | Cross-chain interception returns synthetic remaining gas to the caller. Separate L1-target action meters include nested/reverted L1 work but do not establish native receipt charges. |
| [`CompanionEstimate` and `companion_estimate`](../../../crates/eez-composer/src/composer/gas.rs) | Bounded admission projection adds intercepted source work, measured L1 callback work and protocol reserve, capped at the signed limit. Unsupported shapes keep the declared-limit reserve. |
| [`PostingRuntime::finalize`, `simulate`, `run_bundle`](../../../crates/eez-composer/src/composer/posting.rs) | Exact ordered signed L1 execution returns receipt gas separately from pre-refund/floor sizing gas. Final-limit execution and a larger-limit fallback address forwarding headroom; all intended effects remain required. |
| [`FeeQuote::estimate`, `reserve`, `ReceiptCost`](../../../crates/eez-l1/src/posting_fees.rs) | Expected charges and maximum balance reservation are separate; receipt gas and actual physical-blob prices establish observed spend. |
| [`CostReport::amounts`](../../../crates/eez-composer/src/composer/costs.rs) | Poster and companion receipt costs are separate. Companion fees must not all be counted as operator expense. |
| [Gas tracing runbook](../../runbooks/composer-gas-tracing.md) | Existing diagnostics distinguish admission projections from ordered native preflight transaction results. Traces were not enabled and no service was restarted. |

Primary Ethereum references were rechecked: [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559),
[EIP-150](https://eips.ethereum.org/EIPS/eip-150),
[EIP-4844](https://eips.ethereum.org/EIPS/eip-4844) and
[EIP-7623](https://eips.ethereum.org/EIPS/eip-7623). Use the target chain's active
rules; old EIP blob-count parameters are not a current-network capacity claim.

The design preserves one ordinary user transaction/signature. Estimate the
whole group, allocate real operator costs within its actual batch, and only
then translate its service contribution into the selected route's ordinary
tip fields. Signed-gas reserves, inclusive call-frame totals and remote gas
cannot substitute for collectable source receipt gas. An estimated successful
execution does not cover all possible inclusion failures or unpaid preparation.

Validation for this follow-up is documentation/source inspection only, including
local-link, section-anchor and whitespace checks. No new gas measurements,
Solidity/Rust/process tests, proof timings or live builder payments were
executed. Existing nested-admission audits are historical calibration evidence
at their recorded revisions; they do not validate a price model at this pin.
Per-group shadow accounting, calibrated pre-signing estimates and customer
quote/enforcement logic remain proposed work.

## L2-to-L1 collection options — 2026-10-14

The user chose a low-traffic L1-to-L2 model using a fixed percentage refund
from one sufficiently paying source, then requested the L2-to-L1 options.
The follow-up proposes direct ordinary L2 priority-fee collection with a
batch coverage check, optional transaction-specific quoting, and a separate
future protocol-fee design. See
[Low-traffic L2-to-L1 options](../../proposals/cross-chain-fee-recovery.md#low-traffic-l2-to-l1-options--2026-10-14).

Rechecked `git status --short`, remote/HEAD, protocol gitlink and recursive
submodule status. Source remains `fa916b13254a36423cad130080dde367bb18a7d6`;
the initialized protocol remains `855fe0602484750861b4c31502a1a91355d33ed6`.
Existing unrelated edits and the protocol's untracked lock were preserved.
No operator environment, deployed beneficiary or wallet balance was inspected.

| Source inspected | Finding and limit |
| --- | --- |
| [`EezEvm::transact_raw`](../../../crates/eez-evm/src/evm.rs), [`ordinary_transactions_still_pay_fees_and_outbound_eth_stays_at_the_system_address`](../../../crates/eez-evm/src/tests.rs) | Ordinary transactions use Ethereum execution; the native zero-price exception does not make signed outbound user transactions free. The test was read, not run. |
| [`ComposerSettings::fee_recipient`](../../../crates/eez-composer/src/composer/config.rs), [Live wiring](../../../crates/eez-node/src/lib.rs), [Composer Sync wiring](../../../crates/eez-composer/src/composer.rs), [public example](../../../.env.chiado.example) | An operator can select the L2 beneficiary. Bare-process absence defaults to zero, while the Compose example requires an explicit value. This is source/configuration capability, not verification of the deployed value. |
| [Execution entry specification](../../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md), simple and nested L2-to-L1 examples | Outbound L1 application work executes in the post/consumption transaction; L2 source gas alone does not price that remote work. |
| [Signer profile](../../../crates/eez-proof-signer/SPEC.md), [L2 chain-spec construction](../../../crates/eez-evm/src/profile.rs) | Current base-fee parameters are part of the validated execution profile. Redirecting burns or adding native fees changes execution/replay semantics. |
| [Ingress](../../../crates/eez-node/src/ingress.rs) | Ordinary estimation is forwarded. Existing affordability/classification is not a complete service-fee quote or batch-profit gate. |
| [Settlement ownership](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission) | Speculative source inclusion is not sufficient financial settlement evidence. Canonical partial consumption and retained source groups remain authoritative. |

Rechecked [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559) for ordinary tip
collection and burned base fees, and [OP Stack fees](https://docs.optimism.io/op-stack/transactions/fees)
for the distinct future option of protocol-native data/operator fees. OP
Stack's implementation and fee-vault assumptions are not imported into this
fork; arbitrary remote L1 execution needs its own metering model.

Documentation links/anchors and whitespace were checked; the illustrative
0.003 ETH / 100,000 gas = 30 gwei calculation was checked with integer units.
No runtime code or configuration changed. No unit/process suites, settlement
transactions, treasury withdrawals or builder-payment tests ran. Deployed fee
recipient, L2-to-L1 realizable asset value, actual collection totals and the
profitability of the proposed policy remain unverified.

## Standard wallet RPCs versus a custom quote — 2026-10-14

The user questioned whether `eez_quoteCrossChainFee` is necessary. Corrected
the proposal's interface wording: the custom method is optional; current-rule
transaction-specific tips need a transaction-aware client pricing path, which
can live in an SDK/backend. Standard wallet fee recommendations can instead
support a network-wide policy with the existing proposed batch gate.

Read the primary Ethereum execution-API
[fee definitions](https://github.com/ethereum/execution-apis/blob/main/src/eth/fee_market.yaml)
and [execution definitions](https://github.com/ethereum/execution-apis/blob/main/src/eth/execute.yaml).
The price/tip methods lack transaction arguments; gas estimation receives the
transaction but returns a gas quantity. Rechecked the fork's
[ingress forwarding](../../../crates/eez-node/src/ingress.rs). No RPC or wallet
behavior was changed or exercised.

[Arbitrum's estimation documentation](https://docs.arbitrum.io/arbitrum-essentials/how-to-estimate-gas)
and [fee accounting](https://docs.arbitrum.io/how-arbitrum-works/deep-dives/gas-and-fees)
establish a standard-RPC interface with a real parent-chain charge collected
by the execution rules. They do not establish compatibility with this fork or
pricing of arbitrary nested L1 work. A similar interface here needs the
separate protocol-fee design, including treasury credit, deterministic replay,
signed fee bounds, refunds and upgrade compatibility. Increasing a suggested
gas limit without implementing collection does not recover money.

Source/protocol identity remains as recorded in the preceding follow-up.
Validation covers documentation links/anchors and whitespace only. No runtime
tests, MetaMask/Rabby integration tests or protocol implementation ran.

## Native L2 settlement gas — 2026-10-14

The user requested the native-fee alternative in more detail, including nested
calls and the required changes. Added an exploratory design to
[Native L2 settlement fees through standard wallet RPCs](../../proposals/cross-chain-fee-recovery.md#native-l2-settlement-fees-through-standard-wallet-rpcs).
It retains one ordinary user signature and makes gas estimation reflect an
actual execution charge. It is not an accepted profile or implemented feature.

Rechecked the working tree, remote/HEAD, protocol gitlink and recursive
submodule status. Source remains `fa916b13254a36423cad130080dde367bb18a7d6`;
the initialized protocol is `855fe0602484750861b4c31502a1a91355d33ed6`.
Preserved unrelated configuration, records and the protocol's untracked lock.

The investigation read the pinned core/execution/static/caveat/blob contracts,
the signer and control-RPC contracts, and the current fee/execution paths:

| Evidence | Finding and design consequence |
| --- | --- |
| [`EezEvm::transact_raw`](../../../crates/eez-evm/src/evm.rs) | Ordinary fees still delegate to Ethereum execution. A native settlement debit and treasury credit require execution changes, not only a larger RPC estimate. |
| [`EezBlockExecutor`, `BURNED_AT_BLOCK_START`](../../../crates/eez-evm/src/block.rs) | EEZL2 and SYSTEM_ADDRESS balances are burned at block start. Use a separate spendable treasury. Keep fee effects within each user's checkpoint, rather than a final block-wide credit. |
| [`L2 profile`](../../../crates/eez-protocol/src/l2.rs), [`signer execution profile`](../../../crates/eez-proof-signer/SPEC.md#supported-prefix-commitment-block-profile) | The 30,000,000 block limit and Osaka transaction cap constrain virtual settlement gas too unless the profile changes. Current receipts, headers and native budgets share cumulative gas. |
| [`Signer guarantees`](../../../crates/eez-proof-signer/SPEC.md#11-what-the-signature-does-not-establish), [`Stateless replay`](../../../crates/eez-proof-signer/SPEC.md#63-stateless-replay-guarantees) | Independent L2 replay does not certify arbitrary L1 gas consumption. A bounded tariff can derive from validated semantics; close arbitrary L1 metering needs additional independently validated evidence. |
| [`Native direct V0`](../../proposals/native-system-transactions.md), [`DA contract`](../../../crates/eez-proof-signer/SPEC.md#11-data-availability-and-sync-block-verification) | There is no current settlement-fee side table. A fee context needs a specified authenticated carrier and deterministic historical reconstruction; a Composer-only field is insufficient. |
| [`RPC construction`](../../../crates/eez-node-common/src/rpc.rs), [`front forwarding`](../../../crates/eez-node/src/ingress.rs) | Fee-aware `eth_estimateGas` would be new backend behavior under standard method names. Actual wallet compatibility remains untested. |

Rechecked primary [Arbitrum fee documentation](https://docs.arbitrum.io/how-arbitrum-works/deep-dives/gas-and-fees)
and the [OP execution-engine fee specification](https://specs.optimism.io/protocol/exec-engine.html).
They establish examples of real execution-time parent-chain charges and treasury
allocation, not a ready-made arbitrary nested L1 execution meter for this fork.

The candidate converts a deterministic service tariff `F` into
`S = ceil(F / effectiveGasPrice)` settlement gas, reserved before local execution.
The receipt charges local gas plus `S`; the full settlement component credits
the treasury. Ordinary local burn/tip accounting remains separate. Local-only
transactions have zero settlement charge. The design requires signed spending
bounds, ordinary local refunds/floors before adding `S`, and replayable price
and resource inputs. It prices whole source groups, includes supported reverted
work, avoids inclusive-parent/child double counting, and preserves partial
canonical prefixes. Exact future post receipts are reconciliation evidence,
not a circular input to the block hash/proof they depend on.

Open decisions are the fee-context carrier, work-class tariff and authority,
gas-budget behavior at low L2 prices, and any independent L1 metering extension.
The bounded prototype counts settlement units against current limits and can
reject transactions that do not fit. Production use requires actual price/work
calibration, acceptance evidence, treasury liquidity and coordinated activation.

Validation is documentation/source inspection, local links/anchors and
whitespace checks, plus integer arithmetic checks of the proposed fee split,
rounding and signed-cap bounds. The illustrative 0.003 ETH tariff is 3,000,000
gas at 1 gwei and 300,000,000 gas at 0.01 gwei. These are arithmetic examples,
not measured fees. No execution implementation, unit/process/proof tests,
wallet integration, live settlement, deployment or operator configuration
change was performed. No claim of production profitability or compatibility
is established by this record.

## V1 client-pricing clarification — 2026-10-14

The user clarified that the preferred first version keeps current execution
rules and calculates the transaction-specific tip in the dApp/SDK. The preceding
assistant response had misidentified "option 3" as a network-wide recommendation.
Corrected the proposal's recommendation while preserving the investigated
alternatives and draft status. No implementation or deployment was requested.

The SDK needs a complete source-group cost estimate; standard wallet fee RPCs
alone cannot supply arbitrary nested L1 work. A Composer-backed application
service can provide it without introducing a custom wallet-facing JSON-RPC
method. The SDK sets ordinary fee fields before one wallet signature; Composer
checks the signed transaction's funding at admission and again before dispatch.
Existing effective-tip, native receipt-gas, whole-group, treasury and bounded
failure-exposure requirements still apply.

Rechecked the working tree and existing proposal sections. This follow-up edits
only the two research records. Validation covers whitespace and the clarification's local
Markdown structure; no runtime or wallet tests ran.

## Combined first-iteration assessment — 2026-10-14

The user asked whether the selected approaches form a solid, sufficiently
simple first iteration for both directions. Consolidated the scope in
[First iteration: shared pricing with two collection routes](../../proposals/cross-chain-fee-recovery.md#first-iteration-shared-pricing-with-two-collection-routes).
The design shares cost estimation, dApp/SDK fee setting, admission/dispatch
funding checks and reconciliation. L1 collection uses a funded post with a
fixed refund from one sufficiently paying external user; L2 collection uses
ordinary treasury-owned priority fees. Native settlement gas, sponsorship
combinations, pooled refunds and customer rebates are deferred improvements.

Clarified older pooled-mode guidance so it does not read as a mandatory v1
integration. Preserved the dated coverage study and all its evidence limits.
The proposal remains draft, with no authorization inferred for implementation,
funding or deployment. The broad-builder-coverage objective remains unproven.

Reopened primary [Titan](https://docs.titanbuilder.xyz/api/eth_sendbundle),
[Quasar](https://docs.quasar.win/api/eth_sendbundle) and
[BuilderNet](https://buildernet.org/docs/api) API documentation. It continues to
document the fixed-refund integration candidates. BuilderNet distinguishes
refund-eligible bundle submission from ineligible standalone raw submission;
the wallet/Composer transport is therefore part of the collection validation.
These reads establish API descriptions, not payment for our workload. No fresh
builder-share sample, endpoint probe, broadcast or provider contact occurred.

Checked working-tree status and preserved unrelated edits. This follow-up changes
only research prose. Documentation whitespace, local links and section anchors
were checked. No runtime, wallet, proof, settlement or deployment tests ran.

## Claude v1 review and adjudication — 2026-10-14

The user explicitly requested a fresh Claude review of the current proposal.
Rechecked root status, remote/HEAD, protocol gitlink, recursive submodule status
and protocol edits. Source remains `fa916b13254a36423cad130080dde367bb18a7d6`,
with initialized protocol `855fe0602484750861b4c31502a1a91355d33ed6` and only its
pre-existing untracked network lock. Unrelated operator changes were preserved.

This review used Claude Code 2.1.296 in an isolated temporary directory. Only
the proposal text was supplied; tools, repository access, customizations, MCP,
skills and session persistence were disabled. The first sandboxed attempt
returned `EAI_AGAIN` without a review. Network-enabled execution was permitted
for the prepared text-only request after that failure. The earlier rejected
broader review remains historical evidence above; it was not silently retried
or described as successful. The renewed explicit request and this invocation's
narrow payload/permissions are distinct.

The first successful response reported `claude-opus-5-5`, one model turn and
241.606 seconds. Its full text and SHA-256 identities are retained in
[Claude's v1 review](claude-v1-review.md). The initial verdict was a viable
bounded L2-to-L1 design, with L1-to-L2 enforcement awaiting concrete wallet
transport and real refund qualification; it did not require a different fee
mechanism. No source inspection or operational evidence is attributed to Claude.

Checked the recommendations against the pinned
[request ownership contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission),
[signer guarantees](../../../crates/eez-proof-signer/SPEC.md#11-what-the-signature-does-not-establish),
[front](../../../crates/eez-node/src/ingress.rs),
[poster/companion cost attribution](../../../crates/eez-composer/src/composer/costs.rs),
[fee estimate/reservation split](../../../crates/eez-l1/src/posting_fees.rs),
[block-start burn](../../../crates/eez-evm/src/block.rs) and
[native-receipt indexer path](../../../monitoring/indexer/src/l2_fees.rs).
Read primary [MetaMask transaction documentation](https://docs.metamask.io/metamask-connect/evm/guides/send-transactions/),
[EIP-1193](https://eips.ethereum.org/EIPS/eip-1193) and
[EIP-3085](https://eips.ethereum.org/EIPS/eip-3085) for the wallet/API boundary.
Those documents do not establish actual EEZ routing in MetaMask or Rabby.

| Review point | Adjudication and proposal change |
| --- | --- |
| P1-1: wallet transport | Accepted the missing concrete route. Specify wallet source-chain submission through the EEZ front, pending nonce/status consistency, and per-wallet qualification. Rejected public-mempool appearance as a terminal ownership event: only canonical evidence or an existing explicit disposition authorizes release. |
| P1-2: stale accepted work | Accepted the need to distinguish quote validity, signed caps, reserved preparation/service exposure and indefinite transaction validity. Clarified the normal one-signature path versus optional ordinary replacement. Rejected time-based deletion/eviction and any suggestion that a reserve bypasses signed fee eligibility. |
| P2-3: route-dependent preparation cost | Accepted. Price bounded preparation work using measured landing behavior for the qualified route set. A block-share percentage or `1 / p` model is not a landing guarantee; financial budgets do not replace existing retry counters. |
| P2-4: selected-payer coupling | Accepted proxy/storage-state interaction and ordered receipt revalidation. Qualification begins with one whole source group; shared L1 batches remain conditional v1 scope under the user's accepted higher-fee policy. Rejected cross-transaction access warmth and the conclusion that all shared batches must wait for pooled refunds. |
| P2-5: state-dependent failed groups | Accepted the need for bounded exposure and explicit canonical L2 fee recognition. Rejected eviction or source-failure charges based only on later-state simulation, timeouts or infrastructure uncertainty; preserve validated request-local attribution, dependencies and ownership. |
| P3-6: rollout/scope | Accepted. Separate parallel L2 enforcement preparation and L1 qualification, define combined activation, and label any earlier one-direction pilot. Move sponsorship acceptance to later expansion and label native settlement-gas recommendations as future-only. |
| P3-7: refund attribution | Accepted. Add payment identities, bounded many-to-one allocations, provider plus canonical transfer evidence, reorg reversal and zero revenue credit for unidentified deposits. Per-route recipient addresses are optional, not a new mandatory wallet/account scheme. |

The reviewer also suggested proof-independent placeholder-verifier simulation.
The existing two-stage estimator already permits provisional measurements and
calibrated allowances. No placeholder verifier was accepted as authoritative
preflight or substituted for final signed/proved native validation. The native
receipt indexer issue was rechecked by reading the current code; no fix or
runtime reproduction is claimed in this document-only review.

The proposal changes were sent back to Claude with these disagreements stated
explicitly. A second pass (55.346 seconds) agreed with the rejected remedies
and found four remaining local policy gaps: reserve-funded shortfalls versus
the strict revenue gate, ordinary/same-nonce front behavior, shared failure-cost
allocation, and accepted-unposted versus already-required work.

Resolved them as follows:

- **Strict v1 revenue gate.** A reserve is capital, not customer revenue.
  New demand must cover its allocated required contribution. Price changes can
  leave accepted work blocked; no firm-price or bounded-completion guarantee is
  implied by an indicative quote. Reserves fund bounded preparation, realized
  failures and separately owed backlog, not a known new-demand shortfall.
- **Explicit front/SDK scope.** Read `intercept_send_raw`, `gate_and_hold` and
  [`HeldPool::push_contiguous`](../../../crates/eez-composer/src/held_pool.rs).
  Ordinary/create pass-through, queued dual-fee-bump replacement and rejection
  of in-flight cross-chain replacement are existing behavior. The minimal SDK
  permits one unsettled application submission per sender/source chain and
  shows held/blocked state; it does not claim to manage other apps' nonces.
  Actual wallet routing/nonce behavior still needs integration tests.
- **Shared costs without blame.** Freeze service-cost weights, allocate observed
  shared costs exactly with deterministic rounding, and debit internal group/
  global work-risk budgets once. A later smaller candidate preserves all source
  dependencies and ownership. This is not a new customer fee or permission to
  charge an infrastructure failure to the protocol retry counter.
- **Owned versus owed.** An accepted unposted inbound is owned demand that can
  be funding-blocked. It is not by itself the canonical/policy obligation that
  requires posting existing history from the separate backlog budget.

A third targeted pass (28.933 seconds) maintained the verdict: a viable bounded
integration plan with no material redesign. It requested final clarifications
for mixed backlog/new-demand coverage, classification-error behavior, allocation
terminology and pre-dispatch group reservations. These were applied after that
response. The initial suggestion that fail-closed classification must always
reject was qualified against the current source: pricing-disabled/accounting-only
mode retains the existing held-admission path. Proposed enforced mode rejects
new unclassifiable work before accepting ownership and preserves already owned
requests. This future admission-policy change is explicitly labeled and is not
presented as current behavior. Rechecked transaction-scoped warmth against
[EIP-2929](https://eips.ethereum.org/EIPS/eip-2929).

All three verbatim responses and their reviewed proposal/prompt/response hashes
are in [the review artifact](claude-v1-review.md). The final proposal after the
last wording changes has SHA-256
`79906dbf5d4d5127478e129f68827ab381c424a8a5476ccf181eb43b85de5b51`.
The third pass reviewed the preceding hash recorded in its table; the final
wording changes were source-checked here, not sent for a fourth model approval.
Raw CLI JSON and prompts are local-only in
`/tmp/eez-fee-claude-review-20261014-4k7gq525`; the retained text/hashes above
remain available to another checkout without those temporary files.

Validation covers `git diff --check`, explicit whitespace/fence checks of all
three research documents, local links/section anchors, and 42 integer arithmetic
examples of exact shared-cost allocation with deterministic rounding. The
arithmetic checks exercise the written rule, not implemented ledger code.
No Rust/Solidity/process/proof tests, wallet runs, builder payment trials,
treasury operations or deployment were performed. Remaining launch evidence is
wallet transport/fee/nonce behavior, calibrated preparation and loss budgets,
verified treasury realization, real native-blob refund payments and measured
qualified-builder coverage. The proposal remains draft.

## Implementation-readiness check — 2026-10-14

The user assigned wallet routing to the dashboard and asked whether the design
is complete enough to implement. Treat dashboard routing as an integration
responsibility, not a claim that wallet tests have passed. Rechecked
`git status --short`, `git remote -v`, `git rev-parse HEAD`,
`git ls-tree HEAD eez-core-protocol` and `git submodule status --recursive`.
The source and protocol identities remain the revisions recorded above;
unrelated worktree/operator changes were preserved.

Reread the proposal's requirements, pricing, ownership, treasury and rollout
sections and the retained Claude responses. Checked the current
`CostReport::amounts`, `FeeQuote::estimate`/`reserve`, `gate_and_hold` and
`Selection::new` implementations. They provide payer-separated receipt costs,
expected-versus-upfront estimates, source affordability and source base-fee
eligibility. They do not implement the proposed service-revenue gate or durable
financial reservations.

Found stale wording in the quote/admission and rollout sections permitting
explicit subsidies or reserve-backed batch discounts despite the selected
strict v1 rule. Removed that ambiguity: no reserve-funded admission of
underpriced new work; separately funded preparation, failures, migration and
already-required settlement remain permitted. Future subsidy modes and builder
sponsorship are outside the selected v1. No execution or queue behavior changed.

Added an implementation-readiness section distinguishing the settled design
from contracts still to specify with their first implementation: versioned
quote/status/error shapes, durable reservation write/recovery rules, concrete
configuration and builder adapter profiles. Accounting and shadow quotes can
start before production calibration; enforcement still needs verified payment
routes, treasury realization, funded budgets and an explicit builder-coverage
target/window. This is a staged readiness assessment, not gap-free or launch
certification and not authorization to deploy.

The preceding Claude review's final proposal hash remains historical evidence.
This follow-up proposal has SHA-256
`656c72ef682752c21af1f73c447081e8c1cb5c589b4359066daf3a4fd2a5e71b`.
No additional Claude pass was run. Checks passed: `git diff --check`, explicit
whitespace/fence checks, 164 local links and 26 section anchors across the three
research documents, plus removal of the contradictory strict-funding wording.
Only the proposal and this audit were edited. No runtime, process, wallet,
builder-payment, treasury or deployment tests ran; these prose corrections do
not establish those capabilities.

## First L2 accounting implementation — 2026-10-14

The user authorized development on `feat/l2-fee-recovery`, prioritizing L2-to-L1
fees. Rechecked repository status, remotes, HEAD, protocol gitlink, recursive
submodule status and submodule edits. Baseline remains
`fa916b13254a36423cad130080dde367bb18a7d6` with protocol
`855fe0602484750861b4c31502a1a91355d33ed6`; the pre-existing untracked protocol
lock and unrelated operator changes were preserved. This increment is an
uncommitted working-tree change in the standalone monitoring workspace and the
research documentation. It does not change the node workspace or either lockfile.

Read the pinned execution model, delivery semantics and caveats, the native
transaction profile in the signer specification, the native EVM dispatch path,
and the indexer's receipt, enrichment, checkpoint and database contracts.
Confirmed the first prerequisite defect with an added regression against the
baseline collector:

```bash
cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml \
  --test l2_fees native_system_receipts_meter_gas_without_charging_fees -- --exact
```

The baseline selected one test and failed with
`L2 effective price below base fee` for a zero-priced native `0x76` receipt in a
block with base fee 10. The same named check passes after the implementation;
native gas remains in block totals while its burn and tip are zero. The complete
fee tests also cover ordinary reverted fees, native-only blocks, invalid/missing
types, nonzero native prices, zero/changing beneficiaries, complete attribution,
checkpoint upgrades, unavailable receipts, overflow, premature availability,
restart, canonical promotion and reorg handling.

The [collector](../../../monitoring/indexer/src/l2_fees.rs) now returns typed
block fee evidence and stores decimal ordinary/native gas totals plus
`tipsByBeneficiary`. These are gross payments to each header's actual beneficiary;
they do not prove treasury control, exclude operator transactions, or establish
L1 redeemability. Enrichment version 6 requeues old records and fee-checkpoint
version 1 rescans old ranges before exposing the new fields. Existing burn/tip
keys and canonical L1 history are preserved. Updated the
[indexer contract](../../../monitoring/indexer/README.md) with the new accounting,
upgrade/rollback behavior and focused validation commands. The proposal records
this as part of stage 0 and gives L2 its own release criteria before L1 refunds.

Validation on the final source:

| Check | Result |
| --- | --- |
| `cargo +1.95.0 fmt --manifest-path monitoring/indexer/Cargo.toml --check` | Passed |
| `cargo +1.95.0 clippy --locked --manifest-path monitoring/indexer/Cargo.toml --all-targets -- -D warnings` | Passed |
| `cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml` | 32 default tests passed; two database tests explicitly ignored by this command and run separately below |
| Isolated PostgreSQL admission/recovery tests | Both passed, including v5-to-v6 requeueing, current-version cache retention, restart, writer exclusion, atomic rollback, reorg cleanup and dashboard SQL |
| `git diff --check` and edited documentation checks | Passed; local paths/anchors and shell-example syntax checked |

The initial RPC-test invocation hit the sandbox's loopback-listener restriction;
it failed before either RPC test exercised the collector. Re-running with local
network access passed. This environment restriction is separate from the
reproduced native-fee bug, and is not an intermittent application test failure.

Inspected existing Docker resources before starting a task-owned PostgreSQL
container, `eez-l2-fees-pg-20261014-9d62e7`, from cached `postgres:16`, image
`sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54`.
It used a random localhost port, a 256 MiB temporary data filesystem, one CPU,
512 MiB memory, and a database named `eez_indexer_test`. No deployment database
was used. The final invocation, after the checkpoint-availability refinement, was:

```bash
INDEXER_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:32770/eez_indexer_test \
  cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml \
  --test recovery --test admission -- --ignored --test-threads=1
```

Both tests passed with nonzero selection. The temporary container was stopped
and automatically removed after testing. Existing services were not restarted.
The database and RPC observations were synthetic test data, not live fee receipts.

Final SHA-256 identities:

| Artifact | SHA-256 |
| --- | --- |
| Proposal | `26f4de66ce537bc71bce013f60d3de9be41f6ca51e9b1b52441952bc9b38675a` |
| `monitoring/indexer/src/l2_fees.rs` | `77d7a7b32ac128c583ed3522998b472622c46caa6d556e2ecc5f6070b4a7aa98` |
| `monitoring/indexer/schema.sql` | `a6b95a1b65aece0d5509341eaf38671a2a7a7e0f375a233442f1db1d3956cfdc` |
| `l2_fees-2209d6ca64de13ba` test executable | `1d0f049a64c57fe69a5adf0324737db6c3f8108525c05937ced7bf3f91aca14b` |
| `recovery-ce96eec2734789c8` test executable | `46e8799eaceaa4da69bbb671bddfe0d7861a32d8b1527e86e9462b409084555d` |
| `admission-93414f39c0966c97` test executable | `7f17ed89c1c13596e2e670987471423c8b0f5994b36284f2dddd42b9a05ebf3c` |

Executables are local-only under `monitoring/indexer/target/debug/deps`; source,
test cases and these small evidence records are retained in the worktree.
No root-workspace builds, Solidity/node process suites or independent follower
replay ran: this increment changes only the independent read-only indexer and
its enrichment format. It does not establish wallet behavior, full nested-cost
pricing, treasury realization, builder payments or deployment of this revision.
Shadow quotes, qualified customer-revenue accounting, failed-attempt costs,
durable spending reservations and funding enforcement remain future increments.

## Composer shadow fee observations — 2026-10-14

Continued on `feat/l2-fee-recovery`, with superproject HEAD
`fa916b13254a36423cad130080dde367bb18a7d6` and initialized protocol gitlink/checkout
`855fe0602484750861b4c31502a1a91355d33ed6`. The protocol has no tracked edits;
the pre-existing untracked `.eez-network-e2e.lock` is retained. Existing operator
configuration, explorer work and the earlier indexer increment are preserved.
No manifests, dependency lockfiles, protocol pin or operational settings change.
This is an additive feature, not a reproduced baseline bug fix.

The [observation contract](../../proposals/cross-chain-fee-recovery.md#second-development-increment-composer-shadow-fee-evidence)
defines SF1–SF4. The [implementation](../../../crates/eez-composer/src/composer/fees.rs)
uses cumulative receipts retained by `build_sync_block`, the complete outbound
source set from the existing settlement manifest, and the final ordered native
L1 preflight. It does not re-execute or sum simulated nested frames. Native L2
loads/callbacks consume gas at zero price; ordinary reverted receipts still pay
fees. Source fees use the effective priority price after fee-cap clipping and
the charged receipt gas, not declared gas or intercepted composition gas.

The L1 amount includes only the complete poster transaction and its physical
blobs, with signed-cap balance reservation separate. User-funded L1 companions
are not added to poster expense. Shared historical DA belongs to that whole-post
cost; historical L2 ordinary fees are not included in the terminal-block tip
observation. Gross speculative tips are denominated separately, with beneficiary
and source sender retained. Asset realization, proving, operations, risk and
profit remain unpriced. Required wallet tip and qualified L1 recovery are null.

Existing `preflight`, `source_preflight` and `repriced` event kinds carry bounded
additive details. The current collector preserves them without a database schema
change. Absent old fields remain unknown. Observation errors are diagnostic and
cannot reject, fund, evict, release or complete a request. There is no admission,
execution, proof, queue, retry or settlement-policy change.

Validation:

| Check | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-composer --lib` | 301 passed; one explicitly ignored manual backlog profiling test, not semantic coverage |
| New fee unit cases | Five passed: native/ordinary receipt separation including callbacks/reverts, clipped tips, malformed/missing/duplicate source evidence, L1 identity/target and blob overflow, companion exclusion and unknown pricing |
| Native block construction regressions | Existing prefix-equivalence and native-cap cases now assert retained cumulative receipt gas against independently rebuilt prefixes/native execution |
| `cargo +1.95.0 fmt --all --check` | Passed |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| Independent indexer fmt and all-target Clippy | Passed |
| Indexer `shadow_fees_extend_existing_preflight_records_without_a_schema_upgrade` | One selected test passed; existing parser retains the new source/aggregate detail unchanged |
| `forge build --root contracts` | Passed using Foundry 1.7.1 and Solidity 0.8.34; existing missing-default-library-path, unused-call-return and test lint warnings remain |
| Node process comparisons | Five selected tests passed on their first execution; zero failed/ignored, 111 filtered out. Three compare new shadow fields against canonical receipts; two retain Ethereum/Gnosis L1 pricing regressions. |
| Documentation | Four edited documents: 193 local links/anchors and 10 shell examples checked; `git diff --check` passed |

The initial focused compile stopped in the new test fixture because its
`SignableTransaction` trait import was missing; after correcting that import,
the complete Composer unit suite passed. An initial indexer formatting check
also stopped before tests; formatted output and the selected compatibility test
subsequently passed. Neither was a baseline runtime failure or a retry-only
test pass.

Rust checks exported `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, prepended its `bin`
directory to `PATH`, and used `CARGO_BUILD_JOBS=2`. Integration contracts were
built before source process testing. The initial sandbox process listing exposed
no nodes; this was limited namespace visibility, not evidence of an empty host.
The final host-visible check found two existing Composer/signer pairs started
on October 10 and October 14 at 10:51 UTC, before this run. They were preserved.
Test nodes began at 18:10 UTC and all task-owned children exited. The native
harness binds ephemeral localhost ports; local socket access was approved for
these isolated tests. No Docker or existing-service lifecycle operation ran.

```bash
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH=/usr/lib/llvm-22/bin:$PATH
export CARGO_BUILD_JOBS=2
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_LOG_DIR="$PWD/artifacts/native-e2e/l2-fee-shadow/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-e2e/l2-fee-shadow/datadirs"
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  settlement_policy_cost:: \
  cross_chain::outbound_nested_contract_to_contract_to_proxy_preserves_source_attribution \
  chained_interstate::mixed_direction_state_chain_in_one_slot \
  chained_interstate::identical_outbound_calls_in_separate_transactions_chain_state \
  --test-threads=1
```

The [process assertion](../../../crates/eez-node-e2e/tests/settlement_policy_cost.rs)
compares final post execution/blob amounts against the mined receipt and each
source's L2 gas/tip against separately fetched canonical receipts, including
native system receipts via raw JSON. Existing nested attribution, two-source
ordering, mixed-direction semantics and full-settlement assertions remain.
These are source-based local tests using disposable node processes; they are not
live builder payment or wallet-routing tests.

The five process cases passed in 337.84 seconds, including the harness's first
standalone signer build. The outer test-target build took 15m32s. These are
build/test durations, not throughput or latency benchmarks. The new L2 fee
receipt comparisons use the Ethereum fixture; the Gnosis case checks the existing
L1 pricing/receipt path. There was no diagnostic runtime retry and no skipped
semantic assertion. The initial invocation retained its logs and datadirs.

Small observations retained from the three receipt-checked candidates (all have
one physical blob):

| Fixture | L2 sources / L1 companions | L1 post gas | L2 ordinary / native gas | L1 post cost, L1 native base units | Gross L2 tips, L2 native base units |
| --- | --- | --- | --- | --- | --- |
| Two ordered outbound sources | 2 / 0 | 518116 | 259850 / 285101 | 711868639356788 | 259850000000000 |
| Mixed directions | 1 / 1 | 709514 | 85575 / 708231 | 913465598049062 | 85575000000000 |
| Nested outbound contracts | 1 / 0 | 475436 | 116225 / 224157 | 678627077588908 | 116225000000000 |

The process assertions established equality to separately fetched receipts;
this table is not a production tariff or a cross-currency profitability claim.
The corresponding post hashes, in table order, are
`0xd7b46aa6ca7d9ace864a00e0b03e5a29cc2930185779762eab869b634eb95a72`,
`0x00cbe55daf0daa7691852c7a07ecd44336b6cbfba1b34484c36128e9fc6d1086`,
and `0x9900e77bf8b3dbb03143d24bb33e27debb6afd6a15def8dcc0fa6eb99360e59e`.

Source/executable SHA-256 identities for this increment:

| Artifact | SHA-256 |
| --- | --- |
| `crates/eez-composer/src/composer/fees.rs` | `c28d35f359ffa0c3a982f44975d700861fa2fa362db7da8f412c311301f44667` |
| `crates/eez-composer/src/local/build.rs` | `a8f86f84243cf89e88e0bfce3e54dea8b468cdbc3446640f644ade4b97164726` |
| `crates/eez-composer/src/composer.rs` | `6ebf722aa809cfddf0c0aa6951e0736f95aead34f7cd24226517a4907a10893c` |
| `crates/eez-composer/src/composer/posting.rs` | `744d8b3511c6c297d4ae06ea58232057556b80ed3c4f1c881bd1453ce8b32d94` |
| `crates/eez-node-e2e/tests/settlement_policy_cost.rs` | `81c11f7ab8b42b3955281575f33fb36e1b4cb8d1cead1c75fd2312b90159a19e` |
| `target/debug/deps/eez_composer-a33b06fe92f0bc30` | `1eeabaab389e5ec88374901e4c2bc9dd5fbbe29d3afa4fbaa7c04d586532bbee` |
| `monitoring/indexer/target/debug/deps/admission-93414f39c0966c97` | `d9109230ada55ead079d0f81a12bd03b5f7fa907f06188121356ce47fb4953f9` |
| Current proposal | `970d4af98e3ee2325ad64ccf8a4faf9aae649229127418771c8f42fd096c5870` |
| `crates/eez-node-e2e/tests/chained_interstate.rs` | `7b3062cb1ae0184246e2dcafdaed8d9748e6ac062907746534f025340c74762a` |
| `crates/eez-node-e2e/tests/cross_chain.rs` | `724f207fb4ad383fbb4c705611ed0c6c31ad5c0181af3094da18027b8cd3f80c` |
| `target/debug/deps/node_e2e-974f176d4f809f55` | `999c05bed5cdce6ee2b1f8786dd76924955eff1dc65244251afaf35b4170f79e` |
| `target/debug/eez-composer` | `dcdcb0d7d2629f2bebbac2e79d13a1ccd3c917c8003237c96f177de0013a6067` |
| `target/debug/eez-follower` (built, not replayed in this selection) | `ba0b7a9d629b8d9e67e6555b135c3b2bfb820431e7a8033599efbce3eaf9a458` |
| `target/debug/eez-proof-signer` | `bf6aa8d7e04bbc615c3bdd4d1ac37863677216f7daa763317f19bb693d02d4fc` |
| `artifacts/native-e2e/l2-fee-shadow/process-tests.log` | `54aa76a938cf2d3c4c0404a1c8391090a481e4d91823bf4d0e9bbf3afef3ebc2` |
| `logs/eez-node-node-3542100-2.log` (two-source case) | `70b76b11214d51cbe8ca5e155dfb02897c61e90a138285786c0ef674d1d659a7` |
| `logs/eez-node-node-3542100-5.log` (mixed case) | `1601066a510698a815e7ee1a917e74e446533bb31e15aecfbb543518f913e0f5` |
| `logs/eez-node-node-3542100-8.log` (nested case) | `b7aaed745e2bfc0b26494040a194f89ce7349a53f505d740de7bad5da74eab72` |

Compiled artifacts and process logs/datadirs are local-only under ignored
`target/` and `artifacts/native-e2e/l2-fee-shadow/`. Retained source/tests and this
record are available to another checkout; ignored diagnostics are not.
No deployment or existing-service lifecycle operation is part of this work.
Full workspace tests, full Solidity/protocol suites, the wider queue/reorg E2E
matrix, independent follower replay and live treasury/builder verification are
not claimed. Unsigned-intent estimation, a wallet signing quote, tariffs,
realizable customer income, durable reservations and enforced funding remain
unimplemented; this observation slice does not establish production solvency.

## Configurable L2 shadow pricing — 2026-10-14

Implements the user's approved next slice: pricing policy, calculator and shadow
comparisons for prepared L2-to-L1 candidates. The
[SP1–SP5 contract](../../proposals/cross-chain-fee-recovery.md#third-development-increment-shadow-pricing-policy)
and [operator settings](../../configurable-settlement-policy.md#shadow-l2-fee-pricing)
define the scope. Pre-signing estimation, wallet integration and enforced funding
remain subsequent work.

Source remains `eduadiez/rollupX` at
`fa916b13254a36423cad130080dde367bb18a7d6`, branch `feat/l2-fee-recovery`, with the
previous two increments and this increment as uncommitted changes. Protocol
gitlink and initialized checkout remain
`855fe0602484750861b4c31502a1a91355d33ed6`; its only local status is the preexisting
untracked `.eez-network-e2e.lock`. No manifest, lockfile, protocol or genesis
change was made. Unrelated operator/explorer/Compose edits were preserved,
including the existing Chiado target and beacon-peer changes. Only the new
shadow-setting forwarding lines were added to the already dirty Compose file.

### Implementation and review boundaries

- `composer/fees/pricing.rs` validates one bounded JSON policy and computes the
  complete post's service payment. Proving, operations and treasury costs are
  explicit per-post allowances. Risk/profit are independent rounded-up margins;
  conversion uses U512 intermediate products and checked U256 results. Required
  payment rounds up; projected recovery rounds down.
- Runtime network IDs/rollup and configured beneficiary must match. Each
  observation checks the exact target against policy expiry and the actual L2
  beneficiary. Known operator sources are excluded, including the poster and
  beneficiary; remaining sources are only assumed external. No configured rate
  or beneficiary assertion establishes treasury control or realized income.
- Native system gas and base burns remain outside revenue. Effective clipped
  tips use the final charged source gas. Single-source candidates without L1
  companions receive a diagnostic minimum tip and current-base minimum fee cap.
  Shared/mixed candidates receive aggregate comparisons without a standalone
  source quote. Pricing errors cannot change admission/dispatch/ownership.
- `ComposerSettings` carries the immutable optional policy to `PostingRuntime`.
  `fee_observation` version 2 adds pricing assumptions, their fingerprint and
  projected shortfall/surplus; qualified revenue remains null. Absent policy
  keeps unpriced accounting. Current indexers preserve both versions as JSON.
  Chiado/external Kurtosis Compose and Kurtosis `settlement_env` transport the
  setting; no production policy is enabled by the examples.

The review traced the complete native L1 preflight, source receipt/manifest
measurement, startup configuration and final observation caller. The pinned
core's same-block, nested-entry and partial-consumption contracts and the RPC
ownership rules remain authoritative. No new worker, network oracle, scheduler,
protocol input or financial state was introduced.

### Validation

| Check | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p eez-composer --lib` | 310 passed, zero failed; one existing manual backlog timing profile ignored. Nine new pricing/configuration regressions are included. |
| Root formatting and workspace/all-target Clippy with `-D warnings` | Passed. |
| Indexer formatting, all-target Clippy, selected admission compatibility test | Passed; one selected test preserves both v1 measurements and v2 pricing unchanged. |
| Chiado configuration tests | 17 passed; absent/file/shell/empty/invalid forwarding preserves runtime validation and signer separation. |
| `python3 scripts/check-settlement-policy-config.py` | Both Chiado and external Kurtosis profiles passed using disposable public bindings. |
| Full `scripts/ci/tests` suite | 307 passed, zero failed, five explicit opt-in probes skipped (312 discovered). No live deployment is inferred. |
| `forge build --root contracts` | Passed with Foundry 1.7.1; existing default library-path and test cast/division lint warnings remain. Sources unchanged, compilation cache current. |
| Documentation | 200 local links/anchors and 12 shell examples checked across the proposal, settlement policy, indexer README and this audit; `git diff --check` passed. |
| Native process comparisons | Initial candidate: four passed, one startup failure. Corrected candidate: all five passed, zero failed/ignored, 111 filtered out. |

The first focused 13 fee tests passed with a deprecated conversion warning.
Following that warning's suggested API caused a compile failure because the
pinned `ruint` does not expose `checked_from`. The implementation now uses its
available `checked_from_limbs_slice`, preserving checked narrowing without a
dependency change; the full Composer tests and workspace Clippy then passed.
This is a corrected development compile failure, not a runtime flake.

The first process run exposed a new startup-wiring error in this increment:
`Composer::new` supplied `cross_chain.entry_rollup_id` (the L1 composition entry,
ID 0) to the policy's L2 identity check. A valid policy for owned L2 rollup 1
therefore shut down with `EEZ_L2_FEE_SHADOW_POLICY: chain/rollup identity differs
from runtime`, and the harness timed out connecting to its stopped node. No
source transaction reached execution in that case. The correction checks the
owned L2 rollup map, keeping both chain checks and per-observation rollup checks.
The unit regression distinguishes owned L2 IDs from the L1 entry and an empty
owned set. This is a corrected implementation failure, not a passing retry of
unchanged code or a baseline deployed defect.

After that correction, the complete Composer unit suite again passed with 310
tests, zero failures and the one manual timing-profile exclusion. Root formatting
and workspace/all-target Clippy with `-D warnings` also passed on the corrected
sources. These results are separate from the initial candidate's passing checks.

The failing run selected five tests: four passed, one failed, zero ignored,
111 filtered out (335.70 seconds including a 2m03s first signer build). Its
outer build took 10m13s. Original evidence is retained under
`artifacts/native-e2e/l2-fee-pricing/`: `process-tests.log`,
`logs/eez-node-node-3847510-8.log`, and
`datadirs/eez-cross-chain-3847510-Q3nkgB`. The failing Composer executable SHA-256
was `8e9e496d9f03808b9b5e6ddbd9deb2aef44a36eae2eea8869c0bef54d45797d4`,
test executable `ab3e6ac22c6cf41740277b195cd40cadc9ca22344f66a92f7aededb69b4986dc`,
and process log `e751887d3743d6605ccfccad6a69b91ab5a43bc1564f9226af60408469b0d5df`.
Corrected runs use a separate directory and do not overwrite this evidence.

The corrected run repeated the same five selections and passed all five in
308.07 seconds, including the harness's 2m04s signer build; its outer runtime/test
build took 8m57s. These are validation timings, not performance comparisons.
The complete output is retained as
`artifacts/native-e2e/l2-fee-pricing-corrected/process-tests.log`.

The five Python exclusions are the real data-only Docker payload probe, live
Kurtosis relay resource probe, tiny-workspace archive relocation, and two real
Docker runtime-image probes. All other intended selections ran. Mocked/static
configuration and tooling results do not validate a deployment or live service.

Rust commands use Rust 1.95.0, `--locked`,
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, that directory's `bin` prepended to `PATH`,
and `CARGO_BUILD_JOBS=2`. Process tests clear stale executable overrides and use
fresh task-owned logs/datadirs. Host-visible inventory identified the two
existing Composer/signer pairs and indexer before tests; they were preserved.
The sandbox's narrower process namespace is not evidence of an empty host.

```bash
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH=/usr/lib/llvm-22/bin:$PATH
export CARGO_BUILD_JOBS=2
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_LOG_DIR="$PWD/artifacts/native-e2e/l2-fee-pricing-corrected/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-e2e/l2-fee-pricing-corrected/datadirs"
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  settlement_policy_cost:: \
  cross_chain::outbound_nested_contract_to_contract_to_proxy_preserves_source_attribution \
  chained_interstate::mixed_direction_state_chain_in_one_slot \
  chained_interstate::identical_outbound_calls_in_separate_transactions_chain_state \
  --test-threads=1
```

The nested case enables a synthetic non-parity policy with deliberately high
service allowances. Its oracle computes required payment and projected recovery
from independently fetched canonical L1/L2 receipts, verifies the policy reached
the runtime, and requires a shortfall despite successful complete settlement.
The mixed/two-source cases keep the policy absent and preserve the unpriced
receipt checks. The Ethereum/Gnosis cost cases retain existing L1 price checks.

### Canonical nested candidate sample

The corrected nested case passed both semantic/full-settlement assertions and
the independent receipt oracle. Its single priced candidate used policy
`0xd76ea6f173c9095569ad23e11cb226d74a951603f7c32f3f7993c4bf1de47a4c`,
post `0xdbdc48c683e5bedf22e9b24fc434ef5988009f29999b25b08334e8598b216bff`,
and source `0x1d14ab2f2d19e5690f41455a0b53357ea36612ebae754fef9c9d166f90976bb3`.
The canonical terminal L2 block was
`0x8076139b1b1e3289be5612aea8fd0be858b95ae68a81fc8148f543d1901e44ea`.

| Observation | Value | Unit |
| --- | --- | --- |
| L1 post execution | 678592799458084 | L1 base units |
| One physical blob | 131072 | L1 base units |
| Configured proving + operations + treasury | 6000000000000000 | L1 base units |
| Required payment including risk/profit | 7680381719527530 | L1 base units |
| Required payment after conversion | 11520572579291295 | L2 base units |
| Charged source gas | 116225 | L2 gas |
| Native system gas, earning no tips | 224157 | L2 gas |
| Eligible source tips | 116225000000000 | L2 base units |
| Projected recovery at the synthetic 2/3 rate | 77483333333333 | L1 base units |
| Reported shortfall | 7602898386194197 | L1 base units |
| Diagnostic minimum tip | 99123016385 | L2 base units per gas |

These are synthetic fixture results, not a production tariff or verified asset
conversion. Complete settlement despite the reported shortfall establishes the
observational behavior of this case; it does not establish funding enforcement.
The source observation retained `qualified_recovery_l1: null`.

### Corrected source and executable identity

The corrected process run uses the following SHA-256 identities. The follower
is built by the harness but independent follower replay is not selected.

| File | SHA-256 |
| --- | --- |
| `crates/eez-composer/src/composer/fees.rs` | `0d3f5ac7920f135993b5cd6565fa13166eafb3ce3c22dbf1e0025b910a19ddd9` |
| `crates/eez-composer/src/composer/fees/pricing.rs` | `b4d0b2e37bc99531962b58f8835e86ef58f6d267110e76d98162bd101b545f79` |
| `crates/eez-composer/src/composer/config.rs` | `3987cc5a9c2ca6e0668bbf4a10dd1f8a0e6197c819bf68f175145b78e928f02b` |
| `crates/eez-composer/src/composer.rs` | `a1d3d4fe1ba7e811d0dcc35ec19a3c1ff76eed205564b22c61b862bb1a8fb802` |
| `crates/eez-composer/src/composer/posting.rs` | `46bc7c8df2b621d2c8e8bb3be836c3981774d7f0bc463e24c57e01a711ce004c` |
| `crates/eez-node-e2e/tests/settlement_policy_cost.rs` | `1ce75de20a9a4ed42254d153f61041632e7d1d7aee77ab0c0b7bc90235c3b692` |
| `crates/eez-node-e2e/tests/cross_chain.rs` | `1d1d4d927feaf66c649613001883cbb40af608524feac240114c0b15343af59e` |
| `target/debug/deps/eez_composer-a33b06fe92f0bc30` | `0fabf8a4a5dd22f057905a1ad04dc6e5f85b55595937dffde68dc09288db7c59` |
| `target/debug/deps/node_e2e-974f176d4f809f55` | `1cb43cd9dbeac766001727dc6ec61e331a0da5b85593b0bdf727bdf8347d07f7` |
| `target/debug/eez-composer` | `f802a269ac57fd9be5c2179a60df5f9ff0497a8cdc99ba46d03ddc8ba0a5a3a8` |
| `target/debug/eez-proof-signer` | `bf6aa8d7e04bbc615c3bdd4d1ac37863677216f7daa763317f19bb693d02d4fc` |
| `target/debug/eez-follower` | `ba0b7a9d629b8d9e67e6555b135c3b2bfb820431e7a8033599efbce3eaf9a458` |

Corrected run files below are relative to
`artifacts/native-e2e/l2-fee-pricing-corrected/`:

| Evidence file | SHA-256 |
| --- | --- |
| `process-tests.log` | `274ad2512a0457493dd349aa01377f5a75245fd1054d1b20759c63600a8654bb` |
| `nested-pricing-sample.json` | `eb2f5485bb02b110bffc9d21d96f3f8d4e3f7a5e91e68e11aab5fd23c024532d` |
| `logs/eez-node-node-3897632-8.log` | `2efb5397b8b154a2b838dad09e9caf7e1e4b7a53d9947cf5297bf21d2407389d` |
| `composer-corrected.log` | `f4a17a26b8d3f10f53feb00e0ee36265bb61c6f3d0381e38319adc77d3d8438c` |
| `clippy-corrected.log` | `b9aacbcad1f0b5c082dc4ee935301146d51b8671a9908938b829fcfa352b7157` |

Final host-visible inventory contained the same two Composer/signer pairs and
indexer with their original PIDs/start times. No test-owned runtime, Anvil, Cargo
or Rust compiler process remained. No existing service was restarted or stopped.

Compiled artifacts, unit/lint logs, process logs and the original failing
datadir are local-only under ignored `target/` and
`artifacts/native-e2e/l2-fee-pricing{,-corrected}/`. The harness automatically
deletes passing-test datadirs; the corrected run's datadir root is empty.
Ignored diagnostics are not available in another checkout. The source/tests,
the small evidence record here and its digests are retained for reproduction.

No live treasury realization, economic calibration, wallet submission, builder
refunds, production deployment or independent follower replay is established.
Full protocol/Solidity suites, full workspace unit tests and the wider process
matrix are not run for this local pricing-only addition. No user transaction
was repriced by this code, and no reserve or spending authorization was created.

## Unsigned application quotes — 2026-10-14

Continuation authorized after configurable shadow pricing: implement pre-signing
L2-to-L1 application quotes and an SDK helper. This increment leaves enforcement,
L1 refund integration and treasury qualification for later work. The owning
[HTTP/application contract](../../configurable-settlement-policy.md#unsigned-l2-fee-quotes)
records the schema, limits, compatibility and rollback behavior; the
[proposal](../../proposals/cross-chain-fee-recovery.md#fourth-development-increment-unsigned-advisory-quotes)
records the scope and acceptance checks.

### Checkout and implementation

- Fork/branch: `eduadiez/rollupX`, `feat/l2-fee-recovery`, superproject HEAD
  `fa916b13254a36423cad130080dde367bb18a7d6`, with the preceding uncommitted
  fee increments and unrelated operator edits preserved.
- Protocol gitlink and initialized HEAD:
  `855fe0602484750861b4c31502a1a91355d33ed6`. Only the existing untracked
  `.eez-network-e2e.lock` is present in that submodule; no protocol edits.
- Composer adds a strict unsigned type-2 intent parser, isolated/pinned local
  execution contexts, native L2 replay, retained ordinary-history DA sizing,
  whole-post L1 gas projection and bounded fee iteration. Source gas is checked
  against signed receipt execution in unit/process tests. Maximum-width signature
  placeholders are used only for DA sizing, never as execution authorization.
- The existing L2 HTTP front serves `/eez/fees/l2-to-l1` under the optional shadow
  policy. A Reth blocking worker owns its sole permit through completion;
  request cancellation, timeout and shutdown signal cooperative cancellation.
  No prover, queue mutation, signature, transaction submission or reservation is
  performed by the quote path.
- `sdk/l2-fees.mjs` supplies ordinary fee fields, checks the exact intent,
  policy/context, expiry and an app-defined maximum total fee, then uses standard
  wallet chain/account/nonce/pending-base-fee reads before one
  `eth_sendTransaction`. No wallet permission request or second payment is added.

### Validation and first failures

The initial compiler checks caught a wrong blob-capacity constant, a missing
provider `Clone` bound and a test-envelope conversion mismatch. These were fixed
without manifest or lockfile changes. Clippy also caught a needless borrow in a
new test; the test now passes the recovered transaction by value.

The first unit run passed Composer's tests and the new worker/SDK logic. Five
existing HTTP tests could not bind sockets under the sandbox (`EPERM`), before
reaching their intended behavior. The subsequent host-permitted unit run passed
**317 Composer tests** (one existing manual timing test ignored) and **64 node
tests**, including the socket tests. The new worker test holds execution with a
channel and demonstrates that both cancellation and timeout keep the service
busy until that worker exits. Six focused quote unit cases cover malformed
inputs, rounding/overflow, signature-size bounds, cancellation/frame exhaustion,
and history sizing against the independent encoder; native replay additionally
checks GASPRICE-visible state, nonce/balance validation and signed gas equality.

The first process run obtained a quote but failed the test's no-state-change
oracle: `Value` initializes to **5**, while the test incorrectly expected zero.
The corrected oracle captures the initial value and compares against it. This
was a fixture assertion error, not evidence that quoting mutated chain state.
The first failure, logs and datadir remain at
`artifacts/native-e2e/l2-fee-quotes/`; the datadir is
`datadirs/eez-cross-chain-3984834-W6lhB2`. First executable SHA-256 values are
recorded in `first-process-executables.sha256`. No failure was discarded or
retried unchanged as a pass.

### Completed local checks

| Check | Result |
| --- | --- |
| Composer/node unit suites | 317 + 64 passed; one existing Composer manual timing test ignored |
| SDK tests, Node 26.7.0 | Five passed, including exact intent, expiry, fee ceiling, changed wallet state and one ordinary submission |
| SDK with the native process's actual response | Accepted the captured quote at its issuance time; this is offline response validation, not a wallet session |
| `fee_quotes::unsigned_nested_quote_matches_receipt_and_replays` | One selected and passed; 116 other tests filtered out |
| Existing bidirectional and mixed static/write process regressions | Two selected and passed; 115 other tests filtered out |
| Workspace all-target Clippy with `-D warnings` | Passed |
| Rust formatting and integration contract build | Passed |
| CI tooling unit suite | 312 discovered: 307 passed, five opt-in infrastructure probes skipped |
| Documentation checks | 229 local paths resolved, 11 shell fences passed `bash -n`, `git diff --check` passed |

CI now explicitly selects the `fee_quotes` module in the cross-chain suite.
The inventory checks confirm every declared node process module belongs to
exactly one execution suite. The five tooling exclusions are the real data-only
Docker image probe, real Kurtosis resource probe, real tiny-workspace archive
relocation, and two real Docker runtime image probes. These skipped lanes are
not coverage. SDK tests were run locally; this increment adds no Node runtime
or SDK test step to CI.

The native quote test covers local nested calls into an L1 proxy, retained
ordinary deployment history, wrong-chain and oversized requests, the absence
of quoting on the L1 front, and no quote-induced sender nonce or destination
state change. It signs one ordinary transaction using the returned fees,
checks the destination state and local completion counter, requires a successful
canonical L1 post receipt, and starts an independent follower that reproduces
the source block hash and charged gas. Normal settlement regression coverage is
`cross_chain::both_directions_zero_value_direct_proxy_success_single_call` and
`cross_chain::outbound_mixed_static_reads_and_write_settle_together`.

The portable [unsigned quote sample](unsigned-quote-sample.json) records two
fee iterations and the following synthetic fixture observations:

| Observation | Quote | Canonical receipt/check |
| --- | --- | --- |
| Ordinary source gas, L2 gas units | 116225 | 116225; independently replayed |
| Native system gas, L2 gas units | 224157 | Excluded from paying source gas |
| Whole L1 post gas, L1 gas units | 1036165 projected | 475436 used |
| Physical blobs | 1 | 1 |
| Required L2 priority payment, L2 base units | 12901274208230585 | Gross paid tip 12901274208245300 |
| Qualified L1 recovery | `null` | No realization or liquidity qualification |

Encoded DA is 5535 bytes including retained ordinary history. The L1 target
is block 7; the quoted L2 parent is block 97, while actual inclusion is block
100. The L2 base fee fell before inclusion, leaving the complete quoted tip
payable. This case establishes conservative L1 projection for this workload;
it does not establish an upper bound for arbitrary contracts or changing state.
The source block is
`0x492e853af61908effe88e74e3e6623066d2d72b7585fcafd682af8cc0adfa868`;
the successful L1 post is
`0x47a0a20153a3f9850d18b2ad15b8c6bfeca1d2aa329a3cea2d2bc50e9772bfe2`.

### Source, executable and evidence identity

The [source/sample SHA-256 manifest](unsigned-quote-sources.sha256) identifies
the final quote implementation, shared execution changes, SDK, process test and
CI inventory changes relative to the baseline and earlier increments above.
The three runtime identities are the same for the corrected quote process and
the two subsequent regressions:

| Executable | SHA-256 |
| --- | --- |
| `target/debug/eez-composer` | `fd206b0f51cefc82c35a14c4836630dc50853377033b49dde3e416263fd0b96a` |
| `target/debug/eez-follower` | `ba0b7a9d629b8d9e67e6555b135c3b2bfb820431e7a8033599efbce3eaf9a458` |
| `target/debug/eez-proof-signer` | `bf6aa8d7e04bbc615c3bdd4d1ac37863677216f7daa763317f19bb693d02d4fc` |
| Corrected quote `node_e2e-974f176d4f809f55` | `beff5d88f384b8df6026f72767bd48453197c179884d59419acc3f0ddaaace26` |
| Final regression `node_e2e-974f176d4f809f55` | `02c487caf6324b4e6876a101cf815265dc5a50b3f8463a6d9ef469488b39c5d4` |

The final test-harness rebuild followed the test-only Clippy cleanup. The
runtime executables did not change. Corrected quote and final identities are
also retained locally as `quote-process-executables.sha256` and
`final-executables.sha256`. Evidence paths in the following table are relative
to `artifacts/native-e2e/l2-fee-quotes/`:

| Evidence | SHA-256 |
| --- | --- |
| `process-corrected.log` | `f9d650ccfa4afbe9b2bb0216655042c7f78fc8c4b1141c81a6dd9c2fa888864d` |
| `regressions.log` | `e50b30ca512746be78853d616f033107ccd7a93bb3d301b79f61de554fd7462b` |
| `unit-corrected.log` | `caf3ce13b9cdcf4551d4fd28cb22dba8e84bb1f118d352e517d404e0186ca6f8` |
| `clippy-final.log` | `b48bf035cf4f4a3db221d47075ad4b4df7e1c6f36d7c77a61b8d0fc8f63f36ee` |
| `sdk-tests.log` | `3f652552b39e646443d17098350f97eb4fdbaffb3207aa1310527d34fe1454ff` |
| `sdk-native-sample.log` | `029470a8459b72d8eab7846e7055bf89391ef7f8519209ddbfe79ddad2116103` |
| `ci-tooling.log` | `359d0a6e3f6428799365c48904be065460da89cc1c5e7fffe828594ef895f10a` |

### Reproduction and limits

The source-based process commands use Rust 1.95.0 and Foundry 1.7.1, initialize
no replacement protocol pin, clear stale-prebuilt overrides, and use isolated
fixtures with public test keys and newly allocated ports/datadirs. Contract
artifacts were built with `forge build --root contracts`. Five pre-existing
operator processes (two Composer/signer pairs and an indexer) were inventoried
by PID/name/start time before tests; none was restarted, funded or modified.

```bash
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH=/usr/lib/llvm-22/bin:$PATH
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_LOG_DIR="$PWD/artifacts/native-e2e/l2-fee-quotes/corrected-logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-e2e/l2-fee-quotes/corrected-datadirs"
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- fee_quotes:: --test-threads=1
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::both_directions_zero_value_direct_proxy_success_single_call \
  cross_chain::outbound_mixed_static_reads_and_write_settle_together --test-threads=1
cargo +1.95.0 test --locked -p eez-composer -p eez-node --lib
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 fmt --all --check
node sdk/l2-fees.test.mjs
python3 -m unittest discover -s scripts/ci/tests -v
sha256sum -c docs/audits/2026-10-10-cross-chain-fee-research/unsigned-quote-sources.sha256
```

These are local correctness checks, not a deployment, performance comparison,
or a proof that conservative gas projections bound every contract. The initial
quote profile is explicit: one unsigned type-2 call, fixed gas limit, no access
or authorization lists, no creation/blob source, and bounded complete supported
groups/history. The application must re-quote changed intent or observed base
fees. State changes, wallet approval delays and fee edits can invalidate the
estimate; final native settlement execution remains required. No real
MetaMask/Rabby session, public builder, Gnosis quote process lane, Kurtosis
network or full release matrix was exercised by this increment. Alternating
cross-chain re-entry and fee-nonconvergence do not have dedicated new process
fixtures here; group lowering retains the existing protocol profile and work
bounds fail closed. Neither gross tips nor the quoted conversion constitute
qualified L1 revenue, liquid L1 reserves or a durable financial guarantee.
The full workspace unit suite, wider native process matrix and full
Solidity/protocol suites were not run for this application quote increment.

Diagnostics under ignored `artifacts/` are local-only and unavailable to another
checkout. Source and a small sanitized quote sample are the portable evidence;
full logs and failed databases are not committed. Existing protocol/core test
exclusions are not claimed as coverage. No Solidity, ABI, protocol pin, genesis,
dependency, deployment or indexer behavior changed in this increment.
The final host process inventory retained the original five operator PIDs and
start times; no test-owned runtime, Anvil, Cargo or compiler process remained.
