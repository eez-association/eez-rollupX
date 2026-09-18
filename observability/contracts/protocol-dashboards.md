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

Monetary detail tables multiply stored wei amounts by exact numeric
`0.000000001` and select decimal gwei as text, preserving precision through the
Grafana PostgreSQL driver. Existing allocation/denominator floors apply in wei
before this display conversion; gas quantities remain gas units. L1 and L2 costs remain separate. Included
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


## Trend-first layout

The dashboards retain all existing detailed tables and place these new trend
panels above them. Existing readiness, stage-duration, planning-cut and target-miss
charts also move into the overview. Legends identify the population, direction,
chain or typed outcome; shared tooltips compare observations without summing them.

| Dashboard | New time-series panels (IDs) |
| --- | --- |
| Cross-chain | Admission/terminal/committed observation rates by instance/direction (24); currently confirmed outcomes by admission-time cohort (25); inbound queue states (26); native terminal latency p50/p95/p99 (27); observed-finality latency by admission-time cohort (28); outbound queue states (29); oldest active/serviceable age (30) |
| Gas | Configured-wallet spend (18); paid-posting execution/blob/total fees by success (19); selected-receipt execution/blob/total fees (20); configured L1 poster balance history (21) |
| Blobs | Weighted utilization and measurement coverage (10); authenticated bytes and measured purchased capacity (11); execution/blob/total fees (12); known purchased blobs and measured subset (13) |
| Settlement | Included paid-call count by success (15); receipt execution gas (16) |
| Network | L2 head/safe/finalized settlement heights (24); L1 head/finalized timestamp ages (25); verified safe/explorer lag (26) |

SQL trends use five-minute buckets, apply the existing SQL selectors and selected
range before aggregation, and leave unobserved buckets absent. Counts, bytes, gas
and charges use bars; sparse percentile/ratio cohorts use points without connecting
lines, so missing observations do not become interpolated history. Prometheus
charts use actual range queries, reset-aware native counters/histograms and the
existing freshness gates for current-state gauges. A first scrape cannot establish
a counter rate; missing retention or disabled capabilities remain unavailable.

The confirmed-outcome chart groups **actual admission timestamps**, then shows
which retained admissions have a currently canonical, node-reconciled outcome.
It is retractable when evidence changes and is not a completion-rate counter.
It does not bucket history by the moving scan watermark. Finality trends also
use admission cohorts and retain the existing clock/finality exclusion rules.
Native terminal latency is explicitly a different assurance population. Existing
cohort counts, coverage, exact identifiers and evidence details remain below.

Monetary charts sum the existing receipt population in PostgreSQL numeric
arithmetic, require complete accounting throughout each displayed bucket, divide
by 10^9 and only then convert to plotting numbers. These **gwei plots are
approximate visualizations**, not replacements for the exact decimal gwei tables.
Storage and protocol accounting remain in integer wei. One gwei is 10^-9 of the
chain's native currency (xDAI on Chiado); no ETH/fiat conversion is performed.
Chains remain separate; included reverts count as spend. Wallet, selected-receipt,
posting and blob-posting charts overlap and must never be added together. Execution
and blob fees are disjoint components within the same population. Total series
equal execution plus blobs and are never stacked or summed with their components.
Gas panel 22 also shows execution/blob/total fees for configured L1 wallets over
the selected range, including reverts but excluding transferred value and L2 fees.
Missing accounting leaves these totals unknown. Blob
utilization remains the ratio of summed authenticated measured bytes to summed
purchased usable capacity for those same measured posts; its measured-subset
count is not added to the all-post count.

Focused generated/static coverage is in `tests/test_dashboard_trends.py`:
twenty-two time-series panels, range-query flags, scoped SQL/cohort clocks, complete-accounting
gates, preserved exact detail tables, sparse displays, unique IDs and non-overlapping
layout. The real datasource/browser smoke gate remains necessary for SQL execution,
PromQL evaluation and rendering; static checks alone do not claim those results.


The network overview's Instance catalog includes the observer job and configured
`eez-composer-*` jobs within the selected environment/deployment. Its default
built-in All therefore expands to both real instance labels: native queue
recordings retain the Composer instance, while chain/indexer recordings retain
the observer instance. Selecting native queue mode no longer hides its series
behind an observer-only catalog. This changes no recording-rule source selection
and introduces no RPC/native fallback or double counting; RPC remains the default.


The poster-balance history panel uses retained Prometheus observations of the
configured L1 poster, gated by funding validity and observer reachability. Stored
native-unit gauges are multiplied by 10^9 for display in gwei, without currency
conversion. The current balance panel uses the same display unit. Funding increases and
spending decreases are visible; history before collection or outside retention is
not reconstructed. Like the current configured-account balance, this panel follows
environment/deployment scope independently of native-instance/chain/rollup selectors.


## Attribution-table evaluation boundary

Gas tables 6 and 9 materialize the exact selected L1/L2 context IDs, then materialize
the existing `posting_allocation_status` or `source_posting_allocations` projection
for those pairs once. Receipt-time, source-hash, health and presentation filters
remain outside that boundary. This prevents the flattened outer joins from
re-evaluating shared claim evidence for each paid post. It does not cache results
between requests or copy the allocation policy into the dashboard.

The underlying views still inspect all applicable complete/unchecked declarations,
positive orphan evidence, conflicts and the complete source population before
calculating equal shares. No early time/source/producer filter or LIMIT removes a
competing claim or changes the denominator. Environment/deployment/chain/rollup
selection, unknown states, gwei arithmetic, sorting and limits are unchanged. The
reader keeps its 10-second statement timeout; no migration or runtime update is
required.


## Cross-chain operational overview

The overview now orders information around operational questions, preserving the
existing detailed evidence tables and their filters below:

| Question | Panels | Interpretation |
| --- | --- | --- |
| Is work accumulating, and in which direction? | Inbound 26 and outbound 29 | Ready, blocked, unknown readiness and in-flight reservations are disjoint stacked populations; missing/stale samples are unknown |
| Is something stuck while completed latency looks normal? | Oldest active/serviceable age 30 beside arrivals 24 | Active age is a gauge, not a rate or a completed latency; empty/missing clocks are not zero |
| Where is processing time spent? | Native terminal duration 27, readiness partitions 22, observed finality 28 and coverage 23 | Native duration includes queue time; serviceable includes in-flight processing; quantiles are not additive and sparse samples are not a reliable tail estimate |
| How much observed crosschain traffic enters each block? | Source transactions/block 31 | Unique explicit scoped source transaction identities with canonical receipts, not admissions, internal protocol calls or completed effects |
| How much source gas does that traffic consume? | Source gas/block 32 | Sum of receipt gas_used for the same source-block population; any missing gas leaves the block total NULL |
| How large are paid posting populations? | Verified source members/post 33 | Full verified unique source population per paid post/context, including reverted attempts; known empty is 0, incomplete/contradictory evidence is NULL |

Native snapshots remain 15 seconds; short-lived queue spikes can occur between
samples. All monetary charts elsewhere remain in gwei. Gas quantities here are
gas units, distinct from fees and from destination execution attribution.

Source-block charts materialize distinct `(chain_instance_id, transaction_hash,
direction)` from current linked explicit source references before joining receipts.
Repeated admissions, attempts and later recovery references therefore do not
multiply a transaction. Reference emission time is not a source receipt-time
filter. Chart categories retain context IDs and show the last 50 nonempty canonical
source blocks in deterministic chronological order, within the selected range.
Missing/unindexed history is not reconstructed as empty blocks. Source hash and
native instance restrict this observed subset. L1 sources do not imply a target
rollup; existing source-chain/rollup scope rules are retained.

Gas coverage tests the actual `gas_used` field, not fee-price completeness as a
proxy. The current receipt schema can store NULL gas alongside a missing-price
accounting gap; those historical values cannot be recovered by a dashboard query.
Known zero-price gas still counts. Unknown-gas transaction count is tooltip
coverage and is hidden from the gas axis.

Paid-post membership uses the existing materialized allocation projection and
keeps all authoritative competing claims in scope. It follows deployment/chain/
rollup selection independently of native-instance selection. A source/post hash
selects whole populations after validation; it does not reduce their denominator.
Each of the last 50 paid posts retains its own transaction index within the block,
rollup/context and unknown/reverted label. Known pure-L2/empty populations remain
zero; a missing or conflicting membership never becomes zero or completion.

`tests/block_dashboard_queries.py` runs actual shipped SQL through the reader
under the existing 10-second deadline, with rollback-only source/receipt/claim
fixtures for duplication, same-block sources/posts, unknown gas, zero-price gas,
selectors, freshness, conflicts, reorgs and deterministic last 50 boundaries. The
Grafana smoke additionally exercises each plotted queue state and oldest age
through active, idle, incomplete and stale snapshots.


## Pressure and disposition diagnostics

Cross-chain panel 24 now compares three reset-aware per-second native counters:
admissions, all terminal observations and the committed subset. The exporter-up
gate applies before aggregation. These are process observations, not unique
canonical transactions: replacements and reorg reopening prevent using their
difference as an exact queue size or drain-time estimate. The committed line is
part of the terminal line, not another quantity to add. Assess a sustained flow
imbalance together with actual queue size and oldest active age.

Panel 34 splits active pending ages into <=30s, (30s,120s], >120s and unknown clocks.
The first three bands use direct cumulative gauge buckets, never rate(), and
subtract matching per-instance buckets with `ignoring(le)` before aggregation.
Negative differences are unavailable rather than clamped to zero. Missing buckets
remain missing. Unknown clocks are a separate gauge population even when there
are no known-age observations. Pending ages retain the complete-sample/up/[-5,45]
second freshness gates and intentionally do not depend on readiness evaluation.
These are convenient diagnostic bands, not an agreed SLO or enabled alert.

Panel 35 keeps each noncommitted terminal disposition separate. Reverted sources,
exhausted attempts, nonce gaps, pool discards, replacements and already-completed
sources have different meanings; they are not grouped into a fabricated failure
rate. Missing series and failed scrapes remain absent. Neither diagnostic changes
native processing, SQL assurance, monetary units, sampling cadence or alert state.

The real Grafana/Prometheus fixture checks admission:terminal:committed ratios
5:4:2 at one evaluation time, typed nonce-gap/replacement series, direct age-band
boundaries 30/120/121, unknown clocks, idle zeroes and stale/incomplete suppression.
