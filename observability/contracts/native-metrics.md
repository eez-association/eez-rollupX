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
| `eez_stage_duration_seconds` | Histogram; closed `stage`, `outcome` enumerations below | Existing operation or worker lifetime. One observation per operation; no event for an unpolled future. |
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

The stage-1 current queue recording rules default to RPC. The explicit stack
`queue_source: "native"` switch requires exactly one configured Composer target;
it replaces all three `eez:queue_*:current` rules together. Native sample validity,
evaluation validity, exporter `up`, maximum 45-second sample age and 5-second future
clock tolerance gate the current values. A failed/missing/stale native producer
never falls back to RPC and never adds RPC counts. Source changes also change the
series job/instance and add direction; select the Composer instance in dashboards.
Chain/RPC dependency health continues to describe the observer. The generated
manifest records the source choice, and returning to `rpc` regenerates the prior
rules. Before an operator switches, compare the independently scraped native and
RPC total/state counts side by side during the same stable workload, allowing
collection-time differences; RPC age is wall time while native age is monotonic.
Record rollout validation separately: synthetic rule tests prove selection/failure
semantics, not live deployment equivalence. Native counter/histogram observations
cannot retract reorganized outcomes; canonical progress and costs remain observer-owned.

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
The mixed-workload and process/overhead gates remain open; this contract does not
mark the whole requirement complete.

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

## Stages and active policy

`preparation` retains its existing outcomes: prepared, construction_failed,
capacity_limited, prover_failed, validation_rejected, cancelled and panicked.
`simulation` measures one complete-group source/target execution; `witness`
measures actual blocking witness capture (including store reads/replay), and both
use success/failed/cancelled/panicked. Witness timing lives inside the worker,
so cancelling its await does not fabricate an early worker completion. Mock
proving without witnesses does not produce a zero-duration witness observation.
`submission` starts immediately before the existing send call and includes its
relay/inclusion observation; outcomes are included/excluded/failed/cancelled/panicked.
Preflight failures before dispatch remain preparation observations, not a sent bundle.

`prover_client` covers one remote call, connection, streaming, signature checking
and local request-binding verification. Typed retry status distinguishes unavailable,
deadline and aborted; actionable rejection is rejected, opaque/backend failure
is failed. Retries each have one client duration, without charging another native
settlement attempt. `signer` covers the server RPC; signer_ingestion,
signer_validation, signer_settlement and signer_signing describe its inner stages.
Their status taxonomy is success/invalid/rejected/unavailable/deadline/cancelled/internal,
plus panicked on unwind. Execution and settlement timers remain owned by the one
blocking worker after RPC timeout; the existing permit and cancellation ownership
are unchanged. Signing can succeed locally while the overall RPC subsequently
fails its deadline. These overlapping stages must not be summed into latency.
`eez_signer_requests_total{outcome}` counts server RPC observations once, with all
fixed outcomes initialized to zero at metrics-enabled signer startup. Neither an
invalid request nor a successful signature proves canonical settlement.

`eez_admission_cuts_total{reason}` records gas, da, count, preparation_deadline or
queue_cleanup at the actual finite decision, including final encoded-candidate gas/DA
rejection sites. Counts describe planning cuts, not
unique rejected users. Policy count cuts require exhausted selection capacity and
an unblocked remaining candidate; fee-blocked-only backlog does not count. Legacy
numeric selection retains its existing finite-snapshot meaning.
`eez_target_slot_misses_total{reason}` distinguishes scheduled_late, excluded,
skipped, submission_deadline and target_changed. A scheduled-late slot and a
submitted-bundle miss are different observations, not deduplicated user failures.

`eez_settlement_policy_enabled` and
`eez_settlement_policy_info{pure_l2_mode=disabled/always/interval,admission=fixed/automatic}`
report immutable validated startup configuration. `eez_settlement_policy_setting`
has finite `setting` values: pure_l2_interval_seconds, max_unsettled_blocks,
blob_fullness_bps, operator_blob_cap, user_count_cap, operator_gas_cap, proof_seconds,
submission_slack_seconds and l1_slot_seconds. NaN means inactive/unbounded. These
are approximate nonfinancial gauges; exact fee/reserve values do not use floats.

Policy evaluations expose `eez_settlement_policy_evaluation_valid` and
`eez_settlement_policy_evaluation_timestamp_seconds`, plus
`eez_settlement_unsettled_blocks`, `eez_settlement_anchor_age_seconds`,
`eez_settlement_pending_pure_l2_transactions`, `eez_settlement_due` and
`eez_settlement_blob_allowance`. Their only intrinsic label is `rollup`, drawn
from the finite configured runtime set. Each evaluation first invalidates the old
sample; a successful canonical evaluation publishes after releasing the existing
policy lock. Consumers require validity, recent evaluation and a healthy target.
Anchor age is the candidate target timestamp minus the last canonical settlement's
L1 timestamp, exactly as the scheduling policy uses it. It is not oldest pending
transaction age. Pure-L2 population counts included ordinary transactions,
including reverted ones. No runtime inclusion-grace setting is invented.

Lifecycle drop counters initialize all fixed reasons before the writer starts:
capture_limit, capture_context, clock, encoding, queue_full, writer_closed, shutdown,
io. A failure before the first successful Prometheus scrape can still be missed by
`increase`; current writer health and coverage remain separate evidence.

## Authorized signer response outcomes

`eez_signer_authorized_requests_total{outcome}` is separate from general RPC
outcomes. Its closed values are `success`, `internal`, `deadline`, `panicked`
and `cancelled`, initialized to zero only when signer metrics are enabled. The
observation starts after the complete validation-and-settlement pipeline has
returned `AttestationMaterial`, including final observation replay. Earlier
stream admission, execution-only success, rejected proposals, detached workers
that never return material to the RPC, and slot contention do not enter it.

Success means construction of a locally authorized response within the existing
request deadline; it does not guarantee client transport delivery, L1 submission
or canonical settlement. The outcome covers both attestation deadline checks,
signing errors and unwind. Dropping an unfinished guard records cancellation;
a panic records `panicked`. Every completed guard records once. No request,
transaction, address or caller-controlled text appears in labels, and disabled
metrics produce no observations. Request permit ownership and worker cancellation
are unchanged. The critical alert excludes caller cancellation and requires
repeated known-authorized failures with no local authorized success in the same
window; source availability and upstream health remain separate gates.
