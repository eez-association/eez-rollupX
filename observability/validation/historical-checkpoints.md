# Historical collection checkpoints

The schema-20 live legacy run using observer
`c43171967f9ce0766eff34ab0d1b86a39f11bdd6758467ff853663509436ddeb`
failed after 266.77 seconds: its first exact paid-post allocation remained
unavailable for the 180-second predicate deadline. This was not a query timeout.
The retained native database and original spool are local-only at
`/tmp/eez-native-observer-failure-xi3bBg`; the process log is
`/tmp/eez-native-observer-schema20-legacy-final.log`.

The retained database contains a coherent two-source membership for the paid
post in L1 block 16. Its correlation attempt was recorded but never committed a
settlement range; newer posting occurrences had successful ranges. Historical
collectors used the scanner's exact-tip comparison after RPC retrieval, so an
unrelated append could reject valid history as `inconsistent_chain_facts`.
Recording the attempt before that rejection also placed it behind continuously
arriving unattempted posts. The standalone database suites had not exercised
this live producer/observer interaction.

The fix preserves strict scanner cursor updates and adds a shared historical
checkpoint to settlement, protocol-log, source-receipt and blob collectors.
It checks the exact chain, canonical anchor and persisted reorg revision under
the chain lock. Settlement collection also interleaves retry/new work and keeps
processing the bounded selection after an individual failure. Partial passes
can verify unrelated complete ranges, but retain overall unknown health and do
not advance finality.

`tests/historical_checkpoints.py` executes the production SQL in real PostgreSQL
with the existing 512 MiB, one-CPU, 10-second-statement budget. It proves:

- The previous strict cursor comparison rejects an append; historical evidence
  accepts that append without changing its original anchor.
- Reorgs, restoration of the original hash, frozen chains, wrong contexts,
  incorrect heights and a tip below the anchor reject old observations.
- A retry remains selected beside new and complete work, including after another
  failed attempt.
- Independently complete ranges become verified on a partial pass while source
  health stays unknown and finality remains unchanged.

The fixture passed in
`/tmp/eez-historical-checkpoint-sql-reviewed-complete.log` and is included in the
full database suite. Initial fixture attempts exposed invalid frozen-reason,
immutable-range and missing posting-observation metadata setup; those setup
failures were retained and corrected without changing production constraints.

All 75 observer tests passed with local socket permissions. The first sandboxed
run passed 67 and rejected eight socket fixtures with `PermissionDenied`; it did
not establish socket-test coverage. Reviewed log:
`/tmp/eez-observer-historical-checkpoint-tests-reviewed.log`.
The resulting observer SHA256 is
`ece81ef88ea7e7798c9d379b98ead21ca8eab5980064d8c2f79e06b93c0b9f68`.
The full database suite then passed all 64 groups, including saved-version
upgrades and this regression: `/tmp/eez-observability-schema20-append-database.json`
and its adjacent log. Repeated live native acceptance remains a separate gate.
