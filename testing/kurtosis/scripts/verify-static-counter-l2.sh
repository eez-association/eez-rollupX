#!/usr/bin/env bash
# Independent evidence for a successful upstream staticCounterL2 run. Static
# pool reads have no mutable-entry events; verify their real table calldata,
# trigger state, paired L1 host entry and authenticated semantic blob instead.
set -euo pipefail

SCENARIO_LOG="${1:?usage: verify-static-counter-l2.sh <upstream-log> <output-json>}"
OUTPUT_JSON="${2:?missing evidence output path}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

for var in L1_RPC L2_RPC ROLLUPS MANAGER_L2 PROTOCOL_COMMIT CAUSAL_VERIFIER \
    BEACON_URL BLOB_VERIFIER ROLLUP_ID; do
    [[ -n "${!var:-}" ]] || { echo "missing $var" >&2; exit 1; }
done
for tool in cast jq sed sha256sum; do
    command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 1; }
done
[[ -f "$SCENARIO_LOG" ]] || { echo "scenario log not found: $SCENARIO_LOG" >&2; exit 1; }
[[ -x "$CAUSAL_VERIFIER" ]] || { echo "causal verifier not executable: $CAUSAL_VERIFIER" >&2; exit 1; }

die() { echo "staticCounterL2 evidence error: $*" >&2; exit 1; }

grep -Fq '====== Done ======' "$SCENARIO_LOG" || die "upstream completion marker is absent"
! grep -q '^ERROR:' "$SCENARIO_LOG" || die "upstream recorded an error"
grep -Fq 'VERIFY_PASS reader.lastRead=1 reader.counter=1' "$SCENARIO_LOG" \
    || die "upstream reader state verification is absent"
grep -Eq 'block: .*\(status: 0x1\)' "$SCENARIO_LOG" || die "successful trigger marker is absent"
grep -Eq 'PASS: [1-9][0-9]* settlement tx\(s\) with BatchPosted in range' "$SCENARIO_LOG" \
    || die "L1 settlement range proof is absent"
grep -Fq 'PASS: posted batch calldata matches expected table' "$SCENARIO_LOG" \
    || die "posted L1 table was not field-matched"
grep -Fq 'PASS: posted rolling hashes reproduced from posted roots' "$SCENARIO_LOG" \
    || die "posted L1 rolling hashes were not replayed"

field() { sed -n "s/^$1=//p" "$SCENARIO_LOG" | tail -1; }
reader_l2="$(field READER_L2)"
counter_l1="$(field COUNTER_L1)"
[[ "$reader_l2" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid READER_L2"
[[ "$counter_l1" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid COUNTER_L1"

tx_hash="$(sed -n -E 's/^tx:[[:space:]]*(0x[[:xdigit:]]{64}).*/\1/p' "$SCENARIO_LOG" | tail -1)"
block_hex="$(sed -n -E 's/^block:[[:space:]]*(0x[[:xdigit:]]+)[[:space:]]+\(status: 0x1\).*/\1/p' "$SCENARIO_LOG" | tail -1)"
settlement_hash="$(sed -n -E 's/^Matched settlement tx (0x[[:xdigit:]]{64}).*/\1/p' "$SCENARIO_LOG" | tail -1)"
# The scenario deploys CounterL1 and increments it once. ComputeExpected's
# verbose table is printed only on failure, so it is not a source for this check.
expected_return="$(cast abi-encode 'f(uint256)' 1)"
[[ "$tx_hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid trigger transaction hash"
[[ "$block_hex" =~ ^0x[[:xdigit:]]+$ ]] || die "invalid trigger block"
[[ "$settlement_hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid settlement transaction hash"
[[ "$expected_return" =~ ^0x[[:xdigit:]]+$ ]] || die "invalid expected return data"
block_number="$(cast to-dec "$block_hex")"
predicted_value="$(cast to-dec "$expected_return")"

trigger_receipt="$(cast rpc eth_getTransactionReceipt "$tx_hash" --rpc-url "$L2_RPC")"
trigger_tx="$(cast rpc eth_getTransactionByHash "$tx_hash" --rpc-url "$L2_RPC")"
[[ "$(jq -r '.status' <<<"$trigger_receipt")" == "0x1" ]] || die "trigger reverted"
[[ "$(jq -r '.blockNumber' <<<"$trigger_receipt")" == "$block_hex" ]] \
    || die "trigger receipt block differs from the upstream log"
[[ "$(jq -r '.to | ascii_downcase' <<<"$trigger_tx")" == "${reader_l2,,}" ]] \
    || die "trigger target differs from READER_L2"
increment_selector="$(cast sig 'increment()')"
[[ "$(jq -r '.input | ascii_downcase' <<<"$trigger_tx")" == "${increment_selector,,}" ]] \
    || die "trigger calldata is not increment()"
user_index_hex="$(jq -r '.transactionIndex' <<<"$trigger_receipt")"
user_index="$(cast to-dec "$user_index_hex")"

counter_selector="$(cast sig 'counter()')"
computed_static_key="$(cast call "$MANAGER_L2" \
    'computeCrossChainCallHash(bool,address,uint64,address,uint64,uint256,uint64,bytes)(bytes32)' \
    true "$reader_l2" "$ROLLUP_ID" "$counter_l1" 0 0 0 "$counter_selector" --rpc-url "$L2_RPC")"
[[ "$computed_static_key" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid computed static key"

load_sig="$(jq -r '.methodIdentifiers | keys[] | select(startswith("loadExecutionTable("))' \
    "$REPO/eez-core-protocol/out/EEZL2.sol/EEZL2.json")"
load_selector="0x$(jq -r --arg sig "$load_sig" '.methodIdentifiers[$sig]' \
    "$REPO/eez-core-protocol/out/EEZL2.sol/EEZL2.json")"
block_json="$(cast block "$block_number" --full --json --rpc-url "$L2_RPC")"
jq -en --argjson block "$block_json" --argjson tx "$trigger_tx" --argjson receipt "$trigger_receipt" --arg hash "$tx_hash" '
    $tx.hash == $hash and $tx.hash == $receipt.transactionHash and $tx.blockHash == $block.hash and $receipt.blockHash == $block.hash
    and $receipt.transactionIndex == $tx.transactionIndex and $receipt.blockNumber == $tx.blockNumber
    and $receipt.blockNumber == $block.number
    and ([$block.transactions[] | select(.hash == $tx.hash and .transactionIndex == $tx.transactionIndex
           and .input == $tx.input and .to == $tx.to)] | length) == 1
    ' >/dev/null || die "trigger is not in the canonical L2 block"
matches=()
while IFS=$'\t' read -r candidate_hash candidate_index_hex candidate_input; do
    [[ -n "$candidate_hash" ]] || continue
    candidate_index="$(cast to-dec "$candidate_index_hex")"
    (( candidate_index < user_index )) || continue
    decoded="$(cast decode-calldata --json "$load_sig" "$candidate_input")" || continue
    key_count="$(jq --arg key "${computed_static_key,,}" \
        '[.[1][] | select((.[0] | tostring) == "0" and (.[1] | ascii_downcase) == $key)] | length' <<<"$decoded")"
    (( key_count <= 1 )) || die "matching static key occurs more than once in a table"
    match_count="$(jq \
        --arg key "${computed_static_key,,}" --arg result "${expected_return,,}" '
          [.[1][] | select(
            ((.[0] | tostring) == "0") and
            ((.[1] | ascii_downcase) == $key) and
            ((.[2] | length) == 0) and
            ((.[3] | ascii_downcase) == "0x0000000000000000000000000000000000000000000000000000000000000000") and
            (.[4] == true) and
            ((.[5] | ascii_downcase) == $result)
          )] | length' <<<"$decoded")"
    [[ "$match_count" == "1" ]] || continue
    matches+=("$candidate_hash $candidate_index_hex")
done < <(jq -r --arg manager "${MANAGER_L2,,}" --arg selector "${load_selector,,}" '
    .transactions[]
    | select(((.to // "") | ascii_downcase) == $manager)
    | select(((.input // "") | ascii_downcase) | startswith($selector))
    | [.hash, .transactionIndex, .input] | @tsv
    ' <<<"$block_json")
(( ${#matches[@]} == 1 )) || die "expected one exact static table load before the trigger, found ${#matches[@]}"
read -r load_hash load_index_hex <<<"${matches[0]}"
load_index="$(cast to-dec "$load_index_hex")"
# Both entry points replace the table. A matching old load is not proof the
# trigger used it if another system transaction overwrote it in between.
incoming_selector="0x$(jq -er '.methodIdentifiers | to_entries[] | select(.key | startswith("executeIncomingCrossChainCall(")) | .value' \
    "$REPO/eez-core-protocol/out/EEZL2.sol/EEZL2.json")"
jq -e --arg manager "${MANAGER_L2,,}" --arg load "${load_selector,,}" \
    --arg incoming "${incoming_selector,,}" --argjson start "$load_index" --argjson end "$user_index" '
    [.transactions[($start + 1):$end][] | select((.to // "" | ascii_downcase) == $manager)
      | select((.input | ascii_downcase) | startswith($load) or startswith($incoming))] | length == 0
    ' <<<"$block_json" >/dev/null || die "another table replaced the static lookup before its trigger"
load_receipt="$(cast rpc eth_getTransactionReceipt "$load_hash" --rpc-url "$L2_RPC")"
[[ "$(jq -r '.status' <<<"$load_receipt")" == "0x1" ]] || die "loadExecutionTable reverted"
jq -en --argjson block "$block_json" --argjson receipt "$load_receipt" --arg hash "$load_hash" \
    --arg index "$load_index_hex" '
    $receipt.transactionHash == $hash and $receipt.blockHash == $block.hash
    and $receipt.transactionIndex == $index and $receipt.blockNumber == $block.number
    ' >/dev/null || die "table-load receipt is not in the canonical trigger block"

last_read="$(cast call "$reader_l2" 'lastRead()(uint256)' --block "$block_number" --rpc-url "$L2_RPC")"
reader_counter="$(cast call "$reader_l2" 'counter()(uint256)' --block "$block_number" --rpc-url "$L2_RPC")"
l1_counter="$(cast call "$counter_l1" 'counter()(uint256)' --rpc-url "$L1_RPC")"
[[ "$last_read" == "$predicted_value" ]] || die "reader.lastRead differs from cached return"
[[ "$reader_counter" == "1" ]] || die "reader counter did not advance exactly once"
[[ "$l1_counter" == "$predicted_value" ]] || die "L1 producer differs from the real read result"

settlement_receipt="$(cast rpc eth_getTransactionReceipt "$settlement_hash" --rpc-url "$L1_RPC")"
[[ "$(jq -r '.status' <<<"$settlement_receipt")" == "0x1" ]] || die "L1 settlement reverted"
settlement_tx="$(cast rpc eth_getTransactionByHash "$settlement_hash" --rpc-url "$L1_RPC")"
[[ "$(jq -r '.to | ascii_downcase' <<<"$settlement_tx")" == "${ROLLUPS,,}" ]] \
    || die "settlement target differs from registry"
settlement_block="$(cast rpc eth_getBlockByNumber "$(jq -er '.blockNumber' <<<"$settlement_tx")" true --rpc-url "$L1_RPC")"
jq -en --argjson block "$settlement_block" --argjson tx "$settlement_tx" --argjson receipt "$settlement_receipt" --arg hash "$settlement_hash" '
    $tx.hash == $hash and $tx.hash == $receipt.transactionHash and $receipt.blockHash == $block.hash and $tx.blockHash == $block.hash
    and $receipt.transactionIndex == $tx.transactionIndex and $receipt.blockNumber == $tx.blockNumber
    and $receipt.blockNumber == $block.number
    and ([$block.transactions[] | select(.hash == $tx.hash and .transactionIndex == $tx.transactionIndex
           and .input == $tx.input and .to == $tx.to)] | length) == 1
    ' >/dev/null || die "L1 settlement is not canonical"

tx_object() {
    local role="$1" chain="$2" hash="$3" receipt="$4" tx="$5"
    local object_block object_index
    object_block="$(cast to-dec "$(jq -r '.blockNumber' <<<"$receipt")")"
    object_index="$(cast to-dec "$(jq -r '.transactionIndex' <<<"$receipt")")"
    jq -cn --arg role "$role" --arg chain "$chain" --arg hash "$hash" \
        --argjson receipt "$receipt" --argjson tx "$tx" \
        --argjson block_number "$object_block" --argjson transaction_index "$object_index" '
        {role: $role, chain: $chain, hash: $hash, status: $receipt.status,
         block_number: $block_number, block_hex: $receipt.blockNumber,
         transaction_index: $transaction_index, to: $tx.to, input: $tx.input}'
}
trigger_object="$(tx_object user_trigger L2 "$tx_hash" "$trigger_receipt" "$trigger_tx")"
load_tx="$(cast rpc eth_getTransactionByHash "$load_hash" --rpc-url "$L2_RPC")"
jq -en --argjson block "$block_json" --argjson tx "$load_tx" --argjson receipt "$load_receipt" --arg hash "$load_hash" '
    $tx.hash == $hash and $tx.blockHash == $block.hash and $tx.blockNumber == $block.number
    and $tx.transactionIndex == $receipt.transactionIndex
    and ([$block.transactions[] | select(.hash == $tx.hash and .transactionIndex == $tx.transactionIndex
           and .input == $tx.input and .to == $tx.to)] | length) == 1
    ' >/dev/null || die "table-load transaction differs from the canonical block"
load_object="$(tx_object static_table_load L2 "$load_hash" "$load_receipt" "$load_tx")"
settlement_object="$(tx_object settlement L1 "$settlement_hash" "$settlement_receipt" "$settlement_tx")"
proofs_json="$(sed -n 's/^[[:space:]]*\(PASS: .*$\)/\1/p' "$SCENARIO_LOG" | jq -R . | jq -s .)"
log_sha256="$(sha256sum "$SCENARIO_LOG" | awk '{print $1}')"

causal_tmp="$(mktemp -d /tmp/eez-static-causal-evidence.XXXXXX)"
trap 'rm -rf "$causal_tmp"' EXIT
jq -n \
    --arg direction L2_to_L1 --arg registry "$ROLLUPS" --arg manager_l2 "$MANAGER_L2" \
    --argjson rollup_id "${ROLLUP_ID:-1}" --argjson select_static true \
    --arg l1_calldata "$(jq -r '.input' <<<"$settlement_tx")" \
    --arg l2_calldata "$(jq -r '.input' <<<"$load_tx")" \
    --arg expected_l2_static_key "$computed_static_key" \
    --argjson l1_receipts "[$settlement_receipt]" \
    --argjson l2_receipts "[$trigger_receipt,$load_receipt]" \
    '{direction: $direction, registry: $registry, manager_l2: $manager_l2,
      rollup_id: $rollup_id, select_static: $select_static,
      l1_calldata: $l1_calldata, l2_calldata: $l2_calldata,
      expected_l2_static_key: $expected_l2_static_key,
      expected_l2_entry_hashes: [], l1_receipts: $l1_receipts, l2_receipts: $l2_receipts}' \
    >"$causal_tmp/input.json"
mkdir -p "$(dirname "$OUTPUT_JSON")"
cp "$causal_tmp/input.json" "${OUTPUT_JSON%.json}.causal-input.json"
if ! "$CAUSAL_VERIFIER" "$causal_tmp/input.json" "$causal_tmp/output.json"; then
    die "independent L2-static/L1-host causal verification failed"
fi
causal_graph="$(jq -c . "$causal_tmp/output.json")"

blob_evidence_file="$causal_tmp/blob-evidence.json"
sidecars_file="${OUTPUT_JSON%.json}.blob-sidecars.json"
if ! L1_RPC="$L1_RPC" L2_RPC="$L2_RPC" BEACON_URL="$BEACON_URL" \
    BLOB_VERIFIER="$BLOB_VERIFIER" ROLLUP_ID="$ROLLUP_ID" \
    bash "$(dirname "$0")/verify-semantic-settlement-blob.sh" \
        "$settlement_hash" L2 "$blob_evidence_file" "$sidecars_file" "$tx_hash"; then
    die "settlement blob semantic verification failed"
fi
blob_da="$(jq -c . "$blob_evidence_file")"
[[ "$(cast block "$block_number" --field hash --rpc-url "$L2_RPC")" == "$(jq -r '.hash' <<<"$block_json")" ]] \
    || die "L2 changed during evidence collection"
[[ "$(cast block "$(jq -r '.number' <<<"$settlement_block")" --field hash --rpc-url "$L1_RPC")" == "$(jq -r '.hash' <<<"$settlement_block")" ]] \
    || die "L1 changed during evidence collection"

mkdir -p "$(dirname "$OUTPUT_JSON")"
tmp_output="${OUTPUT_JSON}.tmp"
jq -n \
    --argjson schema_version 2 --arg scenario staticCounterL2 \
    --arg scenario_path script/e2e/static/L2_to_L1/staticCounterL2/E2EStaticCounterL2.s.sol \
    --arg protocol_commit "$PROTOCOL_COMMIT" --arg result PASS \
    --arg verification_mode upstream-static-pool \
    --arg static_key "$computed_static_key" --arg cached_return "$expected_return" \
    --argjson predicted_value "$predicted_value" \
    --argjson trigger "$trigger_object" --argjson load "$load_object" \
    --argjson settlement "$settlement_object" --argjson proof_statements "$proofs_json" \
    --argjson causal_graph "$causal_graph" \
    --argjson blob_da "$blob_da" \
    --arg log "$SCENARIO_LOG" --arg log_sha256 "$log_sha256" \
    '{schema_version: $schema_version, scenario: $scenario, scenario_path: $scenario_path,
      protocol_commit: $protocol_commit, result: $result, verification_mode: $verification_mode,
      trigger_chain: "L2", trigger_transactions: [$trigger],
      settlement_transactions: [$settlement], system_transactions: [$load],
      static_entry: {proxy_entry_hash: $static_key, incoming_calls: 0,
                     rolling_hash: "0x0000000000000000000000000000000000000000000000000000000000000000",
                     success: true, return_data: $cached_return},
      causal_graph: $causal_graph,
      blob_da: $blob_da,
      observations: {predicted_value: $predicted_value, reader_last_read: $predicted_value,
                     reader_counter: 1, producer_counter: $predicted_value},
      claims: {upstream_pass_required: true, receipts_revalidated: true,
               causal_graph_bijection: true,
               blob_kzg_authenticated: true, native_semantics_decoded: true,
               semantic_trigger_bound: true,
               exact_same_block_static_table_decoded: true, table_load_preceded_trigger: true,
               l1_settlement_verified: true, posted_table_field_match: true,
               rolling_hashes_replayed: true, producer_remained_read_only: true},
      proof_statements: $proof_statements,
      source_log: {path: $log, sha256: $log_sha256}}' >"$tmp_output"
mv "$tmp_output" "$OUTPUT_JSON"

cat <<EVIDENCE
STATIC_COUNTER_L2_TX=$tx_hash
STATIC_COUNTER_L2_BLOCK=$block_number
STATIC_COUNTER_L2_LOAD_TX=$load_hash
STATIC_COUNTER_L2_SETTLEMENT_TX=$settlement_hash
PASS: exact static entry selected from the canonical L2 table and independently keyed
PASS: loadExecutionTable preceded the trigger in the same block ($load_index < $user_index)
PASS: reader persisted the static result $predicted_value
PASS: exact L1 settlement and rolling hashes were verified
EVIDENCE
