# Kurtosis local devnet

This package runs a disposable local EEZ network for development and testing.
It deploys the protocol contracts, generates a deployment-bound L2 genesis, and
starts the node, proof signer, private PoS L1, and MEV builder stack.

The included configuration contains deterministic private-network keys. Never
use these keys on a public network or fund them with real assets.

## Quick start

Run every command from the repository root.

### 1. Prepare the host

The local network requires:

- A Linux host with [Docker](https://docs.docker.com/engine/install/) running and
  enough free space to build four images.
- The [Kurtosis CLI](https://docs.kurtosis.com/install/).
- [Foundry](https://getfoundry.sh/introduction/installation/) v1.7.1 (`cast` and
  `forge`).
- Bash, Git, GNU `timeout`, `bc`, `jq`, `curl`, and `openssl`.
- The `eez-core-protocol` submodule at the commit pinned by this checkout.
- The standalone `eez-rollup-ui` repository cloned beside this repository, or
  its path exported as `EEZ_UI_REPO`. A prebuilt image can be used instead.
- Network access to pull container images and the pinned Ethereum package.

Initialize the submodule and start the Kurtosis engine:

```bash
git submodule update --init --recursive eez-core-protocol
kurtosis engine start
```

Optional preflight checks:

```bash
docker info
kurtosis version
forge --version
cast --version
jq --version
```

### 2. Start the network

Keep these variables exported in every shell used to operate or test the
network:

```bash
export KURTOSIS_ENCLAVE=eez-dev
export KURTOSIS_ARGS_FILE="$PWD/testing/kurtosis/ci-args.yaml"

bash testing/kurtosis/start.sh "$KURTOSIS_ARGS_FILE"
```

All lifecycle and workload scripts fall back to the enclave name `eez-ci` when
`KURTOSIS_ENCLAVE` is unset. Always export the intended name in a new shell,
especially before running the destructive `stop.sh` command.

For the interactive devnet with the configurable settlement policy, select
`testing/kurtosis/devnet-args.yaml` and `KURTOSIS_ENCLAVE=eez-devnet`. This profile
uses `always` for included pure L2 transactions and consolidates empty history
every 900 L2 blocks. With its two-second L2 cadence, that is nominally 30
minutes, subject to L1 inclusion. Empty Sync opportunities do not trigger a
post. Valid cross-chain work posts at the next eligible opportunity. Aggregate
blob fullness is disabled in this profile so it cannot shorten the idle period.
The fee floor is explicit for this local PBS fixture.

The default `ci-args.yaml` and interactive `devnet-args.yaml` enable fee
collection in both directions. Their private-network cost model uses 1:1 native
token conversion, zero off-chain service/risk allowances and a 20% quote margin.
L2 tips go to `0x4444444444444444444444444444444444444444`; the builder refunds
90% of the selected L1 source reward, less payout cost, to the separate plain EOA
`0x5555555555555555555555555555555555555555`. The refund percentage is not the
profit margin. Assumptions expire on 2100-01-01; these are synthetic test values,
not production pricing. Ordinary L2 settlement remains subsidized.

Start a fresh enclave to test these defaults; editing an args file does not
change a running Composer's startup snapshot. The specialized blob, settlement
policy, partial-consumption and cross-chain-limit profiles keep fee collection
disabled so their independent policy assertions retain their original inputs.
To exercise disabled behavior in a custom profile, set both collection flags
to `"false"`; to disable advisory L2 quotes too, remove the complete cost model.
The protocol network runner requires successful quotes and cannot run against
that fee-disabled profile.

The source snapshot regression is
[`verify-nonce-snapshot.mjs`](scripts/verify-nonce-snapshot.mjs). It interleaves
ordinary L1 transactions with native cross-chain settlement, then checks explicit
and omitted nonces through both quote methods and both fee encodings. It also
requires a genuine future nonce gap to fail and checks canonical destination
effects. Use an owned fee-enabled enclave with chain IDs 7331/696990, built
`contracts/out/Value.sol/Value.json`, and two distinct funded fixture accounts:

```bash
export KURTOSIS_ENCLAVE=eez-nonce-snapshot
# Set this enclave's EEZ_DEVNET_L1_RPC, EEZ_DEVNET_L2_RPC,
# EEZ_DEVNET_L1_FRONT and EEZ_REGISTRY_ADDRESS from its service bindings.
# Set EEZ_TEST_FEE_KEY and EEZ_COMPANION_TEST_KEY to its public fixture keys.
mkdir -p artifacts/nonce-snapshot
export EEZ_NONCE_SNAPSHOT_EVIDENCE="$PWD/artifacts/nonce-snapshot/result.json"
node testing/kurtosis/scripts/verify-nonce-snapshot.mjs
```

The script requires an ordinary transaction and a cross-chain source to settle
in the same L1 block. It first sends a zero-value L2 warmup and waits for its
canonical settlement (up to ten minutes on a fresh PBS relay awaiting validator
registrations). Retain Composer logs as well: `refreshing fee quote source
snapshot` at debug level establishes that the stale-snapshot branch was reached;
successful receipts alone do not establish that coverage.

Measure idle cadence after the first canonical settlement: a fresh local MEV
relay must receive validator registrations before it can include bundles.

`start.sh` does not request privileged package execution by default. Set
`KURTOSIS_PRIVILEGED=1` only when using a custom topology that requires it.

The first start builds the node, proof-signer, deployment, and standalone UI images and
pulls the Ethereum client images. It can take several minutes. The command returns
after Kurtosis has deployed the contracts and started the network services. It
leaves the enclave running.

Before deployment, startup waits up to 5 minutes for the canonical L1 RPC to
return block zero; an open RPC port alone is insufficient. The devnet deploys
with `EEZ_L1_REFERENCE=genesis`, so L1 block zero supplies the native L2
genesis reference. The L1 genesis block is final by definition, so startup
does not wait for the fresh beacon chain's first finalized checkpoint (more
than 20 minutes with mainnet-preset epochs). The node and signer accept a
block-zero reference once it matches the canonical L1 block zero; any other
reference still requires L1 finality. Public networks (Ethereum mainnet and
Chiado) always use the `finalized` default, and deployment rejects `genesis`
there. The deployment script never substitutes `latest` for `finalized`.

### 3. Check the deployment

```bash
kurtosis enclave inspect "$KURTOSIS_ENCLAVE"
bash testing/kurtosis/scripts/verify-eezl2-deployment.sh
```

The verification checks the live EEZL2 system address, rollup ID,
`USE_GAS_LEFT` setting, runtime code hash, genesis state root, and registered
genesis block hash against the generated deployment bindings.

### 4. Connect to the RPC endpoints

`start.sh` prints an endpoint summary after starting the network. To print it
again and export the URLs into the current shell, source the port helper:

```bash
source testing/kurtosis/ports.sh
```

The summary has this shape:

```text
EEZ Kurtosis devnet endpoints (enclave: eez-dev)
  Canonical L1 RPC:      http://127.0.0.1:<port>
  L2 RPC:                http://127.0.0.1:<port>
  L1 cross-chain front:  http://127.0.0.1:<port>
  L2 cross-chain front:  http://127.0.0.1:<port>
  Proof signer gRPC:     127.0.0.1:<port>
  Protocol playground:   http://127.0.0.1:<port>
  L1 explorer:           http://127.0.0.1:<port>
  L2 explorer:           http://127.0.0.1:<port>
  L1 explorer API:       http://127.0.0.1:<port>
  L2 explorer API:       http://127.0.0.1:<port>
  Blobscan:              http://127.0.0.1:<port>
  Blobscan API:          http://127.0.0.1:<port>
```

The sourced helper exports:

```text
EEZ_DEVNET_L1_RPC
EEZ_DEVNET_L2_RPC
EEZ_DEVNET_L1_FRONT
EEZ_DEVNET_L2_FRONT
EEZ_DEVNET_PROOF_SIGNER_GRPC
EEZ_DEVNET_DASHBOARD
EEZ_DEVNET_L1_EXPLORER
EEZ_DEVNET_L2_EXPLORER
EEZ_DEVNET_L1_EXPLORER_API
EEZ_DEVNET_L2_EXPLORER_API
EEZ_DEVNET_BLOBSCAN
EEZ_DEVNET_BLOBSCAN_API
```

The HTTP URLs can be reused with `cast`, `curl`, or an application under
development:

```bash
cast chain-id --rpc-url "$EEZ_DEVNET_L1_RPC"
cast block-number --rpc-url "$EEZ_DEVNET_L2_RPC"
```

`EEZ_DEVNET_L1_FRONT` is the wallet endpoint for L1 and
`EEZ_DEVNET_L2_FRONT` is the wallet endpoint for L2. Each front traces signed
transactions before submission: L1-to-L2 or L2-to-L1 transactions are held for
composition, while ordinary transactions are forwarded to that chain's normal
mempool. The direct `EEZ_DEVNET_L1_RPC` and `EEZ_DEVNET_L2_RPC` endpoints remain
available for protocol tooling.

The Blockscout variables are exported only when `enable_explorers` is enabled
in the selected arguments file. The Blobscan variables are exported when the
selected Ethereum-package profile includes `blobscan` in `additional_services`.

### 5. Stop the network

```bash
bash testing/kurtosis/stop.sh
```

This force-removes the enclave named by `KURTOSIS_ENCLAVE`, including its
ephemeral chain state. Download any required artifacts or logs before stopping
it.

## What the package deploys

The local topology contains:

- A canonical private PoS L1 with one reth/Lighthouse participant.
- An rbuilder, relay, MEV-Boost, and proposer path for atomic bundle inclusion.
- A deployment task that deploys the L1 contracts, derives the configured L2
  system address, renders the EEZL2 runtime and funded L2 genesis, and registers
  the resulting genesis block hash (the state root remains an independently
  verified genesis integrity value).
- An `eez-node` running the L2, composer, cross-chain RPC fronts, and an
  embedded L1 execution client.
- An `eez-follower` Lighthouse beacon node that follows the canonical L1 and
  drives the embedded execution client through the Engine API.
- An `eez-proof-signer` that validates L2 windows and returns attestations to
  the node.
- The standalone `eez-rollup-ui` image, based on the earlier sync-rollups POC UI
  with a current-network compatibility layer. Its original Dashboard, Counter
  Demo, Bridge, Flash Loan, Aggregator,
  Faucet, and Visualizer screens remain intact. Counter/proxy calls,
  bidirectional bridge transfers, and the nested L1 -> L2 -> L1 flash loan use
  real transactions. The current protocol branch does not deploy the old
  Aggregator or reverse-flash contracts, and the legacy Visualizer batch decoder
  still needs to be ported to `postAndVerifyBatch`.
- Optional L1 and L2 Blockscout instances, each with its own backend, contract
  verifier, and ephemeral PostgreSQL database. They are enabled in the included
  local profile.
- Blobscan web, API, indexer, PostgreSQL, and Redis services. The indexer reads
  the canonical L1 execution and beacon APIs and provides searchable historical
  blob transactions, versioned hashes, commitments, and sidecars. It is enabled
  in `devnet-args.yaml` and `blob-args.yaml`, and intentionally omitted from the
  lean CI profile.

Blobscan `1.40.3` does not define arbitrary private chain IDs in its internal
chain registry, while the upstream Kurtosis launcher uses floating images. The
local integration therefore builds API and web from pinned commit
`424d5e512d57ff4ac42509ef93f3845ead392155`, adds exactly the selected EEZ chain
ID with BPO1 active at genesis, and pins the indexer, PostgreSQL, and Redis image
digests. Set `EEZ_SKIP_BLOBSCAN_BUILD=1` only when those chain-specific images
already exist locally.

The topology deliberately omits dedicated load-generation, reorg, metrics, and
distributed-tracing services. The included shell workloads generate their own
test traffic.

### Isolated blob-DA profile

The developer profile uses Blob DA and includes Blobscan. The isolated profile
provides a smaller, focused Blob DA validation enclave so experiments cannot
mutate a running developer network:

```bash
KURTOSIS_ENCLAVE=eez-blob \
KURTOSIS_ARGS_FILE=testing/kurtosis/blob-args.yaml \
  bash testing/kurtosis/start.sh

KURTOSIS_ENCLAVE=eez-blob \
  bash testing/kurtosis/scripts/verify-blob-da.sh
```

This profile posts the native `eez-core-protocol` blob stream in an EIP-4844
type-3 transaction. The stream contains the rollup `ChainOperation`, every
cross-chain transaction bracket, its ordered `Call`/`StaticCall`, return, and
rollback actions, and `CloseBlobStream`. The proof signer requires those
semantics to reproduce the validated execution tables before it signs.

The verifier uses the Fulu EIP-7594 sidecar wrapper, retrieves the blobs
independently through the standard `/eth/v1/beacon/blobs/{block_id}` REST API,
verifies their KZG versioned hashes,
decodes the native semantic stream, binds each semantic transaction to the
exact raw user transaction, and checks L1/L2 block-hash commitment convergence.
It then verifies that Blobscan indexed each proven posting transaction with the
same versioned hashes and commitments, and byte-compares every PostgreSQL-backed
Blobscan payload returned by both the API and web data proxy with its canonical
Beacon blob. Blobscan evidence is written beside the main summary as
`blobscan.json`. To recheck existing evidence without generating transactions,
run:

```bash
KURTOSIS_ENCLAVE=eez-blob \
  bash testing/kurtosis/scripts/verify-blobscan.sh \
  artifacts/kurtosis-blob/<run>/summary.json
```

The playground exposes a known prefunded development private key in its runtime
configuration so browser actions require no wallet extension. This is suitable
only for the disposable private network. Never reuse the key or this UI
configuration on a public network.

## Operate the network

### Inspect services

Show the service state and published ports:

```bash
kurtosis enclave inspect "$KURTOSIS_ENCLAVE"
```

After `kurtosis service stop` / `start`, source `testing/kurtosis/ports.sh`
again before using the exported RPC URLs: Docker can reassign published host
ports when the same container starts again.

Follow the main service logs:

```bash
kurtosis service logs -f "$KURTOSIS_ENCLAVE" eez-node
kurtosis service logs -f "$KURTOSIS_ENCLAVE" eez-proof-signer
```

Other useful services are:

```text
eez-follower
el-1-reth-lighthouse
el-2-reth-builder-lighthouse
mev-relay-api
l1-blockscout-frontend
l2-blockscout-frontend
blobscan-web
blobscan-api
blobscan-indexer
```

Replace the service name in the log command to inspect one of them. Use `-a`
instead of `-f` to print the available log history without following it.

### Query the chains

After resolving `EEZ_DEVNET_L1_RPC` and `EEZ_DEVNET_L2_RPC` as shown above:

```bash
cast block-number --rpc-url "$EEZ_DEVNET_L1_RPC"
cast block latest --rpc-url "$EEZ_DEVNET_L1_RPC"
cast block-number --rpc-url "$EEZ_DEVNET_L2_RPC"
cast block safe --rpc-url "$EEZ_DEVNET_L2_RPC"
```

### Browse blocks, transactions, and internal calls

The included local profile starts one Blockscout explorer per chain. After
sourcing the endpoint helper, open:

```bash
echo "$EEZ_DEVNET_L1_EXPLORER"
echo "$EEZ_DEVNET_L2_EXPLORER"
```

The L1 explorer indexes the canonical L1, and the L2 explorer indexes the EEZ
L2. Internal calls are available from a transaction's internal-transactions
view after Blockscout indexes its execution trace.

To run without the explorers, set this in the selected arguments file:

```yaml
eez:
  enable_explorers: false
```

When explorers are disabled, `ports.sh` omits their URLs and does not export the
explorer variables.

### Verify contract sources in Blockscout

The deployment artifact preserves the Foundry broadcasts created inside the
Kurtosis deployment task. Download it and submit those exact deployments to the
L1 Blockscout verifier:

```bash
source testing/kurtosis/ports.sh

EEZ_DEVNET_VERIFY_DIR="$(mktemp -d)"
kurtosis files download \
  "$KURTOSIS_ENCLAVE" eez-deployments "$EEZ_DEVNET_VERIFY_DIR"

EEZ_BLOCKSCOUT_URL="$EEZ_DEVNET_L1_EXPLORER_API" \
EEZ_L1_RPC_URL="$EEZ_DEVNET_L1_RPC" \
EEZ_BROADCAST_DIR="$EEZ_DEVNET_VERIFY_DIR/foundry-broadcast" \
  bash scripts/verify-blockscout.sh
```

Then verify the genesis-installed `EEZL2` contract in the L2 explorer:

```bash
EEZ_BLOCKSCOUT_URL="$EEZ_DEVNET_L2_EXPLORER_API" \
EEZ_L2_RPC_URL="$EEZ_DEVNET_L2_RPC" \
EEZ_DEPLOYMENTS_FILE="$EEZ_DEVNET_VERIFY_DIR/deployments.env" \
EEZ_PROTOCOL_DIR="$PWD/eez-core-protocol" \
  bash testing/kurtosis/scripts/verify-eezl2-blockscout.sh
```

The L1 helper submits each saved `CREATE`/`CREATE2` deployment. The L2 helper
verifies the EEZL2 implementation behind the genesis proxy with Standard JSON,
because a genesis predeploy has no creation transaction.
Both print their result without modifying either chain. If an explorer is still
indexing, wait briefly and run the corresponding command again.

### Use a funded development account

The following public Hardhat account has 1,000,000 test ETH in both the
canonical L1 configuration and the L2 development genesis (`genesis.dev.json`):

```text
Private key: 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
Address:     0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
```

Export it and confirm its balances:

```bash
export EEZ_DEVNET_TEST_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
export EEZ_DEVNET_TEST_ACCOUNT="$(
  cast wallet address --private-key "$EEZ_DEVNET_TEST_KEY"
)"

cast balance "$EEZ_DEVNET_TEST_ACCOUNT" --ether \
  --rpc-url "$EEZ_DEVNET_L1_RPC"
cast balance "$EEZ_DEVNET_TEST_ACCOUNT" --ether \
  --rpc-url "$EEZ_DEVNET_L2_RPC"
```

For example, submit an ordinary L2 transfer:

```bash
cast send 0x1111111111111111111111111111111111111111 \
  --value 1wei \
  --private-key "$EEZ_DEVNET_TEST_KEY" \
  --rpc-url "$EEZ_DEVNET_L2_RPC"
```

This account is public and insecure. Use it only on this disposable local
network. The workload generator also uses it for L2 setup and filler
transactions, so avoid sending manual transactions from it while a workload is
running.

### Download deployment artifacts

The `eez-deployments` Kurtosis artifact contains:

- `deployments.env`, with public contract addresses and deployment bindings.
- `l2-genesis.json`, used to start the L2.
- `l2-genesis.profile.json`, with the generated runtime hash and genesis root.
- `foundry-broadcast`, with the L1 creation metadata used for Blockscout source
  verification.

Download it into a new temporary directory:

```bash
EEZ_DEVNET_DEPLOYMENTS_DIR="$(mktemp -d)"
kurtosis files download \
  "$KURTOSIS_ENCLAVE" eez-deployments "$EEZ_DEVNET_DEPLOYMENTS_DIR"
ls -la "$EEZ_DEVNET_DEPLOYMENTS_DIR"
```

Load the public deployment values into the current shell when needed:

```bash
set -a
source "$EEZ_DEVNET_DEPLOYMENTS_DIR/deployments.env"
set +a
```

`deployments.env` does not contain the L2 system private key. The deterministic
private keys used by this local profile remain in `ci-args.yaml`.

### Capture diagnostics

Before removing a network, save its service specifications and logs to a new
directory:

```bash
mkdir -p "$PWD/artifacts"
export EEZ_DEVNET_DUMP_DIR="$PWD/artifacts/kurtosis-dev-dump"
kurtosis enclave dump "$KURTOSIS_ENCLAVE" "$EEZ_DEVNET_DUMP_DIR"
```

Choose a different `EEZ_DEVNET_DUMP_DIR` if that destination already exists.

### Reset or rebuild the network

Protocol deployment and genesis generation happen when the enclave is created.
To redeploy after changing the node, contracts, genesis generation, or package
configuration, remove the current enclave and start it again:

```bash
bash testing/kurtosis/stop.sh
bash testing/kurtosis/start.sh "$KURTOSIS_ARGS_FILE"
```

Stopping is destructive. Use a different `KURTOSIS_ENCLAVE` value if the old
network must remain available.

## Generate test traffic

### Exercise L1 builder refunds

Use an isolated enclave with the [L1 refund profile](../../docs/configurable-settlement-policy.md#l1-builder-refund-profile)
and its matching cost policy in `settlement_env`. The dedicated recipient must
be a plain EOA, different from the source and poster. Keep both chain IDs and
the L2 beneficiary consistent with the profile. Build integration fixtures with
`forge build --root contracts`, export the intended `KURTOSIS_ENCLAVE`, source
`testing/kurtosis/ports.sh` and load the enclave's deployment bindings as above.

`scripts/verify-l1-fees.mjs` requires those endpoints, `EEZ_REGISTRY_ADDRESS`,
an independently funded public devnet fixture key in `EEZ_TEST_FEE_KEY`, and
`EEZ_FEE_EVIDENCE_FILE`. It refuses public chain IDs. The script exercises both
transaction types through the shared SDK, rejects an underpaid source, checks
nested calls and official Safe 1.4.1 creation, then independently reads the native
blob post, source and builder payment from the canonical L1 block. The actual
recipient balance increase must cover measured post/blob/service costs and risk.
It then funds four deterministic private-network actors with one test ETH each,
submits two concurrent waves of nested sources (mixed legacy/EIP-1559), and
requires each wave to settle in one post. The measured largest total priority
payment must produce the only refund. The first shared batch must be undercovered
to prove the accepted subsidy. The next batch must settle after that observation;
it may be profitable as posting fees change. A profitable first batch or a split
batch does not establish this regression. It retains incremental evidence before
broadcasting each source:

```bash
node testing/kurtosis/scripts/verify-l1-fees.mjs
```

This is a real builder test. The development mempool adapter does not preserve
bundle ordering or implement refunds and cannot establish success. The script
does not replace independent follower replay or qualify a public provider;
run the replay procedure below and qualify each deployment's actual route.

### Exercise L2 settlement fees

Use a disposable enclave configured with the complete
[fee cost model](../../docs/configurable-settlement-policy.md#cross-chain-fee-configuration),
a nonzero `EEZ_L2_FEE_RECIPIENT`, and `EEZ_FEES_L2_TO_L1_ENABLED: "true"` in
`settlement_env`. Chain IDs and rollup identity come from runtime wiring. Enable the UI to test
its quote proxy as well. Build integration fixtures with `forge build --root
contracts`, export the intended `KURTOSIS_ENCLAVE` and `KURTOSIS_ARGS_FILE`, and
source `testing/kurtosis/ports.sh` to obtain the RPC endpoints.

`scripts/verify-l2-fees.mjs` requires those L1/L2/front URLs, the deployed
`EEZL2_ADDRESS`, a funded private-network fixture key in `EEZ_TEST_FEE_KEY`, and
an output path in `EEZ_FEE_EVIDENCE_FILE`. Optionally set
`EEZ_TEST_FEE_RPC_URL` to the UI's `/composer/l2` RPC URL. The exercise uses
`eez_debugQuoteFee` for receipt comparisons and the compact SDK fields for signing.
It refuses public chain IDs, creates two independent nested-call fixtures,
rejects an underpaid signed transaction, sends each paid source through the
SDK's single-signature path, and checks receipts, beneficiary, target state
and canonical queue disposition. Run it from the repository root:

```bash
node testing/kurtosis/scripts/verify-l2-fees.mjs
```

The evidence records whether the two sources shared a native block; the
exercise does not guarantee batching. It does not replace independent blob
verification, follower replay or restart checks. The pinned protocol network
runner requests quotes for cross-chain triggers with their prepared nonces; see
its [fee and retry rules](../../eez-core-protocol/script/e2e/RPC.md#eez_quotefee).
The standard CI profile collects fees in both directions. Cross-chain waves
reuse that checked quote/signing client and retain quote evidence alongside
their logs. Submission retries explicitly recognized temporary admission
refusals for at most 120 seconds using identical signed bytes. Underpayment,
unsupported profiles, transport ambiguity and changed response hashes fail the
test. L2 waves wait for the preceding source nonce to settle before
quoting its successor; held future L2 nonces and source nonces changed by
unsettled execution are outside the quote profile. Funding, deployments and pure
L2 fillers retain ordinary node fees.

The state-chaining test prepares its burst before one Sync boundary, with an
explicit 100 gwei priority bid when fees are enabled
(`EEZ_TEST_PRIORITY_GAS_PRICE_WEI` can override it), against the local profile's
2 gwei maximum posting tip. This is an execution-order test under funding
enforcement, not a quote-margin test. With L2 fees enabled it uses three source
accounts to preserve the same-block contract state dependency: signed admission
also rejects held future L2 nonces. Disabled profiles retain the original
single-sender L2 burst; L1 keeps its single-sender burst in both modes.
Invalid calls are signed directly: enabled profiles assert the specific fee
admission rejection and absence of queue ownership/receipts, while disabled
profiles retain the poison-eviction assertion. Neither path has an automatic
fallback from a rejected quote.
The mixed-direction case asserts separate posts in FIFO order when either fee
route is enabled, and verifies proofs for both Sync heights. Disabled profiles
retain the original shared-Sync assertion. Root and contract-state convergence
remain required in both cases.

For receipt-cost calibration, run `node testing/kurtosis/scripts/verify-l2-bridge-fees.mjs`
with the same private-network endpoints/key and the deployed `EEZ_L1_BRIDGE_SENDER`,
`EEZ_L2_BRIDGE_RECEIVER` and `EEZ_BRIDGE_FEE_EVIDENCE_FILE`. Obtain the bridge
addresses from `demo.env` in the enclave's `eez-ui-demo` files artifact; the
base `deployments.env` does not contain these demo contracts. This requires the
1:1, 20% target profit, zero fixed-cost/risk-reserve trial policy. It funds escrow through a
real inbound bridge, then verifies a first type-2 withdrawal and a repeated
legacy withdrawal. It checks principal and fee balance changes, native source
gas, L1 receipt gas within 1,000 of the quote, and payment within 2% above the
actual complete posting/blob cost plus its 20% allowance. This regression catches
excessive pricing that a lower-bound funding check alone cannot detect.

The workload scripts discover the enclave ports and deployment artifact
automatically. They also read the deterministic L1 poster key (never the proof
signer key) as their funding account from `KURTOSIS_ARGS_FILE`, so that variable
must point to the arguments file used to start the network.

### Verify an existing public-network deployment

The same deployment, blob, protocol, wave, state-chaining and replay checks can
use explicit endpoints instead of a Kurtosis enclave:

```bash
export EEZ_USE_LOCAL_DEPLOYMENTS=1
export EEZ_DEPLOYMENTS_FILE=/absolute/path/to/deployments.env
export EEZ_DEVNET_L1_RPC=http://127.0.0.1:18645
export EEZ_DEVNET_L2_RPC=http://127.0.0.1:18688
export EEZ_DEVNET_L1_FRONT=http://127.0.0.1:18999
export EEZ_DEVNET_L2_FRONT=http://127.0.0.1:18998
export EEZ_L1_BEACON_URL=http://127.0.0.1:6152
export EEZ_L1_BUILDER_RPC_URL=https://builder.chiado.gcd.ovh/
export EEZ_NODE_CONTAINER=eez-node-chiado
export EEZ_SIGNER_CONTAINER=eez-proof-signer-chiado
export EEZ_TEST_FUND_AMOUNT_ETH=0.05
```

Supply `PK`, `EEZ_FUND_FROM_KEY`, `EEZ_L1_SETUP_KEY`, and `EEZ_L2_SETUP_KEY`
through a private environment using funded test accounts. Keep the poster's
nonce stream separate from test traffic. `EEZ_TEST_FUND_AMOUNT_ETH` is a positive
native-token amount with at most 18 decimal places; its existing default is 10.
The upstream protocol runner separately defaults to 0.1 native units per worker
per chain. These are funding transfers, not settlement fees.

Explicit mode requires all four endpoints and a readable deployment file; it
never falls back to an enclave. Node/signer log assertions use the named Docker
containers. Blob verification probes the public builder's bundle method while
retaining real inclusion, KZG, decoded payload and correlation assertions.
`EEZ_L1_BUILDER_RPC_URL` may list several comma-separated relays, matching the
Composer setting; the probe requires at least one that serves `eth_sendBundle`
and warns about the others. The `eez.builder_rpc_url` argument accepts the same
format and defaults to the enclave's rbuilder. Set
`EEZ_VERIFY_BLOBSCAN=false` when no Blobscan instance is available; this only
omits explorer checks. Native Beacon/KZG verification remains required.

Independent protocol checks require L1/L2 `debug` APIs, including raw blocks,
receipts, execution witnesses and chain configuration. Lookup-prefix replay also
requires the deployed `EEZ_L2_GENESIS` file; the collector checks its genesis hash
and configuration against the node before replay. It also requires
`EEZ_L1_CHAIN_CONFIG`: the deployed Ethereum genesis file or an existing public
preset (`mainnet`, `sepolia`, `hoodi`, `chiado`). Kurtosis supplies both genesis
files from its artifacts. The retained prefix input embeds that L1 profile,
the manager's registered verification key and proof-system signer read at the original post, and at most 65 raw
L1 headers (the inherited endpoint and up to 64 imports). The offline helper
checks the profile commitment, exact header decoding/linkage, parent trie,
history transaction and PREVRANDAO before reconstructing transaction prefixes.
It performs no RPC; the surrounding collector still establishes canonical L1/L2
identities through RPC. Private Gnosis genesis profiles are not supported by
this prefix helper. Mined raw transactions missing from
`eth_getRawTransactionByHash` are recovered from the exact raw-block position
and must hash to the expected canonical transaction before matching blob data.
Retain transaction lookup indexes and receipt/state history in the L1 client;
a pruned snapshot can otherwise appear healthy until in-memory caches disappear
on restart. Rebuilding an index does not restore already-pruned history.

Fresh-follower replay also requires `EEZ_L1_CHAIN_CONFIG` (`chiado`
for this network), the actual `EEZ_L2_BLOCK_TIME_MS`, and the fixed native
`EEZ_L2_SYSTEM_ADDRESS` (`0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076`). Use the actual proof/slack timing values when supplied.
An existing canonical post satisfies wave bootstrap readiness; each measured
wave still counts only posts occurring after its own baseline. State-ordering
checks wait for canonical safe L2 receipts before recording their Sync height:
a public builder can exclude a bundle and move its transactions to a later Sync
block. L1 log queries start at the selected registry deployment block rather
than scanning unavailable pre-deployment history in a pruned snapshot.

### Run one cross-chain workload

```bash
EEZ_WAVE_MODE=mixed EEZ_WAVE_COUNT=1 \
  bash testing/kurtosis/scripts/cross-chain-wave.sh
```

Available modes are:

| Mode | Workload |
| --- | --- |
| `inbound` | L1-to-L2 calls, including a deposit and direct/wrapped contract calls. |
| `outbound` | L2-to-L1 calls, including a withdrawal and direct/wrapped contract calls. |
| `mixed` | Inbound and outbound calls exercised in one workload. |
| `mixed-pure` | Mixed traffic plus ordinary L2 mempool transactions between waves. |

Each workload verifies transaction inclusion, cross-chain state convergence,
`postBatch` settlement on L1, L1/L2 block-hash commitment agreement, L2 safe-head progress,
and correlation between the proof signer's result and the node's accepted
attestation.

The blob-DA smoke additionally calls both node correlation methods for every
test transaction: `eez_getSettlementByL2Block` must resolve its exact L1 posting
transaction, and `eez_getSettledL2RangesByL1Block` must return a range containing
the exact L2 block number/hash. The verified replies are retained under
`correlation_rpc` in the run's `summary.json`.

Individual workload logs are written under `datadir/smoke-logs`. Override their
locations with `EEZ_NODE_LOG` and `EEZ_PROOF_SIGNER_LOG`.

Useful workload controls include:

- `EEZ_WAVE_COUNT`: number of waves; the script default is three.
- `EEZ_WAVE_GAP_SECS`: delay between waves; the default is 20 seconds.
- `EEZ_FILLER_PER_GAP`: pure L2 transactions between `mixed-pure` waves; the
  default is two.
- `EEZ_RECEIPT_WAIT_SECS`: transaction inclusion timeout; the default is 300
  seconds.
- `EEZ_EFFECTS_WAIT_SECS`: destination-state convergence timeout after source
  receipts land; the default is 120 seconds.
- `EEZ_STATE_ROOT_WAIT_SECS`: legacy-named block-hash commitment convergence
  timeout; the default is 30 seconds.

### Run the state-chaining regression

```bash
bash testing/kurtosis/scripts/verify-state-chaining.sh
```

The regression covers source-state, destination-state, and mixed-operation
chaining in each direction. It requires each group of three transactions to
share one Sync block and asserts every ordered return: repeated destination
calls return `changed = true, false, false`, source-derived calls send
`1, 2, 3`, and the mixed sequence proves that fixed and source-derived calls
see one another's intermediate state. An isolated inbound scenario makes two
identical proxy calls from one source transaction, verifies the ordered
`changed = true, false` results, and confirms its `postBatch` and user
transaction landed in the same builder-produced L1 block. The suite also
places a deployed non-proxy between two valid inbound calls and verifies that
only the poison transaction is evicted while both ordered survivors settle. A
final mixed-direction case verifies separate, FIFO-ordered Sync blocks when
fee collection is enabled; with fees disabled it requires the outbound source
transaction and inbound delivery to share one canonical L2 Sync block. Every
scenario also verifies bundle settlement, proof-signer acceptance, and L1/L2
block-hash commitment convergence. Prepared groups begin collection after a
fresh accepted idle cutoff, independent of whether the settlement policy posts
an empty batch.

### Validate partial consumption

Use an isolated enclave and locally built node/signer images tagged
`eez-node:partial-consumption`, `eez-proof-signer:partial-consumption`, and
`eez-deploy:partial-consumption`. Every profile uses in-memory settlement tracking
and permits included inbound users to revert. This profile pins the builder image
and isolates the canary's chain and funding.

```bash
kurtosis run --enclave eez-partial-consumption ./testing/kurtosis \
  --args-file testing/kurtosis/partial-consumption-args.yaml
```

Deploy the existing cross-chain Value fixture with `cross-chain-wave.sh` on
this enclave, then pass its L1 Value proxy to the canary:

```bash
python3 testing/kurtosis/scripts/verify-partial-consumption.py \
  --enclave eez-partial-consumption --proxy "$VALUE_PROXY_L1" \
  --output artifacts/partial-consumption.json
```

This private network has chain ID 7331 and deterministic development funding
keys. The canary creates three fresh source users, waits for their proved
attempt's dispatch event, and replaces the middle user's nonce before
the target block. It requires the replacement, postBatch and two remaining
source users to land in order in the same builder block. It checks the exact
partial L2 commitment, `Committed` / `SourceNonceSpent` / `SourceReverted`
dispositions from reconciliation events, source receipts, and healthy subsequent
settlement.

The profile pins rbuilder to
`sha256:916ba84da2690e3e368f57da5b0c73bfd919b452de8b95f64895846e0f3a8308`
(source commit `6037fa728b13bf1806e16fff2586414216f6b8fa`). Its generated
`mgp-ordering` and `mp-ordering` builders use `discard_txs = true`: this permits
invalid transactions in the revert allowlist to be skipped. The canary fails
if the image or observed inclusion behavior differs. This builder setting is
separate from the permission to include an EVM revert. No dropping allowlist
is sent.

Pending transactions and retry counters are held in memory. After a restart,
clients may need to resubmit unsettled transactions; canonical state is recovered
from L1.

### Run all included workloads

The suite requires an output directory:

```bash
export EEZ_CI_RESULT_DIR="$PWD/artifacts/kurtosis-dev"
mkdir -p "$EEZ_CI_RESULT_DIR"

bash testing/kurtosis/scripts/verify-cross-chain-waves.sh
```

Despite the variable's historical `CI` name, this command operates on the
already-running local enclave and leaves it running. It executes one `inbound`,
`outbound`, and `mixed` wave, followed by three `mixed-pure` waves and the
inbound/outbound state-chaining regression. Per-mode output is stored under
`$EEZ_CI_RESULT_DIR/checks`. Override the stress count with
`EEZ_MIXED_PURE_WAVE_COUNT`.

### Run the pinned protocol E2E suite

The protocol runner discovers every network scenario from the pinned
`eez-core-protocol` submodule and refuses to run against a different checkout:

```bash
export EEZ_CI_RESULT_DIR="$PWD/artifacts"
bash testing/kurtosis/scripts/run-protocol-e2e.sh
```

The suite uses the pinned upstream `script/e2e/run/network/parallel.sh`, with a
fresh funded wallet per worker. `EEZ_PROTOCOL_MAX_PARALLEL=all` (the local script
default) starts one worker per eligible scenario, including `revertCounter` and
`staticCounterL2`. GitHub-hosted CI sets `EEZ_PROTOCOL_MAX_PARALLEL=4` in both
publication paths; this changes concurrency, not scenario selection. Workers spend
most of their time in the setup confirmation wait below, so additional workers
overlap settlement latency rather than compilation. Foundry
artifacts are warmed before workers start. Set a positive integer to bound
concurrency, or `1` to run serially with
the same assertions. When both bridge scenarios are selected, `bridge` runs
first: its settled deposit supplies the shared rollup escrow required by
`bridgeL2`. The remaining scenarios then run in parallel with fresh worker
wallets. A failed deposit skips the dependent release and fails the suite;
unrelated scenarios still run. Selecting `bridgeL2` alone requires existing
escrow, as specified by the pinned protocol's scenario.

Each classic preparation worker captures its L2 setup transaction hashes in a
private broadcast directory and waits for successful canonical receipts at or
below the L2 `safe` head before signing its trigger. `E2E_SETUP_TIMEOUT` bounds
that wait (default 180 seconds); ordinary L2 activity must settle independently
of the cross-chain trigger. CI retains this confirmation barrier. The upstream
`--no-wait-for-confirmation` / `E2E_NO_WAIT_FOR_CONFIRMATION=true` opt-out is for
explicit development runs and can reintroduce setup/nonce races; it does not
disable fee quoting or post-trigger verification.

The full CI entry point, `run-ci.sh`, completes its protocol and integration fixture builds
before starting the network, readiness, and sequential wave/state-chaining checks.
Cold compilation must not compete with the live PBS fixture's slot deadlines on
a small hosted VM. Each build uses one Forge thread and a 20-minute timeout,
retaining the pinned compiler/optimizer/metadata settings. All preparation must
succeed before protocol scenarios begin. Failure or cancellation stops the
owned preparation process group; a busy protocol-suite
lock or mismatched pin is rejected before network startup/teardown. The existing
upstream `forge build` still runs afterwards and checks the warmed artifacts;
there is no runtime-binary rebuild. The published Kurtosis job restores
`eez-core-protocol/out` and `cache` keyed on the exact protocol gitlink and
Foundry version, with no prefix fallback; only successful jobs save it. A miss
compiles cold, and Forge still recompiles any source whose content hash differs.
The no-publication fallback and the integration fixtures are not cached.

`protocol-build.log` and `integration-build.log` retain compiler output.
`timings.jsonl` records completed
phases with elapsed seconds and exit codes, including individual wave modes and
state chaining; a forcibly cancelled phase may have no completion record.
Nested phase durations must not be added together as wall time.
The protocol wrapper streams phase boundaries and 30-second progress messages
while retaining the raw logs. Scenario output/accounting remains unchanged.

Each worker is funded with `EEZ_PROTOCOL_FUND_ETH` per chain (default `1` test ETH),
which the wrapper passes to upstream as `FUND_ETH` and which therefore overrides
an ambient `FUND_ETH`. Upstream's own default is 0.003 ETH. The private-network
allowance covers multiple deployments and quoted triggers, including elevated
ordinary fee suggestions after the state-chaining test's 100 gwei source bids.
Override it for a custom workload. Funding transfers are not measured settlement
fees or production pricing assumptions.

The wrapper honors the pinned upstream `E2E_EXCLUDE_FROM_ALL` marker when
expanding the default `all` target. The current pin discovers 64 scenarios and
marks 30 as excluded: 22 call-pattern probes, four static reads around local
writes, two missing-callback-proxy static round trips and two static reads
followed by nested writes (`staticThenNestedWrites` / `staticThenNestedWritesL2`).
The default live suite therefore contains 34 scenarios. The summary lists all
discovered scenarios, the live eligible count, and the excluded names;
`suite.complete` means every eligible scenario was selected. Explicit names
can still run excluded scenarios during development, but those runs do not
establish a complete default suite.

`EEZ_PROTOCOL_SUITE_TIMEOUT` bounds all upstream phases together (default 2400 seconds);
a timeout stops its process group and fails the suite. Do not run another
orchestrator against the same enclave concurrently. The wrapper rejects
concurrent invocations in the same checkout; each CI job must own its enclave.

`protocol-e2e/summary.json` records `execution.phases` with scenario lists, exit
codes and elapsed seconds. Logs live in a fresh `protocol-e2e/network.*` directory
per invocation, with raw phase output and normalized per-scenario logs; wallet
CSV files and private keys are not copied into artifacts. Compare protocol-phase
duration and full Kurtosis time at different worker caps, requiring complete
passing evidence in each run. Failed or missing scenario results also print a
bounded, prefixed log tail to the job console so artifact quota failures do not
hide the scenario error.

Parallel scenarios can share settlement and delivery blocks. The collector
therefore selects table transactions by runtime entry hashes recomputed from
canonical L2 calldata, not the upstream summary's first load/all-calls hints.
PR #56 summaries distinguish `L2 system table load` from
`L2 delivery (load+execute)` and can list several `L1 postBatch` transactions.
The collector accepts those discovery hints and independently validates the
canonical transactions; summary roles alone do not establish correct execution.
Missing or duplicate occurrences fail, and full selected receipts/calldata
still go through the causal verifier. Job-local `*.causal-input.json` files
allow those checks to be reproduced offline from retained failure diagnostics.

Multi-transaction L1 scenarios may settle in several blocks. The collector
retains every upstream matched settlement in canonical order, and the causal
verifier pairs deliveries across those batches while preserving each batch's
entry indices. Each trigger must follow its selected settlement in the same L1
block, and its raw bytes must occur in that settlement's authenticated blob.
For split scenarios, `blob_da.settlements` retains the per-batch verification
results and separate sidecar files; `blob_da.verified` requires every batch to
pass. A single-settlement scenario keeps the existing evidence shape.

For every passing scenario, the evidence collector re-reads canonical user,
settlement, and L2 delivery receipts; verifies the causal execution tables;
downloads the exact Beacon blob responses; recomputes their KZG versioned hashes;
decodes the native cross-chain semantics; and requires each trigger's raw
signed transaction bytes to occur in semantic `tx_data`. The command succeeds
only when suite accounting, receipt evidence, causal coverage, and authenticated
semantic-blob coverage are all complete. `protocol-e2e/summary.json` is the
machine-readable index, and each scenario retains its decoded evidence and raw
Beacon blob response under `protocol-e2e/evidence/`.

The generic collector also reconciles the pinned runner's `submission refused`
diagnostics for `reconciliation`, `quote_busy` and `stale_context`: the exact logged
hash must be one of the scenario's successfully included canonical triggers, with
matching chain ID and nonce. All table, causal, blob and final canonical-recheck
gates still apply. The original
log is retained, and `source_log.reconciled_submissions` records these recovered
submissions. Other `ERROR:` lines still fail collection. The specialized lookup
collectors retain their strict rejection of submission errors. This handles a
bounded retry of the same signed operation inside one scenario, not a retry of a
failed test or a reason to ignore missing semantic evidence.

Mixed L2 read/write scenarios bind the complete per-user `loadExecutionTable`
calldata, including every static row and its mutable-entry position, to one L1
host. Their retained mutable-call events must still match exactly. The
`eez-protocol::causal_evidence` test target exercises the shipping verifier
against captured canonical inputs and malformed variants; CI runs it from the
producer's release test archive.

The pinned proxy reads `STATIC_CHECK_GAS()` from the registry before its actual
lookup. Lookup trace evidence requires that exact successful configuration read
and exactly one result-bearing lookup, with the original caller, calldata, and
returned/reverted bytes. Additional registry calls remain a verification error.

Upstream failures are never overridden. `staticCounterL2` is verified through
its static-pool calldata and persisted reader state. `topLevelStaticCounter`
and `revertCounter` are lookup-only: their L1 result is bound to the signed
blob semantics, and the collector actively checks that the lookup key is
absent from L2 manager activity over the authenticated settlement window.
Missing or orphaned L2 blocks fail verification; they do not count as absence.

Shared lookup tables are matched by one exact scenario key, not by total table
size. The L1 lookup verifiers require a canonical call trace proving the exact
proxy/registry call and returned or reverted bytes. Intermediate L2 pins are
reconstructed from transaction-prefix headers, including the position before
its first transaction. V0 operations carry counts and ordinary transaction
bytes, not block headers: the collector binds the calldata's endpoint claims to
canonical L2 blocks and checks their height difference against the V0 count.
A successful L1 posting alone does not establish that every deferred effect
settled.

These CI-only checks require L1 `debug_traceTransaction` and L2
`debug_getRawBlock`, `debug_getRawReceipts` and `debug_chainConfig` on regular
node RPCs (available in the test topology). The augmented
`eez_executionWitnessByBlockHash` method is served only on Composer fronts,
when the Composer's HTTP `debug` namespace is enabled. Set `L2_FRONT` as well
as `L2_RPC` for standalone evidence collection; the suite wrapper supplies both.
All other `eez_*` observations, including settlement correlation, also use the
front. The pinned protocol network runner reads L2 through that front so its
correlation fast path remains available; independent debug evidence uses the
regular node RPC. The augmented method returns the proving feed's witness: Reth's minimal
`debug_executionWitnessByBlockHash` can omit sibling trie nodes that an
intermediate prefix root needs after a deletion, which makes the replay fail
even though the block is valid. The helper runs
Stateless with the native L2 EVM, verifies the full block and receipts, and
reconstructs prefix headers from replayed roots. The empty prefix includes
pre-execution state updates; the parent block's state root cannot replace it.
Chain configuration and canonicality still come from the test-node RPCs, so
this is integration evidence and does not authorize a production proof.
Do not expose debug APIs or debug-enabled Composer fronts publicly for this purpose. Raw trace, witness/prefix
input and selected-root evidence are retained for diagnosis. The helper's
regressions run from the producer's release test archive.

After updating the protocol pin, regenerate the committed genesis artifacts
with `bash scripts/update-eezl2-genesis.sh` and verify them with `--check`.
Solidity metadata changes can also change the proxy CREATE2 vector, even when
the ABI is unchanged. Existing enclaves keep their deployed contracts; use a
fresh, separately named enclave to validate a new pin without removing the devnet.

## Customize the network

### Capture history and cross-chain observation replay

For an owned, fresh enclave, set `eez.history_update_offset: 5` with the existing
two-second, six-slot cadence. This places the authenticated history import at
transaction zero of Sync. The default offset remains unchanged. Set
`eez.prove_capture_image` to an image containing the `eez-testkit` example
`capture_prover`; the package inserts that bounded test adapter between Composer
and the unchanged signer. It retains only acknowledged v2 evidence and successful
proof requests in the adapter's `/data/captures` directory. This unauthenticated
adapter is for private test networks only.

With the integration contracts built, run:

```bash
export KURTOSIS_ENCLAVE=eez-combined-history
export KURTOSIS_ARGS_FILE=/path/to/that-enclaves-args.yaml
export EEZ_COMBINED_CAPTURE_DIR="$PWD/artifacts/combined-history/capture"
bash testing/kurtosis/scripts/capture-combined-history.sh
source testing/kurtosis/ports.sh
export EEZ_COMBINED_L2_RPC="$EEZ_DEVNET_L2_RPC"
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e \
  -- cross_chain::combined_history::kurtosis_combined_history_observations_replay_offline \
  --ignored --exact --nocapture
```

The workload deploys a probe and L1 reader using public fixture accounts. One
source transaction reads the probe statically, records a failed mutable lookup,
delivers a real write, and reads its new value. The replay selects a canonical
capture with an advancing history import, compares the observations with retained
chain state, then uses a separate network-denied signer for the unchanged request
and coherent ABI/blob variants. Earlier-state and post-effect outcomes must fail
at `observation_replay` without a signature. Requests and results remain in
`combined-regression/` for language-neutral replay.

Current Composer policy puts ordinary mempool writes in preceding blocks of the
proved window. The terminal rich Sync contains history and cross-chain effects.
This live test does not claim an ordinary mempool write between those operations
in the same block; the separate signer fixtures cover that more general body.

### Arguments file

`ci-args.yaml` is the included local profile. To modify it without changing the
committed file, copy it and use the copy for both startup and testing:

```bash
export KURTOSIS_ARGS_FILE=/tmp/eez-dev-args.yaml
cp testing/kurtosis/ci-args.yaml "$KURTOSIS_ARGS_FILE"

# Edit $KURTOSIS_ARGS_FILE, then start the network with it.
bash testing/kurtosis/start.sh "$KURTOSIS_ARGS_FILE"
```

The `ethereum_package` section configures the canonical L1 participant, chain,
slot time, prefunded accounts, and MEV stack. The `eez` section configures:

- Node, proof-signer, deployment, and follower images.
- Poster, proof-signer, and L2 system test keys.
- L1 and L2 block timing, proof time, submission slack, and the earliest
  builder-dispatch lead (`bundle_submission_lead_ms`, default `7500`).
- Maximum speculative depth, cross-chain transactions per bundle, and proposer
  fee recipient.
- L1/L2 explorers and their images.

`bundle_submission_lead_ms` is independent of proof time: it is the earliest
wall-clock dispatch point before the selected L1 timestamp. It must be greater
than `submission_slack_ms`, which remains the latest-safe dispatch boundary;
an empty window is rejected at node startup.

Keep the private-network keys distinct, and never reuse them outside a
disposable local environment.

`max_user_txs_per_bundle` accepts a positive integer or the quoted string
`"auto"`. A numeric value limits held user cross-chain transactions, combined
across both directions, offered to one Sync block. The omitted value remains
`50`, matching the composer default. Transactions beyond the selected prefix
remain FIFO-queued; gas and blob capacity can reduce the actual drain.

Setting `max_user_txs_per_bundle: "auto"` selects using measured preparation
time and available gas/blob capacity, without a fixed transaction-count cap.
Configure admission through this existing top-level `eez` option;
`settlement_env` carries the required policy and posting fee options.

The single conditional scheduler requires
`settlement_env.EEZ_SETTLEMENT_START_AFTER_BLOCK: "3"`, not a new top-level
selector. Missing P fails configuration. Set optional
`settlement_env.EEZ_SETTLEMENT_START_DELAY_MS: "250"` for a sub-block delay;
omission means zero and an empty value is invalid. Remove the obsolete
`EEZ_PREPARATION_TIME_MS` input entirely. Explicit strings reach node validation.
Also adjust `proof_time_ms` and `submission_slack_ms`: the packaged 5000/2500 ms
defaults do **not** fit the six seconds remaining after P=3 on the 12s/2s grid.
See [conditional slot production](../../docs/configurable-settlement-policy.md#conditional-slot-production)
for budget validation and the outstanding release gates; this remains draft behavior.

The dedicated `crosschain-limit-args.yaml` profile deliberately sets the cap to
`3` to make spillover observable under a small workload. It is a disposable
validation profile, not the recommended setting for a normal devnet. Start it
under a separate enclave and run its evidence check with:

```bash
KURTOSIS_ENCLAVE=eez-crosschain-limit \
  bash testing/kurtosis/start.sh \
  testing/kurtosis/crosschain-limit-args.yaml

KURTOSIS_ENCLAVE=eez-crosschain-limit \
KURTOSIS_ARGS_FILE=testing/kurtosis/crosschain-limit-args.yaml \
  bash testing/kurtosis/scripts/verify-cross-chain-limit.sh
```

The verifier requires both directions to settle without loss, duplication,
nonce gaps, cap violations, held-transaction evictions, or composition
failures, and writes machine-readable evidence under
`artifacts/kurtosis-crosschain-limit` by default.

### Build or reuse images

`start.sh` reads the service image tags from the selected arguments file and
builds them by default. The UI build context defaults to the sibling
`../eez-rollup-ui` repository; set `EEZ_UI_REPO` when it lives elsewhere. The
default release profile is optimized for a faster development build.

Set `EEZ_OPTIMIZED_BUILD=1` to build the production `maxperf` binaries:

```bash
EEZ_OPTIMIZED_BUILD=1 \
  bash testing/kurtosis/start.sh "$KURTOSIS_ARGS_FILE"
```

To reuse images that already exist locally, put their tags in the arguments
file and skip the corresponding builds:

```bash
EEZ_SKIP_NODE_BUILD=1 \
EEZ_SKIP_PROOF_SIGNER_BUILD=1 \
EEZ_SKIP_DEPLOY_BUILD=1 \
EEZ_SKIP_UI_BUILD=1 \
  bash testing/kurtosis/start.sh "$KURTOSIS_ARGS_FILE"
```

The deployment image is built from the selected node image because it copies
the genesis state-root and block-hash utilities from that image. If the node
changes, rebuild the deployment image as well.

Set `EEZ_PRUNE_BUILD_CACHE=1` to run
`docker builder prune --all --force` after building the images. This deletes
the entire host's unused BuildKit cache, not only cache entries created by this
repository, so enable it deliberately.

### Run multiple local networks

Each network needs a unique enclave name:

```bash
export KURTOSIS_ENCLAVE=eez-dev-2
bash testing/kurtosis/start.sh "$KURTOSIS_ARGS_FILE"
```

Kurtosis assigns different host ports automatically. Remember that
`stop.sh` removes whichever enclave is currently selected by
`KURTOSIS_ENCLAVE`.

## Troubleshooting

- **Relay startup fails with a duplicate database migration key:** the pinned
  Ethereum package starts its housekeeper without a readiness check, allowing
  the API to migrate the same database concurrently. `resources.star` adds a
  bounded check of the housekeeper's loopback `/metrics` endpoint, which the
  pinned relay opens after database migration and beacon readiness. The wait
  follows housekeeper creation and precedes API creation in the plan. Recheck the
  endpoint and in-image `wget` when changing the relay image. Do not disable
  migrations or reuse a partially initialized database to hide this failure.
- **Docker rejects a 3-CPU limit on a 2-CPU runner:** the pinned Ethereum
  package hard-codes this limit for four MEV services. `ci-args.yaml` sets
  `eez.mev_relay_cpu: {min: 100, max: 2000}` (millicpus) to override their CPU
  reservations and limits before creation. Kurtosis resource checks stay on;
  memory, readiness and protocol assertions are unchanged. Profiles without
  this setting retain the upstream defaults. The override requires an explicit
  `ethereum_package.mev_params.mev_relay_image`; recheck the service names and
  Redis image in `resources.star` when updating the Ethereum package pin.
- **A CI scenario fails after earlier scenarios passed:** download the
  `kurtosis-e2e` artifact. The lane captures full service logs with `--all`,
  not Kurtosis's default last 200 lines. Bounded node, signer, builder, beacon,
  validator, MEV-Boost and relay tails are also printed to the job log so an
  artifact storage quota failure does not hide the PBS diagnostics.
- **The submodule is missing or stale:** run
  `git submodule update --init --recursive eez-core-protocol`.
- **The enclave name already exists:** inspect it before deciding whether to
  reuse a different name or remove it with `stop.sh`.
- **A service is not ready:** run `kurtosis enclave inspect` and inspect the
  node, proof-signer, builder, relay, or follower logs.
- **Ports cannot be resolved:** ensure the current shell has the same
  `KURTOSIS_ENCLAVE` value used by `start.sh`. If it is unset, the scripts
  silently target their `eez-ci` fallback.
- **A workload cannot resolve its funding key:** ensure `KURTOSIS_ARGS_FILE`
  points to the arguments file used to start the enclave.
- **EEZL2 deployment verification fails:** inspect `eez-node` and the downloaded
  `deployments.env` before resetting the network.
- **L2 source verification says EEZL2 is not a contract:** recreate the devnet;
  an explorer started by an older package did not import the L2 genesis.
- **Docker runs out of space:** inspect Docker disk usage and remove unused
  resources deliberately before rebuilding. `EEZ_PRUNE_BUILD_CACHE=1` removes
  all unused BuildKit cache from the host after the image builds.

## Developer-facing files

- `main.star` and `kurtosis.yml`: network definition and package manifest.
- `blockscout.star`: prefix-aware L1 and L2 Blockscout service definitions.
- `ci-args.yaml`: default local topology, image selections, timing, and test
  keys.
- `crosschain-limit-args.yaml`: disposable cap-`3` spillover validation profile.
- `l2-genesis-profile.json`: reproducible public inputs and hashes for the
  committed development genesis (`genesis.dev.json`) that the deployment task
  selects with `EEZ_GENESIS_BASE`; it contains no private key.
- `Dockerfile.deploy`: deployment image containing Foundry, contracts, scripts,
  and the genesis state-root/block-hash utilities copied from the selected node image.
- `start.sh` and `stop.sh`: local network lifecycle.
- `ports.sh`: endpoint discovery, summary, and shell exports.
- `scripts/verify-eezl2-deployment.sh`: live EEZL2 deployment verification.
- `scripts/verify-eezl2-blockscout.sh`: L2 Blockscout source verification for
  the genesis-installed EEZL2 contract.
- `scripts/cross-chain-wave.sh`: individual cross-chain workload modes.
- `scripts/verify-cross-chain-limit.sh`: bounded FIFO spillover and loss-safety
  evidence check.
- `scripts/verify-cross-chain-waves.sh`: complete workload suite.
- `scripts/verify-state-chaining.sh`: ordered same-Sync-block state-chaining regression.
