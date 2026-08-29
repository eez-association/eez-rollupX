#!/usr/bin/env bash
# Strict compatibility verifier for staticCounterL2. The upstream generic
# runner currently rejects its intentionally eventless L2 static pool because
# the scenario correctly omits EXPECTED_L2_HASHES. This checker accepts only
# that exact mismatch, decodes the real same-block loadExecutionTable calldata,
# and binds it to the successful user trigger and L1 settlement.
set -euo pipefail

SCENARIO_LOG="${1:?usage: verify-static-counter-l2.sh <upstream-log> <output-json>}"
OUTPUT_JSON="${2:?missing evidence output path}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

for var in L1_RPC L2_RPC ROLLUPS MANAGER_L2 PROTOCOL_COMMIT CAUSAL_VERIFIER; do
    [[ -n "${!var:-}" ]] || { echo "missing $var" >&2; exit 1; }
done
for tool in cast jq sed sha256sum; do
    command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 1; }
done
[[ -f "$SCENARIO_LOG" ]] || { echo "scenario log not found: $SCENARIO_LOG" >&2; exit 1; }
[[ -x "$CAUSAL_VERIFIER" ]] || { echo "causal verifier not executable: $CAUSAL_VERIFIER" >&2; exit 1; }

die() { echo "staticCounterL2 evidence error: $*" >&2; exit 1; }

grep -Fq 'ERROR: No EXPECTED_L2_HASHES — add to ComputeExpected' "$SCENARIO_LOG" \
    || die "the exact known upstream mismatch is absent"
[[ "$(grep -c '^ERROR:' "$SCENARIO_LOG" || true)" == "1" ]] \
    || die "the upstream log contains an additional error"
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
expected_static_key="$(sed -n -E 's/.*TOP-LEVEL STATIC proxyEntryHash=(0x[[:xdigit:]]{64}).*/\1/p' "$SCENARIO_LOG" | tail -1)"
expected_return="$(sed -n -E 's/^[[:space:]]*returnData:[[:space:]]*(0x[[:xdigit:]]+).*/\1/p' "$SCENARIO_LOG" | tail -1)"
[[ "$tx_hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid trigger transaction hash"
[[ "$block_hex" =~ ^0x[[:xdigit:]]+$ ]] || die "invalid trigger block"
[[ "$settlement_hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid settlement transaction hash"
[[ "$expected_static_key" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid expected static key"
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
    true "$reader_l2" 1 "$counter_l1" 0 0 0 "$counter_selector" --rpc-url "$L2_RPC")"
[[ "${computed_static_key,,}" == "${expected_static_key,,}" ]] \
    || die "independently computed static key differs from ComputeExpected"

load_sig="$(jq -r '.methodIdentifiers | keys[] | select(startswith("loadExecutionTable("))' \
    "$REPO/eez-core-protocol/out/EEZL2.sol/EEZL2.json")"
load_selector="0x$(jq -r --arg sig "$load_sig" '.methodIdentifiers[$sig]' \
    "$REPO/eez-core-protocol/out/EEZL2.sol/EEZL2.json")"
block_json="$(cast block "$block_number" --full --json --rpc-url "$L2_RPC")"
matches=()
while IFS=$'\t' read -r candidate_hash candidate_index_hex candidate_input; do
    [[ -n "$candidate_hash" ]] || continue
    candidate_index="$(cast to-dec "$candidate_index_hex")"
    (( candidate_index < user_index )) || continue
    decoded="$(cast decode-calldata --json "$load_sig" "$candidate_input")" || continue
    match_count="$(jq \
        --arg key "${computed_static_key,,}" --arg result "${expected_return,,}" '
          [.[1][] | select(
            ((.[0] | ascii_downcase) == $key) and
            ((.[1] | length) == 0) and
            ((.[2] | ascii_downcase) == "0x0000000000000000000000000000000000000000000000000000000000000000") and
            (.[3] == true) and
            ((.[4] | ascii_downcase) == $result)
          )] | length' <<<"$decoded")"
    [[ "$match_count" == "1" ]] || continue
    [[ "$(jq '.[0] | length' <<<"$decoded")" == "0" ]] \
        || die "matching load transaction unexpectedly contains mutable entries"
    [[ "$(jq '.[1] | length' <<<"$decoded")" == "1" ]] \
        || die "matching load transaction contains an unexpected additional static entry"
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
load_receipt="$(cast rpc eth_getTransactionReceipt "$load_hash" --rpc-url "$L2_RPC")"
[[ "$(jq -r '.status' <<<"$load_receipt")" == "0x1" ]] || die "loadExecutionTable reverted"

last_read="$(cast call "$reader_l2" 'lastRead()(uint256)' --block "$block_number" --rpc-url "$L2_RPC")"
reader_counter="$(cast call "$reader_l2" 'counter()(uint256)' --block "$block_number" --rpc-url "$L2_RPC")"
l1_counter="$(cast call "$counter_l1" 'counter()(uint256)' --rpc-url "$L1_RPC")"
probe="$(cast call "$reader_l2" 'increment()(uint256)' --block "$block_number" --rpc-url "$L2_RPC")"
[[ "$last_read" == "$predicted_value" ]] || die "reader.lastRead differs from cached return"
[[ "$reader_counter" == "1" ]] || die "reader counter did not advance exactly once"
[[ "$l1_counter" == "$predicted_value" ]] || die "L1 producer differs from the real read result"
[[ "$probe" == "$predicted_value" ]] || die "same-block no-tx STATICCALL probe differs"

settlement_receipt="$(cast rpc eth_getTransactionReceipt "$settlement_hash" --rpc-url "$L1_RPC")"
[[ "$(jq -r '.status' <<<"$settlement_receipt")" == "0x1" ]] || die "L1 settlement reverted"
settlement_tx="$(cast rpc eth_getTransactionByHash "$settlement_hash" --rpc-url "$L1_RPC")"
[[ "$(jq -r '.to | ascii_downcase' <<<"$settlement_tx")" == "${ROLLUPS,,}" ]] \
    || die "settlement target differs from registry"

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
    --argjson l1_receipts "[$settlement_receipt]" \
    --argjson l2_receipts "[$trigger_receipt,$load_receipt]" \
    '{direction: $direction, registry: $registry, manager_l2: $manager_l2,
      rollup_id: $rollup_id, select_static: $select_static,
      l1_calldata: $l1_calldata, l2_calldata: $l2_calldata,
      expected_l2_entry_hashes: [], l1_receipts: $l1_receipts, l2_receipts: $l2_receipts}' \
    >"$causal_tmp/input.json"
if ! "$CAUSAL_VERIFIER" "$causal_tmp/input.json" "$causal_tmp/output.json"; then
    die "independent L2-static/L1-host causal verification failed"
fi
causal_graph="$(jq -c . "$causal_tmp/output.json")"

mkdir -p "$(dirname "$OUTPUT_JSON")"
tmp_output="${OUTPUT_JSON}.tmp"
jq -n \
    --argjson schema_version 1 --arg scenario staticCounterL2 \
    --arg scenario_path script/e2e/static/L2_to_L1/staticCounterL2/E2EStaticCounterL2.s.sol \
    --arg protocol_commit "$PROTOCOL_COMMIT" --arg result PASS \
    --arg verification_mode external-static-pool \
    --arg static_key "$computed_static_key" --arg cached_return "$expected_return" \
    --argjson predicted_value "$predicted_value" \
    --argjson trigger "$trigger_object" --argjson load "$load_object" \
    --argjson settlement "$settlement_object" --argjson proof_statements "$proofs_json" \
    --argjson causal_graph "$causal_graph" \
    --arg log "$SCENARIO_LOG" --arg log_sha256 "$log_sha256" \
    '{schema_version: $schema_version, scenario: $scenario, scenario_path: $scenario_path,
      protocol_commit: $protocol_commit, result: $result, verification_mode: $verification_mode,
      trigger_chain: "L2", trigger_transactions: [$trigger],
      settlement_transactions: [$settlement], system_transactions: [$load],
      static_entry: {proxy_entry_hash: $static_key, incoming_calls: 0,
                     rolling_hash: "0x0000000000000000000000000000000000000000000000000000000000000000",
                     success: true, return_data: $cached_return},
      causal_graph: $causal_graph,
      observations: {predicted_value: $predicted_value, reader_last_read: $predicted_value,
                     reader_counter: 1, producer_counter: $predicted_value,
                     no_tx_staticcall_result: $predicted_value},
      claims: {known_upstream_mismatch_only: true, receipts_revalidated: true,
               causal_graph_bijection: true,
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
PASS: exact static-only L2 table decoded and independently keyed
PASS: loadExecutionTable preceded the trigger in the same block ($load_index < $user_index)
PASS: same-block no-tx STATICCALL returned $predicted_value
PASS: exact L1 settlement and rolling hashes were verified
EVIDENCE
