# Public HTTPS gateway

This standalone Docker Compose project publishes an EEZ development network
through one HTTPS domain. It is separate from the Kurtosis package but joins
the enclave's external Docker network to reach its services directly.

## Endpoints

With the defaults:

| Purpose | URL |
|---|---|
| Protocol playground | `https://eez.asuscomm.com/` |
| Network observatory | `https://eez.asuscomm.com/monitor/` |
| Canonical L1 RPC | `https://eez.asuscomm.com/rpc/l1` |
| L2 RPC | `https://eez.asuscomm.com/rpc/l2` |
| L1 wallet/cross-chain front | `https://eez.asuscomm.com/front/l1` |
| L2 wallet/cross-chain front | `https://eez.asuscomm.com/front/l2` |
| Blobscan | `https://eez.asuscomm.com:4443/` |
| L1 Blockscout | `https://eez.asuscomm.com:4444/` |
| L2 Blockscout | `https://eez.asuscomm.com:4445/` |

The browser UI uses `/composer/l1` and `/composer/l2`. Those routes are exact
aliases for `/front/l1` and `/front/l2`, respectively.

The RPC routes rewrite their path to `/` before proxying, so they work with
ordinary Ethereum JSON-RPC clients. Use the front URLs for wallet transactions
and the direct RPC URLs for reads and protocol tooling.

Each public Blockscout origin also routes `/api` and `/socket` to its own
backend. The Kurtosis launcher configures HTTPS browsers to use that origin,
including `wss://eez.asuscomm.com:4444/socket/v2/websocket` for L1 and port
`4445` for L2. This keeps internal Docker hostnames out of browser connections
and allows live updates over HTTPS. Local HTTP frontend URLs continue to use
the Next.js HTTP API proxy.

## Requirements

- The domain A/AAAA records must point to this host. Remove an incorrect AAAA
  record if the host is not reachable over IPv6.
- Inbound TCP 80 and TCP/UDP 443 must reach this host.
- Inbound TCP 4443–4445 must reach this host for Blobscan and both Blockscout
  frontends. A router DMZ configuration normally forwards these ports already.
- The target Kurtosis enclave must be running and its Docker network must
  exist. The default is `kt-eez-engineer-dev`.
- The RPCs are unauthenticated. Use only for a disposable development network.

## Start

```bash
docker compose -f testing/public-gateway/docker-compose.yml up -d
docker compose -f testing/public-gateway/docker-compose.yml logs -f gateway
```

Caddy obtains and renews a publicly trusted certificate automatically. Its
certificate state is retained in the `caddy_data` volume.

Override the domain or enclave network when needed:

```bash
EEZ_DOMAIN=dev.example.com \
EEZ_DOCKER_NETWORK=kt-another-enclave \
  docker compose -f testing/public-gateway/docker-compose.yml up -d
```

The upstream service variables can also be overridden:

```text
EEZ_L1_RPC_UPSTREAM
EEZ_L2_RPC_UPSTREAM
EEZ_L1_FRONT_UPSTREAM
EEZ_L2_FRONT_UPSTREAM
EEZ_UI_UPSTREAM
EEZ_DASHBOARD_UPSTREAM
EEZ_BLOBSCAN_UPSTREAM
EEZ_L1_EXPLORER_UPSTREAM
EEZ_L2_EXPLORER_UPSTREAM
EEZ_L1_EXPLORER_API_UPSTREAM
EEZ_L2_EXPLORER_API_UPSTREAM
```

## Validate and operate

```bash
docker compose -f testing/public-gateway/docker-compose.yml config
docker compose -f testing/public-gateway/docker-compose.yml exec gateway \
  caddy validate --config /etc/caddy/Caddyfile

cast chain-id --rpc-url https://eez.asuscomm.com/rpc/l1
cast chain-id --rpc-url https://eez.asuscomm.com/rpc/l2
```

Recreating a Kurtosis enclave disconnects these standalone containers from its
old Docker network. Reconnect the existing gateway and monitor to the new
network, then restart the monitor to clear cached data from the previous chain:

```bash
docker network connect kt-eez-devnet eez-public-gateway
docker network connect --alias eez-monitor kt-eez-devnet eez-monitor
docker restart eez-monitor
```

Use the actual enclave network name. These commands preserve the gateway's
certificate volumes and existing public routes.

Stop the gateway without deleting its certificate state:

```bash
docker compose -f testing/public-gateway/docker-compose.yml down
```

Do not add `-v` unless the persisted certificate/account state should also be
deleted.

## Chiado deployment

The [Chiado deployment audit](../../docs/audits/2026-09-13-chiado-settlement-policy/README.md)
records the active chain bindings and generated runtime configuration. Its
gateway retains Docker-published ports 80 and 443 and the existing certificate
volumes. It reaches the host UI proxy through `host.docker.internal:18081`; that
proxy forwards the four RPC paths and `/monitor/` to the local Chiado services.
This lets the L1 RPC and monitor keep their loopback bindings.

Preserve the published-port topology when changing this deployment. On this
host, UFW permits the UI proxy port while Docker provides the public HTTPS
forwarding. Moving Caddy to host networking makes a loopback HTTPS check pass
while normal public-DNS requests time out. Validate through normal DNS as well
as any `curl --resolve` checks after a routing change.

The Chiado profile uses the public Gnosis L1 explorer. Its separate
[EEZ explorer stack](../explorers/README.md) is now published through this gateway:
Blobscan at `https://eez.asuscomm.com:4443/` and L2 Blockscout at
`https://eez.asuscomm.com:4445/`. Port 4444 remains unused for Chiado. The gateway
imports `testing/explorers/public.caddy`, joins the explorer Docker network,
and retains the existing certificate volumes. The monitor links to both
explorers and reads its blob archive through the private host API.

The monitor also uses WebSockets throughout:

- `wss://eez.asuscomm.com/ws/l1` forwards to the embedded L1 WS port, 18646.
- `wss://eez.asuscomm.com/ws/l2` forwards to the L2 WS port, 18689.
- `wss://eez.asuscomm.com/monitor/api/live` carries shared monitor snapshots.

Both node WS listeners remain on loopback. The host UI nginx proxy forwards
WebSocket upgrades; Caddy terminates TLS on the existing public port 443.
The monitor receives `newHeads` on both chains and reconciles canonical data
before pushing snapshots. Its ordinary HTTP snapshot API remains the reconnect
fallback. Monitor and bridge application changes live in the separate
`eez-rollup-ui` repository; this repository documents their deployment routes.

Manage the Chiado deployment with its generated Compose file rather than the
Kurtosis gateway file above:

```bash
docker compose -p eez-chiado-public \
  -f .runtime/chiado-settlement-20260913/public/compose.json ps
```
