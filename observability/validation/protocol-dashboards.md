# Protocol dashboard validation

The four new dashboards use actual schema-13 reader views and native metrics.
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

The dashboards explicitly identify unavailable producers for stage breakdowns,
slot/admission cuts, policy/release annotations, finality latency, companion/shared
cost allocation, balances/runway and independent follower/recovery capability.
These remain implementation requirements, not silently substituted measurements.
Mixed native workload reconciliation and browser-level drill-down/expired-trace
navigation remain acceptance work. The backend trace profile has its separate
validation record. See the [dashboard contract](../contracts/protocol-dashboards.md)
and [requirement ledger](../requirements.md); R21 is **partial**, not complete.
