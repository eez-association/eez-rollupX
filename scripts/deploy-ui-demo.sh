#!/usr/bin/env bash
# Deploy the interactive UI's bridge and flash-loan demo contracts on both chains.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
PROTOCOL="$REPO/eez-core-protocol"
L1_RPC="${EEZ_L1_RPC_URL:?set EEZ_L1_RPC_URL}"
L2_RPC="${EEZ_L2_RPC_URL:?set EEZ_L2_RPC_URL}"
DEMO_KEY="${EEZ_UI_DEMO_PRIVATE_KEY:-0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a}"
OUTPUT="${EEZ_UI_DEMO_OUT:-$REPO/.runtime/ui-demo.env}"

test -f "${EEZ_DEPLOYMENTS_FILE:-}" && {
    set -a
    # shellcheck disable=SC1090
    source "$EEZ_DEPLOYMENTS_FILE"
    set +a
}

ROLLUPS="${ROLLUPS:-${EEZ_REGISTRY_ADDRESS:?set ROLLUPS or EEZ_REGISTRY_ADDRESS}}"
MANAGER_L2="${MANAGER_L2:-${EEZL2_ADDRESS:-0x4b2760f84b83d6b3829e30a7573b0172b6d6402c}}"

for tool in cast forge; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

mkdir -p "$(dirname "$OUTPUT")"

(
    cd "$PROTOCOL"
    # shellcheck disable=SC1091
    source script/e2e/lib/E2EBase.sh

    ensure_create2_factory "$L1_RPC" L1 "$DEMO_KEY"
    ensure_create2_factory "$L2_RPC" L2 "$DEMO_KEY"

    export ROLLUPS MANAGER_L2
    deploy_contracts \
        script/e2e/scenarios/nested/L1_to_L2/flash-loan/E2EFlashLoan.s.sol \
        "$L1_RPC" "$L2_RPC" "$DEMO_KEY"

    demo_account="$(cast wallet address --private-key "$DEMO_KEY")"
    umask 077
    {
        echo "EEZ_UI_DEMO_ENABLED=true"
        echo "EEZ_UI_DEMO_ACCOUNT_ADDRESS=$demo_account"
        # The generic UI bridge expects the current periphery Bridge ABI
        # (`manager`, `bridgeEther`, and `bridgeTokens`). The pinned scenario
        # deploys and configures each chain's bridge separately; their addresses
        # can differ when the fixture account has different chain nonces.
        echo "EEZ_L1_BRIDGE_SENDER=$BRIDGE_L1"
        echo "EEZ_L2_BRIDGE_RECEIVER=$BRIDGE_L2"
        echo "EEZ_UI_DEMO_BRIDGE_ADDRESS=$BRIDGE_L1"
        echo "EEZ_UI_DEMO_TOKEN_ADDRESS=$TOKEN"
        echo "EEZ_UI_DEMO_POOL_ADDRESS=$FLASH_LOAN_POOL"
        echo "EEZ_UI_DEMO_EXECUTOR_L1=$EXECUTOR_L1"
        echo "EEZ_UI_DEMO_EXECUTOR_L2=$EXECUTOR_L2"
        # The pinned protocol creates the wrapped token on the first bridge
        # call. Deployment exports its deterministic address in advance.
        echo "EEZ_UI_DEMO_WRAPPED_TOKEN_L2=$PREDICTED_WRAPPED_TOKEN_L2"
        echo "EEZ_UI_DEMO_NFT_L2=$FLASH_LOANERS_NFT"
    } >"$OUTPUT"
)

echo "UI demo deployment written to $OUTPUT"
