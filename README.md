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

Start with the [Chiado quick start](docs/runbooks/chiado-quickstart.md): configure
`.env`, deploy with the published deploy image, and start the services.
`:latest` is the default; use full Git commit tags or digests for a fixed version.
`:local` builds are for development. Use the
[detailed runbook](docs/runbooks/chiado-deployment.md) for preflight checks,
restart, troubleshooting, and recovery limits.
To sync and run a Composer for an already registered L2, follow the
[existing-rollup procedure](docs/runbooks/chiado-existing-rollup.md).

First deployment must generate the registered genesis before startup. Restarting
an existing rollup must preserve its deployment bindings, genesis, keys, and data.

### Optional explorers

Use the [explorer runbook](docs/runbooks/explorers.md) to run L2 Blockscout,
Chiado Blobscan, or both against an existing deployment. They use a separate
Compose project and `.env.explorers`, are operated outside CI, and connect
directly to the node RPC. Preparation reads the L2 chain ID from genesis and
configures Blobscan history from your chosen L1 starting block.

### Staged performance workload

For the 770-job / 870-source staged performance workload against an existing
Chiado deployment, configure `eez-core-protocol/chain.env` (or set
`DEVNET_ENV` to its absolute path), then run:

```bash
bash scripts/run-chiado-staged-performance.sh --check # read-only readiness
bash scripts/run-chiado-staged-performance.sh         # fund, prepare, send, monitor
```

The [helper](scripts/run-chiado-staged-performance.sh) uses the existing test
wallets, locks the shared worker pool for the complete run, and defaults to
`PREPARE_PARALLEL=200`, `FUND_ETH=0.005`, and `FLOOR_ETH=0.005`. It passes
`--no-verify` before the 14 scenario targets, retaining upstream preparation
and receipt monitoring while skipping semantic verification. The upstream run
directory under `eez-core-protocol/tmp/e2e-staged-net/` contains its phase and
job timings. The reported faucet reserve includes refundable funds and is not
the expected cost. Do not start another upstream network runner concurrently.

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
unchanged to the source-chain mempool. The fronts also prepare cross-chain
`eth_estimateGas` calls locally; other `eth_*` methods are forwarded, so a wallet
can use one front URL per chain. Classification fails closed:
if tracing is unavailable, the transaction is held rather than allowing a
cross-chain call to bypass composition. The fronts use the compose env
`EEZ_L1_XCHAIN_PORT` / `EEZ_L2_XCHAIN_PORT`; both ports are required in composer
mode. The `eez-follower` binary does not start them.
Upstreams are `EEZ_L1_RPC_URL` / `EEZ_L2_RPC_URL` respectively.

For wallet gas estimation, supply `from`, `to`, calldata and any intended value,
nonce, fees and access list. Estimate and submit through the same source-chain
front. A cross-chain estimate covers **only the user's source transaction**;
separate system transactions, posting and blob fees are excluded. Omitted block,
`latest` and `pending` use an isolated next Composer context, without queued
predecessors or a slot reservation. Historical/state-override cross-chain calls
are unsupported. Ordinary calls retain upstream estimation after local dispatch
classification; unavailable local history/context returns an error. Numeric
historical blocks and state overrides can be classified; `safe`, `finalized` and
EIP-1898 selectors are unavailable in this first version. Ordinary `pending`
classification observes local head state, not upstream mempool transactions.

L1 estimation runs real registry preparation with one simulation-only proof
assumption; it does not generate or verify a proof. Actual submission still
requires the signer and final native preflight. Both fronts share two estimate
workers, a five-second execution deadline and at most three gas probes. Busy,
reconciling or changed contexts return an error: retry after the context becomes
available. A supplied gas field is a ceiling. Estimates remain advisory when
state, fee fields or bundle position change. See the
[gas estimation design](docs/proposals/composer-gas-estimation.md) for supported
execution, validation and upgrade boundaries.

`EEZ_MAX_USER_TXS_PER_BUNDLE` caps how many held user cross-chain transactions,
combined across both directions, the composer offers to one Sync block. The
in-code fallback and Kurtosis profile use `50`; the Chiado env preset selects
`auto` with settlement policy enabled. Automatic admission uses measured
preparation time and gas/blob capacity to choose a finite FIFO prefix. A numeric
override retains the fixed count cap; traffic beyond the accepted prefix stays
queued for later Sync blocks. The [policy guide](docs/configurable-settlement-policy.md)
describes the active resource and deadline checks.

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
fixed native system address and L2 execution identity to reconstruct Sync blocks:

```bash
# L1-derived follower; optionally add `--sequencer-rpc <URL>` after `node`.
EEZ_L1_CHAIN_ID="<numeric-l1-chain-id>" \
EEZ_L2_SYSTEM_ADDRESS="0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076" \
EEZL2_ADDRESS="<eezl2-contract-address>" \
EEZ_ROLLUP_ID="<numeric-rollup-id>" \
cargo run -p eez-follower -- node \
  --chain "$EEZ_L2_GENESIS_PATH" \
  --datadir "$EEZ_L2_DATADIR"
```

### Partial consumption and recovery

Partial consumption is always enabled. Configure one or more timestamp-pinned
bundle relays in `EEZ_L1_BUILDER_RPC_URL` (comma-separated); every submission
sends the identical bundle to all of them, as described in the
[policy guide](docs/configurable-settlement-policy.md#builder-relay-fan-out).
Only actual signed L1 users following postBatch enter `revertingTxHashes`.
The required postBatch and its native blob sidecar are preserved. There is no
submission policy flag or raw-mempool fallback.

Permission to revert does not guarantee that a builder will omit invalid-nonce
transactions. The [pinned Kurtosis canary](testing/kurtosis/README.md#validate-partial-consumption)
checks both behaviors against the selected builder image and configuration.
No `droppingTxHashes` policy is exposed.

The composer tracks each submitted user's signed bytes and ownership in memory
before dispatch. Reconciliation combines a
complete canonical L1 scan, validated L2 replay groups and canonical source
receipts/nonces. It preserves the applied prefix, never retries spent source
nonces, and requeues eligible omissions once with the existing bounded budget.

Recovery reads source state sequentially and requires the canonical generation
to remain stable through those reads. With a consistently slow RPC, a large
batch can keep recovery pending across successive scans. Monitor the settlement
backlog and reduce `EEZ_MAX_USER_TXS_PER_BUNDLE` (default: 50) or improve RPC
latency before increasing the load.

After a restart, the node reconstructs canonical state from L1 and discards the
local speculative suffix. Pending users and
retry counters are not persisted; users may need to resubmit transactions that
have not settled. `EEZ_L2_DATADIR` remains optional for checkpoint and correlation
caching. Details are in
[the composer specification](crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).

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

### Composer discovery RPC

Composer nodes (not followers) also expose `eez_composerInfo` on **both
cross-chain fronts**, the L1 and L2 endpoints clients submit through
(`EEZ_L1_XCHAIN_PORT` / `EEZ_L2_XCHAIN_PORT`, 18999 / 18998 in the Chiado
profile), so clients can discover the deployment instead of hardcoding it. The
front answers it locally, also before L1 catch-up completes; it is not served by
the L2 node RPC, and batches containing it are rejected like other non-`eth_*`
methods. It takes no
parameters (absent, `[]` or `{}`; any argument is rejected with `-32602`) and
returns the configured contracts, the L1/L2 chain IDs and the Composer version:

```json
{
  "eezContracts": {
    "eezRegistryAddress": "0x…",
    "eezRollupManagerAddress": "0x…",
    "eezL1BridgeSender": "0x…",
    "eezL2Address": "0x…",
    "eezL2BridgeReceiver": "0x…"
  },
  "supportedNetworks": { "eezL1": 10200, "eezL2": 10201 },
  "version": "0.1.0"
}
```

The method and field names match upstream `eez-association/eez-rollup0`
[#164](https://github.com/eez-association/eez-rollup0/pull/164). Addresses are
the stable **proxy** addresses this fork deploys: `eezRegistryAddress` is
`EEZ_REGISTRY_ADDRESS`, `eezL2Address` is `EEZL2_ADDRESS` and
`eezRollupManagerAddress` is `EEZ_ROLLUP_MANAGER_ADDRESS` (all written by
`scripts/deploy.sh`); implementation addresses are never advertised. The rollup
manager and the bridge fields (`EEZ_L1_BRIDGE_SENDER`, `EEZ_L2_BRIDGE_RECEIVER`)
are omitted when unset or empty; a malformed or zero value fails startup.
`eezL1` is the embedded L1's chain ID and `eezL2` the L2 chain specification's.
The response is fixed at startup.

## Build, test, teardown

Use Rust 1.95.0 and the checked-in lockfile. Reth is pinned to upstream v2.7.0 commit
`3d592ece6de8c4559987416a544fc215fd6d6921`, which includes the
forkchoice fix in [#27429](https://github.com/paradigmxyz/reth/pull/27429). Gnosis uses unmodified
[upstream PR #138](https://github.com/gnosischain/reth_gnosis/pull/138), and Stateless
uses the [native replay compatibility branch](https://github.com/eez-association/stateless/tree/0bda6b200c1eb4689c147c080c3c248b50be2ea8).
The checked-in lockfile resolves these dependencies. Private `testing` and
`testing-gnosis` L1 miners retain the previous two-block persistence threshold, without state
masking or an in-memory buffer. They have no consensus peers to recover a lost
chain after a crash. Consensus-driven `devnet` and `chiado` launchers use Reth's
upstream persistence defaults. Recent unpersisted blocks can still be lost on
an abrupt stop; database persistence is not durable queue recovery.

The upstream default build features require LLVM 22, Polly, Clang 22 and `m4`.
Docker builds install them automatically. For a Debian/Ubuntu source build:

```bash
sudo bash scripts/ci/install-llvm.sh
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH="$LLVM_SYS_221_PREFIX/bin:$PATH"
cargo +1.95.0 build --locked --workspace
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 test --locked --workspace                  # Rust; `cd contracts && forge test` for Solidity
bash scripts/teardown-chiado.sh         # stop eez-node + eez-proof-signer + lighthouse (datadirs untouched)
```
