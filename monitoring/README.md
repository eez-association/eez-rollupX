# Composer queue monitoring

This optional stack runs Grafana and Prometheus on the same Linux machine as the
packaged Chiado Composer. Prometheus scrapes Reth's existing localhost metrics
endpoint and retains the six Composer-owned queue metric families below. Grafana
provisions the **EEZ / EEZ Composer queue** dashboard. There is no additional
exporter, database observer, tracing pipeline or alerting service.

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
reasons, oldest tracked pending age, separate sample/evaluation ages, and endpoint
scrape duration. Grafana refreshes every five seconds; zoom into a short time
range to inspect second-scale changes.

### Metric contract

| Exposed metric | Labels | Meaning |
| --- | --- | --- |
| `reth_eez_composer_queue_requests` | `rollup_id`, `direction=inbound\|outbound`, `state=queued\|in_flight` | Current held-pool membership: four gauges per rollup, including zero for empty states. |
| `reth_eez_composer_queue_readiness_requests` | `rollup_id`, `status=ready\|blocked\|unknown` | Queued requests classified against the last evaluated source state, summed across directions. |
| `reth_eez_composer_queue_blocked_requests` | `rollup_id`, `reason=nonce\|fee\|balance\|escrow` | Blocked queued requests, including affected nonce successors, summed across directions. |
| `reth_eez_composer_queue_oldest_pending_timestamp_seconds` | `rollup_id` | Earliest retained admission time among queued and in-flight requests; `NaN` if absent. |
| `reth_eez_composer_queue_evaluation_timestamp_seconds` | `rollup_id` | Start time of the last source-state evaluation; `NaN` before evaluation or after source-state invalidation. |
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
evaluation time. Consult **Last evaluation age** before treating those cached
classifications as recent. The last evaluation may be old during idle periods;
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

The Composer job is scraped every second with a one-second timeout. Short-lived transitions may be missed;
these gauges do not measure throughput or latency. Individual gauge writes
are not an atomic Prometheus scrape, so a scrape can straddle sample updates.
Queue state is in memory and may reset on restart; a decrease does not prove
settlement. No transaction hashes, addresses or request IDs are metric labels.

The queue panels omit values if the scrape is down, the sample timestamp is in
the future, or its age is at least 10 seconds. Missing telemetry produces a gap,
not zero. Sample age is shown separately; a sampling failure stops the sampler
with a warning and leaves its last timestamp unchanged. Fresh queue data alone
does not establish chain progress or successful settlement.

The scrape allowlist intentionally excludes native Reth metrics: embedded L1
and L2 share that recorder and their chain metrics cannot be reliably attributed.
Prometheus's own `up` and scrape diagnostics remain available. Filtering happens
after fetching the full Reth response, so it does not remove endpoint-rendering
cost. The **Metrics endpoint scrape duration** panel exposes elapsed scrape time;
it should stay comfortably below the one-second timeout. This is not a CPU
benchmark. The sampler skips missed ticks rather than accumulating a backlog.
Grafana's datasource minimum interval is one second; global rule evaluation stays
at 30 seconds because this stack has no alert or recording rules.

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

## Upgrade

Image tags and digests are pinned in `compose.yaml`. Change one image at a time,
review its release notes, render the Compose configuration, and verify health
before removing the previous image from the host.

After deploying a Composer build with queue metrics through the normal node
release procedure, run `docker compose up --detach --force-recreate` here to
apply monitoring configuration and dashboard mounts. Expect 14 queue series
for the configured rollup within a few scrape intervals; absent observation
timestamps are represented as `NaN`.
The dashboard is provisioned from Git; edit its JSON to make durable changes.
Rolling back Composer leaves the dashboard with no queue data, rather than
fabricating an empty queue. Monitoring changes do not require a network reset.

## Scope boundary

This change covers queue membership and existing status observations. Latency
distributions, settlement counters, fees, signer or
follower metrics, OpenTelemetry, log collection, public access and alerts remain
separate increments. No changes to Reth or embedded-L1 instrumentation are needed.
