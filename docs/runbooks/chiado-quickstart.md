# Chiado quick start

Create a **new rollup** from an unused deployment checkout; deployment spends
Chiado test xDAI. Run commands from the repository root and stop on failure.
For an existing rollup, use [restart](chiado-deployment.md#restart-and-stop).
Fresh-host deployment and restart rehearsal are pending.

Required: Linux amd64, local Docker Engine/Compose, Git, Bash, curl, jq, OpenSSL,
and Foundry **1.7.1** `cast` for startup checks. Reserve the
[ports and container names](chiado-deployment.md#ports-and-service-ownership),
restrict exposed RPC/engine ports, and allow P2P traffic. Use a checkout path
without whitespace or colons, outside `/repo`, and enough disk for growing databases.
The deploy image supplies contract sources, Forge, and genesis tools.

## 1. Configure

```bash
cp .env.chiado.example .env
chmod 600 .env
```

Edit `.env`:

- Set a synchronized Chiado `EEZ_L1_RPC_URL` and dedicated poster,
  proof-signer, and L2-system keys. Proof/system keys must differ; keep the poster
  separate from the proof signer. Fund the poster with test xDAI; it also deploys
  unless you set and fund a separate `EEZ_DEPLOY_KEY`.
- Verify `EEZ_L1_BUILDER_RPC_URL` supports `eth_sendBundle`. Keep the default
  `COMPOSE_FILE`, paths, Beacon URL, and `eip7594`. Compose loads `.env` automatically.

The three EEZ images default to `:latest`. Docker downloads images automatically;
Compose can update `latest` on startup. To keep a fixed deployment, set all three
image references to the same full Git commit tag, or pin each image's digest.
Use the matching checkout for Compose/configuration; details:
[image selection](chiado-deployment.md#image-selection).

```bash
source .env
cast chain-id --rpc-url "$EEZ_L1_RPC_URL"
```

Require **10200** before continuing. The deploy image's base genesis includes
funded development accounts; review [genesis allocations](chiado-deployment.md#genesis-allocations)
before registering a rollup intended for shared use.

## 2. Prepare chain data — once

```bash
mkdir -p data
docker run --rm -v "$PWD/data/chiado-l1:/data" \
  ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
  download --chain chiado --minimal --datadir /data
openssl rand -hex 32 > data/jwt.hex
```

Wait for a successful download. Preserve the databases and JWT on restart.

## 3. Deploy contracts — once

**Broadcasts transactions. Do not repeat after a partial failure:** inspect
`contracts/broadcast/` receipts and deployment output first. The deploy command
checks genesis generation and tools before its first broadcast.

```bash
source .env
mkdir -p datadir contracts/broadcast
docker run --rm --pull always --platform linux/amd64 --network host \
  -v "$PWD:$PWD" \
  -v "$PWD/.env:/repo/.env:ro" \
  -v "$PWD/contracts/broadcast:/repo/contracts/broadcast" \
  -e EEZ_DEPLOYMENTS_FILE="$PWD/deployments.env" \
  -e EEZ_GENESIS_OUT="$PWD/datadir/genesis.json" \
  -e EEZ_DEPLOY_SKIP_SIMULATION=1 \
  "$EEZ_DEPLOY_IMAGE"
```

Require a successful exit and completed `deployments.env`, `datadir/genesis.json`,
and `datadir/genesis.profile.json`. Output files are owned by root. The simulation
flag skips Foundry simulation; transactions still broadcast. A genesis file alone
does not establish a completed deployment.

## 4. Start and check

```bash
bash scripts/chiado-up.sh
docker compose ps
cast block-number --rpc-url http://127.0.0.1:18688
```

Startup pulls images through Compose, starts the three services, and verifies
advancing L2/L1 commitments plus retrievable blobs from a new type-3 settlement.
It preserves data and requires existing deployment bindings. A timeout leaves
containers running; inspect logs instead of redeploying:

```bash
docker compose logs --tail=100 eez-node eez-proof-signer lighthouse
```

Optional stop, retaining chain data:

```bash
bash scripts/teardown-chiado.sh
```

RPC ports: L2 `18688`, embedded L1 `18645`, inbound front `18999`, outbound front
`18998`. Preserve `.env`, `deployments.env`, `datadir/`, `data/`,
receipts, and image references/digests. Use the detailed runbook for
[verification](chiado-deployment.md#start-and-verify),
[failures](chiado-deployment.md#troubleshooting), and
[local development](chiado-deployment.md#local-source-development).
