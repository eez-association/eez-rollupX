# eez-rollup0

An L2 rollup where L1 and L2 can call into each other as part of the same
flow. Four components do the work: a **sequencer** produces L2 blocks (it
drives a stock [reth](https://github.com/paradigmxyz/reth) node), a
**composer** posts those blocks to the `EEZ` contract on L1, and a
**proof signer** independently re-executes and attests them. A **deriver** can
rebuild the whole L2 chain from L1 alone. The supported L1 is Gnosis **Chiado**
(a testnet).

## What it does

- **Produces L2 blocks** on a schedule tied to L1: ordinary blocks most of
  the time, plus one special **Sync block** per L1 block that carries the
  cross-chain work.
- **Routes cross-chain calls.** A transaction aimed at a cross-chain proxy
  (or sent from L1) is held aside, simulated, and packed into the next Sync
  block. The matching L1 `postAndVerifyBatch` call and the user's L1 transactions
  are submitted together so they all land in the *same* L1 block — all or
  nothing.
- **Commits first, repairs if needed.** The Sync block is added to L2 right
  away; the L1 submission is watched in the background, and if it doesn't make
  it onto L1 the L2 block is rolled back. L1 is the source of truth — L2 only
  keeps what L1 confirms.
- **Can be re-derived from L1.** A **follower** rebuilds the identical L2
  chain just by reading L1 (the `BatchPosted` events) and re-running the same
  transactions — no need to trust the sequencer.

`eez-proof-signer` statelessly re-executes each proposed batch and validates
its settlement effects before signing the recomputed public-input hash.
`ECDSAProofSystem` verifies that hash-bound attestation on L1. This is not yet
a succinct validity proof, but the deployed verifier no longer accepts an
unbound mock signature.

The [Composer-to-prover gRPC specification](crates/eez-control-rpc/SPEC.md)
defines request construction, response validation, and settlement recovery for
the current native-blob profile.

## Run the local protocol network

The Kurtosis package starts the private L1, L2, protocol contracts, proof
signer, explorers, and a copy of the earlier `sync-rollups-poc` UI with a thin
compatibility layer for this network:

```bash
git submodule update --init --recursive eez-core-protocol
export KURTOSIS_ENCLAVE=eez-dev
bash testing/kurtosis/start.sh testing/kurtosis/ci-args.yaml
source testing/kurtosis/ports.sh
```

Open `$EEZ_DEVNET_DASHBOARD` to use the original Dashboard, Counter Demo,
Bridge, Flash Loan, Aggregator, Faucet, and Visualizer screens. Counter calls,
bidirectional bridging, and the L1 -> L2 -> L1 flash loan are adapted to the
current network. The Aggregator and reverse-flash screens are retained but this
protocol branch does not deploy their contracts, and the legacy visualizer's
batch decoder is still pending a `postAndVerifyBatch` port. The UI signs with a
known local-only development key; never use it on a public network. See
[`testing/kurtosis/README.md`](testing/kurtosis/README.md) for prerequisites,
test workloads, and lifecycle commands.

## Run a chiado L2 (Docker)

Runs three containers: `eez-node` (whose entrypoint is the `eez-composer`
binary, embedding a Chiado L1 node alongside the L2), `eez-proof-signer`, and a
**lighthouse** consensus client that drives the L1.
There's no separate L1 node to run. Cross-chain batches are submitted to
Chiado's block builder; the L1 block to aim them at is read from the embedded
L1 once it has caught up to the chain tip.

Chiado is post-Fulu. The deployment therefore posts native EEZ payloads in
type-3 blob transactions using the EIP-7594 pooled sidecar wrapper. Its
Lighthouse runs as a PeerDAS semi-supernode, custodying half the data columns
and reconstructing full blobs for the deriver through
`/eth/v1/beacon/blobs/{block_id}`. The deprecated pre-Fulu sidecar endpoint is
retained only as a compatibility fallback.

> **One command** (after the one-time setup + `.env`/`.env.chiado`): `bash
> scripts/chiado-up.sh` deploys the protocol (skipped if already deployed),
> prepares the datadirs, starts the stack, waits until it's healthy, and
> prints the RPC URLs. The numbered steps below are that same flow done by
> hand.

### One-time setup

```bash
git submodule update --init --recursive

# 1. Build the node and proof-signer images.
docker build -t eez-node:local .
docker build -f Dockerfile.signer -t eez-proof-signer:local .

# 2. Download a minimal chiado L1 snapshot (skips syncing from genesis).
mkdir -p data
docker run --rm -v "$PWD/data/chiado-l1:/data" \
  ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
  download --chain chiado --minimal --datadir /data

# 3. Shared engine-API JWT for eez-node <-> lighthouse.
openssl rand -hex 32 > data/jwt.hex

# 4. Current Chiado consensus config (including the activated Fulu epoch).
git clone https://github.com/gnosischain/configs.git /tmp/gnosis-configs
mkdir -p configs && cp -r /tmp/gnosis-configs/chiado configs/chiado

# 5. Fund the operator/poster keys with xDAI (https://faucet.chiadochain.net).
```

### Deploy the protocol (once, against a fully-synced chiado RPC)

The embedded L1 is still syncing at this point, so run the deploy against a
Chiado RPC that's already at the chain tip — the public endpoint, or a
standalone chiado-reth.

```bash
cp .env.example .env
#   EEZ_L1_RPC_URL=<tip chiado RPC>   EEZ_L1_POSTER_KEY=<operator key>
#   EEZ_PROOF_SIGNER_KEY=<operator key>   (its address becomes the proof system's authorizedSigner)
#   EEZ_L2_SYSTEM_KEY=<separate L2 system-transaction key>
EEZ_DEPLOY_SKIP_SIMULATION=1 make deploy-protocol
```

This deploys EEZ + ECDSAProofSystem + the rollup manager, registers the
rollup, deploys the L1 bridge contracts, and writes **`deployments.env`**
(registry, proof system, rollup id, deploy block, bridge, and EEZL2 addresses).
The deploy derives the public L2 system address from `EEZ_L2_SYSTEM_KEY`,
generates and funds its canonical EEZL2 genesis, and registers that exact state
root; the private key is never written to `deployments.env`. It also writes the
L2 **`datadir/genesis.json`** whose timestamp is pinned to the deploy block. The
container loads `deployments.env` automatically; `.env.chiado`'s
`FRESH_GENESIS` points at that genesis (default `./datadir/genesis.json`), so
**deploy must run before `up`** — there is no separate genesis-creation step.
(Set `EEZ_BLOCKSCOUT_URL` first to also verify the contracts on Blockscout —
opt-in.)

### Configure and start

```bash
cp .env.chiado.example .env.chiado    # paths, keys, builder, and blob settings
bash scripts/chiado-up.sh
```

The embedded L1 checkpoint-syncs (lighthouse, ~5 min) and catches up past the
deploy block; then the L2 sequencer + composer run. Startup refuses an implicit
DA mode, rejects a non-EIP-7594 Chiado blob configuration, probes the builder's
`eth_sendBundle` method, confirms the synchronized Beacon endpoint identifies
post-Fulu Chiado and serves the full-blob API, and waits for a new settlement.
In blob mode it additionally requires that settlement to be type 3 and checks
that every versioned-hash-selected blob is retrievable. Basic health checks:

```bash
cast block-number --rpc-url http://localhost:18645   # embedded chiado L1 climbing
cast block-number --rpc-url http://localhost:18688   # L2 producing
```

The static deployment wiring is also checked in CI and can be validated
without starting a network:

```bash
bash scripts/check-chiado-blob-config.sh
```

The colocated semi-supernode is sufficient for live operation and the normal
consensus-client retention window. It reconstructs full blobs on request, so
retrieval can be slower than from a supernode. Long-horizon recovery still
requires a durable archival Beacon source or a protocol snapshot/bootstrap
strategy; do not describe a Chiado deployment as independently recoverable
beyond that window until this is provided and tested.

### Endpoints & cross-chain fronts

| Endpoint | URL | Use |
|---|---|---|
| L2 RPC | `http://localhost:18688` | L2 `eth_*` |
| Embedded chiado L1 RPC | `http://localhost:18645` | L1 `eth_*` (the composer's L1 view) |
| **L1→L2 front** (Inbound) | `http://localhost:18999` | send L1-origin cross-chain txs here |
| **L2→L1 front** (Outbound) | `http://localhost:18998` | send L2-origin cross-chain txs here |

The two **cross-chain ingress fronts** are transparent, wallet-compatible
proxies. Each signed transaction is classified with a read-only call trace:
transactions that enter the source chain's EEZ cross-chain dispatcher are held
and composed into the next Sync block, while ordinary transactions are forwarded
unchanged to the source-chain mempool. Every other `eth_*` method is forwarded as
well, so a wallet can use one front URL per chain. Classification fails closed:
if tracing is unavailable, the transaction is held rather than allowing a
cross-chain call to bypass composition. The fronts use the compose env
`EEZ_L1_XCHAIN_PORT` / `EEZ_L2_XCHAIN_PORT`; both ports are required in composer
mode. The `eez-follower` binary does not start them.
Upstreams are `EEZ_L1_RPC_URL` / `EEZ_L2_RPC_URL` respectively.

`EEZ_MAX_USER_TXS_PER_BUNDLE` caps how many held user cross-chain transactions,
combined across both directions, the composer offers to one Sync block. The
in-code fallback and Kurtosis profile use `50`; the Chiado profile overrides it
to `12` for its smaller L1 gas limit. The composer drains the queue FIFO:
traffic beyond the cap remains queued for later Sync blocks, and block-gas
fitting may select fewer than the configured maximum. Treat the value as a
deployment-specific safety bound, not as a throughput target or an
unlimited-mode switch.

### Exercise it

`scripts/xchain-test.sh` is the cross-chain test driver (new-format successor to
`devnet-test.sh`). It deploys fresh targets/proxies/wrappers, drives the ingress
fronts through **both directions × all op types** (`setValue` / `setValueNoRet` /
`deposit` / `withdraw`) × **direct + wrapper**, then checks L1↔L2 reconciliation +
semantic effects and reports pipeline metrics (N+1 next-slot hit-rate,
consecutive-L1-slot landing, bundle drops/evictions, divergence). Every run mints
recipients + senders fresh, so re-runs never collide with stale state.

```bash
# MATRIX (default): waves of the full cross-chain matrix + pure-L2 + poison
EEZ_WAVE_COUNT=5 bash scripts/xchain-test.sh

# LOAD: high volume from distinct fresh senders, optionally paced + node restart
EEZ_MODE=load EEZ_IN_N=100 EEZ_OUT_N=100 bash scripts/xchain-test.sh            # burst
EEZ_MODE=load EEZ_IN_N=100 EEZ_OUT_N=100 EEZ_PACE_N=10 EEZ_PACE_INTERVAL=10 \
  bash scripts/xchain-test.sh                                                  # ~1 tx/s
EEZ_RESTART=1 EEZ_MODE=load ... bash scripts/xchain-test.sh                    # restart mid-run
```

(`scripts/devnet-test.sh` is the earlier, simpler driver — setter+deposit only,
raw-RPC — kept for reference.)

## Run node roles locally

`eez-composer` is the package default. Run it directly, or use `make run-node`:

```bash
cargo run -p eez-node -- node \
  --chain "$EEZ_L2_GENESIS_PATH" \
  --datadir "$EEZ_L2_DATADIR"
```

Select the non-default follower role explicitly. `EEZ_L1_CHAIN_ID` must match
the numeric chain ID served by `EEZ_L1_RPC_URL`. A follower also requires the
system signer and L2 execution identity so it can reconstruct Sync blocks:

```bash
# L1-derived follower; optionally add `--sequencer-rpc <URL>` after `node`.
EEZ_L1_CHAIN_ID="<numeric-l1-chain-id>" \
EEZ_L2_SYSTEM_KEY="<system-private-key>" \
EEZL2_ADDRESS="<eezl2-contract-address>" \
EEZ_ROLLUP_ID="<numeric-rollup-id>" \
cargo run -p eez-follower -- node \
  --chain "$EEZ_L2_GENESIS_PATH" \
  --datadir "$EEZ_L2_DATADIR"
```

### Partial consumption and recovery

Partial consumption and durable recovery are always enabled. Configure a writable
`EEZ_L2_DATADIR` and a timestamp-pinned bundle relay; all submissions use the relay.
Only actual signed L1 users following postBatch enter `revertingTxHashes`.
The required postBatch and its native blob sidecar are preserved. There is no
submission policy flag or raw-mempool fallback.

Permission to revert does not guarantee that a builder will omit invalid-nonce
transactions. The [pinned Kurtosis canary](testing/kurtosis/README.md#validate-partial-consumption)
checks both behaviors against the selected builder image and configuration.
No `droppingTxHashes` policy is exposed.

The composer records each submitted user's signed bytes and ownership in
`$EEZ_L2_DATADIR/settlement-journal.json` before dispatch. Recovery combines a
complete canonical L1 scan, validated L2 replay groups and canonical source
receipts/nonces. It preserves the applied prefix, never retries spent source
nonces, and requeues eligible omissions once with the existing bounded budget.
Composer startup requires a usable datadir and opens the journal before launching
either reth instance. A corrupt, locked or unwritable journal stops startup;
later write failures stop recovery and dispatch without discarding ownership.

Recovery reads source state sequentially and requires the canonical generation
to remain stable through those reads. With a consistently slow RPC, a large
batch can keep recovery pending across successive scans. Monitor the settlement
backlog and reduce `EEZ_MAX_USER_TXS_PER_BUNDLE` (default: 50) or improve RPC
latency before increasing the load.

Keep the datadir and journal across restarts. The journal does not persist ingress
that has never entered a submitted attempt. Details are in
[the composer specification](crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission)
and the [default-behavior validation report](docs/audits/2026-09-11-partial-consumption/default-policy.md).

### L1/L2 settlement correlation RPC

Composer and follower L2 RPCs expose the exact canonical mapping derived from
accepted `postAndVerifyBatch` transactions:

```bash
# L2 number or 32-byte hash -> its L1 posting block/transaction and exact range.
curl -s http://localhost:18688 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eez_getSettlementByL2Block","params":["0xcc"]}'

# L1 number or 32-byte hash -> zero or more settled L2 ranges.
curl -s http://localhost:18688 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":2,"method":"eez_getSettledL2RangesByL1Block","params":["0x23"]}'
```

Selectors also accept `{"blockNumber":"0xcc"}` or
`{"blockHash":"0x…"}`. Numbers are Ethereum hex quantities. An unknown L2
block returns `null`; an L1 block with no accepted batches returns `[]`.
Responses include every exact L2 block hash, the posting transaction hash and
index, `canonicalL2`, and `l2Finalized`. A same-height replacement remains
discoverable by its old hash but is marked non-canonical.

Set `EEZ_L2_DATADIR` to retain the MDBX index under
`$EEZ_L2_DATADIR/eez-correlations`. On its first run after this feature is
enabled, the deriver performs one full L1 scan to backfill history. With no
configured datadir the methods still work for batches observed during that
process, but their history does not survive restart.

This mapping means “the L1 transaction that published and settled this L2
block.” It is distinct from an L1-origin field embedded in every L2 header,
which this protocol does not currently define.

## Build, test, teardown

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                  # Rust; `cd contracts && forge test` for Solidity
bash scripts/teardown-chiado.sh         # stop eez-node + eez-proof-signer + lighthouse (datadirs untouched)
```
