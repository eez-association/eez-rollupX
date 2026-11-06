# Bootstrap a Nethermind RollupX follower with SNAP

Download state through the existing ETH/SNAP protocols, then continue as an EEZ
follower. This requires the EEZ build in [PR13867](https://github.com/NethermindEth/nethermind/pull/13867),
not a standard release. Start with the deployment inputs and security rules in
[the follower runbook](nethermind-follower.md).

## Status and network

The qualified consumer build is `542dc93652718b8a0ed0c102f81c4e3b35a5ef21`:

```text
nethermindeth/nethermind:eez-542dc9365271@sha256:4e56266079ea16ff70a42b3956d10dbd6a6585b767361b67313223a9bac389ae
```

[Image publication](https://github.com/NethermindEth/nethermind/actions/runs/42052492557),
[live CI](https://github.com/NethermindEth/nethermind/actions/runs/42052396645)
and [streaming CI](https://github.com/NethermindEth/nethermind/actions/runs/42052395594)
identify the source and validation. The exact Compose overlays downloaded pivot
339690 over public discovery from another VM, followed to block 341006 and
reached 341090 after same-database restart. Canonical roots, state and receipts
matched at fixed heights, with live-head lag 1 before and after restart. The
build also recovered the database that exposed an earlier restart defect.
See [the retained evidence](../audits/2026-11-05-nethermind-snap/README.md).
This is not evidence of transfer from a different client or a general sync-speed benchmark.

The following operator-provided Nethermind bootnode serves only this network:

| Field | Value |
| --- | --- |
| Network | Ethereum-backed RollupX, `rollupx.eez.dev` |
| L2 chain ID | 696991 |
| Genesis hash | `0xb6bd210b7e824a1aabc4c1ae5b64f5643715441b332c7152e641a36ba01835b2` |
| Public P2P/discovery | TCP and UDP 30313 |

```text
enode://94c3e3ae1718f646508770483ce335175f97420fe635be7a545a739ab9981d06de2fb3d1ec37737ddfc55ad619f2b1d12922f48dbd15f44501b8f244c9f99d72@104.196.3.9:30313
```

An enode contains a public node ID and address, not a private key. It is an
initial discovery seed, not an exclusive or trusted state source. Peers must
match the network and protocol; state proofs and the canonical finalized L1
commitment remain mandatory. Do not use this seed for a Chiado-backed L2.
Availability and retained state can change. Use additional operator-provided
seeds when available; multiple enodes are comma-separated.

## Required inputs

Keep the L2 genesis, its hash and the L1 deployment inputs from the network
operator. L1 RPC and Beacon history are still required to authenticate the pivot
and derive later blocks. A bootnode does not replace those inputs.

Use a separate empty data directory, an unused Compose project and unused host
ports. Preserve an existing follower's database. The serving peer must retain
the finalized pivot state. Archive data on disk alone does not establish that
the root is inside its SNAP serving window.

The commands below run from the repository root with Docker Compose v2, `curl`
and `jq`. Use a shell without another profile's exported `EEZ_*`, `NETHERMIND_*`
or `COMPOSE_FILE` settings. Exported values override the env file.

## Download into an empty database

1. Prepare a separate env file and directory. Stop if either already exists;
   inspect a previous attempt instead of erasing it:

   ```sh
   test ! -e .env.nethermind-snap &&
     test ! -e data/nethermind-snap &&
     cp -n .env.nethermind.example .env.nethermind-snap &&
     chmod 600 .env.nethermind-snap &&
     mkdir -p data/nethermind-snap/db data/nethermind-snap/logs data/nethermind-snap/keystore
   ```

2. Fill the deployment inputs as in [first setup](nethermind-follower.md#first-setup).
   Set `NETHERMIND_DATA_DIR=./data/nethermind-snap`. For a second node on the
   same host, choose unused RPC, metrics and P2P ports. Set these additional
   values in `.env.nethermind-snap`:

   ```text
   NETHERMIND_P2P_IP=<your externally reachable IPv4 address>
   NETHERMIND_BOOTNODES=<enode for this exact L2>
   NETHERMIND_SNAP_DOWNLOAD=true
   NETHERMIND_PROCESSING_ENABLED=false
   NETHERMIND_SNAP_RESTART_POLICY=no
   ```

   Allow the selected P2P port through NAT/firewalls for TCP and UDP. The
   advertised port equals the published port. Discovery and the default recent-IP
   peer filter can delay replacement peers that reuse an address; check the
   logs before restarting another instance. RPC and metrics remain published
   only on host loopback; Engine is not published. No proof key is needed.

3. Read a finalized L2 block from the network source. Replace the URL if using
   another deployment:

   ```sh
   pivot=$(curl --fail --silent --show-error https://rollupx.eez.dev/rpc/l2 \
     -H 'Content-Type: application/json' \
     --data '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["finalized",false]}')
   printf '%s\n' "$pivot" | jq -e '.result | {number, hash, stateRoot}'
   number=$(printf '%s\n' "$pivot" | jq -er '.result.number')
   hash=$(printf '%s\n' "$pivot" | jq -er '.result.hash')
   printf 'NETHERMIND_PIVOT_NUMBER=%d\nNETHERMIND_PIVOT_HASH=%s\n' "$number" "$hash"
   ```

   Copy the two printed values into the env file. Require a nonzero pivot.
   Keep this number and hash fixed throughout download, handoff and later
   restarts. The node independently authorizes them against canonical finalized
   L1 history; this RPC read alone does not authorize state.

4. Select the base, networking and SNAP overlays, then render and start:

   ```sh
   export COMPOSE_FILE=docker-compose.nethermind-follower.yml:docker-compose.nethermind-p2p.yml:docker-compose.nethermind-snap.yml
   docker compose --env-file .env.nethermind-snap -p nethermind-snap config --quiet
   docker compose --env-file .env.nethermind-snap -p nethermind-snap pull nethermind-follower
   docker compose --env-file .env.nethermind-snap -p nethermind-snap up -d nethermind-follower
   container=$(docker compose --env-file .env.nethermind-snap -p nethermind-snap ps -aq nethermind-follower)
   test "$(docker wait "$container")" = 0
   docker logs --tail=100 "$container"
   ```

   Require both `Snap sync completed.` and `EEZ SNAP download verified.` in the
   download logs. The node checks trie/code completeness, header ancestry, block
   data and L1 authorization before exiting. A peer count, matching height or
   an idle sync mode is not success. If it exits with an error, keep the data
   and logs and investigate. Never advance to the next step to bypass a failure.

## Continue as a follower

Keep the same project, files, pivot and database. Change only these env values:

```text
NETHERMIND_SNAP_DOWNLOAD=false
NETHERMIND_PROCESSING_ENABLED=true
NETHERMIND_SNAP_RESTART_POLICY=unless-stopped
```

Then recreate the stopped service:

```sh
docker compose --env-file .env.nethermind-snap -p nethermind-snap config --quiet
docker compose --env-file .env.nethermind-snap -p nethermind-snap up -d --force-recreate nethermind-follower
docker compose --env-file .env.nethermind-snap -p nethermind-snap logs --tail=100 nethermind-follower
```

Startup verifies the downloaded database and L1 commitment again, installs its
checkpoint and starts ordinary EEZ following. Generic synchronization is now
off. Follow [the canonical progress checks](nethermind-follower.md#verify-progress):
compare hashes, roots and receipts at exact heights; verify execution beyond
the pivot, live-head lag and safe/finalized progress. Repeat after a clean restart
using the same env file, overlays, project and database.

SNAP supplies state at the pivot. It does not recreate execution witnesses or
historical state before the pivot. Downloaded historical headers, bodies and
receipts are separate from historical state availability.

## Serve SNAP from an existing follower

Use the base and P2P overlay only. Do not apply the SNAP bootstrap overlay to
replace an existing processed chain. Take a stopped database/configuration
backup before changing the image or network settings.

Set `NETHERMIND_P2P_IP` and an unused `NETHERMIND_P2P_PORT` in that follower's env
file, preserve its data path and use:

```sh
export COMPOSE_FILE=docker-compose.nethermind-follower.yml:docker-compose.nethermind-p2p.yml
docker compose --env-file .env.nethermind config --quiet
docker compose --env-file .env.nethermind up -d nethermind-follower
```

The overlay enables normal discovery and SNAP serving with a default root
window of 16,384 blocks and a 20-peer limit. It keeps ordinary synchronization off.
Use a supported flat or half-path backend; this does not migrate database keys.
Size retention for the network's actual finalized L1-to-L2 gap and resource
budget. Reject an unavailable root rather than substituting an unsafe pivot.

Read `admin_nodeInfo` over the local RPC for the enode. Confirm its advertised
IP/port and public TCP/UDP reachability before sharing it. Keep static/trusted
lists and any client matcher free of unintended restrictions. Publish only the
enode, network identity and verification scope, never a node key, JWT or signer
key. Keep Admin, Engine, RPC and metrics off public interfaces.

For rollback, preserve the failed database and restore the matching stopped
backup and configuration. Changing an image tag alone is not a data rollback.
