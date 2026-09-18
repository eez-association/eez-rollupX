# Grafana through the existing HTTPS gateway

The current test stack is available at **https://eez.asuscomm.com/grafana/**.
It still uses simulated chain data and the existing Grafana login. Anonymous
access and account signup remain disabled; the password is in the private demo
runtime, not in this repository.

Set the stack input's `grafana_public_url` to
`https://eez.asuscomm.com/grafana/`, then re-run `prepare.py` against the same
runtime. This preserves passwords and generates the prefix for Grafana's root
URL, health check, metrics scrape, dashboard links and runbooks. HTTPS also
enables secure session cookies. Existing root-URL configurations remain valid.

Keep manually supplied service inputs in a separate private Compose environment
file or your shell: `prepare.py` owns `runtime/compose.env`. The retained demo
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
The demo's `compose.sh` includes the overlay and separate environment file, so
its documented stop/start commands retain this wiring. Stopping the demo makes
the Grafana route unavailable while leaving other gateway routes intact.

Validation checked public DNS/TLS access to the login page, correctly resolved
CSS/image assets, health, and rejection of anonymous dashboard API requests.
The existing UI and monitor stayed HTTP 200. Both RPC paths returned HTTP 502
before and after this change; this routing task did not repair that existing
upstream condition. Automated public credential submission was blocked by
automatic approval review, so a successful authenticated public browser session
is not claimed. Subpath and network configuration regressions passed separately.

Grafana's [reverse-proxy guide](https://grafana.com/tutorials/run-grafana-behind-a-proxy/)
documents `root_url` and `serve_from_sub_path` for this arrangement.
