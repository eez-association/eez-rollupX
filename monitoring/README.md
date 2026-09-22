# Composer queue and settlement monitoring

This optional stack runs Grafana and Prometheus on the same Linux machine as the
packaged Chiado Composer. Prometheus scrapes Reth's existing localhost metrics
endpoint and retains the Composer-owned metric families below. Grafana
provisions **EEZ Composer queue** in the EEZ folder. The optional indexer adds
**EEZ Settlement history**; the older **EEZ Settlement** dashboard is retired.
For persistent settlement history, exact transaction links and receipt fees, add
the optional [EEZ indexer](indexer/README.md). It reads L1 directly and adds
PostgreSQL plus an **EEZ Settlement history** dashboard without changing Composer.
Its optional L2/Beacon collector adds the latest posting's L2 ranges, blob hashes,
retrieval status and explorer links; missing details do not block L1 indexing.

Grafana, Prometheus, and the Composer metrics endpoint all bind to localhost.
The monitoring services use host networking solely to reach that private Reth
endpoint without exposing it on another interface. Prometheus is provisioned as
Grafana's default datasource.

## Start

From this directory:

```bash
cp .env.monitoring.example .env
umask 077
chmod 700 secrets
openssl rand -out secrets/grafana-admin-password -base64 32
chmod 644 secrets/grafana-admin-password
docker compose config --quiet
docker compose up --detach
docker compose ps
```

Open Grafana at `http://127.0.0.1:19300`. The default username is `admin`; its
password is the contents of `secrets/grafana-admin-password`. Prometheus is at
`http://127.0.0.1:19090`.

The packaged `docker-compose.chiado-node.yml` enables Reth metrics at
`http://127.0.0.1:19091/metrics`. If the Composer is not running from that
definition, Prometheus will correctly report its only target as down. Queue
metrics require a Composer build containing this change: an older build can be
scraped successfully while the queue panels show no data.

Change the local ports, retention, or admin username in `.env` before starting.
The `.env` file and password are ignored by Git. The password file must be
readable by Grafana's unprivileged container user; its private parent directory
prevents other host users from traversing to it.

## Operate

Open the queue dashboard at `http://127.0.0.1:19300/d/eez-composer-queue`.
It shows queued and in-flight requests over time, with separate lines for inbound
(L1 source) and outbound (L2 source), plus last-evaluated readiness, blocking
reasons, oldest tracked pending age, last evaluation age, and last canonical
cross-chain completion age. Grafana refreshes every five seconds; zoom into a short time
range to inspect second-scale changes.

### Metric contract

| Exposed metric | Labels | Meaning |
| --- | --- | --- |
| `reth_eez_composer_queue_requests` | `rollup_id`, `direction=inbound\|outbound`, `state=queued\|in_flight` | Current held-pool membership: four gauges per rollup, including zero for empty states. |
| `reth_eez_composer_queue_readiness_requests` | `rollup_id`, `status=ready\|blocked\|unknown` | Queued requests classified against the last evaluated source state, summed across directions. |
| `reth_eez_composer_queue_blocked_requests` | `rollup_id`, `reason=nonce\|fee\|balance\|escrow` | Blocked queued requests, including affected nonce successors, summed across directions. |
| `reth_eez_composer_queue_oldest_pending_timestamp_seconds` | `rollup_id` | Earliest retained admission time among queued and in-flight requests; `NaN` if absent. |
| `reth_eez_composer_queue_evaluation_timestamp_seconds` | `rollup_id` | Start time of the last source-state evaluation; `NaN` before evaluation or after source-state invalidation. |
| `reth_eez_composer_queue_last_completion_timestamp_seconds` | `rollup_id` | Local observation time of the latest retained canonical cross-chain completion; `NaN` after restart or an invalidating reorg. |
| `reth_eez_composer_queue_last_completion_l1_block` | `rollup_id` | L1 state checked when confirming that completion; may be later than its transaction inclusion block. `NaN` when no completion is retained. |
| `reth_eez_composer_queue_sample_timestamp_seconds` | `rollup_id` | Unix seconds at the last successful queue sample. This is not a readiness evaluation or settlement timestamp. |

Reth's existing recorder adds the `reth_` prefix. The sampler registers after
node launch and runs only when the existing `--metrics` endpoint is enabled;
there is no new CLI or environment setting. It samples immediately, then every
second, using one pool lock for membership and existing status observations. The scan
is linear in queued plus in-flight membership, reads no transaction payloads,
and runs on a blocking worker. Sampling tasks never overlap; Reth's executor
cancels the loop on shutdown. An already running blocking sample may finish.
No admission or execution path updates these gauges. Metrics and the existing
queue RPC use the same snapshot calculation, preserving its classification rules.

Within each snapshot, `ready + blocked + unknown = queued`, and the blocking
reason counts sum to `blocked`. In-flight work is not classified as queued.
The dashboard explicitly labels readiness and blocking reasons as **last-evaluated**:
the sampler does not reevaluate source state, and a new sample does not advance
evaluation time. The age cards are omitted from the queue dashboard; these cached
classifications must not be treated as freshly evaluated. The last evaluation may be old during idle periods;
its age alone is not a stall alert. Same-height source-hash changes expire old
classifications according to the existing queue rules.

Oldest pending age uses retained admission timestamps, not a new timer or a
latency histogram. Replacement records a new admission; recovery preserves a
retained timestamp. Empty membership or absent history exports `NaN`, and the
age graph shows a gap. If only some members retain admission history, the metric
reports the oldest *tracked* admission. Restart discards these observations.

`queued` includes ready, blocked and not-yet-evaluated requests. `in_flight`
matches the queue RPC's `inFlight`: outstanding held-pool reservations, including
requests awaiting settlement evidence. It is not active CPU work, a success
counter, or a count of all historical unsettled requests. The two categories
are disjoint in the pool snapshot. This is Composer's cross-chain held pool,
not Reth's ordinary L2 transaction pool.

The Cross-chain queue chart combines four separate series: inbound/outbound
queued and inbound/outbound in-flight. Direction determines color; in-flight
uses dashed lines. Readiness remains separate because ready + blocked + unknown
is a breakdown of queued membership, not additional requests, and its current
metrics aggregate both directions. The dashboard does not infer a directional
readiness breakdown that the metrics do not provide.

The top summary shows outstanding Total/Inbound/Outbound requests, Queued/In-flight
states, and the queued Ready/Blocked/Awaiting evaluation breakdown. These are
instant snapshots at the selected end time (now for live ranges), not totals
across the time range. The same scrape-health and freshness guards apply; missing
samples remain unavailable. The views overlap intentionally: readiness divides
queued membership, and direction divides all outstanding membership.

The top row also includes Oldest age metrics with minimum, maximum and average
for the selected period. The full-width Oldest tracked pending request chart
labels its series Oldest request age and keeps the legend on the right. Prometheus evaluates the guarded age expression at
two-second intervals before calculating these statistics, independently of chart
resolution. Empty-queue, stale and unavailable observations do not count as zero.
The average is the mean of sampled oldest ages while requests are present, not
mean transaction latency. Peaks between samples can still be missed; a range
with no valid observations shows `-`; null and NaN display values across the
queue dashboard also use `-`, never zero. The age graph's Last value
includes a missing final sample instead of carrying forward an old age.

The Composer job is scraped every three seconds with a three-second timeout.
The former two-second deadline caused occasional timeouts on the shared host,
despite normally fast responses. The longer interval provides timeout headroom
without overlapping scrapes; it trades some collection resolution for resilience.
The in-process sampler still updates once per second. Short-lived transitions may be missed;
these gauges do not measure throughput or latency. Individual gauge writes
are not an atomic Prometheus scrape, so a scrape can straddle sample updates.
Queue state is in memory and may reset on restart; a decrease does not prove
settlement. No transaction hashes, addresses or request IDs are metric labels.

The queue panels omit values if the scrape is down, the sample timestamp is more
than three seconds in the future, or its age is at least 10 seconds. The bounded
three-second tolerance matches the scrape timeout and avoids hiding healthy samples
whose exported timestamp is slightly ahead of the query timestamp. Missing
telemetry remains a gap, not zero; lines are not joined across missing samples.
Legends sit on the right, use concise direction/status labels, and show **Last**
(including an empty last sample), rather than **Last *** (last nonempty sample),
alongside Max. A sampling failure stops the sampler
with a warning and leaves its last timestamp unchanged. Fresh queue data alone
does not establish chain progress or successful settlement.

The scrape allowlist intentionally excludes native Reth metrics: embedded L1
and L2 share that recorder and their chain metrics cannot be reliably attributed.
Prometheus's own `up` and scrape diagnostics remain available. Filtering happens
after fetching the full Reth response, so it does not remove endpoint-rendering
cost. Scrape diagnostics remain available in Prometheus; the operational dashboards
omit scrape status, sample age, and scrape duration panels. The sampler skips
missed ticks rather than accumulating a backlog.
Grafana's datasource minimum interval is one second; global rule evaluation stays
at 30 seconds because this stack has no alert or recording rules.

### Settlement: when and why

Use **EEZ Composer queue** for posting timing, outcomes and trigger reasons.
For canonical settlement details, enable the [indexer](indexer/README.md) and open
`http://127.0.0.1:19300/d/eez-settlement-history`.
The policy's existing evaluation supplies the due reasons and conditional timing;
the existing attempt ledger supplies submission observations. No RPC calls,
re-evaluation of policy, transaction copying, or new lifecycle tracker are used
to collect this data. Queue, policy and ledger locks are not held together across
the sampling steps. These are adjacent observations, not a transactional snapshot
of the entire settlement pipeline. The sample timestamp is captured before these
reads, so lock delays cannot make an older snapshot appear newly collected.

All settlement metric names below start with `reth_eez_composer_settlement_` and
carry `rollup_id`. The sampler uses the same one-second loop and queue sample
timestamp; there is no new service or configuration option.

| Suffix | Meaning |
| --- | --- |
| `due` | Last evaluated policy: 1 = due, 0 = waiting for a trigger, `NaN` = unknown. |
| `trigger{reason=...}` | Seven independent 0/1 reasons: `general`, `pure_always`, `pure_interval`, `cross_chain`, `blob_fullness`, `drain`, `window_limit`. Multiple reasons can be active. |
| `anchor_timestamp_seconds` | Canonical L1 timestamp used by the policy clock. Before the first commitment advance, this can be initial registration. |
| `anchor_l2_block` | Validated L2 commitment used by the last policy evaluation. |
| `anchor_l1_block` | L1 block containing the commitment used by the last policy evaluation. |
| `unsettled_blocks` | Distance from anchor to evaluated candidate, including the candidate Sync block. |
| `block_threshold` | Configured general trigger in unsettled L2 blocks; not a time interval or packing limit. |
| `pure_interval_deadline_timestamp_seconds` | Anchor time plus the configured pure-L2 interval, only when pending ordinary activity exists in interval mode. Otherwise `NaN`. |
| `target_timestamp_seconds` | Selected L1 target supplied to the evaluation; not a guaranteed dispatch/inclusion time. |
| `evaluation_timestamp_seconds` | Local time of the successful policy evaluation, distinct from sample time. |
| `submission{state=...}` | One active state for the lowest unresolved attempt: `none`, `prepared`, `dispatched`, `included`, `excluded`, `uncertain`. |

Policy fields become unknown when a new evaluation starts and remain unknown
if it fails or is cancelled. An incomplete canonical scan, changed generation,
or changed validated endpoint also hides the previous evaluation until a new
successful one is available. This deliberately favors gaps over stale decisions,
including during forward canonical publication as well as reorgs. Sampling does
not advance policy evaluation time. Submission observations remain independently
available when policy evaluation is unknown.

The exported interval deadline is **conditional**: other triggers
can fire earlier, and proving, pricing, target availability and reconciliation can
delay dispatch. It is absent when no ordinary L2 activity is pending. The
general consolidation rule is measured in blocks, so this dashboard does not
convert it into an assumed wall-clock deadline. Read
[settlement policy](../docs/configurable-settlement-policy.md) for the rules.

`dispatched` is recorded immediately before the network request and does not
prove relay acceptance. `included` is a receipt observation and does not prove
all cross-chain effects settled. `none` means no unresolved attempt, not success.
The separate cross-chain completion metric advances only through the held pool's
existing canonical reconciliation observations, and reorg invalidation clears it.
It includes any reconciled attempt and is not a count of this Composer's posts.
Its timestamp is local observation time, whereas the policy anchor uses L1 block
time and advances only when the canonical commitment changes.

The main settlement view shows time since settlement/completion, active triggers,
pending blocks, and submission outcomes and durations. Internal policy status,
evaluation age, evaluated target, and the conditional interval countdown are
available as metrics but omitted from the dashboard. It does not classify
every preflight/prover/fee-cap failure or reconstruct every attempt's history; those
details remain in existing structured Composer logs. Short-lived states may occur
between samples. Neither an old completion time during inactivity nor a due
decision alone is a failure alert.

The two latest-event cards show elapsed time, an absolute timestamp, and an L1
block link. Settlement uses the anchor's L1 timestamp/block. Completion uses local
confirmation time and the L1 state checked during reconciliation, which can be
later than transaction inclusion. Neither card substitutes the latest submitted
transaction for the event's transaction. Transaction hashes are not exported as
Prometheus labels, so these cards provide block links rather than transaction
links. Set the dashboard's **L1 explorer URL** for another network (default: Chiado).
The block trigger is shown as percentage progress; other triggers can settle
before it reaches 100%.

### Checks

Check service health:

```bash
docker compose ps
curl --fail --silent --show-error http://127.0.0.1:19090/-/ready
curl --fail --silent --show-error http://127.0.0.1:19300/api/health
curl --fail --silent --show-error http://127.0.0.1:19090/api/v1/targets
curl --fail --silent --show-error http://127.0.0.1:19091/metrics \
  | rg '^reth_eez_composer_queue_'
```

Stop the services while retaining their data:

```bash
docker compose down
```

The named `prometheus-data` and `grafana-data` volumes survive ordinary Compose
restarts and `docker compose down`. The dashboard and datasource are reproducible
from the checked-in provisioning files. Historical metrics live in the Prometheus
volume; they are diagnostic data, not a source for queue recovery or accounting.
Deleting that volume loses history. Production backup/restore automation remains
outside this small monitoring setup.

## Submission events, timing, and Sync counts

The settlement dashboard combines sampled current state with event counters. An
unresolved attempt can go directly from dispatched to none between samples; the
submission-event counters still record its included/excluded/uncertain outcome.
They are recorded when the submitter returns, even when canonical reconciliation
has already retired the attempt. They reset on process restart and are not a
persistent event audit trail. Reorgs do not erase historical submission observations.

| Metric suffix (prefix `reth_eez_composer_`) | Meaning |
| --- | --- |
| `submission_events_total{event}` | Prepared tasks, dispatch callbacks, included receipts, exclusions, uncertain results, or preflight failures. These count attempts, not users. |
| `submission_event_timestamp_seconds{event}` | Latest local observation time; zero before that event has been observed. |
| `submission_exclusions_total{reason}` | Bounded reasons: excluded, slot_skipped, submission_window_missed, target_changed. Excluded cannot distinguish auction loss from builder/relay rejection. |
| `submission_last_duration_seconds{stage}` | Last monotonic prepared-to-dispatch, dispatch-to-included/excluded/uncertain, or prepared-to-preflight-failure duration. |
| `submission_duration_seconds` | Distribution and cumulative sum/count for those same durations, exported using Reth's recorder. |
| `submission_target_l1_block`, `submission_target_timestamp_seconds`, `submission_included_l1_block` | Latest actual dispatch target and latest observed receipt block. These may refer to different attempts; they are not paired records. |
| `submission_sync_l2_block`, `submission_sync_requests{direction}` | Endpoint height and inbound/outbound user counts of the latest prepared Sync submission attempt. |
| `submission_sync_requests_per_attempt` | Per-direction distribution and cumulative sum/count of user requests selected per prepared submission attempt, including zero-user anchor-only attempts. |
| `queue_completion_observations_total{direction}` | Canonical user-effect completion observations, deduplicated within retained attempt history. Invalidated completions observed again after a reorg count again; repeated sampling does not. |

All series have `rollup_id`. Block heights and transaction hashes are never metric
labels. The existing metrics flag/recorder controls export; no new service, timer,
configuration option, or Reth patch is required. The Composer uses the already
locked metrics crate for event emission; sampling still runs once per second.

Prepared means the background submission task has started **after** proving. Its
wait and final pricing are included in prepared-to-dispatch; earlier composition
and proving are not. Included timestamps measure when a canonical receipt was
observed, not the containing L1 block's timestamp. Undispatched failures have no
invented dispatch duration. Preflight failure is an unsent local failure and is
shown separately from submitter exclusions. Receipt inclusion does not guarantee
that all selected users completed; use the separate canonical-completion counts.

Sync counts refer to original signed user requests selected for an attempt, not
internal calls, generated system transactions, or guaranteed canonical successes.
Retries can count the same request in another attempt. Idle Sync blocks without
submission do not update the last-attempt chart; a historical anchor-only attempt
can refer to an earlier empty block. The structured event
`eez.composer.submission.sync_requests` records each exact `sync_height`, `inbound`,
and `outbound` count for block-by-block inspection. The last-value chart can miss
multiple attempts within a sample interval; the aggregate distribution retains
those observations. This avoids unbounded per-block Prometheus series or a new
block-indexing service.

## Upgrade

Deploy Composer and proof signer from the same dependency lockfile for this
increment. The proof RPC validation identity includes the entire `Cargo.lock`;
adding the Composer metrics dependency therefore changes the identity even though
signer execution logic is unchanged. A new Composer with the previous signer
fails startup with `prover validation identity differs from local execution rules`.
Upgrade both images together; do not bypass the compatibility check. No genesis,
contract, or chain database reset is required.

Monitoring image tags and digests are pinned in `compose.yaml`. Change one monitoring image at a time,
review its release notes, render the Compose configuration, and verify health
before removing the previous image from the host.

After deploying a Composer build with queue metrics through the normal node
release procedure, run `docker compose up --detach --force-recreate` here to
apply monitoring configuration and dashboard mounts. Expect 18 queue series and
22 settlement-policy series plus the submission-event families
for the configured rollup within a few scrape intervals; absent observation
timestamps are represented as `NaN`.
The dashboard is provisioned from Git; edit its JSON to make durable changes.
Rolling back Composer leaves the dashboard with no queue data, rather than
fabricating an empty queue. Monitoring changes do not require a network reset.

## Scope boundary

This stack covers queue membership, policy observations, recorded submission
outcomes/timings, per-attempt Sync counts and canonical user-completion observations.
End-to-end user latency distributions, fees, signer or
follower metrics, OpenTelemetry, general-purpose log collection, public access and alerts remain
separate increments. No changes to Reth or embedded-L1 instrumentation are needed.

### Per-posting lifecycle history

The queue dashboard also has a **Current unresolved submission**
status card using fresh, healthy Composer samples. **Settlement trigger reasons**
counts captured reasons per posting attempt; the **Posting submissions** table
uses the same standalone indexer history, recording
short-lived transitions that sampled gauges can miss. Enable the
[posting log collector](indexer/README.md#posting-lifecycle-history-optional)
to populate it; no new Prometheus labels or Composer database client are needed.
