# Monitoring backend validation

Date: 2026-09-16. Branch: `feat/network-observability`, following exporter commit
`0e518b2`. This implements the WP2 backend foundation and validates it against
isolated fixtures. It is not a live Chiado deployment or completion of all
stage-1 operational gates.

The [captured report](backend-stack.json) identifies the disposable project,
image digests and completed checks. The real Rust exporter ran against test RPCs
inside that project's Docker network. Every notification went to the test
receiver. The project's containers and volumes were removed afterward; existing
Chiado services and host firewall rules were not changed.

| Implemented capability | Validation |
| --- | --- |
| Private Compose backend with resource limits and persistent volumes | Configuration parsed; pinned Prometheus, Grafana, Loki, Alloy and blackbox images started |
| Prometheus recording rules | `promtool check config` and two regression scenarios passed, including stale readiness and failed scrapes with prior samples still present |
| Network and infrastructure dashboards | Both provisioned through Grafana API; every Prometheus panel expression executed successfully against fixture data; no missing-data-to-zero fallback |
| File-based logs | Approved JSONL file flowed through Alloy to Loki and remained queryable after backend container replacement |
| Optional probes and host metrics | Fixture RPC/TCP probes succeeded; optional host target scraped successfully; no Docker socket required |
| Alert routing | Stale queue, failed queue RPC and stopped exporter each produced a firing notification and a subsequent resolved notification |
| Heartbeat | Separate fixture receiver received repeated callbacks and detected their absence after all monitoring-backend services stopped |
| Persistent state | Replaced Prometheus, Grafana, Loki and Alloy containers with named volumes preserved; dashboards, historical metric samples and logs survived |
| Preparation | Four unit tests cover secret preservation, strict config validation, label ownership, alert no-data/error behavior and dashboard contracts |
| CI compatibility | Rust 1.94 formatting, Clippy with warnings denied, and all nine exporter tests passed; the dedicated workflow runs checks on relevant pull requests |

The integration test deliberately uses shortened notification/evaluation timings.
Normal deployment settings scrape/record every 15 seconds, evaluate Grafana
alerts every 30 seconds, require two minutes of failure, and repeat heartbeats
every minute. The fixture's detector uses three short test intervals; a real
receiver must implement its own three-minute expiry and independent notification
path on another host. A same-host fixture cannot prove whole-host outage delivery.

Issues found and corrected during validation:

- RPC recordings now require a successful exporter scrape as well as fresh
  source data. Prometheus lookback must not keep failed exporters looking healthy.
- Alloy runs as its image's `alloy` user, matching the state directory ownership
  while retaining dropped capabilities and a read-only log mount.
- Grafana query models contain literal `$A` references; unlike typed provisioning
  values, their raw JSON must not receive an extra dollar escape. Webhook values
  retain the separately tested generation escape rules.
- Test fixtures moved into the disposable Docker network after the host firewall
  blocked bridge-to-host traffic. No firewall exception was introduced.
- Dynamic test port mappings are rediscovered after container replacement.
- Loki's WAL is explicitly stored in its persistent volume.

The selected image versions were checked against the projects' public release
metadata and locked by digest in [images.lock.json](../images.lock.json).
Configuration was checked against official [Grafana provisioning](https://grafana.com/docs/grafana/latest/administration/provisioning/),
[Grafana alert provisioning](https://grafana.com/docs/grafana/latest/alerting/set-up/provision-alerting-resources/file-provisioning/),
[Prometheus configuration](https://prometheus.io/docs/prometheus/latest/configuration/configuration/),
[Loki filesystem/TSDB examples](https://grafana.com/docs/loki/latest/configure/examples/configuration-examples/),
and [Alloy run flags](https://grafana.com/docs/alloy/latest/reference/cli/run/).
Grafana's normal 30-second alert interval respects its documented ten-second
[scheduler interval](https://grafana.com/docs/grafana/latest/setup-grafana/configure-grafana/#min_interval).

Still outstanding: native embedded-client metric capture/attribution and signer
instrumentation; lifecycle schema/writer and canonical accounting; real endpoint
and approved JSON-log wiring; actual operator/heartbeat receiver configuration;
independent probe placement; clean-host backup restoration; and workload/overhead
calibration. Restart/replacement persistence does not establish clean-host
recoverability. The CI workflow is added, but has not yet run on GitHub.
