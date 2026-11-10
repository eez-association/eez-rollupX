#!/usr/bin/env bash
# Read-only evidence for upstream's L1 lookup-only scenarios. None has an L2
# delivery to pair: bind the real L1 result/table to its semantic blob, then
# positively check absence over the authenticated L2 settlement range.
# topLevelStaticReentrantCounter's static entry also carries one static
# callback to L1, which L1 executes while resolving the lookup.
# staticLocalWrite resolves three reads under one key/root around local writes;
# its callback attempts include the candidates rejected by their result hash.
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
grep -Eq 'PASS: none of [1-9][0-9]* absent key\(s\) appears on L2' "$SCENARIO_LOG" \
    || die "upstream absence verification is absent"

semantic_trace=false
call_data=""
callback_source=""; callback_target=""; callback_proxy=""
result_two=""; callback_rolling_hash_two=""; callback_forward_input=""
case "$SCENARIO" in
    topLevelStaticCounter|topLevelStaticReentrantCounter)
        grep -Fq 'PASS: posted batch static entries match expected static table' "$SCENARIO_LOG" \
            || die "upstream static-table verification is absent"
        source="$(field READER_L1)"; destination="$(field COUNTER_L2)"
        result="$(field PREDICTED_STATIC_RESULT)"
        [[ "$result" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid predicted result"
        trigger_sig='increment()'; call_sig='counter()'; static=true; kind=StaticCall; success=true
        if [[ "$SCENARIO" == topLevelStaticReentrantCounter ]]; then
            # The L2 forwarder's helper reads the L1 producer back through
            # CALLBACK_PROXY; the producer's value is also the read result.
            callback_source="$(field CALLBACK_L2)"; callback_target="$(field COUNTER_L1)"
            callback_proxy="$(field CALLBACK_PROXY)"
        fi
        ;;
    staticLocalWrite)
        grep -Fq 'PASS: posted batch static entries match expected static table' "$SCENARIO_LOG" \
            || die "upstream static-table verification is absent"
        source="$(field READER)"; destination="$(field QUOTE)"
        # The scenario requires 1 -> 2 -> 1, independently of the prediction
        # text or saved results supplied by the Composer.
        result="$(cast abi-encode 'f(uint256)' 1)"
        result_two="$(cast abi-encode 'f(uint256)' 2)"
        trigger_sig='run()'; call_sig='quote()'; static=true; kind=StaticCall; success=true
        callback_source="$destination"; callback_target="$source"
        callback_proxy="$(cast call "$ROLLUPS" 'computeCrossChainProxyAddress(address,uint64)(address)' \
            "$callback_source" "$ROLLUP_ID" --rpc-url "$L1_RPC")"
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
    catchRemoteRevertAfterCallback|staticReadCallsBackAndReadsAgain)
        semantic_trace=true
        source="$(field ACTOR_L1)"; destination="$(field ACTOR_L2)"
        grep -Fq 'PASS: posted batch calldata matches expected table' "$SCENARIO_LOG" \
            || die "upstream table verification is absent"
        if [[ "$SCENARIO" == catchRemoteRevertAfterCallback ]]; then
            trigger_sig='runCatchRemoteRevertAfterCallback()'; call_sig='failAfterChild()'
            result="$(cast calldata 'FrameFailed(uint256)' 2)"; static=false; kind=Call; success=false
        else
            trigger_sig='runStaticReadCallsBackAndReadsAgain()'; call_sig='readChain(uint8)'
            call_data="$(cast calldata "$call_sig" 2)"
            result="$(cast abi-encode 'f(uint256)' 2)"; static=true; kind=StaticCall; success=true
            grep -Fq 'PASS: posted batch static entries match expected static table' "$SCENARIO_LOG" \
                || die "upstream static-table verification is absent"
        fi
        ;;
    topLevelStaticReentrantMissingProxy)
        semantic_trace=true
        source="$(field READER_L1)"; destination="$(field COUNTER_L2)"
        trigger_sig='increment()'; call_sig='counter()'
        result="$(cast abi-encode 'f(uint256)' 1)"; static=true; kind=StaticCall; success=true
        grep -Fq 'PASS: posted batch static entries match expected static table' "$SCENARIO_LOG" \
            || die "upstream static-table verification is absent"
        ;;
    *) die "unsupported lookup scenario" ;;
esac
for address in "$source" "$destination"; do
    [[ "$address" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid scenario address"
done
[[ -n "$call_data" ]] || call_data="$(cast sig "$call_sig")"
callback_sig="$call_sig"
[[ "$SCENARIO" != staticLocalWrite ]] || callback_sig='rate()'
callback_rolling_hash=""; proxy_query_input=""; proxy_query_output=""
if [[ -n "$callback_target" ]]; then
    for address in "$callback_source" "$callback_target" "$callback_proxy"; do
        [[ "$address" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid callback address"
    done
    [[ "$(cast call "$ROLLUPS" 'computeCrossChainProxyAddress(address,uint64)(address)' \
        "$callback_source" "$ROLLUP_ID" --rpc-url "$L1_RPC" | tr 'A-F' 'a-f')" == "${callback_proxy,,}" ]] \
        || die "CALLBACK_PROXY is not the registry's proxy for the callback source"
    # The untagged static fold over the one successful callback result:
    # keccak256(bytes32(0) || 0x01 || returnData).
    callback_rolling_hash="$(cast keccak "0x$(printf '%064d' 0)01${result#0x}")"
    if [[ "$SCENARIO" == staticLocalWrite ]]; then
        callback_rolling_hash_two="$(cast keccak "0x$(printf '%064d' 0)01${result_two#0x}")"
        callback_forward_input="$(cast calldata 'executeOnBehalf(address,uint64,bytes)' \
            "$callback_target" 0 "$(cast sig "$callback_sig")")"
    else
        # The round-trip reader queries the callback proxy before its read.
        # staticLocalWrite has no such query in its source transaction.
        proxy_query_input="$(cast calldata 'computeCrossChainProxyAddress(address,uint64)' \
            "$callback_source" "$ROLLUP_ID")"
        proxy_query_output="$(cast abi-encode 'f(address)' "$callback_proxy")"
    fi
fi
trigger_hash="$(sed -n -E 's/^User tx:[[:space:]]*(0x[[:xdigit:]]{64}).*/\1/p' "$SCENARIO_LOG")"
post_hash="$(sed -n -E 's/^L1 postBatch:[[:space:]]*(0x[[:xdigit:]]{64}).*/\1/p' "$SCENARIO_LOG")"
for hash in "$trigger_hash" "$post_hash"; do
    [[ "$hash" =~ ^0x[[:xdigit:]]{64}$ ]] || die "expected one trigger and one settlement hash"
done
key="$(cast call "$ROLLUPS" \
    'computeCrossChainCallHash(bool,address,uint64,address,uint64,uint256,uint64,bytes)(bytes32)' \
    "$static" "$source" 0 "$destination" "$ROLLUP_ID" 0 0 "$call_data" --rpc-url "$L1_RPC")"
[[ "$key" =~ ^0x[[:xdigit:]]{64}$ ]] || die "invalid lookup key"
if [[ "$SCENARIO" == staticLocalWrite ]]; then
    local_key="$(cast keccak "$(cast abi-encode \
        'f(bool,address,uint64,address,uint64,uint256,uint64,bytes)' \
        true "$source" 0 "$destination" "$ROLLUP_ID" 0 0 "$call_data")")"
    [[ "${key,,}" == "${local_key,,}" ]] || die "lookup identity differs from canonical ABI hash"
fi

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
mkdir -p "$(dirname "$OUTPUT_JSON")"
trace_file="${OUTPUT_JSON%.json}.lookup-trace.json"
cast rpc debug_traceTransaction "$trigger_hash" '{"tracer":"callTracer","timeout":"30s"}' \
    --rpc-url "$L1_RPC" >"$trace_file" || die "canonical lookup trace unavailable"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
bash "$HERE/verify-semantic-settlement-blob.sh" "$post_hash" L1 \
    "$scratch/blob.json" "${OUTPUT_JSON%.json}.blob-sidecars.json" "$trigger_hash"
if [[ "$semantic_trace" == true ]]; then
    bracket="$(jq -ce --arg source "${source,,}" '
      if .verified != true or .semantic_evidence.semantics_bound_to_settlement != true
      then error("unauthenticated semantics") else . end
      | [.payload.semantic_transactions[] | select(.origin_chain == 0 and .tx_data == "0x")
          | select(any(.actions[]; .parent_index == null and (.from_address | ascii_downcase) == $source))]
      | if length == 1 then .[0] else error("ambiguous trigger semantics") end
    ' "$scratch/blob.json")" || die "lookup has no unique authenticated semantic bracket"
    jq -e --arg source "${source,,}" --arg destination "${destination,,}" --arg data "${call_data,,}" \
        --arg result "${result,,}" --arg kind "$kind" --argjson success "$success" '
        [.actions[] | select(.parent_index == null)] | length == 1
        and (.[0] | (.from_address | ascii_downcase) == $source
             and (.to_address | ascii_downcase) == $destination and .data == $data
             and .kind == $kind and .success == $success and .return_data == $result)
    ' <<<"$bracket" >/dev/null || die "root action differs from the pinned scenario"
    proxies='{}'
    while IFS= read -r actor; do
        proxy="$(cast call "$ROLLUPS" 'computeCrossChainProxyAddress(address,uint64)(address)' \
            "$actor" "$ROLLUP_ID" --block "$block_hex" --rpc-url "$L1_RPC")"
        [[ "$proxy" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid registry-derived proxy"
        proxies="$(jq -c --arg actor "${actor,,}" --arg proxy "${proxy,,}" '. + {($actor): $proxy}' <<<"$proxies")"
    done < <(jq -r '[.actions[] | if .source_chain == 0 then .to_address else .from_address end] | unique[]' <<<"$bracket")
    trace_input="${OUTPUT_JSON%.json}.lookup-input.json"
    jq -n --slurpfile trace "$trace_file" --argjson trigger "$trigger" --argjson bracket "$bracket" \
        --arg registry "${ROLLUPS,,}" --argjson rollup "$ROLLUP_ID" --argjson proxies "$proxies" \
        --argjson decoded "$decoded" \
        '{trace: $trace[0], trigger: $trigger, semantic_transaction: $bracket, registry: $registry,
          rollup_id: $rollup, proxies: $proxies, decoded_settlement: $decoded}' >"$trace_input"
    bound="$(python3 "$REPO/scripts/ci/verify-protocol-lookup-trace.py" "$trace_input")" \
        || die "canonical lookup trace or deferred entries differ from authenticated semantics"
    entry="$(jq -ce '.lookup_entry' <<<"$bound")"
    trace_evidence="$(jq -ce '.lookup_trace' <<<"$bound")"
    keys="$(jq -ce '.keys' <<<"$entry")"; roots="$(jq -ce '.roots' <<<"$entry")"
    expected_absence="$(jq 'length' <<<"$keys")"
    grep -Fq "PASS: none of $expected_absence absent key(s) appears on L2" "$SCENARIO_LOG" \
        || die "upstream did not verify every lookup key's absence"
    if [[ "$SCENARIO" == topLevelStaticReentrantMissingProxy ]]; then
        [[ "$(jq '.missing_proxy_probes | length' <<<"$trace_evidence")" == 1 ]] \
            || die "missing-proxy scenario did not execute its probe and retry"
        for read in 'counter:1' 'lastRead:1'; do
            [[ "$(cast call "$source" "${read%:*}()(uint256)" --block "$block_hex" --rpc-url "$L1_RPC")" == "${read#*:}" ]] \
                || die "reader persisted the wrong state"
        done
    else
        for read in 'completed:1' 'total:0'; do
            [[ "$(cast call "$source" "${read%:*}()(uint256)" --block "$block_hex" --rpc-url "$L1_RPC")" == "${read#*:}" ]] \
                || die "source persisted the wrong state"
        done
        if [[ "$SCENARIO" == catchRemoteRevertAfterCallback ]]; then
            [[ "$(cast call "$source" 'failure()(bytes)' --block "$block_hex" --rpc-url "$L1_RPC")" == "$result" ]] \
                || die "source persisted the wrong failure"
        else
            [[ "$(cast call "$source" 'firstRead()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == 2 ]] \
                || die "source persisted the wrong recursive read"
        fi
    fi
else
grep -Fq 'PASS: none of 1 absent key(s) appears on L2' "$SCENARIO_LOG" \
    || die "upstream absence verification is absent"
entry="$(jq -ce --arg scenario "$SCENARIO" --arg key "${key,,}" --arg result "${result,,}" \
    --arg callback_source "${callback_source,,}" --arg callback_target "${callback_target,,}" \
    --arg callback_data "$(cast sig "$callback_sig")" --arg callback_rolling_hash "${callback_rolling_hash,,}" \
    --arg result_two "${result_two,,}" --arg callback_rolling_hash_two "${callback_rolling_hash_two,,}" \
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
lookup_input="$(cast calldata "$lookup_method" "$source" "$call_data")"
trace_evidence="$(jq -ce --arg scenario "$SCENARIO" --arg source "${source,,}" --arg registry "${ROLLUPS,,}" \
    --arg proxy "${proxy,,}" --arg trigger_input "$(cast sig "$trigger_sig")" \
    --arg lookup_input "${lookup_input,,}" --arg output "${trace_output,,}" --argjson static "$static" \
    --arg static_check_gas_input "$(cast sig 'STATIC_CHECK_GAS()')" \
    --arg callback_proxy "${callback_proxy,,}" --arg callback_target "${callback_target,,}" \
    --arg callback_input "$(cast sig "$callback_sig")" --arg callback_output "${result,,}" \
    --arg call_input "$call_data" --arg result_two "${result_two,,}" \
    --arg callback_forward_input "${callback_forward_input,,}" \
    --argjson selected_candidates "$(jq -c '.selected_candidate_indices // []' <<<"$entry")" \
    --arg proxy_query_input "${proxy_query_input,,}" --arg proxy_query_output "${proxy_query_output,,}" \
    -f "$HERE/lookup-trace.jq" "$trace_file")" || die "actual registry lookup returned the wrong result/revert"

if [[ "$SCENARIO" == staticLocalWrite ]]; then
    for read in 'rate:1' 'quoteAtRate1:1' 'quoteAtRate2:2' 'quoteAfterResetToRate1:1' 'runs:1'; do
        [[ "$(cast call "$source" "${read%:*}()(uint256)" --block "$block_hex" --rpc-url "$L1_RPC")" == "${read#*:}" ]] \
            || die "reader persisted the wrong local-write state (${read%:*})"
    done
else
    [[ "$(cast call "$source" 'counter()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == 1 ]] \
        || die "caller did not execute exactly once"
    if [[ "$static" == true ]]; then
        predicted="$(cast to-dec "$result")"
        [[ "$(cast call "$source" 'lastRead()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == "$predicted" ]] \
            || die "reader persisted the wrong result"
        if [[ -n "$callback_target" ]]; then
            # The L2 forwarder cannot be called directly: its callback needs L1.
            [[ "$(cast call "$callback_target" 'counter()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == "$predicted" ]] \
                || die "L1 producer differs from the read result"
        else
            [[ "$(cast call "$destination" 'counter()(uint256)' --rpc-url "$L2_RPC")" == "$predicted" ]] \
                || die "L2 producer changed"
        fi
    else
        [[ "$(cast call "$source" 'lastCallFailed()(bool)' --block "$block_hex" --rpc-url "$L1_RPC")" == true ]] \
            || die "caller did not catch the revert"
        [[ "$(cast call "$source" 'targetCounter()(uint256)' --block "$block_hex" --rpc-url "$L1_RPC")" == 0 ]] \
            || die "caller recorded a successful target result"
    fi
fi

# L1-origin brackets carry no transaction bytes. The blob verifier bound every
# bracket to the posted settlement tables; this trigger's traced lookup
# (scenario-unique source/destination) must be exactly one of those brackets,
# not an unrelated transaction in an otherwise valid blob.
jq -e --arg scenario "$SCENARIO" --arg source "${source,,}" --arg destination "${destination,,}" --arg kind "$kind" \
    --arg data "$call_data" --arg result "${result,,}" --argjson success "$success" \
    --arg callback_data "$(cast sig "$callback_sig")" --arg result_two "${result_two,,}" \
    --arg callback_source "${callback_source,,}" --arg callback_target "${callback_target,,}" \
    --argjson rollup "$ROLLUP_ID" '
    def root_action: .parent_index == null and .kind == $kind and .source_chain == 0
      and .target_chain == $rollup and (.from_address | ascii_downcase) == $source
      and (.to_address | ascii_downcase) == $destination and .value == "0" and .data == $data
      and .success == $success and (.return_data | ascii_downcase) == $result and .revert_spans == [];
    # The one static callback returns the L1 producer value, which is also the
    # read result.
    def callback_action: .parent_index == 0 and .kind == "StaticCall" and .source_chain == $rollup
      and .target_chain == 0 and (.from_address | ascii_downcase) == $callback_source
      and (.to_address | ascii_downcase) == $callback_target and .value == "0" and .data == $data
      and .success == true and (.return_data | ascii_downcase) == $result and .revert_spans == [];
    .semantic_evidence.semantics_bound_to_settlement == true
    and ([.payload.semantic_transactions[] | select(.origin_chain == 0 and .tx_data == "0x"
      and (if $scenario == "staticLocalWrite" then
             .actions as $actions | [$result, $result_two, $result] as $results
             | ($actions | length) == 6 and all(range(0; 3); . as $i
                 | ($actions[2*$i] | .index == 2*$i and .parent_index == null
                     and .kind == "StaticCall" and .source_chain == 0 and .target_chain == $rollup
                     and (.from_address | ascii_downcase) == $source
                     and (.to_address | ascii_downcase) == $destination and .value == "0"
                     and .data == $data and .success == true and .result_kind == "ReturnSuccess"
                     and .return_data == $results[$i] and .revert_spans == [])
                 and ($actions[2*$i+1] | .index == 2*$i+1 and .parent_index == 2*$i
                     and .kind == "StaticCall" and .source_chain == $rollup and .target_chain == 0
                     and (.from_address | ascii_downcase) == $callback_source
                     and (.to_address | ascii_downcase) == $callback_target and .value == "0"
                     and .data == $callback_data and .success == true and .result_kind == "ReturnSuccess"
                     and .return_data == $results[$i] and .revert_spans == []))
           elif $callback_target == "" then
             (.actions | length) == 1 and (.actions[0] | root_action)
           else
             (.actions | length) == 2 and (.actions[0] | root_action) and (.actions[1] | callback_action)
           end))]
      | length == 1)
    ' "$scratch/blob.json" >/dev/null || die "lookup is not bound to the authenticated semantic action"
keys="$(jq -nc --arg key "$key" '[$key]')"
roots="$(jq -c '[.root]' <<<"$entry")"
fi
# V0 carries counts and ordinary transaction bytes, not block headers. Bind
# the calldata's endpoint claims to canonical L2 headers before using its range.
# A successful post alone does not establish that all deferred entries settled.
jq -e '
  .verified == true
  and (.settlement_claim.pre_block_hash | test("^0x[0-9a-fA-F]{64}$"))
  and (.settlement_claim.post_block_hash | test("^0x[0-9a-fA-F]{64}$"))
  and (.payload.block_count | type == "number" and . > 0 and . <= 4294967295 and floor == .)
  and (.payload.count_runs | type == "array" and length > 0)
  and all(.payload.count_runs[];
      (.blocks | type == "number" and . > 0 and floor == .)
      and (.transactions_per_block | type == "number" and . >= 0 and . <= 16383 and floor == .))
  and ([.payload.count_runs[].blocks] | add) == .payload.block_count
' "$scratch/blob.json" >/dev/null || die "authenticated blob has invalid V0 window claims"
pre_hash="$(jq -er '.settlement_claim.pre_block_hash' "$scratch/blob.json")"
terminal_hash="$(jq -er '.settlement_claim.post_block_hash' "$scratch/blob.json")"
deadline=$((SECONDS + 120))
while :; do
    terminal="$(cast rpc eth_getBlockByHash "$terminal_hash" false --rpc-url "$L2_RPC")"
    [[ "$terminal" != null ]] && break
    (( SECONDS < deadline )) || die "L2 has not reached the claimed settlement endpoint"
    sleep 2
done
pre="$(cast rpc eth_getBlockByHash "$pre_hash" false --rpc-url "$L2_RPC")"
[[ "$pre" != null ]] || die "settlement start is not an available canonical block"
last="$(cast to-dec "$(jq -er '.number' <<<"$terminal")")"
first=$(( $(cast to-dec "$(jq -er '.number' <<<"$pre")") + 1 ))
count="$(jq -er '.payload.block_count' "$scratch/blob.json")"
(( last >= first && last - first + 1 == count )) || die "V0 count disagrees with canonical endpoint heights"
[[ "$(jq -er '.hash' <<<"$pre")" == "$pre_hash" && "$(jq -er '.hash' <<<"$terminal")" == "$terminal_hash" ]] \
    || die "RPC returned different settlement endpoints"
[[ "$(cast block "$((first - 1))" --field hash --rpc-url "$L2_RPC")" == "$pre_hash" \
   && "$(cast block "$last" --field hash --rpc-url "$L2_RPC")" == "$terminal_hash" ]] \
    || die "claimed window endpoints are not canonical on L2"
root_file="${OUTPUT_JSON%.json}.lookup-root.json"
root_index=0
root_parts=()
root_count="$(jq 'length' <<<"$roots")"
while IFS= read -r root; do
    root_part="${OUTPUT_JSON%.json}.lookup-root-$root_index.json"
    (( root_count != 1 )) || root_part="$root_file"
    bash "$HERE/verify-lookup-root.sh" "$scratch/blob.json" "$root" "$root_part" \
        || die "lookup root is not bound to the authenticated L2 execution"
    root_parts+=("$root_part")
    root_index=$((root_index + 1))
done < <(jq -r '.[]' <<<"$roots")
(( root_index > 0 )) || die "no lookup roots were checked"
if (( root_index > 1 )); then
    jq -s '{verified: all(.[]; .verified == true), roots: .}' "${root_parts[@]}" >"$root_file"
fi
end="$(cast block-number --rpc-url "$L2_RPC")"
(
    cd "$REPO/eez-core-protocol"
    forge script script/e2e/scenarios/shared/Verify.s.sol:VerifyL2Absent --rpc-url "$L2_RPC" \
        --sig 'run(uint256,uint256,address,bytes32[])' "$first" "$end" "$MANAGER_L2" "$(jq -r '"[" + join(",") + "]"' <<<"$keys")"
) >"$scratch/absence.log" 2>&1 || { cat "$scratch/absence.log" >&2; die "lookup appeared on L2"; }
# Detect a reorg while collecting evidence; no retroactive eth_call of the
# static pool is used because later batches may legitimately replace its pins.
[[ "$(cast block "$block_hex" --field hash --rpc-url "$L1_RPC")" == "$(jq -r '.hash' <<<"$block")" ]] \
    || die "L1 changed during evidence collection"
[[ "$(cast block "$last" --field hash --rpc-url "$L2_RPC")" == "$terminal_hash" ]] \
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
               blob_kzg_authenticated: true, native_semantics_decoded: true, semantic_trigger_bound: true,
               actual_lookup_result_traced: true, lookup_root_bound: true},
      source_log: {path: $log, sha256: $log_sha256}}' >"${OUTPUT_JSON}.tmp"
mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
echo "PASS: $SCENARIO canonical L1 lookup, authenticated semantics and L2 absence ($first..$end) verified"
