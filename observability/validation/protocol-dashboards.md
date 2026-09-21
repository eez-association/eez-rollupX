# Protocol dashboard validation

The four new dashboards now use actual schema-16 reader views and native metrics.
They are provisioned with the database profile alongside the two existing
operational dashboards; stage 1 continues to provision only those existing two.
No live node or monitoring deployment was changed.

## Executed checks

- `python3 -m unittest discover -s observability/tests -p 'test_prepare.py'`:
  **10 passed**. Includes profile enable/downgrade, secret preservation, strict
  explorer URL validation, optional trace/explorer link removal, stable datasource
  IDs and unknown-data conventions.
- `python3 observability/tests/smoke-protocol-dashboards.py --report /tmp/eez-protocol-dashboard-validation.json`:
  **passed 63 generated queries** through real Grafana PostgreSQL/Prometheus
  datasource APIs, including every new panel, variable and annotation query.
- Real Grafana loaded all six dashboard UIDs. Semantic assertions checked one
  canonical committed admission; historical P and current Q at the same L2 height;
  exact receipt amounts retained as strings and separated by chain; same-hash
  transactions on different chains; transaction filter escaping; request discovery
  and source-linked preparation/dispatch/submission timelines; local-policy
  annotations; configured
  explorer links; weighted blob utilization over 108,000 known bytes / three
  measured purchased blobs, with 2/3 post measurement coverage.
- Failing SQL collection made aggregate population panels unavailable rather
  than zero. Stopping the owned native metric fixture removed current queue values
  after the scrape failed, even though previous raw samples remained retained.
- Owned Python files compile; `git diff --check` passed.

The result and dashboard source hashes are retained in
[protocol-dashboards.json](protocol-dashboards.json). The full local log is
`/tmp/eez-protocol-dashboard-validation.log`. Tests use pinned image digests and
an isolated Docker network; all owned containers and temporary volumes were removed.
Synthetic SQL cache/blob rows test dashboard query semantics, not native ingestion,
KZG verification or independent protocol replay. Those remain separate suites.

## Integration findings and fixes

1. An internal Docker network did not expose the requested host port. The harness
   now executes its HTTP client inside that isolated network and publishes no ports.
2. The pinned Grafana macro parser incorrectly expanded nested expressions in
   `$__timeFilter(to_timestamp(...))`. All filters now place complete timestamp
   expressions between zero-argument `$__timeFrom()` and `$__timeTo()` boundaries.
   This failure was only visible through the actual datasource API.
3. Ordering a DISTINCT projection by its unprojected numeric source expressions
   failed PostgreSQL validation. Endpoint rows now deduplicate before the final
   exact-text projection, preserving numeric ordering and exact identifiers.
4. The initial endpoint query duplicated expanded posting views and exhausted the
   **unchanged 512 MiB fixture budget**, killing its PostgreSQL backend. The fixed
   query materializes only scoped/time-filtered receipt identities and their
   matching endpoint set before display. No budget or test timeout was raised.
   The final real Grafana call took **0.266 seconds**. Its reader-role
   `EXPLAIN (ANALYZE,BUFFERS,SETTINGS)` measured **53.791 ms planning / 1.244 ms
   execution**, with JIT disabled and a 10-second statement deadline.

First failures remain in `/tmp/eez-protocol-dashboard-smoke-{first,reviewed,macros,ordering,bounded}.*`;
the SIGKILL server log is
`/tmp/eez-protocol-dashboard-smoke-ordering-postgres.log`. The corrected plan is
`/tmp/eez-protocol-dashboard-validation-endpoint-plan.log`. Tiny fixture timings
are not production-volume query-latency or resource-capacity measurements.

## Scope still open

The baseline 63-query record predates the native-stage/policy, independent follower
and financial extensions. After c36bbd6 the native-stage/policy extension passed 68
real generated queries (`/tmp/eez-protocol-native-stages-validation.json`). Current
policy panels were then changed to instant queries and stronger value/outage
assertions were added; follower agreement/unknown/lag panels were also added.
The coordinated schema-14 run passed **73 generated queries**, including those
extensions, allocation coverage, exact `equal_share_v1` estimates, deduplicated
companion charges, source hash escaping and stale-source suppression. Two shares
of 100 base units reproduced a 200-unit posting receipt; its separate 200-unit
companion receipt appeared once. Dynamic follower fixtures distinguished agreement,
actual reported mismatch, lag, disabled configuration and stale comparison data.
These are real datasource tests with synthetic producer/cache inputs, not native
follower or settlement execution evidence.

The current report is [protocol-dashboards.json](protocol-dashboards.json), from
`/tmp/eez-protocol-costs-follower-dashboards.{json,log}`. Its endpoint query took
0.406 seconds through Grafana; reader EXPLAIN measured 87.136 ms planning and
1.383 ms execution within the same 512 MiB budget and 10-second deadline.
The earlier 63-query timings above remain historical evidence of the initial
resource fix. All owned containers/networks were removed after the current run.
Release annotations, finality latency and historical recovery capability remain
separately tracked gaps, never silently substituted measurements.
Mixed native workload reconciliation and browser-level drill-down/expired-trace
navigation remain acceptance work. The backend trace profile has its separate
validation record. See the [dashboard contract](../contracts/protocol-dashboards.md)
and [requirement ledger](../requirements.md); R21 is **partial**, not complete.

## Schema-16 combined gate

The next combined run passed **78 generated queries** through actual Grafana
datasources (`/tmp/eez-observability-schema16-dashboards.{json,log}`). It includes
the completed-source cost cohort and funding panels. One committed source with
allocation evidence reports exactly 100 base units; removing its allocation
evidence preserves the completed count, reports one missing measurement, and
returns NULL for the measured-subset mean. Funding cases distinguish actual zero
balance, unavailable balance, unknown burn, expired native candidate estimates
and separately scoped configured accounts. Existing stage/policy, follower,
canonical-cost and stale-source assertions also passed. The endpoint query took
0.270 seconds, within the unchanged 512 MiB database budget and 10-second reader
statement deadline. This is synthetic producer/SQL data, not native workload
acceptance. The JSON record contains the exact dashboard and migration digests;
earlier query counts and timings above remain historical.

## Finality and settled-transaction unit costs

The schema18 combined gate passed **82 generated queries**, including observed
finality percentiles/counts, explicit clock/finality gaps, maximum-u64 admission
time filtering, exact first-applied transaction costs, repeated applied ranges
and reverted fees. A three-transaction cohort with 551 base units of covered
posting spend reports floor(551/3)=183; it does not average the per-post 100 and
300 values. Receipt gaps remove uncertain denominators without removing paid
reverted-post fees. Final report: `/tmp/eez-observability-schema18-dashboards-final.json`;
its dashboard hashes describe the actual provisioned bytes and its migration
hashes describe the actual applied SQL. This avoids attributing a long-running
fixture to later shared-worktree edits.

The first combined attempt correctly showed stale lifecycle data after earlier
Prometheus scenarios; the fixture now refreshes only its owned synthetic source
before dependent finality assertions. Production freshness gates are unchanged.
First failure: `/tmp/eez-observability-schema18-dashboards.log`. The subsequent
release-annotation extension has separate validation; the 82-query result does
not validate that later change.

## Release and policy annotation extension

The final combined run passed **86 generated queries**, including actual Grafana
annotation API reads. Three synthetic release records used overlapping deployment
and environment tags; each configured release query returned exactly its one
matching record, excluding both other scopes. The 82-query finality/unit-cost
checks also remained green. Evidence: `/tmp/eez-observability-annotations-dashboards.{json,log}`.
The generator's two annotation tests and all 15 preparation tests passed.
Configured Composer startup/policy queries additionally passed against real Loki
in the node-agent fixture (see `node-agent.json`); unconfigured sources do not
acquire a synthetic policy history. No real deployment release was recorded.


## Actual browser interpolation and navigation (schema20)

The final fixture passed **86 datasource queries and 15 grouped checks**, including
an actual Chromium login, all four protocol dashboards, and all three links from
the retained request table. Request timeline, related attempts and source receipt
links preserve environment/deployment/time scope and select the exact source;
the destination panel's actual browser-issued query must return retained rows.
There were no page or datasource errors. The verifier uses Playwright 1.63.0,
matching the repository's existing browser tooling convention. CI now installs it
in a disposable runner directory and requires `--browser`; screenshots and bounded
query diagnostics are retained on failure.

Evidence: `/tmp/eez-observability-browser-linked-data.{json,log}` and the associated
`-browser/` directory (local-only); the sanitized report, actual provisioned
source hashes and browser verifier digest are retained in
[protocol-dashboards.json](protocol-dashboards.json). The three workflow contract
tests and all 30 observability Python tests passed. This is synthetic producer
data with actual backends and browser interactions, not a native mixed-workload
or production capacity result.

Actual navigation exposed failures that manually substituted datasource API
queries did not catch:

- Grafana custom All values bypass SQL formatters. Chain/rollup All now supplies
  its SQL string literal; instance All expands ordinary escaped lists for SQL and
  regular expressions for Prometheus. Configured metric-only native instances
  are included before the first lifecycle event, with an explicit unavailable
  fallback for an empty catalog.
- Concurrent large SQL plans exceeded the unchanged 512 MiB PostgreSQL budget.
  The generated reader datasource now permits one open/idle connection, queuing
  concurrent panels. Schema19 bounds repeated finality planning; schema20 bounds
  repeated cost-claim verification. The final browser/API run peaked at
  **437,710,848 bytes**, with zero memory-limit hits or OOM kills.
- Grafana itself was OOM-killed under the original 512 MiB limit (Docker recorded
  `oom` and exit 137 for `eez-dash-test-455df2b54f-grafana`). The packaged service
  now sets `GOMEMLIMIT=384MiB` within that unchanged container cap. The final
  fixture consumes the packaged 1-CPU/memory configuration and remained running,
  with zero OOM kills. Its cgroup did reclaim at the memory cap (4,033 limit hits;
  peak 536,875,008 bytes), so this is not a claim of spare production capacity.
  The [Go runtime documentation](https://pkg.go.dev/runtime#hdr-Environment_Variables)
  defines this as a soft Go-managed-memory target; container limits remain the
  hard boundary, including file caches and non-Go allocations.
- Browser-wide network idle is not a data contract for a live dashboard. The
  verifier now waits for completed datasource response validation, checks actual
  linked data and retains outstanding-request diagnostics. Query errors and
  failed datasource requests remain hard failures.

Prior failures remain under `/tmp/eez-observability-browser-{bounded-reader,datasource-sync}*`;
the earlier successful navigation-only run is
`/tmp/eez-observability-browser-grafana-memory.*`. The fixture now retains stopped
container exit/OOM state until diagnostics are collected and removes only its own
containers in final cleanup. Native mixed-workload reconciliation, expired-trace
browser presentation and deployment baseline acceptance remain separately open.


The exact-string rendering follow-up also passed 86 datasource queries and 15
grouped checks (`/tmp/eez-observability-browser-exact-strings.{json,log}`). It is the
current machine-readable record. PostgreSQL peaked at 446,160,896 bytes with no
limit hits or OOM kills; Grafana stayed running with no OOM kills (3,211 reclaim
limit hits). SQL string fields now use Grafana's explicit String unit, preserving
full decimal amounts and numeric-only hexadecimal identifiers instead of applying
SI abbreviations. Numeric duration/percentage fields retain their original units.
The earlier browser run exposed this as a 32-digit epoch displayed with a scaled
unit. The actual epoch UI regression is part of the separate trace-retention
browser gate; this datasource run does not itself prove the expiry presentation.


Expired-trace browser presentation subsequently passed in the actual retention
fixture: [trace-retention.md](trace-retention.md). It verifies the generated link,
exact Not Found result, preserved durable SQL reference and unscaled numeric-only
epoch. That closes the separately noted browser-expiry gate; native mixed-workload
and deployment baseline acceptance remain open.


## Quiet and exercised populations

The current follow-up passed **88 generated queries and 16 grouped checks**
(`/tmp/eez-observability-browser-quiet-range.{json,log}`), including real browser
idle presentation beside independently retained canonical success evidence. Fresh
native zero values display Idle / unexercised; positive work stays numeric.
Unknown/stale/failed native sources produce no values. Retained committed history
is unaffected by the queue becoming idle or unavailable, and disappears when its
own chain evidence becomes invalid. No metric absence becomes a synthetic zero
or a green service verdict. PostgreSQL peaked at 433,602,560 bytes with no memory
limit hits; both PostgreSQL and Grafana recorded zero OOM kills.

The first browser assertion used the 1970 SQL fixture range for live metrics and
correctly saw Unknown. The revised test checks current activity against a current
range, then historical SQL drill-downs against their original absolute range.
Native instant queries evaluate at the selected range end; this is explicit in
the new activity panel. The successful-history panel deliberately covers full
retained selected history and displays its checked L1 timestamp, so old success
cannot masquerade as recent exercise. Failure evidence remains in
`/tmp/eez-observability-browser-quiet.*`.
