# Protocol dashboards

The database profile provisions four additional read-only Grafana dashboards,
using `eez-postgres`, `eez-prometheus` and optional `eez-tempo` alongside the two
stage-1 dashboards. `prepare.py` copies the supported set into the private runtime
configuration. Disabling the database profile removes these four generated files;
it does not delete database history or operator-authored dashboard files.

| Dashboard / UID | Actual producers and populations |
| --- | --- |
| Cross-chain / `eez-cross-chain` | Native direction/state queue snapshots and monotonic current ages; currently anchored canonical outcomes and admission-to-reconciliation durations; retained admission statuses, readiness observations, request timelines, lifecycle gaps and trace references |
| Settlement / `eez-settlement` | Native preparation histograms; retained preparation/attempt/repricing/dispatch/reorg observations; exact canonical paid posting calls; separately verified applied endpoints, including historical same-height prefixes |
| Blobs / `eez-blobs` | Canonical decoded type-3 posting receipts; authenticated codec-v0 lengths, weighted purchased utilization, missing-measurement states and coverage, latest retrieval duration; separately labelled composer-reported allowance measurements |
| Gas / `eez-gas` | Configured wallet spend, paid posting calls, daily chain-separated selected receipt charges, exact receipt detail and explicitly unallocated posting overhead |

Environment/deployment selectors use the restricted
`eez.dashboard_chain_context` reader view. Chain and rollup selectors constrain SQL
populations where that identity exists. Native queue/preparation metrics are
process-scoped and use the instance selector; they cannot gain chain attribution
from a scrape target after collection. Inbound unattempted history has source L1
identity but no inferred target-rollup assignment. Native stage-1 RPC recording
rules retain their producer: these dashboards do not add native and RPC queue
counts together or silently switch sources.

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
attempts are separate populations. Companion/source allocation remains unavailable:
all known posting overhead is explicitly unallocated, not divided across members.
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
policy and unresolved-restart states remain visible in admission history. Releases
and active policy-change annotations have no native producer yet.

Remaining producer gaps are displayed in the dashboards: separate simulation,
witness and submission timings; slot misses and admission cuts; active policy;
accepted-to-finality durations; verified companion allocation and per-source/shared
costs; settled-L2-transaction unit costs; balances/runway; independent follower and
historical recovery capability. A disabled/unavailable capability is not shown as
healthy. This dashboard slice does not close those implementation requirements.

Validation uses `tests/smoke-protocol-dashboards.py`: pinned real Grafana,
PostgreSQL and Prometheus services in an owned isolated network; actual generated
panel, variable and annotation queries; exact integer/ratio/population assertions;
and stale-source/failed-scrape checks. Synthetic coherence and blob metadata seeds
exercise query semantics, not native ingestion, KZG authentication or protocol
replay. The full database/native suites retain those separate obligations.
