# Chiado quick start

Use a fresh deployment directory. Contract deployment uses this checkout, so
its source revision must match the images ([version checks](chiado-deployment.md#1-select-a-published-revision-and-check-the-checkout)).
Run commands in order and stop on any failure. This creates a
**new rollup and spends Chiado test xDAI**. For an existing rollup, use
[restart](chiado-deployment.md#restart-and-stop); preserve its bindings, genesis,
keys, images, and databases. Full validation and recovery:
[Chiado runbook](chiado-deployment.md). Fresh-host rehearsal is pending.

Required: Linux amd64 with glibc 2.36+ (for example Ubuntu 24.04), local Docker
Engine/Compose, Git, Bash, Make, curl, jq, Python 3, OpenSSL, and Foundry **1.7.1**
(`forge`, `cast`). Reserve the [ports and container names](chiado-deployment.md#ports-and-service-ownership),
restrict exposed RPC/engine ports, and allow P2P traffic. Use a checkout path
without whitespace or colons and enough disk for the L1 snapshot and growing databases.

## 1. Choose images and configure

Images default to `:latest`, published after `main` passes CI. To pin a version,
use the same full Git commit tag on both images, or use their image digests.

```bash
git submodule update --init --recursive eez-core-protocol
cp .env.example .env
cp .env.chiado.example .env.chiado
chmod 600 .env .env.chiado
```

Edit both files before continuing:

- `.env`: synchronized external Chiado `EEZ_L1_RPC_URL` (chain ID **10200**),
  dedicated poster, proof-signer, and L2-system keys. Fund the poster with test
  xDAI; it also deploys unless you set/fund a separate `EEZ_DEPLOY_KEY`.
- `.env.chiado`: the **same three keys** and a working
  `EEZ_L1_BUILDER_RPC_URL` supporting `eth_sendBundle`. Keep the default paths,
  Beacon URL and `eip7594`. Keep `:latest` or set both image references to the
  version you selected. Proof/system keys must differ; keep poster separate
  from proof signer. Review the base genesis's funded development accounts before
  registration; changing genesis later changes the rollup identity.

## 2. Pull images and prepare tools

```bash
source .env.chiado
docker pull --platform linux/amd64 "$EEZ_NODE_IMAGE"
docker pull --platform linux/amd64 "$EEZ_PROOF_SIGNER_IMAGE"
docker pull --platform linux/amd64 sigp/lighthouse:v8.1.3
mkdir -p .runtime/chiado-tools
tools_container="$(docker create --platform linux/amd64 "$EEZ_NODE_IMAGE")"
docker cp "$tools_container:/usr/local/bin/eez-genesis-state-root" .runtime/chiado-tools/
docker cp "$tools_container:/usr/local/bin/eez-genesis-block-hash" .runtime/chiado-tools/
docker rm "$tools_container"
export PATH="$PWD/.runtime/chiado-tools:$PATH"
eez-genesis-state-root genesis.json
eez-genesis-block-hash genesis.json
forge build --root contracts
bash scripts/check-chiado-blob-config.sh
```

Expect two 32-byte hashes, a successful contract build, and configuration `PASS`.
No Rust build is needed. Before broadcasting, run the detailed
[preflight configuration check](chiado-deployment.md#preflight-configuration) to
verify keys, RPC freshness, funding, builder availability, and genesis rendering.

## 3. Prepare chain data

```bash
mkdir -p data
docker run --rm -v "$PWD/data/chiado-l1:/data" \
  ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
  download --chain chiado --minimal --datadir /data
openssl rand -hex 32 > data/jwt.hex
```

Wait for a successful download. Run this once; preserve the snapshot/databases
and JWT on restart.

## 4. Deploy, then start

**Broadcasts new contracts; do not repeat after a partial failure.** Preserve logs
and `contracts/broadcast/` receipts if deployment fails.

```bash
EEZ_DEPLOY_SKIP_SIMULATION=1 make deploy-protocol
```

On success, `deployments.env`, `datadir/genesis.json`, and
`datadir/genesis.profile.json` exist. Then start:

```bash
chmod 644 datadir/genesis.json
bash scripts/chiado-up.sh
```

The simulation flag bypasses Foundry simulation, **not broadcasting**. Startup
requires the selected images already present and never builds, pulls, redeploys,
or deletes data. Success means L2 advances, the L1 commitment advances, and a
post-start type-3 settlement's blobs are retrievable. A timeout leaves containers
running: inspect logs instead of redeploying.

## 5. Check or stop

```bash
docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml ps
docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml logs --tail=100 eez-node
cast block-number --rpc-url http://127.0.0.1:18688
```

Optional stop (retains on-disk chain data):

```bash
bash scripts/teardown-chiado.sh
```

RPC: L2 `18688`, embedded L1 `18645`, inbound front `18999`, outbound front
`18998`. Preserve `.env`, `.env.chiado`, `deployments.env`, `datadir/`, `data/`,
the selected commit and resolved image digests. Pulling `latest` again selects an
upgrade; ordinary startup reuses installed images. Pin commit/digest references
in `.env.chiado` when you need a fixed version. Use the detailed runbook for
[canonical/genesis verification](chiado-deployment.md#start-and-verify),
[failures](chiado-deployment.md#troubleshooting), and
[local development builds](chiado-deployment.md#local-source-development).
