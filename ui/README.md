# Sync Rollups POC UI on the EEZ Kurtosis network

This directory keeps the previous
[`eez-association/sync-rollups-poc`](https://github.com/eez-association/sync-rollups-poc)
UI as its source baseline (main commit `8b8eaeef`). The application structure,
components, tabs, assets, and styling were copied as-is. The changes here are a
network compatibility layer: runtime endpoints, local transaction routing,
current contract selectors and bytecode, and current deployment addresses.

The original screens remain available:

- Dashboard
- Counter Demo and generic cross-chain calls
- Bridge
- Flash Loan, including the original reverse-flow screen
- Aggregator
- Faucet
- Visualizer

## Current protocol compatibility

| POC scenario | Current Kurtosis network |
|---|---|
| Counter deployment and local calls | Supported |
| Counter proxy and L1 -> L2 call | Supported and transaction-tested |
| Generic L1 -> L2 calls | Supported |
| ETH bridge L1 -> L2 | Supported and transaction-tested |
| ETH bridge L2 -> L1 | Supported and transaction-tested |
| Demo ERC-20 bridge | Supported by the deployed demo bridge |
| Nested flash loan L1 -> L2 -> L1 | Supported by the defensive-checks demo contracts |
| Faucet | Supported with the disposable Kurtosis signer |
| Reverse flash loan L2 -> L1 -> L2 | UI preserved; contracts are not present in this protocol branch |
| Cross-chain aggregator | UI preserved; contracts are not present in this protocol branch |
| Legacy execution-table visualizer | UI preserved; its old batch/event decoder needs a separate port to `postAndVerifyBatch` |

## Run against an existing Kurtosis network

New networks started with `testing/kurtosis/ci-args.yaml` deploy the interactive
demo contracts automatically. From the repository root:

```bash
export KURTOSIS_ENCLAVE=eez-protocol-dev
bash ui/scripts/configure-kurtosis.sh
cd ui
npm ci
npm run dev
```

Open <http://127.0.0.1:8080>. Vite serves the copied POC and proxies its L1,
L2, and composer requests to the enclave's dynamically allocated ports.

When the UI is launched as part of the Kurtosis package, nginx provides the
same proxy routes and generates `config.json` from the protocol and demo
deployment artifacts.

If the enclave predates `enable_demo_apps`, deploy the interactive contracts
once before generating the UI configuration:

```bash
export KURTOSIS_ENCLAVE=eez-protocol-dev
source testing/kurtosis/ports.sh

EEZ_UI_DEPLOYMENTS_DIR="$(mktemp -d)"
kurtosis files download \
  "$KURTOSIS_ENCLAVE" eez-deployments "$EEZ_UI_DEPLOYMENTS_DIR"

EEZ_L1_RPC_URL="$EEZ_DEVNET_L1_RPC" \
EEZ_L2_RPC_URL="$EEZ_DEVNET_L2_RPC" \
EEZ_DEPLOYMENTS_FILE="$EEZ_UI_DEPLOYMENTS_DIR/deployments.env" \
  bash scripts/deploy-ui-demo.sh

bash ui/scripts/configure-kurtosis.sh
```

The fallback deployment is written to `ui/.runtime/demo.env`, which is ignored
by Git and picked up by `configure-kurtosis.sh`.

## Wallet and local fallback signer

Rabby or MetaMask is the primary signer when an injected wallet is present. The
UI registers the L1 and L2 cross-chain fronts as their chain RPC URLs; each front
forwards ordinary transactions and holds cross-chain transactions for
composition, so the wallet needs only one URL for each chain.

Browsers without an injected wallet fall back to a deterministic, prefunded
development account. Its private key is intentionally delivered in the local
runtime configuration. Never use this configuration, account, or key on a public
network or with real funds.
