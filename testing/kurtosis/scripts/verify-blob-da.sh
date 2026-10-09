#!/usr/bin/env bash
# Exercise blob DA with a real contract deployment and state-changing calls,
# then independently verify the L1 transactions, Beacon blob responses, decoded
# payloads, L2 safe head, and L1 registry root.
set -euo pipefail

KURTOSIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$KURTOSIS_DIR/../.." && pwd)"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-blob}"
source "$KURTOSIS_DIR/scripts/lib.sh"
ROLLUP_ID="${EEZ_ROLLUP_ID:-1}"
TIMEOUT="${EEZ_BLOB_SMOKE_TIMEOUT:-600}"
RESULT_ROOT="${EEZ_BLOB_RESULT_DIR:-$REPO/artifacts/kurtosis-blob}"
PK="${PK:-0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a}"
VERIFY_BLOBSCAN="${EEZ_VERIFY_BLOBSCAN-true}"
case "$VERIFY_BLOBSCAN" in
    true|false) ;;
    *) echo "EEZ_VERIFY_BLOBSCAN must be true or false" >&2; exit 1 ;;
esac

for tool in cast curl forge jq kurtosis; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done
[[ "$ROLLUP_ID" =~ ^[0-9]+$ ]] || { echo "EEZ_ROLLUP_ID must be a decimal u64" >&2; exit 1; }
[[ "$TIMEOUT" =~ ^[0-9]+$ ]] || { echo "EEZ_BLOB_SMOKE_TIMEOUT must be seconds" >&2; exit 1; }

export KURTOSIS_ENCLAVE="$ENCLAVE"
source "$KURTOSIS_DIR/ports.sh" >/dev/null
L1_RPC="${L1_RPC:-$EEZ_DEVNET_L1_RPC}"
L2_RPC="${L2_RPC:-$EEZ_DEVNET_L2_RPC}"
L2_FRONT="${L2_FRONT:-$EEZ_DEVNET_L2_FRONT}"

http_url() {
    case "$1" in
        http://*|https://*) printf '%s\n' "$1" ;;
        *) printf 'http://%s\n' "$1" ;;
    esac
}

BEACON_URL="${EEZ_L1_BEACON_URL:-$(http_url "$(kurtosis port print "$ENCLAVE" cl-1-lighthouse-reth http)")}"
if [[ "${EEZ_USE_LOCAL_DEPLOYMENTS:-0}" == "1" ]]; then
    RELAY_URL="${EEZ_L1_BUILDER_RPC_URL:?set the public-network bundle relay}"
else
    RELAY_URL="${EEZ_MEV_RELAY_URL:-$(http_url "$(kurtosis port print "$ENCLAVE" mev-relay-api http)")}"
fi

run_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="$RESULT_ROOT/$run_stamp"
mkdir -p "$RUN_DIR/batches"
temp_dir="$(mktemp -d /tmp/eez-blob-smoke.XXXXXX)"
trap 'rm -rf "$temp_dir"' EXIT

mkdir -p "$temp_dir/deployments"
load_deployment_artifact "$temp_dir/deployments"
set -a
source "$temp_dir/deployments/deployments.env"
set +a
ROLLUPS="${ROLLUPS:-$EEZ_REGISTRY_ADDRESS}"

source "$KURTOSIS_DIR/scripts/runtime-tools.sh"
VERIFIER="$(resolve_runtime_tool eez-l1 verify_blob_evidence "${EEZ_BLOB_VERIFIER_BIN:-}")"

# Build before recording the L1 starting block so compilation latency cannot
# move the test transactions across more composer windows than necessary.
forge build --root "$REPO/contracts" >/dev/null

if [[ "${EEZ_USE_LOCAL_DEPLOYMENTS:-0}" == "1" ]]; then
    # A public bundle builder need not expose the local PBS fixture's
    # validator-registration API. Check its real submission method instead;
    # successful inclusion and full blob retrieval are still required below.
    # The list may name several comma-separated relays; at least one must
    # serve eth_sendBundle.
    source "$REPO/scripts/builder-relays.sh"
    probe_bundle_relays "$RELAY_URL"
    echo "==> public bundle submission endpoint available"
else
echo "==> waiting for MEV relay validator registrations"
deadline=$((SECONDS + TIMEOUT))
registrations=0
while (( SECONDS < deadline )); do
    registrations="$(curl -fsS "$RELAY_URL/relay/v1/builder/validators" 2>/dev/null | jq -r 'length' 2>/dev/null || true)"
    if [[ "$registrations" =~ ^[0-9]+$ ]] && (( registrations > 0 )); then
        break
    fi
    sleep 3
done
if ! [[ "$registrations" =~ ^[0-9]+$ ]] || (( registrations == 0 )); then
    echo "MEV relay did not receive validator registrations within ${TIMEOUT}s" >&2
    exit 1
fi
echo "    relay ready with $registrations registrations"
fi

start_l1_block="$(cast block-number --rpc-url "$L1_RPC")"
sender="$(cast wallet address --private-key "$PK")"

echo "==> deploying and mutating Counter on L2"
deployment_json="$(
    forge create --root "$REPO/contracts" --broadcast --json \
        --rpc-url "$L2_RPC" --private-key "$PK" src/Counter.sol:Counter
)"
counter="$(jq -er '.deployedTo' <<<"$deployment_json")"
deploy_hash="$(jq -er '.transactionHash' <<<"$deployment_json")"
deploy_receipt="$(cast rpc eth_getTransactionReceipt "$deploy_hash" --rpc-url "$L2_RPC")"
[[ "$(jq -r '.status' <<<"$deploy_receipt")" == "0x1" ]] || {
    echo "Counter deployment $deploy_hash failed" >&2
    exit 1
}

increment_receipt="$(
    cast send "$counter" 'increment()(uint256)' --json \
        --rpc-url "$L2_RPC" --private-key "$PK"
)"
add_receipt="$(
    cast send "$counter" 'add(uint256)(uint256)' 41 --json \
        --rpc-url "$L2_RPC" --private-key "$PK"
)"
for receipt in "$increment_receipt" "$add_receipt"; do
    [[ "$(jq -r '.status' <<<"$receipt")" == "0x1" ]] || {
        echo "Counter call failed: $(jq -c . <<<"$receipt")" >&2
        exit 1
    }
done
increment_hash="$(jq -er '.transactionHash' <<<"$increment_receipt")"
add_hash="$(jq -er '.transactionHash' <<<"$add_receipt")"
expected_hashes=("${deploy_hash,,}" "${increment_hash,,}" "${add_hash,,}")

max_l2_block=0
declare -A l2_block_by_tx=()
declare -A l2_block_hash_by_tx=()
for receipt in "$deploy_receipt" "$increment_receipt" "$add_receipt"; do
    block_hex="$(jq -er '.blockNumber' <<<"$receipt")"
    block_number=$((block_hex))
    receipt_tx="$(jq -er '.transactionHash | ascii_downcase' <<<"$receipt")"
    l2_block_by_tx["$receipt_tx"]="$block_number"
    l2_block_hash_by_tx["$receipt_tx"]="$(jq -er '.blockHash | ascii_downcase' <<<"$receipt")"
    (( block_number > max_l2_block )) && max_l2_block="$block_number"
done

declare -A covered_by=()
declare -A seen_l1_transactions=()
verified_batch_files=()

verify_l1_blob_batch() {
    local l1_hash="$1"
    local tx_json receipt_json block_hash block_json timestamp_hex timestamp
    local genesis_json spec_json genesis_time seconds_per_slot elapsed slot
    local sidecars_file input_file output_file attempt

    tx_json="$(cast rpc eth_getTransactionByHash "$l1_hash" --rpc-url "$L1_RPC")"
    receipt_json="$(cast rpc eth_getTransactionReceipt "$l1_hash" --rpc-url "$L1_RPC")"
    block_hash="$(jq -er '.blockHash' <<<"$tx_json")"
    block_json="$(cast rpc eth_getBlockByHash "$block_hash" false --rpc-url "$L1_RPC")"
    timestamp_hex="$(jq -er '.timestamp' <<<"$block_json")"
    timestamp=$((timestamp_hex))
    genesis_json="$(curl -fsS "$BEACON_URL/eth/v1/beacon/genesis")"
    spec_json="$(curl -fsS "$BEACON_URL/eth/v1/config/spec")"
    genesis_time="$(jq -er '.data.genesis_time | tonumber' <<<"$genesis_json")"
    seconds_per_slot="$(jq -er '.data.SECONDS_PER_SLOT | tonumber' <<<"$spec_json")"
    (( timestamp >= genesis_time )) || return 1
    elapsed=$((timestamp - genesis_time))
    (( elapsed % seconds_per_slot == 0 )) || return 1
    slot=$((elapsed / seconds_per_slot))

    sidecars_file="$temp_dir/beacon-blobs-$slot.json"
    for attempt in {1..20}; do
        if curl -fsS "$BEACON_URL/eth/v1/beacon/blobs/$slot" -o "$sidecars_file" 2>/dev/null ||
            curl -fsS "$BEACON_URL/eth/v1/beacon/blob_sidecars/$slot" -o "$sidecars_file" 2>/dev/null; then
            break
        fi
        sleep 2
    done
    [[ -s "$sidecars_file" ]] || return 1

    input_file="$temp_dir/input-${l1_hash#0x}.json"
    output_file="$RUN_DIR/batches/${l1_hash#0x}.json"
    jq -n \
        --argjson rollup_id "$ROLLUP_ID" \
        --argjson beacon_slot "$slot" \
        --argjson l1_transaction "$tx_json" \
        --argjson l1_receipt "$receipt_json" \
        --slurpfile beacon_sidecars "$sidecars_file" \
        '{rollup_id:$rollup_id, beacon_slot:$beacon_slot,
          l1_transaction:$l1_transaction, l1_receipt:$l1_receipt,
          beacon_sidecars:$beacon_sidecars[0]}' >"$input_file"
    if ! "$VERIFIER" "$input_file" >"$output_file" 2>"$temp_dir/verifier-error"; then
        rm -f "$output_file"
        return 1
    fi
    verified_batch_files+=("$output_file")

    local payload_hash expected
    while IFS= read -r payload_hash; do
        for expected in "${expected_hashes[@]}"; do
            if [[ "${payload_hash,,}" == "$expected" ]]; then
                covered_by["$expected"]="$l1_hash"
            fi
        done
    done < <(jq -r '.payload.transaction_hashes[]' "$output_file")
    echo "    verified L1 blob batch $l1_hash (slot $slot)"
}

all_covered() {
    local expected
    for expected in "${expected_hashes[@]}"; do
        [[ -n "${covered_by[$expected]+x}" ]] || return 1
    done
}

echo "==> locating the contract transactions inside authenticated blobs"
next_l1_block="$start_l1_block"
deadline=$((SECONDS + TIMEOUT))
while (( SECONDS < deadline )); do
    latest_l1_block="$(cast block-number --rpc-url "$L1_RPC")"
    while (( next_l1_block <= latest_l1_block )); do
        block_hex="$(cast to-hex "$next_l1_block")"
        block_json="$(cast rpc eth_getBlockByNumber "$block_hex" true --rpc-url "$L1_RPC")"
        while IFS= read -r l1_hash; do
            [[ -n "$l1_hash" ]] || continue
            l1_hash="${l1_hash,,}"
            [[ -z "${seen_l1_transactions[$l1_hash]+x}" ]] || continue
            seen_l1_transactions["$l1_hash"]=1
            verify_l1_blob_batch "$l1_hash" || true
        done < <(jq -r '.transactions[] | select((.type | ascii_downcase) == "0x3") | .hash' <<<"$block_json")
        next_l1_block=$((next_l1_block + 1))
    done
    all_covered && break
    sleep 3
done
if ! all_covered; then
    echo "not every L2 contract transaction was recovered from blob DA within ${TIMEOUT}s" >&2
    for expected in "${expected_hashes[@]}"; do
        echo "  $expected -> ${covered_by[$expected]:-missing}" >&2
    done
    exit 1
fi

echo "==> waiting for the exercised L2 state to become safe"
deadline=$((SECONDS + TIMEOUT))
safe_number=0
while (( SECONDS < deadline )); do
    safe_json="$(cast block safe --json --rpc-url "$L2_RPC" 2>/dev/null || true)"
    safe_number_hex="$(jq -r '.number // empty' <<<"$safe_json" 2>/dev/null || true)"
    if [[ "$safe_number_hex" == 0x* ]]; then
        safe_number=$((safe_number_hex))
        (( safe_number >= max_l2_block )) && break
    fi
    sleep 3
done
(( safe_number >= max_l2_block )) || {
    echo "L2 safe head did not reach exercised block $max_l2_block within ${TIMEOUT}s" >&2
    exit 1
}

safe_block_hash="$(jq -er '.hash' <<<"$safe_json")"
rollup_data="$(cast call "$ROLLUPS" 'rollups(uint64)(address,bytes32,uint256)' "$ROLLUP_ID" --rpc-url "$L1_RPC")"
registry_root="$(sed -n '2p' <<<"$rollup_data" | tr -d '[:space:]')"
[[ "${safe_block_hash,,}" == "${registry_root,,}" ]] || {
    echo "safe L2 block hash $safe_block_hash does not match L1 registry commitment $registry_root" >&2
    exit 1
}

safe_count="$(cast call "$counter" 'count()(uint256)' --block "$safe_number" --rpc-url "$L2_RPC")"
[[ "$safe_count" == "42" ]] || {
    echo "Counter value at safe block $safe_number is $safe_count, expected 42" >&2
    exit 1
}

echo "==> verifying both L1/L2 correlation RPC directions"
correlation_evidence='[]'
for l2_tx in "${expected_hashes[@]}"; do
    l2_block="${l2_block_by_tx[$l2_tx]}"
    l2_block_hash="${l2_block_hash_by_tx[$l2_tx]}"
    expected_l1_tx="${covered_by[$l2_tx],,}"
    l2_selector="$(cast to-hex "$l2_block")"

    l2_to_l1_response="$(
        jq -cn --arg selector "$l2_selector" \
          '{jsonrpc:"2.0",id:1,method:"eez_getSettlementByL2Block",params:[$selector]}' |
          curl -fsS "$L2_FRONT" -H 'content-type: application/json' --data-binary @-
    )"
    [[ "$(jq -r '.error // empty' <<<"$l2_to_l1_response")" == "" ]] || {
        echo "L2->L1 correlation RPC failed: $l2_to_l1_response" >&2
        exit 1
    }
    observed_l1_tx="$(jq -er '.result.l1TransactionHash | ascii_downcase' <<<"$l2_to_l1_response")"
    observed_l2_hash="$(jq -er '.result.matchedL2Block.hash | ascii_downcase' <<<"$l2_to_l1_response")"
    [[ "$observed_l1_tx" == "$expected_l1_tx" ]] || {
        echo "L2 block $l2_block maps to $observed_l1_tx, expected $expected_l1_tx" >&2
        exit 1
    }
    [[ "$observed_l2_hash" == "$l2_block_hash" ]] || {
        echo "correlation RPC returned L2 hash $observed_l2_hash, receipt has $l2_block_hash" >&2
        exit 1
    }
    [[ "$(jq -r '.result.canonicalL2' <<<"$l2_to_l1_response")" == "true" ]] || {
        echo "settled L2 block $l2_block is not marked canonical" >&2
        exit 1
    }

    batch_file="$RUN_DIR/batches/${expected_l1_tx#0x}.json"
    l1_block="$(jq -er '.l1_block_number' "$batch_file")"
    l1_selector="$(cast to-hex "$l1_block")"
    l1_to_l2_response="$(
        jq -cn --arg selector "$l1_selector" \
          '{jsonrpc:"2.0",id:2,method:"eez_getSettledL2RangesByL1Block",params:[$selector]}' |
          curl -fsS "$L2_FRONT" -H 'content-type: application/json' --data-binary @-
    )"
    [[ "$(jq -r '.error // empty' <<<"$l1_to_l2_response")" == "" ]] || {
        echo "L1->L2 correlation RPC failed: $l1_to_l2_response" >&2
        exit 1
    }
    matching_range="$(
        jq -cer --arg tx "$expected_l1_tx" \
          '.result[] | select((.l1TransactionHash | ascii_downcase) == $tx)' \
          <<<"$l1_to_l2_response"
    )" || {
        echo "L1 block $l1_block has no range for posting tx $expected_l1_tx" >&2
        exit 1
    }
    first_hex="$(jq -er '.l2Range.firstBlockNumber' <<<"$matching_range")"
    last_hex="$(jq -er '.l2Range.lastBlockNumber' <<<"$matching_range")"
    first_l2=$((first_hex))
    last_l2=$((last_hex))
    (( l2_block >= first_l2 && l2_block <= last_l2 )) || {
        echo "L1 tx $expected_l1_tx range $first_l2..$last_l2 does not contain L2 block $l2_block" >&2
        exit 1
    }

    correlation_evidence="$(
        jq -c \
          --arg l2_transaction "$l2_tx" \
          --argjson l2_to_l1 "$(jq -c '.result' <<<"$l2_to_l1_response")" \
          --argjson l1_to_l2 "$matching_range" \
          '. + [{l2_transaction:$l2_transaction,l2_to_l1:$l2_to_l1,l1_to_l2:$l1_to_l2}]' \
          <<<"$correlation_evidence"
    )"
done

batches_json="$(jq -s . "${verified_batch_files[@]}")"
coverage_json='{}'
for expected in "${expected_hashes[@]}"; do
    coverage_json="$(jq -c --arg tx "$expected" --arg batch "${covered_by[$expected]}" '. + {($tx): $batch}' <<<"$coverage_json")"
done

summary_file="$RUN_DIR/summary.json"
jq -n \
    --arg enclave "$ENCLAVE" \
    --arg sender "$sender" \
    --arg counter "$counter" \
    --arg deploy_hash "$deploy_hash" \
    --arg increment_hash "$increment_hash" \
    --arg add_hash "$add_hash" \
    --arg safe_block_hash "$safe_block_hash" \
    --arg registry_root "$registry_root" \
    --argjson rollup_id "$ROLLUP_ID" \
    --argjson safe_l2_block "$safe_number" \
    --argjson counter_at_safe "$safe_count" \
    --argjson coverage "$coverage_json" \
    --argjson correlation_evidence "$correlation_evidence" \
    --argjson batches "$batches_json" \
    '{verified:true, enclave:$enclave, rollup_id:$rollup_id, sender:$sender,
      contract:{address:$counter, deployment_tx:$deploy_hash,
        increment_tx:$increment_hash, add_41_tx:$add_hash, value_at_safe:$counter_at_safe},
      settlement:{safe_l2_block:$safe_l2_block, safe_l2_block_hash:$safe_block_hash,
        l1_registry_commitment:$registry_root},
      l2_transaction_to_l1_blob_batch:$coverage,
      correlation_rpc:$correlation_evidence,
      verified_blob_batches:$batches}' >"$summary_file"

echo "==> blob DA smoke test passed"
echo "    Counter:        $counter"
echo "    safe L2 block:  $safe_number"
echo "    settled hash:   $safe_block_hash"
echo "    evidence:       $summary_file"

if [[ "$VERIFY_BLOBSCAN" == true ]]; then
    "$KURTOSIS_DIR/scripts/verify-blobscan.sh" \
        "$summary_file" \
        "$RUN_DIR/blobscan.json"
else
    echo "==> Blobscan verification explicitly disabled (native blob DA checks passed)"
fi
