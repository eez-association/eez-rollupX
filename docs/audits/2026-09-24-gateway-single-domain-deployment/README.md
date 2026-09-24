# Single-domain gateway path deployment

| Field | Value |
| --- | --- |
| Date and status | 2026-09-24 UTC; deployed and locally verified, with the live limitations below |
| Scope | Public browser paths, network paths, and monitoring subpaths on `eez.dev`; no node, signer, contract, database, or DNS change |
| Source | Gateway `82afbfed93a150c0512f8ae929dd05a046d431e2`; dashboard `1ca6382dc4bcbd54a78ede03ab0aaf872c6503a5`; RollupX explorer branch `da4f8e56d3a2df93fd4fb4f7fee1b5f2dec78973`, based on fork `c18fea0692b0bdb8659b7eee39c85b28db1ca05e` |
| Protocol | Pinned gitlink `3c0cdf4955941a5c23639346932b55b33babebbd`; unchanged |
| Environment | Existing host-network Caddy and monitoring Compose projects; existing dashboard and explorer projects and data volumes; private ignored configuration migrated in place |
| Related records | [Design proposal](../../proposals/gateway-single-domain-paths.md), [explorer runbook](../../runbooks/explorers.md), gateway repository README |

## Findings

The earlier public `/dashboard/` path returned 404. The live gateway now uses one
`PUBLIC_DOMAIN=eez.dev`. `/` redirects to `/dashboard/`; dashboard, monitor,
Blockscout, Blobscan, Grafana, and Prometheus are served at their documented
paths. RPC, Composer, and WebSocket routes retain the exact L1/L2 paths, and
Beacon retains GET/HEAD access with POST rejected. The dashboard build default
is `/dashboard/`; `127.0.0.1:8080` is its private upstream, not a user-facing
URL.

Grafana and Prometheus needed only service subpath settings and Caddy routing.
The dashboard needed prefixed build assets. The two explorer frontends needed
separate path-aware images because both emit Next.js root asset paths without
those builds. Existing explorer databases and indexers were not recreated.

## Evidence inventory

| ID | Contents / provenance | Location and digest | Availability / retention |
| --- | --- | --- | --- |
| E1 | Pre-cutover gateway, dashboard, explorer, and monitoring configuration plus old container image IDs | `artifacts/gateway-single-domain-cutover-20260924/` in the operator checkout | Local only, private, ignored by Git; preserve until rollback is no longer needed |
| E2 | Gateway Caddy image, dashboard image, Blockscout web, Blobscan web | `sha256:5f5c8640aae01df9654968d946d8f1a56c497f1dd5c5cda4cf95ab7c14d58648`; `sha256:221f5218252e0cb770c5c285b33532b83ab0ceccff2dfb68fae55f6983f9a132`; `sha256:1ab9ecd342f786d61b57b839458896350ba28cb25b48bda868e9b2387802fc49`; `sha256:074f78bcf3b2ed81927e3bc6636b9f35d0ef3621638caff40f2ef2e8172bbbc0` | Local Docker image IDs at deployment; tags are mutable |
| E3 | Grafana and Prometheus images | `sha256:ac461fb352abc50da10a51c7d02462e9c05488f11f53f14b3ad79a8145f638a0`; `sha256:5ce7540c3c00ef4ab0c9d2c995c6a5b9c421f44b4a115d97a2c7af3b1c21cbb0` | Pinned monitoring image content |

## Reproduction and executed checks

Commands ran on the deployment host. Private env files were never printed.
Gateway Caddy validation used `docker compose -f compose.validate.yaml run
--rm --no-deps validate` after staging every optional path site; it returned
`Valid configuration`. Dashboard `docker compose build ui` with the default
`EEZ_UI_BASE_PATH=/dashboard/` passed TypeScript and Vite. A disposable UI
container served `/dashboard/`, its JavaScript asset, `/dashboard/config.json`,
and `/monitor/` with HTTP 200. The pinned Blockscout and Blobscan path images
had already built; Blobscan's Chiado profile and blob proxy checks passed.

| Check | Evidence type | Result |
| --- | --- | --- |
| Gateway Compose and Caddy validation with all sites | Static/process | Passed; no-network validation container |
| Explorer and monitoring Compose overlays | Static | Passed using the existing private env files; no secret values printed |
| Public `/`, `/dashboard/`, `/monitor/`, `/explorer`, `/blobscan`, `/grafana/`, `/prometheus/` | Live HTTP | 308 to dashboard; browser pages 200 (Grafana root 302 to its login); Prometheus unauthenticated 401 |
| Dashboard, monitor, explorer, Blobscan, and Grafana assets | Live HTTP/browser | Representative CSS/JS/assets 200; disposable Chromium rendered all five pages |
| `/monitor/api/health`, snapshot, and same-origin `/monitor/api/live` | Live HTTP/WebSocket | 200, 200, and 101; missing Origin was rejected 403 as designed |
| `/explorer/api/v2/stats`, `/explorer/socket/v2/websocket`, `/blobscan/api/env` | Live HTTP/WebSocket | 200, 101, and 200; explorer `/blocks` and Blobscan `/blocks` pages 200 |
| `/rpc/l1`, `/rpc/l2`, `/composer/l1`, `/composer/l2` | Live JSON-RPC | `eth_chainId` returned `0x27d8`, `0x1893`, `0x27d8`, `0x1893` |
| `/ws/l1`, `/ws/l2`; Beacon health GET and POST | Live WebSocket/HTTP | 101 and 101; Beacon GET 200, POST 405 |
| Public DNS and TLS | Live HTTP from the host | `eez.dev` resolved to the public address; TLS verification succeeded for dashboard and Grafana login |
| Dashboard/monitor explorer links | Live config/API | Both point to `/explorer`; monitor Blobscan link points to `/blobscan` |
| Container health | Live Docker state | Gateway, explorers, UI, monitor, Grafana, and Prometheus running; services with health checks healthy |

These checks establish routing and browser loading. They do not establish
semantic chain replay, explorer archive completeness, a successful authenticated
Prometheus login, or reachability from every external network. The web retrieval
tool could not fetch the new pages; host `curl` used the public DNS address with
valid TLS, and disposable Chromium ran on the deployment host.

## Operational changes and recovery

The user authorized deploying unmerged branches and confirmed all service
paths. Gateway and dashboard stable checkouts were switched to their reviewed
branches without merging. Private `.env` values and enabled gateway site files
were migrated after the E1 backup. The explorer web/proxy and monitoring
subpath settings use private, stable Compose overlays under E1; their
`COMPOSE_FILE` values now include those overlays so later Compose operations
retain the layout. Caddy reused its certificate volumes. Dashboard and monitor
were recreated to update browser links; explorer database/indexer, node,
signer, and beacon containers were not restarted.

Rollback was prepared but **not executed**. Restore the private E1 `.env` and
gateway site copies, switch the gateway and dashboard checkouts back to their
previous commits, and recreate their Compose services. Restore the E1 dashboard,
explorer, and monitoring env copies before recreating their affected web and
monitoring services with the old Compose selection. Preserve all Docker volumes;
do not use `down -v`. The old image IDs are recorded in E1. This changes
browser sessions and WebSocket connections; it does not provide durable recovery
of in-memory Composer requests after a separate node restart.

## Open work

| Uncertainty | Impact | Next check |
| --- | --- | --- |
| Authenticated Prometheus browser flow | 401 confirms protection, not a successful authorized login | An operator with the existing credentials should open `/prometheus/` |
| Independent off-host reachability | Host public-DNS requests and local Chromium do not cover every client network | Confirm from a separate external connection |
| Explorer semantic/data-download checks | Landing pages and APIs do not prove archive completeness | Run the explorer runbook's canonical verification and browser data-download checks when needed |
