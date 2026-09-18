# Grafana through the existing HTTPS gateway

The current test stack is available at **https://eez.asuscomm.com/grafana/**.
It now monitors the real fresh Chiado deployment (L1 10200 / L2 6291), with the
existing Grafana login. Anonymous access and signup remain disabled. The password
is in the private deployment runtime, not this repository. The simulated demo
was stopped with its volumes retained. See the [deployment evidence and limits](../../docs/audits/2026-09-18-chiado-observability-deployment/README.md).

Set the stack input's `grafana_public_url` to
`https://eez.asuscomm.com/grafana/`, then re-run `prepare.py` against the same
runtime. This preserves passwords and generates the prefix for Grafana's root
URL, health check, metrics scrape, dashboard links and runbooks. HTTPS also
enables secure session cookies. Existing root-URL configurations remain valid.

Keep manually supplied service inputs in a separate private Compose environment
file or your shell: `prepare.py` owns `runtime/compose.env`. The active deployment
uses `service.env` for its observer image/mount inputs and
`GRAFANA_GATEWAY_NETWORK=eez-chiado-public_default`.

Add [compose.gateway.yaml](../compose.gateway.yaml) to the existing monitoring
Compose command. It joins only Grafana to the gateway's existing network with
the alias `eez-observability-grafana`; PostgreSQL, Prometheus, Loki and the
observer remain on their private networks. Recreate only Grafana to apply its
environment/network settings, and reload Prometheus for the new metrics path.
Keep the existing data volumes.

Insert [grafana.caddy](../gateway/grafana.caddy) inside the existing domain's site
block. Forward `/grafana/*` unchanged: Grafana serves that prefix itself. Caddy's
reverse proxy handles WebSocket upgrades. Validate the complete candidate before
reloading the existing gateway; preserve its other routes, ports and certificate
volumes. No new public port or host firewall rule is required.

The live change used a configuration reload, not a gateway replacement. Private
rollback copies are retained under `target/observability-public-gateway/`.
The active monitoring `compose.sh` includes the overlay and separate environment
file. Its host-local operating notes are in
`.runtime/chiado-observability-20260918/README.md` (main checkout). Stopping that
monitoring project makes the Grafana route unavailable; the stopped simulated
project no longer owns the gateway alias.

Validation checked public DNS/TLS access to the login page, correctly resolved
CSS/image assets, health, and rejection of anonymous dashboard API requests.
The existing UI and monitor stayed HTTP 200. At the initial gateway-only
validation, both RPC paths returned HTTP 502 due to the old upstream condition.
The later fresh deployment restored both public RPC paths and verified chain IDs
10200/6291. Automated public credential submission was blocked by
automatic approval review, so a successful authenticated public browser session
is not claimed. The later deployment verified the preserved password and actual
dashboard queries through loopback. Subpath and network configuration regressions
passed separately.

Grafana's [reverse-proxy guide](https://grafana.com/tutorials/run-grafana-behind-a-proxy/)
documents `root_url` and `serve_from_sub_path` for this arrangement.
