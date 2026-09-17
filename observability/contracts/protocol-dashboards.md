# Protocol dashboards

The database profile provisions four additional read-only Grafana dashboards,
using `eez-postgres`, `eez-prometheus` and optional `eez-tempo` alongside the two
stage-1 dashboards. `prepare.py` copies the supported set into the private runtime
configuration. Disabling the database profile removes these four generated files;
it does not delete database history or operator-authored dashboard files.

| Dashboard / UID | Actual producers and populations |
| --- | --- |
| Cross-chain / `eez-cross-chain` | Native direction/state queue snapshots and monotonic current ages; currently anchored canonical outcomes, admission-to-reconciliation and admission-to-observed-finality durations with coverage; retained admission statuses, readiness observations, request timelines, lifecycle gaps and trace references |
| Settlement / `eez-settlement` | Native preparation histograms; retained preparation/attempt/repricing/dispatch/reorg observations; exact canonical paid posting calls; separately verified applied endpoints, including historical same-height prefixes |
| Blobs / `eez-blobs` | Canonical decoded type-3 posting receipts; authenticated codec-v0 lengths, weighted purchased utilization, missing-measurement states and coverage, latest retrieval duration; separately labelled composer-reported allowance measurements |
| Gas / `eez-gas` | Configured wallet spend, paid posting calls, daily chain-separated charges, source allocation estimates, exact covered settled-transaction unit costs, unallocated overhead, scoped balances and runway |

Environment/deployment selectors use the restricted
`eez.dashboard_chain_context` reader view. Chain and rollup selectors constrain SQL
populations where that identity exists. Native queue/preparation metrics are
process-scoped and use the instance selector; they cannot gain chain attribution
from a scrape target after collection. Inbound unattempted history has source L1
identity but no inferred target-rollup assignment. Stage-1 current queue rules
default to RPC and allow the explicit validated `queue_source: "native"` switch;
these dashboards never add native and RPC queue counts or silently choose a source.

Time filters use the pinned Grafana SQL macro implementation's zero-argument
`$__timeFrom()`/`$__timeTo()` boundaries around complete timestamp expressions.
Nested expressions inside `$__timeFilter(...)` are not supported correctly by
that parser. Receipt panels use canonical block timestamps; admission cohorts use
native admission observation UTC; currently anchored outcome/latency panels use
the checked L1 scan-watermark timestamp. Each population is labelled. Coverage
panels deliberately describe the full indexed interval, independent of the selected
dashboard time window.

Canonical outcome queries use the deduplicated, retractable reader views. Native
terminal counters are not a canonical success ledger. Latency percentiles preserve
outcome distinctions and display their completed population count; current pending
age distributions sit beside them. Native current values require a successful
scrape, valid complete sample and sample timestamp between five seconds in the
future and 45 seconds old. Empty/unknown ages remain unavailable. No source is
filled with `or vector(0)`, and stale SQL chains do not produce a fabricated zero
population. An empty queue without completion evidence is idle/unexercised, not
a continuously verified service.

All monetary integer fields are selected as text, preserving exact decimal values
through the Grafana PostgreSQL driver. L1 and L2 costs remain separate. Included
reverts count as spend. Posting calls, applied settlements and native candidate
attempts are separate populations. Schema 14 adds receipt-deduplicated companion
charges and `equal_share_v1` shared-overhead estimates over verified unique native
source membership. The allocation coverage panel retains unallocated reasons per
rollup context; its context rows can overlap and must not be added across rollups.
The [allocation contract](cost-allocation.md) defines the exact integer remainder
rule and the distinction between receipt charges and per-source estimates.
The per-call mean is suppressed when any receipt charge is incomplete.

Blob utilization is the ratio of summed measured bytes to purchased usable capacity
for exactly those measured posts, including measured reverted posts. No measurement
or zero capacity yields NULL. Its denominator covers decoded posting calls; nearby
posting-classification coverage exposes unknown/invalid calls. A false
`has_applied_settlement` means no current verified range evidence, not proof that
the post was unapplied. Composer-reported allowance data is never mixed with
canonical authenticated byte measurements.

The request table enumerates actual retained admissions and links their source
identity to the request timeline, related attempts and canonical receipt detail.
Hash-filtered timelines reuse validated `lifecycle_source_references` and explicit
process/epoch/generation or preparation IDs to include related dispatch, repricing,
submission and preparation-finish events. Projection gaps can make the timeline
incomplete; membership is never inferred from transaction ordering. Source evidence
outside the displayed time window can link an event inside it. Optional
`explorer_urls` in stack configuration accepts `l1`/`l2` HTTP(S) base URLs without
credentials, query strings or fragments; absent entries have no fabricated URL.
For example, `"explorer_urls":{"l1":"https://explorer.example"}` constructs
`/tx/0x…` links for L1 receipt rows only. URLs are escaped as SQL literals in the
generated dashboard, and profile/link reconfiguration removes obsolete links.
Only retained records containing a valid trace ID appear in the trace-reference
table. Tempo links are added only with the trace profile enabled; sampling,
export failure or expiry can still make a trace unavailable without affecting
canonical receipt or lifecycle history.

Process-start/restart annotations come from actual epoch headers, not an age
timeout. Explicit local-policy dispositions have separate annotations; local
policy and unresolved-restart states remain visible in admission history. Scoped
release records use Grafana's annotation store and require an operator-verified
deployment record, never an inferred package version. Configured Composer JSON
log sources add actual startup/active-policy markers through Loki. These identify
resolved startup settings, not a hot-reload claim. See the
[annotation procedure](../runbooks/annotations.md) for scope, provenance and retention.

Settlement panels now show actual simulation/witness/prover-client/signer/submission
histograms, finite planning cuts, exact-slot misses and active immutable policy.
Current policy values use instant queries and freshness/up gates; historical
stage rates retain their observation-window semantics. Cross-chain panels show
independent follower agreement only when comparison is known; unknown reasons
remain explicit. Follower lag is measured even when comparison is unknown.
Agreement/lag use the observer producer's 90-second freshness window, while
unknown reasons remain visible through retry backoff and explicit disabled state.
These observer gauges are scoped to observer instance, not the native instance selector.

The [finality timing](finality-timing.md), [unit-cost](cost-allocation.md) and
[funding](funding.md) contracts specify the new producers' exact populations and
unknown states. Their combined datasource tests do not establish native mixed
workload acceptance, archival availability or deployment capacity. Browser
navigation is covered separately by the required Chromium fixture. A disabled/unavailable capability is never shown as healthy.

Validation uses `tests/smoke-protocol-dashboards.py`: pinned real Grafana,
PostgreSQL and Prometheus services in an owned isolated network; actual generated
panel, variable and annotation queries; exact integer/ratio/population assertions;
and stale-source/failed-scrape checks. Synthetic coherence and blob metadata seeds
exercise query semantics, not native ingestion, KZG authentication or protocol
replay. The full database/native suites retain those separate obligations.


The protocol dashboard fixture's `--browser` mode tests Grafana's real variable
interpolation and request-table links, including selected-source rows on each
destination. Chain/rollup custom All values include SQL quoting; native-instance
All uses Grafana's normal escaped list/regular-expression expansion. Native
instances come from retained lifecycle sources plus configured metric targets;
an empty catalog displays unavailable without inventing an active instance.

The packaged PostgreSQL datasource allows one open and one idle connection to
bound concurrent planning memory. Multiple panels queue for that reader; the
10-second SQL statement limit remains in force. Grafana's 512 MiB container cap
includes a 384 MiB soft Go-runtime memory target. These tested fixture limits are
starting budgets, not a production concurrency or capacity guarantee. Measure
representative dashboard users and observer writes together during the baseline.


SQL string fields in tables explicitly use the String unit, including exact
base-unit amounts, full-width identifiers and numeric-only hashes/epochs; they
must not inherit the default short/SI formatter. Tempo links use the frontend
TraceQL mode, which routes hexadecimal trace IDs to the backend lookup. The
retention browser test verifies the real unavailable result and confirms that
the durable SQL row stays accessible after trace deletion.


## Quiet activity and retained success

The overview maps a known zero active count to Idle / unexercised. Its coverage
text explicitly limits passive reachability/queue observations. The cross-chain
dashboard places per-instance/direction activity beside the last retained
node-reconciled committed admission's checked L1 timestamp. The latter covers
full retained history, applies the SQL instance/chain/rollup selectors and never
infers a never-used or healthy state from absence. Native instant samples use the
selected time range end; historical periods outside metric retention are Unknown.
Default relative ranges end at now. Stale native samples remain absent while
independently valid SQL history can remain visible. Neither panel represents a
continuous funded canary or independent replay verification.
