#!/usr/bin/env bash
# Bind an L1 lookup pin to the authenticated L2 window. Intermediate pins name
# virtual transaction-prefix headers, not separately mined L2 blocks. The
# caller must first bind both calldata endpoint claims to canonical L2 headers
# and check their heights against the V0 block count.
set -euo pipefail
blob="${1:?usage: verify-lookup-root.sh <verified-blob.json> <root> <output.json>}"
root="${2:?missing root}"
output="${3:?missing output}"
[[ "$root" =~ ^0x[[:xdigit:]]{64}$ ]] || exit 1
mkdir -p "$(dirname "$output")"
jq -e '.verified == true and (.payload.block_count > 0)' "$blob" >/dev/null
if jq -e --arg root "$root" '[.settlement_claim.pre_block_hash, .settlement_claim.post_block_hash] | index($root) != null' "$blob" >/dev/null; then
    jq -n --arg root "$root" '{verified:true, root:$root, kind:"canonical_block_boundary"}' >"$output"
    exit 0
fi

hash="$(jq -er '.settlement_claim.post_block_hash' "$blob")"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cast rpc debug_getRawBlock "$hash" --rpc-url "$L2_RPC" >"$scratch/block.json"
cast rpc debug_getRawReceipts "$hash" --rpc-url "$L2_RPC" >"$scratch/receipts.json"
cast rpc debug_executionWitnessByBlockHash "$hash" --rpc-url "$L2_RPC" >"$scratch/witness.json"
cast rpc debug_chainConfig --rpc-url "$L2_RPC" >"$scratch/config.json"
jq -n --arg hash "$hash" --slurpfile block "$scratch/block.json" \
    --slurpfile receipts "$scratch/receipts.json" --slurpfile witness "$scratch/witness.json" \
    --slurpfile config "$scratch/config.json" \
    '{block_hash:$hash, block_rlp:$block[0], receipts_rlp:$receipts[0], witness:$witness[0], chain_config:$config[0]}' \
    >"${output%.json}.input.json"
"$BLOB_VERIFIER" --prefixes "${output%.json}.input.json" >"$scratch/prefixes.json"
jq -e --arg root "$root" --arg hash "$hash" '
    select(.verified == true and .block_hash == $hash)
    | [.prefixes[] | select(.hash == $root)]
    | if length == 1 then {verified:true, root:$root, kind:"execution_prefix",
        terminal_block_hash:$hash, transaction_index:.[0].transaction_index,
        transaction_count:.[0].transaction_count,
        state_root_source:"stateless_execution_witness_replay"}
      else error("lookup root is not a canonical block boundary or reconstructed execution prefix") end
    ' "$scratch/prefixes.json" >"$output"
