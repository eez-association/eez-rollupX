# Run a Nethermind RollupX follower

This runs Nethermind as the L2 execution client and follower of an existing
RollupX network. It re-executes the sequencer's unsafe blocks and derives the
safe and finalized chain from canonical L1 evidence and blobs. It does not
replace the Composer's embedded L1 execution client or start a prover.

## Status and source

EEZ support currently requires the build from
[Nethermind PR13867](https://github.com/NethermindEth/nethermind/pull/13867).
It is not part of a standard Nethermind release.

The procedure below uses Nethermind revision
`542dc93652718b8a0ed0c102f81c4e3b35a5ef21`, published as:

```text
nethermindeth/nethermind:eez-542dc9365271@sha256:4e56266079ea16ff70a42b3956d10dbd6a6585b767361b67313223a9bac389ae
```

[Streaming conformance](https://github.com/NethermindEth/nethermind/actions/runs/42052395594)
passed 2,965 EEZ tests and 48 execution/witness checks per architecture on x64 and
ARM64. [Live integration](https://github.com/NethermindEth/nethermind/actions/runs/42052396645)
records the selected upstream sources and scenarios. See the real-network SNAP
and restart results in [the retained evidence](../audits/2026-11-05-nethermind-snap/README.md).

Two operated followers use the EEZ implementation: a Chiado-backed network with
L2 chain ID 699169 and an Ethereum-backed network with L2 chain ID 696991. Both
are RollupX. Confirm the current network bundle with its operator; names such
as "mainnet" do not establish production readiness.

This base procedure keeps P2P and synchronization disabled. To serve SNAP or
bootstrap a fresh database through an operator-provided bootnode, follow the
separate [SNAP runbook](nethermind-snap.md). Its real-network test used Nethermind
on both ends; transfer from a different client is not yet qualified. A stock
execution client still cannot join this L2 from the genesis file alone.

## Required inputs

Get the deployment bundle from the network operator. It must provide:

- The exact L2 genesis JSON, L2 chain ID and expected genesis block hash.
- L1 chain ID and complete L1 genesis JSON. For Chiado, the `chiado` preset is supported.
- Registry contract address, its L1 deployment block and the rollup ID.
- L2 block time in whole seconds and the L2 gas limit.
- The current sequencer L2 JSON-RPC URL.
- An L1 execution JSON-RPC URL with the headers, logs and receipts needed from
  the registry deployment onward.
- A Beacon API URL for that same L1, including historical blobs from the batches
  the follower must replay. A synced head alone does not prove blob availability.

The follower needs no proof-signing key, funding or signer registration. A
consensus client must not drive its L2 Engine API; the EEZ follower owns forkchoice.
Keep L1 execution and Beacon services separate from the L2 follower.

Use an SSD data directory with enough room for archive state, receipts, logs and
backups. The configuration below disables pruning. It has no fixed disk bound.
Size and monitor resources for the selected chain and traffic.

## First setup

Run from the repository root. This procedure uses
[`docker-compose.nethermind-follower.yml`](../../docker-compose.nethermind-follower.yml)
and [`.env.nethermind.example`](../../.env.nethermind.example), separate from the
Composer's `.env`. It requires Docker Compose v2.

1. Use a shell without exported Composer profile settings. Exported environment
   variables override `--env-file`; do not let another network's `EEZ_*` values
   select this follower's chain or endpoints.
2. Create the configuration and data directories on the intended data disk:

   ```sh
   mkdir -p datadir nethermind-config data/nethermind/db data/nethermind/logs data/nethermind/keystore
   test -e .env.nethermind || cp .env.nethermind.example .env.nethermind
   chmod 600 .env.nethermind
   ```

3. Copy the deployment's L2 genesis to `datadir/genesis.json`. Preserve an existing
   genesis and database. Use a separate data directory for another network.
   Fill every blank value in `.env.nethermind` from the deployment bundle. Keep
   the image digest matched to the network's qualified build. `EEZ_L2_BLOCK_TIME_SECONDS`
   is in whole seconds. L1 and sequencer URLs must be reachable from inside the
   container; its loopback address does not reach a service on the host.
4. For Chiado, keep `EEZ_L1_CHAIN_CONFIG=chiado`. For Ethereum, copy the complete
   L1 genesis to `nethermind-config/l1.json` and set
   `EEZ_L1_CHAIN_CONFIG=/config/l1/l1.json`. Keep `EEZ_L1_CONFIG_DIR` limited to
   public chain configuration; do not mount a directory of credentials.
5. Give the image's runtime user write access to the data directory. Set
   `NETHERMIND_DATA_DIR` to the data disk path if using another location. The
   configuration disables pruning and has no fixed disk bound.
6. Render and start the follower:

   ```sh
   docker compose --env-file .env.nethermind -f docker-compose.nethermind-follower.yml config --quiet
   docker compose --env-file .env.nethermind -f docker-compose.nethermind-follower.yml pull nethermind-follower
   docker compose --env-file .env.nethermind -f docker-compose.nethermind-follower.yml up -d nethermind-follower
   docker compose --env-file .env.nethermind -f docker-compose.nethermind-follower.yml logs --tail=100 nethermind-follower
   ```

Only RPC and metrics are published, both on host loopback. Engine is unpublished
and binds to container loopback. No proof key is passed to the follower. Keep
`.env.nethermind`, the database and keystore private. Do not enable account
unlocking or signing RPC methods. Static Compose rendering is checked; this
packaged procedure still needs a fresh-host rehearsal.

## Verify progress

Call the follower's RPC:

```sh
curl --fail --silent --show-error http://127.0.0.1:8545 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}'
curl --fail --silent --show-error http://127.0.0.1:8545 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["latest",false]}'
```

Check `eth_getBlockByNumber` for `0x0`, `latest`, `safe` and `finalized`:

1. Chain ID and genesis hash must equal the deployment bundle.
2. Compare each local block at its exact height with the network source. Hash,
   state root, transaction root and receipt root must match. Compare transaction
   receipts too. Equal heights alone are insufficient.
3. `finalized <= safe <= latest`. Safe and finalized depend on L1 settlement and
   finality. They need not advance with every unsafe L2 block.
4. Repeat the check. With `SequencerRpcUrl`, latest should follow the sequencer
   between settlements. Waiting hours for the next settlement is not healthy
   unsafe following.
5. Check container restart count, errors and disk space. After a clean restart,
   repeat the exact-height and receipt comparisons.

Stop and investigate any divergent canonical hash or root. Do not disable
validation, alter the genesis, skip missing blobs or erase the database to make
a health check pass. A stale L1, unavailable historical blobs or unreachable
sequencer must be corrected at its source.

## Restart, upgrade and recovery

Use `docker compose --env-file .env.nethermind -f docker-compose.nethermind-follower.yml stop nethermind-follower`
and wait for clean database closure before
copying the database. Preserve genesis, configuration and keystore with the backup.
The same command with `start nethermind-follower` resumes the same database.

Before upgrading, record the image digest and source revision, verify the new
build against the network's current rules, and take a stopped database snapshot.
If the new process writes to the database, changing the image tag alone is not a
safe rollback. Restore the matching stopped snapshot and configuration. A network
redeployment or incompatible genesis requires a separate data directory.
