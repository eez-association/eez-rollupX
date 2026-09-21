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
| Other local `terminal` | Actual reservation removal by direct eviction, in-flight release or recovery conflict | Local-policy disposition only; captures metadata before retention prunes it, then publishes only for removed users without an unresolved canonical owner. |

| `readiness` | Accepted pool readiness/blocking observation, while the hash remains queued or in flight | Exact evaluated L1/L2 block references and a bounded reason. Identical source/reason observations are deduplicated; changed source hashes are distinct observations. |
| `preparation_started` | First poll of rich, minimal or historical post-batch preparation | Distinct epoch/sequence preparation identity, proposed L2 endpoint and complete candidate admission membership before a signed attempt exists. |
| `preparation_finished` | Return or destruction of that preparation future | One typed outcome and monotonic elapsed duration; successful preparation does not imply ledger registration or dispatch. |
| `attempt_prepared` | Successful existing ledger `begin`, through the held-pool owner | Native generation/signed hash, exact admission membership and manifest coordinates, candidate stream bytes/blob allowance, nonce and proposed L2 endpoint. No receipt charge or canonical completion claim. |
| `attempt_repriced` | Successful replacement of a never-dispatched candidate with a different signed hash | Explicit previous/replacement IDs in the same native generation. An unchanged hash/byte pair emits no variant; failed replacements emit nothing. |
| `dispatched` | Accepted existing ledger dispatch callback before the network request | Actual resolved target number/timestamp, signed hash and nonce. Rejected duplicate or superseded callbacks emit nothing. |
| `submission_observed` | Accepted exact-attempt callback | Included, excluded, uncertain or prepared observation; inclusion alone does not resolve source ownership. |
| `reconciled` | Existing ledger resolution plus pool reservation update succeeds under the pool owner | Complete bounded decision snapshot: chain scopes, checked L1/L2 snapshots, actual post/source receipt positions, applied replay coordinates and changed admission dispositions. Native evidence remains an observer-validation input. |
| `terminal` linked to reconciliation | Actual changed ledger owner becomes terminal | Same admission/owner/outcome/assurance/monotonic duration as its parent resolution; canonical outcomes are native claims, and exhausted retries/nonce-gap cascades remain local policy. |
| `reorg` | Successful pool invalidation from an exact L1 watcher reorg observation | Actual common ancestor hash, old head hash and new head, scoped to both chain identities. Height-only finality-audit invalidation cannot manufacture this event. |


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

Preparation identity covers post-batch construction, witness/proof work and signing
inside `prepare_post_batch_raw`; it starts after source selection and Sync-block
construction. It is not the entire admission-to-completion duration. The existing
future retains ownership; no new executor/task is spawned. A small wrapper starts
on first poll and a move-only guard finishes once on return, cancellation or unwind.
An unpolled dropped future emits nothing. A process abort/crash can still lose the
finish; an absent finish stays unknown rather than being inferred as cancellation.

Preparation failure classification matches the existing typed error variants:
construction failure, capacity limit, prover failure or validation rejection. It
does not parse or export error strings and does not alter the returned error.
Cancellation and panic unwinding have separate outcomes. A successfully built
candidate receives its preparation ID in the later `attempt_prepared.details`,
even though preparation can also finish without any signed attempt being registered.
The started event retains candidate membership so failed preparations remain
discoverable by admission. Missing/oversized membership creates a counted start
gap while the original work continues; a finish alone cannot prove complete history.

The builder future is boxed once at the shared wrapper boundary to keep enclosing
futures within the repository's size limit. Boxing happens before polling, while
event allocation/publication still begins only on first poll. This adds a local
allocation, not a new task, retry or protocol decision. Resource/overhead acceptance
remains in the workload gates.

Before copying membership, capture checks limits of 4,096 members, 32,768 effects
and 32,768 source actions. Exceeding a limit or lacking any admission context loses
the entire capture with a counted `capture_limit`/`capture_context` gap; it never
truncates membership, fabricates an admission or rejects the protocol attempt.
The transport also enforces its independent 8 MiB encoded payload cap. These are
telemetry limits, not new transaction or protocol admission limits.

Native queue snapshots and preparation histograms have a separate
[operational metric contract](native-metrics.md); they work independently of the
lifecycle writer. Additional stage/terminal metrics and trace context retain their
separate acceptance gates. A transport schema field alone does not establish that
the runtime emits it.
The observer's [transport history](lifecycle-ingestion.md) likewise does not
establish protocol-success assurance without the required anchor/membership checks.

Reconciliation snapshots compare active ledger ownership before resolution with
its state after the existing pool reservation update. Duplicate, superseded,
stale-generation and failed partitions emit no decision. The complete before/after
copy is bounded to 4,096 candidate users (including queued/composing users that
may be affected by a nonce-gap cascade). Receipt-consumption coordinates have a
separate aggregate 32,768 limit. Missing or oversized evidence discards the whole
parent capture with a counted gap, while protocol ownership still advances.
Resolution ordering follows the ledger's ordered transaction-hash map.

The outer reconciliation `anchor` is the checked L1 scan watermark, **not** a
posting receipt inclusion. The details retain actual source and post block
hash/number/index/status, the applied L2 endpoint (which can differ from the
proposed Sync hash after partial application), replay selection, observed source
nonces and consumption coordinates. Outbound observations distinguish an applied
own group from source inclusion through a competing canonical batch. No extra
RPC calls or blob decoding are performed for capture.

A terminal child references the parent record's epoch/sequence and repeats its
exact disposition and elapsed duration. Its `anchor` is null; the exact evidence
is in the parent. The observer must validate parent/child agreement and membership,
then independently check the available chain anchors before presenting the
`node_reconciled` assurance. Neither row is another paid receipt or a separate
canonical completion to sum. A missing/conflicting parent leaves the child claim
unresolved. Local policy decisions never advance canonical terminal progress.
The compatible height-only `resolve_attempt` API captures canonical claims as
`unknown` when no receipt/endpoint facts were supplied.

Direct local disposal snapshots only selected active candidates. After the existing
pool mutation and ownership update, it emits for reservations actually removed.
Queued users passed to an in-flight-only release are unchanged; duplicate releases
emit no second terminal. An unresolved canonical ledger owner remains unresolved,
even if an ordinary operation removes its pool reservation. Snapshotting metadata
before retention allows pre-submission removals to retain their admission ID.
Readmission later allocates a fresh ID; it does not reopen a locally disposed ID.

Generic eviction reports `pool_discarded` for the root and `nonce_gap` for the
removed suffix. The existing settlement-failure path explicitly supplies
`attempts_exhausted` for its root. Generic release and recovery conflicts report
`pool_discarded`; the known unprocessed nonce suffix reports `nonce_gap`. When
reconciliation exhausts a submitted root, it captures any removed unsubmitted
suffix separately as local policy. Ledger-owned users remain exclusively in the
parent reconciliation partition, preventing duplicate terminal events.
These diagnostic distinctions do not rewrite the ledger's local-discard reason
or alter selection, retry counters, return values or recovery ordering.

Local candidate snapshots deduplicate hashes and enforce the same 4,096-user
limit. Exceeding it loses the whole snapshot with a counted gap while eviction
continues normally. Missing admission context likewise produces a capture gap.
The before/after snapshots and sequence allocation share the existing pool lock;
serialization and nonblocking publication happen after release. No persistent
terminal flag or second ownership journal is introduced.

Reorg capture follows the same pool-to-ledger lock order and publishes after
release. Reopening an owner preserves its admission and attempt; a later
reconciliation emits new evidence without rewriting the old event. Repeated
watcher observations remain observations, not counters of unique reorganizations.
The observer must retract affected anchor-dependent views using its independently
validated canonical chain, including when a native reorg event was lost.

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
Preparation regressions cover unchanged success/error results, source membership,
signed-attempt linkage, unpolled drop, cancellation, panic unwinding, disabled/closed
capture and membership gaps. Expanded native canaries require each registered
attempt to reference one matching start and one successful finish.

Reconciliation regressions cover distinct snapshot/inclusion blocks, same-block
receipt positions, applied versus proposed hashes, duplicate resolution, stale
snapshots, failed atomic partitions, retry charging, retained-owner nonce-gap
cascades, capture loss, concurrent callbacks and same-height reorg reopening.
Native bidirectional canaries additionally require a linked terminal/parent pair
and compare its source/post receipts and applied L2 endpoint with RPC. Current
execution results are recorded in the validation evidence; the assertions alone
do not establish a passed process gate.

Local-disposition regressions cover queued/in-flight and direction boundaries,
repeated release, explicit readmission, unresolved canonical owners, recovery
conflicts, unsubmitted nonce-gap victims, missing context, closed writers and the
snapshot limit. The reconciliation process report predates this local slice;
local-disposition process checks must be recorded separately.
