# Release, restart and policy annotations

Every provisioned dashboard includes a Grafana annotation query for release
records scoped by **all three** tags: `eez-release`, `environment:<environment>`
and `deployment:<deployment>`. Use the dashboard's actual single-value filters.
Changing filters must never show another deployment's releases.

Create a release record only after the deployment operator has verified the
running image digest against the tested runtime artifact manifest and completed
the deployment's health checks. Include the full EEZ source commit, protocol
gitlink, image digest, affected services and deployment result in the annotation
text. Use the observed deployment time, not a build or configuration-generation
time. A rollback gets its own record identifying the restored digest. Keep keys,
authenticated URLs, private hosts and other credentials out of annotation text.

Write an organization annotation through Grafana's
[annotations API](https://grafana.com/docs/grafana/latest/developer-resources/api-reference/http-api/api-legacy/annotations/)
(`POST /api/annotations`) using an operator identity authorized for annotations.
The request contains `time` in UTC milliseconds, `text`, and the three tags above;
omit `dashboardUID`/`panelId` so the same release appears on all scoped dashboards.
Use the existing private Grafana access path and protected authentication, then
query `GET /api/annotations` with repeated `tags` parameters and the deployment
time window to verify the stored record. Neither running `prepare.py` nor process
startup writes a release event. Package version `0.1` and upstream Reth's build
revision do not identify an EEZ release.

This is an operator-observed deployment record, not an automatic attestation of
the running binary. Keep the corresponding deployment manifest and CI evidence.
Grafana stores these records in its own database; preserve `grafana-data` with
the configuration in the [restore procedure](restore.md).

When the node-agent configuration includes an actual Composer `reth_json`
source, the generator also enables **Composer startup and active policy**
annotations. Their bounded Loki log query selects the two reviewed startup
events emitted after the active settings are resolved. It does not infer a hot
reload or release change. The records retain constrained setting values and
instance identity. An unavailable or unconfigured diagnostic source supplies no
markers; absence is not evidence that policy stayed unchanged. Loki retention
limits this history, and queries return at most 100 markers in the selected
interval. Narrow the interval when inspecting frequent restarts. See the
[node-agent contract](node-agent.md) and Grafana's
[Loki annotation behavior](https://grafana.com/docs/grafana/latest/datasources/loki/annotations/).

The protocol dashboards separately retain lifecycle epoch/restart and local
policy disposition annotations from PostgreSQL. Those events describe observed
process/request history, not verified release provenance.
