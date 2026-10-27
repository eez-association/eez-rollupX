#!/usr/bin/env bash
# Owned offset-5 devnet only. Produce real static/lookup/delivery evidence.
set -euo pipefail
: "${KURTOSIS_ENCLAVE:?select the owned combined-history enclave}"
: "${KURTOSIS_ARGS_FILE:?select its exact args file}"
: "${EEZ_COMBINED_CAPTURE_DIR:?choose a fresh evidence directory}"
K="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$K/../.." && pwd)"
ENCLAVE="$KURTOSIS_ENCLAVE"
source "$K/ports.sh" >/dev/null
source "$K/scripts/lib.sh"
L1="$EEZ_DEVNET_L1_RPC"; L2="$EEZ_DEVNET_L2_RPC"; L1F="$EEZ_DEVNET_L1_FRONT"
DIR="$EEZ_COMBINED_CAPTURE_DIR"
[[ ! -e "$DIR" ]] || { echo "Choose a new evidence directory" >&2; exit 1; }
mkdir -p "$DIR/deployment" "$DIR/quotes"
load_deployment_artifact "$DIR/deployment"
set -a
source "$DIR/deployment/deployments.env"
set +a
jq -e '.config.rollupX | .blockIntervalSeconds == 2 and .syncIntervalBlocks == 6 and .historyUpdateOffset == 5' \
    "$DIR/deployment/l2-genesis.json" >/dev/null
cp "$DIR/deployment/l2-genesis.json" "$DIR/l2-genesis.json"
kurtosis files download "$ENCLAVE" el_cl_genesis_data "$DIR/l1" >/dev/null
cp "$DIR/l1/genesis.json" "$DIR/l1-genesis.json"
# Public fixture key 2 is funded independently of the poster and attester.
KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
SENDER=$(cast wallet address --private-key "$KEY")
forge create --root "$REPO/contracts" src/CombinedHistoryObservation.sol:CombinedHistoryObservation \
    --rpc-url "$L2" --private-key "$KEY" --broadcast --json \
    >"$DIR/probe-deployment.json"
PROBE=$(jq -er '.deployedTo' "$DIR/probe-deployment.json")
PROXY=$(forge_deploy "$L1" "$KEY" CreateValueProxy.s.sol:CreateValueProxy \
    'run(address,address,uint64)' "$EEZ_REGISTRY_ADDRESS" "$PROBE" "$EEZ_ROLLUP_ID" | grab_address EEZ_VALUE_PROXY)
forge create --root "$REPO/contracts" src/CombinedHistoryObservation.sol:CombinedHistoryReader \
    --rpc-url "$L1" --private-key "$KEY" --broadcast --json --constructor-args "$PROXY" \
    >"$DIR/reader-deployment.json"
READER=$(jq -er '.deployedTo' "$DIR/reader-deployment.json")
ATT=$(cast wallet address --private-key "$(yaml_value proof_signer_key)")
jq -n --arg probe "$PROBE" --arg reader "$READER" --arg registry "$EEZ_REGISTRY_ADDRESS" \
    --arg manager "$EEZ_ROLLUP_MANAGER_ADDRESS" --arg ps "$EEZ_PROOF_SYSTEM" --arg attester "$ATT" \
    --arg vkey "$EEZ_VKEY" \
    '{probe:$probe,reader:$reader,registry_address:$registry,rollup_manager_address:$manager,proof_system:$ps,attester:$attester,vkey:$vkey,rollup_id:1}' \
    >"$DIR/identity.json"

# Populate the proved window with ordinary writes. Rich Sync blocks currently
# contain history followed by cross-chain groups; ordinary mempool writes land
# in preceding blocks. The replay test records this boundary explicitly.
(
    nonce=$(cast nonce "$SENDER" --rpc-url "$L2")
    for ((i=0; i<360; i++)); do
        cast send --async --rpc-url "$L2" --private-key "$KEY" --nonce "$nonce" \
            --gas-limit 100000 --gas-price 100000000000 --priority-gas-price 1000000000 \
            "$PROBE" 'bump()'
        nonce=$((nonce + 1))
        sleep 0.5
    done
) >"$DIR/ordinary-transactions.log" 2>&1 &
ordinary_pid=$!
trap 'kill "$ordinary_pid" 2>/dev/null || true; wait "$ordinary_pid" 2>/dev/null || true' EXIT

for attempt in 1 2 3; do
    nonce=$(cast nonce "$SENDER" --rpc-url "$L1")
    raw=$(cast mktx --rpc-url "$L1" --private-key "$KEY" --nonce "$nonce" \
        --gas-limit 3000000 --gas-price 200000000000 --priority-gas-price 100000000000 \
        "$READER" 'run(uint256)' "$((attempt * 1000000))")
    raw=$(sign_fee_quoted "$L1F" "$L1" "$KEY" "$raw" "$DIR/quotes/$attempt.json")
    hash=$(cast keccak "$raw")
    send_front "$L1F" "$raw" "$hash"
    timeout 600s cast receipt "$hash" --rpc-url "$L1" --json >"$DIR/source-$attempt.json"
    jq -e '.status == "0x1"' "$DIR/source-$attempt.json" >/dev/null
    cast call "$READER" 'runs()(uint256)' --rpc-url "$L1" >"$DIR/runs-$attempt.txt"
    [[ "$(awk '{print $1}' "$DIR/runs-$attempt.txt")" == "$attempt" ]] || {
        echo "Reader did not record exactly $attempt completed source calls" >&2; exit 1;
    }
done

cast block safe --rpc-url "$L2" --json >"$DIR/safe-block.json"
container=$(docker ps --filter "label=com.kurtosistech.enclave-name=$ENCLAVE" \
    --filter 'label=com.kurtosistech.id=eez-prove-capture' --format '{{.ID}}')
[[ "$container" =~ ^[0-9a-f]{12,64}$ ]] || { echo "Capture container not uniquely identified" >&2; exit 1; }
docker cp "$container:/data/captures/." "$DIR/"
capture_runtime_log eez-node "$DIR/node.log"
capture_runtime_log eez-proof-signer "$DIR/signer.log"
printf 'Captured real requests in %s\n' "$DIR"
