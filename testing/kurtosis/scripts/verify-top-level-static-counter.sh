#!/usr/bin/env bash
# Strict network verifier for the protocol's eventless, view-only
# topLevelStaticCounter scenario. The upstream generic runner cannot publish
# it because the scenario intentionally has no ComputeExpected contract. This
# checker publishes one real trigger, decodes the actual postAndVerifyBatch
# calldata, and proves the exact static entry that made the read succeed.
set -euo pipefail

SCENARIO_LOG="${1:?usage: verify-top-level-static-counter.sh <upstream-log> <output-json>}"
OUTPUT_JSON="${2:?missing evidence output path}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

for var in L1_RPC L1_FRONT L2_RPC ROLLUPS PK PROTOCOL_COMMIT ROLLUP_ID \
    BEACON_URL BLOB_VERIFIER; do
    [[ -n "${!var:-}" ]] || { echo "missing $var" >&2; exit 1; }
done
for tool in cast jq sed sha256sum; do
    command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 1; }
done
[[ -f "$SCENARIO_LOG" ]] || { echo "scenario log not found: $SCENARIO_LOG" >&2; exit 1; }

die() { echo "topLevelStaticCounter evidence error: $*" >&2; exit 1; }

# Pin the workaround to the exact upstream stop. Refuse to publish if the
# runner recorded another error or already sent a user transaction.
grep -Fq '====== Compute Expected Entries ======' "$SCENARIO_LOG" \
    || die "upstream did not reach its known no-ComputeExpected stop"
! grep -q '^ERROR:' "$SCENARIO_LOG" || die "upstream recorded an unexpected error"
! grep -q '====== Execute L1 (user tx) ======' "$SCENARIO_LOG" \
    || die "upstream already sent a user transaction"

field() { sed -n "s/^$1=//p" "$SCENARIO_LOG" | tail -1; }
counter_l2="$(field COUNTER_L2)"
predicted="$(field PREDICTED_STATIC_RESULT)"
proxy_l1="$(field COUNTER_PROXY)"
reader_l1="$(field READER_L1)"

for address in "$counter_l2" "$proxy_l1" "$reader_l1"; do
    [[ "$address" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid scenario address: $address"
done
[[ "$predicted" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid predicted returndata"
predicted_value="$(cast to-dec "$predicted")"

# Prove the fresh deployments are in their declared pre-state before firing
# the one real trigger.
[[ "$(cast call "$reader_l1" 'lastRead()(uint256)' --rpc-url "$L1_RPC")" == "0" ]] \
    || die "reader.lastRead pre-state is not zero"
[[ "$(cast call "$reader_l1" 'counter()(uint256)' --rpc-url "$L1_RPC")" == "0" ]] \
    || die "reader counter pre-state is not zero"
[[ "$(cast call "$counter_l2" 'counter()(uint256)' --rpc-url "$L2_RPC")" == "$predicted_value" ]] \
    || die "L2 producer differs from the prediction before the trigger"

sender="$(cast wallet address --private-key "$PK")"
nonce_rpc="$(cast nonce "$sender" --rpc-url "$L1_RPC")"
nonce_front="$(cast nonce "$sender" --rpc-url "$L1_FRONT")"
nonce=$(( nonce_rpc > nonce_front ? nonce_rpc : nonce_front ))
increment_selector="$(cast sig 'increment()')"
raw_tx="$(cast mktx "$reader_l1" "$increment_selector" \
    --value 0wei --gas-limit 2000000 --nonce "$nonce" \
    --private-key "$PK" --rpc-url "$L1_RPC")"
tx_hash="$(cast rpc eth_sendRawTransaction "$raw_tx" --rpc-url "$L1_FRONT" | tr -d '"[:space:]')"
[[ "$tx_hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid trigger transaction hash: $tx_hash"

deadline=$((SECONDS + ${RECEIPT_TIMEOUT:-300}))
receipt=""
while (( SECONDS < deadline )); do
    candidate="$(cast rpc eth_getTransactionReceipt "$tx_hash" --rpc-url "$L1_RPC" 2>/dev/null || true)"
    if jq -e 'type == "object" and .blockNumber != null' >/dev/null 2>&1 <<<"$candidate"; then
        receipt="$candidate"
        break
    fi
    sleep 5
done
[[ -n "$receipt" ]] || die "no L1 receipt for $tx_hash"
[[ "$(jq -r '.status' <<<"$receipt")" == "0x1" ]] || die "trigger reverted"
block_hex="$(jq -r '.blockNumber' <<<"$receipt")"
block_number="$(cast to-dec "$block_hex")"
user_index="$(cast to-dec "$(jq -r '.transactionIndex' <<<"$receipt")")"
trigger_tx="$(cast rpc eth_getTransactionByHash "$tx_hash" --rpc-url "$L1_RPC")"
[[ "$(jq -r '.to | ascii_downcase' <<<"$trigger_tx")" == "${reader_l1,,}" ]] \
    || die "trigger target differs from READER_L1"
[[ "$(jq -r '.input | ascii_downcase' <<<"$trigger_tx")" == "${increment_selector,,}" ]] \
    || die "trigger calldata is not increment()"

# The reader commits the cached value while the L2 producer remains read-only.
[[ "$(cast call "$reader_l1" 'lastRead()(uint256)' --rpc-url "$L1_RPC")" == "$predicted_value" ]] \
    || die "reader.lastRead differs from the predicted value"
[[ "$(cast call "$reader_l1" 'counter()(uint256)' --rpc-url "$L1_RPC")" == "1" ]] \
    || die "reader counter did not advance exactly once"
[[ "$(cast call "$counter_l2" 'counter()(uint256)' --rpc-url "$L2_RPC")" == "$predicted_value" ]] \
    || die "L2 producer mutated"
probe="$(cast call "$reader_l1" 'increment()(uint256)' --block "$block_number" --rpc-url "$L1_RPC")"
[[ "$probe" == "$predicted_value" ]] || die "no-tx STATICCALL probe returned a different value"

# Derive the exact static key independently through the deployed protocol's
# public hash helper. This binds mode, caller, both rollup IDs, target, value,
# gas mode, and calldata.
counter_selector="$(cast sig 'counter()')"
static_key="$(cast call "$ROLLUPS" \
    'computeCrossChainCallHash(bool,address,uint64,address,uint64,uint256,uint64,bytes)(bytes32)' \
    true "$reader_l1" 0 "$counter_l2" 1 0 0 "$counter_selector" --rpc-url "$L1_RPC")"
[[ "$static_key" =~ ^0x[[:xdigit:]]{64}$ ]] || die "could not derive the static key"

# The pin must equal the registry's live root in the trigger block; otherwise
# the successful trigger could not have resolved this entry.
rollup_data="$(cast call "$ROLLUPS" 'rollups(uint64)(address,bytes32,uint256)' 1 \
    --block "$block_number" --rpc-url "$L1_RPC")"
registry_root="$(sed -n '2p' <<<"$rollup_data" | tr -d '[:space:]')"
[[ "$registry_root" =~ ^0x[[:xdigit:]]{64}$ ]] || die "could not read the pinned registry root"

# Find every postAndVerifyBatch before the user tx in the same block, decode
# each complete ABI tuple, and accept exactly one containing the expected
# static entry. Other aggregated mutable entries are allowed.
post_sig="$(jq -r '.methodIdentifiers | keys[] | select(startswith("postAndVerifyBatch("))' \
    "$REPO/eez-core-protocol/out/EEZ.sol/EEZ.json")"
post_selector="0x$(jq -r --arg sig "$post_sig" '.methodIdentifiers[$sig]' \
    "$REPO/eez-core-protocol/out/EEZ.sol/EEZ.json")"
block_json="$(cast block "$block_number" --full --json --rpc-url "$L1_RPC")"
matches=()
while IFS=$'\t' read -r candidate_hash candidate_index_hex candidate_input; do
    [[ -n "$candidate_hash" ]] || continue
    candidate_index="$(cast to-dec "$candidate_index_hex")"
    (( candidate_index < user_index )) || continue
    decoded="$(cast decode-calldata --json "$post_sig" "$candidate_input")" || continue
    match_count="$(jq \
        --arg key "${static_key,,}" --arg result "${predicted,,}" --arg root "${registry_root,,}" '
          [.[0][2][] | select(
            ((.[0] | length) == 1) and
            (.[0][0][0] == 1) and
            ((.[0][0][1] | ascii_downcase) == $root) and
            ((.[1] | ascii_downcase) == $key) and
            ((.[2] | length) == 0) and
            ((.[3] | ascii_downcase) == "0x0000000000000000000000000000000000000000000000000000000000000000") and
            (.[4] == 1) and
            (.[5] == true) and
            ((.[6] | ascii_downcase) == $result)
          )] | length' <<<"$decoded")"
    [[ "$match_count" == "1" ]] || continue
    matches+=("$candidate_hash $candidate_index_hex")
done < <(jq -r --arg registry "${ROLLUPS,,}" --arg selector "${post_selector,,}" '
    .transactions[]
    | select(((.to // "") | ascii_downcase) == $registry)
    | select(((.input // "") | ascii_downcase) | startswith($selector))
    | [.hash, .transactionIndex, .input] | @tsv
    ' <<<"$block_json")
(( ${#matches[@]} == 1 )) \
    || die "expected one exact static postBatch before the trigger, found ${#matches[@]}"
read -r post_hash post_index_hex <<<"${matches[0]}"
post_index="$(cast to-dec "$post_index_hex")"
post_receipt="$(cast rpc eth_getTransactionReceipt "$post_hash" --rpc-url "$L1_RPC")"
post_tx="$(cast rpc eth_getTransactionByHash "$post_hash" --rpc-url "$L1_RPC")"
[[ "$(jq -r '.status' <<<"$post_receipt")" == "0x1" ]] || die "postBatch reverted"

tx_object() {
    local role="$1" chain="$2" hash="$3" object_receipt="$4" object_tx="$5"
    local object_block object_index
    object_block="$(cast to-dec "$(jq -r '.blockNumber' <<<"$object_receipt")")"
    object_index="$(cast to-dec "$(jq -r '.transactionIndex' <<<"$object_receipt")")"
    jq -cn --arg role "$role" --arg chain "$chain" --arg hash "$hash" \
        --argjson receipt "$object_receipt" --argjson tx "$object_tx" \
        --argjson block_number "$object_block" --argjson transaction_index "$object_index" '
        {role: $role, chain: $chain, hash: $hash, status: $receipt.status,
         block_number: $block_number, block_hex: $receipt.blockNumber,
         transaction_index: $transaction_index, to: $tx.to, input: $tx.input}'
}
trigger_object="$(tx_object user_trigger L1 "$tx_hash" "$receipt" "$trigger_tx")"
post_object="$(tx_object static_entry_post L1 "$post_hash" "$post_receipt" "$post_tx")"
log_sha256="$(sha256sum "$SCENARIO_LOG" | awk '{print $1}')"
causal_graph="$(jq -cn \
    --arg direction L1_to_L2 --arg trigger_hash "$tx_hash" --arg post_hash "$post_hash" \
    --arg static_key "$static_key" --arg root_pin "$registry_root" \
    '{verified: true, direction: $direction, mode: "view_only_no_destination",
      source_transactions: [$trigger_hash], settlement_transactions: [$post_hash],
      l2_system_transactions: 0, pairs: [],
      static_entry: {proxy_entry_hash: $static_key, root_pin: $root_pin},
      claims: {canonical_l1_calldata: true, exact_posted_static_entry: true,
               root_pin_matched_live_registry: true, same_block_order_verified: true,
               no_destination_delivery_required: true}}')"

blob_tmp="$(mktemp -d /tmp/eez-top-level-static-blob.XXXXXX)"
trap 'rm -rf "$blob_tmp"' EXIT
blob_evidence_file="$blob_tmp/evidence.json"
sidecars_file="${OUTPUT_JSON%.json}.blob-sidecars.json"
if ! L1_RPC="$L1_RPC" L2_RPC="$L2_RPC" BEACON_URL="$BEACON_URL" \
    BLOB_VERIFIER="$BLOB_VERIFIER" ROLLUP_ID="$ROLLUP_ID" \
    bash "$(dirname "$0")/verify-semantic-settlement-blob.sh" \
        "$post_hash" L1 "$blob_evidence_file" "$sidecars_file" "$tx_hash"; then
    die "settlement blob semantic verification failed"
fi
blob_da="$(jq -c . "$blob_evidence_file")"

mkdir -p "$(dirname "$OUTPUT_JSON")"
tmp_output="${OUTPUT_JSON}.tmp"
jq -n \
    --argjson schema_version 2 --arg scenario topLevelStaticCounter \
    --arg scenario_path script/e2e/static/L1_to_L2/topLevelStaticCounter/E2ETopLevelStaticCounter.s.sol \
    --arg protocol_commit "$PROTOCOL_COMMIT" --arg result PASS \
    --arg verification_mode external-view-only-static \
    --arg static_key "$static_key" --arg root_pin "$registry_root" \
    --arg cached_return "$predicted" --argjson predicted_value "$predicted_value" \
    --argjson trigger "$trigger_object" --argjson post "$post_object" \
    --argjson causal_graph "$causal_graph" \
    --argjson blob_da "$blob_da" \
    --arg log "$SCENARIO_LOG" --arg log_sha256 "$log_sha256" \
    '{schema_version: $schema_version, scenario: $scenario, scenario_path: $scenario_path,
      protocol_commit: $protocol_commit, result: $result, verification_mode: $verification_mode,
      trigger_chain: "L1", trigger_transactions: [$trigger],
      settlement_transactions: [$post], system_transactions: [],
      static_entry: {expected_roots: [{rollup_id: 1, root: $root_pin}],
                     proxy_entry_hash: $static_key, l2_to_l1_calls: 0,
                     rolling_hash: "0x0000000000000000000000000000000000000000000000000000000000000000",
                     destination_rollup_id: 1, success: true, return_data: $cached_return},
      causal_graph: $causal_graph,
      blob_da: $blob_da,
      observations: {predicted_value: $predicted_value, reader_last_read: $predicted_value,
                     reader_counter: 1, producer_counter: $predicted_value,
                     no_tx_staticcall_result: $predicted_value},
      claims: {upstream_stopped_at_known_eventless_phase: true, receipts_revalidated: true,
               causal_coverage_complete: true,
               blob_kzg_authenticated: true, native_semantics_decoded: true,
               semantic_trigger_bytes_bound: true,
               exact_posted_static_entry_decoded: true, root_pin_matched_live_registry: true,
               post_batch_preceded_trigger: true, no_destination_delivery_required: true,
               producer_remained_read_only: true},
      proof_statements: [
        "PASS: exact posted static entry decoded and independently keyed",
        "PASS: root pin matched the live L1 registry root at the trigger block",
        "PASS: postBatch preceded the trigger in the same block",
        "PASS: no-tx STATICCALL returned the predicted value",
        "PASS: L2 producer remained read-only"
      ],
      source_log: {path: $log, sha256: $log_sha256}}' >"$tmp_output"
mv "$tmp_output" "$OUTPUT_JSON"

cat <<EVIDENCE
TOP_LEVEL_STATIC_TX=$tx_hash
TOP_LEVEL_STATIC_BLOCK=$block_number
TOP_LEVEL_STATIC_POST_BATCH_TX=$post_hash
PASS: exact posted static entry decoded and independently keyed
PASS: root pin matched live registry root $registry_root
PASS: postBatch preceded the trigger in the same block ($post_index < $user_index)
PASS: reader.lastRead=$predicted_value and reader.counter=1
PASS: no-tx STATICCALL probe returned $predicted_value
PASS: L2 producer remained read-only at counter=$predicted_value
EVIDENCE
