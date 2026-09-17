# Native operational metrics

Native metrics use the existing process recorder and the configured EEZ histogram
buckets. They are enabled by the optional telemetry configuration's `metrics`
section, independently of lifecycle capture and writer health. No configuration
file preserves disabled behavior: no queue sampler, extra admission clocks or
preparation timer. Applications add only the closed labels below; Prometheus
supplies deployment/service/instance labels. Reth prefixes these names with `reth_`.

| Name | Type and intrinsic labels | Producer and meaning |
| --- | --- | --- |
| `eez_native_admissions_total` | Counter; `direction=inbound/outbound` | Successful new acceptance, including replacement/readmission; duplicates and internal recovery do not increment it. |
| `eez_native_terminal_observations_total` | Counter; `direction`, existing closed `outcome` taxonomy | Actual native terminal transitions; never authoritative canonical progress. Reopening after reorg can produce another observation. |
| `eez_request_duration_seconds` | Histogram; `direction`, `outcome` | Monotonic admission-to-terminal duration for observed native transitions with a retained admission clock. Separate from current pending-age gauges. |
| `eez_native_terminal_duration_unknown_total` | Counter; `direction`, `outcome` | Terminal observation without a retained local clock; counted but excluded from the duration histogram. |
| `eez_native_terminal_capture_gaps_total` | Counter; `reason=capture_limit/capture_context` | Whole terminal capture unavailable; do not interpret missing observations as zero outcomes. |
| `eez_stage_duration_seconds` | Histogram; `stage=preparation`, `outcome=prepared/construction_failed/capacity_limited/prover_failed/validation_rejected/cancelled/panicked` | Existing post-batch preparation future, from its first poll until return or destruction. One observation per operation; no event for an unpolled future. |
| `eez_native_queue_sample_valid` | Gauge, no labels | 1 when the complete active population fits the 4,096-source capture bound. Above it, 0 and all population values are NaN. This is a telemetry bound, not an admission limit. |
| `eez_native_queue_sample_timestamp_seconds` | Gauge, no labels | Last sample's UTC time; downstream consumers must gate on scrape availability and a recent sample (45 seconds for the 15-second sampler). |
| `eez_native_queue_evaluation_valid` | Gauge, no labels | 1 with a complete sample and a known monotonic readiness evaluation no older than 60 seconds. Reorg invalidation clears evaluation. |
| `eez_native_queue_evaluation_age_seconds` | Gauge, no labels | Monotonic age of the last evaluation; NaN before evaluation or after reorg invalidation. |
| `eez_native_queue_transactions` | Gauge; `direction=inbound/outbound`, `state=queued_ready/queued_blocked/queued_unknown/in_flight` | Disjoint active reservation populations. Stale queued readiness becomes unknown. In-flight is membership, not completion or service success. |
| `eez_native_queue_age_seconds_bucket` | **Gauge**; `direction`, `kind=pending/serviceable`, fixed `le` | Cumulative bucket counts of the current population's monotonic ages. Query directly, **never with `rate()`**; this is not a completed-request histogram. |
| `eez_native_queue_age_count` | Gauge; `direction`, `kind` | Current sources with a known age in the corresponding distribution. |
| `eez_native_queue_age_unknown_transactions` | Gauge; `direction`, `kind` | Included active sources whose local admission/serviceable clock is unavailable. They are not fabricated as age zero. |
| `eez_native_queue_age_sum_seconds` | Gauge; `direction`, `kind` | Sum over the known-age current population only. |
| `eez_native_queue_oldest_age_seconds` | Gauge; `direction`, `kind` | Maximum known current age; NaN for an empty/unknown population. |

The age bounds are 0.1, 1, 5, 10, 30, 60, 120, 300, 600, 1,800,
3,600, 21,600 and 86,400 seconds, plus infinity. Snapshot values can decrease when
sources leave the population. Pending includes every queued or in-flight source;
serviceable includes currently ready queued sources and in-flight sources with a
retained readiness clock. In-flight sources without that clock contribute to the
serviceable unknown count. If evaluation is missing/stale, the entire serviceable
distribution is NaN, rather than zero. Readiness records from legacy policy feed
these native observations without changing the existing queue RPC's fields.

Admission starts a monotonic clock only after successful acceptance. Duplicate
submission retains it; replacement/readmission creates a new clock; internal
requeue keeps it. Ready time survives claiming/recovery and successive accepted
ready observations. Explicit blocking removes that source's ready clock. A reorg
clears all readiness; active pending age survives. Terminal/removed reservations
leave current distributions, even when historical ledger metadata remains.
Restart starts with the actual in-memory pool and never recreates prior pending
requests from telemetry. Unresolved restart history remains the observer's job.

The node's existing task executor owns one 15-second sampler, skips missed ticks
and cancels it with the node. Under the existing pool lock it aggregates at most
4,096 active sources into fixed-size arrays. It performs no encoding, recorder
calls or I/O there; publication follows release. No per-source task or unbounded
metric label is introduced. Capture failure does not mutate queue ownership.

Preparation uses the existing future's lifetime and typed error classification.
Its stage histogram works when lifecycle capture is disabled or lost, and does
not claim signer-only latency, source completion latency, validated canonical
progress or successful settlement. Repeated attempts are separate preparations.
Stage timing includes telemetry capture inside that preparation wrapper; crash
loss still leaves an absent duration, not a fabricated cancellation.

The stage-1 RPC recording rules remain their existing authoritative producer;
these metrics do not silently replace or add to them. Native counter/histogram
observations cannot retract reorganized outcomes. Canonical progress and costs
remain observer-owned. A reviewed recording-rule switch and mixed native canary
comparison remain required before protocol dashboards consume this family.

Coverage: `held_pool::status::native::tests` checks disabled operation, direction
partitions, pending versus serviceable time, claiming/recovery, removal/readmission,
replacement/duplicates, legacy RPC compatibility, missing clocks, stale/reorg
invalidation, capture bounds and preparation without a lifecycle writer.
`eez_telemetry::operational::tests` checks age boundaries, fixed histogram output,
one finish on cancellation/panic/return, and unavailable values replacing stale
samples. The existing bidirectional lifecycle process canary now also configures an isolated
private metrics endpoint and requires one admission, one committed terminal and
one completed-duration sample per direction, plus empty current pending/in-flight
populations after sampling. It retains every prior semantic/proof/lifecycle check.
The integrated source-based run passed both default-policy and enabled-policy
canaries; see [validation](../validation/native-metrics.md). Full mixed-workload
and overhead gates remain open.
Additional finite stage timers and the process/overhead gates remain subsequent
R20 integration; this contract does not mark the whole requirement complete.

Terminal observations retain all nine outcomes: committed, source_reverted,
source_completed, source_completed_with_missing_effects, source_nonce_spent,
attempts_exhausted, nonce_gap, pool_discarded and replaced. The native ledger's
actual before/after partition selects reconciled transitions. The same local
reservation-removal check selects local dispositions for both lifecycle capture
and metrics; rejected, duplicate and unresolved-owner removals emit no terminal
observation. Replacement records the old admission and counts a new acceptance.
These checks work without lifecycle export, and metrics publish after release of
both pool and ledger locks. They do not upgrade native outcome claims into
observer-verified evidence. Histogram rates describe native transition
observations in the query window, not unique settled users or reorg-adjusted
success rates. Always show unknown-duration/capture-gap coverage beside them.
