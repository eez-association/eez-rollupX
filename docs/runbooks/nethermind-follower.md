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
`a75cf55f016595162fa297a9e99d91ef477e01cf`, published as:

```text
nethermindeth/nethermind:eez-a75cf55f016@sha256:75ac0f1157ef0f92ce2aea7389e0d48265d4be2b3536255224d4df7242056fa2
```

[Live integration](https://github.com/NethermindEth/nethermind/actions/runs/41116587960)
and [streaming conformance](https://github.com/NethermindEth/nethermind/actions/runs/41116587786)
passed against RollupX `c702f90da19cc149890d0a17a4ef6c200dcb6a21`.
Those results do not qualify a later upstream revision or deployment.

Two operated followers use this build: a Chiado-backed network with L2 chain ID
699169 and an Ethereum-backed network with L2 chain ID 696991. These are both
RollupX networks. Confirm the current network bundle with its operator; names
such as "mainnet" do not identify the L1 or establish production readiness.

P2P discovery, snap state serving and snap bootstrap have not yet been qualified
for this topology. This procedure deliberately uses L1 derivation and the
sequencer RPC. Do not advertise it as a snap bootnode or assume that a stock
execution client can join this L2 from the genesis file alone.

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

Use an empty directory on the intended data disk. Preserve an existing database
and configuration; do not copy another execution client's database into it.

Create `config/`, `db/`, `logs/` and `keystore/`. Put the supplied genesis at
`config/genesis.json` and the L1 genesis at `config/l1.json` if using a file.

Create `config/nethermind.json` from the following template. Replace every
`REPLACE_*` field. Numeric fields must be JSON numbers, not strings. Set
`Init.GenesisHash` to the supplied L2 genesis hash; do not derive it with an
Ethereum-only tool that does not implement the selected EEZ header rules.

```json
{
  "Init": {
    "ChainSpecPath": "/config/genesis.json",
    "GenesisHash": "REPLACE_L2_GENESIS_HASH",
    "BaseDbPath": "/nethermind/nethermind_db",
    "LogDirectory": "/nethermind/logs",
    "DiscoveryEnabled": false,
    "PeerManagerEnabled": false,
    "WebSocketsEnabled": false
  },
  "KeyStore": {
    "KeyStoreDirectory": "/nethermind/keystore"
  },
  "Sync": {
    "NetworkingEnabled": false,
    "SynchronizationEnabled": false,
    "FastSync": false,
    "SnapSync": false
  },
  "Eez": {
    "Enabled": true,
    "SkipConsensusLayerCalls": true,
    "FollowerEnabled": true,
    "SequencerRpcUrl": "REPLACE_SEQUENCER_RPC",
    "L1RpcUrl": "REPLACE_L1_EXECUTION_RPC",
    "L1BeaconUrl": "REPLACE_L1_BEACON_API",
    "L1ChainId": "REPLACE_WITH_NUMERIC_L1_CHAIN_ID",
    "L1ChainConfigPath": "/config/l1.json",
    "RegistryAddress": "REPLACE_REGISTRY_ADDRESS",
    "RegistryDeployBlock": "REPLACE_WITH_NUMERIC_DEPLOYMENT_BLOCK",
    "RollupId": "REPLACE_WITH_NUMERIC_ROLLUP_ID",
    "L2BlockTimeSeconds": "REPLACE_WITH_NUMERIC_BLOCK_TIME",
    "L2GasLimit": "REPLACE_WITH_NUMERIC_GAS_LIMIT",
    "L1PollingIntervalMs": 2000,
    "L1LogScanBlocks": 128
  },
  "Pruning": { "Mode": "None" },
  "Receipt": { "TxLookupLimit": 0 },
  "JsonRpc": {
    "Enabled": true,
    "Host": "0.0.0.0",
    "Port": 8545,
    "EngineHost": "127.0.0.1",
    "EnginePort": 8551,
    "JwtSecretFile": "/nethermind/keystore/jwt-secret",
    "EnabledModules": ["Eth", "Net", "Web3"]
  },
  "Metrics": {
    "Enabled": true,
    "ExposeHost": "0.0.0.0",
    "ExposePort": 8008
  }
}
```

For Chiado, set `Eez.L1ChainConfigPath` to `chiado` instead of `/config/l1.json`.
Use endpoints reachable from inside the container. Its loopback address refers
to the container, not an L1 service on the host.

Create `compose.yaml`:

```yaml
services:
  follower:
    image: nethermindeth/nethermind:eez-a75cf55f016@sha256:75ac0f1157ef0f92ce2aea7389e0d48265d4be2b3536255224d4df7242056fa2
    restart: unless-stopped
    stop_grace_period: 3m
    command: ["--config=/config/nethermind.json"]
    volumes:
      - ./config:/config:ro
      - ./db:/nethermind/nethermind_db
      - ./logs:/nethermind/logs
      - ./keystore:/nethermind/keystore
    ports:
      - "127.0.0.1:8545:8545"
      - "127.0.0.1:8008:8008"
```

Give the image's runtime user write access to `db/`, `logs/` and `keystore/`.
The Compose file publishes only local RPC and metrics. Keep Engine, database
files and credentials private. Do not enable account-unlocking or signing RPC.

From the directory containing `compose.yaml`:

```sh
docker compose config --quiet
docker compose pull follower
docker compose up -d follower
docker compose logs --tail=100 follower
```

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

Use `docker compose stop follower` and wait for clean database closure before
copying the database. Preserve genesis, configuration and keystore with the backup.
`docker compose start follower` resumes the same database.

Before upgrading, record the image digest and source revision, verify the new
build against the network's current rules, and take a stopped database snapshot.
If the new process writes to the database, changing the image tag alone is not a
safe rollback. Restore the matching stopped snapshot and configuration. A network
redeployment or incompatible genesis requires a separate data directory.
