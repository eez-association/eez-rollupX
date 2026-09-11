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

Stop the gateway without deleting its certificate state:

```bash
docker compose -f testing/public-gateway/docker-compose.yml down
```

Do not add `-v` unless the persisted certificate/account state should also be
deleted.
