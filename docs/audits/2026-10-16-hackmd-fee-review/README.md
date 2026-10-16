# HackMD fee holds and quote/admission review

Status: read-only investigation completed. The pending L1 nonce limitation has
later implementation and live evidence; pricing feedback and diagnostic gaps
remain. No runtime/configuration changes or new transaction submissions were
performed for this review.

| Field | Value |
| --- | --- |
| Report | [Fee holds and quote/admission failures](https://hackmd.io/@Yu7RfKfWRP6MwaKqIFALjw/HJuLQvJhzx), run `20261016-100516`, approximately 08:05 UTC |
| Historical runtime | `ce6574788682c54bdd3f9742870f5dbdbd9a4a9c`; executable identities in the [original stress record](../2026-10-16-chiado-batching-stress/README.md) |
| Reviewed checkout | `eez-association/eez-rollupX`, `17fe00fa9420ad3b9ddec9e003a9a9a6e35f6e04`; clean before this audit |
| Current runtime | `be8477bc7ba48d3f287d33cd6c010089458885ec`; [image identities](../2026-10-16-chiado-batching-rerun/timing-executables.json) |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Environment | Chiado L1 10200 / L2 906969; preserved runtime logs, historical public L1 RPC and current local L2 RPC |
| Contract | [Fee quote and settlement policy](../../configurable-settlement-policy.md#compact-fee-quotes) |
| Evidence | [Sanitized observations and calculations](evidence.json) |

The initially selected root checkout was `fa916b13254a36423cad130080dde367bb18a7d6`
with protocol gitlink `855fe0602484750861b4c31502a1a91355d33ed6`. Its operator work
was preserved. Production analysis used the separate fee-feature checkout above,
not that root revision or upstream main.

## A: six accepted L2 sources

Historical logs identify repeated `tariff_deferred` / `l2_tip_underfunded`
decisions, with 26 or 27 deferrals per source. The sources were accepted between
08:06:13 and 08:06:19 UTC. At 08:06:20.761 the selected posting priority rose from
1 to 2 gwei. At 08:08:32.256 it returned to 1 gwei; all six sources became ready
within 22 milliseconds. Their original hashes settled in L2 block 744535 and
L1 block 23544842. The public L1 receipt confirms post gas 2,157,408 and execution
plus blob cost 2,288,480,015,101,856 wei. The final native preflight recorded
4,246,664,426,825,556 wei of eligible L2 priority payments and cost coverage.

L2 base fees stayed at 49 wei in the retained preflight observations. Thus the
49-wei difference between signed maximum fee and priority cap does not explain
this incident as an L2 base-fee spike. It supplies exactly the sampled source
base fee. The diagnostic event does not retain each rejected prefix's offered
and required amounts, so exact per-source shortfalls cannot be reconstructed
from these logs alone.

The fee-history input also explains the posting-price increase. Querying
`eth_feeHistory` for 20 blocks ending at 23544820 with percentile 90 reproduces
the logged raw estimate of 3,316,607,925 wei, clamped to 2 gwei. Only the last
three sampled blocks had p90 rewards above 1 gwei. All 16 high-tip source
transactions in those blocks match Composer `source_accepted` records. Each
block contains the poster at 1 gwei, these L1 sources, and zero-tip builder
payments. Their gas utilization was approximately 21%, 11% and 5%.

This is observed feedback from refund-funded L1 source tips into the posting
price estimator. Gross source tips recover service costs and are not a clean
measure of the competitive bid required for a post. The observation does not
establish that every future post will land at 1 gwei or that congestion can be
ignored. The estimator and its p90-of-p90 selection remain unchanged in the
current runtime. The later fresh `all` run also recorded similar fee deferrals;
passing that run does not establish stable quote-to-settlement latency.

## B: pending L1 nonce sequence

The report predates deployment of the pending-prefix support at 09:31 UTC.
Current L1 quotes can replay up to eight contiguous owned predecessors when the
caller explicitly requests the next nonce. Omitting `nonce` still selects the
pinned canonical nonce and can encounter the already-owned nonce rejection.
Submit nonce N and obtain acceptance before quoting explicit N+1; a predecessor
entering composition can require a transient retry. L2 future nonces and ordinary
mempool predecessors are outside this supported profile.

A new read-only grouping of the later stress run's canonical receipts confirms
that all 50 three-source counter jobs placed at least two sources in one L1
block; 43 placed all three together, while seven split 2+1. This additional check
is separate from the pinned semantic verifier. It is successful workload
evidence, not an atomic same-block inclusion guarantee.

## C: exact quoted fields rejected at signed admission

The report's `nestedCallRevertL2` quote expires at 08:06:50 UTC. With the current
30-second quote lifetime, its preparation was around 08:06:20, coinciding with
the posting-price increase. This makes changed settlement pricing a plausible
cause. It is not proven: retained logs lack a matching signed-admission record
with the required/offered amounts and pinned fee context. Matching wallet fields
rules out field alteration; it does not establish identical quote and admission
state. The transaction was rejected before acceptance, not reverted on-chain.

Quotes are advisory; expiration is not a price reservation. The current cost
floor can still reject an unchanged envelope after a sufficiently large price
increase. Do not mark this historical failure resolved solely because later
protocol tests passed. A controlled nested-revert regression needs both unchanged
context quote-to-admission acceptance and an explicit posting-price transition.

## Visibility and remaining work

The runner's correction is appropriate: a held transaction may be absent from
ordinary transaction/receipt lookups while Composer retains ownership. Use
`eez_getCrossChainTransaction` before classifying it as lost. Unknown submission
results require reconciliation of the original hash, not an automatic new
transaction. `predecessorHash` can refer to the queried source itself because
status applies the first blocked nonce to that nonce and its successors.

Current Composer code classifies a settlement tariff shortfall using
`SourceCondition::Fee`, whose error text says the source cap is below base fee.
That text is misleading here. Separate source base-fee failures from settlement
cost shortfalls, and include offered payment, required cost, sampled L1 priority,
source base fee and relevant block references in rejection/status diagnostics.
Keep normal quotes compact. Discovery should expose a useful build identity.

Prioritize correcting posting-price feedback while retaining the final cost
floor. Evaluate a calibrated posting-bid policy that does not interpret our
refund-funded gross source tips as external competition, and verify it with a
mixed-direction workload spanning multiple fee-history windows. Preserve the
20% quote target and ability to consume that margin; increasing every user quote
or dropping cost validation is not established as necessary by this report.

## Evidence and executed checks

- Downloaded the public report's `/download` response; its SHA-256 is in
  `evidence.json`. Web extraction failed, so the public HTTP download was used.
- Parsed preserved pre-upgrade JSON logs, including the nested admission events,
  and correlated all six source hashes with fee selection and readiness events.
- Queried public Chiado `eth_feeHistory`, `eth_getBlockByNumber` and the final
  `eth_getTransactionReceipt`; queried local L2 block/receipt evidence. Local L1
  did not retain the requested historical receipt, so public L1 was used.
- Recomputed the exact p90-of-p90 estimator and matched all 16 high-tip L1 sources
  to historical Composer acceptance. The initial parser looked at the outer
  event instead of `detail.source_hash`; the corrected check asserts 16 matches.
- Grouped the retained canonical receipts for the 50 newer multi-source jobs;
  asserted nonzero selection and checked source-block co-location explicitly.
- Traced current quote nonce handling, signed admission, tariff deferral,
  standard RPC forwarding and Composer ownership/status paths. No new process
  or runtime tests were run for this documentation-only investigation.

Full report/log/RPC extracts are local-only under
`artifacts/hackmd-fee-review-20261016.0xbu1a2m` in the operator's original checkout;
they are not available in another checkout and have no guaranteed retention.
The source log is the preserved `eez-node-chiado.before-upgrade.log` from the
2026-10-16 recovery upgrade. Sanitized evidence accompanies this record; the
later test identities, semantic assertions and independent replay are documented
in the [rerun audit](../2026-10-16-chiado-batching-rerun/README.md), not rerun here.
