#!/usr/bin/env bash
# Generate the local Vite runtime config for a running EEZ Kurtosis enclave.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-protocol-dev}"
RUNTIME_DIR="$HERE/.runtime"

for tool in git jq kurtosis; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

http_url() {
    case "$1" in
        http://*|https://*) printf '%s\n' "$1" ;;
        "") printf '\n' ;;
        *) printf 'http://%s\n' "$1" ;;
    esac
}

tmp_dir="$(mktemp -d -t eez-ui-config-XXXXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT
mkdir -p "$RUNTIME_DIR"

kurtosis files download "$ENCLAVE" eez-deployments "$tmp_dir" >/dev/null
set -a
# shellcheck disable=SC1091
source "$tmp_dir/deployments.env"
set +a

demo_env=""
mkdir -p "$tmp_dir/demo"
if kurtosis files download "$ENCLAVE" eez-ui-demo "$tmp_dir/demo" >/dev/null 2>&1; then
    demo_env="$tmp_dir/demo/demo.env"
elif [[ -f "$RUNTIME_DIR/demo.env" ]]; then
    demo_env="$RUNTIME_DIR/demo.env"
fi

if [[ -n "$demo_env" && -f "$demo_env" ]]; then
    set -a
    # shellcheck disable=SC1090
    source "$demo_env"
    set +a
fi

EEZ_UI_DEMO_ENABLED="${EEZ_UI_DEMO_ENABLED:-false}"
EEZ_UI_DEMO_ACCOUNT_ADDRESS="${EEZ_UI_DEMO_ACCOUNT_ADDRESS:-}"
EEZ_UI_DEMO_PRIVATE_KEY="${EEZ_UI_DEMO_PRIVATE_KEY:-}"
if [[ "$EEZ_UI_DEMO_ENABLED" == "true" && -z "$EEZ_UI_DEMO_PRIVATE_KEY" ]]; then
    EEZ_UI_DEMO_PRIVATE_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
fi
EEZ_UI_DEMO_BRIDGE_ADDRESS="${EEZ_UI_DEMO_BRIDGE_ADDRESS:-}"
EEZ_UI_DEMO_TOKEN_ADDRESS="${EEZ_UI_DEMO_TOKEN_ADDRESS:-}"
EEZ_UI_DEMO_POOL_ADDRESS="${EEZ_UI_DEMO_POOL_ADDRESS:-}"
EEZ_UI_DEMO_EXECUTOR_L1="${EEZ_UI_DEMO_EXECUTOR_L1:-}"
EEZ_UI_DEMO_EXECUTOR_L2="${EEZ_UI_DEMO_EXECUTOR_L2:-}"
EEZ_UI_DEMO_WRAPPED_TOKEN_L2="${EEZ_UI_DEMO_WRAPPED_TOKEN_L2:-}"
EEZ_UI_DEMO_NFT_L2="${EEZ_UI_DEMO_NFT_L2:-}"

l1_rpc="$(http_url "$(kurtosis port print "$ENCLAVE" el-1-reth-lighthouse rpc)")"
l2_rpc="$(http_url "$(kurtosis port print "$ENCLAVE" eez-node l2-rpc)")"
l1_front="$(http_url "$(kurtosis port print "$ENCLAVE" eez-node l1-xchain)")"
l2_front="$(http_url "$(kurtosis port print "$ENCLAVE" eez-node l2-xchain)")"
l1_explorer="$(http_url "$(kurtosis port print "$ENCLAVE" l1-blockscout-frontend http 2>/dev/null || true)")"
l2_explorer="$(http_url "$(kurtosis port print "$ENCLAVE" l2-blockscout-frontend http 2>/dev/null || true)")"
l2_explorer_api="$(http_url "$(kurtosis port print "$ENCLAVE" l2-blockscout http 2>/dev/null || true)")"
protocol_commit="$(git -C "$REPO/eez-core-protocol" rev-parse --short HEAD)"

jq -n \
    --arg enclave "$ENCLAVE" \
    --arg protocolCommit "$protocol_commit" \
    --arg l1Rpc "$l1_rpc" \
    --arg l2Rpc "$l2_rpc" \
    --arg l1Front "$l1_front" \
    --arg l2Front "$l2_front" \
    --arg l1Explorer "$l1_explorer" \
    --arg l2Explorer "$l2_explorer" \
    --arg l2ExplorerApi "$l2_explorer_api" \
    --arg registry "$EEZ_REGISTRY_ADDRESS" \
    --arg eezl2 "$EEZL2_ADDRESS" \
    --arg proofSystem "$EEZ_ECDSA_PROOF_SYSTEM_ADDRESS" \
    --arg rollupManager "$EEZ_ROLLUP_MANAGER_ADDRESS" \
    --arg bridgeL1 "$EEZ_L1_BRIDGE_SENDER" \
    --arg bridgeL2 "$EEZ_L2_BRIDGE_RECEIVER" \
    --arg rollupId "$EEZ_ROLLUP_ID" \
    --argjson demoEnabled "$EEZ_UI_DEMO_ENABLED" \
    --arg demoAccount "$EEZ_UI_DEMO_ACCOUNT_ADDRESS" \
    --arg demoPrivateKey "$EEZ_UI_DEMO_PRIVATE_KEY" \
    --arg demoBridge "$EEZ_UI_DEMO_BRIDGE_ADDRESS" \
    --arg demoToken "$EEZ_UI_DEMO_TOKEN_ADDRESS" \
    --arg demoPool "$EEZ_UI_DEMO_POOL_ADDRESS" \
    --arg demoExecutorL1 "$EEZ_UI_DEMO_EXECUTOR_L1" \
    --arg demoExecutorL2 "$EEZ_UI_DEMO_EXECUTOR_L2" \
    --arg demoWrappedTokenL2 "$EEZ_UI_DEMO_WRAPPED_TOKEN_L2" \
    --arg demoNftL2 "$EEZ_UI_DEMO_NFT_L2" \
    '{
      browser: {
        networkName: "EEZ Kurtosis devnet",
        enclave: $enclave,
        l1RpcUrl: "/rpc/l1",
        l2RpcUrl: "/rpc/l2",
        l1FrontUrl: "/composer/l1",
        l2FrontUrl: "/composer/l2",
        l1ExplorerUrl: $l1Explorer,
        l2ExplorerUrl: $l2Explorer,
        l2ExplorerApiUrl: $l2ExplorerApi,
        l1ContractAddress: $registry,
        l2ContractAddress: $eezl2,
        proofSystemAddress: $proofSystem,
        rollupManagerAddress: $rollupManager,
        bridgeL1Address: $bridgeL1,
        bridgeL2Address: $bridgeL2,
        rollupId: $rollupId,
        protocolCommit: $protocolCommit,
        demoEnabled: $demoEnabled,
        demoAccountAddress: $demoAccount,
        demoPrivateKey: $demoPrivateKey,
        demoBridgeAddress: $demoBridge,
        demoTokenAddress: $demoToken,
        demoPoolAddress: $demoPool,
        demoExecutorL1: $demoExecutorL1,
        demoExecutorL2: $demoExecutorL2,
        demoWrappedTokenL2: $demoWrappedTokenL2,
        demoNftL2: $demoNftL2
      },
      proxy: {
        l1Rpc: $l1Rpc,
        l2Rpc: $l2Rpc,
        l1Front: $l1Front,
        l2Front: $l2Front
      }
    }' > "$RUNTIME_DIR/config.json"

printf 'EEZ UI configured for enclave %s\n' "$ENCLAVE"
printf 'Run: cd %s && npm run dev\n' "$HERE"
