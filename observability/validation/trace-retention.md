# Actual trace retention fixture

## Browser and schema20 follow-up

The current [machine-readable result](trace-retention.json) passed **six acceptance
groups in 192.34 seconds**, including three actual Chromium assertions and verified
owned-resource cleanup. Observer SHA-256:
`ece81ef88ea7e7798c9d379b98ead21ca8eab5980064d8c2f79e06b93c0b9f68`.
Local report/log: `/tmp/eez-observability-retention-browser-final.{json,log}`;
browser report/screenshot: the associated `-browser/` directory. The report binds
the generated dashboards and browser verifier to their actual SHA-256 digests.

The browser follows the real retained SQL table link into Grafana Explore, issues
the exact expired trace-ID query and displays the downstream Not Found error.
The original SQL trace reference remains visible; the full structured accounting,
cursor and lifecycle snapshot remains unchanged, and a recent control trace still
queries successfully. Numeric-only epoch identifiers render as their exact 32
hexadecimal characters, without SI scaling. CI requires `--browser` and installs
pinned Playwright 1.63.0/Chromium before this fixture.

This caught a generated-link bug: the bundled Tempo frontend selects the
`traceql` editor mode and internally converts a hexadecimal input into its
backend `traceId` request. Supplying `traceId` as the frontend mode silently
produced a generic No data view without a request. The corrected link follows the
pinned plugin's dispatcher (read from its shipped source map). The browser also
scopes its click to the named retained-trace panel, rather than a duplicate ID in
the timeline. Grafana wraps Tempo's 404 as a datasource request 400/result 500 with
an exact Not Found message; the test checks that real representation instead of
requiring a frontend 404. The separate direct query still must return 404.

Failures are retained under `/tmp/eez-observability-retention-browser{,-query,-panel}*`.
They are not counted as passes. The prior schema16 retention result below remains
historical evidence; the browser follow-up adds actual UI navigation and exact
string rendering, not a production baseline or overhead result.


The earlier schema16 run passed **five acceptance groups**, cleanup verified,
using observer
`723555148a6afa72fc00d58053a59cc5e1c6c6d9fbe1c6cd28b2a49a734b1099`.
The controlled run completed its semantic checks in 185.62 seconds; cleanup
was then verified. The complete source/report is `/tmp/eez-trace-retention-reviewed.{json,log}`.

`smoke-trace-retention.py` reuses the pinned Compose configuration and the existing
restore fixture's PostgreSQL/indexer setup. It runs only in a unique disposable
project, with ephemeral loopback ports and private credentials. Production Tempo
configuration and its **168-hour** retention remain unchanged.

The test admits an eight-day-old synthetic OTLP trace through Alloy and queries
it successfully through Grafana. Read-only volume inspection must then observe
its real persisted block with an end timestamp before the seven-day cutoff,
followed by `meta.compacted.json` for the same block and eventual removal of the
entire block directory. A subsequent Grafana trace request must return 404 while
a recent control trace remains queryable. PostgreSQL receipt totals, observations,
cursors, migration checksums, lifecycle payloads and import checkpoints must be
equal as structured data to their populated pre-expiry snapshot; the expired
trace's identity must remain in its lifecycle row.

Fixture-only overrides admit old timestamps (240-hour ingestion slack), shorten
block flush/poll/live-cache cleanup cadence and schedule retention work every
90 seconds. Marked-block retention is ten seconds, exceeding twice the two-second
blocklist poll. The live-store reader reclamation grace retains its default;
production admission and timing settings are not changed. These overrides make
the actual retention path observable within minutes without changing the
seven-day eligibility rule or directly deleting any block.

The initial run proved historical ingestion, backend marking and deletion but
failed the final query-unavailable assertion: live-store cleanup retained its
five-minute default cadence. The pinned implementation uses the misleadingly
named `flush_op_timeout` field as the interval for
[`perTenantCleanupLoop`](https://github.com/grafana/tempo/blob/v3.0.3/modules/livestore/live_store_background.go).
The reviewed fixture sets only that test cadence to two seconds. Initial failure
evidence is retained at `/tmp/eez-trace-retention-first.{json,log,logs.txt}`.

This is a controlled historical-data retention test, not seven elapsed days of
production operation, a storage-capacity measurement, or proof of a hard disk
quota. It does not validate native tracing overhead or signer shutdown deadlines.

Run with an already built observer:

```bash
python3 observability/tests/smoke-trace-retention.py --browser \
  --binary target/debug/eez-observer \
  --report /tmp/eez-trace-retention.json
```

CI requires this fixture with a ten-minute bound and preserves sanitized failure
reports/screenshots. The three workflow contract and all 30 observability Python
tests pass. The browser uses the disposable installation described in the
[README](../README.md#browser-validation).
