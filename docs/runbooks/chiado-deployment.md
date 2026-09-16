# Chiado deployment and operation

Use [`docker-compose.chiado-node.yml`](../../docker-compose.chiado-node.yml):
`eez-node` runs the Composer, embedded Chiado L1 execution client, and L2;
`eez-proof-signer` independently validates batches; Lighthouse drives L1 consensus
and reconstructs blobs. `docker-compose.chiado.yml` is a different, Beacon-only
profile. Entry points: [deploy](../../scripts/deploy.sh),
[start](../../scripts/chiado-up.sh), [stop](../../scripts/teardown-chiado.sh).

| Verification record | Value |
| --- | --- |
| Runtime source reviewed | 2026-09-16; fork `ddf40bc2eb5fac8f7973b112eb383256cd8aac74`; protocol `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Checked | Source/configuration consistency, relative links, Bash syntax, stubbed startup/health/blob examples, `scripts/check-chiado-blob-config.sh` |
| Pending | Full fresh-host deployment and restart rehearsal; record execution in a dated [audit](../audits/TEMPLATE.md) |

## Select the procedure

| Operation | Procedure | Required starting state |
| --- | --- | --- |
| Create a new registry and rollup | [First deployment](#first-deployment), then [start and verify](#start-and-verify) | Unused deployment paths; no existing stack on these names/ports |
| Start/restart the same rollup | [Restart and stop](#restart-and-stop), then [start and verify](#start-and-verify) | Original bindings, genesis, keys, JWT, images, and datadirs |
| Move hosts, upgrade, or recover missing state | [Recovery and upgrades](#recovery-and-upgrades) | Deployment inventory, preserved state, and an identified recovery source |

**Deployment is not idempotent.** `make deploy-protocol` broadcasts a new deployment
even if `deployments.env` exists. If that file is missing, `chiado-up.sh` can deploy
and **delete `L2_DATA_DIR` and `LIGHTHOUSE_DATA`**. Do not use `make clean-deploy`,
`make clean-l2`, or removal of bindings as a restart/recovery procedure.

## Shell, host, and network prerequisites

Run commands in Bash from the intended deployment checkout's root. Use a checkout
path without whitespace or commas: generated bindings contain unquoted paths and
Docker's mount syntax uses commas. Use the default paths and ports below. Custom
layouts require adapting Compose, the startup wrapper (which hardcodes the L1/L2
health-check ports), and the commands together.
A worktree or `COMPOSE_PROJECT_NAME` does not isolate fixed container names,
host-network ports, or bind-mounted databases.

Use a fresh shell without inherited `EEZ_*`, `LH_*`, `COMPOSE_*`, or data-path
overrides. Define this helper in **each operator shell**; it gives direct Compose
commands the same `.env` then `.env.chiado` precedence as startup:

```bash
chiado_compose() (
  set -euo pipefail
  set -a
  source .env
  source .env.chiado
  set +a
  docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml "$@"
)
```

Stop at a failed command/check; do not paste all alternative procedures into one
shell. Deploying, funding traffic, stopping services, and recovery must be within
the task's authorization.

| Requirement | Check/constraint |
| --- | --- |
| Linux, local Docker Engine, Compose, BuildKit | `docker info`; `docker compose version`. Bind mounts refer to this host; CI runtime artifacts target Linux amd64. |
| Host tools | Bash, Git, Make, `curl`, `jq`, Python 3, OpenSSL, GNU utilities (`realpath`, `sha256sum`, `cmp`), `ss` (iproute2) |
| Foundry | `forge` and `cast` from **1.7.1**; genesis generation enforces the `forge` version |
| Build/storage capacity | Space and memory for Rust image builds, L1 snapshot, PeerDAS data, and growing databases. Check `df -h .` and Docker storage; no measured minimum is established here. |
| Network access | Dependencies/images, snapshot download, checkpoint source, deployment RPC, builder, and P2P peers |
| Deployment RPC | Synchronized Chiado execution RPC, chain ID **10200**; remains required by startup even when the embedded L1 is running |
| Builder | Separate endpoint implementing `eth_sendBundle`; an ordinary execution RPC is insufficient |
| Beacon | Packaged Lighthouse **v8.1.3**, checked-in `configs/chiado`, `--semi-supernode`, full-blob API; built-in Chiado config in this image leaves Fulu disabled |

### Ports and service ownership

Run `ss -lntup` and `docker ps -a --format '{{.Names}}'` before first deployment.
The fixed containers are `eez-node-chiado`, `eez-proof-signer-chiado`, and
`eez-chiado-lighthouse`. Resolve existing owners before proceeding.

| Default port | Service/access |
| --- | --- |
| 18645 / 18646 | Embedded Chiado L1 HTTP / WebSocket RPC |
| 18688 / 18689 | L2 HTTP / WebSocket RPC |
| 18999 / 18998 | L1-to-L2 / L2-to-L1 transaction fronts; ordinary RPC calls proxy to the source chain |
| 18551 / 18684 | L1 / L2 engine APIs; shared JWT protects the L1 engine connection |
| 50061 / 6152 | Proof signer gRPC / Beacon HTTP; loopback |
| 30544 TCP/UDP, 30554 UDP | Embedded L1 P2P/discv4 and discv5 (`EEZ_L1_P2P_PORT + 10`) |
| 9205 TCP/UDP, 9206 UDP | Lighthouse P2P/discovery and QUIC |
| 30640 / 30641 | L2 P2P/discovery settings reserved by the profile; discovery disabled |

L2 HTTP, both fronts, and the L1 engine API bind all host interfaces. Restrict
unintended access with the host firewall; host networking provides no Docker port
isolation. Permit required P2P traffic and configure NAT where needed. Public
HTTPS, explorers, and UI routing are separate; see the
[gateway's Chiado notes](../../testing/public-gateway/README.md#chiado-deployment).

### Keys, funding, and files

| Key | Purpose | Funding |
| --- | --- | --- |
| `EEZ_L1_POSTER_KEY` | Settlement sender | Chiado native gas funds (test xDAI) |
| `EEZ_DEPLOY_KEY` (optional) | Contract deployer/owner; defaults to poster | Chiado gas funds if separate |
| `EEZ_PROOF_SIGNER_KEY` | Batch attestation; registered in proof system | No transaction funding needed for attestation |
| `EEZ_L2_SYSTEM_KEY` | L2 system transactions | Address funded in generated L2 genesis |

Generate dedicated keys with your wallet tooling. Proof signer and L2 system
addresses **must differ**; keep the poster separate from the proof signer.
A separate deploy key avoids nonce competition with an active poster. Do not
share a posting wallet with concurrent workloads. Fund deployer and poster before
broadcasting; the [environment example](../../.env.chiado.example) identifies a
Chiado faucet. The preflight below prints addresses/balances; choose a gas budget
for the intended deployment and posting duration rather than assuming a fixed cost.

The base [`genesis.json`](../../genesis.json) contains funded development accounts;
some workloads use public development keys. Review allocations **before** creating
the rollup. Changing genesis after registration changes its identity.

| File/variable | Default location and ownership |
| --- | --- |
| `.env` | Private deployment RPC/keys; read by `make deploy-protocol` |
| `.env.chiado` | Private service keys and Compose settings; overrides `.env` during startup |
| `deployments.env` | Generated public contract/rollup bindings and genesis/profile paths; preserve |
| `FRESH_GENESIS` | `./datadir/genesis.json`; generated at deployment, mounted into node and signer |
| Genesis profile | `./datadir/genesis.profile.json`; generated code/state commitments; preserve |
| `L1_SNAPSHOT_DIR` | `./data/chiado-l1`; writable embedded L1 database |
| `L2_DATA_DIR` | `./data/eez-l2`; writable L2 database, witnesses, and correlations |
| `LIGHTHOUSE_DATA` | `./data/chiado-lighthouse`; writable consensus/PeerDAS data |
| `JWT_FILE` | `./data/jwt.hex`; private, shared by node and Lighthouse; create once |
| `CHIADO_CONFIG_DIR` | `./configs/chiado`; versioned network config, including `bootstrap_nodes.yaml` alias |

The bare-binary `EEZ_L2_DATADIR` example does not replace Docker's `L2_DATA_DIR`.
Keep `EEZ_ENV_FILE`, `EEZ_DEPLOYMENTS_FILE`, `EEZ_GENESIS_OUT`,
`EEZ_GENESIS_PROFILE_OUT`, and `EEZ_CHIADO_ENV_FILE` unset for this procedure;
the scripts do not all consume those overrides. Generated genesis paths in
`deployments.env` are absolute: moving a checkout requires path reconciliation.

## First deployment

Complete steps 1–4 in order. Keep the stack stopped until **Start and verify**.
After a partial failure, inspect retained outputs and resume only the uncompleted
setup step; do not repeat contract deployment without reconciling broadcasts.

### 1. Check the checkout and build

Inspect local changes before initializing the pinned submodule. Do not substitute
upstream `main`. The guard below refuses an existing deployment/configuration:

```bash
(
  set -euo pipefail
  for path in .env .env.chiado deployments.env datadir/genesis.json \
      data/chiado-l1 data/eez-l2 data/chiado-lighthouse data/jwt.hex; do
    [[ ! -e "$path" ]] || { echo "Existing state: $path; identify the deployment first"; exit 1; }
  done
  git status --short
  git rev-parse HEAD
  git ls-tree HEAD eez-core-protocol
  git submodule update --init --recursive eez-core-protocol
  git submodule status --recursive
  docker info --format '{{.ServerVersion}}'
  docker compose version
  forge --version
  cast --version
  docker build -t eez-node:local .
  docker build -f Dockerfile.signer -t eez-proof-signer:local .
  docker image inspect --format '{{.RepoTags}} {{.Id}}' eez-node:local eez-proof-signer:local
  bash scripts/check-chiado-blob-config.sh
)
```

**Pass:** submodule matches the gitlink, both builds succeed, static check prints
`Chiado post-Fulu blob deployment configuration: PASS`. Record image IDs;
`:local` tags are mutable. This check uses examples, not the live configuration.

#### Host genesis tools

Deployment runs on the host and needs `eez-genesis-state-root` and
`eez-genesis-block-hash`. Supply them from the built node image to avoid a host
Cargo build after contracts have already been broadcast. These wrappers mount
only the input file, with networking and image pulling disabled:

```bash
export PATH="$PWD/.runtime/chiado-tools:$PATH"
(
  set -euo pipefail
  mkdir -p .runtime/chiado-tools
  cat > .runtime/chiado-tools/genesis-tool <<'SH'
#!/usr/bin/env bash
set -euo pipefail
genesis_path="$(realpath -- "${1:?pass the genesis JSON path}")"
exec docker run --rm --pull never --network none \
  --mount "type=bind,source=$genesis_path,target=/genesis.json,readonly" \
  --entrypoint "/usr/local/bin/$(basename "$0")" \
  eez-node:local /genesis.json
SH
  chmod 755 .runtime/chiado-tools/genesis-tool
  ln -sfn genesis-tool .runtime/chiado-tools/eez-genesis-state-root
  ln -sfn genesis-tool .runtime/chiado-tools/eez-genesis-block-hash
  eez-genesis-state-root "$PWD/genesis.json"
  eez-genesis-block-hash "$PWD/genesis.json"
  forge build --root contracts
)
```

**Pass:** two 32-byte hex hashes and a successful contract build. The repository
fixture here tests tools; it is not the deployment's genesis. Re-export `PATH`
in a new shell. On restart, reuse the recorded node image; if wrappers are missing,
recreate only this wrapper block, without rebuilding/retagging the images.

### 2. Configure and preflight

Create private files, then edit them:

```bash
(
  set -euo pipefail
  [[ ! -e .env && ! -e .env.chiado && ! -e deployments.env ]]
  umask 077
  cp .env.example .env
  cp .env.chiado.example .env.chiado
)
```

| File | Required edits |
| --- | --- |
| `.env` | Replace the example `EEZ_L1_RPC_URL` with your synchronized Chiado RPC; set the three required keys; optionally set `EEZ_DEPLOY_KEY`. |
| `.env.chiado` | Set the **same three keys**, verify `EEZ_L1_BUILDER_RPC_URL`, and retain the default paths above, `EEZ_L1_BEACON_URL=http://127.0.0.1:6152/`, and `EEZ_L1_BLOB_SIDECAR_ENCODING=eip7594`. |

Checkpoint sync defaults to `https://checkpoint.chiadochain.net/`; set
`LH_CHECKPOINT_URL` in `.env.chiado` to use another trusted checkpoint source.

Keep policy disabled for initial bring-up unless another policy is part of the
operation. For opt-in policy, use the [configuration guide](../configurable-settlement-policy.md):
remove `EEZ_MAX_BLOCKS_PER_BATCH`, set required fee ceilings and scheduling rules,
and distinguish omitted values from empty strings. Compose explicitly supplies
the running node's embedded L1 RPC, so do not replace the external deployment
RPC with an unsynchronized local endpoint. Do not publish expanded Compose
configuration or environment dumps; they contain keys.

Before spending gas, verify RPC identity, key consistency, funding, builder method
availability, and genesis rendering. Output labels public addresses and balances
in wei; the empty builder probe submits no transactions:

```bash
(
  set -euo pipefail
  source .env
  [[ "$(cast chain-id --rpc-url "$EEZ_L1_RPC_URL")" == 10200 ]]
  cast block latest --rpc-url "$EEZ_L1_RPC_URL" --json | jq '{number, timestamp, hash}'
  poster="$(cast wallet address --private-key "$EEZ_L1_POSTER_KEY")"
  deployer="$(cast wallet address --private-key "${EEZ_DEPLOY_KEY:-$EEZ_L1_POSTER_KEY}")"
  attester="$(cast wallet address --private-key "$EEZ_PROOF_SIGNER_KEY")"
  system="$(cast wallet address --private-key "$EEZ_L2_SYSTEM_KEY")"
  [[ "${attester,,}" != "${system,,}" && "${attester,,}" != "${poster,,}" ]]
  printf 'deployer=%s balance(wei)=' "$deployer"
  cast balance "$deployer" --rpc-url "$EEZ_L1_RPC_URL"
  printf 'poster=%s balance(wei)=' "$poster"
  cast balance "$poster" --rpc-url "$EEZ_L1_RPC_URL"
  source .env.chiado
  [[ "$poster" == "$(cast wallet address --private-key "$EEZ_L1_POSTER_KEY")" ]]
  [[ "$attester" == "$(cast wallet address --private-key "$EEZ_PROOF_SIGNER_KEY")" ]]
  [[ "$system" == "$(cast wallet address --private-key "$EEZ_L2_SYSTEM_KEY")" ]]
  [[ "$EEZ_L1_BLOB_SIDECAR_ENCODING" == eip7594 ]]
  builder_probe="$(curl -fsS --max-time 15 -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","method":"eth_sendBundle","params":[{"txs":[],"blockNumber":"0x0"}],"id":1}' \
    "$EEZ_L1_BUILDER_RPC_URL")"
  jq -e 'has("result") or (has("error") and .error.code != -32601)' >/dev/null <<<"$builder_probe"
  bash scripts/update-eezl2-genesis.sh --render \
    --rollup-id 1 --system-address "$system" --base "$PWD/genesis.json" \
    --output "$PWD/.runtime/chiado-tools/preflight-genesis.json" \
    --profile-output "$PWD/.runtime/chiado-tools/preflight-profile.json"
)
```

**Pass:** chain ID 10200, recent advancing head, sufficient deployer/poster funds,
matching keys, builder responding without method-not-found, successful render.
Compare the RPC head with a trusted tip source;
chain ID alone does not establish freshness. The `.runtime/` genesis is disposable;
never use it as `FRESH_GENESIS`.

### 3. Download the L1 snapshot and create JWT

This populates unused host paths; it must not run over an existing L1 database:

```bash
(
  set -euo pipefail
  [[ ! -e data/chiado-l1 && ! -e data/jwt.hex ]]
  mkdir -p data
  docker run --rm -v "$PWD/data/chiado-l1:/data" \
    ghcr.io/gnosischain/reth_gnosis:v2.0.0 \
    download --chain chiado --minimal --datadir /data
  (umask 077; openssl rand -hex 32 > data/jwt.hex)
  test -d data/chiado-l1
  test "$(wc -c < data/jwt.hex)" -eq 65
  test -s configs/chiado/bootstrap_nodes.yaml
  cmp configs/chiado/bootstrap_nodes.yaml configs/chiado/bootnodes.yaml
)
```

**Pass:** download exits successfully, JWT contains 64 hex characters plus newline,
and bootstrap alias matches. Both services mount the same JWT. Preserve the
checked-in network config with the pinned Lighthouse image; validate config/image
upgrades separately. Do not regenerate JWT or redownload the snapshot on restart.

### 4. Deploy and verify registration — broadcasts transactions

This spends deployer gas and creates a fresh registry, proof system, manager,
rollup ID **1**, and L1 bridge contracts. It generates genesis with the configured
L2 system address and a timestamp from the recorded L1 deploy block, then registers
its **block hash**, not just its state root.

```bash
(
  set -euo pipefail
  [[ ! -e deployments.env ]] || { echo 'Existing deployment; use restart'; exit 1; }
  umask 077
  mkdir -p .runtime/chiado-deployment
  EEZ_DEPLOY_SKIP_SIMULATION=1 make deploy-protocol \
    2>&1 | tee ".runtime/chiado-deployment/deploy-$(date -u +%Y%m%dT%H%M%SZ).log"
  source .env
  source deployments.env
  registered="$(cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' \
    "$EEZ_ROLLUP_ID" --rpc-url "$EEZ_L1_RPC_URL" | sed -n '2p' | tr -d '[:space:]')"
  [[ "$registered" == "$EEZ_INITIAL_BLOCK_HASH" ]]
)
```

`EEZ_DEPLOY_SKIP_SIMULATION=1` is the RPC block-availability workaround: it skips
Foundry simulation, **not broadcasting**. Omit it when simulation works reliably.
The initial-commitment equality is valid only before the first settlement.

**Pass:** completed `deployments.env`, `datadir/genesis.json`,
`datadir/genesis.profile.json`, `contracts/broadcast/` receipts, and matching
registered initial block hash. Optional `EEZ_BLOCKSCOUT_URL` verification is best
effort; its failure does not undo deployment. Retain raw logs privately because
an authenticated RPC URL may appear in them.

**Failure:** contracts can exist even when no complete bindings file was written.
The script is not resumable. Preserve receipts/logs and inspect confirmed
transactions, contract addresses, and deployer nonce before planning continuation.
Do not blindly rerun. On success, continue below.

## Start and verify

Required state: deployment bindings and genesis/profile exist; L1 snapshot, JWT,
and network config are prepared; images match the inventory. For an existing
rollup, first complete the preservation checks in [Restart and stop](#restart-and-stop).
This step starts/recreates the three services and can begin spending poster funds.

Check keys and genesis against deployment identity before starting. The signer
runs as UID 65532 and needs read access to the public genesis; only that file is
made world-readable. The on-chain proof-system signer must match the configured
attester, and the system key must match the genesis binding:

```bash
export PATH="$PWD/.runtime/chiado-tools:$PATH"
(
  set -euo pipefail
  test -s .env
  test -s .env.chiado
  test -s deployments.env
  source .env
  source .env.chiado
  source deployments.env
  test -s "$EEZ_L2_GENESIS_PROFILE_PATH"
  cmp "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PATH"
  [[ "$(eez-genesis-block-hash "$FRESH_GENESIS")" == "$EEZ_INITIAL_BLOCK_HASH" ]]
  [[ "$(eez-genesis-state-root "$FRESH_GENESIS")" == "$EEZ_L2_GENESIS_STATE_ROOT" ]]
  attester="$(cast wallet address --private-key "$EEZ_PROOF_SIGNER_KEY")"
  system="$(cast wallet address --private-key "$EEZ_L2_SYSTEM_KEY")"
  onchain="$(cast call "$EEZ_ECDSA_PROOF_SYSTEM_ADDRESS" 'signer()(address)' --rpc-url "$EEZ_L1_RPC_URL")"
  [[ "${attester,,}" == "${EEZ_ATTESTER_ADDRESS,,}" && "${attester,,}" == "${onchain,,}" ]]
  [[ "${system,,}" == "${EEZ_L2_SYSTEM_ADDRESS,,}" ]]
  chmod 644 "$FRESH_GENESIS"
  sha256sum "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PROFILE_PATH"
  chiado_compose config --quiet
  bash scripts/chiado-up.sh
)
```

`chiado-up.sh` requires `FRESH_GENESIS` before its optional deploy branch; first
registration must therefore be explicit. With the bindings file present it skips
deployment/data deletion and creates missing initial L2/Lighthouse directories.

**Startup pass:** `chiado L2 is UP`, L2 height greater than 3, changed registry
commitment, and a new `BatchPosted` transaction of type 3 whose versioned-hash-selected
full blobs are retrievable. Retain its transaction hash. The empty-bundle builder
probe establishes method availability only; it does not prove inclusion.

A timeout does **not** stop containers. Inspect [health](#health-checks) and logs;
do not redeploy. Startup allows 90 Beacon polls (5-second sleeps plus HTTP timeouts)
and 90 settlement polls (10-second sleeps plus RPC time). Enabled policy with no
due work or a long interval can exceed the settlement wait while services remain
operational; inspect policy rather than weakening it to obtain the success banner.

### Health checks

Read-only diagnostics; no funding or workload generation:

```bash
chiado_compose ps
chiado_compose logs --tail=100 eez-node eez-proof-signer lighthouse
cast rpc eth_syncing --rpc-url http://127.0.0.1:18645
curl -fsS http://127.0.0.1:6152/eth/v1/node/syncing | jq '.data'
curl -fsS http://127.0.0.1:6152/eth/v1/node/peer_count | jq '.data'
```

**Required:** three running services, signer healthy; execution sync complete;
Beacon `is_syncing=false`, `is_optimistic=false`, `el_offline=false`, and connected
peers. Signer health is TCP readiness, not proof of a successful attestation.

Verify both chain IDs, local genesis identity, L1 agreement at a freshly sampled
external tip, and advancing L2 height:

```bash
(
  set -euo pipefail
  source .env
  source .env.chiado
  source deployments.env
  l1=http://127.0.0.1:18645
  l2=http://127.0.0.1:18688
  [[ "$(cast chain-id --rpc-url "$EEZ_L1_RPC_URL")" == 10200 ]]
  [[ "$(cast chain-id --rpc-url "$l1")" == 10200 ]]
  [[ "$(cast chain-id --rpc-url "$l2")" == "$(jq -r '.config.chainId' "$FRESH_GENESIS")" ]]
  [[ "$(cast block 0 --rpc-url "$l2" --json | jq -r '.hash')" == "$EEZ_INITIAL_BLOCK_HASH" ]]
  tip="$(cast block-number --rpc-url "$EEZ_L1_RPC_URL")"
  reference="$(cast block "$tip" --rpc-url "$EEZ_L1_RPC_URL" --json | jq -r '.hash')"
  [[ "$(cast block "$tip" --rpc-url "$l1" --json | jq -r '.hash')" == "$reference" ]]
  before="$(cast block-number --rpc-url "$l2")"
  sleep 10
  after="$(cast block-number --rpc-url "$l2")"
  (( after > before ))
  commitment="$(cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' \
    "$EEZ_ROLLUP_ID" --rpc-url "$l1" | sed -n '2p' | tr -d '[:space:]')"
  cast rpc eez_getSettlementByL2Block "$commitment" --rpc-url "$l2" \
    | jq -e 'select(.canonicalL2 == true) | {l1TransactionHash, l2Range, canonicalL2, l2Finalized}'
)
```

Retry the tip comparison if L1 is still catching up or the sampled block was
reorganized. Repeat over several Chiado slots (5 seconds). After settlement,
correlation must report the committed L2 hash as canonical and identify its L1
transaction/range. `l2Finalized` can remain false until finality advances. A `null`
lookup or a latest speculative L2 block does not prove settlement; see the
[correlation RPC contract](../../README.md#l1l2-settlement-correlation-rpc).

### Optional workloads

Run funded traffic only when the operation includes it. Inspect the
[matrix driver](../../scripts/xchain-test.sh) before use: its funder must hold funds
on **both chains**, and it assumes public development accounts. The
[staged performance runner](../../scripts/run-chiado-staged-performance.sh) locks
shared wallets; `--check` is readiness-only and normal runs use `--no-verify`.
Do not run another upstream workload concurrently. See [workload commands](../../README.md#exercise-it).
Startup/throughput/receipts do not validate all cross-chain semantics or independent
follower replay; record those checks separately when required.

## Restart and stop

Use the recorded checkout, image IDs, keys, bindings, genesis/profile, JWT, and data
paths. Rebuilding a mutable `:local` tag changes the next start into an upgrade.
Do not rerun first-deployment steps for an ordinary restart.

Before stopping, pause submissions and record unsettled transaction hashes,
canonical receipts, and sender nonces. Pending requests/retry counters are in memory;
restart discards the speculative suffix and reconstructs canonical state from L1.
After startup, reconcile receipts/nonces before resubmitting unsettled work.
A datadir backup does not retain volatile request ownership.

**Stop changes running services; preserves bind-mounted data:**

```bash
bash scripts/teardown-chiado.sh
```

**Pass:** the script confirms all three containers stopped. The signer's grace
period is up to 11 minutes to drain admitted requests. If teardown fails, inspect
remaining containers before proceeding; preserve files and data.

Before starting an existing rollup, require its retained data instead of letting
startup silently create replacement directories:

```bash
(
  set -euo pipefail
  test -s .env
  test -s .env.chiado
  test -s deployments.env
  source .env
  source .env.chiado
  source deployments.env
  cmp "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PATH"
  test -s "$EEZ_L2_GENESIS_PROFILE_PATH"
  test -s "$JWT_FILE"
  test -d "$L1_SNAPSHOT_DIR"
  test -d "$L2_DATA_DIR"
  test -d "$LIGHTHOUSE_DATA"
  docker image inspect --format '{{.RepoTags}} {{.Id}}' eez-node:local eez-proof-signer:local
)
```

Compare image IDs and file paths/digests with the inventory, then run
[Start and verify](#start-and-verify). Missing/empty databases require recovery
assessment; directory existence alone is not a backup-integrity check.

Configuration changes require `chiado_compose up -d` (also used by startup) to
recreate affected containers; `docker restart` retains their old environment.
For planned changes, preserve the previous configuration and repeat health checks.

## Troubleshooting

Start with `chiado_compose ps` and the failing service's logs. Use the first
matching diagnostic below; do not delete state to clear an unexplained error.

| Failure | First check and action |
| --- | --- |
| Missing/mismatched genesis | Use the [startup block](#start-and-verify) to load all three env files and check hashes; stop before `chiado-up.sh` while diagnosing. Fresh deployment: finish registration. Existing deployment: recover exact bindings/genesis, including timestamp. |
| Deploy failed before bindings were written | Inspect the timestamped private log and `contracts/broadcast/`. Use `cast receipt` for each broadcast transaction and `cast nonce` for the deployer against the configured deployment RPC. Reconcile confirmed addresses/nonces before another deploy. |
| Signer unhealthy/rejecting | `chiado_compose logs --tail=100 eez-proof-signer`; repeat the startup key/hash checks. Check genesis mode with `stat -c '%a %n' datadir/genesis.json` (public file should be 644), UID 65532 readability, and L1 chain config `chiado`. |
| Port in use/database locked | `ss -lntup`; `docker ps -a --format '{{.Names}}'`; `chiado_compose ps`. Identify the owning process/deployment; a worktree/project name cannot isolate this profile. Never share a writable datadir. |
| L1/Beacon cannot catch up | `chiado_compose logs --tail=100 lighthouse eez-node`; repeat sync/peer checks. Inspect JWT agreement, checkpoint source (`LH_CHECKPOINT_URL`), snapshot backfill, bootstrap alias, and Fulu config before investigating network peers. |
| Beacon has no peers | `curl -fsS http://127.0.0.1:6152/eth/v1/node/identity \| jq '.data'`; inspect advertised `ip`/`udp`, firewall, NAT, and UDP 9205 reachability. See discovery change below. |
| Blob retrieval fails | Check the **settlement's slot and versioned hashes**, not only `blobs/head` (an empty array is valid at a blob-free head). Use the blob diagnostic below; check retention, sync, Fulu config, and semi-supernode mode. |
| L2 advances without settlement | `chiado_compose logs --tail=200 eez-node eez-proof-signer`; repeat L1 tip comparison, check poster balance/nonce on that L1, then builder inclusion, proof failure, policy due-work, fee ceilings, and gas/blob/timing deferrals. |
| Simulation cannot fetch a block | Verify deployment RPC freshness. Skip-simulation bypasses that phase only; inspect prior broadcasts before retrying. |

### Discovery change — recreates Lighthouse only

If the advertised ENR lacks the externally reachable endpoint, set
`LH_ENR_ADDRESS` and `LH_ENR_UDP_PORT` in `.env.chiado` to the public address and
mapped discovery port (default UDP **9205**). These flags do not configure NAT.
Keep both unset for automatic advertisement when that works. Then:

```bash
chiado_compose up -d --no-deps lighthouse
curl -fsS http://127.0.0.1:6152/eth/v1/node/peer_count | jq '.data'
curl -fsS http://127.0.0.1:6152/eth/v1/node/syncing | jq '.data'
```

Allow reconnection; require connected peers and successful sync, not just HTTP.

### Blob diagnostic — read-only

Supply the settlement transaction hash from startup output or `l1TransactionHash`
in the correlation response. This derives its Beacon slot and queries every
committed versioned hash through the configured Beacon API:

```bash
(
  set -euo pipefail
  source .env
  source .env.chiado
  read -r -p 'Settlement transaction hash: ' settlement_tx
  [[ "$settlement_tx" =~ ^0x[[:xdigit:]]{64}$ ]]
  beacon="${EEZ_L1_BEACON_URL%/}"
  tx="$(cast rpc eth_getTransactionByHash "$settlement_tx" --rpc-url http://127.0.0.1:18645)"
  jq -e '(.type == "0x3" or .type == "0x03") and (.blobVersionedHashes | length > 0)' <<<"$tx"
  block="$(jq -er '.blockHash' <<<"$tx")"
  timestamp="$(cast block "$block" --rpc-url http://127.0.0.1:18645 --json | jq -er '.timestamp')"
  genesis="$(curl -fsS "$beacon/eth/v1/beacon/genesis" | jq -er '.data.genesis_time')"
  seconds="$(curl -fsS "$beacon/eth/v1/config/spec" | jq -er '.data.SECONDS_PER_SLOT')"
  [[ "$timestamp" =~ ^(0x[[:xdigit:]]+|[0-9]+)$ && "$genesis" =~ ^[0-9]+$ && "$seconds" =~ ^[0-9]+$ ]]
  (( seconds > 0 && timestamp >= genesis && (timestamp - genesis) % seconds == 0 ))
  slot=$(( (timestamp - genesis) / seconds ))
  hashes="$(jq -er '.blobVersionedHashes[]' <<<"$tx")"
  while read -r versioned_hash; do
    curl -fsS --max-time 30 --get --data-urlencode "versioned_hashes=$versioned_hash" \
      "$beacon/eth/v1/beacon/blobs/$slot" \
      | jq -e '.data | length == 1 and (.[0] | type == "string" and startswith("0x") and length == 262146)'
  done <<<"$hashes"
)
```

**Pass:** every selected full blob is returned with the expected length. This
checks retrieval/shape; it does not independently verify KZG proofs or replay.

## Recovery and upgrades

1. Preserve the [inventory](#handoff-record), previous images/configuration, and
   a consistent backup. Stop database writers for a filesystem copy, or use a
   documented database-consistent snapshot method; copying live files is insufficient.
2. Establish history availability before rebuilding L2 state. The semi-supernode
   serves live blobs within its retention window. Older recovery requires a durable
   archival Beacon source or a validated protocol snapshot/bootstrap procedure.
   An execution snapshot does not provide missing blob history. Do not delete the
   current L2 database until the recovery path has been tested.
3. For upgrades, check node/signer/protocol compatibility, stored-state formats,
   configuration changes, and rollout order. Retain previous image IDs and backups;
   a binary downgrade works only if old code can read the resulting data and speak
   the deployed protocol. Repeat startup, semantic, and replay checks required by
   the change; record which actually ran.
4. For host/checkout moves, restore the exact keys, JWT, genesis/profile, bindings,
   compatible images and consistent data. Reconcile absolute paths in
   `deployments.env` with `FRESH_GENESIS` and the restored files, preserving all
   addresses/commitments. Recompute hashes before starting.

New contracts or a changed genesis create a different deployment; neither migrates
user state automatically. Missing bindings after a partial deploy require receipt
reconciliation, not the ordinary restart path. Do not claim recovery beyond retained
history until the complete recovery procedure has been exercised.

## Handoff record

Maintain the operational inventory privately; commit only sanitized evidence using
the [audit template](../audits/TEMPLATE.md), linked to this runbook:

| Record | Required contents |
| --- | --- |
| Source/runtime | UTC date, operator, host/checkout reference, fork commit and dirty patch identity, protocol pin, tool versions, immutable image IDs |
| Chain identity | L1/L2 chain IDs, registry/rollup/manager, proof system and attester, system/bridge addresses, deploy block, transaction receipts |
| State/files | Genesis/profile SHA-256 digests, initial block hash and state root; retained locations of bindings, data, JWT, and backups |
| Credentials | Private secret-manager/access references; never commit keys or authenticated URLs |
| Configuration | Timing, settlement/fee/blob policy, endpoint ownership, checkpoint/archival dependencies, and retention limits |
| Results/next actions | Current health, canonical settlement transaction/range, pending requests, executed semantic/restart/recovery checks, failures and untested assumptions |

Keep raw logs/databases outside Git. For shared evidence, retain a retrievable
artifact location and digest; an ignored local path is not available to another
operator. Update this procedure with verified corrections and preserve the dated
audit as the record of the particular execution.
