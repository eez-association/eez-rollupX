# Run explorers for an existing Chiado rollup

One [Compose file](../../docker-compose.explorers.yml), one private env file.
Select the explorers once; use ordinary Docker Compose commands to run them.
Explorers are operated separately and are not built, deployed, or tested by CI.
For the underlying rollup setup and deployment inputs, see the
[Chiado deployment runbook](chiado-deployment.md).

Source baseline: fork `5469c88` plus this explorer change; protocol
`3c0cdf4955941a5c23639346932b55b33babebbd`. On 2026-09-17, clean Blobscan API/web
builds, profile/proxy checks, and isolated API/web HTTP smoke checks passed.
The runtime settings endpoint returned Chiado and the configured history boundary.
Compose/helper checks passed for all three profile selections. Live indexing and
backfill with the hostname endpoints have not been rehearsed; verify node access
from the explorer network before starting the indexers.

## One-time setup

Run from the repository root:

```bash
# Blockscout only
python3 deploy/explorers/prepare.py --genesis /absolute/path/to/l2/genesis.json \
  --l2-rpc http://YOUR_NODE_HOST:18688

# Blobscan only: use your deployment's first L1 block (EEZ_REGISTRY_DEPLOY_BLOCK).
python3 deploy/explorers/prepare.py --start-block YOUR_L1_DEPLOY_BLOCK \
  --l1-rpc http://YOUR_NODE_HOST:18645 --beacon http://YOUR_BEACON_HOST:6152

# Both
python3 deploy/explorers/prepare.py \
  --genesis /absolute/path/to/l2/genesis.json --l2-rpc http://YOUR_NODE_HOST:18688 \
  --start-block YOUR_L1_DEPLOY_BLOCK \
  --l1-rpc http://YOUR_NODE_HOST:18645 --beacon http://YOUR_BEACON_HOST:6152
```

This creates `.env.explorers` with credentials and the selected profiles.
Blockscout's `L2_CHAIN_ID` is read from the genesis. For Blobscan, preparation
checks Chiado's chain ID and the starting block against the Beacon API, then
records `BLOBSCAN_START_BLOCK`, `BLOBSCAN_START_SLOT`, and
`BLOBSCAN_START_BLOCK_HASH`. The same boundary controls initialization,
statistics, navigation, and the UI label; the images work for different ranges
without rebuilding. The block must be at or after Chiado's Deneb activation.
Use a finalized deployment block whose blob history your beacon retains.

Replace the hostname placeholders with node DNS names reachable from the host
and explorer containers. `--l2-rpc` is required with `--genesis`; `--l1-rpc` and
`--beacon` are required with `--start-block`. URLs are saved in `.env.explorers`.
All explorer services use a bridge network and internal service names; none
use host networking, fixed Docker IPs, or `host.docker.internal`. Loopback in a
container refers to that container, so do not use `127.0.0.1` for node endpoints.
Repeat runs preserve credentials and profile
selection. Add a missing explorer's configuration with the corresponding flag,
then update `COMPOSE_PROFILES`; changing an existing chain/history boundary
requires a new env file and Compose project.

Edit `.env.explorers` to select what runs:

| Selection | Setting |
| --- | --- |
| L2 Blockscout | `COMPOSE_PROFILES=blockscout` |
| Chiado Blobscan | `COMPOSE_PROFILES=blobscan` |
| Both | `COMPOSE_PROFILES=blockscout,blobscan` |

Profiles initially follow the supplied `--genesis` and `--start-block` flags.
The file also contains commented port overrides. Keep `COMPOSE_FILE` and all
secrets in this file. It is ignored by Git and created with mode 0600.

**If Blobscan is selected**, build its Chiado images once on this Docker host:

```bash
bash deploy/explorers/build-blobscan.sh
```

The `1.40.3-chiado-configurable` images supply Chiado support and runtime
history configuration. Rebuild once if you previously used `1.40.3-chiado`.
Repeat this build only when its image inputs change or the images are missing.
Blockscout's small derived image is built directly by Compose.

## Run

```bash
docker compose --env-file .env.explorers up -d --build
docker compose --env-file .env.explorers ps -a
docker compose --env-file .env.explorers logs --tail 100
docker compose --env-file .env.explorers down
```

Blobscan initializes automatically before indexing. An exited (0)
`blobscan-init` is normal. Initialization saves its origin in the
`blobscan-origin` volume and preserves an existing cursor. Restarts with the
same origin do not require fetching the historical starting beacon block.
A changed boundary, mismatched slot/hash, or an existing cursor without a saved
origin is rejected; migrate/validate old archives explicitly or use a new
project. Back up this small origin volume alongside the database. If initial
setup fails, correct the endpoint/history problem and rerun `up`.
Readiness is not completed catch-up: check a known L2 transaction and
Blobscan's `/blockchain-sync-state`.

`down` preserves database and blob data; `down -v` deletes it. Before changing
`COMPOSE_PROFILES`, run `down` with the old selection, then edit the file and
run `up` again. Simply starting a new selection does not stop the old one.
The rollup runs independently.

## Single-domain gateway paths

The default images above serve the explorer frontends at `/`. For a gateway
publishing both explorers on one hostname, build the separate path images before
changing the running services:

```bash
bash deploy/explorers/build-blockscout-frontend-paths.sh
bash deploy/explorers/build-blobscan.sh --paths
docker compose --env-file .env.explorers \
  -f docker-compose.explorers.yml -f docker-compose.explorers.paths.yml config --quiet
```

The pinned Blockscout frontend is built for `/explorer`; the pinned Chiado
Blobscan web image is built for `/blobscan`. The overlay selects only those web
images and the path-aware Blockscout Nginx proxy. It reuses the existing API,
indexer, databases, ports, and volumes. Run lifecycle commands with both
`-f` files in that order after reviewing the rendered configuration. The
`COMPOSE_FILE` value in `.env.explorers` does not automatically append the path
overlay when `-f` is supplied.

Blockscout's canonical landing URL is `/explorer`, and Blobscan's is
`/blobscan`; both accept prefixed assets and deep links. The public gateway
must preserve these prefixes. Its `/explorer/api/*` and
`/explorer/socket/*` routes reach the local Blockscout Nginx proxy, which
removes the prefix for the backend. Keep Blobscan's archive API private.

Before a live cutover, verify prefixed pages, a known deep link, browser API
calls, Blockscout WebSocket updates, and a Blobscan data download against
the intended images. Preserve the current root-path image tags and Compose
selection for rollback. Recreating the web services interrupts browser
sessions; it does not migrate or discard the explorer databases. Coordinate
the frontend image change with the gateway and public links. See the
[gateway proposal](../proposals/gateway-single-domain-paths.md) for the
mixed-version boundary and validation status.

| Explorer | Browser | API |
| --- | --- | --- |
| L2 Blockscout | http://127.0.0.1:18110 | http://127.0.0.1:18111/api/v2/ |
| Chiado Blobscan | http://127.0.0.1:18100 | http://127.0.0.1:18101 |

Ports bind to loopback. For remote browser access, forward the selected web
ports over SSH:

```bash
ssh -N -L 18110:127.0.0.1:18110 -L 18100:127.0.0.1:18100 YOUR_SSH_HOST
```

## Deployment requirements

- Linux Docker host, Compose v2 or newer, and Python 3. The Blobscan build also
  uses Buildx, Bash, curl, tar, and patch.
- Blockscout uses the chain ID and allocation from your actual L2 genesis.
  `L2_RPC_URL` points directly to the node hostname supplied during preparation.
  The RPC must expose `eth`, `net`, `web3`, and `trace` and be reachable
  from the container. The packaged Chiado Compose file binds HTTP on
  `0.0.0.0:18688` and now includes `trace`. If an existing node lacks `trace`,
  enable it on its next planned recreation. The host firewall must allow the
  explorer Docker network to reach TCP 18688; a listening socket alone does
  not establish container reachability. Keep public RPC access restricted.
  For a remote or differently bound node, edit `L2_RPC_URL` in `.env.explorers`.
- Blobscan remains **Chiado L1 chain 10200**; L2 chain IDs are independent.
  Its execution and beacon endpoints must retain blocks and blobs starting at
  your configured L1 block. Preparation checks the boundary's block/hash/slot;
  complete historical blob availability is checked by `verify.py`, not by
  merely resolving that starting block.
- A hostname does not make a loopback-only listener reachable. The Chiado
  preset's Beacon API listens on `127.0.0.1:6152`; use a reachable Beacon
  endpoint retaining the required history, or explicitly configure its binding
  and firewall during a planned node change. The explorers do not change node
  listeners. Check L1, L2, and Beacon access from the explorer network before
  starting the indexers.
- Worktrees do not share ignored deployment files. Use the absolute path to
  the actual genesis. Preserve `.env.explorers` alongside database backups.
  The `eez-explorers` project does not automatically reuse older
  `eez-chiado-explorers` volumes. Plan migration or reindexing and avoid
  overlapping ports.

For an env file created by the earlier host-network configuration, replace
`L2_RPC_FROM_DOCKER` with `L2_RPC_URL` and set `BLOBSCAN_L1_RPC_URL` and
`BLOBSCAN_BEACON_URL` to reachable node hostnames. Preserve credentials and
history settings. Recreate only the explorer services with `up -d` after
updating their URLs; their volumes are preserved.

See [build and verification notes](../../deploy/explorers/README.md) for
post-Fulu proof limitations, canonical-data verification, and statistics.

If direct RPC times out in Blockscout, compare a host RPC request with a
request from the explorer Docker network. Host success plus a container timeout
indicates a routing/firewall issue; check access from that Docker subnet before
debugging the indexer.
