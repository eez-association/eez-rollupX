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
     it, and that it retains the history required for
     [recovery](#recovery-with-a-pruned-l1); or
   - an empty directory. A mainnet sync takes days, and the default (archive)
     storage needs several TB.

   A non-reth datadir (geth, Nethermind, Erigon…) cannot be reused.
3. **Lighthouse.** An empty `LIGHTHOUSE_DATA` checkpoint-syncs from
   `LH_CHECKPOINT_URL`. An existing datadir can be reused with its JWT. Switching a
   regular node to semi-supernode backfills data columns before blobs are complete.
4. Keys: a funded poster, the registered proof signer, and a **dedicated unfunded**
   `EEZ_L1_BUNDLE_SIGNING_KEY` (BuilderNet rejects unsigned bundles).
5. **Settlement policy and fee limits.** Review the example's starting values
   before deploying ([policy reference](../configurable-settlement-policy.md)):
   - **Price ceilings:** 10 gwei (base fee + tip) and 2 gwei (blob). A post above a ceiling waits and retries; it never pays more.
   - **Tip:** the median of recent blocks' median tips (`EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE=50`), kept within 0.05–2 gwei.
   - **Cross-chain tip:** bundles that execute cross-chain calls on L1 bid p90 of p90 within 0.05–3.5 gwei (`EEZ_L1_POSTBATCH_CROSS_CHAIN_*`), so they more often run before same-block transactions that change the L1 state they read ([cross-chain priority](../configurable-settlement-policy.md#cross-chain-priority)).
   - **Cadence:** ordinary L2 activity settles at most every 15 minutes; an idle chain posts every 4 hours.
   - **Proof window:** `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS=8192`, so an idle cycle needs one post.

   Confirm proof time for a full window and the timing budgets in the rehearsal. The
   poster balance is the only total spending limit.

Cross-chain fee collection uses the shared [fee configuration](../configurable-settlement-policy.md#cross-chain-fee-configuration).
Fee policies have no expiration. Remove the obsolete
`EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP` setting before upgrading; its presence,
including empty, prevents startup. Follow the
[expiration migration](../configurable-settlement-policy.md#removed-fee-policy-expiration)
when upgrading an existing deployment or rolling back Composer.

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

## Recovery with a pruned L1

Preserve the L2 datadir, including its persistent L1/L2 correlation index. With
a valid saved checkpoint, startup scans from its containing L1 block, not from
`EEZ_REGISTRY_DEPLOY_BLOCK`. The checkpoint block is included: retain its receipts,
logs and transaction bodies, subsequent history, and required native blobs.
Older canonical headers are still needed to authenticate the genesis reference
and restore indexed settlement history. Allow additional retention for downtime
and reorg recovery to an earlier checkpoint; the latest L1 head is not a safe
pruning boundary.

Retained headers do not guarantee that `eth_getBlockByNumber` can serve old
blocks: Reth also requires their bodies, and its `eth_getHeaderByNumber` follows
the same path. On pruning error code `4444`, genesis and derivation authentication
fall back to `debug_getRawHeader` at the same canonical height/tag. The reader
exact-decodes the RLP, recomputes its hash, and retains the height, genesis identity
and finality checks. The embedded profile already enables this RPC. An external
source must expose it when block bodies have expired, or serve the full historical
blocks. Missing/malformed raw headers still fail authentication.

A fresh node, missing index, or rejected checkpoint can require history back to
deployment. This fix does not recover already-pruned data. Increasing retention
does not restore it either. If the selected recovery range is unavailable, use
a history-serving source or a consistent supported backup; do not advance the
deployment block, delete the index, or reset L2 to bypass validation. Execution
RPC history does not replace missing Beacon blob data.

Unavailable history now fails during catch-up rather than the deployment-block
preflight. Readiness stays closed while catch-up retries; persistent RPC errors
(including the receipt-pruning error) exhaust the existing 32-failure budget and
exit. Repeated restarts cannot restore missing history.

Check startup in order:

1. `L1 chain and finality ready` means preflight only, not successful recovery.
2. `boot seeded from the persistent L1/L2 correlation index` reports the anchor
   in `l1_block`; `starting batch scan` reports the inclusive `from_l1_block`.
   Without a usable checkpoint, expect a scan starting at deployment.
3. Require `spawning eez composer umbrella` (or `spawning eez deriver` for a
   follower) after catch-up. Check advancing L2 blocks, a subsequent canonical
   settlement and independent replay; container/RPC health alone is insufficient.

Before returning to local RPC, verify address-filtered `eth_getLogs` at the
checkpoint and availability of the full recovery range, current state and blobs.
Preserve datadirs and the working configuration for rollback. Upgrade the signer
as well as the Composer and followers: each authenticates historical L1 headers.
The signer can upgrade first; the proof wire format, protocol pin, genesis and
database format are unchanged. Older readers still fail on pruned block bodies;
binaries predating checkpoint-based preflight also require deployment-era logs.
Rolling back can therefore require a history-serving source again.

The shared registry may contain other rollups with different DA formats. Readers
exclude posts whose `BatchPosted.rollupIds` do not include their configured rollup
before native-blob validation. Every post that does include it remains a queue
replacement boundary, even without a root change. Upgrade Composer and follower
readers together before relying on replay past a foreign blob-free post; older
readers can halt at that transaction again. Verify recovery with a new canonical
settlement and independent follower replay, not just an advancing speculative head.
