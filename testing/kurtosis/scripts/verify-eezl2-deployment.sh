#!/usr/bin/env bash
# Verify the live EEZL2 predeploy against the public deployment bindings.

set -euo pipefail

K="$(cd "$(dirname "$0")/.." && pwd)"
REPO="$(cd "$K/../.." && pwd)"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-ci}"
source "$K/scripts/lib.sh"
PROXY_VECTOR_ORIGINAL_ADDRESS="0x11223344556677889900aabbccddeeff00112233"
PROXY_VECTOR_ORIGINAL_ROLLUP_ID=0
EXPECTED_PROXY_ADDRESS="0xc789b12d9aa4a9f0f0130346d80f749be743ebde"

for tool in cast jq kurtosis; do
    command -v "$tool" >/dev/null || { echo "$tool not in PATH" >&2; exit 1; }
done

http_url() {
    case "$1" in
        http://*|https://*) printf '%s\n' "$1" ;;
        "") printf '\n' ;;
        *) printf 'http://%s\n' "$1" ;;
    esac
}

L2="${L2:-$(http_url "$(kurtosis port print "$ENCLAVE" eez-node l2-rpc 2>/dev/null || true)")}"
[[ -n "$L2" ]] || { echo "could not resolve the enclave L2 RPC" >&2; exit 1; }

deploy_dir="${TMPDIR:-/tmp}/eez-deployment-check-$ENCLAVE"
rm -rf "$deploy_dir"
mkdir -p "$deploy_dir"
trap 'rm -rf "$deploy_dir"' EXIT
load_deployment_artifact "$deploy_dir"

set -a
# shellcheck disable=SC1091
source "$deploy_dir/deployments.env"
set +a

for name in \
    EEZL2_ADDRESS \
    EEZ_L2_SYSTEM_ADDRESS \
    EEZ_L2_EEZL2_CODE_HASH \
    EEZ_ROLLUP_ID \
    EEZ_INITIAL_BLOCK_HASH \
    EEZ_L2_GENESIS_STATE_ROOT
do
    [[ -n "${!name:-}" ]] || { echo "$name missing from deployments.env" >&2; exit 1; }
done

actual_system="$(cast call "$EEZL2_ADDRESS" 'SYSTEM_ADDRESS()(address)' --rpc-url "$L2")"
actual_rollup_id="$(cast call "$EEZL2_ADDRESS" 'ROLLUP_ID()(uint64)' --rpc-url "$L2" | awk '{print $1}')"
actual_use_gas_left="$(cast call "$EEZL2_ADDRESS" 'USE_GAS_LEFT()(bool)' --rpc-url "$L2")"
runtime="$(cast code "$EEZL2_ADDRESS" --rpc-url "$L2")"
actual_runtime_hash="$(cast keccak "$runtime")"
actual_state_root="$(cast block 0 --rpc-url "$L2" --json | jq -er '.stateRoot')"
actual_block_hash="$(cast block 0 --rpc-url "$L2" --json | jq -er '.hash')"
actual_proxy_address="$(cast call \
    "$EEZL2_ADDRESS" \
    'computeCrossChainProxyAddress(address,uint64)(address)' \
    "$PROXY_VECTOR_ORIGINAL_ADDRESS" \
    "$PROXY_VECTOR_ORIGINAL_ROLLUP_ID" \
    --rpc-url "$L2")"

[[ "${actual_system,,}" == "${EEZ_L2_SYSTEM_ADDRESS,,}" ]] || {
    echo "live EEZL2 SYSTEM_ADDRESS mismatch: $actual_system != $EEZ_L2_SYSTEM_ADDRESS" >&2
    exit 1
}
[[ "$actual_rollup_id" == "$EEZ_ROLLUP_ID" ]] || {
    echo "live EEZL2 ROLLUP_ID mismatch: $actual_rollup_id != $EEZ_ROLLUP_ID" >&2
    exit 1
}
[[ "$actual_use_gas_left" == "false" ]] || {
    echo "live EEZL2 USE_GAS_LEFT is $actual_use_gas_left; expected false" >&2
    exit 1
}
[[ "${actual_runtime_hash,,}" == "${EEZ_L2_EEZL2_CODE_HASH,,}" ]] || {
    echo "live EEZL2 runtime hash mismatch: $actual_runtime_hash != $EEZ_L2_EEZL2_CODE_HASH" >&2
    exit 1
}
[[ "${actual_state_root,,}" == "${EEZ_L2_GENESIS_STATE_ROOT,,}" ]] || {
    echo "live L2 genesis state root mismatch: $actual_state_root != $EEZ_L2_GENESIS_STATE_ROOT" >&2
    exit 1
}
[[ "${actual_block_hash,,}" == "${EEZ_INITIAL_BLOCK_HASH,,}" ]] || {
    echo "live L2 genesis block hash mismatch: $actual_block_hash != $EEZ_INITIAL_BLOCK_HASH" >&2
    exit 1
}
[[ "${actual_proxy_address,,}" == "${EXPECTED_PROXY_ADDRESS,,}" ]] || {
    echo "live EEZL2 proxy derivation mismatch: $actual_proxy_address != $EXPECTED_PROXY_ADDRESS" >&2
    exit 1
}

echo "EEZL2 deployment bindings and proxy derivation verified (rollupId=$EEZ_ROLLUP_ID, system=$EEZ_L2_SYSTEM_ADDRESS)"
