# Single-domain paths for the public gateway

| Field | Value |
| --- | --- |
| Design status | Draft; implementation prepared, live cutover and backend-backed browser validation pending |
| Date | 2026-09-24 |
| Source baseline | RollupX fork `c18fea0692b0bdb8659b7eee39c85b28db1ca05e`; isolated worktree, existing operator edits in the primary checkout preserved |
| Protocol baseline | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Related work | `rollupx-gateway` initial commit `f37320d5b83c0a36b39e665cd01c8a290cd491be`; `eez-rollup-dashboard` baseline `d00bf27dab25be5abe529296002a8aa7065c7b33` |

## Problem and scope

The public gateway currently gives dashboard, monitor, explorers, and observability services separate hostnames. The desired layout uses one `PUBLIC_DOMAIN`, with `/dashboard/`, `/monitor/`, `/explorer`, `/blobscan`, `/grafana/`, and `/prometheus/`. Existing exact `/rpc/*`, `/composer/*`, `/ws/*`, and read-only `/beacon/*` paths stay on that hostname.

The browser frontends emit root-relative URLs. Merely stripping a prefix in Caddy leaves their static assets, APIs, WebSockets, redirects, and links on the wrong path. Blockscout and Blobscan require dedicated path-aware builds; the dashboard requires a Vite build prefix. Grafana and Prometheus require their own external URL settings. This change does not deploy, restart, or migrate live services.

## Requirements and acceptance

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| R1 | One hostname serves all public browser apps without asset, API, WebSocket, or link collisions. | Isolated browser checks from each prefixed landing page through a deep link and network request. |
| R2 | L1/L2 RPC, Composer, and WebSocket routes retain exact path and upstream behavior; Beacon remains GET/HEAD only. | Compare adapted routes and targeted HTTP/WebSocket process checks. |
| R3 | Explorer path images are versioned separately from current root-path images; current service state and data remain untouched during preparation. | Inspect image names, Compose output, and build/source revisions. |
| R4 | Credentials and private upstream hosts stay outside tracked files. | Review exact staged files, ignored operator state, and the final commit. |
| R5 | Upgrade and rollback account for Caddy, dashboard, explorers, Grafana, Prometheus, DNS, and links as a coordinated cutover. | Runbook documents ordered preparation, validation, cutover, and rollback; no deployment is claimed before live checks. |

## Design and alternatives

The gateway uses one Caddy site block. It strips `/dashboard` before proxying to the UI, retains `/monitor` so the UI's existing monitor API proxy can serve `/monitor/api/*`, and retains prefixes for Next.js explorers, Grafana, and Prometheus. The path-aware dashboard build uses Vite's base path. Explorer images use pinned upstream revisions and build-time Next.js `basePath` settings, plus explicit fixes for manual root-relative URLs. The existing root-path images remain available for rollback.

A gateway-only rewrite was rejected: the two Next.js apps both use `/_next` and root-relative APIs/assets, and a proxy cannot assign those requests to the correct app reliably across tabs and rebuilds.

## Compatibility and recovery

The old subdomains remain valid until the cutover; no private `.env`, enabled gateway site, database, or certificate volume is modified during preparation. New frontend image tags must not replace the running root-path tags. Build and start the path-aware upstreams first, validate them on loopback, then change the gateway and browser links together. Preserve old images, site configuration, and certificates for rollback. The rollup and signer are unchanged. In-memory Composer requests still have their documented restart behavior if an operator separately restarts the node; this gateway migration does not require it.

## Validation plan

Validate Caddy with every optional path template in an isolated no-network container, then exercise the path router against synthetic upstreams. Build the dashboard and both pinned explorer frontends. Check generated HTML assets and browser deep links, API calls, and WebSocket URLs. Validate Grafana and Prometheus path settings against disposable instances or the prepared deployment; do not infer readiness from Caddy syntax alone. Run the fork's explorer configuration and relevant browser checks before cutover. A live cutover needs a separate prepared operation and post-cutover checks.

## Implementation and follow-up

Implementation is in isolated worktrees. No live service has been changed.
On 2026-09-24, the path-aware dashboard, Blockscout frontend, and Chiado
Blobscan web images built with distinct tags. The Blobscan build passed its
existing Chiado profile and blob-proxy checks. A no-network Caddy configuration
with every optional path enabled validated, and a synthetic upstream confirmed
prefix stripping/preservation, exact network routes, Prometheus authentication,
404 fallback, and Beacon POST rejection. Isolated HTTP checks confirmed the
dashboard and monitor HTML, prefixed Next.js assets, Blockscout's `/explorer`
landing route, and Blobscan's `/blobscan` landing and favicon routes. A
disposable Chromium check rendered dashboard, monitor, and Blobscan through
the gateway. These checks used synthetic settings and no live backend.

The live RPC/Composer/Beacon bindings, monitor API, explorer data/API/WebSocket
flows, Grafana, Prometheus, TLS, and DNS still need prepared cutover checks.
The current public monitor endpoint returned 502 during this investigation;
that condition predates these changes and was not altered here. The gateway
and frontend image changes must be applied together because their old and new
path assumptions are incompatible.
