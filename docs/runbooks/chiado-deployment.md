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
| Source and protocol revisions | [Offline-history implementation record](../audits/2026-10-22-l1-history-offline-proving/README.md) |
| Local checks | Source/configuration consistency, genesis/profile checks, isolated process tests and offline proving; exact results and outstanding checks are in that record |
| Pending | Published images for this profile, public fresh-host deployment and live restore rehearsal; record execution in a dated [audit](../audits/TEMPLATE.md) |

The [2026-09-23 network replacement](../audits/2026-09-23-events-network/README.md)
records the live deployment of protocol `820e50f` to a new registry and L2 chain
6291, including backup boundaries, active configuration locations and semantic
checks. That dated record predates the offline-history profile and does not
validate deploying or restoring this revision. This revision creates new networks;
read the [compatibility boundary](#offline-v2-proving-and-fresh-network-boundary)
before choosing images or starting services.

## Select the procedure

| Operation | Procedure | Required starting state |
| --- | --- | --- |
| Create a new registry and rollup | [First deployment](#first-deployment), then [start and verify](#start-and-verify) | Unused deployment paths; no existing stack on these names/ports |
| Add or replace a Composer on an offline-profile L2 | [Existing-rollup Composer](chiado-existing-rollup.md) | Original deployment files/keys, matching profile/runtime, and L2 backup or complete replay history |
| Start/restart the same rollup | [Restart and stop](#restart-and-stop), then [start and verify](#start-and-verify) | Original bindings, genesis, keys, JWT, images, and datadirs |
| Move hosts, replace a compatible runtime, or recover state | [Recovery and upgrades](#recovery-and-upgrades) | Same offline profile, deployment inventory, preserved state, and an identified recovery source |

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
| Builder | One or more endpoints implementing `eth_sendBundle`, comma-separated in `EEZ_L1_BUILDER_RPC_URL`; an ordinary execution RPC is insufficient. Each bundle goes to every listed relay ([fan-out contract](../configurable-settlement-policy.md#builder-relay-fan-out)) |
| Beacon | Packaged Lighthouse **v8.1.3**, checked-in `configs/chiado`, `--semi-supernode`, full-blob API, and `EEZ_L1_BLOB_SIDECAR_ENCODING=eip7594`; built-in Chiado config in this image leaves Fulu disabled |

### Ports and service ownership

Run `ss -lntup` and `docker ps -a --format '{{.Names}}'` before first deployment.
The fixed containers are `eez-node-chiado`, `eez-proof-signer-chiado`, and
`eez-chiado-lighthouse`. Resolve existing owners before proceeding.

| Default port | Service/access |
| --- | --- |
| 18645 / 18646 | Embedded Chiado L1 HTTP / WebSocket RPC |
| 18688 / 18689 | L2 HTTP / WebSocket RPC |
| 18999 / 18998 | Composer L1 / L2 fronts; all `eez_*` methods and wallet discovery; ordinary Ethereum RPC calls proxy to the source chain |
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

Generate dedicated keys with your wallet tooling. Keep the poster separate from
the proof signer. The L1 registry and Rollup are deployed behind transparent
proxies, as in the pinned protocol's
[deployment procedure](../../eez-core-protocol/deployment/README.md). Their
ProxyAdmin owners, `EEZ_UPGRADE_OWNER` and `EEZ_ROLLUP_UPGRADE_OWNER`, default to
the deployer and can replace the implementations; set them to dedicated
accounts, such as multisigs, when the deployer should not hold that authority. Native L2 system transactions use the reserved `0x76` sender;
they have no signing key or genesis funding.
A separate deploy key avoids nonce competition with an active poster. Do not
share a posting wallet with concurrent workloads. Fund deployer and poster before
broadcasting; the [environment example](../../.env.chiado.example) identifies a
Chiado faucet. Funding must cover deployment and ongoing settlement gas.

| File/variable | Default location and ownership |
| --- | --- |
| `.env` | Private RPC/keys, node/signer/deploy images, and Compose settings; copied from `.env.chiado.example`, used by deployment and startup; preserve |
| `deployments.env` | Generated public contract/rollup bindings and genesis/profile paths; preserve |
| `EEZ_L2_FEE_RECIPIENT` | Required in `.env`: coinbase of every Live and Sync L2 block, which receives priority fees. Compose refuses to start while it is unset or empty; set the zero address to choose the historical zero coinbase deliberately. Not a consensus value: it can change at any restart, and followers take each block's published beneficiary. |
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

Edit `.env`: set a synchronized external Chiado `EEZ_L1_RPC_URL`, the poster and proof-signer keys,
and optionally `EEZ_DEPLOY_KEY`. The deployment RPC must remain external:
startup queries it before starting the embedded L1.

Checkpoint sync defaults to `https://checkpoint.chiadochain.net/`; set
`LH_CHECKPOINT_URL` to use another trusted source. The env example selects the
live Chiado policy and timing preset below. Review its fee ceilings and timing
for the intended host/workload. Existing private `.env` files retain their own
values; update them deliberately rather than replacing keys, bindings, or paths.

| Setting | Packaged Chiado preset |
| --- | --- |
| Settlement | Enabled; pure L2 `always`; general trigger at 300 unsettled L2 blocks; fullness trigger `off` |
| Admission | `EEZ_MAX_USER_TXS_PER_BUNDLE=auto` |
| Preparation position / delay | P=2 / 0 ms (required position; no legacy preparation setting) |
| Proof seed / submission slack | 750 / 700 ms |
| Execution / blob fee ceiling | 500,000,000,000 / 10,000,000,000 native base units per gas |
| Priority fee | Network estimate plus 2000 bps, with a 1,000,000,000 minimum |
| Signer request capacity | 2048 blocks |
| L2 HTTP APIs | `eth,net,web3,debug,trace` |

The history update delay is fixed in genesis by
`EEZ_L2_HISTORY_UPDATE_OFFSET` (q=2 by default, 0..4 for this timing).
It is separate from the Composer's preparation delay and provides delivery
headroom, not finality. Preserve the original genesis schedule on restart.
Runtime images must implement this exact offline profile; an older published
image does not become compatible by accepting these environment names.
Restrict the HTTP RPC, including debug/trace, to intended clients. For
policy-disabled operation, use the current [settlement contract](../configurable-settlement-policy.md).
Enabled policy requires `EEZ_MAX_BLOCKS_PER_BATCH` to remain absent.

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

The deploy image bundles two base genesis files:

- `/repo/genesis.json`, the default production base. It contains only the EEZL2
  (proxy, implementation and ProxyAdmin) and EIP-2935 predeploys and allocates no
  ETH: all L2 ETH is minted against value locked on L1. Its default L1 reference
  is Ethereum mainnet's genesis block and its L2 chain ID is 696990; deployment
  replaces the reference with the live finalized L1 block.
- `/repo/genesis.dev.json`, which additionally funds the public development
  accounts used by the staged workloads. `.env.chiado.example` selects it with
  `EEZ_GENESIS_BASE=/repo/genesis.dev.json` for the Chiado test network.

To inspect the selected base's allocations:

```bash
source .env
mkdir -p datadir
docker run --rm --pull always --platform linux/amd64 --entrypoint cat \
  "$EEZ_DEPLOY_IMAGE" "${EEZ_GENESIS_BASE:-/repo/genesis.json}" \
  > datadir/base-genesis.json
chmod 644 datadir/base-genesis.json
```

Review the allocations in `datadir/base-genesis.json`. To deploy with changes, edit
that file and set
`EEZ_GENESIS_BASE=/absolute/path/to/checkout/datadir/base-genesis.json` in `.env`.
The deploy command's absolute-path mount makes that file available. Preserve the
EEZL2 proxy entry at `0x4B2760F84B83d6b3829E30A7573B0172B6d6402C`; deployment
regenerates the proxy, its implementation at
`0x4C3D971b1079E58843D0E1af155A2e54b9E54dbA` and its ProxyAdmin with the fixed
native system address and `EEZ_L2_UPGRADE_OWNER`, which requires no prefunding.
That owner upgrades EEZL2 with an L2 transaction, so it needs L2 gas; coordinate
any behavioural upgrade with a matching node/signer release. Keep the base file mode `644`: generated genesis inherits it,
and signer UID 65532 must be able to read the public genesis. Changing genesis
after registration changes the rollup identity.
The host's tracked genesis files are not used unless selected explicitly.

Every L2 block burns the balances of EEZL2 and the native system address before
its first transaction, so neither account holds ETH at a block start; see the
[signer specification](../../crates/eez-proof-signer/SPEC.md). This changes
execution: nodes, followers and signers from before this rule cannot replay a
network created with it, and it is not a binary-only upgrade for an existing
network.

### 2. Download the L1 snapshot and create JWT

```bash
mkdir -p data
docker run --rm -v "$PWD/data/chiado-l1:/data" \
  ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
  download --chain chiado --minimal --datadir /data
openssl rand -hex 32 > data/jwt.hex
```

### 3. Deploy — broadcasts transactions

This spends gas and creates a fresh registry, proof system, manager and rollup
**1**. Existing bindings or genesis/profile output files cause rejection before
broadcast. The registry and manager are
proxies; `deployments.env` also records their implementations and ProxyAdmins.
Use only the proxy addresses in configuration and integrations. It is not a restart
command. Before broadcasting, the deploy image reads a finalized L1 reference,
initializes permanent history and RANDAO from it, and records its exact chain/block
identity under `config.rollupX`. L2 genesis starts one Sync interval after that
reference. Chiado requires 1-second L2 blocks and five positions per Sync interval.
`EEZ_L2_HISTORY_UPDATE_OFFSET` fixes the delay at genesis (default 2, allowed 0–4);
it is read from the mounted `.env` by the deployment script. Missing finalized
data or conflicting timing aborts
before a broadcast. The image checks genesis rendering and tools, then registers
that same genesis **block hash** without a deployment-time timestamp change.
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

### Optional explorers

After the rollup is running, use the [explorer runbook](explorers.md) to add
L2 Blockscout, Chiado Blobscan, or both. These use a separate Compose project
and `.env.explorers`; they are not built, deployed, or tested by CI.

Use the actual genesis selected by `FRESH_GENESIS` in `.env` for Blockscout.
For Blobscan, use the desired first L1 block, normally
`EEZ_REGISTRY_DEPLOY_BLOCK` from `deployments.env`, and endpoints retaining that
history. Preparation reads the L2 chain ID and derives Blobscan's starting slot.
Supply reachable node hostnames for the L2 RPC, L1 RPC, and Beacon API during
preparation. Explorers use a bridge network and service names internally.
Blockscout connects directly to port 18688, with `trace` enabled and access
allowed from the explorer Docker network. The preset's Beacon listener is
loopback-only; see the explorer runbook's endpoint requirements before using it.

Explorer startup/shutdown is independent of the node stack. Preserve its env
file, databases, and Blobscan origin volume. Explorer readiness and indexed
L2 confirmations do not establish completed catch-up or canonical L1 settlement.

### Optional wallet bridge UI

The wallet UI requires the protocol's `src/periphery/Bridge.sol` contract on both
chains. Protocol deployment does not create any bridge contract.

Deploy the pinned protocol bridge with the same address on both chains, initialize
L1 with the current registry and rollup ID 0, and L2 with `EEZL2_ADDRESS` and the
actual rollup ID. Preserve the bridge admin key and deployment receipts. Keep UI
bindings separate from the workload bindings. Updating the UI does not require a
Composer restart or a data reset.

Before publishing, check contract bytecode and call `manager()` and `rollupId()`
through both public RPC routes. Verify the managers belong to this deployment;
a nonzero manager from an older network is insufficient. Exercise both bridge
readiness checks in the browser and simulate `bridgeEther` through both Composer
fronts. Simulation/readiness does not establish canonical cross-chain inclusion.

If the page says `Unable to check the bridge on L1. Retrying…`, reproduce
`eth_getCode` and `eth_call` with selector `0x481c6a75` (`manager()`) against the
configured address. A revert with existing code can indicate the wrong contract,
not a connectivity failure. Preserve the deployed UI image and other environment
values when correcting its Compose bindings, and verify again after recreation.

### Optional workloads

For an explicitly authorized delayed-cutoff canary with continuous ordinary
traffic and bidirectional waves, use the
[mixed-load validation procedure](chiado-mixed-validation.md). It includes
canonical, semantic, blob and independent follower checks without restarting
the node or signer.

Run funded traffic only when the operation includes it. Inspect the
[matrix driver](../../scripts/xchain-test.sh) before use: its funder must hold funds
on **both chains**, and it assumes public development accounts. The
[staged performance runner](../../scripts/run-chiado-staged-performance.sh) locks
shared wallets; `--check` is readiness-only and normal runs use `--no-verify`.
Add `--verify` to run per-job semantic assertions after the measured mining phase,
under the same wallet lock. This does not itself run an independent follower.
`EEZ_STAGED_SCALE` must resolve to an integer from 1 through 10 after loading
`DEVNET_ENV`. Profile assignments take precedence over inherited environment
values. The helper supports fresh runs only; the diagnostic `--resend` shortcut
was removed because saved endpoint URLs do not identify a replaced network.
Recovering an older run requires a separate assessment of its original network,
signed transaction identities and current source nonces before any resubmission.
The lock is per protocol checkout: coordinate wallet use across worktrees/hosts
and do not bypass it with a concurrent upstream runner. See
[workload commands](../../README.md#exercise-it). Startup/throughput/receipts do
not validate cross-chain semantics or independent follower replay; record those
checks separately when required.

## Restart and stop

The base node Compose file writes bounded posting lifecycle logs under
`POSTING_LOGS_DIR` (default `./data/posting-logs`). Set the node's `.env` to the
existing log root when retaining monitoring history. The normal startup script
preserves this mount and logging configuration. The old posting overlay remains
compatible but is no longer required. Point the optional indexer collector at
the same host directory as described in the
[monitoring setup](../../monitoring/indexer/README.md).

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

Composer forwards graceful shutdown to both embedded engines. Their persistence
workers must finish cleanup before termination is acknowledged; see the
[shutdown backport](../../crates/vendor/reth/README.md). A forced stop or logged
persistence/shutdown failure still requires recovery assessment. The existing
CLI shutdown budget is unchanged, and this does not make unsettled requests
durable.

The fresh-network offline profile uses the
[portable validation identity](../../crates/eez-control-rpc/VALIDATION_IDENTITY.md).
Schema 6 replaces the earlier lockfile fingerprint. Deploy Composer and signer
with matching protocol versions and local network/deployment configuration;
schema 5/6 pairs fail the startup capability check. Clear retained prover sessions
when the identity changes. Lockfile-only changes no longer automatically change
the identity, but still require dependency/semantic review. This handshake update
requires no contract, genesis, wire-field or database migration. It does not make
the broader fresh-network profile an upgrade for existing Chiado contracts.
Rollback requires compatible endpoints and does not restore volatile queue state.

**Stop changes running services; preserves bind-mounted data:**

```bash
bash scripts/teardown-chiado.sh
```

**Pass:** the script confirms all three containers stopped. The signer's grace
period is 30 seconds, matching the live deployment. An admitted request still
running at that limit can be killed; increase `stop_grace_period` if the operation
requires draining the full request deadline. If teardown fails, inspect
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
| Fee quote reports `L1 fee sample is too old` | Check the embedded head timestamp against the external RPC and Beacon sync status. The quote may correctly reject a stalled upstream; do not relax its freshness check to hide that condition. See [stalled consensus client](#stalled-consensus-client). |
| Beacon has no peers | `curl -fsS http://127.0.0.1:6152/eth/v1/node/identity \| jq '.data'`; inspect advertised `ip`/`udp`, firewall, NAT, and UDP 9205 reachability. See [discovery change](#discovery-change--recreates-lighthouse-only). |
| Blob retrieval fails | Use the [blob diagnostic](#blob-diagnostic--read-only) with the settlement's slot and versioned hash. Check retention, sync, Fulu config, and semi-supernode mode. |
| Builder probe fails | Check reachability and `eth_sendBundle` support of each `EEZ_L1_BUILDER_RPC_URL` entry. Startup sends an empty bundle to every listed relay and names them `builder[<index>] (<host>)`; JSON-RPC error `-32601` means the method is unavailable. Startup fails only when no relay passes; failing additional relays print a warning. Probe success does not prove inclusion. |
| L2 advances without settlement | Check [L1 agreement](#health-checks), then [poster funding and settlement](#settlement-diagnostic). Inspect node/signer logs for builder inclusion, proof failure, policy due-work, fee ceilings, and gas/blob/timing deferrals. |
| Simulation cannot fetch a block | Verify deployment RPC freshness. Skip-simulation bypasses that phase only; inspect prior broadcasts before retrying. |

### Stalled consensus client

An observed Chiado incident left Lighthouse reporting empty slots with peers
while its execution hash stayed fixed and the public chain advanced. It later
reported a growing sync distance. Restarting the existing Lighthouse container
restored catch-up; no configuration, image or database replacement was needed.
The [deployment record](../audits/2026-10-15-simplified-fees/README.md) retains
the before/after evidence. This is a verified recovery for that incident, not a
diagnosis of every stale-head error.

After recording logs, checking L1 agreement and confirming this stalled state,
an authorized service recovery can restart only the existing client:

```bash
docker compose restart lighthouse
```

Repeat the health checks below. Require a fresh embedded head,
`is_syncing=false`, `is_optimistic=false` and `el_offline=false` before treating
quotes or settlement as recovered. A running container or `eth_syncing=false`
alone is insufficient. Preserve existing data and JWT throughout.

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
test "${EEZ_L2_SYSTEM_ADDRESS,,}" = 0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076
```

The two hashes must equal `EEZ_INITIAL_BLOCK_HASH` and `EEZ_L2_GENESIS_STATE_ROOT`.
Both the proof-key address and on-chain signer must equal `EEZ_ATTESTER_ADDRESS`;
`EEZ_L2_SYSTEM_ADDRESS` must equal the fixed native address (case-insensitive).
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
cast rpc eez_getSettlementByL2Block "$commitment" --rpc-url http://127.0.0.1:18998 | jq .
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

This procedure applies only to networks created with the current
[offline-history profile](#offline-v2-proving-and-fresh-network-boundary).
It supports replacing a compatible runtime, moving hosts and restoring the same
network. It provides no conversion of an earlier Chiado deployment. Existing
networks keep their own matching release, contracts, genesis and data.

For ordered restore/replay setup, use the
[existing-rollup Composer procedure](chiado-existing-rollup.md).

1. Pause external ingress and account for accepted work before stopping writers.
   An empty cross-chain queue does not establish that ordinary speculative L2
   transactions have settled. Preserve the [inventory](#handoff-record), previous
   images/configuration and a consistent backup. Stop database writers for a
   filesystem copy, or use a documented database-consistent snapshot method.
2. Establish history availability before rebuilding L2 state. Replay needs every
   produced L1 header between each authenticated history endpoint and anchor,
   plus the canonical transactions/logs and complete blobs in its scan range.
   The semi-supernode's live retention and an execution snapshot do not guarantee
   those historical blobs. Obtain archival coverage or a complete same-profile
   L2 backup; retain the original database until recovery has been validated.
3. Check the proposed node/signer/protocol identity, local L1 rules, genesis
   profile, configuration and stored-state formats. The v2 capability preflight
   rejects incompatible validation identities. Select matching image references
   and checkout; preserve the previous `.env`, image digests and backups. This
   feature has no alternate proving profile or database migration.
4. For host/checkout moves, restore the exact keys, JWT, genesis/profile, bindings
   and consistent data into separately owned paths. Reconcile absolute paths in
   `deployments.env` with `FRESH_GENESIS` and the restored files, preserving all
   addresses and commitments. Check [deployment identity](#deployment-identity)
   before starting. The signer needs the local profiles and signing key, with
   no L1 or L2 RPC configuration.
5. Repeat startup, canonical settlement and independent replay checks against
   the new executable identities. If fees are enabled, verify current nonce
   quotes, signed admission and the configured cost floor on each enabled
   direction. Follow the owning [fee/settlement contract](../configurable-settlement-policy.md)
   for parameter changes and [L1 refund qualification](#enabling-l1-fee-recovery)
   for builder changes. Keep ingress closed until the required checks pass.

Rollback is possible only to a binary that understands this deployed profile and
its resulting state. Restoring a database cannot reverse L1 settlements: reconcile
canonical progress after the backup before admitting new work. Pending requests,
retry counters and signer sessions remain volatile; clients may need to resubmit
unsettled work. Database persistence does not make these queues durable.

New contracts or a changed genesis create a different network; neither migrates
user state. Missing bindings after a partial deployment require receipt
reconciliation, not the ordinary restart path. Dated deployment audits describe
their recorded revisions and do not validate this profile's public deployment or
backup restoration. Record the exact checks exercised and any missing history.

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
| Chain identity | L1/L2 chain IDs, registry/rollup/manager proxies with their implementations, ProxyAdmins and upgrade owners, proof system and attester, system/bridge addresses, deploy block, transaction receipts |
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

## Fee configuration

Keep fees on the Composer and its clients; the signer does not consume fee-policy
settings. Use the current [configuration contract](../configurable-settlement-policy.md)
for units, defaults, precedence and removed inputs. Apply changes to every
effective source, including Compose overrides, without printing rendered secrets.
Obsolete settings cause rejection and must not be retained as empty placeholders.

Before changing an operating same-profile network, drain or explicitly account
for accepted work, preserve stopped state and the prior configuration, and use the
[recovery procedure](#recovery-and-upgrades). Verify compact/debug quotes, signed
fee rejection, canonical settlement and refund observations for each enabled
direction. A configuration change does not qualify a new builder route or make
an earlier protocol deployment compatible.

### Enabling L1 fee recovery

Leave `EEZ_FEES_L1_TO_L2_ENABLED` unset or `false` for public traffic until each configured builder has passed the
[L1 refund profile](../configurable-settlement-policy.md#l1-builder-refund-profile)
on this network with a real native-blob post, signed source and canonical payout.
The startup bundle API probe does not test refund support. Public builder docs
or a successful private Kurtosis test do not qualify the Chiado route.

Qualification on an existing network uses a controlled canary: pause public
transaction ingress, drain accepted work, preserve stopped state and configure
the candidate with the refund policy before submitting operator test sources
through the local front. Keep public ingress paused until canonical payments and
replay are verified. This temporary test configuration does not qualify the route
by itself; retain failed submissions and receipts before recovery or rollback.

Use a dedicated plain EOA recipient, keep the existing cost policy and L2 fee
settings, and drain accepted work before enabling the L1 policy. Both fronts
use `eez_quoteFee` and `eez_debugQuoteFee`; only the source chain and recovery
mechanism differ. The SDK needs the expected L1 chain ID and `/composer/l1`.
The matching dashboard uses `EEZ_UI_L1_FEES_ENABLED=true` and its independent
`EEZ_UI_L1_FEE_MAX_TOTAL_WEI` reservation cap; retain the existing L2 settings.
Older clients that omit the L1 fee quote can be rejected as underfunded after
enforcement is enabled. The compact response format is unchanged.
Verify both transaction formats, source/target effects, native blob evidence,
independent follower replay and the canonical `refund_status = qualified` event.

The [October 15 qualification](../audits/2026-10-15-l1-fee-refunds/README.md)
records an earlier deployment's checks. It does not qualify this offline-profile
network, additional provider URLs or durable restart recovery.

L1 posts may contain several complete source groups. Only the largest measured
external priority payment is selected for a refund, even when it cannot cover
the combined posting cost. A missing, ambiguous or insufficient refund does not
pause quotes or settlement. Preserve posting logs and inspect source/post/payout
receipts when the [refund dashboard](../../monitoring/README.md#l1-refund-observations)
shows missing payments or shortfalls. A shortfall can be the accepted batching
subsidy rather than a builder payment failure; compare the agreed refund terms.

Per-post attribution and monitoring totals remain in memory and are not restored
by node databases. Restart does not require clearing a refund pause flag, but may
lose pending requests and observations. Drain or account for work before
restarting. Disabling the policy also disables L1 quote/admission checks and
refund requests.

## Offline v2 proving and fresh-network boundary

The signer consumes local L1/L2 profiles and complete witness evidence, with no
L1 or L2 RPC. Configure `EEZ_ROLLUP_MANAGER_ADDRESS` for the deployed offline
manager; keep node, follower and sender RPC settings on their own services.
Obsolete signer RPC variables fail startup, including empty values. Follow the
[signer operations guide](../../crates/eez-proof-signer/docs/operations.md#offline-configuration-and-recovery)
for limits, configuration and loss of volatile sessions.

This profile requires freshly deployed contracts, authenticated history bootstrap,
a newly rendered genesis and fresh datadirs. Existing Chiado deployments are
outside its compatibility boundary. There is no upgrade, database migration or
legacy execution mode in these binaries. Do not rerun deployment or delete
operator state to convert an existing network. Source and private fixture tests
do not establish a public deployment or restart rehearsal.
