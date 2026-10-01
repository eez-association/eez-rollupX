# Ethereum mainnet deployment (draft)

> **Status: draft, not executable end to end.** The contract deployment and
> genesis rendering support Ethereum mainnet (L1 chain ID 1), but the Composer
> cannot yet run against mainnet and no mainnet runtime profile is packaged.
> Resolve every [blocker](#readiness-blockers) and record a rehearsal in an
> [audit](../audits/TEMPLATE.md) before broadcasting with real value. Reading
> this runbook does not authorize deployment or funding.

| Verification record | Value |
| --- | --- |
| Source reviewed | 2026-10-01; fork `c388cf5`; protocol `855fe0602484750861b4c31502a1a91355d33ed6` |
| Checked | `deploy.sh` broadcast against a local Anvil L1 (chain 31337) with on-chain proxy, owner, registration and rendered-genesis checks; chain-1 cadence and reference code paths by source review |
| Not checked | Any mainnet or mainnet-fork deployment; Composer, signer or follower against mainnet; restart, recovery and settlement on mainnet |

This runbook covers creating a **new** registry and rollup whose L1 is Ethereum
mainnet. It reuses the [Chiado deployment](chiado-deployment.md) procedure where
the steps are chain-independent and links to it instead of repeating them.

## Readiness blockers

| Gap | Evidence | Needed before mainnet |
| --- | --- | --- |
| Composer has no mainnet L1 | `L1ChainKind` in [`l1_embedded.rs`](../../crates/eez-node/src/l1_embedded.rs) accepts `chiado`, `devnet`, `testing` and `testing-gnosis`; an unset or unknown `EEZ_L1_CHAIN` silently selects `testing` | A mainnet kind (or a validated `devnet` + `EEZ_L1_CHAIN_PATH=mainnet` profile) with a synchronized mainnet execution datadir and consensus client, rejecting unknown values |
| No runtime profile | Only [`docker-compose.chiado-node.yml`](../../docker-compose.chiado-node.yml) and [`.env.chiado.example`](../../.env.chiado.example) exist; they hardcode Chiado chain config, 5000/1000 ms timing, Lighthouse `configs/chiado`, checkpoint URL and P2P addresses | Mainnet Compose/env profile: signer `EEZ_L1_CHAIN_CONFIG=mainnet`, 12000/2000 ms timing, Lighthouse `--network mainnet`, a trusted mainnet checkpoint source, `EEZ_L1_BLOB_SIDECAR_ENCODING=eip7594` |
| Startup checks are Chiado-only | [`chiado-up.sh`](../../scripts/chiado-up.sh) requires beacon `CONFIG_NAME=chiado` and 5-second slots | A mainnet startup/health script |
| Configuration checks are Chiado/Kurtosis-only | [`check-settlement-policy-config.py`](../../scripts/check-settlement-policy-config.py) and `test_chiado_config.py` | A checked mainnet settlement and timing preset |
| Historical L1 data | No durable blob archive or L2 snapshot ([readiness list](../../PRODUCTION_READINESS_TODO.md)); mainnet beacon nodes prune blobs after the retention window | An archive or bootstrap path for followers and Composer replacement ([existing-rollup constraints](chiado-existing-rollup.md)) |
| Explorers and monitoring | Blobscan helpers fail unless L1 is 10200; dashboards link Chiado explorers ([explorers](explorers.md), [monitoring](../../monitoring/README.md)) | Mainnet explorer configuration, if used |
| Production readiness | Open P0/P1 items in [`PRODUCTION_READINESS_TODO.md`](../../PRODUCTION_READINESS_TODO.md), including security review, signer key custody and startup verification | Closed or explicitly accepted |

## Network profile fixed by the code

These values are enforced, not configurable, for L1 chain ID 1:

| Property | Value | Source |
| --- | --- | --- |
| L1 / L2 block time | 12000 / 2000 ms (six L2 positions per L1 slot) | [`deploy.sh`](../../scripts/deploy.sh) cadence defaults; [`derivation.rs`](../../crates/eez-protocol/src/l2/derivation.rs) rejects any other mainnet schedule |
| Genesis L1 reference | The `finalized` block at deployment; `EEZ_L1_REFERENCE=genesis` is rejected on mainnet | `deploy.sh` |
| Genesis timestamp and RANDAO | Copied from the reference block | `deploy.sh`; checked by `Schedule::new` |
| Blob limits | Taken from Reth's mainnet chain spec (Osaka and BPO forks); keep `EEZ_MAX_BLOBS_PER_BATCH` within the per-transaction limit | [`chain_config`](../../crates/eez-l1/src/chain_config.rs) |
| Signer L1 profile | `EEZ_L1_CHAIN_CONFIG=mainnet`; must match genesis `l1Reference.chainId` | [signer SPEC](../../crates/eez-proof-signer/SPEC.md) |

Because the reference is the finalized block, the L2 starts roughly two epochs
behind the L1 head and catches up.

## Decisions before deployment

Record each decision in the [handoff record](#handoff-record). Values that end up
in genesis or immutables cannot be changed without a new deployment.

| Decision | Default | Notes |
| --- | --- | --- |
| EEZ registry upgrade owner (`EEZ_UPGRADE_OWNER`) | Deployer | Can replace the registry, which holds every rollup's root and escrowed ETH. Use a multisig/timelock. |
| Rollup upgrade owner (`EEZ_ROLLUP_UPGRADE_OWNER`) | Deployer | Can replace the Rollup implementation. |
| EEZL2 upgrade owner (`EEZ_L2_UPGRADE_OWNER`) | Deployer | Baked into L2 genesis; upgrades with an L2 transaction and needs L2 gas. Trusted with the L2 manager's behaviour ([signer SPEC](../../crates/eez-proof-signer/SPEC.md)). |
| Rollup operational owner | Deployer (not configurable in `deploy.sh`) | Can change proof systems and verification keys, set the threshold (including 0) and override the root. Transfer it after registration if the deployer should not keep it. |
| EEZ recovery address | Deployer (not configurable in `deploy.sh`) | Immutable recipient of ETH swept from prefunded L1 proxies. |
| EEZL2 recovery address (`EEZ_L2_RECOVERY_ADDRESS`) | Poster | Immutable in genesis. |
| Proof attester (`EEZ_PROOF_SIGNER_KEY`) | — | A single ECDSA attester with threshold 1 authorizes every settlement. Generate a fresh key: public inputs do not bind chain ID or registry ([CAVEATS](../../eez-core-protocol/docs/CAVEATS.md)), so never reuse a testnet attester. |
| L2 chain ID | 696990 (base `genesis.json`) | Not a deploy input; changing it requires an edited `EEZ_GENESIS_BASE`. |
| Genesis base | `/repo/genesis.json` (no ETH allocations) | Never use `genesis.dev.json` on mainnet: it funds public development keys. To review or customize it, follow [genesis allocations](chiado-deployment.md#genesis-allocations); with a deploy image built from this checkout, copy `genesis.json` directly. |
| Settlement fees and builder | — | Choose fee ceilings, priority band and an `eth_sendBundle` builder for mainnet; Chiado and Kurtosis values are not mainnet defaults ([settlement policy](../configurable-settlement-policy.md)). A single builder limits inclusion to blocks it wins. |

## Keys, funding and files

| Key | Purpose | Funding |
| --- | --- | --- |
| `EEZ_DEPLOY_KEY` | Deployer; defaults to the poster. Use a separate key so posting cannot shift deployment nonces. | Deployment gas (below) |
| `EEZ_L1_POSTER_KEY` | Settlement sender | Ongoing settlement gas and blob fees |
| `EEZ_PROOF_SIGNER_KEY` | Attestation only; never injected into the node | None |

`deploy.sh` broadcasts six L1 transactions: EEZ implementation, EEZ proxy (which
creates its ProxyAdmin), ECDSA proof system, Rollup implementation, Rollup proxy
with initialization, and `registerRollup`. A local Anvil run on 2026-10-01 used
about **7.5M gas** in total (5.24M of it for the EEZ implementation); this is
unrecorded local evidence, so re-measure on a mainnet fork and fund the deployer
with margin at the current base fee.

Keys are raw hex in `.env`; there is no keystore or KMS integration yet. Keep
`.env` mode `600`, record only secret-manager references, and see the file table
in the [Chiado runbook](chiado-deployment.md#keys-funding-and-files) for the
files to preserve.

## Procedure

### 1. Configure

Start from `.env.chiado.example` until a mainnet example exists, and change at
least: `EEZ_L1_RPC_URL` (synchronized mainnet execution RPC, chain ID 1),
`EEZ_GENESIS_BASE` (remove it or set `/repo/genesis.json`), the three keys, the
three upgrade owners, `EEZ_L2_RECOVERY_ADDRESS` if not the poster, and the
deploy/node/signer images pinned to the **same full commit or digests**
([image selection](chiado-deployment.md#image-selection)). Remove Chiado-only
settings (checkpoint URL, Chiado config directory, builder URL, fee preset).

### 2. Rehearse on a mainnet fork

Run the same deploy image against a local fork so the chain-1 code paths, gas
use and owner wiring are exercised without spending ETH:

```bash
anvil --fork-url "$MAINNET_RPC_URL" --port 18545
```

Use a copy of `.env` with `EEZ_L1_RPC_URL=http://127.0.0.1:18545` and funded
Anvil keys, run step 3 against it, then step 4. This rehearsal is not yet
verified: confirm that the fork serves the `finalized` tag that `deploy.sh`
requires, and record the result and gas used in an audit.

### 3. Deploy — broadcasts transactions

Use the deploy command from the [Chiado runbook](chiado-deployment.md#3-deploy--broadcasts-transactions)
**without** `EEZ_DEPLOY_SKIP_SIMULATION=1`, so a failing transaction is caught
before it spends gas. Deployment is not resumable: on failure, inspect
`contracts/broadcast/` and the chain before deciding anything; do not rerun
blindly.

Expected `deploy:` header lines show the deployer, the three upgrade owners,
the authorized signer and the L2 recovery address. The run ends with
`deploy: wrote …/deployments.env`.

### 4. Verify the deployment on chain

With `deployments.env` sourced and `L1=$EEZ_L1_RPC_URL`:

```bash
impl=0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc
admin=0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103
cast storage "$EEZ_REGISTRY_ADDRESS" "$impl" --rpc-url "$L1"        # = EEZ_REGISTRY_IMPLEMENTATION
cast storage "$EEZ_REGISTRY_ADDRESS" "$admin" --rpc-url "$L1"       # = EEZ_REGISTRY_PROXY_ADMIN
cast call "$EEZ_REGISTRY_PROXY_ADMIN" 'owner()(address)' --rpc-url "$L1"   # = EEZ_UPGRADE_OWNER
cast storage "$EEZ_ROLLUP_MANAGER_ADDRESS" "$impl" --rpc-url "$L1"  # = EEZ_ROLLUP_IMPLEMENTATION
cast call "$EEZ_ROLLUP_PROXY_ADMIN" 'owner()(address)' --rpc-url "$L1"     # = EEZ_ROLLUP_UPGRADE_OWNER
cast call "$EEZ_ROLLUP_MANAGER_ADDRESS" 'owner()(address)' --rpc-url "$L1" # Rollup operational owner
cast call "$EEZ_ROLLUP_MANAGER_ADDRESS" 'EEZContract()(address)' --rpc-url "$L1"  # = EEZ_REGISTRY_ADDRESS
cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' 1 --rpc-url "$L1"
cast call "$EEZ_REGISTRY_ADDRESS" 'RECOVERY_ADDRESS()(address)' --rpc-url "$L1"
```

`rollups(1)` must return the Rollup proxy and `EEZ_INITIAL_BLOCK_HASH`. In the
rendered genesis (`EEZ_L2_GENESIS_PATH`), check `config.chainId`, that
`config.rollupX.l1Reference` is chain 1 at a finalized block, and the EEZL2 proxy
slots and ProxyAdmin owner (`EEZ_L2_UPGRADE_OWNER`). The Kurtosis
[EEZL2 verifier](../../testing/kurtosis/scripts/verify-eezl2-deployment.sh) shows
the L2-side checks once an L2 node runs. If the Rollup operational owner should
not remain the deployer, transfer it now and verify.

### 5. Start the stack — blocked

Starting the Composer, signer and followers requires the
[readiness blockers](#readiness-blockers) to be resolved. Do not adapt the Chiado
Compose file ad hoc for mainnet: its embedded L1, consensus client and startup
checks are Chiado-specific, and an unrecognized `EEZ_L1_CHAIN` falls back to a
local test chain.

## Handoff record

Keep the [Chiado handoff record](chiado-deployment.md#handoff-record) and add:
L1 chain ID 1 with the genesis reference block number and hash; every proxy,
implementation, ProxyAdmin and owner from `deployments.env`; the Rollup
operational owner and EEZ recovery address; the attester address and key custody
reference; deployment transaction hashes and gas used; and the decisions table
above with the values chosen.
