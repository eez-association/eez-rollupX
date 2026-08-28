#!/usr/bin/env bash
# Run supported eez-core-protocol scenarios against the CI enclave.
# The full scenario suite runs separately against Anvil in the normal CI.
set -euo pipefail

KURTOSIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$KURTOSIS_DIR/../.." && pwd)"
PROTOCOL="$REPO/eez-core-protocol"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-ci}"
RESULT_DIR="${EEZ_CI_RESULT_DIR:-$REPO/artifacts/kurtosis-e2e}/protocol-e2e"

SUPPORTED_TARGETS=(
    one_way                    # bridge, counter, and counterL2
    multi-call-twice           # one L1 user tx -> the same L2 target twice
    multi-call-two-diff        # one L1 user tx -> two L2 targets
    multi-call-twiceL2         # one L2 user tx -> the same L1 target twice
    multi-call-two-diffL2      # one L2 user tx -> two L1 targets
    counter-multi-tx           # three source transactions in one settlement
    nestedCounter              # L1 -> L2 -> L1
    nestedCounterL2            # L2 -> L1 -> L2
    multi-call-nested          # three nested L1 -> L2 -> L1 flows
    multi-call-nestedL2        # one L2 user tx -> two L1 roots -> two L2 callbacks
    deepNested                 # multi-level alternating callback chain
    flash-loan                 # stateful L1 -> L2 -> L1 callback flow
    nestedCallRevert           # failed L2 -> L1 callback caught by its L2 caller
    nestedCallRevertL2         # failed L1 -> L2 callback caught by its L1 caller
    revertCounter              # single reverted callback (L1 -> L2 -> L1)
    revertFromOtherChain       # remote-frame revert followed by a committed retry
    revertCounterL2            # single reverted callback (L2 -> L1 -> L2)
    revertFromOtherChainL2     # remote L1 frame reverts an L2-origin call
    revertFromOtherChainAndCallAgainL2 # remote revert followed by a committed retry
    revertFromOtherChainNested # nested remote revert followed by a committed call
    reentrant                  # four-boundary L1 -> L2 -> L1 -> L2 -> L1 chain
)

UNSUPPORTED_TARGETS=()

for tool in bash bc cast forge git jq kurtosis; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

expected_protocol="$(git -C "$REPO" ls-files -s eez-core-protocol | awk '{print $2}')"
actual_protocol="$(git -C "$PROTOCOL" rev-parse HEAD)"
if [[ -z "$expected_protocol" || "$actual_protocol" != "$expected_protocol" ]]; then
    echo "eez-core-protocol is not at the commit pinned by this checkout" >&2
    echo "expected: ${expected_protocol:-missing}; found: $actual_protocol" >&2
    echo "Run: git submodule update --init --recursive eez-core-protocol" >&2
    exit 1
fi

source "$KURTOSIS_DIR/ports.sh" >/dev/null
L1_RPC="${L1_RPC:-$EEZ_DEVNET_L1_RPC}"
L2_RPC="${L2_RPC:-$EEZ_DEVNET_L2_RPC}"
L1_FRONT="${L1_FRONT:-$EEZ_DEVNET_L1_FRONT}"
L2_FRONT="${L2_FRONT:-$EEZ_DEVNET_L2_FRONT}"

mkdir -p "$RESULT_DIR"
deploy_dir="$(mktemp -d /tmp/eez-protocol-deployments.XXXXXX)"
trap 'rm -rf "$deploy_dir"' EXIT

kurtosis files download "$ENCLAVE" eez-deployments "$deploy_dir" >/dev/null
set -a
source "$deploy_dir/deployments.env"
set +a

ROLLUPS="${ROLLUPS:-$EEZ_REGISTRY_ADDRESS}"
MANAGER_L2="${MANAGER_L2:-${EEZL2_ADDRESS:-0x4200000000000000000000000000000000000007}}"
ROLLUP_ID="${EEZ_ROLLUP_ID:-1}"

# Hardhat account #2, shared with the earlier cross-chain wave.
PK="${PK:-0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a}"
SENDER="$(cast wallet address --private-key "$PK")"

echo "==> protocol E2E endpoints"
echo "    protocol:     $actual_protocol"
echo "    L1 RPC/front: $L1_RPC / $L1_FRONT"
echo "    L2 RPC/front: $L2_RPC / $L2_FRONT"
echo "    registry:     $ROLLUPS"

wait_for_protocol_ready() {
    local timeout="${EEZ_PROTOCOL_READY_TIMEOUT:-300}"
    local deadline=$((SECONDS + timeout))
    local safe_json safe_number_hex safe_number safe_root rollup_data registry_root

    echo "==> waiting for the first L2 safe root to settle on L1"
    while (( SECONDS < deadline )); do
        if safe_json="$(cast block safe --json --rpc-url "$L2_RPC" 2>/dev/null)" &&
            safe_number_hex="$(jq -er '.number' <<<"$safe_json" 2>/dev/null)" &&
            safe_number="$(cast to-dec "$safe_number_hex" 2>/dev/null)" &&
            safe_root="$(jq -er '.stateRoot' <<<"$safe_json" 2>/dev/null)" &&
            rollup_data="$(cast call "$ROLLUPS" \
                'rollups(uint64)(address,bytes32,uint256)' "$ROLLUP_ID" \
                --rpc-url "$L1_RPC" 2>/dev/null)"; then
            registry_root="$(sed -n '2p' <<<"$rollup_data" | tr -d '[:space:]')"
            if [[ "$safe_number" =~ ^[0-9]+$ ]] &&
                (( 10#$safe_number > 0 )) &&
                [[ "${registry_root,,}" == "${safe_root,,}" ]]; then
                echo "    ready: safe L2 block $safe_number, root $safe_root"
                return 0
            fi
        fi
        sleep 3
    done

    echo "protocol network did not become settlement-ready within ${timeout}s" >&2
    echo "last safe block/root: ${safe_number:-unavailable} / ${safe_root:-unavailable}" >&2
    echo "last registry root: ${registry_root:-unavailable}" >&2
    return 1
}

wait_for_protocol_ready

(
    cd "$PROTOCOL"
    bash script/e2e/run/prepare-network.sh \
        --l1-rpc "$L1_RPC" \
        --l1-front "$L1_FRONT" \
        --l2-rpc "$L2_RPC" \
        --pk "$PK" \
        --rollups "$ROLLUPS"
)

# Independently enforce the balance required by the scenario deployments.
min_l2_balance=10000000000000000
l2_balance="$(cast balance "$SENDER" --rpc-url "$L2_RPC")"
if (( $(echo "$l2_balance < $min_l2_balance" | bc) )); then
    echo "L2 balance for $SENDER is $l2_balance wei, below the $min_l2_balance wei minimum" >&2
    exit 1
fi

env_file="$deploy_dir/chain.env"
cat >"$env_file" <<ENV
L1_RPC=$L1_RPC
L1_FRONT=$L1_FRONT
L2_RPC=$L2_RPC
L2_FRONT=$L2_FRONT
ROLLUPS=$ROLLUPS
MANAGER_L2=$MANAGER_L2
PK=$PK
ENV

if [[ -n "${EEZ_PROTOCOL_SCENARIOS:-}" ]]; then
    read -r -a targets <<<"$EEZ_PROTOCOL_SCENARIOS"
else
    targets=("${SUPPORTED_TARGETS[@]}")
fi

run_log="$RESULT_DIR/network-sequential.log"
rm -rf "$PROTOCOL/tmp/e2e-network"
status=0
(
    cd "$PROTOCOL"
    DEVNET_ENV="$env_file" \
        bash script/e2e/run/network-sequential.sh "${targets[@]}"
) >"$run_log" 2>&1 || status=$?
cat "$run_log"
if [[ -d "$PROTOCOL/tmp/e2e-network" ]]; then
    cp -R "$PROTOCOL/tmp/e2e-network/." "$RESULT_DIR/"
fi

passed=()
failed=()
skipped=()
while read -r name verdict; do
    case "$verdict" in
        PASS) passed+=("$name") ;;
        FAIL) failed+=("$name") ;;
        SKIP) skipped+=("$name") ;;
    esac
done < <(sed -n 's/^RESULT \([^:]*\): \([A-Z]*\).*/\1 \2/p' "$run_log")

json_array() {
    jq -cn --args '$ARGS.positional' "$@"
}

jq -n \
    --arg protocol_commit "$actual_protocol" \
    --argjson passed "$(json_array ${passed[@]+"${passed[@]}"})" \
    --argjson failed "$(json_array ${failed[@]+"${failed[@]}"})" \
    --argjson skipped "$(json_array ${skipped[@]+"${skipped[@]}"})" \
    --argjson targets "$(json_array ${targets[@]+"${targets[@]}"})" \
    --argjson unsupported "$(json_array "${UNSUPPORTED_TARGETS[@]}")" \
    '{protocol_commit: $protocol_commit, targets: $targets, passed: $passed,
      failed: $failed, skipped: $skipped, unsupported: $unsupported}' \
    >"$RESULT_DIR/summary.json"

echo "==> protocol E2E result: ${#passed[@]} passed, ${#failed[@]} failed, ${#skipped[@]} skipped"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
        echo "### Protocol network E2E"
        echo
        echo "- Targets run: ${targets[*]}"
        echo "- Passed: ${#passed[@]}"
        echo "- Failed: ${#failed[@]}"
        echo "- Skipped (local-only): ${#skipped[@]}"
        echo "- Unsupported on this node: ${UNSUPPORTED_TARGETS[*]}"
    } >>"$GITHUB_STEP_SUMMARY"
fi

if (( status != 0 && ${#failed[@]} == 0 )); then
    echo "network-sequential.sh exited $status without reporting a failed scenario" >&2
    exit "$status"
fi
(( ${#failed[@]} == 0 ))
