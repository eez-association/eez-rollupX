#!/usr/bin/env bash
# Verify that settled L2 state and bidirectional L1/L2 correlation survive an
# eez-node service restart. This is deliberately a clean process restart; it
# does not claim coverage for kill -9 or storage-write crash injection.
set -euo pipefail

KURTOSIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$KURTOSIS_DIR/../.." && pwd)"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-devnet}"
ROLLUP_ID="${EEZ_ROLLUP_ID:-1}"
TIMEOUT="${EEZ_RESTART_RECOVERY_TIMEOUT:-240}"
RESULT_ROOT="${EEZ_RESTART_RECOVERY_RESULT_DIR:-$REPO/artifacts/restart-recovery}"

for tool in cast curl jq kurtosis; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done
[[ "$ROLLUP_ID" =~ ^[0-9]+$ ]] || { echo "EEZ_ROLLUP_ID must be a decimal u64" >&2; exit 1; }
[[ "$TIMEOUT" =~ ^[0-9]+$ ]] || { echo "EEZ_RESTART_RECOVERY_TIMEOUT must be seconds" >&2; exit 1; }

export KURTOSIS_ENCLAVE="$ENCLAVE"
L1_RPC_OVERRIDE="${L1_RPC:-}"
L2_RPC_OVERRIDE="${L2_RPC:-}"

# Kurtosis can assign a new host port when a stopped service is started again.
# Preserve explicit caller overrides, but refresh dynamically discovered URLs at
# every lifecycle boundary so the recovery check follows the restarted service.
refresh_dynamic_rpcs() {
    source "$KURTOSIS_DIR/ports.sh" >/dev/null
    L1_RPC="${L1_RPC_OVERRIDE:-$EEZ_DEVNET_L1_RPC}"
    L2_RPC="${L2_RPC_OVERRIDE:-$EEZ_DEVNET_L2_RPC}"
}

refresh_dynamic_rpcs
PUBLIC_L2_RPC="${EEZ_PUBLIC_L2_RPC:-}"

run_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
run_dir="$RESULT_ROOT/$run_stamp"
mkdir -p "$run_dir"
temp_dir="$(mktemp -d /tmp/eez-restart-recovery.XXXXXX)"
node_stopped=false

cleanup() {
    if [[ "$node_stopped" == true ]]; then
        echo "cleanup: restarting eez-node after failed recovery check" >&2
        kurtosis service start "$ENCLAVE" eez-node >/dev/null 2>&1 || true
    fi
    rm -rf "$temp_dir"
}
trap cleanup EXIT

kurtosis files download "$ENCLAVE" eez-deployments "$temp_dir/deployments" >/dev/null
set -a
source "$temp_dir/deployments/deployments.env"
set +a
ROLLUPS="${ROLLUPS:-$EEZ_REGISTRY_ADDRESS}"

rpc_request() {
    local method="$1" params="$2"
    jq -cn --arg method "$method" --argjson params "$params" \
        '{jsonrpc:"2.0",id:1,method:$method,params:$params}' |
        curl -fsS --max-time 10 "$L2_RPC" \
            -H 'content-type: application/json' --data-binary @-
}

settlement_by_l2() {
    rpc_request eez_getSettlementByL2Block "[$(jq -Rn --arg value "$1" '$value')]"
}

ranges_by_l1() {
    rpc_request eez_getSettledL2RangesByL1Block "[$(jq -Rn --arg value "$1" '$value')]"
}

assert_rpc_result() {
    local label="$1" response="$2"
    if [[ "$(jq -r '.error // empty' <<<"$response")" != "" ]] ||
        [[ "$(jq -r '.result == null' <<<"$response")" == true ]]; then
        echo "$label RPC failed: $response" >&2
        exit 1
    fi
}

echo "==> selecting a settled L2 block before restart"
before_safe="$(cast block safe --json --rpc-url "$L2_RPC")"
target_number_hex="$(jq -er '.number' <<<"$before_safe")"
target_number=$((target_number_hex))
target_hash="$(jq -er '.hash | ascii_downcase' <<<"$before_safe")"
(( target_number > 0 )) || { echo "safe L2 block must be above genesis" >&2; exit 1; }

before_l2_to_l1="$(settlement_by_l2 "$target_number_hex")"
assert_rpc_result "pre-restart L2->L1 correlation" "$before_l2_to_l1"
[[ "$(jq -r '.result.canonicalL2' <<<"$before_l2_to_l1")" == true ]] || {
    echo "pre-restart target block is not canonical" >&2
    exit 1
}
matched_hash="$(jq -er '.result.matchedL2Block.hash | ascii_downcase' <<<"$before_l2_to_l1")"
[[ "$matched_hash" == "$target_hash" ]] || {
    echo "pre-restart correlation hash $matched_hash != safe hash $target_hash" >&2
    exit 1
}

l1_tx="$(jq -er '.result.l1TransactionHash | ascii_downcase' <<<"$before_l2_to_l1")"
l1_block_hash="$(jq -er '.result.l1BlockHash | ascii_downcase' <<<"$before_l2_to_l1")"
l1_block_number_hex="$(jq -er '.result.l1BlockNumber' <<<"$before_l2_to_l1")"
before_l1_to_l2="$(ranges_by_l1 "$l1_block_number_hex")"
assert_rpc_result "pre-restart L1->L2 correlation" "$before_l1_to_l2"
before_range="$(
    jq -cer --arg tx "$l1_tx" \
        '.result[] | select((.l1TransactionHash | ascii_downcase) == $tx)' \
        <<<"$before_l1_to_l2"
)" || { echo "pre-restart reverse correlation is missing $l1_tx" >&2; exit 1; }

retrieved_before="$(cast rpc eth_getBlockByHash "$target_hash" false --rpc-url "$L2_RPC")"
[[ "$(jq -er '.hash | ascii_downcase' <<<"$retrieved_before")" == "$target_hash" ]] || {
    echo "safe block is not retrievable by hash before restart" >&2
    exit 1
}

echo "==> stopping eez-node"
restart_started_at="$(date -u +%s)"
kurtosis service stop "$ENCLAVE" eez-node >/dev/null
node_stopped=true

outage_observed=false
if ! curl -fsS --max-time 3 "$L2_RPC" \
    -H 'content-type: application/json' \
    --data-binary '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
    >/dev/null 2>&1; then
    outage_observed=true
fi
[[ "$outage_observed" == true ]] || {
    echo "eez-node RPC remained reachable after Kurtosis reported it stopped" >&2
    exit 1
}

echo "==> starting eez-node and waiting for recovery"
kurtosis service start "$ENCLAVE" eez-node >/dev/null
node_stopped=false
refresh_dynamic_rpcs

deadline=$((SECONDS + TIMEOUT))
recovered=false
while (( SECONDS < deadline )); do
    chain_id="$(cast chain-id --rpc-url "$L2_RPC" 2>/dev/null || true)"
    if [[ "$chain_id" == "6290" ]] &&
        cast rpc eth_getBlockByHash "$target_hash" false --rpc-url "$L2_RPC" \
            >"$temp_dir/retrieved-after.json" 2>/dev/null &&
        [[ "$(jq -r '.hash // empty | ascii_downcase' "$temp_dir/retrieved-after.json")" == "$target_hash" ]]; then
        recovered=true
        break
    fi
    sleep 2
done
[[ "$recovered" == true ]] || {
    echo "eez-node did not recover the pre-restart safe block within ${TIMEOUT}s" >&2
    exit 1
}
restart_recovered_at="$(date -u +%s)"

echo "==> verifying persisted correlation in both directions"
deadline=$((SECONDS + TIMEOUT))
correlation_recovered=false
after_l2_to_l1='{}'
after_l1_to_l2='{}'
after_range=''
while (( SECONDS < deadline )); do
    after_l2_to_l1="$(settlement_by_l2 "$target_number_hex")"
    if [[ "$(jq -r '.error // empty' <<<"$after_l2_to_l1")" == "" ]] &&
        [[ "$(jq -r '.result == null' <<<"$after_l2_to_l1")" == false ]]; then
        after_l1_to_l2="$(ranges_by_l1 "$l1_block_number_hex")"
        if [[ "$(jq -r '.error // empty' <<<"$after_l1_to_l2")" == "" ]] &&
            [[ "$(jq -r '.result == null' <<<"$after_l1_to_l2")" == false ]] &&
            after_range="$(
                jq -cer --arg tx "$l1_tx" \
                    '.result[] | select((.l1TransactionHash | ascii_downcase) == $tx)' \
                    <<<"$after_l1_to_l2"
            )"; then
            correlation_recovered=true
            break
        fi
    fi
    sleep 2
done
[[ "$correlation_recovered" == true ]] || {
    echo "bidirectional correlation did not recover within ${TIMEOUT}s" >&2
    echo "last L2->L1 response: $after_l2_to_l1" >&2
    echo "last L1->L2 response: $after_l1_to_l2" >&2
    exit 1
}
correlation_recovered_at="$(date -u +%s)"

[[ "$(jq -er '.result.l1TransactionHash | ascii_downcase' <<<"$after_l2_to_l1")" == "$l1_tx" ]]
[[ "$(jq -er '.result.l1BlockHash | ascii_downcase' <<<"$after_l2_to_l1")" == "$l1_block_hash" ]]
[[ "$(jq -er '.result.matchedL2Block.hash | ascii_downcase' <<<"$after_l2_to_l1")" == "$target_hash" ]]
[[ "$(jq -r '.result.canonicalL2' <<<"$after_l2_to_l1")" == true ]]

for field in l1TransactionHash l1BlockHash l1BlockNumber l1TransactionIndex; do
    before_value="$(jq -er --arg field "$field" '.[$field] | ascii_downcase' <<<"$before_range")"
    after_value="$(jq -er --arg field "$field" '.[$field] | ascii_downcase' <<<"$after_range")"
    [[ "$before_value" == "$after_value" ]] || {
        echo "post-restart reverse correlation changed $field" >&2
        exit 1
    }
done
[[ "$(jq -er '.l2Range.firstBlockNumber' <<<"$before_range")" == \
    "$(jq -er '.l2Range.firstBlockNumber' <<<"$after_range")" ]]
[[ "$(jq -er '.l2Range.lastBlockNumber' <<<"$before_range")" == \
    "$(jq -er '.l2Range.lastBlockNumber' <<<"$after_range")" ]]

echo "==> verifying current safe-head and L1 registry convergence"
deadline=$((SECONDS + TIMEOUT))
registry_matches=false
after_safe='{}'
registry_root=""
while (( SECONDS < deadline )); do
    after_safe="$(cast block safe --json --rpc-url "$L2_RPC" 2>/dev/null || echo '{}')"
    after_safe_hash="$(jq -r '.hash // empty | ascii_downcase' <<<"$after_safe")"
    rollup_data="$(
        cast call "$ROLLUPS" 'rollups(uint64)(address,bytes32,uint256)' "$ROLLUP_ID" \
            --rpc-url "$L1_RPC" 2>/dev/null || true
    )"
    registry_root="$(sed -n '2p' <<<"$rollup_data" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
    if [[ -n "$after_safe_hash" && "$registry_root" == "$after_safe_hash" ]]; then
        registry_matches=true
        break
    fi
    sleep 2
done
[[ "$registry_matches" == true ]] || {
    echo "post-restart L1 registry root did not converge to the L2 safe hash" >&2
    exit 1
}

public_https_recovered=null
if [[ -n "$PUBLIC_L2_RPC" ]]; then
    public_chain_id="$(cast chain-id --rpc-url "$PUBLIC_L2_RPC")"
    [[ "$public_chain_id" == "6290" ]] || {
        echo "public L2 RPC returned chain ID $public_chain_id after restart" >&2
        exit 1
    }
    public_https_recovered=true
fi

summary_file="$run_dir/summary.json"
jq -n \
    --arg enclave "$ENCLAVE" \
    --arg target_l2_block "$target_number_hex" \
    --arg target_l2_hash "$target_hash" \
    --arg l1_transaction_hash "$l1_tx" \
    --arg l1_block_hash "$l1_block_hash" \
    --arg l1_block_number "$l1_block_number_hex" \
    --arg after_safe_block "$(jq -er '.number' <<<"$after_safe")" \
    --arg after_safe_hash "$(jq -er '.hash | ascii_downcase' <<<"$after_safe")" \
    --arg registry_root "$registry_root" \
    --argjson outage_observed "$outage_observed" \
    --argjson public_https_recovered "$public_https_recovered" \
    --argjson rpc_recovery_seconds "$((restart_recovered_at - restart_started_at))" \
    --argjson correlation_recovery_seconds "$((correlation_recovered_at - restart_started_at))" \
    --argjson before_l2_to_l1 "$(jq -c '.result' <<<"$before_l2_to_l1")" \
    --argjson after_l2_to_l1 "$(jq -c '.result' <<<"$after_l2_to_l1")" \
    --argjson before_l1_to_l2 "$before_range" \
    --argjson after_l1_to_l2 "$after_range" \
    '{
      verified:true,
      test_kind:"clean_node_restart",
      enclave:$enclave,
      outage_observed:$outage_observed,
      rpc_recovery_seconds:$rpc_recovery_seconds,
      correlation_recovery_seconds:$correlation_recovery_seconds,
      public_https_recovered:$public_https_recovered,
      target:{
        l2_block_number:$target_l2_block,
        l2_block_hash:$target_l2_hash,
        l1_transaction_hash:$l1_transaction_hash,
        l1_block_hash:$l1_block_hash,
        l1_block_number:$l1_block_number
      },
      correlation:{
        before:{l2_to_l1:$before_l2_to_l1,l1_to_l2:$before_l1_to_l2},
        after:{l2_to_l1:$after_l2_to_l1,l1_to_l2:$after_l1_to_l2}
      },
      convergence:{
        safe_l2_block:$after_safe_block,
        safe_l2_hash:$after_safe_hash,
        l1_registry_root:$registry_root
      }
    }' >"$summary_file"

echo "==> node restart recovery passed"
echo "    target L2:  $target_number ($target_hash)"
echo "    L1 posting: $l1_tx"
echo "    RPC recovery:         $((restart_recovered_at - restart_started_at))s"
echo "    correlation recovery: $((correlation_recovered_at - restart_started_at))s"
echo "    evidence:   $summary_file"
