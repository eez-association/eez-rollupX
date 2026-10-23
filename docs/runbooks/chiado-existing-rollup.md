# Run a Composer for an existing Chiado rollup

Use this procedure to add or replace a Composer for a Chiado L2 created with
this revision's offline-history profile. It uses the
[packaged profile](../../docker-compose.chiado-node.yml):
embedded L1, Composer/L2, local proof signer, and Lighthouse. For an unchanged
installation, use [restart](chiado-deployment.md#restart-and-stop).

The original deployment must have the matching `OfflineRollup`, history
predeploy, authenticated bootstrap and delayed origin/PREVRANDAO schedule from
genesis. Native `0x76` transactions alone do not establish compatibility.
Earlier networks remain on their original runtime, configuration and databases;
this procedure does not convert them. See the
[compatibility boundary](chiado-deployment.md#offline-v2-proving-and-fresh-network-boundary).

**Do not run deployment or regenerate genesis.** This profile derives settled L2
history from L1 transactions and Beacon blobs; an existing L2 RPC URL alone is not
a bootstrap source. Starting the node automatically enables sequencing and posting
after catch-up; there is no separate activation step in this procedure.

Verification: source reviewed at the [deployment runbook's baseline](chiado-deployment.md),
including [startup](../../crates/eez-node/src/lib.rs),
[reconciliation](../../crates/eez-deriver/src/deriver.rs), and
[blob retrieval](../../crates/eez-l1/src/blob_source.rs). Shell syntax and links checked;
fresh replay and backup restoration have **not** been rehearsed on a live rollup.

The [September 28 cold-copy trial](../audits/2026-09-28-chiado-strict-fifo/README.md)
predates this profile and does not validate its live restore procedure. Retain
original databases and configuration when testing a runtime replacement; never
roll an older binary back onto candidate-modified state. Pruned-genesis RPC
handling also requires the
[compatible monitoring identity check](../../monitoring/indexer/README.md#start-on-the-same-machine).

## 1. Obtain the deployment and choose the state source

Run commands in Bash from the deployment checkout root; stop on failure. Meet the shared
[host, tools, ports, and network prerequisites](chiado-deployment.md#shell-host-and-network-prerequisites).
Use independent writable databases and an available set of the profile's fixed
ports/container names. A second worktree on the same host does not provide these.

Obtain these inputs from the existing network's operator:

| Input | Required contents or constraint |
| --- | --- |
| Deployment files | Original `deployments.env`, `datadir/genesis.json`, and `datadir/genesis.profile.json`. Keep registry/deployment block, rollup ID, proof-system/vkey/attester, offline manager/bridge addresses, system address, and initial block/state commitments. Preserve the complete history bootstrap, schedule and local L1 rules bound by that manager. |
| Runtime configuration | Matching offline-profile node/signer images, configuration checkout/protocol revision, and settlement/admission settings. The packaged timing is 5-second L1 / 1-second L2, with EEZL2 at proxy `0x4B2760F84B83d6b3829E30A7573B0172B6d6402C` and history at `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077`. The genesis fixes the history delay; startup cannot override it. |
| System and attester bindings | The deployed system address must be `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076`; no system private key exists. Supply `EEZ_PROOF_SIGNER_KEY` for the registered attester privately. A fresh attestation key cannot replace that binding; this Compose profile runs a local signer. |
| Posting wallet | A dedicated `EEZ_L1_POSTER_KEY` funded with Chiado test xDAI. Do not share it with another active Composer or workload. No deployer key is needed. |
| State/history source | Choose one L2 path below; obtain compatible L1 data and Beacon coverage for its replay range. Preserve the backup's source revision and last settled L1/L2 hashes. |

| L2 starting state | Preparation and replay requirement |
| --- | --- |
| Complete backup | Restore a consistent copy of the entire `L2_DATA_DIR` to `data/eez-l2`, including `eez-correlations`, witnesses, and chain data. Use a stopped writer or a database-consistent snapshot. The deriver validates the correlation tail against canonical L1 and local L2, then scans **from that L1 block inclusively**. |
| No L2 backup | Leave `data/eez-l2` absent; startup creates it and replays from `EEZ_REGISTRY_DEPLOY_BLOCK`. Every settled batch and its blobs must remain available. |

A missing or unusable correlation index can force scanning from the deployment
block even when L2 blocks exist. Copying only `eez-reconcile-checkpoint` does not
restore the database or complete index. Do not erase an existing database to select
the fresh path; use separate storage and retain the recovery copy.

For either path, the embedded L1 must serve every produced canonical header from
the authenticated predecessor's history endpoint through each required anchor,
including intermediate headers that no L2 block selected as its origin. It must
also serve logs and transaction bodies throughout the selected scan range.
This chain access belongs to the node and follower; the signer receives complete
evidence over v2 and uses only local profiles. With a valid checkpoint,
deployment-era bodies/logs need
not remain available; follow [pruned-L1 recovery](ethereum-node.md#recovery-with-a-pruned-l1)
for raw-header RPC support and recovery boundaries. The Beacon source
must serve every batch's full blobs over that range, including the restored anchor
block. A recent execution snapshot or Lighthouse checkpoint sync does not supply
historical blobs. If retention no longer covers the range, obtain an archival
Beacon source or a newer complete L2 backup before proceeding; this repository
does not provide a rollup snapshot download or a guaranteed blob archive.

## 2. Install files and configure this Composer

On a new checkout, create `.env` **only if it does not already exist**:

```bash
cp .env.chiado.example .env
chmod 600 .env
```

Place the operator-supplied bindings at `./deployments.env` and original genesis
and profile at `./datadir/genesis.json` and `./datadir/genesis.profile.json`.
Preserve the genesis/profile contents and deployment identity. In `deployments.env`,
reconcile only the host paths
`EEZ_L2_GENESIS_PATH` and `EEZ_L2_GENESIS_PROFILE_PATH` with their new absolute
locations. `FRESH_GENESIS=./datadir/genesis.json` must select that original genesis,
despite the variable's name.

Edit `.env` with the supplied attestation key, this Composer's poster key,
a synchronized Chiado `EEZ_L1_RPC_URL`, and the existing settlement settings.
Use compatible [image references](chiado-deployment.md#image-selection) and the
matching checkout: the default `:latest` may select an upgrade. Preserve existing
operator settings instead of replacing them with template defaults.

If local Lighthouse cannot serve the replay range, set `EEZ_L1_BEACON_URL` to the
archival Chiado Beacon API. It must serve current head, genesis/configuration, and
`/eth/v1/beacon/blobs/{slot}` for historical batches; startup also probes
`/eth/v1/beacon/blobs/head`. Local Lighthouse still runs to drive the embedded L1.
Changing `.env`'s `EEZ_L1_RPC_URL` does **not** replace that embedded execution
source: Compose fixes the node's L1 reads to `http://127.0.0.1:18645`.

For a restored installation, preserve its L1/Lighthouse databases and JWT. For a
new L1 database, populate `data/chiado-l1` using an operator-provided consistent
backup, or download the Chiado snapshot into that **unused** directory:

```bash
mkdir -p data
docker run --rm -v "$PWD/data/chiado-l1:/data" \
  ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
  download --chain chiado --minimal --datadir /data
```

The downloaded snapshot is a starting point, not proof of replay coverage; the
embedded L1 must sync/backfill the required history. For a new installation with
no `data/jwt.hex`, create the secret shared by its execution and consensus clients:

```bash
openssl rand -hex 32 > data/jwt.hex
```

Restore the selected matching-profile L2 backup now, if applicable. Complete the
[deployment identity checks](chiado-deployment.md#deployment-identity): genesis
hash/state root, history profile, local L1 rules and system/attester addresses must
match the supplied bindings, offline manager and deployed proof system. Keep
genesis readable by the signer (UID 65532). Do not configure signer chain RPCs;
obsolete inputs are rejected even when empty.

## 3. Start, catch up, and verify

This starts services and can spend the poster's funds once catch-up completes.
The profile accepts settlements from other Composers. For a replacement, coordinate
the handover and stop using the old posting wallet before reusing it; for an
additional Composer, keep a separate poster and databases.

```bash
bash scripts/chiado-up.sh
```

Follow progress in another shell at the same checkout:

```bash
docker compose logs -f --tail=100 eez-node eez-proof-signer lighthouse
```

Verify these stages:

1. Embedded L1 and Beacon catch up; use the shared [health checks](chiado-deployment.md#health-checks).
   A responding L2 RPC alone does not establish readiness.
2. Node logs reach `spawning eez composer umbrella` after boot reconciliation and
   startup validation. The node has now enabled sequencing/posting; cross-chain
   ingress remains closed before catch-up completes.
3. Run the [settlement diagnostic](chiado-deployment.md#settlement-diagnostic).
   The registry's current commitment must resolve locally with `canonicalL2=true`
   and an `l1TransactionHash`. This checks agreement with L1 settlement, not merely
   a growing speculative L2 head; finality may lag.
4. Startup reports `chiado L2 is UP` after commitment advancement and retrieval of
   new settlement blobs. With multiple Composers, this can reflect another poster's
   work. To confirm **this Composer posts**, use a post-start transaction hash from
   the settlement diagnostic and compare its sender with this poster:

```bash
source .env
cast wallet address --private-key "$EEZ_L1_POSTER_KEY"
read -r -p 'Settled L1 transaction hash: ' settlement_tx
cast tx "$settlement_tx" --rpc-url http://127.0.0.1:18645 --json | jq '{hash, from}'
```

The addresses must match (case-insensitive). If another Composer won that batch,
check this node's later submissions; adopting someone else's settlement verifies
catch-up, not this signer's/poster's ability to settle.

A startup timeout leaves containers running. Initial replay or an enabled policy
with no due work can exceed the health-check window; inspect progress before
rerunning startup. Do not redeploy or reset state to obtain a success banner.

Pending requests and retry counters are not in the backup. Reconcile receipts and
sender nonces before resubmitting unsettled work. Boot reconciles speculative
history; policy-enabled startup can retain a validated pure-L2 prefix, but rolls
back unowned cross-chain Sync blocks and their descendants.

Once caught up, you can add [optional explorers](chiado-deployment.md#optional-explorers)
using this rollup's original genesis and desired L1 history boundary. Their
separate Compose project is operated outside CI and does not bootstrap the Composer.

## If catch-up stalls

| Symptom | First action |
| --- | --- |
| `L1 preflight never became ready` | Check the configured chain ID, head height and genesis-reference finality. History is checked afterwards at the [validated recovery anchor](ethereum-node.md#recovery-with-a-pruned-l1). |
| `L1 never became able to serve block` or `boot_catch_up.source_incomplete` naming an execution block/transaction | Check embedded L1 sync and the named history, using `http://127.0.0.1:18645`. A higher head does not prove old bodies/logs exist. Recover/backfill the L1 database; changing the external `.env` RPC alone will not repair it. |
| Missing blobs or an HTTP error naming a historical slot | Use the [blob diagnostic](chiado-deployment.md#blob-diagnostic--read-only) for that settlement. Obtain full-blob coverage at `EEZ_L1_BEACON_URL`; waiting at the current head cannot restore expired data. |
| `correlation.tail_rejected`, or replay starts at deployment despite a backup | Check that genesis, chain data, and `eez-correlations` came from the same consistent backup. A reorg can also invalidate the tail and extend the required replay range. Preserve files before recovery. |
| Genesis/key mismatch, replay validation failure, or signer rejection | Recheck [deployment identity](chiado-deployment.md#deployment-identity) and runtime/protocol compatibility. Preserve the exact error; do not replace keys/genesis or bypass validation. |
| Catch-up succeeds but this poster never settles | Check signer/node logs, poster funds/nonce, and [settlement diagnostics](chiado-deployment.md#settlement-diagnostic); account for competition and the configured policy. |

Use [restart/stop](chiado-deployment.md#restart-and-stop) after setup and retain the
[handoff inventory](chiado-deployment.md#handoff-record), including backup and
historical-data coverage. Record live bootstrap/recovery results separately from
the source review above.
