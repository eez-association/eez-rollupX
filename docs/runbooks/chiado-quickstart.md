# Chiado quick start

Create a **new rollup** from an unused deployment checkout; deployment spends
Chiado test xDAI. Run commands from the repository root and stop on failure.
This revision requires the offline-history profile from genesis, with new
contracts and fresh L2 datadirs. For an existing rollup, keep its matching
release and follow that release's restart/Composer procedure; these binaries
do not migrate it. See the [profile boundary](chiado-deployment.md#offline-v2-proving-and-fresh-network-boundary).
Fresh-host deployment and restart rehearsal are pending.

Required: Linux amd64, local Docker Engine/Compose, Git, Bash, curl, jq, OpenSSL,
and Foundry **1.7.1** `cast` for startup checks. Reserve the
[ports and container names](chiado-deployment.md#ports-and-service-ownership),
restrict exposed RPC/engine ports, and allow P2P traffic. Use a checkout path
without whitespace or colons, outside `/repo`, and enough disk for growing databases.
The deploy image supplies contract sources, Forge, and genesis tools.

## 1. Configure

Create `.env` only if it does not already exist; preserve existing settings:

```bash
cp .env.chiado.example .env
chmod 600 .env
```

Edit `.env`: set a synchronized Chiado `EEZ_L1_RPC_URL` and dedicated poster,
proof-signer keys. Keep the poster separate from the proof signer. Native L2
transactions use a fixed address and need no key. Fund the poster with test xDAI; it also deploys
unless you set and fund a separate `EEZ_DEPLOY_KEY`.

The example enables the [live Chiado policy/timing preset](chiado-deployment.md#1-configure):
automatic admission, `always` settlement, and explicit fee ceilings. Review those
values for this deployment; preserve existing operator settings when restarting.

The three EEZ images default to `:latest`, which can change on startup.
To keep a fixed deployment, set all three image references to the same full Git
commit tag, or pin each image's digest.
Use the matching checkout for Compose/configuration; details:
[image selection](chiado-deployment.md#image-selection).

The deploy image's base genesis includes funded development accounts;
review [genesis allocations](chiado-deployment.md#genesis-allocations)
before registering a rollup intended for shared use.

## 2. Prepare chain data — once

```bash
mkdir -p data
docker run --rm -v "$PWD/data/chiado-l1:/data" \
  ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
  download --chain chiado --minimal --datadir /data
openssl rand -hex 32 > data/jwt.hex
```

## 3. Deploy contracts — once

**Broadcasts transactions. Do not repeat after a partial failure:** inspect
`contracts/broadcast/` receipts and deployment output first.

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
and `datadir/genesis.profile.json`.

## 4. Start and check

```bash
bash scripts/chiado-up.sh
```

Success: `chiado L2 is UP` and a settlement transaction hash. The script starts
the node, signer, and Lighthouse, then checks L2 height, L1 settlement, and blob
retrieval. A timeout leaves containers running; inspect logs:

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
[failures](chiado-deployment.md#troubleshooting) and
[local development](chiado-deployment.md#local-source-development).

Optional L2 Blockscout and Chiado Blobscan run separately after startup; see
[explorers](chiado-deployment.md#optional-explorers) for the deployment inputs
and setup procedure. They are excluded from CI.
