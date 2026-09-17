# Private trace pilot

The optional `traces` Compose profile adds Tempo 3.0.3 and a dedicated Alloy trace
collector. Application export remains separately opt-in in the
[native tracing configuration](../contracts/tracing.md). The existing file-log
collector continues independently. This profile uses a single Tempo process and
local persistent storage; it requires no Kafka or external hosted service.

Add these fields to the private stack configuration used by `prepare.py`:

```json
{
  "traces_enabled": true,
  "tempo_port": 19320,
  "otlp_http_port": 14318
}
```

This is a fragment, not a complete configuration. Ports default to the values
shown and always bind to host loopback. Zero requests an ephemeral port for local
tests. Port options without `traces_enabled: true` are rejected. The config
preparer adds backend scrapes, trace-health alerts, a Tempo Grafana datasource,
and links between trace IDs and JSON logs. It writes the bounded collector config
to the private runtime directory. It does not start any service.

Use the same explicit Compose project and runtime env file as the main stack,
adding `--profile traces` to its normal command:

```bash
docker compose --project-name "$OBS_PROJECT" \
  --env-file "$OBS_RUNTIME/compose.env" -f observability/compose.yaml \
  --profile traces up -d
```

For same-host applications, configure their native trace receiver as
`127.0.0.1:14318` (or the chosen OTLP port) with an explicit pilot sampling ratio.
For another node host, use a private authenticated forwarding path to this
loopback listener. Do not expose the unauthenticated OTLP or Tempo query ports
publicly. The trace receiver accepts OTLP/HTTP only; the application exporter uses
`/v1/traces`. Native process identities come from the same generated configuration
as Prometheus labels, and native JSON span fields contain optional `trace_id`.

## Navigation and missing traces

Grafana provisions `eez-tempo` and a Loki derived field that extracts exactly
32 lowercase hex characters from a JSON `trace_id`. IDs remain fields, never
Loki labels. Tempo's trace-to-log link selects the configured environment and
deployment, then filters the trace ID within a one-minute-expanded span window.
The normal diagnostic log collector must have read access to those JSON files.

The link explicitly says that a trace is optional and may be unavailable. Missing,
sampled-out or expired traces do not retract receipt costs or lifecycle history.
Tempo returns an explicit not-found response for an unavailable trace. Use the
stable admission/attempt identities and PostgreSQL records for canonical history;
raw lifecycle transport chunks are not a replacement for JSON diagnostic logs.

## Retention, limits and alerts

Tempo persists WAL, blocks and scheduler state in `tempo-data`. Both its scheduler
and worker receive seven-day block retention (`168h`); compaction and polling can
delay physical deletion. This is a retention policy, not a filesystem quota.
Provision and monitor a separate storage budget using the main host/storage
runbook. Recreating the container preserves its named volume.

Initial pilot container budgets are 1 GiB/one CPU for Tempo and 256 MiB/half a CPU
for Alloy traces. These are tested small-fixture limits, not a measured production
capacity claim. Tempo ingestion is limited to 1 MiB/s with a 4 MiB burst, 10,000
live traces and 2 MiB per trace. Span-metric/service-graph generation is disabled;
canonical accounting never comes from sampled trace aggregates.

Alloy accepts requests up to 4 MiB, uses a 128 MiB memory limit with 32 MiB spike
headroom, and queues at most 2,048 spans with one exporter consumer. Admission
rejects queue overflow without waiting. Each HTTP export has a one-second timeout;
retries last at most ten seconds. The queue is in memory: process loss or a longer
outage can lose traces, with explicit refusal/failure counters. Alloy's persistent
volume is not a durable trace queue.

`EEZBackendUnavailable_tempo` and `EEZBackendUnavailable_alloy-traces` detect
missing backend scrapes, including absent targets. `EEZTraceCollectionLoss`
reports recent collector refusal/export failure while the collector is reachable.
These counters can overlap or include retry failures, so they are diagnostic
signals rather than a unique count of lost spans. Native processes with configured
tracing get `EEZNativeTraceLoss_<service>-<instance>` for their bounded exporter
losses. Healthy endpoint scrapes alone never establish successful trace delivery.
Loss alerts use a five-minute window and the normal two-minute alert hold.

## Disable, upgrade and recover

To disable the backend, stop only `alloy-traces` and `tempo` in the explicit
project, remove trace port fields and set `traces_enabled: false`, then rerun
preparation and the main stack. Datasource pruning removes optional trace links;
leave named volumes intact for rollback/recovery. Disable application export or
redirect it to another authorized private receiver before stopping collection.

Image tags and digests are pinned with the main stack. Back up the runtime config
and Tempo named volume consistently before upgrading. Do not run an older binary
against an incompatible upgraded storage format; restore the prior snapshot into
a separate volume. No clean-host restore or seven-day production baseline is
claimed by the disposable fixture test.

Run the independent backend acceptance test with:

```bash
python3 observability/tests/smoke-traces.py \
  --report /tmp/eez-observability-traces.json
```

It allocates a unique project and ephemeral loopback ports, tests ingestion,
query/log links, bounded collector outages/loss and recovery, and container
replacement, then removes only its owned project resources. Native runtime
neutrality, real workload sampling/overhead and clean-host recovery remain
separate gates.

Implementation references: [Tempo monolithic deployment](https://grafana.com/docs/tempo/latest/set-up-for-tracing/setup-tempo/deploy/locally/linux/),
[Tempo configuration](https://grafana.com/docs/tempo/latest/configuration/), and
[Alloy bounded OTLP exporter](https://grafana.com/docs/alloy/latest/reference/components/otelcol/otelcol.exporter.otlphttp/).
