#!/usr/bin/env bash
# Read-only evidence for upstream's two L1 lookup-only scenarios. Neither has
# an L2 delivery to pair: bind the real L1 result/table to its semantic blob,
# then positively check absence over the authenticated L2 settlement range.
set -euo pipefail

SCENARIO="${1:?usage: verify-l1-lookup.sh <scenario> <sol> <log> <output-json>}"
SCENARIO_SOL="${2:?missing scenario path}"
SCENARIO_LOG="${3:?missing scenario log}"
OUTPUT_JSON="${4:?missing output path}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
for var in L1_RPC L2_RPC ROLLUPS MANAGER_L2 PROTOCOL_COMMIT ROLLUP_ID BEACON_URL BLOB_VERIFIER; do
    [[ -n "${!var:-}" ]] || { echo "missing $var" >&2; exit 1; }
done
die() { echo "$SCENARIO evidence error: $*" >&2; exit 1; }
field() { sed -n "s/^$1=//p" "$SCENARIO_LOG" | tail -1; }
grep -Fq '====== Done ======' "$SCENARIO_LOG" || die "upstream did not complete"
! grep -q '^ERROR:' "$SCENARIO_LOG" || die "upstream recorded an error"
grep -Fq 'PASS: none of 1 absent key(s) appears on L2' "$SCENARIO_LOG" \
    || die "upstream absence verification is absent"

case "$SCENARIO" in
    topLevelStaticCounter)
        grep -Fq 'PASS: posted batch static entries match expected static table' "$SCENARIO_LOG" \
            || die "upstream static-table verification is absent"
        source="$(field READER_L1)"; destination="$(field COUNTER_L2)"
        result="$(field PREDICTED_STATIC_RESULT)"
        [[ "$result" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid predicted result"
        trigger_sig='increment()'; call_sig='counter()'; static=true; kind=StaticCall; success=true
        ;;
    revertCounter)
        grep -Fq 'PASS: posted batch calldata matches expected table' "$SCENARIO_LOG" \
            || die "upstream table verification is absent"
        source="$(field SAFE_CALLER)"; destination="$(field REV_COUNTER_L2)"
        result="$(cast calldata 'Error(string)' 'always reverts')"
        trigger_sig='incrementProxy()'; call_sig='increment()'; static=false; kind=Call; success=false
        # This scenario has no EXPECTED_L1_STEPS, so upstream does not replay
        # rolling hashes. Independently check the posted hash below instead.
        ;;
    *) die "unsupported lookup scenario" ;;
esac
for address in "$source" "$destination"; do
    [[ "$address" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid scenario address"
done
trigger_hash="$(sed -n -E 's/^User tx:[[:space:]]*(0x[[:xdigit:]]{64}).*/\1/p' "$SCENARIO_LOG")"
post_hash="$(sed -n -E 's/^L1 postBatch:[[:space:]]*(0x[[:xdigit:]]{64}).*/\1/p' "$SCENARIO_LOG")"
for hash in "$trigger_hash" "$post_hash"; do
    [[ "$hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "expected one trigger and one settlement hash"
done
key="$(cast call "$ROLLUPS" \
    'computeCrossChainCallHash(bool,address,uint64,address,uint64,uint256,uint64,bytes)(bytes32)' \
    "$static" "$source" 0 "$destination" "$ROLLUP_ID" 0 0 "$(cast sig "$call_sig")" --rpc-url "$L1_RPC")"
[[ "$key" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid lookup key"

# Receipt existence is not enough: old orphan receipts can remain queryable.
# Bind both txs to their positions in the same current canonical block.
trigger="$(cast rpc eth_getTransactionByHash "$trigger_hash" --rpc-url "$L1_RPC")"
post="$(cast rpc eth_getTransactionByHash "$post_hash" --rpc-url "$L1_RPC")"
trigger_receipt="$(cast rpc eth_getTransactionReceipt "$trigger_hash" --rpc-url "$L1_RPC")"
post_receipt="$(cast rpc eth_getTransactionReceipt "$post_hash" --rpc-url "$L1_RPC")"
block_hex="$(jq -er '.blockNumber' <<<"$trigger_receipt")"
block="$(cast rpc eth_getBlockByNumber "$block_hex" true --rpc-url "$L1_RPC")"
validate_receipt() {
    jq -en --argjson tx "$1" --argjson receipt "$2" --arg hash "$3" --argjson block "$block" '
      $receipt.status == "0x1" and $receipt.transactionHash == $hash and $tx.hash == $hash
      and $receipt.blockHash == $block.hash and $tx.blockHash == $block.hash
      and $receipt.blockNumber == $block.number and $tx.blockNumber == $block.number
      and $receipt.transactionIndex == $tx.transactionIndex
      and ([$block.transactions[] | select(.hash == $hash
            and .transactionIndex == $tx.transactionIndex and .input == $tx.input and .to == $tx.to)] | length) == 1
    ' >/dev/null || die "transaction $3 is not successful and canonical in the trigger block"
}
validate_receipt "$trigger" "$trigger_receipt" "$trigger_hash"
validate_receipt "$post" "$post_receipt" "$post_hash"
[[ "$(jq -r '.to | ascii_downcase' <<<"$trigger")" == "${source,,}" ]] || die "wrong trigger target"
[[ "$(jq -r '.input | ascii_downcase' <<<"$trigger")" == "$(cast sig "$trigger_sig")" ]] || die "wrong trigger calldata"
[[ "$(jq -r '.to | ascii_downcase' <<<"$post")" == "${ROLLUPS,,}" ]] || die "wrong settlement target"
post_index="$(cast to-dec "$(jq -er '.transactionIndex' <<<"$post")")"
trigger_index="$(cast to-dec "$(jq -er '.transactionIndex' <<<"$trigger")")"
(( post_index < trigger_index )) || die "settlement did not precede the trigger"

post_sig="$(jq -er '.methodIdentifiers | keys[] | select(startswith("postAndVerifyBatch("))' \
    "$REPO/eez-core-protocol/out/EEZ.sol/EEZ.json")"
post_selector="$(cast sig "$post_sig")"
# A later verification replaces queues. These lookup scenarios must
# resolve the batch we authenticated, not just catch ExecutionNotFound after
# another batch displaced it. Check block-array order, not lexicographic hex.
jq -e --arg registry "${ROLLUPS,,}" --arg selector "$post_selector" \
    --argjson start "$post_index" --argjson end "$trigger_index" '
    [.transactions[($start + 1):$end][] | select((.to // "" | ascii_downcase) == $registry)
      | select(.input | startswith($selector))] | length == 0
    ' <<<"$block" >/dev/null || die "another batch replaced the lookup before its trigger"
decoded="$(cast decode-calldata --json "$post_sig" "$(jq -er '.input' <<<"$post")")"
entry="$(jq -ce --arg scenario "$SCENARIO" --arg key "${key,,}" --arg result "${result,,}" \
    --argjson rollup "$ROLLUP_ID" -f "$HERE/lookup-entry.jq" <<<"$decoded")" \
    || die "posted lookup has the wrong identity, result, root transition or nested calls"
if [[ "$static" == false ]]; then
    # With no calls/nested actions the rolling trace is exactly entryBegin.
    zero="0x$(printf '%064d' 0)"
    states_hash="$(cast keccak "$(cast abi-encode --packed 'f(bytes32,uint64,bytes32)' \
        "$zero" "$ROLLUP_ID" "$(jq -r '.root' <<<"$entry")")")"
    rolling_hash="$(cast keccak "$(cast abi-encode --packed 'f(bytes32,bytes32)' "$states_hash" "$key")")"
    [[ "$rolling_hash" == "$(jq -r '.rolling_hash' <<<"$entry")" ]] || die "wrong empty-call rolling hash"
fi

# A shared queue can contain many unrelated entries. Prove the expected call
# actually resolved: caller.lastCallFailed alone also accepts a wrong lookup.
proxy="$(cast call "$ROLLUPS" 'computeCrossChainProxyAddress(address,uint64)(address)' \
    "$destination" "$ROLLUP_ID" --rpc-url "$L1_RPC")"
[[ "$proxy" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid destination proxy"
lookup_method='executeCrossChainCall(address,bytes)'
trace_output="$result"
if [[ "$static" == true ]]; then
    lookup_method='staticCrossChainCall(address,bytes)'
    trace_output="$(cast abi-encode 'f(bytes)' "$result")"
fi
lookup_input="$(cast calldata "$lookup_method" "$source" "$(cast sig "$call_sig")")"
mkdir -p "$(dirname "$OUTPUT_JSON")"
trace_file="${OUTPUT_JSON%.json}.lookup-trace.json"
cast rpc debug_traceTransaction "$trigger_hash" '{"tracer":"callTracer","timeout":"30s"}' \
    --rpc-url "$L1_RPC" >"$trace_file" || die "canonical lookup trace unavailable"
trace_evidence="$(jq -ce --arg source "${source,,}" --arg registry "${ROLLUPS,,}" \
    --arg proxy "${proxy,,}" --arg trigger_input "$(cast sig "$trigger_sig")" \
    --arg lookup_input "${lookup_input,,}" --arg output "${trace_output,,}" --argjson static "$static" \
    --arg static_check_gas_input "$(cast sig 'STATIC_CHECK_GAS()')" \
    -f "$HERE/lookup-trace.jq" "$trace_file")" || die "actual registry lookup returned the wrong result/revert"

[[ "$(cast call "$source" 'counter()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == 1 ]] \
    || die "caller did not execute exactly once"
if [[ "$static" == true ]]; then
    predicted="$(cast to-dec "$result")"
    [[ "$(cast call "$source" 'lastRead()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == "$predicted" ]] \
        || die "reader persisted the wrong result"
    [[ "$(cast call "$destination" 'counter()(uint256)' --rpc-url "$L2_RPC")" == "$predicted" ]] \
        || die "L2 producer changed"
else
    [[ "$(cast call "$source" 'lastCallFailed()(bool)' --block "$block_hex" --rpc-url "$L1_RPC")" == true ]] \
        || die "caller did not catch the revert"
    [[ "$(cast call "$source" 'targetCounter()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == 0 ]] \
        || die "caller recorded a successful target result"
fi

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
bash "$HERE/verify-semantic-settlement-blob.sh" "$post_hash" L1 \
    "$scratch/blob.json" "${OUTPUT_JSON%.json}.blob-sidecars.json" "$trigger_hash"
# The exact trigger's signed semantics must contain this one lookup action,
# not an unrelated successful transaction in an otherwise valid blob.
jq -e --arg source "${source,,}" --arg destination "${destination,,}" --arg kind "$kind" \
    --arg data "$(cast sig "$call_sig")" --arg result "${result,,}" --argjson success "$success" \
    --argjson rollup "$ROLLUP_ID" '
    .semantic_evidence.raw_trigger_transactions[0].raw_transaction as $raw
    | [.payload.semantic_transactions[] | select(.tx_data == $raw)]
    | length == 1 and (.[0].origin_chain == 0 and (.[0].actions | length) == 1
      and (.[0].actions[0] | .parent_index == null and .kind == $kind and .source_chain == 0
           and .target_chain == $rollup and (.from_address | ascii_downcase) == $source
           and (.to_address | ascii_downcase) == $destination and .value == "0" and .data == $data
           and .success == $success and (.return_data | ascii_downcase) == $result and .revert_span == null))
    ' "$scratch/blob.json" >/dev/null || die "lookup is not bound to the authenticated semantic action"
# Wait for the proven L2 window to be available, then check the blob's block
# commitments against the canonical chain before scanning ALL manager activity.
# Derived records intentionally omit intermediate hashes: the authenticated
# terminal hash commits to their ancestry, while any full records still expose
# hashes that can be checked directly. Do not accept a timeout as evidence of
# absence.
last="$(jq -er '.payload.blocks[-1].number' "$scratch/blob.json")"
first="$(jq -er '.payload.blocks[0].number' "$scratch/blob.json")"
jq -e '
  .verified == true and (.payload.blocks | length) > 0
  and (.payload.blocks[-1].representation == "terminal")
  and (.payload.blocks[-1].hash | test("^0x[0-9a-fA-F]{64}$"))
  and ([.payload.blocks[] | select(.representation == "terminal")] | length) == 1
  and (.payload.blocks as $blocks
       | all(range(0; $blocks | length);
             $blocks[.].number == ($blocks[0].number + .)))
  and all(.payload.blocks[];
      if .representation == "derived" then
        (.timestamp_step_seconds | type == "number" and . > 0)
        and (.transaction_count | type == "number" and . >= 0)
      elif (.representation == "full" or .representation == "terminal") then
        (.hash | test("^0x[0-9a-fA-F]{64}$"))
      else false end)
' "$scratch/blob.json" >/dev/null || die "authenticated blob has an invalid block window"
deadline=$((SECONDS + 120))
while (( $(cast block-number --rpc-url "$L2_RPC") < last )); do
    (( SECONDS < deadline )) || die "L2 has not reached the authenticated settlement window"
    sleep 2
done
while read -r number hash; do
    [[ "$(cast block "$number" --field hash --rpc-url "$L2_RPC")" == "$hash" ]] \
        || die "blob block $number is not canonical on L2"
done < <(jq -r '.payload.blocks[] | select(.hash != null) | "\(.number) \(.hash)"' "$scratch/blob.json")
root_file="${OUTPUT_JSON%.json}.lookup-root.json"
bash "$HERE/verify-lookup-root.sh" "$scratch/blob.json" "$(jq -er '.root' <<<"$entry")" "$root_file" \
    || die "lookup root is not bound to the authenticated L2 execution"
end="$(cast block-number --rpc-url "$L2_RPC")"
(
    cd "$REPO/eez-core-protocol"
    forge script script/e2e/shared/Verify.s.sol:VerifyL2Absent --rpc-url "$L2_RPC" \
        --sig 'run(uint256,uint256,address,bytes32[])' "$first" "$end" "$MANAGER_L2" "[$key]"
) >"$scratch/absence.log" 2>&1 || { cat "$scratch/absence.log" >&2; die "lookup appeared on L2"; }
# Detect a reorg while collecting evidence; no retroactive eth_call of the
# static pool is used because later batches may legitimately replace its pins.
[[ "$(cast block "$block_hex" --field hash --rpc-url "$L1_RPC")" == "$(jq -r '.hash' <<<"$block")" ]] \
    || die "L1 changed during evidence collection"
[[ "$(cast block "$last" --field hash --rpc-url "$L2_RPC")" == "$(jq -r '.payload.blocks[-1].hash' "$scratch/blob.json")" ]] \
    || die "L2 changed during evidence collection"

mkdir -p "$(dirname "$OUTPUT_JSON")"
jq -n --arg scenario "$SCENARIO" --arg scenario_path "$SCENARIO_SOL" --arg protocol_commit "$PROTOCOL_COMMIT" \
    --arg log "$SCENARIO_LOG" --arg log_sha256 "$(sha256sum "$SCENARIO_LOG" | awk '{print $1}')" \
    --argjson trigger "$trigger" --argjson post "$post" --argjson entry "$entry" \
    --argjson lookup_trace "$trace_evidence" --slurpfile root_evidence "$root_file" \
    --argjson first "$first" --argjson end "$end" --slurpfile blob "$scratch/blob.json" \
    '{schema_version: 2, scenario: $scenario, scenario_path: $scenario_path, protocol_commit: $protocol_commit,
      result: "PASS", verification_mode: "upstream-lookup-only", trigger_chain: "L1",
      trigger_transactions: [$trigger], settlement_transactions: [$post], system_transactions: [],
      lookup_entry: $entry, lookup_trace: $lookup_trace, lookup_root: $root_evidence[0], blob_da: $blob[0],
      causal_graph: {verified: true, direction: "L1_to_L2", mode: "lookup_only_no_destination", pairs: [],
                     l2_absence_range: {from: $first, to: $end}},
      claims: {upstream_pass_required: true, receipts_revalidated: true, post_batch_preceded_trigger: true,
               no_destination_delivery_required: true, l2_absence_verified: true,
               blob_kzg_authenticated: true, native_semantics_decoded: true, semantic_trigger_bytes_bound: true,
               actual_lookup_result_traced: true, lookup_root_bound: true},
      source_log: {path: $log, sha256: $log_sha256}}' >"${OUTPUT_JSON}.tmp"
mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
echo "PASS: $SCENARIO canonical L1 lookup, authenticated semantics and L2 absence ($first..$end) verified"
