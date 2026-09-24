# Single-domain paths for the public gateway

| Field | Value |
| --- | --- |
| Design status | Draft; implemented and deployed from feature branches on 2026-09-24, merge pending |
| Date | 2026-09-24 |
| Source baseline | RollupX fork `c18fea0692b0bdb8659b7eee39c85b28db1ca05e`; isolated worktree, existing operator edits in the primary checkout preserved |
| Protocol baseline | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Related work | `rollupx-gateway` initial commit `f37320d5b83c0a36b39e665cd01c8a290cd491be`; `eez-rollup-dashboard` baseline `d00bf27dab25be5abe529296002a8aa7065c7b33` |

## Problem and scope

The public gateway previously gave dashboard, monitor, explorers, and observability services separate hostnames. The deployed layout uses one `PUBLIC_DOMAIN`, with `/dashboard/`, `/monitor/`, `/explorer`, `/blobscan`, `/grafana/`, and `/prometheus/`. Existing exact `/rpc/*`, `/composer/*`, `/ws/*`, and read-only `/beacon/*` paths stay on that hostname.

The browser frontends emit root-relative URLs. Merely stripping a prefix in Caddy leaves their static assets, APIs, WebSockets, redirects, and links on the wrong path. Blockscout and Blobscan require dedicated path-aware builds; the dashboard requires a Vite build prefix. Grafana and Prometheus require their own external URL settings.

## Requirements and acceptance

| ID | Required behavior or invariant | Acceptance check |
| --- | --- | --- |
| R1 | One hostname serves all public browser apps without asset, API, WebSocket, or link collisions. | Isolated browser checks from each prefixed landing page through a deep link and network request. |
| R2 | L1/L2 RPC, Composer, and WebSocket routes retain exact path and upstream behavior; Beacon remains GET/HEAD only. | Compare adapted routes and targeted HTTP/WebSocket process checks. |
| R3 | Explorer path images are versioned separately from current root-path images; current service state and data remain untouched during preparation. | Inspect image names, Compose output, and build/source revisions. |
| R4 | Credentials and private upstream hosts stay outside tracked files. | Review exact staged files, ignored operator state, and the final commit. |
| R5 | Upgrade and rollback account for Caddy, dashboard, explorers, Grafana, Prometheus, DNS, and links as a coordinated cutover. | Runbook documents the procedure; the dated audit records the live cutover, checks, and rollback limits. |

## Design and alternatives

The gateway uses one Caddy site block. It strips `/dashboard` before proxying to the UI, retains `/monitor` so the UI's existing monitor API proxy can serve `/monitor/api/*`, and retains prefixes for Next.js explorers, Grafana, and Prometheus. The path-aware dashboard build uses Vite's base path. Explorer images use pinned upstream revisions and build-time Next.js `basePath` settings, plus explicit fixes for manual root-relative URLs. The existing root-path images remain available for rollback.

A gateway-only rewrite was rejected: the two Next.js apps both use `/_next` and root-relative APIs/assets, and a proxy cannot assign those requests to the correct app reliably across tabs and rebuilds.

## Compatibility and recovery

The old subdomains remained valid until the cutover; no private `.env`, enabled gateway site, database, or certificate volume was modified during preparation. New frontend image tags did not replace the running root-path tags. The path-aware upstreams were built and validated before changing the gateway and browser links. Old images, site configuration, and certificates were preserved for rollback. The rollup and signer are unchanged. In-memory Composer requests still have their documented restart behavior if an operator separately restarts the node; this gateway migration did not require it.

## Validation plan

Validate Caddy with every optional path template in an isolated no-network container, then exercise the path router against synthetic upstreams. Build the dashboard and both pinned explorer frontends. Check generated HTML assets and browser deep links, API calls, and WebSocket URLs. Validate Grafana and Prometheus path settings against disposable instances or the prepared deployment; do not infer readiness from Caddy syntax alone. Run the fork's explorer configuration and relevant browser checks before cutover. A live cutover needs a separate prepared operation and post-cutover checks.

## Implementation evidence

The [2026-09-24 deployment record](../audits/2026-09-24-gateway-single-domain-deployment/README.md)
records the builds, isolated checks, coordinated live cutover, image identities,
and remaining verification limits. The proposal remains a draft until the
branches are reviewed and merged.
