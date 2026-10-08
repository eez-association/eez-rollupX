#!/usr/bin/env bash
# Build machine-readable evidence for one scenario that passed the upstream
# network runner directly. This deliberately re-reads the mined transactions
# from the canonical RPCs instead of treating the runner's PASS text as the
# final source of truth.
set -euo pipefail

SCENARIO_NAME="${1:?usage: collect-protocol-e2e-evidence.sh <name> <scenario-sol> <log> <output-json>}"
SCENARIO_SOL="${2:?missing scenario Solidity path}"
SCENARIO_LOG="${3:?missing scenario log}"
OUTPUT_JSON="${4:?missing evidence output path}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

for var in L1_RPC L2_RPC ROLLUPS MANAGER_L2 PROTOCOL_COMMIT CAUSAL_VERIFIER \
    BEACON_URL BLOB_VERIFIER ROLLUP_ID; do
    [[ -n "${!var:-}" ]] || { echo "missing $var" >&2; exit 1; }
done
for tool in cast curl jq python3 sed sha256sum; do
    command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 1; }
done
[[ -f "$SCENARIO_SOL" ]] || { echo "scenario not found: $SCENARIO_SOL" >&2; exit 1; }
[[ -f "$SCENARIO_LOG" ]] || { echo "scenario log not found: $SCENARIO_LOG" >&2; exit 1; }
[[ -x "$CAUSAL_VERIFIER" ]] || { echo "causal verifier not executable: $CAUSAL_VERIFIER" >&2; exit 1; }

die() {
    echo "$SCENARIO_NAME evidence error: $*" >&2
    exit 1
}

grep -Fq '====== Done ======' "$SCENARIO_LOG" || die "upstream completion marker is absent"
! grep -q '^ERROR:' "$SCENARIO_LOG" || die "upstream log contains an ERROR"
case "$SCENARIO_NAME" in
    staticCounterL2|topLevelStaticReentrantCounterL2)
        exec bash "$(dirname "$0")/verify-static-counter-l2.sh" "$SCENARIO_LOG" "$OUTPUT_JSON" "$SCENARIO_NAME"
        ;;
    topLevelStaticCounter|topLevelStaticReentrantCounter|revertCounter)
        exec bash "$(dirname "$0")/verify-l1-lookup.sh" \
            "$SCENARIO_NAME" "$SCENARIO_SOL" "$SCENARIO_LOG" "$OUTPUT_JSON"
        ;;
esac
grep -Fq 'PASS: posted batch calldata matches expected table' "$SCENARIO_LOG" \
    || die "posted L1 table was not field-matched"
grep -Fq 'PASS: L2 field checks on' "$SCENARIO_LOG" \
    || die "loaded L2 table was not field-matched"
if ! grep -Eq 'PASS: L1 field checks on|PASS: [1-9][0-9]* settlement tx\(s\) with BatchPosted' "$SCENARIO_LOG"; then
    die "no successful L1 settlement verification was recorded"
fi

if grep -qE 'contract ExecuteNetworkL2 ' "$SCENARIO_SOL"; then
    trigger_chain="L2"
    trigger_rpc="$L2_RPC"
else
    trigger_chain="L1"
    trigger_rpc="$L1_RPC"
fi

target="$(sed -n 's/^target:[[:space:]]*//p' "$SCENARIO_LOG" | tail -1)"
[[ "$target" =~ ^0x[[:xdigit:]]{40}$ ]] || die "invalid or missing trigger target"

mapfile -t user_rows < <(
    sed -n -E 's/^User tx(\[[0-9]+\])?:[[:space:]]*(0x[[:xdigit:]]{64})[[:space:]]+\(block ([0-9]+)\).*/\2 \3/p' \
        "$SCENARIO_LOG"
)
(( ${#user_rows[@]} > 0 )) || die "no user transaction was reported in the summary"

post_row="$(
    sed -n -E 's/^L1 postBatch:[[:space:]]*(0x[[:xdigit:]]{64})[[:space:]]+\(block ([0-9]+)\).*/\1 \2/p' \
        "$SCENARIO_LOG"
)"
[[ "$(wc -l <<<"$post_row" | tr -d '[:space:]')" == "1" && -n "$post_row" ]] \
    || die "expected exactly one L1 postBatch summary row"

load_row="$(
    sed -n -E 's/^L2 loadTable:[[:space:]]*(0x[[:xdigit:]]{64})[[:space:]]+\(block ([0-9]+)\).*/\1 \2/p' \
        "$SCENARIO_LOG"
)"
[[ "$(wc -l <<<"$load_row" | tr -d '[:space:]')" == "1" && -n "$load_row" ]] \
    || die "expected exactly one L2 loadTable summary row"

causal_tmp="$(mktemp -d /tmp/eez-causal-evidence.XXXXXX)"
trap 'rm -rf "$causal_tmp"' EXIT

# Pass evidence JSON to jq through files (`--slurpfile`), never argv: receipts,
# calldata, tables and blob evidence grow with the batch and exceed Linux's
# 128 KiB per-argument limit (E2BIG, "Argument list too long"). Writing the
# file uses the printf builtin, so the value itself never crosses exec.
json_file() {
    local path="$causal_tmp/arg-$1.json"
    printf '%s\n' "$2" >"$path"
    printf '%s\n' "$path"
}

transaction_evidence() {
    local rpc="$1" chain="$2" role="$3" hash="$4" expected_block="$5"
    local canonical_tx="${6:-}"
    local receipt tx status block_hex block_number tx_to input reported_block_matches canonical_block tx_index

    receipt="$(cast rpc eth_getTransactionReceipt "$hash" --rpc-url "$rpc")" \
        || die "cannot read $role receipt $hash on $chain"
    tx="$(cast rpc eth_getTransactionByHash "$hash" --rpc-url "$rpc")" \
        || die "cannot read $role transaction $hash on $chain"
    status="$(jq -er '.status' <<<"$receipt")" || die "missing $role receipt status"
    [[ "$status" == "0x1" ]] || die "$role transaction $hash reverted"
    block_hex="$(jq -er '.blockNumber' <<<"$receipt")" || die "missing $role receipt block"
    if [[ -z "$canonical_tx" ]]; then
        canonical_block="$(cast block "$block_hex" --full --json --rpc-url "$rpc")" \
            || die "cannot read canonical $chain block $block_hex"
        tx_index="$(cast to-dec "$(jq -er '.transactionIndex' <<<"$receipt")")" \
            || die "invalid $role transaction index"
        canonical_tx="$(jq -ce --arg hash "$hash" --argjson index "$tx_index" \
            --slurpfile receipt "$(json_file receipt "$receipt")" '
            $receipt[0] as $receipt |
            select(.number == $receipt.blockNumber and .hash == $receipt.blockHash)
            | . as $block | .transactions[$index]
            | select(.hash == $hash and .blockHash == $block.hash and .blockNumber == $block.number)
            ' <<<"$canonical_block")" || die "$role transaction $hash is absent from its canonical block"
    fi
    jq -en --arg hash "$hash" --slurpfile canonical "$(json_file canonical "$canonical_tx")" \
        --slurpfile tx "$(json_file tx "$tx")" --slurpfile receipt "$(json_file receipt "$receipt")" '
        $canonical[0] as $canonical | $tx[0] as $tx | $receipt[0] as $receipt |
        $canonical.hash == $hash and $tx.hash == $hash and $receipt.transactionHash == $hash
        and $tx.blockHash == $canonical.blockHash and $receipt.blockHash == $canonical.blockHash
        and $tx.blockNumber == $canonical.blockNumber and $receipt.blockNumber == $canonical.blockNumber
        and $tx.transactionIndex == $canonical.transactionIndex
        and $receipt.transactionIndex == $canonical.transactionIndex
        and $tx.to == $canonical.to and $tx.input == $canonical.input
        ' >/dev/null || die "$role transaction/receipt $hash differs from the canonical block"
    # Feed every exact validated receipt to Rust, not a second RPC snapshot.
    printf '%s\n' "$receipt" >"$causal_tmp/$chain-$hash.receipt.json"
    block_number="$(cast to-dec "$block_hex")"
    tx_index="$(cast to-dec "$(jq -er '.transactionIndex' <<<"$receipt")")"
    tx_to="$(jq -er '.to' <<<"$tx")"
    input="$(jq -er '.input' <<<"$tx")"
    expected_block="${expected_block:-$block_number}"

    reported_block_matches=false
    [[ "$block_number" == "$expected_block" ]] && reported_block_matches=true
    # The runner retains the first optimistic L2 receipt even after replay,
    # and its settlement summary can print the scan-start block. Keep those
    # hints in evidence; the current canonical transaction and tables decide.
    if [[ "$reported_block_matches" != true ]]; then
        case "$chain:$role" in
            L2:user_trigger|L1:settlement) ;;
            *) die "$role transaction $hash moved from reported block $expected_block to $block_number" ;;
        esac
    fi

    jq -cn \
        --arg role "$role" --arg chain "$chain" --arg hash "$hash" \
        --arg status "$status" --arg block_hex "$block_hex" \
        --argjson block_number "$block_number" --argjson reported_block "$expected_block" \
        --argjson transaction_index "$tx_index" \
        --argjson reported_block_matches "$reported_block_matches" \
        --arg to "$tx_to" --rawfile input <(printf '%s' "$input") \
        '{role: $role, chain: $chain, hash: $hash, status: $status,
          block_number: $block_number, block_hex: $block_hex,
          transaction_index: $transaction_index,
          reported_block: $reported_block, reported_block_matches: $reported_block_matches,
          to: $to, input: $input}'
}

user_receipts=()
for row in "${user_rows[@]}"; do
    read -r hash block <<<"$row"
    evidence="$(transaction_evidence "$trigger_rpc" "$trigger_chain" user_trigger "$hash" "$block")"
    [[ "$(jq -r '.to | ascii_downcase' <<<"$evidence")" == "${target,,}" ]] \
        || die "user transaction $hash target differs from $target"
    user_receipts+=("$evidence")
done
user_receipts_json="$(printf '%s\n' "${user_receipts[@]}" | jq -s .)"

read -r post_hash post_block <<<"$post_row"
# The summary names only the first batch, even when the upstream runner matched
# a multi-transaction scenario across several settlements. Retain every match,
# then order the canonical transactions independently of the log's scan order.
mapfile -t post_hashes < <(
    { printf '%s\n' "$post_hash";
      sed -n -E 's/^Matched settlement tx (0x[[:xdigit:]]{64})[[:space:]]*$/\1/p' "$SCENARIO_LOG";
    } | awk '!seen[$0]++'
)
post_receipts=()
for hash in "${post_hashes[@]}"; do
    expected_block=""
    [[ "$hash" == "$post_hash" ]] && expected_block="$post_block"
    evidence="$(transaction_evidence "$L1_RPC" L1 settlement "$hash" "$expected_block")"
    [[ "$(jq -r '.to | ascii_downcase' <<<"$evidence")" == "${ROLLUPS,,}" ]] \
        || die "postBatch transaction target differs from registry $ROLLUPS"
    post_receipts+=("$evidence")
done
post_receipts_json="$(printf '%s\n' "${post_receipts[@]}" | jq -s 'sort_by(.block_number, .transaction_index)')"
if (( ${#post_hashes[@]} > 1 )); then
    [[ "$trigger_chain" == L1 ]] || die "multiple settlements require L1 triggers"
    # The pinned L1 protocol consumes a posted table only in its posting block.
    # Require one selected settlement before each trigger in that block; never
    # assign a trigger to a blob merely because it appeared in a summary.
    jq -en --slurpfile posts "$(json_file posts "$post_receipts_json")" \
        --slurpfile users "$(json_file users "$user_receipts_json")" '
        $posts[0] as $posts | $users[0] as $users |
        all($users[]; . as $user |
            [$posts[] | select(.block_number == $user.block_number and
                              .transaction_index < $user.transaction_index)] | length == 1)
        and all($posts[]; . as $post |
            any($users[]; .block_number == $post.block_number and
                          .transaction_index > $post.transaction_index))
        ' >/dev/null || die "settlements do not uniquely cover trigger blocks"
fi

read -r load_hash load_block <<<"$load_row"
hashes_from_line() {
    local label="$1"
    sed -n "s/^${label}:[[:space:]]*//p" "$SCENARIO_LOG" \
        | head -1 \
        | awk '{
            while (match($0, /0x[0-9a-fA-F]{64}/)) {
                print substr($0, RSTART, RLENGTH)
                $0 = substr($0, RSTART + RLENGTH)
            }
        }' \
        | jq -R . \
        | jq -s .
}

proofs_json="$(sed -n 's/^[[:space:]]*\(PASS: .*$\)/\1/p' "$SCENARIO_LOG" | jq -R . | jq -s .)"
expected_l1_json="$(hashes_from_line 'L1 expected entries')"
expected_l2_json="$(hashes_from_line 'L2 table expected')"
expected_l1_calls_json="$(hashes_from_line 'L1 expected calls')"
expected_l2_calls_json="$(hashes_from_line 'L2 calls expected')"

# Upstream prints the FIRST load and ALL calls in a block, including other
# parallel scenarios. Discover the actual scenario tables by runtime entry hash
# in canonical block calldata. Keep each selected transaction/receipt whole;
# the unchanged Rust verifier still enforces exact causal/event multiplicity.
if [[ "$trigger_chain" == L2 ]]; then
    # Revert-only L2 scenarios have no surviving call events and skip the
    # upstream call search/"blocks to verify" line. Their tables must be in
    # the same blocks as the actual consuming user transactions. After replay,
    # the upstream first-load block hint can refer to an old optimistic block.
    l2_blocks="$(jq '[.[].block_number] | unique' <<<"$user_receipts_json")"
else
    l2_blocks="$(sed -n 's/^L2 blocks to verify:[[:space:]]*//p' "$SCENARIO_LOG" | tail -1)"
fi
jq -e --arg chain "$trigger_chain" --argjson first "$load_block" '
    type == "array" and length > 0 and length == (unique | length)
    and all(.[]; type == "number" and . >= 0 and . == floor)
    and ($chain == "L2" or index($first) != null)' <<<"$l2_blocks" >/dev/null \
    || die "missing or invalid verified L2 block list"
while read -r block; do
    cast block "$block" --full --json --rpc-url "$L2_RPC" >>"$causal_tmp/l2-blocks.jsonl" \
        || die "cannot read canonical L2 block $block"
done < <(jq -r '.[]' <<<"$l2_blocks")
jq -s . "$causal_tmp/l2-blocks.jsonl" >"$causal_tmp/l2-blocks.json"
selected_systems="$(python3 "$REPO/scripts/ci/select-protocol-l2-transactions.py" \
    --abi "$REPO/eez-core-protocol/out/EEZL2.sol/EEZL2.json" --manager "$MANAGER_L2" \
    --expected "$expected_l2_json" <"$causal_tmp/l2-blocks.json")" \
    || die "cannot select exact scenario table transactions from canonical L2 blocks"
system_hashes=()
system_receipts=()
while read -r canonical_tx; do
    hash="$(jq -r '.hash' <<<"$canonical_tx")"
    block="$(cast to-dec "$(jq -r '.blockNumber' <<<"$canonical_tx")")"
    evidence="$(transaction_evidence "$L2_RPC" L2 delivery "$hash" "$block" "$canonical_tx")"
    system_hashes+=("$hash")
    system_receipts+=("$evidence")
done < <(jq -c '.[]' <<<"$selected_systems")
system_receipts_json="$(printf '%s\n' "${system_receipts[@]}" | jq -s .)"

log_sha256="$(sha256sum "$SCENARIO_LOG" | awk '{print $1}')"
rolling_replayed=false
grep -Fq 'PASS: posted rolling hashes reproduced from posted roots' "$SCENARIO_LOG" \
    && rolling_replayed=true
l2_calls_verified=false
grep -Eq 'PASS: [0-9]+/[0-9]+ expected L2 calls verified' "$SCENARIO_LOG" \
    && l2_calls_verified=true
summary_blocks_match="$(jq -n \
    --slurpfile users "$(json_file users "$user_receipts_json")" \
    --slurpfile settlements "$(json_file settlements "$post_receipts_json")" \
    --slurpfile systems "$(json_file systems "$system_receipts_json")" \
    '($users[0] + $settlements[0] + $systems[0]) | all(.reported_block_matches)')"

# Build a canonical receipt/calldata input for the Rust verifier. It invokes
# the same independent two-table verifier used by the deriver, then requires
# exact event multiplicity for the selected trigger flow.
: >"$causal_tmp/l1-receipts.jsonl"
: >"$causal_tmp/l2-receipts.jsonl"
for evidence in "${user_receipts[@]}"; do
    hash="$(jq -r '.hash' <<<"$evidence")"
    jq -c . "$causal_tmp/$trigger_chain-$hash.receipt.json" \
        >>"$causal_tmp/${trigger_chain,,}-receipts.jsonl"
done
while read -r hash; do
    jq -c . "$causal_tmp/L1-$hash.receipt.json" >>"$causal_tmp/l1-receipts.jsonl"
done < <(jq -r '.[].hash' <<<"$post_receipts_json")
for hash in "${system_hashes[@]}"; do
    jq -c . "$causal_tmp/L2-$hash.receipt.json" >>"$causal_tmp/l2-receipts.jsonl"
done
jq -s . "$causal_tmp/l1-receipts.jsonl" >"$causal_tmp/l1-receipts.json"
jq -s . "$causal_tmp/l2-receipts.jsonl" >"$causal_tmp/l2-receipts.json"
direction=L1_to_L2
[[ "$trigger_chain" == L2 ]] && direction=L2_to_L1
jq -n \
    --arg direction "$direction" --arg registry "$ROLLUPS" --arg manager_l2 "$MANAGER_L2" \
    --argjson rollup_id "${ROLLUP_ID:-1}" \
    --slurpfile l1_calldata <(jq 'map(.input) | if length == 1 then .[0] else . end' <<<"$post_receipts_json") \
    --slurpfile l2_calldata <(jq '[.[].input]' <<<"$system_receipts_json") \
    --slurpfile expected_l2_entry_hashes "$(json_file expected_l2_entry_hashes "$expected_l2_json")" \
    --slurpfile l1_receipts "$causal_tmp/l1-receipts.json" \
    --slurpfile l2_receipts "$causal_tmp/l2-receipts.json" \
    '{direction: $direction, registry: $registry, manager_l2: $manager_l2,
      rollup_id: $rollup_id, l1_calldata: $l1_calldata[0], l2_calldata: $l2_calldata[0],
      expected_l2_entry_hashes: $expected_l2_entry_hashes[0],
      l1_receipts: $l1_receipts[0], l2_receipts: $l2_receipts[0]}' \
    >"$causal_tmp/input.json"
# Public canonical calldata/receipts permit offline reproduction of failures.
mkdir -p "$(dirname "$OUTPUT_JSON")"
cp "$causal_tmp/input.json" "${OUTPUT_JSON%.json}.causal-input.json"
if ! "$CAUSAL_VERIFIER" "$causal_tmp/input.json" "$causal_tmp/output.json"; then
    die "independent trigger/table/event causal verification failed"
fi
causal_graph="$(jq -c . "$causal_tmp/output.json")"

# Authenticate the exact physical blob selected by postBatch, decode the
# native semantic stream and bind its brackets to the posted settlement
# tables. An L2 trigger must also appear byte-for-byte as semantic tx_data. An
# L1 trigger (empty L1-origin tx_data) is bound through those tables and the
# causal trigger-to-table bijection verified above.
blob_evidence=()
while read -r post; do
    hash="$(jq -r '.hash' <<<"$post")"
    mapfile -t trigger_hashes < <(jq -r --slurpfile post "$(json_file post "$post")" \
        --argjson count "${#post_hashes[@]}" \
        '$post[0] as $post | .[] | select($count == 1 or .block_number == $post.block_number) | .hash' \
        <<<"$user_receipts_json")
    blob_evidence_file="$causal_tmp/blob-$hash.json"
    sidecars_file="${OUTPUT_JSON%.json}.blob-sidecars.json"
    if (( ${#post_hashes[@]} > 1 )); then
        sidecars_file="${OUTPUT_JSON%.json}.$hash.blob-sidecars.json"
    fi
    if ! L1_RPC="$L1_RPC" L2_RPC="$L2_RPC" BEACON_URL="$BEACON_URL" \
        BLOB_VERIFIER="$BLOB_VERIFIER" ROLLUP_ID="$ROLLUP_ID" \
        bash "$(dirname "$0")/verify-semantic-settlement-blob.sh" \
            "$hash" "$trigger_chain" "$blob_evidence_file" "$sidecars_file" \
            "${trigger_hashes[@]}"; then
        die "settlement blob semantic verification failed"
    fi
    jq -e '.verified == true' "$blob_evidence_file" >/dev/null \
        || die "settlement blob semantic verification failed"
    blob_evidence+=("$(jq -c . "$blob_evidence_file")")
done < <(jq -c '.[]' <<<"$post_receipts_json")
blob_da="$(printf '%s\n' "${blob_evidence[@]}" | jq -s '
    if length == 1 then .[0] else {verified: all(.verified == true), settlements: .} end')"

# A reorg during RPC collection must not turn individually valid snapshots
# into evidence for incompatible blocks. Recheck both chains after verification.
for chain in L1 L2; do
    rpc="$L1_RPC"
    [[ "$chain" == L2 ]] && rpc="$L2_RPC"
    while IFS=$'\t' read -r block hash; do
        [[ "$(cast block "$block" --field hash --rpc-url "$rpc")" == "$hash" ]] \
            || die "$chain changed during evidence collection at block $block"
    done < <(jq -rs 'map([.blockNumber, .blockHash]) | unique[] | @tsv' "$causal_tmp/$chain-"*.receipt.json)
done

mkdir -p "$(dirname "$OUTPUT_JSON")"
tmp_output="${OUTPUT_JSON}.tmp"
jq -n \
    --argjson schema_version 2 \
    --arg scenario "$SCENARIO_NAME" \
    --arg scenario_path "$SCENARIO_SOL" \
    --arg protocol_commit "$PROTOCOL_COMMIT" \
    --arg verification_mode upstream-direct \
    --arg result PASS \
    --arg log "$SCENARIO_LOG" \
    --arg log_sha256 "$log_sha256" \
    --arg upstream_table_hash "$load_hash" \
    --arg trigger_chain "$trigger_chain" \
    --slurpfile trigger_transactions "$(json_file trigger_transactions "$user_receipts_json")" \
    --slurpfile settlement_transactions "$(json_file settlement_transactions "$post_receipts_json")" \
    --slurpfile system_transactions "$(json_file system_transactions "$system_receipts_json")" \
    --slurpfile expected_l1_entries "$(json_file expected_l1_entries "$expected_l1_json")" \
    --slurpfile expected_l2_entries "$(json_file expected_l2_entries "$expected_l2_json")" \
    --slurpfile expected_l1_calls "$(json_file expected_l1_calls "$expected_l1_calls_json")" \
    --slurpfile expected_l2_calls "$(json_file expected_l2_calls "$expected_l2_calls_json")" \
    --slurpfile proof_statements "$(json_file proof_statements "$proofs_json")" \
    --argjson rolling_replayed "$rolling_replayed" \
    --argjson l2_calls_verified "$l2_calls_verified" \
    --argjson summary_blocks_match "$summary_blocks_match" \
    --slurpfile causal_graph "$(json_file causal_graph "$causal_graph")" \
    --slurpfile blob_da "$(json_file blob_da "$blob_da")" \
    '$trigger_transactions[0] as $trigger_transactions | $settlement_transactions[0] as $settlement_transactions | $system_transactions[0] as $system_transactions | $expected_l1_entries[0] as $expected_l1_entries | $expected_l2_entries[0] as $expected_l2_entries | $expected_l1_calls[0] as $expected_l1_calls | $expected_l2_calls[0] as $expected_l2_calls | $proof_statements[0] as $proof_statements | $causal_graph[0] as $causal_graph | $blob_da[0] as $blob_da |
     {schema_version: $schema_version, scenario: $scenario, scenario_path: $scenario_path,
      protocol_commit: $protocol_commit, result: $result, verification_mode: $verification_mode,
      trigger_chain: $trigger_chain, trigger_transactions: $trigger_transactions,
      settlement_transactions: $settlement_transactions,
      system_transactions: $system_transactions,
      expected: {l1_entries: $expected_l1_entries, l2_entries: $expected_l2_entries,
                 l1_calls: $expected_l1_calls, l2_calls: $expected_l2_calls},
      causal_graph: $causal_graph,
      blob_da: $blob_da,
      claims: {upstream_completed: true, receipts_revalidated: true,
               scenario_tables_selected_from_canonical_calldata: true,
               upstream_summary_blocks_match_receipts: $summary_blocks_match,
               causal_graph_bijection: true,
               blob_kzg_authenticated: true, native_semantics_decoded: true,
               semantic_trigger_bound: true,
               l1_settlement_verified: true, posted_table_field_match: true,
               rolling_hashes_replayed: $rolling_replayed, l2_table_field_match: true,
               l2_calls_verified: $l2_calls_verified},
      proof_statements: $proof_statements,
      source_log: {path: $log, sha256: $log_sha256,
                   upstream_first_table_hash: $upstream_table_hash}}' >"$tmp_output"
mv "$tmp_output" "$OUTPUT_JSON"
