# Run a Composer on Ethereum (mainnet, Hoodi or Sepolia)

**Scope.** Start and check the packaged Composer, proof signer and Lighthouse for
an L2 whose L1 is a public Ethereum network, using
[`docker-compose.ethereum-node.yml`](../../docker-compose.ethereum-node.yml) and
[`.env.ethereum.example`](../../.env.ethereum.example). One `EEZ_L1_NETWORK` value
selects the same network for all three services, so a Hoodi or Sepolia rehearsal
uses the mainnet definition unchanged.

**Not covered.** Deploying or registering contracts and rendering the L2 genesis
(`deployments.env` and `datadir/genesis.json` must already exist), explorers and
monitoring. Registration is irreversible: rehearse it on Hoodi first.

**Verification status.** Static checks only: Compose rendering
(`scripts/ci/tests/test_ethereum_config.py`,
`scripts/check-settlement-policy-config.py`), the embedded-L1 and signer network
selection unit tests, and builder acceptance probes (2026-10-02). Not yet run
against a synced Ethereum node; no settlement on Ethereum has been observed.

## Topology

- `eez-node` embeds its own **reth v2.7** as the L1 execution client
  (`EEZ_L1_CHAIN=<network>`, reth's built-in chain spec). It does not connect to
  an external execution client. Its datadir is `L1_SNAPSHOT_DIR`.
- `lighthouse` drives that reth through the engine API on `L1_AUTH_PORT` (shared
  `JWT_FILE`) and runs as a **semi-supernode**, so the deriver can read full
  PeerDAS blobs from `127.0.0.1:6152`.
- `eez-proof-signer` trusts the same network (`EEZ_L1_CHAIN_CONFIG=<network>`).
- The schedule is fixed by the genesis profile: 12-second L1 slots, 2-second L2
  blocks (six per slot).
- Host networking with the Chiado profile's ports: run one packaged network per
  host.

## Prerequisites

1. `deployments.env` and the registered `datadir/genesis.json` (mode `644`).
2. **L1 execution datadir.** Either:
   - a **reth** datadir already synced for the network. Stop the reth that uses
     it first: the Composer becomes its only writer. Confirm reth v2.7 can open
     it, and that its pruning keeps history from `EEZ_REGISTRY_DEPLOY_BLOCK`; or
   - an empty directory. A mainnet sync takes days, and the default (archive)
     storage needs several TB.

   A non-reth datadir (geth, Nethermind, Erigon…) cannot be reused.
3. **Lighthouse.** An empty `LIGHTHOUSE_DATA` checkpoint-syncs from
   `LH_CHECKPOINT_URL`. An existing datadir can be reused with its JWT. Switching a
   regular node to semi-supernode backfills data columns before blobs are complete.
4. Keys: a funded poster, the registered proof signer, and a **dedicated unfunded**
   `EEZ_L1_BUNDLE_SIGNING_KEY` (BuilderNet rejects unsigned bundles).
5. **Settlement policy and fee limits.** The example sets no Ethereum fee values.
   The node refuses to start until `EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS`,
   `EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS` and the priority bounds are chosen
   ([policy reference](../configurable-settlement-policy.md)).

## First start

From the repository root, in a shell that has not exported another profile's
`.env` (shell variables override `.env`):

```bash
test -f .env || cp .env.ethereum.example .env      # then edit every CHANGE_ME / <full-commit>
openssl rand -hex 32 > data/jwt.hex                 # only if not reusing a Lighthouse JWT
docker compose config --quiet                       # fails on any missing required value
docker compose pull
docker compose up -d
docker compose ps
```

For a rehearsal set `EEZ_L1_NETWORK=hoodi`, `EEZ_L1_CHAIN_ID=560048`,
`LH_CHECKPOINT_URL=https://checkpoint-sync.hoodi.ethpandaops.io` and
`EEZ_L1_BUILDER_RPC_URL=https://rpc-hoodi.titanbuilder.xyz`.

## Checks

```bash
docker logs eez-node-ethereum 2>&1 | grep -E 'l1_embedded.(launching|ready)'   # kind="ethereum", l1_chain_id=<id>
cast chain-id --rpc-url http://127.0.0.1:18645                                    # embedded L1
cast block latest -f hash --rpc-url http://127.0.0.1:18645                        # equals (or trails by 1-2) a public RPC
curl -s http://127.0.0.1:6152/eth/v1/node/syncing | jq .data                      # is_syncing=false
cast block-number --rpc-url http://127.0.0.1:18688                                # L2 height advancing
```

**Stop conditions:**
- **Unexpected L1 kind or chain ID:** if the embedded L1 reports a different chain ID or `kind`, stop. `EEZ_L1_CHAIN` must be exactly `mainnet`, `hoodi` or `sepolia`; any other value is a startup error.
- **Signer refuses to start:** a `prover validation identity differs` error means the node and signer images come from different commits.
- **Invalid builder signature:** `Invalid signature` replies from BuilderNet mean `EEZ_L1_BUNDLE_SIGNING_KEY` is missing.

Bundle acceptance is not inclusion. Measure the share of settlement slots that
land before relying on a builder set.

## Stop and restart

`docker compose down` keeps every datadir. Pending requests and retry counters
are in memory: a restart reconciles canonical state, and clients may need to
resubmit unsettled work.
