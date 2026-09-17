# Native lifecycle capture

The Composer attaches an optional diagnostic capture context to its existing
held pool after both embedded chain specifications are available and before
cross-chain ingress starts. It uses the existing process publisher from
`EEZ_TELEMETRY_CONFIG`; an absent/disabled lifecycle writer leaves capture disabled.
No additional identity environment variables, queue owner or protocol journal
are introduced. Startup derives chain IDs/genesis hashes from those actual
specifications and the existing rollup configuration.

| Event | Implemented capture point | Evidence scope |
| --- | --- | --- |
| `admitted` | Successful `HeldPool::push_contiguous`, under its existing mutex, after nonce/replacement validation and queue mutation | A distinct admission ID is the captured epoch/sequence. Active duplicates, rejected submissions and internal requeues do not create admissions. |
| `terminal: replaced` | The same successful queued replacement | Explicit local-policy disposition of the prior retained admission; no canonical anchor or settlement claim. |
| `readiness` | Accepted pool readiness/blocking observation, while the hash remains queued or in flight | Exact evaluated L1/L2 block references and a bounded reason. Identical source/reason observations are deduplicated; changed source hashes are distinct observations. |

Legacy policy emits readiness observations after successful group validation
without changing its existing queue RPC fields. Enabled-policy capture shares the
existing readiness update. Neither path changes selection or settlement decisions.

A successful readmission links its previous admission when the old diagnostic
metadata is still retained. Metadata follows the existing queue/RPC ownership
retention, including retained ledger users; it is not an unbounded admission
archive. If earlier metadata was discarded, the new epoch/sequence still identifies
a new admission, but `previous` remains unknown. Recovery keeps the existing
admission identity. A replacement links the old admission explicitly and emits
its local terminal event. Inbound sources use L1 identity/rollup zero; outbound
sources use the configured L2 identity/rollup.

Capture reserves a ticket and copies small typed fields under the existing pool
owner. The caller releases the pool (and nested ledger) locks before serialization
and nonblocking publication. No raw envelope, private key or arbitrary error text
is copied into a lifecycle payload. Writer closure/loss cannot reject a protocol
admission. Replacement elapsed time uses `Instant`; the existing queue RPC's
wall-clock age fields retain their compatible behavior.

Readiness events are observations, not unique request counters or proof of
canonical completion. They require a currently active pool reservation; retained
terminal metadata alone cannot authorize another readiness event. Reorg invalidation
clears readiness deduplication. The existing RPC's evaluated source state remains
the stage-1 freshness producer. Native event counts must not be added to that RPC's
aggregate gauges.

Attempt/preparation membership, dispatch/repricing links, canonical reconciliation,
other terminal dispositions, exact reorg events, native queue/stage metrics and
trace context remain separate capture work. The transport schema names those events
but their presence in the schema is not evidence that the runtime emits them.
The observer's [transport history](lifecycle-ingestion.md) likewise does not
establish protocol-success assurance without the required anchor/membership checks.

Component regressions exercise accepted versus duplicate/rejected admission,
replacement links, both chain identities, readiness deduplication, same-height
source changes, recovery without readmission, explicit readmission after local
termination, late historical callbacks and a closed writer. The process-level
bidirectional canary additionally checks native source identity against RPC genesis
hashes while retaining the same semantic/proof assertions as its disabled baseline.
See the [requirement ledger](../requirements.md) for current validation status.
