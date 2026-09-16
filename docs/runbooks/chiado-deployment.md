# Chiado deployment and operation

For the short first-deployment path, use the [quick start](chiado-quickstart.md).

Use [`docker-compose.chiado-node.yml`](../../docker-compose.chiado-node.yml):
`eez-node` runs the Composer, embedded Chiado L1 execution client, and L2;
`eez-proof-signer` independently validates batches; Lighthouse drives L1 consensus
and reconstructs blobs. `docker-compose.chiado.yml` is a different, Beacon-only
profile. Entry points: [deploy](../../scripts/deploy.sh),
[start](../../scripts/chiado-up.sh), [stop](../../scripts/teardown-chiado.sh).

| Verification record | Value |
| --- | --- |
| Runtime source reviewed | 2026-09-16; fork `ddf40bc2eb5fac8f7973b112eb383256cd8aac74`; protocol `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Checked | Source/configuration consistency, Compose rendering, Bash syntax, and offline regressions; see [validation record](../proposals/chiado-published-images.md#compatibility-and-validation) |
| Pending | Full fresh-host deployment and restart rehearsal; record execution in a dated [audit](../audits/TEMPLATE.md) |

## Select the procedure

| Operation | Procedure | Required starting state |
| --- | --- | --- |
| Create a new registry and rollup | [First deployment](#first-deployment), then [start and verify](#start-and-verify) | Unused deployment paths; no existing stack on these names/ports |
| Add or replace a Composer on an existing L2 | [Existing-rollup Composer](chiado-existing-rollup.md) | Original deployment files/keys, compatible runtime, and L2 backup or complete replay history |
| Start/restart the same rollup | [Restart and stop](#restart-and-stop), then [start and verify](#start-and-verify) | Original bindings, genesis, keys, JWT, images, and datadirs |
| Move hosts, upgrade, or recover missing state | [Recovery and upgrades](#recovery-and-upgrades) | Deployment inventory, preserved state, and an identified recovery source |

## Shell, host, and network prerequisites

Run commands in Bash from the intended deployment checkout's root. Use a checkout
path without whitespace or colons, and outside `/repo`: generated bindings contain
unquoted paths, volume syntax uses colons, and the deploy image owns `/repo`.
Use the default paths and ports below. Custom layouts require adapting Compose,
the startup script (which hardcodes the L1/L2 health-check ports), and the commands
together.
A worktree or `COMPOSE_PROJECT_NAME` does not isolate fixed container names,
host-network ports, or bind-mounted databases.

Use a fresh shell without inherited `EEZ_*`, `LH_*`, `COMPOSE_*`, or data-path
overrides, which can take precedence over `.env`. Its `COMPOSE_FILE` selects
`docker-compose.chiado-node.yml`. Stop at a failed command.

| Requirement | Check/constraint |
| --- | --- |
| Linux amd64, local Docker Engine and Compose | Bind mounts refer to this host. The deploy image supplies Forge, protocol sources, and genesis tools. |
| Host tools | Bash, Git, `curl`, `jq`, OpenSSL, GNU utilities (`sha256sum`, `cmp`), `ss` (iproute2) |
| Foundry | Host `cast` from **1.7.1** for startup and diagnostics; the deploy image supplies Forge **1.7.1** |
| Storage capacity | Space for images, L1 snapshot, PeerDAS data, and growing databases. Check `df -h .` and Docker storage; no measured minimum is established here. |
| Network access | Public GHCR images, Solidity compiler download, snapshot download, checkpoint source, deployment RPC, builder, and P2P peers. |
| Deployment RPC | Synchronized Chiado execution RPC, chain ID **10200**; remains required by startup even when the embedded L1 is running |
| Builder | Separate endpoint implementing `eth_sendBundle`; an ordinary execution RPC is insufficient |
| Beacon | Packaged Lighthouse **v8.1.3**, checked-in `configs/chiado`, `--semi-supernode`, full-blob API, and `EEZ_L1_BLOB_SIDECAR_ENCODING=eip7594`; built-in Chiado config in this image leaves Fulu disabled |

### Ports and service ownership

Run `ss -lntup` and `docker ps -a --format '{{.Names}}'` before first deployment.
The fixed containers are `eez-node-chiado`, `eez-proof-signer-chiado`, and
`eez-chiado-lighthouse`. Resolve existing owners before proceeding.

| Default port | Service/access |
| --- | --- |
| 18645 / 18646 | Embedded Chiado L1 HTTP / WebSocket RPC |
| 18688 / 18689 | L2 HTTP / WebSocket RPC |
| 18999 / 18998 | L1-to-L2 / L2-to-L1 transaction fronts; ordinary RPC calls proxy to the source chain |
| 18551 / 18684 | L1 / L2 engine APIs; shared JWT protects the L1 engine connection |
| 50061 / 6152 | Proof signer gRPC / Beacon HTTP; loopback |
| 30544 TCP/UDP, 30554 UDP | Embedded L1 P2P/discv4 and discv5 (`EEZ_L1_P2P_PORT + 10`) |
| 9205 TCP/UDP, 9206 UDP | Lighthouse P2P/discovery and QUIC |
| 30640 / 30641 | L2 P2P/discovery settings reserved by the profile; discovery disabled |

L2 HTTP, both fronts, and the L1 engine API bind all host interfaces. Restrict
unintended access with the host firewall; host networking provides no Docker port
isolation. Permit required P2P traffic and configure NAT where needed. Public
HTTPS, explorers, and UI routing are separate; see the
[gateway's Chiado notes](../../testing/public-gateway/README.md#chiado-deployment).

### Keys, funding, and files

| Key | Purpose | Funding |
| --- | --- | --- |
| `EEZ_L1_POSTER_KEY` | Settlement sender | Chiado native gas funds (test xDAI) |
| `EEZ_DEPLOY_KEY` (optional) | Contract deployer/owner; defaults to poster | Chiado gas funds if separate |
| `EEZ_PROOF_SIGNER_KEY` | Batch attestation; registered in proof system | No transaction funding needed for attestation |
| `EEZ_L2_SYSTEM_KEY` | L2 system transactions | Address funded in generated L2 genesis |

Generate dedicated keys with your wallet tooling. Proof signer and L2 system
addresses **must differ**; keep the poster separate from the proof signer.
A separate deploy key avoids nonce competition with an active poster. Do not
share a posting wallet with concurrent workloads. Fund deployer and poster before
broadcasting; the [environment example](../../.env.chiado.example) identifies a
Chiado faucet. Funding must cover deployment and ongoing settlement gas.

| File/variable | Default location and ownership |
| --- | --- |
| `.env` | Private RPC/keys, node/signer/deploy images, and Compose settings; copied from `.env.chiado.example`, used by deployment and startup; preserve |
| `deployments.env` | Generated public contract/rollup bindings and genesis/profile paths; preserve |
| `FRESH_GENESIS` | `./datadir/genesis.json`; generated at deployment, mounted into node and signer |
| Genesis profile | `./datadir/genesis.profile.json`; generated code/state commitments; preserve |
| `L1_SNAPSHOT_DIR` | `./data/chiado-l1`; writable embedded L1 database |
| `L2_DATA_DIR` | `./data/eez-l2`; writable L2 database, witnesses, and correlations |
| `LIGHTHOUSE_DATA` | `./data/chiado-lighthouse`; writable consensus/PeerDAS data |
| `JWT_FILE` | `./data/jwt.hex`; private, shared by node and Lighthouse; create once |
| `CHIADO_CONFIG_DIR` | `./configs/chiado`; versioned network config, including `bootstrap_nodes.yaml` alias |

The bare-binary `EEZ_L2_DATADIR` example does not replace Docker's `L2_DATA_DIR`.
Do not set `EEZ_ENV_FILE`, `EEZ_DEPLOYMENTS_FILE`, `EEZ_GENESIS_OUT`,
or `EEZ_GENESIS_PROFILE_OUT` in `.env`; the deploy command supplies its output
paths explicitly. Generated paths in `deployments.env` are absolute: moving a
checkout requires path reconciliation.

## First deployment

Complete steps 1–3, then [start and verify](#start-and-verify). Use an unused
deployment checkout and data paths; keep the stack stopped during setup.

### 1. Configure

Create `.env` only if it does not already exist; preserve existing settings:

```bash
cp .env.chiado.example .env
chmod 600 .env
```

Edit `.env`: set a synchronized external Chiado `EEZ_L1_RPC_URL`, the three keys,
and optionally `EEZ_DEPLOY_KEY`. The deployment RPC must remain external:
startup queries it before starting the embedded L1.

Checkpoint sync defaults to `https://checkpoint.chiadochain.net/`; set
`LH_CHECKPOINT_URL` to use another trusted source. Keep policy disabled for initial
bring-up unless another policy is part of the operation. For opt-in policy, follow
the [configuration guide](../configurable-settlement-policy.md): remove
`EEZ_MAX_BLOCKS_PER_BATCH`, set the required fee ceilings and scheduling rules,
and distinguish omitted settings from empty strings.

Expanded Compose configuration and environment dumps contain keys; keep them private.

#### Image selection

Defaults:

| Variable | Image |
| --- | --- |
| `EEZ_NODE_IMAGE` | `ghcr.io/eduadiez/rollupx-node:latest` |
| `EEZ_PROOF_SIGNER_IMAGE` | `ghcr.io/eduadiez/rollupx-proof-signer:latest` |
| `EEZ_DEPLOY_IMAGE` | `ghcr.io/eduadiez/rollupx-deploy:latest` |

Use `:latest` for [CI-validated main](https://github.com/eduadiez/rollupX/actions/workflows/ci.yml),
the same **full Git commit tag on all three images** for a fixed source revision,
or each image's `@sha256:<digest>` for exact bytes. A successful rebuild can refresh
a commit tag. Starting with `latest` can select an upgrade, and the three tags
update separately; pin references before broadcasting if setup may span a release.

Match the configuration checkout and protocol pin to the images' source/protocol
revisions; record them in the [inventory](#handoff-record). The deploy image
supplies matching contracts and tools; host builds and submodules are unnecessary.

#### Genesis allocations

The deploy image uses its bundled `/repo/genesis.json`, which contains funded
public development accounts. To inspect the selected image's allocations:

```bash
source .env
mkdir -p datadir
docker run --rm --pull always --platform linux/amd64 --entrypoint cat \
  "$EEZ_DEPLOY_IMAGE" /repo/genesis.json > datadir/base-genesis.json
chmod 644 datadir/base-genesis.json
```

Review the allocations in `datadir/base-genesis.json`. To deploy with changes, edit
that file and set
`EEZ_GENESIS_BASE=/absolute/path/to/checkout/datadir/base-genesis.json` in `.env`.
The deploy command's absolute-path mount makes that file available. Preserve the
EEZL2 predeploy entry; deployment regenerates its code and funds the configured
system address. Keep the base file mode `644`: generated genesis inherits it,
and signer UID 65532 must be able to read the public genesis. Changing genesis
after registration changes the rollup identity.
The host's tracked `genesis.json` is not used unless selected explicitly.

### 2. Download the L1 snapshot and create JWT

```bash
mkdir -p data
docker run --rm -v "$PWD/data/chiado-l1:/data" \
  ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
  download --chain chiado --minimal --datadir /data
openssl rand -hex 32 > data/jwt.hex
```

### 3. Deploy — broadcasts transactions

This spends gas and creates a fresh registry, proof system, manager, rollup **1**,
and L1 bridges, even if `deployments.env` already exists. It is not a restart
command. The deploy image checks genesis rendering and tools before broadcasting,
then aligns genesis time to the L1 deployment block and registers its **block hash**.
It uses bundled sources under `/repo`; do not mount your checkout over `/repo`.

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

The first mount keeps generated absolute paths valid on the host and in the
container. The `.env` mount preserves shell parsing; the receipts mount retains
`contracts/broadcast/` even on failure. The container runs as root; generated
files are root-owned and public genesis/profile files are readable by the signer
(UID 65532). Keep deployment output private because RPC credentials may appear.

`EEZ_DEPLOY_SKIP_SIMULATION=1` bypasses Foundry's pre-broadcast simulation when
public RPCs cannot serve its sampled block; broadcasting still occurs. Omit it
when simulation works reliably. Optional `EEZ_BLOCKSCOUT_URL` verification is
best effort and cannot undo deployment.

**Pass:** successful exit, completed `deployments.env`, `datadir/genesis.json`,
`datadir/genesis.profile.json`, and broadcast receipts.

**Failure:** provisional genesis files can exist before broadcasting; contracts
can exist before complete bindings are written. The script is not resumable.
Preserve receipts/output and inspect confirmed transactions, addresses, and the
deployer nonce before deciding how to continue. Do not blindly rerun.

## Start and verify

Required: completed bindings and genesis/profile, L1 snapshot, JWT, configuration,
and funded poster. For an existing rollup, first follow [restart](#restart-and-stop).
Startup can recreate services and begin spending poster funds.

```bash
bash scripts/chiado-up.sh
```

**Pass:** `chiado L2 is UP`, L2 height greater than 3, changed registry commitment,
and a new `BatchPosted` type-3 transaction whose versioned-hash-selected full blobs
are retrievable. Retain its transaction hash. A timeout leaves containers running;
use [troubleshooting](#troubleshooting).

Startup allows 90 Beacon polls (5-second sleeps plus HTTP timeouts) and 90 settlement
polls (10-second sleeps plus RPC time). An enabled policy with no due work or a long
interval can exceed that window; inspect policy instead of weakening it to obtain
the success banner.

### Optional workloads

Run funded traffic only when the operation includes it. Inspect the
[matrix driver](../../scripts/xchain-test.sh) before use: its funder must hold funds
on **both chains**, and it assumes public development accounts. The
[staged performance runner](../../scripts/run-chiado-staged-performance.sh) locks
shared wallets; `--check` is readiness-only and normal runs use `--no-verify`.
The lock is per protocol checkout: coordinate wallet use across worktrees/hosts
and do not bypass it with a concurrent upstream runner. See
[workload commands](../../README.md#exercise-it). Startup/throughput/receipts do
not validate cross-chain semantics or independent follower replay; record those
checks separately when required.

## Restart and stop

Use the recorded checkout, keys, bindings, genesis/profile, JWT, and datadirs.
Pin the recorded commit/digests in `.env` to keep the same runtime: Compose can
refresh `:latest` during `up`.
Do not rerun deployment, regenerate genesis/JWT, or use `make clean-deploy` or
`make clean-l2`. Check that configured paths still point to the retained data:
startup creates missing L2/Lighthouse directories. Missing or empty databases
require [recovery assessment](#recovery-and-upgrades) before startup.

Before stopping, pause submissions and record unsettled transaction hashes,
canonical receipts, and sender nonces. Pending requests/retry counters are in memory.
Restart reconciles against canonical L1; with settlement policy enabled it can retain
a validated pure-L2 prefix, but unowned Sync blocks and their descendants are rolled back.
After startup, reconcile receipts/nonces before resubmitting unsettled work.
A datadir backup does not retain volatile request ownership.

**Stop changes running services; preserves bind-mounted data:**

```bash
bash scripts/teardown-chiado.sh
```

**Pass:** the script confirms all three containers stopped. The signer's grace
period is up to 11 minutes to drain admitted requests. If teardown fails, inspect
remaining containers before proceeding; preserve files and data.

Run [Start and verify](#start-and-verify) to start stopped services or apply `.env`
changes; preserve the previous configuration. For a process-only restart of
existing containers, `docker compose restart` retains their images and environment;
it does not apply configuration changes. Recheck [health](#health-checks) afterward.

## Troubleshooting

Use these diagnostics when startup fails or a running service stalls. Begin with
[health and logs](#health-checks); do not delete state to clear an unexplained error.

| Failure | First check and action |
| --- | --- |
| Missing configuration/bindings | Use `.env.chiado.example` as the configuration reference. A missing `deployments.env` requires completing deployment or restoring its original bindings; do not delete data. |
| Image pull fails | Check network access, the package/tag/digest in `.env`, and CI promotion results. For `:local`, follow [local development](#local-source-development). |
| Image revisions differ | Inspect [image labels](#handoff-record). Select the same validated full commit on node/signer/deploy images and the matching checkout; follow [upgrade checks](#recovery-and-upgrades). |
| Genesis generation fails | Read the first deployment error; allow the deploy image's Solidity compiler download. For a custom base, check the EEZL2 allocation and path. Initial render/tool checks precede broadcasts; later failures require receipt reconciliation. |
| Missing/mismatched genesis | Check [deployment identity](#deployment-identity) before starting. Restore exact bindings/genesis, including timestamp; a partial deployment requires receipt reconciliation. |
| Deploy failed before bindings were written | Inspect retained deployment output and `contracts/broadcast/`. Use `cast receipt` for each broadcast transaction and `cast nonce` for the deployer against the configured deployment RPC. Reconcile confirmed addresses/nonces before another deploy. |
| Signer unhealthy/rejecting | `docker compose logs --tail=100 eez-proof-signer`; check [keys/genesis](#deployment-identity), genesis mode (`stat -c '%a %n' datadir/genesis.json`, expected 644), UID 65532 readability, and L1 chain config `chiado`. |
| Port in use/database locked | `ss -lntup`; `docker ps -a --format '{{.Names}}'`; `docker compose ps`. Identify the owning process/deployment; a worktree/project name cannot isolate this profile. Never share a writable datadir. |
| L1/Beacon cannot catch up | Check [sync/peers and L1 agreement](#health-checks), shared JWT, checkpoint source (`LH_CHECKPOINT_URL`), snapshot backfill, and checked-in Fulu config. `configs/chiado/bootstrap_nodes.yaml` must be nonempty and match `bootnodes.yaml`. |
| Beacon has no peers | `curl -fsS http://127.0.0.1:6152/eth/v1/node/identity \| jq '.data'`; inspect advertised `ip`/`udp`, firewall, NAT, and UDP 9205 reachability. See [discovery change](#discovery-change--recreates-lighthouse-only). |
| Blob retrieval fails | Use the [blob diagnostic](#blob-diagnostic--read-only) with the settlement's slot and versioned hash. Check retention, sync, Fulu config, and semi-supernode mode. |
| Builder probe fails | Check `EEZ_L1_BUILDER_RPC_URL` reachability and `eth_sendBundle` support. Startup sends an empty bundle; JSON-RPC error `-32601` means the method is unavailable. Probe success does not prove inclusion. |
| L2 advances without settlement | Check [L1 agreement](#health-checks), then [poster funding and settlement](#settlement-diagnostic). Inspect node/signer logs for builder inclusion, proof failure, policy due-work, fee ceilings, and gas/blob/timing deferrals. |
| Simulation cannot fetch a block | Verify deployment RPC freshness. Skip-simulation bypasses that phase only; inspect prior broadcasts before retrying. |

### Health checks

```bash
docker compose ps
docker compose logs --tail=100 eez-node eez-proof-signer lighthouse
cast rpc eth_syncing --rpc-url http://127.0.0.1:18645
curl -fsS http://127.0.0.1:6152/eth/v1/node/syncing | jq '.data'
curl -fsS http://127.0.0.1:6152/eth/v1/node/peer_count | jq '.data'
```

Expected: three running services; signer healthy; `eth_syncing=false`; Beacon
`is_syncing=false`, `is_optimistic=false`, `el_offline=false`, and connected peers.
Signer health establishes TCP readiness, not successful attestation.

If the embedded L1 appears behind or disagrees with the external RPC, compare the
same block on both:

```bash
source .env
tip="$(cast block-number --rpc-url "$EEZ_L1_RPC_URL")"
cast block "$tip" --rpc-url "$EEZ_L1_RPC_URL" --json | jq '{number, timestamp, hash}'
cast block "$tip" --rpc-url http://127.0.0.1:18645 --json | jq '{number, timestamp, hash}'
```

Hashes should match at a recent, advancing tip. Retry after catch-up or a reorg.

### Deployment identity

Use before starting restored/modified state, or when the signer rejects keys or
genesis. Use the recorded deploy image and compare against the retained bindings:

```bash
source .env
source deployments.env
docker run --rm --platform linux/amd64 --entrypoint eez-genesis-block-hash \
  -v "$PWD/datadir/genesis.json:/genesis.json:ro" "$EEZ_DEPLOY_IMAGE" /genesis.json
docker run --rm --platform linux/amd64 --entrypoint eez-genesis-state-root \
  -v "$PWD/datadir/genesis.json:/genesis.json:ro" "$EEZ_DEPLOY_IMAGE" /genesis.json
cast wallet address --private-key "$EEZ_PROOF_SIGNER_KEY"
cast call "$EEZ_ECDSA_PROOF_SYSTEM_ADDRESS" 'signer()(address)' --rpc-url "$EEZ_L1_RPC_URL"
cast wallet address --private-key "$EEZ_L2_SYSTEM_KEY"
```

The two hashes must equal `EEZ_INITIAL_BLOCK_HASH` and `EEZ_L2_GENESIS_STATE_ROOT`.
Both the proof-key address and on-chain signer must equal `EEZ_ATTESTER_ADDRESS`;
the system-key address must equal `EEZ_L2_SYSTEM_ADDRESS` (case-insensitive).
Compare genesis/profile file digests with the [inventory](#handoff-record).
If a restored root-owned public genesis is unreadable by signer UID 65532,
correct its mode with `sudo chmod 644 datadir/genesis.json`.

### Settlement diagnostic

For a stalled poster, inspect its gas balance (wei) and confirmed nonce:

```bash
source .env
poster_address="$(cast wallet address --private-key "$EEZ_L1_POSTER_KEY")"
cast balance "$poster_address" --rpc-url "$EEZ_L1_RPC_URL"
cast nonce "$poster_address" --rpc-url "$EEZ_L1_RPC_URL"
```

Compare the nonce with pending transactions and poster logs. To identify the L1
transaction for the registry's current L2 commitment:

```bash
source .env
source deployments.env
commitment="$(cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' \
  "$EEZ_ROLLUP_ID" --rpc-url http://127.0.0.1:18645 | sed -n '2p' | tr -d '[:space:]')"
cast rpc eez_getSettlementByL2Block "$commitment" --rpc-url http://127.0.0.1:18688 | jq .
```

After settlement, expect `canonicalL2=true`, an `l1TransactionHash`, and an
`l2Range`; `l2Finalized` can remain false until finality advances. A `null` result
or a latest speculative L2 block does not establish settlement. Before the first
settlement the registry commitment is `EEZ_INITIAL_BLOCK_HASH`. See the
[correlation RPC contract](../../README.md#l1l2-settlement-correlation-rpc).

### Discovery change — recreates Lighthouse only

If the advertised ENR lacks the externally reachable endpoint, set
`LH_ENR_ADDRESS` and `LH_ENR_UDP_PORT` in `.env` to the public address and
mapped discovery port (default UDP **9205**). These flags do not configure NAT.
Keep both unset for automatic advertisement when that works. Then:

```bash
docker compose up -d --no-deps lighthouse
curl -fsS http://127.0.0.1:6152/eth/v1/node/peer_count | jq '.data'
curl -fsS http://127.0.0.1:6152/eth/v1/node/syncing | jq '.data'
```

Allow reconnection; require connected peers and successful sync, not just HTTP.

### Blob diagnostic — read-only

Supply the settlement transaction hash from startup output or `l1TransactionHash`
in the correlation response. Inspect the mined transaction first:

```bash
source .env
read -r -p 'Settlement transaction hash: ' settlement_tx
tx="$(cast rpc eth_getTransactionByHash "$settlement_tx" --rpc-url http://127.0.0.1:18645)"
jq '{type, blockHash, blobVersionedHashes}' <<<"$tx"
```

Expected: type `0x3` (or `0x03`), a non-null `blockHash`, and at least one versioned
hash. Derive its Beacon slot and query the first hash; change `[0]` to inspect
another hash from the transaction:

```bash
beacon="${EEZ_L1_BEACON_URL%/}"
block="$(jq -er '.blockHash' <<<"$tx")"
timestamp="$(cast block "$block" --rpc-url http://127.0.0.1:18645 --json | jq -er '.timestamp')"
genesis_time="$(curl -fsS "$beacon/eth/v1/beacon/genesis" | jq -er '.data.genesis_time')"
seconds_per_slot="$(curl -fsS "$beacon/eth/v1/config/spec" | jq -er '.data.SECONDS_PER_SLOT')"
slot=$(( (timestamp - genesis_time) / seconds_per_slot ))
versioned_hash="$(jq -er '.blobVersionedHashes[0]' <<<"$tx")"
curl -fsS --max-time 30 --get --data-urlencode "versioned_hashes=$versioned_hash" \
  "$beacon/eth/v1/beacon/blobs/$slot" | jq '.data | map(length)'
```

Expected: `[262146]`, the character count of one full hex-encoded blob. An empty
array means that selected blob was not returned; `blobs/head` may legitimately be
empty and cannot diagnose an older settlement. Startup checks every committed
blob; this diagnostic checks one retrieval, not KZG proofs or replay.

## Recovery and upgrades

For the ordered restore/replay setup, use the
[existing-rollup Composer procedure](chiado-existing-rollup.md).

1. Preserve the [inventory](#handoff-record), previous images/configuration, and
   a consistent backup. Stop database writers for a filesystem copy, or use a
   documented database-consistent snapshot method; copying live files is insufficient.
2. Establish history availability before rebuilding L2 state. The semi-supernode
   serves live blobs within its retention window. Older recovery requires a durable
   archival Beacon source or a validated protocol snapshot/bootstrap procedure.
   An execution snapshot does not provide missing blob history. Do not delete the
   current L2 database until the recovery path has been tested.
3. For upgrades, check node/signer/protocol compatibility, stored-state formats,
   configuration changes, and rollout order. Select the intended node/signer
   references in `.env` and matching configuration checkout. Verify source labels
   and retain the previous `.env`, image digests, checkout, and backups.
   A binary downgrade works only if old code can read the resulting data and speak
   the deployed protocol. Repeat startup, semantic, and replay checks required by
   the change; record which actually ran.
4. For host/checkout moves, restore the exact keys, JWT, genesis/profile, bindings,
   compatible images and consistent data. Reconcile absolute paths in
   `deployments.env` with `FRESH_GENESIS` and the restored files, preserving all
   addresses/commitments. Check [deployment identity](#deployment-identity) before starting.

New contracts or a changed genesis create a different deployment; neither migrates
user state automatically. Missing bindings after a partial deploy require receipt
reconciliation, not the ordinary restart path. Do not claim recovery beyond retained
history until the complete recovery procedure has been exercised.

## Local source development

Use an isolated deployment whose ports/names/datadirs you own. Initialize protocol
sources and build the three images from the same checkout:

```bash
git submodule update --init --recursive eez-core-protocol
docker build -t eez-node:local .
docker build -f Dockerfile.signer -t eez-proof-signer:local .
docker build -f testing/kurtosis/Dockerfile.deploy \
  --build-arg EEZ_NODE_IMAGE=eez-node:local -t eez-deploy:local .
```

Set these references in `.env`:

```dotenv
EEZ_NODE_IMAGE=eez-node:local
EEZ_PROOF_SIGNER_IMAGE=eez-proof-signer:local
EEZ_DEPLOY_IMAGE=eez-deploy:local
```

Use the same deployment/start procedures, with `--pull never` on the deployment
command to use the local image. Builds require BuildKit and Rust build capacity.
Local Dockerfiles do not supply CI provenance labels; record the
commit/dirty patch and image IDs. Rebuilding a tag changes its bytes; assess the
next restart as an upgrade.

## Handoff record

Maintain the operational inventory privately; commit only sanitized evidence using
the [audit template](../audits/TEMPLATE.md), linked to this runbook:

| Record | Required contents |
| --- | --- |
| Source/runtime | UTC date, operator, host/checkout reference, fork commit and dirty patch identity, protocol pin, CI run URL, node/signer/deploy references, resolved digests and local IDs, Lighthouse tag/resolved digest, tool versions |
| Chain identity | L1/L2 chain IDs, registry/rollup/manager, proof system and attester, system/bridge addresses, deploy block, transaction receipts |
| State/files | Genesis/profile SHA-256 digests, initial block hash and state root; retained locations of bindings, data, JWT, and backups |
| Credentials | Private secret-manager/access references; never commit keys or authenticated URLs |
| Configuration | Timing, settlement/fee/blob policy, endpoint ownership, checkpoint/archival dependencies, and retention limits |
| Results/next actions | Current health, canonical settlement transaction/range, pending requests, executed semantic/restart/recovery checks, failures and untested assumptions |

After the images have run, collect image identities and file digests:

```bash
source .env
source deployments.env
docker image inspect --format '{{.Id}} {{index .Config.Labels "org.opencontainers.image.revision"}} {{index .Config.Labels "io.eez.protocol.revision"}} {{json .RepoDigests}}' \
  "$EEZ_NODE_IMAGE" "$EEZ_PROOF_SIGNER_IMAGE" "$EEZ_DEPLOY_IMAGE"
git rev-parse HEAD
git ls-tree HEAD eez-core-protocol
sha256sum "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PROFILE_PATH"
```

Published image source labels must match each other and the checkout; protocol
labels must match its gitlink. Reconcile mismatches through
[upgrade checks](#recovery-and-upgrades), preserving local edits and deployed state.

Keep raw logs/databases outside Git. For shared evidence, retain a retrievable
artifact location and digest; an ignored local path is not available to another
operator. Update this procedure with verified corrections and preserve the dated
audit as the record of the particular execution.
