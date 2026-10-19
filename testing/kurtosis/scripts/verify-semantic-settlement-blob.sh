#!/usr/bin/env bash
# Authenticate and decode the exact blob-backed settlement for one protocol
# scenario and bind its semantic brackets to the posted settlement tables.
# An L2 trigger is additionally bound byte-for-byte to its bracket's tx_data.
# L1-origin brackets publish empty tx_data (the L1 transaction is already on
# L1), so an L1 trigger is bound through the tables: the caller's canonical
# trigger-to-table verification plus this bracket-to-table binding.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

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

l2_chain_id="$(cast chain-id --rpc-url "$L2_RPC")" || die "cannot read L2 chain id"
verifier_input="$tmp_dir/input.json"
verifier_output="$tmp_dir/output.json"
# Packed settlement transactions and receipts can exceed the OS argument limit.
# Keep the full evidence in files rather than passing it through --argjson.
printf '%s\n' "$post_tx" >"$tmp_dir/post-tx.json"
printf '%s\n' "$post_receipt" >"$tmp_dir/post-receipt.json"
jq -n \
    --argjson rollup_id "$ROLLUP_ID" \
    --argjson l2_chain_id "$l2_chain_id" \
    --argjson beacon_slot "$slot" \
    --slurpfile l1_transaction "$tmp_dir/post-tx.json" \
    --slurpfile l1_receipt "$tmp_dir/post-receipt.json" \
    --slurpfile beacon_sidecars "$sidecars_tmp" \
    '{rollup_id:$rollup_id, l2_chain_id:$l2_chain_id, beacon_slot:$beacon_slot,
      l1_transaction:$l1_transaction[0], l1_receipt:$l1_receipt[0],
      beacon_sidecars:$beacon_sidecars[0]}' >"$verifier_input"
"$BLOB_VERIFIER" "$verifier_input" >"$verifier_output" \
    || die "independent KZG/native semantic verification failed"
jq -e '.verified == true and .payload.semantic_transaction_count > 0 and
       .payload.semantic_action_count > 0' "$verifier_output" >/dev/null \
    || die "verified blob contains no cross-chain semantic actions"
jq -e '.payload.semantics_bound_to_settlement == true' "$verifier_output" >/dev/null \
    || die "semantic brackets are not bound to the posted settlement tables"
jq -e '[.payload.semantic_transactions[] | select(.origin_chain == 0) | .tx_data] | all(. == "0x")' \
    "$verifier_output" >/dev/null \
    || die "an L1-origin bracket publishes transaction bytes"

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
    raw="$(raw_transaction_for "$trigger_rpc" "$trigger_hash" "$tmp_dir")" \
        || die "cannot read raw trigger transaction $trigger_hash"
    if [[ "$TRIGGER_CHAIN" == L2 ]]; then
        jq -e --arg raw "${raw,,}" \
            '[.payload.semantic_transactions[].tx_data | ascii_downcase] | index($raw) != null' \
            "$verifier_output" >/dev/null \
            || die "raw trigger $trigger_hash is absent from semantic tx_data"
    fi
    raw_triggers+=("$(jq -cn --arg hash "$trigger_hash" --arg raw "$raw" \
        '{hash:$hash, raw_transaction:$raw}')")
done
printf '%s\n' "${raw_triggers[@]}" | jq -s . >"$tmp_dir/raw-triggers.json"

mkdir -p "$(dirname "$OUTPUT_JSON")" "$(dirname "$SIDECARS_JSON")"
cp "$sidecars_tmp" "$SIDECARS_JSON"
jq \
    --arg trigger_chain "$TRIGGER_CHAIN" \
    --arg sidecars_file "$SIDECARS_JSON" \
    --argjson expected_origin "$expected_origin" \
    --slurpfile raw_triggers "$tmp_dir/raw-triggers.json" \
    '. + {semantic_evidence: {
      trigger_chain: $trigger_chain,
      expected_origin: $expected_origin,
      raw_trigger_transactions: $raw_triggers[0],
      trigger_binding: (if $trigger_chain == "L2" then "exact_tx_data" else "settlement_table" end),
      semantics_bound_to_settlement: true,
      all_trigger_bytes_bound: ($trigger_chain == "L2"),
      beacon_sidecars_file: $sidecars_file
    }}' "$verifier_output" >"$OUTPUT_JSON"
