# Observability operations

This stack is private operational tooling. The first dashboards show the RPC
exporter's aggregate observations, scrape health and explicitly configured log
files/probes. They do not establish canonical completion or finality.

**Alert ownership and missing data.** Assign an operator for the webhook route
and a separate owner/host for the heartbeat receiver before live use. Grafana
owns alert evaluation and delivery; Prometheus owns recording rules. Normal
alerts evaluate every 30 seconds and require two minutes of failure. Grafana's
scheduler uses ten-second ticks, so rule intervals are multiples of ten.
Prometheus scrapes/records every 15 seconds. A healthy exporter response does
not prove fresh queue evaluation. The rule inputs separately test RPC success,
source-state age and the Prometheus scrape result.

`EEZObserverUnavailable` is the parent failure for an unreachable exporter.
Source-collection alerts explicitly suppress no-data when that parent is down,
but raise query errors; while the exporter is reachable, a missing required
source metric is a failure. Stale readiness has its own alert. No panel fills
absent data with zero. A blank error table alone is not a health signal.

For an observer/source alert, check the exporter process, private scrape path,
expected chain/genesis configuration, and its bounded error reason. Fix a bad
endpoint or stale node instead of weakening expected identity. For an L1 head
age alert, compare the optional reference and consensus node; do not infer local
failure merely because an upstream network paused. Signer TCP probes establish
listener availability only. A public RPC probe checks a read-only JSON-RPC
response; a probe running on this backend is not an independent external probe.

`EEZNativeTelemetryUnavailable_<service>-<instance>` is provisioned for each
configured native metrics target, with explicit alerting on missing data and
query errors. Check that `EEZ_TELEMETRY_CONFIG` points to the generated file,
its service identity matches the executable, and its effective listener is
reachable from Prometheus. An explicit Reth `--metrics` overrides the file's
listen address; keep the scrape target consistent with that override. See the
[startup contract](../contracts/native-startup.md). An available metrics endpoint
does not prove a healthy lifecycle spool or successful protocol execution.

**Heartbeat.** Grafana's `EEZHeartbeat` sends a firing webhook every minute to
the independently hosted receiver. That receiver must track the timestamp of
actual received heartbeats and notify through its own delivery path after three
missed minutes. Merely accepting the webhook is insufficient. This repo tests
the callback and a missing-heartbeat detector in an independent test process;
it does not provision the receiver host or choose real on-call recipients.
A same-host receiver cannot detect whole-host failure reliably.

**Logs and storage.** Alloy tails only `*.jsonl` in the configured dedicated
read-only directory. It drops the `filename` label to avoid per-rotation indexed
labels; JSON fields remain queryable from the line. Keep logs from arbitrary
sources/secrets out of that directory. Give the container's `alloy` user read
access to the approved files via filesystem permissions or a matching group;
do not grant a Docker socket. The current profile collects on the backend host;
for a separate node host, run Alloy there with a private Loki endpoint. Do not
assume a remote path is available on the monitoring host.

Prometheus retains at most 30 days / 2 GB of metric blocks, with WAL/head
headroom required. Loki uses single-instance TSDB plus filesystem storage and
14-day compactor retention. Retention is not a filesystem quota: provision a
separate bounded disk/volume, monitor its capacity and allow compaction delay.
Alloy positions, Loki data, Grafana SQLite/configuration state, and Prometheus
TSDB are persistent volumes. RAM/CPU and local console-log bounds are in Compose.
Configured host metrics enable a 10 GiB disk warning and a six-hour growth trend
projected over 24 hours; tune mountpoint exclusions/floors for the host.

**Restart and restore.** Recreating containers preserves named volumes. A
normal stop uses `docker compose ... down` without `--volumes`. Restore the
runtime config and admin-password file together with Grafana's data and the
monitoring volumes. For consistent filesystem backups, stop this Compose
project first or use storage snapshots with the backend's documented consistency
requirements. Do not delete node data or reset the chain to recover monitoring.
The [restore drill](restore.md) additionally verifies SQL/configuration and cold
volume restoration into a fresh isolated project. Its measured fixture timings
do not establish production recovery objectives; test off-host transfer and the
actual deployment's dataset and keys before relying on a recovery target.

Changing `secrets/grafana-admin-password` does not rotate an existing Grafana
user's password: the environment bootstrap secret applies when its database is
initialized. Use Grafana's supported administrator workflow for later rotation.
Protect config backups, which contain webhook credentials and the bootstrap
password. Provisioned dashboard edits come from Git; UI edits are disabled.

**Upgrade/rollback.** Image tags and content digests are locked in
`images.lock.json` and Compose. Update them together, run config/rule tests and
the isolated smoke stack, then back up and recreate only this monitoring project.
Do not roll back a database-bearing backend to an older version against an
incompatible upgraded data volume; restore the prior backup to a separate
volume first. Exporter collection can continue independently of Grafana/Loki.
