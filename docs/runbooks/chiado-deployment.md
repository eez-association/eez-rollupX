# Deploy and operate a Chiado rollup

This procedure covers the packaged Linux Docker stack: an `eez-node` containing
the Composer, embedded Chiado execution client and L2, an independent
`eez-proof-signer`, and Lighthouse. Use
[`docker-compose.chiado-node.yml`](../../docker-compose.chiado-node.yml).
`docker-compose.chiado.yml` is the standalone Beacon profile and is not this stack.
The implementing scripts are [deploy](../../scripts/deploy.sh),
[startup](../../scripts/chiado-up.sh), and [teardown](../../scripts/teardown-chiado.sh).

Source reviewed on 2026-09-16 against fork runtime commit
`ddf40bc2eb5fac8f7973b112eb383256cd8aac74` and protocol pin
`3c0cdf4955941a5c23639346932b55b33babebbd`. Validation covered relative links,
Bash example syntax, and `scripts/check-chiado-blob-config.sh`. A fresh-host
deployment and restart rehearsal remain to be performed and recorded in a
dated deployment audit.

## Choose the operation

| Intent | Start here | State to retain |
| --- | --- | --- |
| Create a new registry and rollup | [First deployment](#first-deployment) | Use unused deployment paths and keys selected for this deployment |
| Start or restart the same rollup | [Restart and stop](#restart-and-stop) | Existing keys, deployment bindings, genesis, JWT, and datadirs |
| Move hosts, change binaries, or recover missing state | [Recovery and upgrades](#recovery-and-upgrades) | Establish the deployment inventory and recovery source before changes |

Run commands from the root of the intended deployment checkout, in Bash, and
stop on a failed check. Use a shell without stale exported `EEZ_*`, path, or
Compose overrides. The packaged scripts use `.env`, `.env.chiado`, and
`deployments.env`; this procedure keeps those filenames and the example paths.
Only operate the deployment and resources covered by the task's authorization.

Do not remove `deployments.env` to restart. `make deploy-protocol` always attempts
a new deployment, even when that file exists. `chiado-up.sh` can deploy when it
is missing and then **delete the configured L2 and Lighthouse datadirs**.
Missing bindings require recovery of the deployment inventory, not a blind rerun.

## Prerequisites

- A Linux host with Docker Engine, the Compose plugin, BuildKit, and access to
  its local Docker daemon. This profile uses host networking and fixed container names.
  A second Compose project name or Git worktree does not isolate its ports or
  containers; use a separate host or an explicitly adapted profile.
- Bash, Git, Make, `curl`, `jq`, Python 3, OpenSSL, and standard GNU utilities
  (including `realpath`, `sha256sum`, and `cmp`). Use host `forge` and `cast` from
  **Foundry 1.7.1**; the `forge` version is enforced by
  [`update-eezl2-genesis.sh`](../../scripts/update-eezl2-genesis.sh).
  Docker builds use the toolchains in the two Dockerfiles; a separate host Rust
  build is unnecessary with the genesis-tool wrappers below.
- Disk and memory for the Rust image builds, the L1 snapshot, Lighthouse's
  PeerDAS data, and growing L1/L2 databases. This procedure does not establish
  a measured minimum hardware size; record available capacity and monitor it.
- Outbound access for dependencies, images, the snapshot/checkpoint sources,
  a synchronized Chiado deployment RPC, and a builder supporting `eth_sendBundle`.
  The RPC must report chain ID **10200**. A general Ethereum RPC is not a builder.
- P2P connectivity for embedded L1 port **30544 TCP/UDP** and Lighthouse
  **9205 TCP/UDP**, **9206 UDP**. Check host firewall/NAT configuration.
  See [discovery troubleshooting](#troubleshooting) if automatic advertisement fails.

The default local service ports are:

| Port | Purpose |
| --- | --- |
| 18645 | Embedded Chiado L1 HTTP RPC |
| 18688 / 18689 | L2 HTTP / WebSocket RPC |
| 18999 / 18998 | L1-to-L2 / L2-to-L1 cross-chain fronts |
| 18551 / 18684 | L1 / L2 engine APIs |
| 50061 / 6152 | Proof signer gRPC / Beacon HTTP API |
| 30640 / 30641 | Reserved L2 P2P/discovery ports in the profile |

Check for existing listeners and the three named containers before first startup.
The L2 HTTP RPC, cross-chain fronts, and L1 engine API bind all host interfaces
in this profile. Restrict access using the host's network policy; host networking
does not provide Docker port isolation. Engine JWT and signer/Beacon endpoints
belong on the operator network. A public gateway is a separate deployment; see
the [gateway's Chiado notes](../../testing/public-gateway/README.md#chiado-deployment).

### Keys and funding

| Input | Role | Funding |
| --- | --- | --- |
| `EEZ_L1_POSTER_KEY` | Sends settlement transactions | Chiado native gas funds |
| `EEZ_DEPLOY_KEY` (optional) | Deploys and owns contracts; defaults to poster | Chiado native gas funds if separate |
| `EEZ_PROOF_SIGNER_KEY` | Attests batches; its address is registered in the proof system | No L1 transaction funding needed for attestation |
| `EEZ_L2_SYSTEM_KEY` | Signs L2 system transactions | Its address is funded by the generated L2 genesis |

Use dedicated keys. Proof signer and L2 system addresses **must differ**. Keep
the poster separate from the proof signer. A separate deploy key avoids nonce
competition with an already active poster. Do not share a posting wallet with
concurrent traffic generators. Obtain testnet funds through an operator-approved
source and check balances before broadcasting; no fixed gas cost is assumed here.

The base [`genesis.json`](../../genesis.json) also contains funded development
accounts, and some test drivers use public development keys. This is a testnet
profile. Review allocations before deployment if those accounts are unsuitable;
changing genesis after registration changes the rollup's identity.

## First deployment

### 1. Record source and build the images

Confirm the target commit and existing changes before initializing the pinned
submodule. Do not update it to upstream `main` as part of deployment.

```bash
(
  set -euo pipefail
  git status --short
  git rev-parse HEAD
  git ls-tree HEAD eez-core-protocol
  git submodule update --init --recursive
  git submodule status --recursive
  docker compose version
  forge --version
  cast --version
  docker build -t eez-node:local .
  docker build -f Dockerfile.signer -t eez-proof-signer:local .
  docker image inspect --format '{{.RepoTags}} {{.Id}}' eez-node:local eez-proof-signer:local
  bash scripts/check-chiado-blob-config.sh
)
```

Expected: submodule matches the recorded gitlink, both builds succeed, and the
static check prints `Chiado post-Fulu blob deployment configuration: PASS`.
Record image IDs: `:local` tags can later point to different builds. This static
check uses example configuration and does not check a live network.

The host deployment script also needs `eez-genesis-state-root` and
`eez-genesis-block-hash`. Without them it falls back to a host Cargo build,
potentially after contracts have already been broadcast. The node image includes
both tools. These local wrappers run that image with only the supplied genesis
file mounted and networking disabled:

```bash
export PATH="$PWD/.runtime/chiado-tools:$PATH"
(
  set -euo pipefail
  mkdir -p .runtime/chiado-tools
  cat > .runtime/chiado-tools/genesis-tool <<'SH'
#!/usr/bin/env bash
set -euo pipefail
genesis_path="$(realpath -- "${1:?pass the genesis JSON path}")"
exec docker run --rm --pull never --network none \
  --mount "type=bind,source=$genesis_path,target=/genesis.json,readonly" \
  --entrypoint "/usr/local/bin/$(basename "$0")" \
  eez-node:local /genesis.json
SH
  chmod 755 .runtime/chiado-tools/genesis-tool
  ln -sfn genesis-tool .runtime/chiado-tools/eez-genesis-state-root
  ln -sfn genesis-tool .runtime/chiado-tools/eez-genesis-block-hash
  eez-genesis-state-root "$PWD/genesis.json"
  eez-genesis-block-hash "$PWD/genesis.json"
  forge build --root contracts
)
```

Expected: each helper prints a 32-byte hex hash, and contracts compile. The two
hashes have different meanings; registration uses the **genesis block hash**.
These checks use the repository fixture only to verify tools, not as the new
deployment's genesis. Re-export this `PATH` if deploying from a new shell.

### 2. Configure the deployment

Create private configuration only when no deployment configuration already exists:

```bash
(
  set -euo pipefail
  for path in .env .env.chiado deployments.env; do
    [[ ! -e "$path" ]] || { echo "Already exists: $path; inspect the existing deployment"; exit 1; }
  done
  umask 077
  cp .env.example .env
  cp .env.chiado.example .env.chiado
)
```

Edit both files before continuing:

| File | Required settings |
| --- | --- |
| [`.env`](../../.env.example) | Set `EEZ_L1_RPC_URL` to your synchronized Chiado RPC; replace all three key placeholders; optionally set a separate `EEZ_DEPLOY_KEY`. Replace the example RPC address rather than assuming it is available. |
| [`.env.chiado`](../../.env.chiado.example) | Set the **same three keys**, unused host data paths, the builder URL, `EEZ_L1_BEACON_URL=http://127.0.0.1:6152/`, and `EEZ_L1_BLOB_SIDECAR_ENCODING=eip7594`. Keep `FRESH_GENESIS=./datadir/genesis.json`. |

For this default path, leave `EEZ_ENV_FILE`, `EEZ_DEPLOYMENTS_FILE`,
`EEZ_GENESIS_OUT`, and `EEZ_GENESIS_PROFILE_OUT` unset. Their deploy-script
overrides are not consistently consumed by the packaged startup wrapper.
`L2_DATA_DIR` is the Docker host data path; the bare-binary `EEZ_L2_DATADIR`
example is not a replacement for it.

`make deploy-protocol` reads `.env`. `chiado-up.sh` reads `.env` followed by
`.env.chiado`, so the latter wins. Compose gives the running node the embedded
L1 RPC; the external tip RPC is still needed by the startup checks. Keep files
consistent and do not paste expanded Compose configuration or environment dumps
into reports: they include secrets.

Keep the example settlement policy disabled for the first deployment unless a
different policy is part of the task. To enable it, follow the
[settlement configuration guide](../configurable-settlement-policy.md), including
removing the legacy block cap and setting the required fee ceilings. Omitted
settings and empty strings are not interchangeable; changes require recreation
of the affected service.

Before spending gas, check the deployment RPC and key addresses, then rehearse
genesis rendering into an ignored directory:

```bash
(
  set -euo pipefail
  source .env
  [[ "$(cast chain-id --rpc-url "$EEZ_L1_RPC_URL")" == 10200 ]]
  cast block latest --rpc-url "$EEZ_L1_RPC_URL" --json | jq '{number, timestamp, hash}'
  poster="$(cast wallet address --private-key "$EEZ_L1_POSTER_KEY")"
  deployer="$(cast wallet address --private-key "${EEZ_DEPLOY_KEY:-$EEZ_L1_POSTER_KEY}")"
  attester="$(cast wallet address --private-key "$EEZ_PROOF_SIGNER_KEY")"
  system="$(cast wallet address --private-key "$EEZ_L2_SYSTEM_KEY")"
  [[ "${attester,,}" != "${system,,}" ]]
  cast balance "$deployer" --rpc-url "$EEZ_L1_RPC_URL"
  cast balance "$poster" --rpc-url "$EEZ_L1_RPC_URL"
  source .env.chiado
  [[ "$poster" == "$(cast wallet address --private-key "$EEZ_L1_POSTER_KEY")" ]]
  [[ "$attester" == "$(cast wallet address --private-key "$EEZ_PROOF_SIGNER_KEY")" ]]
  [[ "$system" == "$(cast wallet address --private-key "$EEZ_L2_SYSTEM_KEY")" ]]
  bash scripts/update-eezl2-genesis.sh --render \
    --rollup-id 1 --system-address "$system" \
    --base "$PWD/genesis.json" \
    --output "$PWD/.runtime/chiado-tools/preflight-genesis.json" \
    --profile-output "$PWD/.runtime/chiado-tools/preflight-profile.json"
)
```

Expected: chain ID 10200, a recent advancing L1 head, balances sufficient for
deployment/posting, matching keys in both files, and successful genesis rendering.
Check head freshness against a trusted tip source; chain ID alone is insufficient.
The preflight genesis is disposable and must not become `FRESH_GENESIS`.

### 3. Prepare L1 data and the shared JWT

The following uses the default paths. It refuses existing paths so an old
deployment is not overwritten. If you selected custom paths, adapt this block
to those exact paths before executing it.

```bash
(
  set -euo pipefail
  for path in data/chiado-l1 data/eez-l2 data/chiado-lighthouse data/jwt.hex datadir/genesis.json; do
    [[ ! -e "$path" ]] || { echo "Already exists: $path; identify its owner first"; exit 1; }
  done
  mkdir -p data
  docker run --rm -v "$PWD/data/chiado-l1:/data" \
    ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
    download --chain chiado --minimal --datadir /data
  (umask 077; openssl rand -hex 32 > data/jwt.hex)
  test -s configs/chiado/bootstrap_nodes.yaml
  cmp configs/chiado/bootstrap_nodes.yaml configs/chiado/bootnodes.yaml
)
```

Expected: snapshot download completes, the JWT is created once, and the
bootstrap alias matches. Retain the checked-in Chiado config with the pinned
Lighthouse v8.1.3 image: its built-in Chiado profile does not activate Fulu.
Refreshing network config or upgrading Lighthouse needs separate validation.
Both node and Beacon must use the same JWT. Do not download over a running L1
database or regenerate its JWT as a routine restart step.

### 4. Deploy contracts and generate the bound genesis

This step broadcasts Chiado transactions and spends the deployer's gas funds.
The script creates a fresh registry, proof system, manager, first rollup (ID 1),
and L1 bridge contracts. It renders the deployment's system address into genesis,
aligns the timestamp to the recorded L1 deploy block, and registers the resulting
**block hash**, not just its state root.

```bash
(
  set -euo pipefail
  [[ ! -e deployments.env ]] || { echo "Existing deployment: use the restart procedure"; exit 1; }
  umask 077
  mkdir -p .runtime/chiado-deployment
  EEZ_DEPLOY_SKIP_SIMULATION=1 make deploy-protocol \
    2>&1 | tee ".runtime/chiado-deployment/deploy-$(date -u +%Y%m%dT%H%M%SZ).log"
)
```

`EEZ_DEPLOY_SKIP_SIMULATION=1` bypasses Foundry's pre-broadcast simulation to work
around RPC block-availability failures. It is **not a dry run**. Omit it when
using a deployment RPC on which simulation works reliably.

Expected outputs: `deployments.env`, `datadir/genesis.json`,
`datadir/genesis.profile.json`, and receipts under `contracts/broadcast/`.
The bindings contain public addresses and hashes; the keys remain in private
configuration. Keep the deployment log private until sanitized because it may
include an authenticated RPC URL. Optional Blockscout verification is best effort
and does not determine deployment success.

The script is not atomic or resumable: a failure can leave contracts on L1
without a completed `deployments.env`. Preserve logs and broadcast receipts,
inspect actual transaction outcomes and nonces, and reconcile the deployment
before deciding how to continue. Repeating the command creates another deployment.

### 5. Verify identity and start the stack

The non-root signer must be able to read the **public** generated genesis.
The following also checks the registered block hash before any node starts:

```bash
(
  set -euo pipefail
  source .env
  source .env.chiado
  source deployments.env
  test -s "$EEZ_L2_GENESIS_PROFILE_PATH"
  cmp "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PATH"
  [[ "$(eez-genesis-block-hash "$FRESH_GENESIS")" == "$EEZ_INITIAL_BLOCK_HASH" ]]
  [[ "$(eez-genesis-state-root "$FRESH_GENESIS")" == "$EEZ_L2_GENESIS_STATE_ROOT" ]]
  registered="$(cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' \
    "$EEZ_ROLLUP_ID" --rpc-url "$EEZ_L1_RPC_URL" | sed -n '2p' | tr -d '[:space:]')"
  [[ "$registered" == "$EEZ_INITIAL_BLOCK_HASH" ]]
  chmod 644 "$FRESH_GENESIS"
  sha256sum "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PROFILE_PATH"
  set -a
  source .env
  source .env.chiado
  set +a
  docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml config --quiet
  bash scripts/chiado-up.sh
)
```

Stop if identity/configuration checks fail. The initial registered-hash equality
above applies **before the first settlement**; after operation the registry's
commitment should have advanced. Keep secret files restricted; the `chmod`
above only changes the genesis file mounted into the signer.

Initial deployment must precede `chiado-up.sh`: that wrapper checks that
`FRESH_GENESIS` exists before reaching its optional deployment branch. On this
path, the existing `deployments.env` makes it skip deployment and data deletion.

## Health and acceptance

Startup checks builder method availability, the post-Fulu Beacon API, L2 block
production, and a changed registry commitment. It then finds a new `BatchPosted`
transaction, checks type 3, and retrieves each selected full blob by its versioned
hash. Success ends with `chiado L2 is UP` and the settlement transaction hash.

The builder probe uses an intentionally empty bundle; its response only checks
method availability. A new settlement provides stronger pipeline evidence.
Neither that probe nor startup success establishes all cross-chain semantics or
independent follower replay. A startup timeout can leave all containers running;
inspect them before another operation.

Useful read-only checks:

```bash
docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml ps
docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml logs --tail=100 eez-node eez-proof-signer lighthouse
cast chain-id --rpc-url http://127.0.0.1:18645
cast block-number --rpc-url http://127.0.0.1:18645
cast chain-id --rpc-url http://127.0.0.1:18688
cast block-number --rpc-url http://127.0.0.1:18688
curl -fsS http://127.0.0.1:6152/eth/v1/node/syncing | jq '.data'
curl -fsS http://127.0.0.1:6152/eth/v1/node/peer_count | jq '.data'
```

Expected: three running services, healthy signer, L1 chain 10200 close to a trusted
tip and advancing, L2 chain ID matching `.config.chainId` in the generated genesis,
advancing L2 blocks, connected Beacon peers, and a synchronized non-optimistic
Beacon without an execution-client-offline flag. Repeat head checks over several
slots. A TCP health check or a single HTTP response is insufficient.

Record the settled L2 range using the
[settlement correlation RPC](../../README.md#l1l2-settlement-correlation-rpc), its
canonical L1 transaction, and the blob retrieval result. If the task requires
cross-chain semantics, run an appropriately scoped funded workload and record its
assertions separately. The [matrix driver](../../scripts/xchain-test.sh) requires
a funder on **both chains** and has public test-key assumptions; inspect those
before use. The [staged performance runner](../../scripts/run-chiado-staged-performance.sh)
uses shared wallet locks and `--no-verify`: its `--check` is readiness-only, and
its traffic results do not establish semantic correctness. See
[workload instructions](../../README.md#exercise-it).

## Restart and stop

Use the existing checkout, images, `.env`, `.env.chiado`, `deployments.env`,
genesis/profile, JWT, and the same data paths. Verify these against the deployment
inventory. Do not run the first-deployment steps, rebuild images, or regenerate
genesis for an ordinary restart.

Pending requests and retry counters are in memory. Before a planned stop, pause
new submissions and record unsettled requests and their canonical receipt/nonce
status. After restart, reconcile before resubmission; a database backup does not
preserve volatile queue ownership. Canonically settled state is reconstructed
from L1, while the speculative suffix is discarded.

Stop the owned stack with:

```bash
bash scripts/teardown-chiado.sh
```

Expected: all three named containers are stopped and bind-mounted datadirs remain.
The signer's shutdown grace period is up to 11 minutes for admitted requests.
Preserve the configuration and data even if stop fails.

Before starting again, require the existing bindings and genesis:

```bash
(
  set -euo pipefail
  test -s .env
  test -s .env.chiado
  test -s deployments.env
  source .env.chiado
  source deployments.env
  : "${EEZ_INITIAL_BLOCK_HASH:?missing deployment identity}"
  cmp "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PATH"
  test -s "$JWT_FILE"
  test -d "$L1_SNAPSHOT_DIR"
  test -d "$L2_DATA_DIR"
  test -d "$LIGHTHOUSE_DATA"
  bash scripts/chiado-up.sh
)
```

Expected: startup skips deployment, retains data, and repeats the health checks.
It waits for a **new** settlement. With settlement policy enabled, a long interval
or no due work can outlast that wait; consult the policy and logs rather than
redeploying or weakening the policy merely to obtain the success banner.

Configuration changes require `compose up -d` (as used by `chiado-up.sh`) to
recreate affected services. `docker restart` alone keeps the old container
environment. Rebuilding a `:local` image turns the next start into an upgrade;
record that deliberately.

## Troubleshooting

| Symptom | Check and next action |
| --- | --- |
| Missing genesis or mismatched hash | On a new deployment, finish the explicit deploy step. For an existing one, recover the exact genesis and bindings; do not edit the registered timestamp or substitute the fixture. |
| Partial deploy; no bindings file | Preserve `contracts/broadcast/` and the private deploy log. Check confirmed transactions, contract addresses, and deployer nonce before planning recovery. |
| Signer unhealthy | Read signer logs; verify attester and system keys match deployment, proof/system keys differ, the mounted public genesis is readable by UID 65532, and chain config matches. TCP readiness alone does not prove signing succeeds. |
| Ports busy or database locked | Identify the owner of the listener/container/datadir. Fixed names and host ports collide across worktrees; do not kill unrelated processes or share a writable database. |
| Beacon has no peers or cannot catch up | Check snapshot catch-up, shared JWT, checkpoint source, Fulu config and bootstrap alias, then `/eth/v1/node/identity`, peer count, and firewall/NAT. |
| Missing public discovery address | Set `LH_ENR_ADDRESS` and `LH_ENR_UDP_PORT` in `.env.chiado` to the mapped discovery endpoint (default UDP 9205). These settings advertise an endpoint; they do not create port forwarding. Recreate Lighthouse and verify peers and synchronization. |
| Blob API empty or unavailable | An empty `blobs/head` is valid when the head has no blobs. Check the actual settlement slot and versioned hashes, post-Fulu config, semi-supernode mode, synchronization, and whether data is still retained. |
| L2 advances but no settlement | Check L1/Beacon freshness, poster funds/nonces, builder submission and inclusion, proof failures, fee/capacity limits, and whether the configured policy has due work. Separate a retained/deferring request from a failed attempt. |
| Simulation reports an unavailable block | Verify the RPC is at tip. The documented skip-simulation flag bypasses that phase only; inspect broadcast results independently. |

For a discovery-only change, recreate only the owned Beacon service using the
same environment precedence as startup:

```bash
(
  set -euo pipefail
  set -a
  source .env
  source .env.chiado
  set +a
  docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml up -d --no-deps lighthouse
)
```

## Recovery and upgrades

Preserve the deployment inventory and take a consistent backup before planned
data or binary changes. Stop writers for a filesystem copy, or use a documented
database-consistent snapshot method. A copy of an actively written datadir is
not automatically a usable backup.

The colocated semi-supernode reconstructs full blobs for live operation within
its retention window. Recovery beyond retained history needs a durable archival
Beacon source or a validated protocol snapshot/bootstrap procedure. Neither an
execution snapshot nor successful startup proves historical blob availability.
Do not promise full re-derivation until the required history and recovery method
have been tested. Missing history requires an explicit recovery plan, not deletion
of the current L2 database.

Before upgrading, record source/protocol/image identities, inspect configuration
and persistent-state compatibility, and plan the node/signer/protocol rollout
together. Keep previous images and backups. A downgrade is only an available
rollback when the old binaries can read the resulting state and speak the deployed
protocol. New contracts or a different genesis create a different deployment;
they are not an in-place restart or an automatic migration of user state.

## Handoff and completion record

Maintain a private inventory so the next operator can find the deployment without
searching shell history. Record:

- UTC date, operator, checkout/host reference, fork commit and dirty changes,
  protocol pin, tool versions, and immutable image IDs.
- Network/chain IDs, registry, rollup ID, manager, proof-system/attester and
  system addresses, bridge addresses, deployment block, and transaction receipts.
- Genesis/profile SHA-256 digests, initial block hash, state root, and retained
  locations of `deployments.env`, genesis/profile, datadirs, JWT, and backups.
- Secret-manager references for keys and private RPC credentials; never the keys
  themselves in a committed record. Retain access instructions privately.
- Effective timing, fee, settlement-policy and blob settings; endpoint ownership;
  health evidence, latest confirmed settlement, pending requests, and next actions.
- Archival/history dependencies, backup retention, and which restart/recovery or
  semantic checks were actually performed versus still pending.

For an executed deployment or investigation, use the
[audit template](../audits/TEMPLATE.md) for sanitized evidence and link back here.
Keep private raw logs and bulk data outside Git; identify retrievable artifacts
and their digests. Update this runbook when a verified step changes, and preserve
the dated audit as the record of what that particular operation established.
