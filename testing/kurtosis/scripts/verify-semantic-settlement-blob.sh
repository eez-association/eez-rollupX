#!/usr/bin/env bash
# Authenticate and decode the exact blob-backed settlement for one protocol
# scenario, then bind every reported user trigger to semantic tx_data.
set -euo pipefail

POST_HASH="${1:?usage: verify-semantic-settlement-blob.sh <post-hash> <L1|L2> <output-json> <sidecars-json> <trigger-hash>...}"
TRIGGER_CHAIN="${2:?missing trigger chain}"
OUTPUT_JSON="${3:?missing verifier output path}"
SIDECARS_JSON="${4:?missing beacon sidecars output path}"
shift 4
TRIGGER_HASHES=("$@")

for var in L1_RPC L2_RPC BEACON_URL BLOB_VERIFIER ROLLUP_ID; do
    [[ -n "${!var:-}" ]] || { echo "missing $var" >&2; exit 1; }
done
for tool in cast cp curl jq; do
    command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 1; }
done
[[ "$TRIGGER_CHAIN" == L1 || "$TRIGGER_CHAIN" == L2 ]] \
    || { echo "trigger chain must be L1 or L2" >&2; exit 1; }
[[ "$POST_HASH" =~ ^0x[[:xdigit:]]{64}$ ]] || { echo "invalid settlement hash" >&2; exit 1; }
(( ${#TRIGGER_HASHES[@]} > 0 )) || { echo "at least one trigger hash is required" >&2; exit 1; }
[[ -x "$BLOB_VERIFIER" ]] || { echo "blob verifier is not executable: $BLOB_VERIFIER" >&2; exit 1; }

die() {
    echo "semantic blob evidence error: $*" >&2
    exit 1
}

tmp_dir="$(mktemp -d /tmp/eez-semantic-blob-evidence.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT

post_tx="$(cast rpc eth_getTransactionByHash "$POST_HASH" --rpc-url "$L1_RPC")" \
    || die "cannot read settlement transaction $POST_HASH"
post_receipt="$(cast rpc eth_getTransactionReceipt "$POST_HASH" --rpc-url "$L1_RPC")" \
    || die "cannot read settlement receipt $POST_HASH"
[[ "$(jq -r '.status' <<<"$post_receipt")" == 0x1 ]] || die "settlement reverted"
[[ "$(jq -r '.type | ascii_downcase' <<<"$post_tx")" == 0x3 ]] \
    || die "settlement is not an EIP-4844 type-3 transaction"

block_hash="$(jq -er '.blockHash' <<<"$post_tx")"
block_json="$(cast rpc eth_getBlockByHash "$block_hash" false --rpc-url "$L1_RPC")" \
    || die "cannot read settlement block $block_hash"
timestamp_hex="$(jq -er '.timestamp' <<<"$block_json")"
timestamp=$((timestamp_hex))
genesis_json="$(curl -fsS "$BEACON_URL/eth/v1/beacon/genesis")" \
    || die "cannot read beacon genesis"
spec_json="$(curl -fsS "$BEACON_URL/eth/v1/config/spec")" \
    || die "cannot read beacon spec"
genesis_time="$(jq -er '.data.genesis_time | tonumber' <<<"$genesis_json")"
seconds_per_slot="$(jq -er '.data.SECONDS_PER_SLOT | tonumber' <<<"$spec_json")"
(( seconds_per_slot > 0 && timestamp >= genesis_time )) || die "invalid beacon timing"
elapsed=$((timestamp - genesis_time))
(( elapsed % seconds_per_slot == 0 )) || die "execution timestamp is not on a beacon slot"
slot=$((elapsed / seconds_per_slot))

sidecars_tmp="$tmp_dir/beacon-blobs.json"
for _attempt in {1..20}; do
    if curl -fsS "$BEACON_URL/eth/v1/beacon/blobs/$slot" -o "$sidecars_tmp" 2>/dev/null ||
        curl -fsS "$BEACON_URL/eth/v1/beacon/blob_sidecars/$slot" -o "$sidecars_tmp" 2>/dev/null; then
        break
    fi
    sleep 2
done
[[ -s "$sidecars_tmp" ]] || die "beacon blobs for slot $slot are unavailable"

verifier_input="$tmp_dir/input.json"
verifier_output="$tmp_dir/output.json"
jq -n \
    --argjson rollup_id "$ROLLUP_ID" \
    --argjson beacon_slot "$slot" \
    --argjson l1_transaction "$post_tx" \
    --argjson l1_receipt "$post_receipt" \
    --slurpfile beacon_sidecars "$sidecars_tmp" \
    '{rollup_id:$rollup_id, beacon_slot:$beacon_slot,
      l1_transaction:$l1_transaction, l1_receipt:$l1_receipt,
      beacon_sidecars:$beacon_sidecars[0]}' >"$verifier_input"
"$BLOB_VERIFIER" "$verifier_input" >"$verifier_output" \
    || die "independent KZG/native semantic verification failed"
jq -e '.verified == true and .payload.semantic_transaction_count > 0 and
       .payload.semantic_action_count > 0' "$verifier_output" >/dev/null \
    || die "verified blob contains no cross-chain semantic actions"

expected_origin=0
trigger_rpc="$L1_RPC"
if [[ "$TRIGGER_CHAIN" == L2 ]]; then
    expected_origin="$ROLLUP_ID"
    trigger_rpc="$L2_RPC"
fi
jq -e --argjson origin "$expected_origin" \
    '.payload.semantic_origins | index($origin) != null' "$verifier_output" >/dev/null \
    || die "semantic stream does not contain origin chain $expected_origin"

raw_triggers=()
for trigger_hash in "${TRIGGER_HASHES[@]}"; do
    [[ "$trigger_hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid trigger hash $trigger_hash"
    raw="$(cast rpc eth_getRawTransactionByHash "$trigger_hash" --rpc-url "$trigger_rpc" | jq -er '.')" \
        || die "cannot read raw trigger transaction $trigger_hash"
    jq -e --arg raw "${raw,,}" \
        '[.payload.semantic_transactions[].tx_data | ascii_downcase] | index($raw) != null' \
        "$verifier_output" >/dev/null \
        || die "raw trigger $trigger_hash is absent from semantic tx_data"
    raw_triggers+=("$(jq -cn --arg hash "$trigger_hash" --arg raw "$raw" \
        '{hash:$hash, raw_transaction:$raw}')")
done
raw_triggers_json="$(printf '%s\n' "${raw_triggers[@]}" | jq -s .)"

mkdir -p "$(dirname "$OUTPUT_JSON")" "$(dirname "$SIDECARS_JSON")"
cp "$sidecars_tmp" "$SIDECARS_JSON"
jq \
    --arg trigger_chain "$TRIGGER_CHAIN" \
    --arg sidecars_file "$SIDECARS_JSON" \
    --argjson expected_origin "$expected_origin" \
    --argjson raw_triggers "$raw_triggers_json" \
    '. + {semantic_evidence: {
      trigger_chain: $trigger_chain,
      expected_origin: $expected_origin,
      raw_trigger_transactions: $raw_triggers,
      all_trigger_bytes_bound: true,
      beacon_sidecars_file: $sidecars_file
    }}' "$verifier_output" >"$OUTPUT_JSON"
