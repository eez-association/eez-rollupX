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
5. **Settlement policy and fee limits.** Review the example's starting values
   before deploying ([policy reference](../configurable-settlement-policy.md)):
   - **Price ceilings:** 10 gwei (base fee + tip) and 2 gwei (blob). A post above a ceiling waits and retries; it never pays more.
   - **Tip:** the median of recent blocks' median tips (`EEZ_L1_POSTBATCH_PRIORITY_PERCENTILE=50`), kept within 0.05–2 gwei.
   - **Cadence:** ordinary L2 activity settles at most every 15 minutes; an idle chain posts every 4 hours.
   - **Proof window:** `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS=8192`, so an idle cycle needs one post.

   Confirm proof time for a full window and the timing budgets in the rehearsal. The
   poster balance is the only total spending limit.

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

### Why a post was excluded

The Composer logs `bundle dropped … reason=excluded` without saying why.
[`scripts/why-excluded.py`](../../scripts/why-excluded.py) investigates excluded
targets using Composer JSON logs, canonical block data, relay deliveries and
builder bundle stats. It uses only Python's standard library and makes read-only
requests; it does not send bundles or change the Composer. The built-in relay
and stats endpoints serve **Ethereum mainnet**. The script checks `eth_chainId`
and rejects other networks, including Hoodi and Sepolia.

```bash
source .env
sudo docker logs --since 2h eez-node-ethereum 2>&1 | \
  python3 scripts/why-excluded.py --logs - --builders "$EEZ_L1_BUILDER_RPC_URL"
# Explicit targets provide block/relay context, without logged bundle hashes.
python3 scripts/why-excluded.py 26131577 26131674 --builders "$EEZ_L1_BUILDER_RPC_URL"
```

Pass the builder list explicitly as above: sourcing a Compose `.env` does not
export its variables to Python. Use logs from one Composer and the builder list
it used for those targets. The script retains the latest observed bundle hash
per builder and target height; it is not a journal of every submission attempt.
`--rpc` defaults to the embedded L1 (`http://127.0.0.1:18645`). A saved log file
can replace `-`; preserve it if the container's logs are about to rotate.

| Category | Meaning | Action |
| --- | --- | --- |
| `POSSIBLY_LOCAL` | Client-like `extraData`; all listed relays answered but none reported this block | Local construction is only a possibility: an unlisted relay may have delivered it |
| `OTHER_BUILDER` | A relay delivered this exact block, but its `extraData` does not match the configured builders | Investigate builder coverage; `extraData` is an identity hint, not authentication |
| `CONFIGURED_BUILDER` | A relay delivered this exact block and its `extraData` matches a configured builder | Inspect the per-builder stats below; this category alone does not explain exclusion |
| `TARGET_MISMATCH` | The target height has another timestamp | Skipped slot or reorg |
| `UNKNOWN` | Evidence is insufficient, including unavailable relay APIs or an absent builder list | Resolve missing evidence; do not interpret it as a builder failure or local construction |
| `NOT_YET_PRODUCED` / `RPC_UNAVAILABLE` | The block is not available yet / the RPC read failed | Retry the observation; builder stats may still be available |

Relay queries and returned records must match the canonical block **hash**, not
just its height. Public relay coverage is incomplete, and a missing delivery
never proves that MEV-Boost was not used.

Stats are collected for **every acknowledging builder**, even when another
builder won or the target timestamp changed. The supported APIs are
[Titan](https://docs.titanbuilder.xyz/bundle-tracing) at
`https://stats.titanbuilder.xyz` and
[Bombora](https://bombora.build/docs/bombora-getBundleStats/) at
`https://stats.bombora.build`. Titan traces become available about five minutes
after submission; Bombora stats become available after four blocks (about 48
seconds). Save the logs promptly and rerun after those delays. API retention is
limited; missing or expired stats cannot establish a rejection.

| Builder result | Interpretation |
| --- | --- |
| `Invalid` / `SimulationFail` | The builder reports a validation or simulation failure. Inspect its complete `error` field and the exact captured bundle. |
| `SimulationPass` | The builder reports a successful simulation. This does not establish why it omitted the bundle. |
| `IncludedInBlock` / `Submitted` | Included in a builder candidate / submitted to a relay. Neither establishes canonical inclusion. |
| `Pending` / `NotFound` / `Received` | Evidence is incomplete. `Received` has builder-specific meaning; consult that builder's API documentation. |
| `unsupported` / `missing_hash` / `unavailable` | No supported stats API, no valid hash in the acknowledgement, or a failed stats query. These are diagnostic limitations, not bundle failures. |

The exact post transaction hash from the drop log is checked in the target and
up to `--horizon` subsequent blocks (default 12; zero checks only the target).
An incomplete scan reports `pending` or `unavailable`, not a negative result.
A later transaction from the same poster, including a recomposed replacement,
does not establish inclusion of the original post. `--poster ADDRESS` optionally
lists that address's target-block transactions as **activity only**. Without a
logged post hash, inclusion remains unknown. Transaction inclusion also does not
prove successful execution or consumption of every cross-chain request; see
[settlement ownership](../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).

`--json` prints one object per target. `builder_bundle_stats` is a map keyed by
builder host, with `bundle_hash`, `availability`, and the API `result` or query
`error`. Consumers of the initial draft must handle this map and the replacement
of `LOCAL` with `POSSIBLY_LOCAL`. Exit status zero means reporting completed, not
that every builder is healthy; inspect the categories and stats. Invalid input
or a network-check failure exits with status 2; a target RPC read failure exits
with status 1. This diagnostic does not change settlement-attempt accounting.

## Stop and restart

`docker compose down` keeps every datadir. Pending requests and retry counters
are in memory: a restart reconciles canonical state, and clients may need to
resubmit unsettled work.
