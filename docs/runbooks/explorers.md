# Run explorers for an existing Chiado rollup

One [Compose file](../../docker-compose.explorers.yml), one private env file.
Select the explorers once; use ordinary Docker Compose commands to run them.
Explorers are operated separately and are not built, deployed, or tested by CI.
For the underlying rollup setup and deployment inputs, see the
[Chiado deployment runbook](chiado-deployment.md).

Source baseline: fork `5469c88` plus this explorer change; protocol
`3c0cdf4955941a5c23639346932b55b33babebbd`. Compose/helper checks and isolated
Blobscan source checks passed. A full fresh explorer deployment has not been
rehearsed; the tested host's Docker bridge RPC request timed out while host RPC
and tracing responded. Check Docker-to-host access before starting Blockscout.

## One-time setup

Run from the repository root:

```bash
# Blockscout only
python3 deploy/explorers/prepare.py --genesis /absolute/path/to/l2/genesis.json

# Blobscan only: use your deployment's first L1 block (EEZ_REGISTRY_DEPLOY_BLOCK).
python3 deploy/explorers/prepare.py --start-block YOUR_L1_DEPLOY_BLOCK

# Both
python3 deploy/explorers/prepare.py \
  --genesis /absolute/path/to/l2/genesis.json --start-block YOUR_L1_DEPLOY_BLOCK
```

This creates `.env.explorers` with credentials and the selected profiles.
Blockscout's `L2_CHAIN_ID` is read from the genesis. For Blobscan, preparation
checks Chiado's chain ID and the starting block against the Beacon API, then
records `BLOBSCAN_START_BLOCK`, `BLOBSCAN_START_SLOT`, and
`BLOBSCAN_START_BLOCK_HASH`. The same boundary controls initialization,
statistics, navigation, and the UI label; the images work for different ranges
without rebuilding. The block must be at or after Chiado's Deneb activation.
Use a finalized deployment block whose blob history your beacon retains.

Preparation uses `http://127.0.0.1:18645` and `http://127.0.0.1:6152` by default.
Pass `--l1-rpc URL --beacon URL` for other endpoints. These URLs are saved for
the host-networked indexer. Repeat runs preserve credentials and profile
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
The file also contains commented RPC and port overrides. Keep `COMPOSE_FILE` and all
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
  uses Bash, curl, tar, and patch.
- Blockscout uses the chain ID and allocation from your actual L2 genesis.
  `L2_RPC_FROM_DOCKER` defaults to `http://host.docker.internal:18688`, directly
  reaching the packaged node through Docker's host gateway. No UI proxy is
  needed. The RPC must expose `eth`, `net`, `web3`, and `trace` and be reachable
  from the container. The packaged Chiado Compose file binds HTTP on
  `0.0.0.0:18688` and now includes `trace`. If an existing node lacks `trace`,
  enable it on its next planned recreation. The host firewall must allow the
  explorer Docker network to reach TCP 18688; a listening socket alone does
  not establish container reachability. Keep public RPC access restricted.
  For a remote or differently bound node, override the
  RPC URL in `.env.explorers`; a loopback-only custom listener needs a
  Docker-reachable binding.
- Blobscan remains **Chiado L1 chain 10200**; L2 chain IDs are independent.
  Its execution and beacon endpoints must retain blocks and blobs starting at
  your configured L1 block. Preparation checks the boundary's block/hash/slot;
  complete historical blob availability is checked by `verify.py`, not by
  merely resolving that starting block.
- Worktrees do not share ignored deployment files. Use the absolute path to
  the actual genesis. Preserve `.env.explorers` alongside database backups.
  The `eez-explorers` project does not automatically reuse older
  `eez-chiado-explorers` volumes. Plan migration or reindexing and avoid
  overlapping ports.

See [build and verification notes](../../deploy/explorers/README.md) for
post-Fulu proof limitations, canonical-data verification, and statistics.

If direct RPC times out in Blockscout, compare a host RPC request with a
request from the explorer Docker network. Host success plus a container timeout
indicates a routing/firewall issue; check access from that Docker subnet before
debugging the indexer. The UI proxy is no longer part of this path.
