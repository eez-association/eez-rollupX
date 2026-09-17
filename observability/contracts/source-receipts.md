# Lifecycle-referenced source receipts (schemas 5 and 9)

This extends the [canonical indexer](canonical-indexer.md) and
[lifecycle importer](lifecycle-ingestion.md). It establishes receipt facts for
transaction hashes named by explicit scoped source references in retained lifecycle events. It does not establish companion
ownership, cross-chain completion, or a per-admission allocation of a shared fee.

An existing nonempty `lifecycle` configuration enables the independent source
receipt worker. An omitted/empty array leaves it disabled. No additional identity,
endpoint, credential or configuration knob is introduced. Both configured chain
instances must have been registered before source projection begins. Later
receipt failures are handled separately for each chain, while the normal receipt
scanners and lifecycle importers continue independently.

In `--once` mode, one bounded source pass follows lifecycle ingestion. As with
other optional enrichments, the ordinary receipt scan determines the exit code;
zero does not prove source lookup health or an empty backlog. Inspect the source
metrics, lookup/coverage views and logs. Large retained histories need additional
passes. Restart resets in-memory retry timers without deleting persisted requests.

## Selection and provenance

`eez.selected_transactions` records the original scanner's configured selection.
Migration backfills it from every pre-existing transaction. The original
`canonical_receipts`, wallet/posting views and totals retain that selection.
`canonical_receipt_facts` shares their immutable receipt storage and canonical
block flags, including additional source dependencies. The same checked parser,
transaction/receipt identity checks, exact fee arithmetic and SQL fact guards
serve both paths. Dependencies cannot change configured bootstrap or selection.

Each source pass projects at most 64 distinct event/source references per
configured lifecycle source. Eligible records are complete `admitted`,
`readiness`, `terminal`, `preparation_started`, `attempt_prepared`, and `reconciled`
events. The first three carry a direct source or member; preparation/attempt
records carry members; reconciliation carries `details.resolutions[].member`.
Only those explicit full `Source` objects are used. Bare transaction hashes in
other fields cannot supply missing chain/genesis/rollup/direction information.

SQL extracts and deduplicates the fixed-size source objects within each event;
the worker never downloads or deserializes enclosing manifests. Complete payloads
already have the lifecycle importer's 8 MiB bound, and existing database statement
deadlines also bound projection queries. A large membership record can take
multiple passes; no source is silently truncated at the 64-reference pass budget.
Duplicate members retain one event/source link. An omitted historical
reconciliation `details` value produces no invented references. Direction, chain ID, genesis hash and
rollup ID must match the configured pair. A mismatch is recorded and counted,
and produces no receipt request to an inferred or substituted endpoint.

Projection locks and rechecks the complete event before committing its source
link and lookup request. Links retain the exact source JSON and configured chain
context, so another configured pair cannot poison a correct projection with its
own mismatch result. A later transport conflict retracts the relationship. A
reviewed replay that changes the accepted source object creates another immutable
link version; only the object matching the current complete event is active.
Original receipt facts remain stored independently of the disputed relationship.

Lookup requests deduplicate by chain instance and transaction hash. Repeated
admissions, later lifecycle members and multiple producers therefore share one
receipt fact. An explicit later member can recover a source receipt despite loss
of its admission record, including after restart. It does not create an admission,
reconstruct queue history or validate the claimed terminal outcome. Existing
requests from formerly configured producers remain eligible while a complete
source-reference relationship remains, even if that producer's spool is no longer read.
Disabling the entire lifecycle feature stops these enrichment lookups.

## Lookup and recovery

A cycle considers at most 31 missing/gapped receipts and one already complete
receipt per chain, in oldest-attempt order. Missing/reorged receipts thus receive
priority without permanently skipping rechecks of known facts. Each chain has a
30-second work deadline and monotonic 15/30/60-second failure backoff. A failed
chain does not stop the other chain's turn. Successful cycles wait five seconds;
database/projection failures have their own bounded backoff. RPC calls share the
existing four-request semaphore, five-second HTTP timeout and response caps;
transaction/block retrieval uses bounded 8 MiB helpers. Schema 6 also gives receipt
retrieval that explicit 8 MiB bound to accommodate execution logs.

For an observed receipt, the worker checks its source identity and interval,
fetches the canonical block body and exact transaction, and verifies the hash at
the receipt's transaction position. It rechecks RPC chain/genesis and the captured
scan cursor before committing. Under the same cursor lock used by the scanner,
it requires the source block's exact hash/height to be indexed and canonical.
It never inserts extra historical blocks or extends the scan cursor.

The worker uses the shared immutable transaction/receipt writer. A selected
receipt can repair its existing accounting gap; an unselected dependency does not
alter selected receipt coverage. Conflicting complete facts reach the existing
SQL guard and freeze the affected chain. Frozen chains receive no enrichment
writes. Reorg flags retract source views immediately when the scanner commits
the reorg; reinclusion preserves both observations and exposes one canonical
charge. A deadline may leave already committed receipts or an uncertain final
commit; retrying uses the same unique keys and cannot duplicate their charges.

A successful RPC returning null means `not_observed`, not an RPC error or a zero
charge. A found receipt outside the indexed interval, an unavailable transaction,
an unindexed source block, an accounting gap and a failed lookup are separate
states. Existing complete charges survive a later incomplete response. Unknown
source outcomes remain unknown; local disposal does not prove a receipt can never
appear in a later canonical history.

## Reader views and signals

| View | Population / interpretation |
| --- | --- |
| `canonical_receipt_facts` | One row per stored canonical inclusion, including selected transactions and dependencies; `source_valid` retains the indexer's freshness/freeze gate |
| `lifecycle_declared_sources` | Explicit source objects extracted from complete eligible events, including references awaiting identity projection |
| `lifecycle_source_references` | Current complete identity-checked event/source links, with event kind; contains no inferred admissions |
| `lifecycle_source_admissions` | Subset of references whose actual event kind is `admitted`; repeated admissions remain separate relationships |
| `lifecycle_source_link_status` | Current projection result and configured context, including identity mismatches |
| `source_receipts` | Canonical receipt facts referenced by at least one active source link; `EXISTS` prevents join multiplication |
| `source_receipt_cost_totals` | Exact known source receipt costs, accounted/incomplete/reverted populations; overlaps with wallet spend and must not be added to it |
| `source_receipt_coverage` | Known sources, missing receipts, accounting gaps, failed lookups and distinct unresolved lookup states |
| `source_receipt_lookup_status` | Per-transaction lookup state, diagnostic reason and attempt/success timestamps |

`source_receipts.evidence_class` is `receipt_verified` only while the underlying
indexed chain source is valid; otherwise it is `unknown`. It is a receipt fact,
not a verified EEZ outcome. L2 `l2_settlement_verified` and
`l2_finality_verified` are true only with matching independently collected applied
hash/finality evidence. Null means not established (or not applicable on L1),
including unsafe or unavailable range evidence; absence is never relabelled as a
confirmed negative. These views describe the latest committed index snapshot,
not an instantaneous guarantee about an unsampled chain head.

Metrics use the `eez_observer_source_receipt_` prefix. Projection success,
last-success time, pending-admission count and identity errors describe the
bounded projection pass; success does not imply an empty backlog. The additional
`pending_references` gauge counts all unprojected distinct event/source references;
`pending_admissions` retains its original admission-only meaning. A single
multi-member event can therefore contribute multiple pending references. Receipt
collection success, last-success time, errors and coverage counts have only the
bounded `chain=l1|l2` label. Errors add the existing closed reason label. A failure
or a snapshot older than 90 seconds hides that chain's data counts while retaining
its last-success time. The other chain's healthy counts remain available.
Unobserved receipts and interval boundaries are distinct gauges, not failure
counts. Pair these signals with lifecycle gaps and canonical index coverage.

## Upgrade and limits

Use stop/backup/explicit `migrate`/start. Schema 9 adds the source extraction and
reference views, replaces only the affected view definitions, and leaves
immutable links and requests intact. Existing schema-5-through-8 admission links
are immediately visible through both the reference and admission views; retained
later members are recovered by subsequent bounded passes. Earlier binaries refuse
the newer schema. Rollback must use a tested compatible binary or a separate
restored backup. Selected costs, orphan/reinclusion history and complete lifecycle records
survive the upgrade. SQL roles remain separate; no native signing key or raw
pending envelope is required. The read-only role can inspect the listed views.

The lookup limits are initial bounded budgets, not measured production capacity.
Backlog, database growth, request load and retention must be calibrated before
rollout. Canonical outcome validation, restart disposition, source/companion
allocation and the protocol dashboards remain separate acceptance work. A complete
source receipt does not close any of those gates.

## Requirement coverage for later-member recovery

| Requirement | Implementation | Verification |
| --- | --- | --- |
| R10/R11 exact independently fetched source costs, no duplicate charge | `source_receipts.rs` shared lookup path; schema-9 `source_receipts` and lookup/coverage view selection use `EXISTS` over active references | `smoke-database.py` later-member case: source fee recovered, selected total unchanged, repeated members and multiple event kinds share receipt |
| R19 missing admission remains missing | Separate `lifecycle_source_admissions` predicate; no writes to admission history | Same process case asserts zero actual admissions/status rows while later members recover receipts |
| Scoped identity, replay and conflict retraction | Existing `source_role`; immutable context/source links; complete-current-event extractor joined in active views and rechecked under event lock | Existing identity/context cases plus later-member source replay/conflict cases |
| Bounded work and repeatability | 64 distinct references per configured source/pass; original chain-specific deadline, retry and request limits unchanged | 71-reference process fixture reaches 64 then 71; duplicate members collapse; existing chain isolation/deadline checks remain |
| Safe upgrade | Explicit immutable migration 9 and exact schema verification | Optional saved schema-8 binary lane verifies admission-link preservation and rejects downgrade |

The [schema-9 recovery validation record](../validation/source-references.md)
records the executed observer tests and disposable process/upgrade suite.
This extension does not implement companion allocation or canonical outcome
assurance.
