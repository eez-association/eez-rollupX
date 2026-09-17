# Admission to observed finality (schema 17)

`lifecycle_admission_finality` reports a separate finality duration for actual
`Admitted` observations. Existing native monotonic admission-to-reconciliation
durations remain unchanged. Later source membership never invents an admission
or reconstructs its UTC timestamp from a terminal duration.

The existing scanner records `blocks.finalized_observed_at` on its first actual
canonical finalization transition. PostgreSQL captures the observation timestamp
in the transaction that changes finality; rollback removes both. Repeated scans,
restart and unrelated updates retain the first timestamp. Previously finalized
rows have no historical observation timestamp and remain explicitly unknown.
Migration time and block timestamps are never substituted for this missing data.

The finality view requires the existing selected canonical outcome and all its
current native/canonical checks. Its exact L1 scan watermark must be finalized.
Every non-genesis L2 snapshot, applied endpoint and outbound source reference
must match an exact current canonical settled hash/number with retained verified
L2 finality. Historical prefix P cannot borrow current same-height Q's finality;
its canonical outcome can remain valid while finality is unknown. A missing
dependency, stale source, late event conflict, unfinalized reorg or frozen chain
retracts positive timing on the next query. Receipt fees remain independent.

The observation time is the latest required finality observation across the
selected equivalent claims. The duration is that UTC time minus the actual
admission's producer UTC. It includes finality collection/backfill delay and may
conservatively include the later native scan watermark. It is **observed
finality**, not an estimate of the earliest consensus-finalization instant.

The explicit `clock_basis` is `synchronized_utc_assumed_not_verified`. This view
does not prove clock synchronization. Admission UTC more than five seconds ahead
of import/current time, finality observation more than five seconds ahead of
current time, a reversed admission/terminal order, or more than five seconds of
disagreement between producer UTC elapsed time and native monotonic elapsed time
produces `clock_inconsistent` with no duration. Undetected common clock offset
remains a limitation. A finality observation predating a late admission is
`finality_observed_before_admission`, also without a zero-clamped duration.

The cross-chain dashboard shows p50/p95/p99 and measured counts by direction and
outcome for the selected **admission cohort**, alongside all timing/unknown states
and the existing current pending/serviceable age distributions. Source-only
reverts, successful commits and local-policy dispositions remain distinct.
Percentiles exclude missing or inconsistent durations without hiding their counts.

No new polling worker, endpoint, configuration option or native persistence is
introduced. The existing explicit stop/backup/migrate/start procedure applies.
Schema 17 is additive; prior migrations and their checksums are unchanged. Old
binaries reject the newer schema. Downgrade requires a compatible binary or a
separate restored backup; no down migration fabricates historical timing.

| Requirement | Implementation | Verification |
| --- | --- | --- |
| R19 exact canonical finality | Schema 17 dependencies/claims views reuse schema 3 and 11 assurance | `tests/finality_timing.py`: exact P/Q distinction, L1-only insufficiency, actual Q proof, late conflict, reorg and freeze |
| R19 timestamp persistence and missing history | Finalization trigger; actual-admission join | Same fixture: repeat update, missing admission, historical NULL, rollback; existing scanner restart/finality tests remain applicable |
| R19/R21 separate latency and clock semantics | UTC classification and nullable duration; unchanged monotonic reconciliation view | Same fixture: producer drift/future admission, observer wall-clock reversal and pre-admission finality |
| R21 counts, taxonomy and visible gaps | Cross-chain panels 17–18 | Real Grafana query fixture: measured source revert, percentiles/count, clock gap and frozen evidence |
