# Cost attribution and shared-fee validation

Worktree based on `c36bbd6`, with the agreed main and protocol pins unchanged.
No live services or funded workload was used.

## Implemented behavior

Schema 14 adds digest-bound native attempt populations, exact companion receipt
attribution and versioned integer shared-fee estimates. The observer checks paid
reconciliations even when every source was requeued and no terminal event exists.
Receipt totals remain independently queryable when attribution is unavailable.
The [contract](../contracts/cost-allocation.md) defines evidence, scope, missing
coverage and the equal-share assumption required by the proposal's unspecified
allocation weights.

## Executed checks

- Observer unit suite: **65 passed** in the combined run, including shared
  prepared/repricing validation, paid/requeued attempts without completion, source
  scope and exact receipt-reference rejection. Follower tests account for some of
  this combined count; it is not a count of new allocation tests.
- Initial complete schema-14 database run: **47 check groups passed**, including
  actual lifecycle importer/cost projection and the new canonical SQL fixture.
  Report: `/tmp/eez-observability-costs-database-reviewed.json`.
- Saved-binary upgrade run: **55 check groups passed** using saved versions 1, 4,
  5, 6, 8 and 13. The actual schema-13 binary
  `9de5c53e16545e8e72c5001eb3e8360ccdcf5adb63da9d36eea6b006a6c4f86e`
  created populated receipt/lifecycle/native-anchor data; schema 14 preserved
  exact costs and outcome anchors, left absent cost populations unknown, and the
  old binary explicitly refused downgrade. Report:
  `/tmp/eez-observability-costs-database-upgrades.json`.
- Both complete runs used observer
  `d9ce7136eeccdf44055404ed62d03d91ed6821155181fbe166f096c8ccaaf297`.
  A later SQL-only correction excludes explicitly unrelated rollup posts from
  allocation coverage. The final SQL fixture passed **22 assertions** against
  all migrations in an isolated 512 MiB PostgreSQL instance; its log is
  `/tmp/eez-observability-costs-sql-scope.log`. Final combined binary validation
  must include that later migration checksum.
- The integrated dashboard run passed **73 generated Grafana queries**, including
  two exact 100-unit shared estimates reproducing one 200-unit posting charge,
  one separate 200-unit companion receipt, source filtering/escaping, unallocated
  coverage and stale-source suppression. See [dashboard evidence](protocol-dashboards.md).

SQL assertions include execution/blob component conservation, deterministic odd
remainders, reverted posts and companions, absence of terminal rows, duplicate
and conflicting native populations, new unchecked claims, changed digest
dependencies, stale sources, orphaned watermarks/receipts, ambiguous candidate
posts, empty populations and unrelated/multi-rollup boundaries. SQL cache/wire
stubs are intentionally synthetic; real importer and typed-checker tests provide
separate coverage. Neither constitutes independent protocol replay.

## Findings and remaining gates

The first full fixture tried to orphan a receipt by updating its flag directly.
The existing trigger correctly derives that flag from its block, so the test did
not create a reorg and failed. The corrected fixture changes the block's canonical
state. No runtime guard was weakened. First evidence remains in
`/tmp/eez-observability-costs-database.{json,log}`.

A temporary focused SQL harness initially used the obsolete PostgreSQL data
directory mount and failed to start PostgreSQL 18. It was corrected to the
repository's existing container setup; successful runs cleaned only their owned
containers and volumes. This was a local test harness failure, not a migration
failure.

R11 still needs exact settled-L2-transaction unit-cost denominators and the mixed
native workload cost reconciliation gate. Complete cross-rollup membership is
not inferred; such overhead remains explicitly unallocated. Larger-population
query/capacity and final workspace/native integration checks remain required.

## Schema 16 claim invalidation correction

Independent review found two real gaps in schema 14's asynchronous membership
view: a new producer was absent until its first check, and quarantining a parent
could remove its claim from the active population. A failed check could also
clear its cached post identity before the parent later disappeared. These could
leave another cached population falsely complete. Companions had a related
cross-post window: an unchecked competing declaration was absent from the
verified candidate set and could leave a falsely unique owner.

The additive schema-16 registry captures explicit scoped declarations at import,
retains identity independently of check outcome, and considers pending competing
companion claims without authorizing them. Schema-14/15 checksums and existing
view column types remain unchanged. The first standalone migration attempt
caught a hash-domain versus `bytea` view compatibility error; the replacement
projection now preserves the original column type. A separate test startup hit
PostgreSQL's temporary initialization server; the owned SQL-only harness now
waits for final initialization. Both failed logs are retained.

- Fresh source migrations 1–16 plus the complete existing/extended cost fixture
  passed **32 SQL assertions**, under the unchanged 10-second statement deadline
  in a private 512 MiB PostgreSQL container with networking disabled.
  Log: `/tmp/eez-cost16-sql-ready-reviewed.log`.
- A populated pre-16 database then passed five additional upgrade assertions:
  backfill from complete parents, cached-only quarantined-parent scope/identity,
  explicit uncertainty for an unrecoverable legacy gap, conservative allocation
  and unchanged exact receipt totals.
  Log: `/tmp/eez-cost16-populated-upgrade.log`.
- New regressions cover a producer with no check rows, parent quarantine behind
  another verified duplicate, initial gap → quarantine, coherent → gap →
  quarantine, an unchecked competing companion, its later quarantine, and
  positive orphan retirement. A different post's uncertainty does not erase the
  known posting shares of an unrelated post.

Schema 16 at this checkpoint has SHA-256
`b6dd8de4577fa19db36b8d477dde6af653a4205dafde4e1138ed9242876cf596`.
These are actual SQL checks, distinct from a saved-binary upgrade. The
coordinator's final observer build and saved-binary/full importer suite must
include schema 15 and 16 before final acceptance. Larger historical backfills
still need deployment-scale migration/query budgeting; uncertain legacy
attribution is never repaired by inventing scope or a post hash.

## Combined schema16 and subsequent unit-cost validation

The frozen schema16 observer `723555148a6afa72fc00d58053a59cc5e1c6c6d9fbe1c6cd28b2a49a734b1099`
passed all 59 groups in the full database process suite, including populated
saved-version upgrades (1/4/5/6/8/13/14), claim invalidation and funding while
canonical heads advance. Report/log: `/tmp/eez-observability-schema16-database-reviewed.{json,log}`.
The first frozen-harness attempt omitted its required follower Rust SQL source;
that failure remains in `/tmp/eez-observability-schema16-database.log`. No database
assertion or deadline was weakened. These results predate migrations17/18.

Schema18's focused real PostgreSQL fixture subsequently passed 16 assertions
covering exact first-applied counts, overlapping ranges, reverted L2 transactions,
reverted posting fees, missing receipts, stale collection, exact-hash reorgs,
historical P/Q prefixes, conflicting claims, prior classification/history gaps,
selective scans and multi-rollup attribution gaps. The 512 MiB/1CPU database
budget and 10-second statement deadline were unchanged. Evidence and source
digests are in the adjacent JSON; combined runtime/dashboard validation remains
separate until its current result is recorded.

## Schema-20 live-query regression

The first live native→observer legacy integration, using schema19 binary
`eea6a93bba854a5bbf1d2857a7cea9c3569b76d8493bd5ad40395f36d376fc3b`,
reached exact paid-post/outcome/receipt checks but failed a source-filtered
allocation SELECT at the unchanged 10-second statement deadline. The same run
exposed a separate FK/chain-lock deadlock, fixed and tested by the coordinator.
Failure logs remain `/tmp/eez-native-observer-lifecycle-legacy-reviewed.log` and
`/tmp/eez-native-observer-first-deadlock-postgres.log`. These failures are not
counted as live workload acceptance.

An existing successful canonical SQL fixture plus 100 equivalent paid claims
reproduced the allocation timeout without a native race. The first attempted
per-post aggregate materialization did not solve it and was not shipped.
EXPLAIN with 11 claims showed the underlying digest validation executed 121
times before the declared-claim join filter. Schema20 materializes that thin
validation population once per configured chain pair. The identical 101-claim
query then took 111ms planning and 335ms execution; peak container memory was
162,926,592 bytes, with max/oom/oom_kill counters zero. The explicitly scoped
reader-role regression took 75ms planning and 189ms execution and retained exactly
one source share with all 101 claims verified. Both stayed within 512 MiB/one CPU
and the 10-second server limit.

All 32 existing cost canonicality/conflict/scope/conservation assertions passed
against this change. `tests/cost_query_boundaries.py` adds the duplicate-history
reader regression to the full database suite. Migration20 SHA256:
`b3186d6738c69996abc0f273dd397e65f93aefaa06cacd6f2a5cfb7544b33e45`.
Logs: `/tmp/eez-cost20-scale-first.log`,
`/tmp/eez-cost20-materialized.log`,
`/tmp/eez-cost20-materialized-invariants.log`, and
`/tmp/eez-cost20-reader-check.log`. Final live native re-execution is pending the
coordinated schema20 build; synthetic regression success does not replace it.
