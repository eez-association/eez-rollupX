#!/usr/bin/env bash
# Bind an L1 lookup pin to the authenticated L2 window. Intermediate pins name
# virtual transaction-prefix headers, not separately mined L2 blocks.
set -euo pipefail
blob="${1:?usage: verify-lookup-root.sh <verified-blob.json> <root> <output.json>}"
root="${2:?missing root}"
output="${3:?missing output}"
[[ "$root" =~ ^0x[[:xdigit:]]{64}$ ]] || exit 1
mkdir -p "$(dirname "$output")"
jq -e '.verified == true and (.payload.blocks | length) > 0' "$blob" >/dev/null
if jq -e --arg root "$root" '[.payload.blocks[].hash, .payload.blocks[-1].parent_hash] | index($root) != null' "$blob" >/dev/null; then
    jq -n --arg root "$root" '{verified:true, root:$root, kind:"authenticated_block_boundary"}' >"$output"
    exit 0
fi

hash="$(jq -er '.payload.blocks[-1].hash' "$blob")"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cast rpc debug_getRawBlock "$hash" --rpc-url "$L2_RPC" >"$scratch/block.json"
cast rpc debug_getRawReceipts "$hash" --rpc-url "$L2_RPC" >"$scratch/receipts.json"
cast rpc debug_intermediateRoots "$hash" --rpc-url "$L2_RPC" >"$scratch/roots.json"
jq -n --arg hash "$hash" --slurpfile block "$scratch/block.json" \
    --slurpfile receipts "$scratch/receipts.json" --slurpfile roots "$scratch/roots.json" \
    '{block_hash:$hash, block_rlp:$block[0], receipts_rlp:$receipts[0], state_roots:$roots[0]}' \
    >"${output%.json}.input.json"
"$BLOB_VERIFIER" --prefixes "${output%.json}.input.json" >"$scratch/prefixes.json"
jq -e --arg root "$root" --arg hash "$hash" '
    select(.verified == true and .block_hash == $hash)
    | [.prefixes[] | select(.hash == $root)]
    | if length == 1 then {verified:true, root:$root, kind:"execution_prefix",
        terminal_block_hash:$hash, transaction_index:.[0].transaction_index,
        state_root_source:"debug_intermediateRoots"}
      else error("lookup root is not an authenticated block boundary or reconstructed execution prefix") end
    ' "$scratch/prefixes.json" >"$output"
