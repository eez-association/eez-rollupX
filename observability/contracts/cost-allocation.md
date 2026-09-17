# Receipt costs and native allocation populations

Schema 14 adds cost attribution without changing receipt accounting. The observer
projects complete native reconciliations through the same prepared-membership and
ordered repricing validator used by canonical outcome checking. A terminal event
is not required: a reverted paid posting attempt can requeue every member. The
node's cost-report logs are not summed or used as the billing ledger.

`lifecycle_cost_checks` retains the exact transport digests and chain context.
Readers invalidate changed evidence or additional earlier lineage immediately.
New, unprojected complete reconciliations prevent an older cached population from
being treated as complete. All active claims for a post must be verified and
agree on its source population. Only a positively orphaned snapshot is excluded
from that comparison. Duplicate admissions/producers do not duplicate a paid
receipt. Source admission IDs remain in lifecycle history; the allocation unit is
the unique scoped source transaction.

Schema 16 preserves declared claim identity before asynchronous checking can
succeed or fail. An importer trigger records the explicit producer/epoch/sequence,
L1/L2 scopes, post identity, snapshot and bounded minimal receipt declarations.
The registry is independent of `lifecycle_cost_checks.decision`: a check gap or a
quarantined parent cannot erase a claim and make another population falsely
complete. New producers participate before their first check. Existing complete
parents and earlier coherent decisions are backfilled without changing schema
14's checksum or inferring a scope from a bare hash. Declaration alone never
authorizes an allocation; all active claims must still pass independent checks.

If an older gap had already lost both its parent and cached decision before this
migration, its precise post cannot be recovered. The affected configured chain
context stays explicitly unknown until evidence is recovered. This affects
allocation estimates, not independently retained paid receipt totals. Explicitly
unpaid declarations remain separate and do not create synthetic paid claims.

The independent projection worker uses a separate database connection and a
30-second pass, with 24 new and eight repair candidates interleaved repair-first.
It bounds lineage to 128 records, a single normalized record to 16 MiB and a
check to 32 MiB. SQL retains the existing statement/lock deadlines. Cancellation
leaves the candidate eligible for the fair repair lane. Optional enrichment does
not change the receipt scanner's `--once` success or native execution behavior.
Projection success and last-success metrics report collection health, not a
source completion or verified cost count.

## Canonical evidence and companions

`lifecycle_cost_evidence` checks the exact L1 scan watermark, posting inclusion
hash, number, transaction position and receipt status; both configured chains
must be fresh and unfrozen. The decoded call must list the configured rollup.
Successful posts additionally require a validated canonical settlement endpoint.
Reverted posts retain their paid cost without claiming an applied settlement.

`companion_receipts` requires explicit inbound attempt membership, an independently
fetched matching source receipt and the same L1 block hash after the post's exact
position. A relevant successful intervening post, or a call whose classification
is unknown, closes the provable window. Posts that list only other rollups do not
close it. Multiple candidate post/context owners leave attribution unknown.
Missing posting receipts cannot be replaced with a dispatch target height.

Uniqueness also considers unchecked explicit competing companion declarations.
A second post cannot become invisible merely because its producer has not yet
been projected, its check failed, or its parent was quarantined. That competing
claim can prevent attribution but cannot authorize its own charge. Positive
orphaning of its exact retained snapshot retires it. Unknown/unbounded source
populations and unrecoverable legacy claim contexts stay conservative.

Canonical reverted companions remain charged. Missing accounting fields remain
NULL/gaps. Source receipt costs remain queryable even when attribution is unknown;
L2 user fees remain a separate chain population. Neither source inclusion nor
companion attribution establishes successful cross-chain effects.

## Explicit shared-fee estimate

The proposal requires an explicit allocation rule but does not specify weights.
`equal_share_v1` divides each posting fee component equally among the complete
unique native source population. This conservative estimate uses no invented
per-source gas measurements. Source order is `(chain_id, genesis_hash, rollup_id,
direction, transaction_hash)`, with inbound before outbound. Each source receives
the integer quotient; the first remainder-count sources receive one additional
base unit. Execution and blob components are allocated separately, so their sums
each reproduce the exact receipt amount. Results are **estimates of shared
overhead**, not exact receipt charges paid by those users.

`posting_allocation_status` exposes reasons for unallocated overhead, including
missing/conflicting population, missing accounting fields, stale receipt data,
no source members and unknown multi-rollup populations. A pure-L2 posting call
does not acquire synthetic source members. The initial single-rollup population
cannot claim the full fee of a multi-rollup post: those fees remain unallocated
until complete cross-rollup membership is available. This follows the proposal's
explicit missing-coverage rule rather than assigning unknown participants zero.

`source_posting_allocations` has one row per canonical post/context/source. Never
sum posting receipt totals after joining these rows; sum their allocated component
columns, or query the receipt population separately. Companion receipts are
deduplicated before many-to-many lifecycle joins. Wallet, posting and companion
populations can overlap; they are not independent bills to add together.

The completed-source dashboard groups distinct scoped transactions that currently
have a verified `committed` outcome. Its time filter selects the first retained
canonical reconciliation watermark for that source; it then sums all known
posting allocations for that cohort, including paid retry attempts. Its
integer-floor mean uses only sources with allocation evidence. Sources without
such evidence retain a separate missing count and NULL cost, never a zero-cost
contribution. This is a measured-subset posting-cost estimate, not a complete
external bill; direct source receipts, L2 fees and unattributed posting overhead
remain separate.

## Upgrade and validation scope

Run the explicit migrations before starting the schema-20 observer. Older binaries
refuse the newer schema; rollback uses a compatible binary or a separately restored
database. Existing receipt identities, amounts and orphan history are unchanged.
The reader receives only the new views; the projection table remains writer-only.

Unit tests cover shared membership and paid/requeued attempts without promoting
terminal outcomes. The importer fixture covers missing admissions, independently
keyed chain contexts, digest/lineage conflicts and idempotent cost projection. The
SQL fixture exercises exact shares, odd remainders, reverted companions, duplicate
and conflicting claims, late unchecked evidence and canonical retraction. These
synthetic fixtures are separate from native mixed-workload acceptance. Dashboard
reconciliation and larger-population performance remain acceptance work until recorded.

## Settled L2 transaction unit costs

Schema 18's `settled_transaction_costs` uses exact currently canonical applied
block hashes and complete receipts from the existing `include_all_transactions`
L2 scan. It counts distinct transaction hashes, including reverted L2 transactions,
against their first observed canonical application ordered by L1 block and
transaction position. Overlapping repeated ranges cannot count a transaction
twice. A successful repeated range can therefore have zero new transactions.
Reverted posting calls retain their fees and have zero applied transactions.
Division by zero yields NULL; integer display ratios use an explicit floor.

Proving first application requires retained history: both chain scans must start
at genesis, earlier relevant successful posts must have complete verified ranges,
and earlier contract calls must have known classification. A later bootstrap does
not prove that a transaction was never settled previously. Existing selective
scans, partial bootstrap, unavailable historical prefixes, conflicting claims,
missing receipts and multi-rollup fees without an allocation therefore have
explicit unknown denominators. Configuration defaults and persisted selection
remain unchanged. A full-history namespace/rescan is optional operational work,
not something the observer silently performs on upgrade.

In particular, a historical prefix P and later full block Q at the same height
are different hashes. Q cannot supply missing P membership. This reader keeps
the corresponding unit costs unknown while exact independently paid fees remain
available. It neither changes canonical outcomes nor invents transaction counts
from source members or a proposed block.

The dashboard's aggregate uses the selected L1 posting-time population. It divides
the sum of covered posting fees (including covered reverted posts) by the sum of
known newly settled transactions. It displays measured and unmeasured post counts
alongside the ratio, and never averages per-post ratios. Companions and L2 user
fees remain separate. Freshness failures, orphaned receipts and newly conflicting
range evidence retract these derived values on the next SQL read.

## Query evaluation boundary (schema 20)

Native cost evidence is evaluated once per exact configured L1/L2 chain pair
before joining durable declared claims. PostgreSQL previously flattened that
join and repeated digest/lineage validation for candidate join pairs: 11 checks
executed the same validation 121 times. A source-filtered allocation query with
101 equivalent valid claims exceeded the unchanged 10-second deadline.

The materialized population contains only identifiers, current evidence state
and source population. Context filters remain on the outer chain-pair relation;
new producers without any scheduled check still enter through their explicit
deployment/chain/genesis/rollup declaration. Missing checks, conflicting parents,
positive orphan retirement, global unbound uncertainty and competing companion
claims preserve their previous semantics. No receipt or persisted claim is
rewritten, and no historical claim is ignored to meet the budget.

The reader-role regression keeps the same 512 MiB/one-CPU database and 10-second
statement limit. It verifies all 101 claims remain active and verified while the
source retains exactly one allocated share. Larger deployment capacity remains
a measured rollout concern; this bound is not a claim of unlimited history.
