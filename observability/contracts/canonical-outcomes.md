# Canonical outcome anchors and selection (schema 11)

This extends [native coherence](native-outcomes.md) with independently collected
chain anchors. `lifecycle_outcome_anchors` preserves each claim and its explicit
validation state. `lifecycle_canonical_outcomes` selects one outcome per observed
admission identity and configured chain pair. `node_reconciled` means the native
runtime supplied exact membership/decision evidence and the observer checked its
available anchors. It does not mean independent source execution or follower replay.

No new endpoint, identity or enable setting is introduced. The existing lifecycle
projection worker refreshes native coherence and canonical progress snapshots;
SQL views derive assurance from current evidence on every read. Receipt accounting
remains independent of lifecycle/chain-outcome gaps.

## Anchor checks

| Evidence | Required check |
| --- | --- |
| Native claim | Current complete terminal/parent/manifest/repricing digests and successful bounded coherence validation |
| L1 scan watermark | Exact stored canonical block hash and number; never substituted for posting inclusion |
| L2 snapshot | Exact genesis identity, current verified settlement-range member, or historical endpoint bound to a canonical L1 root occurrence no later than the L1 watermark |
| Available posting receipt | Exact signed attempt hash, inclusion hash/number/index and observed status |
| Available applied post | Successful configured-contract posting call, exact calldata hash and occurrence, decoded verification table containing the configured rollup |
| Applied endpoint | Exact node-reported range endpoint plus configured EEZ emitter/rollup root in the post's own canonical L1 window |
| Source receipt basis | Independently fetched canonical source receipt with exact inclusion/index/status; missing fee fields do not erase valid receipt status |
| Outbound source block | Exact current settled range membership or historical canonical-root endpoint, no later than the scan watermark |
| Inbound committed source | Whole native effect membership, exact source receipt, planned consumed call hashes/queue indices from that receipt, and no known intervening successful post for the same EEZ/rollup |

A known canonical source revert contradicts a `Committed` success claim even if
the native grouping code used that label. Its receipt and paid costs remain
available. For outbound native group/canonical-inclusion claims, a missing source
receipt is explicitly unavailable; the node-reconciled membership and exact
settlement anchors do not fabricate a receipt success or fee. Available source
receipt status is exposed independently in the anchor view.

`SourceCompleted`, `SourceCompletedWithMissingEffects`, `SourceNonceSpent` and
`SourceReverted` retain their native distinctions. In particular, nonce movement
does not identify which transaction succeeded. An independently verified revert
receipt remains a `receipt_verified` source fact; the corresponding retained native
terminal handling can separately be `node_reconciled`.

Both chains must be fresh and unfrozen. Settlement evidence also retains its own
correlation freshness/conflict/finality gates. A missing or conflicting anchor
produces a named unknown state, not an assumed zero or successful completion.

## Same-height prefixes and posting windows

A post can apply prefix P of an L2 block and a later post can extend it to Q at the
same height. P may never have appeared in the observer's current-height scanner.
`canonical_settlement_endpoints` therefore checks P against its exact canonical
L1 post occurrence, retained node correlation endpoint and matching EEZ root log;
it does not require P to remain the current L2 hash or have `range_verified_at`.
Q remains a separate snapshot/source receipt fact. The proposed full sync hash,
actual applied endpoint and later snapshot are never interchangeable.

A root may be emitted by the post or a later companion in the same L1 block. Its
window ends at the next successful posting transaction to the same configured
EEZ contract whose decoded verification table lists this rollup. Posts listing
only other rollups do not reset this queue. An intervening tracked successful call
with unavailable/invalid posting classification or an unknown verification table
leaves the window unverified. Full receipt-log conflicts remove the entire log set.

The observer checks retained occurrences and available posting boundaries against
native membership; it does not independently replay execution or reconstruct every
unindexed internal call. These are explicitly node-reconciled associations. Exact
source/companion allocation remains a separate consumer and acceptance requirement.

## Conflicts, reorgs and admission history

Every complete terminal observation participates in selection, including a newly
imported terminal that the projection worker has not checked yet. A pending or
contradictory claim suppresses stronger assurance. All active claims must have
verified anchors and agree on source membership, outcome, owner and elapsed time.
Equivalent observations produce one admission outcome. Original event provenance
and individual anchor decisions remain inspectable.

Only positive evidence that the exact L1 scan watermark is orphaned removes an
old claim from selection. Absence, stale collection and missing parents cannot
prove orphaning. A later independently valid decision can therefore supersede
orphaned history without choosing a winner by wall clock or random epoch order.

The existing `lifecycle_admission_status` adds `canonical` status when exactly one
configured context verifies the actual admission's source. Local-policy handling
and unresolved/ended-epoch behavior remain intact. Membership from later events
can establish an observed canonical terminal identity without manufacturing a
missing `Admitted` row. Readmissions retain separate epoch/sequence identities.

## Progress signals

The observer emits recomputed gauges, not monotonic completion counters:

- `eez_observer_canonical_outcome_evaluation_valid` gates the snapshot.
- `eez_observer_canonical_terminal_admissions{direction,outcome}` counts verified
  admission outcomes across the five canonical terminal labels.
- `eez_observer_canonical_outcome_unresolved_admissions` reports retained native
  claims without selected assurance.
- `eez_observer_last_canonical_terminal_timestamp_seconds{direction}` tracks all
  verified terminal handling; `eez_observer_last_canonical_commit_timestamp_seconds`
  separately tracks successful commits.

Progress timestamps come from the canonical L1 scan-watermark block, not process
UTC emission time. A direction without known progress omits its timestamp rather
than emitting epoch zero. Repeated admissions of one transaction remain separate
admission observations here; these counts are not unique paid-source totals.

Counts, chain health and scan cursors are read in one PostgreSQL statement/snapshot.
Scraping suppresses them after a cursor change, a chain error, invalid source
health or 90-second staleness. SQL anchor/outcome views retract immediately on
committed evidence changes; cached gauges refresh on the next bounded projection
pass and carry the existing projection last-success timestamp. Missing/gapped
history remains visible and must accompany dashboard/alert interpretation.

## Upgrade and acceptance

Use explicit stop/backup/migrate/start. Schema 11 adds derived views and preserves
existing admission view columns. Old binaries refuse the newer schema; rollback
uses a compatible binary or separate tested restore. The reader role has only
SELECT access. No native signing or consensus data changes.

Observer connections and the packaged reader/writer roles disable PostgreSQL JIT
for this database. On the same one-claim fixture, JIT compiled for 13.25 seconds
and total execution took 13.37 seconds; interpreted execution took 160 ms, with
planning measured separately. The existing 10-second statement deadline is
unchanged. A custom Grafana login that inherits the reader role must also set
`jit=off` for its own database login; role configuration is not inherited through
`SET ROLE`. This is query-compilation evidence, not production capacity evidence.

Tests map R19 to pure native-coherence regressions, actual native lifecycle
captures, importer/projection tests and `tests/canonical-outcomes.sql` anchor and
selection cases. Cursor/staleness/reorg suppression of progress is covered by an
observer unit regression. These new schema-11 cases are under coordinated
validation; native restart/reorg workload acceptance and finality latency reporting
remain open. Earlier schema-10 test results do not establish schema-11 acceptance.
