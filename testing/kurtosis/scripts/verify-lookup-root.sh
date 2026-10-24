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
# Reth's minimal debug witness can omit sibling nodes needed to recompute an
# intermediate prefix root after a deletion. Use the proving feed's witness.
cast rpc eez_executionWitnessByBlockHash "$hash" --rpc-url "${L2_FRONT:?set the Composer L2 front URL}" >"$scratch/witness.json"
jq -e 'type == "object"' "$scratch/witness.json" >/dev/null
cast rpc debug_chainConfig --rpc-url "$L2_RPC" >"$scratch/config.json"
genesis_hash="$(cast rpc eth_getBlockByNumber 0x0 false --rpc-url "$L2_RPC" | jq -er '.hash')"
# The proving feed's witness contains L2 ancestry, not L1 execution headers.
# These RPC reads locate a bounded range; the offline replay authenticates it
# against parent storage and the exact history transaction, never RPC roots.
history_address=0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077
parent="$(cast rpc eth_getBlockByHash "$hash" false --rpc-url "$L2_RPC" | jq -er '.parentHash')"
[[ "$parent" =~ ^0x[[:xdigit:]]{64}$ ]] || exit 1
cast rpc eth_getStorageAt "$history_address" 0x2 "$parent" --rpc-url "$L2_RPC" >"$scratch/first.json"
cast rpc eth_getStorageAt "$history_address" 0x2 "$hash" --rpc-url "$L2_RPC" >"$scratch/last.json"
python3 - "$scratch/first.json" "$scratch/last.json" >"$scratch/numbers" <<'PY'
import json, re, sys

def endpoint(path):
    with open(path) as stream:
        value = json.load(stream)
    if not isinstance(value, str) or re.fullmatch(r"0x[0-9a-fA-F]{64}", value) is None:
        raise SystemExit("L1 history endpoint is not a storage word")
    number = int(value, 16)
    if number >= 2**64:
        raise SystemExit("L1 history endpoint exceeds uint64")
    return number

first, last = map(endpoint, sys.argv[1:])
# Shared protocol MAX_IMPORT_HEADERS is 64; Rust checks the exact import count.
if last < first or last - first > 64:
    raise SystemExit("L1 history endpoint range exceeds one bounded import")
for number in range(first, last + 1):
    print(hex(number))
PY
while IFS= read -r number; do
    cast rpc debug_getRawHeader "$number" --rpc-url "$L1_RPC" >>"$scratch/l1-headers.jsonl"
done <"$scratch/numbers"
post_block="$(jq -er '.l1_block_hash' "$blob")"
[[ "$post_block" =~ ^0x[[:xdigit:]]{64}$ ]] || exit 1
manager="$(cast call "$ROLLUPS" 'rollups(uint64)(address,bytes32,uint256)' \
    "$ROLLUP_ID" --block "$post_block" --rpc-url "$L1_RPC" | sed -n '1p' | tr -d '[:space:]')"
[[ "$manager" =~ ^0x[[:xdigit:]]{40}$ && "$manager" != 0x0000000000000000000000000000000000000000 ]] || exit 1
profile_hash="$(cast call "$manager" 'protocolConfigHash()(bytes32)' --block "$post_block" --rpc-url "$L1_RPC")"
[[ "$profile_hash" =~ ^0x[[:xdigit:]]{64}$ ]] || exit 1
case "${EEZ_L1_CHAIN_CONFIG:?set the deployed L1 profile}" in
    mainnet|sepolia|hoodi|chiado)
        jq -n --arg preset "$EEZ_L1_CHAIN_CONFIG" '$preset' >"$scratch/l1-config.json" ;;
    *) jq . "$EEZ_L1_CHAIN_CONFIG" >"$scratch/l1-config.json" ;;
esac
jq -n --arg hash "$hash" --arg genesis_hash "$genesis_hash" --arg profile_hash "$profile_hash" \
    --slurpfile genesis "${EEZ_L2_GENESIS:?set the deployed L2 genesis path}" \
    --slurpfile l1_config "$scratch/l1-config.json" \
    --slurpfile l1_headers "$scratch/l1-headers.jsonl" \
    --slurpfile block "$scratch/block.json" \
    --slurpfile receipts "$scratch/receipts.json" --slurpfile witness "$scratch/witness.json" \
    --slurpfile config "$scratch/config.json" \
    '{block_hash:$hash, block_rlp:$block[0], receipts_rlp:$receipts[0], witness:$witness[0],
      chain_config:$config[0], genesis:$genesis[0], genesis_hash:$genesis_hash,
      l1_chain_config:$l1_config[0], l1_headers_rlp:$l1_headers, protocol_config_hash:$profile_hash}' \
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
