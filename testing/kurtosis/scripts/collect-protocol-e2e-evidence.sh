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

for var in L1_RPC L2_RPC ROLLUPS MANAGER_L2 PROTOCOL_COMMIT CAUSAL_VERIFIER \
    BEACON_URL BLOB_VERIFIER ROLLUP_ID; do
    [[ -n "${!var:-}" ]] || { echo "missing $var" >&2; exit 1; }
done
for tool in cast curl jq sed sha256sum; do
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

transaction_evidence() {
    local rpc="$1" chain="$2" role="$3" hash="$4" expected_block="$5"
    local receipt tx status block_hex block_number tx_to input reported_block_matches

    receipt="$(cast rpc eth_getTransactionReceipt "$hash" --rpc-url "$rpc")" \
        || die "cannot read $role receipt $hash on $chain"
    tx="$(cast rpc eth_getTransactionByHash "$hash" --rpc-url "$rpc")" \
        || die "cannot read $role transaction $hash on $chain"
    status="$(jq -er '.status' <<<"$receipt")"
    block_hex="$(jq -er '.blockNumber' <<<"$receipt")"
    block_number="$(cast to-dec "$block_hex")"
    tx_to="$(jq -er '.to' <<<"$tx")"
    input="$(jq -er '.input' <<<"$tx")"

    [[ "$status" == "0x1" ]] || die "$role transaction $hash reverted"
    reported_block_matches=false
    [[ "$block_number" == "$expected_block" ]] && reported_block_matches=true
    # Trigger and L2 table summaries are receipt-derived and must be exact. The
    # upstream L2-origin settlement summary sometimes prints the scan-start
    # block rather than the matched candidate's block; retain that discrepancy
    # in evidence while trusting the canonical receipt.
    if [[ "$role" != settlement && "$reported_block_matches" != true ]]; then
        die "$role transaction $hash moved from reported block $expected_block to $block_number"
    fi

    jq -cn \
        --arg role "$role" --arg chain "$chain" --arg hash "$hash" \
        --arg status "$status" --arg block_hex "$block_hex" \
        --argjson block_number "$block_number" --argjson reported_block "$expected_block" \
        --argjson reported_block_matches "$reported_block_matches" \
        --arg to "$tx_to" --arg input "$input" \
        '{role: $role, chain: $chain, hash: $hash, status: $status,
          block_number: $block_number, block_hex: $block_hex,
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
post_receipt="$(transaction_evidence "$L1_RPC" L1 settlement "$post_hash" "$post_block")"
[[ "$(jq -r '.to | ascii_downcase' <<<"$post_receipt")" == "${ROLLUPS,,}" ]] \
    || die "postBatch transaction target differs from registry $ROLLUPS"

read -r load_hash load_block <<<"$load_row"
# An inbound trigger may be split across several L2 delivery transactions.
# The upstream summary prints those hashes below one `L2 call exec:` label,
# sometimes repeating a hash when one transaction executes multiple calls.
# Retain every distinct transaction, in first-seen order. Outbound scenarios
# instead report their user transaction(s) under that label, so their only
# system transaction is the separately reported table load.
system_hashes=("$load_hash")
if [[ "$trigger_chain" == L1 ]]; then
    mapfile -t reported_delivery_hashes < <(
        sed -n '/^L2 call exec:/,/^====== Done/p' "$SCENARIO_LOG" \
            | awk '{
                while (match($0, /0x[0-9a-fA-F]{64}/)) {
                    hash = substr($0, RSTART, RLENGTH)
                    if (!seen[tolower(hash)]++) print hash
                    $0 = substr($0, RSTART + RLENGTH)
                }
            }'
    )
    for hash in "${reported_delivery_hashes[@]}"; do
        already_present=false
        for existing_hash in "${system_hashes[@]}"; do
            [[ "${hash,,}" == "${existing_hash,,}" ]] && already_present=true
        done
        [[ "$already_present" == true ]] || system_hashes+=("$hash")
    done
fi

system_receipts=()
for hash in "${system_hashes[@]}"; do
    evidence="$(transaction_evidence "$L2_RPC" L2 delivery "$hash" "$load_block")"
    [[ "$(jq -r '.to | ascii_downcase' <<<"$evidence")" == "${MANAGER_L2,,}" ]] \
        || die "L2 system transaction $hash target differs from manager $MANAGER_L2"
    system_receipts+=("$evidence")
done
system_receipts_json="$(printf '%s\n' "${system_receipts[@]}" | jq -s .)"
load_receipt="${system_receipts[0]}"

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
log_sha256="$(sha256sum "$SCENARIO_LOG" | awk '{print $1}')"
rolling_replayed=false
grep -Fq 'PASS: posted rolling hashes reproduced from posted roots' "$SCENARIO_LOG" \
    && rolling_replayed=true
l2_calls_verified=false
grep -Eq 'PASS: [0-9]+/[0-9]+ expected L2 calls verified' "$SCENARIO_LOG" \
    && l2_calls_verified=true
summary_blocks_match="$(jq -n \
    --argjson users "$user_receipts_json" --argjson settlement "$post_receipt" \
    --argjson systems "$system_receipts_json" \
    '($users + [$settlement] + $systems) | all(.reported_block_matches)')"

# Build a canonical receipt/calldata input for the Rust verifier. It invokes
# the same independent two-table verifier used by the deriver, then requires
# exact event multiplicity for the selected trigger flow.
causal_tmp="$(mktemp -d /tmp/eez-causal-evidence.XXXXXX)"
trap 'rm -rf "$causal_tmp"' EXIT
: >"$causal_tmp/l1-receipts.jsonl"
: >"$causal_tmp/l2-receipts.jsonl"
for evidence in "${user_receipts[@]}"; do
    hash="$(jq -r '.hash' <<<"$evidence")"
    if [[ "$trigger_chain" == L1 ]]; then
        cast rpc eth_getTransactionReceipt "$hash" --rpc-url "$L1_RPC" | jq -c . \
            >>"$causal_tmp/l1-receipts.jsonl"
    else
        cast rpc eth_getTransactionReceipt "$hash" --rpc-url "$L2_RPC" | jq -c . \
            >>"$causal_tmp/l2-receipts.jsonl"
    fi
done
cast rpc eth_getTransactionReceipt "$post_hash" --rpc-url "$L1_RPC" | jq -c . \
    >>"$causal_tmp/l1-receipts.jsonl"
for hash in "${system_hashes[@]}"; do
    cast rpc eth_getTransactionReceipt "$hash" --rpc-url "$L2_RPC" | jq -c . \
        >>"$causal_tmp/l2-receipts.jsonl"
done
jq -s . "$causal_tmp/l1-receipts.jsonl" >"$causal_tmp/l1-receipts.json"
jq -s . "$causal_tmp/l2-receipts.jsonl" >"$causal_tmp/l2-receipts.json"
direction=L1_to_L2
[[ "$trigger_chain" == L2 ]] && direction=L2_to_L1
jq -n \
    --arg direction "$direction" --arg registry "$ROLLUPS" --arg manager_l2 "$MANAGER_L2" \
    --argjson rollup_id "${ROLLUP_ID:-1}" \
    --arg l1_calldata "$(jq -r '.input' <<<"$post_receipt")" \
    --argjson l2_calldata "$(jq '[.[].input]' <<<"$system_receipts_json")" \
    --argjson expected_l2_entry_hashes "$expected_l2_json" \
    --slurpfile l1_receipts "$causal_tmp/l1-receipts.json" \
    --slurpfile l2_receipts "$causal_tmp/l2-receipts.json" \
    '{direction: $direction, registry: $registry, manager_l2: $manager_l2,
      rollup_id: $rollup_id, l1_calldata: $l1_calldata, l2_calldata: $l2_calldata,
      expected_l2_entry_hashes: $expected_l2_entry_hashes,
      l1_receipts: $l1_receipts[0], l2_receipts: $l2_receipts[0]}' \
    >"$causal_tmp/input.json"
if ! "$CAUSAL_VERIFIER" "$causal_tmp/input.json" "$causal_tmp/output.json"; then
    die "independent trigger/table/event causal verification failed"
fi
causal_graph="$(jq -c . "$causal_tmp/output.json")"

# Authenticate the exact physical blob selected by postBatch, decode the
# native semantic stream, and require every canonical raw trigger transaction
# to appear byte-for-byte as semantic tx_data.
trigger_hashes=()
for evidence in "${user_receipts[@]}"; do
    trigger_hashes+=("$(jq -r '.hash' <<<"$evidence")")
done
blob_evidence_file="$causal_tmp/blob-evidence.json"
sidecars_file="${OUTPUT_JSON%.json}.blob-sidecars.json"
if ! L1_RPC="$L1_RPC" L2_RPC="$L2_RPC" BEACON_URL="$BEACON_URL" \
    BLOB_VERIFIER="$BLOB_VERIFIER" ROLLUP_ID="$ROLLUP_ID" \
    bash "$(dirname "$0")/verify-semantic-settlement-blob.sh" \
        "$post_hash" "$trigger_chain" "$blob_evidence_file" "$sidecars_file" \
        "${trigger_hashes[@]}"; then
    die "settlement blob semantic verification failed"
fi
blob_da="$(jq -c . "$blob_evidence_file")"

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
    --arg trigger_chain "$trigger_chain" \
    --argjson trigger_transactions "$user_receipts_json" \
    --argjson settlement_transaction "$post_receipt" \
    --argjson system_transactions "$system_receipts_json" \
    --argjson expected_l1_entries "$expected_l1_json" \
    --argjson expected_l2_entries "$expected_l2_json" \
    --argjson expected_l1_calls "$expected_l1_calls_json" \
    --argjson expected_l2_calls "$expected_l2_calls_json" \
    --argjson proof_statements "$proofs_json" \
    --argjson rolling_replayed "$rolling_replayed" \
    --argjson l2_calls_verified "$l2_calls_verified" \
    --argjson summary_blocks_match "$summary_blocks_match" \
    --argjson causal_graph "$causal_graph" \
    --argjson blob_da "$blob_da" \
    '{schema_version: $schema_version, scenario: $scenario, scenario_path: $scenario_path,
      protocol_commit: $protocol_commit, result: $result, verification_mode: $verification_mode,
      trigger_chain: $trigger_chain, trigger_transactions: $trigger_transactions,
      settlement_transactions: [$settlement_transaction],
      system_transactions: $system_transactions,
      expected: {l1_entries: $expected_l1_entries, l2_entries: $expected_l2_entries,
                 l1_calls: $expected_l1_calls, l2_calls: $expected_l2_calls},
      causal_graph: $causal_graph,
      blob_da: $blob_da,
      claims: {upstream_completed: true, receipts_revalidated: true,
               upstream_summary_blocks_match_receipts: $summary_blocks_match,
               causal_graph_bijection: true,
               blob_kzg_authenticated: true, native_semantics_decoded: true,
               semantic_trigger_bytes_bound: true,
               l1_settlement_verified: true, posted_table_field_match: true,
               rolling_hashes_replayed: $rolling_replayed, l2_table_field_match: true,
               l2_calls_verified: $l2_calls_verified},
      proof_statements: $proof_statements,
      source_log: {path: $log, sha256: $log_sha256}}' >"$tmp_output"
mv "$tmp_output" "$OUTPUT_JSON"
