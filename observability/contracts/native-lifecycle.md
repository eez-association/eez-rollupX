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
| `attempt_prepared` | Successful existing ledger `begin`, through the held-pool owner | Native generation/signed hash, exact admission membership and manifest coordinates, candidate stream bytes/blob allowance, nonce and proposed L2 endpoint. No receipt charge or canonical completion claim. |
| `attempt_repriced` | Successful replacement of a never-dispatched candidate with a different signed hash | Explicit previous/replacement IDs in the same native generation. An unchanged hash/byte pair emits no variant; failed replacements emit nothing. |
| `dispatched` | Accepted existing ledger dispatch callback before the network request | Actual resolved target number/timestamp, signed hash and nonce. Rejected duplicate or superseded callbacks emit nothing. |
| `submission_observed` | Accepted exact-attempt callback | Included, excluded, uncertain or prepared observation; inclusion alone does not resolve source ownership. |

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

Attempt registration and submission wrappers retain the existing pool-to-ledger
lock order. They call the existing mutation, then reserve capture only after that
mutation succeeds, while the pool still excludes ingress/recovery. No event is
emitted from the ledger's fallible cloned-state mutation. Publication happens after
both locks are released. Native generation allocation, repricing validation and
late-callback behavior remain in the existing ledger.

Preparation returns its already measured encoded length, blob count/allowance and
signing nonce alongside the signed bytes. No telemetry decoder reprocesses the
envelope. Repricing uses the existing codec's fee/gas-only replacement, preserving
that nonce and manifest. Membership plans are in the same order as event members:
source sender/nonce/charged episodes, outbound group, producing-effect interval,
source-action coordinates and complete effect endpoint hashes. These coordinates
are diagnostic node evidence, not independent replay verification.

Before copying membership, capture checks limits of 4,096 members, 32,768 effects
and 32,768 source actions. Exceeding a limit or lacking any admission context loses
the entire capture with a counted `capture_limit`/`capture_context` gap; it never
truncates membership, fabricates an admission or rejects the protocol attempt.
The transport also enforces its independent 8 MiB encoded payload cap. These are
telemetry limits, not new transaction or protocol admission limits.

Pre-signing preparation identities, canonical reconciliation, other terminal
dispositions, exact reorg events, native queue/stage metrics and trace context
remain separate capture work. The transport schema names some of those events
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

Attempt regressions additionally cover failed registration without a sequence,
same-generation repricing, no-op/invalid replacement, duplicate and concurrent
dispatch, superseded callbacks, repeated source membership across retry generations,
and capture gaps that preserve protocol ownership. Process acceptance for the
expanded attempt assertions is recorded separately from the earlier admission-only
canary; an earlier passing binary does not validate new capture paths.
