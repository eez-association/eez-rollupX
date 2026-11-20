#!/usr/bin/env bash
# Rollup0 #155 service outages on the single-signer, real-PBS Kurtosis fixture.
set -euo pipefail
K="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$K/../.." && pwd)"
ENCLAVE="${KURTOSIS_ENCLAVE:?select the disposable Kurtosis enclave}"
[[ "${EEZ_USE_LOCAL_DEPLOYMENTS:-0}" == 0 ]] || { echo "requires an enclave deployment" >&2; exit 1; }
result="${EEZ_CI_RESULT_DIR:-$REPO/artifacts/kurtosis-e2e}/checks/fault-recovery"
mkdir -p "$result"
scratch=$(mktemp -d)
stopped=""
cleanup() {
    if [[ -n "$stopped" ]]; then kurtosis service start "$ENCLAVE" "$stopped" >&2 || true; fi
    rm -rf "$scratch"
}
trap cleanup EXIT
source "$K/scripts/lib.sh"
refresh_ports() {
    source "$K/ports.sh" >/dev/null
    L1="$EEZ_DEVNET_L1_RPC"
    L2="$EEZ_DEVNET_L2_RPC"
    L1F="$EEZ_DEVNET_L1_FRONT"
}
refresh_ports
kurtosis files download "$ENCLAVE" eez-deployments "$scratch" >/dev/null
source "$scratch/deployments.env"
deploy_key="${EEZ_L2_SETUP_KEY:-0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a}"
funding_key="${EEZ_FUND_FROM_KEY:-$deploy_key}"
user_key="0x$(openssl rand -hex 32)"
sender=$(cast wallet address --private-key "$user_key")
fund "$L1" "$funding_key" "$sender"
target=$(forge_deploy "$L2" "$deploy_key" DeployValueL2.s.sol:DeployValueL2 'run(uint256)' 0 | grab_address EEZ_VALUE_ADDRESS)
proxy=$(forge_deploy "$L1" "${EEZ_L1_SETUP_KEY:-$funding_key}" CreateValueProxy.s.sol:CreateValueProxy \
    'run(address,address,uint64)' "$EEZ_REGISTRY_ADDRESS" "$target" "$EEZ_ROLLUP_ID" | grab_address EEZ_VALUE_PROXY)
chain=$(cast chain-id --rpc-url "$L1")

prepare_source() {
    local value="$1" nonce
    nonce=$(cast nonce "$sender" --rpc-url "$L1")
    raw=$(cast mktx --chain-id "$chain" --private-key "$user_key" --nonce "$nonce" \
        --gas-limit 700000 --gas-price "$(gas_price_for "$L1")" --priority-gas-price "$PRIORITY_GAS_PRICE" \
        "$proxy" 'setValue(uint256)' "$value")
    if fee_collection_enabled in; then
        raw=$(sign_fee_quoted "$L1F" "$L1" "$user_key" "$raw" "$result/quote-$value.json")
    fi
    hash=$(cast keccak "$raw")
}

# Pin the destination read to the block whose hash matches the registry.
wait_committed_value() {
    local value="$1" deadline=$((SECONDS + 300)) safe commitment
    while (( SECONDS < deadline )); do
        safe=$(cast block safe --rpc-url "$L2" --json)
        safe_hash=$(jq -er .hash <<<"$safe")
        safe_height=$(cast to-dec "$(jq -er .number <<<"$safe")")
        commitment=$(cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' \
            "$EEZ_ROLLUP_ID" --rpc-url "$L1" | sed -n '2p')
        if [[ "${commitment,,}" == "${safe_hash,,}" ]] &&
            [[ "$(cast call "$target" 'value()(uint256)' --block "$safe_hash" --rpc-url "$L2" 2>/dev/null)" == "$value" ]]; then
            return 0
        fi
        sleep 2
    done
    echo "registry commitment and safe destination value $value did not converge" >&2
    return 1
}

verify_recovery() {
    local value="$1" deadline=$((SECONDS + 300)) status
    while (( SECONDS < deadline )); do
        status=$(cast rpc --rpc-url "$L1F" eez_getCrossChainTransaction "$hash")
        if jq -e --arg h "$hash" '.hash == $h and .ownership.reason == "Committed" and
            .transactions.complete and .transactions.l1.hash == $h' <<<"$status" >/dev/null; then
            [[ "$(receipt_status "$hash" "$L1")" == 1 ]] || return 1
            wait_committed_value "$value" || return 1
            jq --arg block "$safe_hash" --argjson height "$safe_height" \
                '{hash, ownership, transactions, safe: {hash: $block, height: $height}}' \
                <<<"$status" | tee "$result/settled-$value.json"
            return 0
        fi
        sleep 2
    done
    echo "source $hash did not reach committed ownership (last observation: $status)" >&2
    return 1
}

wait_committed_value 0
previous=0
value=41
for service in eez-proof-signer mev-relay-api "${KURTOSIS_BUILDER_SERVICE:-el-2-reth-builder-lighthouse}"; do
    prepare_source "$value"
    echo "==> stopping $service and admitting $hash"
    kurtosis service stop "$ENCLAVE" "$service"
    stopped="$service"
    send_front "$L1F" "$raw" "$hash"
    start=$(cast block-number --rpc-url "$L1")
    deadline=$((SECONDS + 300))
    until (( $(cast block-number --rpc-url "$L1") >= start + 2 )); do
        (( SECONDS < deadline )) || { echo "canonical L1 stalled with $service down" >&2; exit 1; }
        sleep 2
    done
    rpc_receipt=$(receipt_json "$hash" "$L1")
    jq -e 'has("result") and .result == null and .error == null' <<<"$rpc_receipt" >/dev/null
    [[ "$(cast call "$target" 'value()(uint256)' --block safe --rpc-url "$L2")" == "$previous" ]]
    cast rpc --rpc-url "$L1F" eez_getCrossChainTransaction "$hash" >"$result/outage-$value.json"
    jq -e --arg h "$hash" '.hash == $h and
        (.lifecycle == "queued" or .lifecycle == "in_flight" or .lifecycle == "awaiting_evidence"
         or .lifecycle == "composing")' "$result/outage-$value.json" >/dev/null
    kurtosis service start "$ENCLAVE" "$service"
    stopped=""
    verify_recovery "$value"
    previous=$value
    (( ++value ))
done

# Reuse the fork's clean restart and bidirectional settlement-correlation checks.
before_height=$safe_height
before_hash=$safe_hash
(
    unset L1_RPC L2_RPC L2_FRONT EEZ_PUBLIC_L2_RPC ROLLUPS
    EEZ_RESTART_RECOVERY_RESULT_DIR="$result/restart" bash "$K/scripts/verify-node-restart-recovery.sh"
)
refresh_ports
replayed=$(cast block "$before_height" --field hash --rpc-url "$L2")
[[ "${replayed,,}" == "${before_hash,,}" ]]
prepare_source 47
send_front "$L1F" "$raw" "$hash"
verify_recovery 47
(( safe_height > before_height ))
