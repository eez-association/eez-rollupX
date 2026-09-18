# Single-machine Compose testing checkpoint

The user clarified the immediate goal on 2026-09-18: a good-enough setup to test
on one machine with Docker Compose, not a perfected production deployment.
That local milestone is verified. The [start/test/stop guide](../runbooks/local-testing.md)
and [sanitized evidence](local-testing.json) record the result and its limits.

The demo uses the real release observer from `d633d87319497c44744512328271ad84e94b5a4e`
and the existing pinned backend images. The standard artifact consumer verified
its image/archive identity; 1,308 archived unit tests passed with two intentional
utility exclusions. The node image contains the same observer executable; the
demo builds no replacement binary. RPC data is explicitly simulated.

| Requirement | Implementation | Observable verification |
| --- | --- | --- |
| All monitoring runs under Compose on one host | `compose.observer.yaml` joins the existing backend; separate explicit migration service | Actual migration to schema22, continuous observer and all backend services started |
| Private service access without host-network assumptions | Observer joins default/ledger networks; generated container connection files use `postgres:5432`; observer publishes no host port | Real ingestion and Prometheus scrape, with actual container image/network/mount assertions |
| Administrative credentials remain separate | Migration gets only admin connection file; indexer gets writer file and read-only configuration/spool | Rendered and running observer mounts inspected; no administrator file mounted |
| Existing setup still works | Existing host connection files and enabled-alerting defaults retained; overlay is opt-in | Preparation compatibility tests and base-Compose regression |
| No external receiver required to try it | Explicit `alerting_enabled: false` pauses current/previous generated EEZ rules; unrelated Grafana routing is preserved | Fresh Grafana API confirms paused rules and no EEZ webhook contacts; preparation tests cover disable/re-enable |
| Useful, exact accounting and dashboards | Real indexer consumes existing deterministic RPC fixture; all seven dashboards provisioned | Two receipts and 738,216 wei per chain, two settled L2 blocks, actual Grafana SQL values and dashboard inventory |
| Restart preserves history | PostgreSQL volumes plus persistent simulated-RPC initial state | Observer restart and full Compose stop/start each require fresh successful ingestion and unchanged receipt counts/costs |
| User can inspect and stop it | `smoke-single-host.py --runtime PATH --keep` writes private `DEMO.md` and `compose.sh` | Successful demo intentionally retained; default/error cleanup targets only its unique project |
| Real nodes can share the setup | Optional `compose.node-network.yaml` attaches only the observer to an existing node network | Compose validation confirms attachment, explicit network-name requirement and no changes to other services; live-node connection is not claimed |

The full Python selection passed **144 tests with one explicit opt-in resource
skip** (145 selected), including 22 preparation tests and five real Compose
configuration checks. The initial sandbox run's five socket-permission errors
are retained; allowing the ephemeral HTTP fixtures resolved them without code
changes. No performance workload was run.

Two earlier demo failures exposed host-network dependencies: PostgreSQL was not
reachable through an internal-only network's published port, and Prometheus
could not scrape the host-network observer through this host's gateway route.
The final overlay uses private Compose DNS for both paths. No host firewall or
live EEZ services were changed. Both failed demo projects were cleaned up; their
private reports/logs remain as evidence.

The retained demo's current local address and private credential-file location
are in `.runtime/observability-compose-demo/DEMO.md`. Its data is simulated, its
alerts are paused, and missing native lifecycle/blob/trace evidence remains
unknown. It is not monitoring the existing live Chiado deployment. Independent
notification delivery, off-host disaster recovery, full production CI and
operational calibration remain future work under the clarified milestone.

The retained demo was subsequently published at the user's request through
[the existing HTTPS gateway](../runbooks/gateway.md). Its current address is
https://eez.asuscomm.com/grafana/; authentication remains enabled. The original
local smoke report is retained unchanged as historical evidence.

## Superseded live instance, 2026-09-18

The subsequent [fresh Chiado deployment](../../docs/audits/2026-09-18-chiado-observability-deployment/README.md)
replaced the public demo with real L1 10200 / L2 6291 monitoring. The simulated
project is stopped; its original report, configuration and all five volumes are
retained. The public Grafana URL and password are unchanged, but the active
PostgreSQL/Prometheus/Loki/Grafana data belongs to a separate fresh project. The
historical statements above describe the earlier simulated checkpoint.
