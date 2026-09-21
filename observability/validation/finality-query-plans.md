# Finality query resource regression

Broader schema18 runs killed the PostgreSQL backend while querying finality.
The first schema19 draft avoided a kill but still reached the 512 MiB cgroup
limit and its query exceeded the unchanged ten-second deadline. Per-view reader
plans isolated repeated expansion of canonical validation inside aggregates and
finality/admission joins. The final schema19 migration materializes ephemeral
statement-local validation results and reuses the canonical context already
selected for the actual admission. It preserves existing column order/types,
adds two canonical context columns to admission status, and leaves migration17
and18 checksums unchanged. These are SQL execution boundaries, not persisted
caches: every query still observes current conflicts, reorgs and freshness.

All affected canonical outcome, admission status, cost allocation, unit-cost and
finality SQL regressions passed. The regression database peaked at 449,306,624
bytes with zero cgroup limit/OOM events under its unchanged 536,870,912-byte limit.
The finality reader count plan measured 361.224 ms planning and 8.000 ms execution.
The fixture is small; this does not establish deployment-scale query capacity or
concurrent dashboard load. See the adjacent JSON for source digest and logs.

A separate administrative admission fixture initially retained JIT's default,
although actual writer/reader sessions explicitly disable it. That fixture now
uses the same JIT-off behavior and enforces the ten-second deadline; no predicate
was removed or deadline increased. Its initial failure is retained.

The next Grafana run completed all initial datasource queries and reached the
browser check; that browser attempt stopped at the real login form because HTTP
basic credentials did not create a UI session. The harness now logs in through
the form. This is not yet a browser-navigation pass.

The frozen schema19 observer `eea6a93bba854a5bbf1d2857a7cea9c3569b76d8493bd5ad40395f36d376fc3b` subsequently passed all **61 groups** in the full database process suite, including populated upgrades from saved versions1/4/5/6/8/13/14. The report is `/tmp/eez-observability-schema19-database.json`; prior schema18 failures remain retained.
