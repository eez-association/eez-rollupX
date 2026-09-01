#!/usr/bin/env bash
#
# Clean chiado bring-up — one command, start to finish:
#   1. deploy the protocol on chiado (skipped if deployments.env already
#      exists — re-running this must not mint new contracts over a live one)
#   2. prepare the L2 + lighthouse datadirs (fresh, since a fresh deploy means
#      a new genesis — old chain data under it would just diverge)
#   3. start the docker compose stack (eez-node + eez-proof-signer + lighthouse)
#   4. wait until the pipeline is healthy: L2 producing AND a batch has
#      actually settled on L1 (not just "the RPCs answer")
#   5. print the RPC URLs + deployed addresses
#
# Config (see README):
#   .env         — deploy inputs: EEZ_L1_RPC_URL (a TIP chiado RPC), keys,
#                  optionally EEZ_BLOCKSCOUT_URL.
#   .env.chiado  — docker host paths + funded keys + bundler URL. Consumed by
#                  `docker compose --env-file .env.chiado`.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"; cd "$REPO"
COMPOSE=(docker compose --env-file .env.chiado -f docker-compose.chiado-node.yml)

for t in docker cast curl jq make; do command -v "$t" >/dev/null || { echo "✗ $t not in PATH"; exit 1; }; done
[[ -f .env ]]        || { echo "✗ .env missing — cp .env.example .env and fill it in first"; exit 1; }
[[ -f .env.chiado ]] || { echo "✗ .env.chiado missing — cp .env.chiado.example .env.chiado and fill it in first"; exit 1; }

set -a; source .env; source .env.chiado; set +a
DEPLOY_RPC="$EEZ_L1_RPC_URL"
for v in L1_SNAPSHOT_DIR JWT_FILE CHIADO_CONFIG_DIR FRESH_GENESIS; do
    [[ -e "${!v:-}" ]] || { echo "✗ $v=${!v:-<unset>} missing — finish the One-time setup steps first"; exit 1; }
done
: "${L2_DATA_DIR:?set L2_DATA_DIR in .env.chiado}"
: "${LIGHTHOUSE_DATA:?set LIGHTHOUSE_DATA in .env.chiado}"

# Native blob DA is consensus-critical and mandatory.
: "${EEZ_L1_BUILDER_RPC_URL:?set the Chiado eth_sendBundle endpoint}"
: "${EEZ_L1_BEACON_URL:?set EEZ_L1_BEACON_URL for blob derivation}"
: "${EEZ_L1_BLOB_SIDECAR_ENCODING:?set EEZ_L1_BLOB_SIDECAR_ENCODING=eip7594 for post-Fulu Chiado}"
[[ "$EEZ_L1_BLOB_SIDECAR_ENCODING" == "eip7594" ]] || {
    echo "✗ Chiado is post-Fulu; EEZ_L1_BLOB_SIDECAR_ENCODING must be eip7594" >&2
    exit 1
}
[[ "$EEZ_L1_BEACON_URL" == http://* || "$EEZ_L1_BEACON_URL" == https://* ]] || {
    echo "✗ EEZ_L1_BEACON_URL must be an http(s) URL" >&2
    exit 1
}

# Every settlement, including a blob-backed one, is submitted atomically via
# eth_sendBundle. Accept relay validation errors for the deliberately empty
# probe, but reject an unreachable endpoint or a relay without the method.
builder_probe="$(curl --fail --silent --show-error --max-time 15 \
    -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","method":"eth_sendBundle","params":[{"txs":[],"blockNumber":"0x0"}],"id":1}' \
    "$EEZ_L1_BUILDER_RPC_URL")" || {
        echo "✗ Chiado builder is unreachable: $EEZ_L1_BUILDER_RPC_URL" >&2
        exit 1
    }
if ! jq -e 'has("result") or (has("error") and .error.code != -32601)' >/dev/null <<<"$builder_probe"; then
    echo "✗ Chiado builder does not expose eth_sendBundle" >&2
    exit 1
fi
echo "==> builder eth_sendBundle endpoint available"

# ── 1. Deploy the protocol ─────────────────────────────────────────────
if [[ -f deployments.env ]]; then
    echo "==> [1/5] deployments.env already present — skipping deploy (rm it first to redeploy)"
    fresh_deploy=0
else
    echo "==> [1/5] deploying protocol on chiado"
    EEZ_DEPLOY_SKIP_SIMULATION="${EEZ_DEPLOY_SKIP_SIMULATION:-1}" make deploy-protocol
    fresh_deploy=1
fi
set -a; source deployments.env; set +a
: "${EEZ_REGISTRY_ADDRESS:?deploy did not write EEZ_REGISTRY_ADDRESS}"
: "${EEZ_INITIAL_BLOCK_HASH:?deployments.env predates block-hash commitments; redeploy on a fresh registry}"
: "${EEZ_L2_GENESIS_PATH:?deploy did not write EEZ_L2_GENESIS_PATH}"
if ! cmp -s "$FRESH_GENESIS" "$EEZ_L2_GENESIS_PATH"; then
    echo "✗ FRESH_GENESIS must be byte-identical to the genesis rendered and registered by deploy.sh" >&2
    echo "  mounted:    $FRESH_GENESIS" >&2
    echo "  registered: $EEZ_L2_GENESIS_PATH" >&2
    exit 1
fi
START_L1_BLOCK="$(cast block-number --rpc-url "$DEPLOY_RPC")"
START_ROOT="$(cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' "${EEZ_ROLLUP_ID:-1}" \
    --rpc-url "$DEPLOY_RPC" | sed -n '2p' | tr -d '[:space:]')"
[[ -n "$START_ROOT" ]] || { echo "✗ could not read the pre-start Chiado commitment" >&2; exit 1; }

# ── 2. Prepare datadirs ─────────────────────────────────────────────────
echo "==> [2/5] preparing datadirs"
if [[ "$fresh_deploy" == 1 ]]; then
    echo "    fresh deploy → wiping L2/lighthouse datadirs (new genesis, old chain data would diverge)"
    rm -rf "$L2_DATA_DIR" "$LIGHTHOUSE_DATA"
fi
mkdir -p "$L2_DATA_DIR" "$LIGHTHOUSE_DATA"

# ── 3. Start the stack ──────────────────────────────────────────────────
echo "==> [3/5] docker compose up (eez-node + eez-proof-signer + lighthouse)"
"${COMPOSE[@]}" up -d --remove-orphans
"${COMPOSE[@]}" ps

echo "==> validating post-Fulu Chiado Beacon blob API"
beacon_base="${EEZ_L1_BEACON_URL%/}"
beacon_ready=0
for i in $(seq 1 90); do
    genesis_json="$(curl -fsS --max-time 5 "$beacon_base/eth/v1/beacon/genesis" 2>/dev/null || true)"
    spec_json="$(curl -fsS --max-time 5 "$beacon_base/eth/v1/config/spec" 2>/dev/null || true)"
    head_json="$(curl -fsS --max-time 5 "$beacon_base/eth/v1/beacon/headers/head" 2>/dev/null || true)"
    blobs_json="$(curl -fsS --max-time 15 "$beacon_base/eth/v1/beacon/blobs/head" 2>/dev/null || true)"
    if jq -e '.data.genesis_time | tonumber' >/dev/null 2>&1 <<<"$genesis_json" &&
        jq -e '.data.CONFIG_NAME == "chiado" and (.data.SECONDS_PER_SLOT | tonumber) == 5' >/dev/null 2>&1 <<<"$spec_json" &&
        jq -e '.data.header.message.slot | tonumber' >/dev/null 2>&1 <<<"$head_json" &&
        jq -e '.data | type == "array"' >/dev/null 2>&1 <<<"$blobs_json"; then
        fulu_epoch="$(jq -r '.data.FULU_FORK_EPOCH | tonumber' <<<"$spec_json")"
        slots_per_epoch="$(jq -r '.data.SLOTS_PER_EPOCH | tonumber' <<<"$spec_json")"
        head_slot="$(jq -r '.data.header.message.slot | tonumber' <<<"$head_json")"
        if (( slots_per_epoch > 0 && head_slot / slots_per_epoch >= fulu_epoch )); then
            beacon_ready=1
            echo "    ✓ Chiado Fulu active; full-blob API available at slot $head_slot"
            break
        fi
    fi
    echo "    [$i] waiting for synchronized semi-supernode blob API"
    sleep 5
done
[[ "$beacon_ready" == 1 ]] || {
    echo "✗ Beacon endpoint did not provide synchronized post-Fulu full blobs" >&2
    echo "  Check: ${COMPOSE[*]} logs --tail=100 lighthouse" >&2
    exit 1
}

# ── 4. Wait for a healthy pipeline ──────────────────────────────────────
echo "==> [4/5] waiting for L1 catch-up + L2 production + first settle (up to ~15 min)"
L1=http://localhost:18645
L2=http://localhost:18688
healthy=0
for i in $(seq 1 90); do
    l2h=$(cast block-number --rpc-url "$L2" 2>/dev/null || echo 0)
    l1r=$(cast call "$EEZ_REGISTRY_ADDRESS" 'rollups(uint64)(address,bytes32,uint256)' "${EEZ_ROLLUP_ID:-1}" \
            --rpc-url "$L1" 2>/dev/null | sed -n '2p' | tr -d '[:space:]' || echo "")
    echo "    [$i] L2 head=$l2h  L1 commitment=${l1r:0:14}…"
    if [[ "$l2h" -gt 3 && -n "$l1r" && "$l1r" != "$START_ROOT" ]]; then
        healthy=1; echo "    ✓ pipeline healthy"; break
    fi
    sleep 10
done
[[ "$healthy" == 1 ]] || { echo "✗ not healthy in window; check: ${COMPOSE[*]} logs --tail=100 eez-node"; exit 1; }

# Bind the health result to an actual post-start settlement. This also proves
# that the public builder included a type-3 transaction and that
# every committed blob is retrievable through the post-Fulu Beacon API.
batch_logs="$(cast logs --address "$EEZ_REGISTRY_ADDRESS" \
    --from-block "$START_L1_BLOCK" --to-block latest \
    'BatchPosted(uint256)' --rpc-url "$L1" --json 2>/dev/null || true)"
settlement_tx="$(jq -r 'if type == "array" and length > 0 then .[-1].transactionHash else empty end' <<<"$batch_logs")"
[[ "$settlement_tx" =~ ^0x[[:xdigit:]]{64}$ ]] || {
    echo "✗ commitment advanced but no post-start BatchPosted transaction was found" >&2
    exit 1
}

settlement_json="$(cast rpc eth_getTransactionByHash "$settlement_tx" --rpc-url "$L1")"
settlement_type="$(jq -r '.type | ascii_downcase' <<<"$settlement_json")"
[[ "$settlement_type" == "0x3" || "$settlement_type" == "0x03" ]] || {
    echo "✗ blob settlement $settlement_tx has transaction type $settlement_type" >&2
    exit 1
}
mapfile -t settlement_blob_hashes < <(jq -er '.blobVersionedHashes[]' <<<"$settlement_json")
(( ${#settlement_blob_hashes[@]} > 0 )) || {
    echo "✗ blob settlement $settlement_tx has no versioned hashes" >&2
    exit 1
}
settlement_block_hash="$(jq -er '.blockHash' <<<"$settlement_json")"
settlement_block="$(cast rpc eth_getBlockByHash "$settlement_block_hash" false --rpc-url "$L1")"
settlement_timestamp="$(( $(jq -er '.timestamp' <<<"$settlement_block") ))"
beacon_genesis_time="$(jq -r '.data.genesis_time | tonumber' <<<"$genesis_json")"
beacon_seconds_per_slot="$(jq -r '.data.SECONDS_PER_SLOT | tonumber' <<<"$spec_json")"
(( settlement_timestamp >= beacon_genesis_time )) || {
    echo "✗ settlement timestamp predates configured Beacon genesis" >&2
    exit 1
}
settlement_elapsed=$((settlement_timestamp - beacon_genesis_time))
(( settlement_elapsed % beacon_seconds_per_slot == 0 )) || {
    echo "✗ settlement timestamp does not align to a Chiado Beacon slot" >&2
    exit 1
}
settlement_slot=$((settlement_elapsed / beacon_seconds_per_slot))
for versioned_hash in "${settlement_blob_hashes[@]}"; do
    selected_blob="$(curl -fsS --max-time 30 --get \
        --data-urlencode "versioned_hashes=$versioned_hash" \
        "$beacon_base/eth/v1/beacon/blobs/$settlement_slot")" || {
            echo "✗ Beacon API cannot retrieve settlement blob $versioned_hash" >&2
            exit 1
        }
    jq -e '.data | type == "array" and length == 1 and
        (.[0] | type == "string" and startswith("0x") and length == 262146)' \
        >/dev/null <<<"$selected_blob" || {
            echo "✗ Beacon API returned an invalid full blob for $versioned_hash" >&2
            exit 1
        }
done
echo "    ✓ settlement $settlement_tx: type 3, ${#settlement_blob_hashes[@]} versioned-hash-selected blob(s) retrievable"

# ── 5. Report ────────────────────────────────────────────────────────────
echo
echo "════════════════════════════════════════════════════════════════"
echo " chiado L2 is UP"
echo "════════════════════════════════════════════════════════════════"
echo " RPC endpoints:"
echo "   L2 RPC                          $L2"
echo "   Embedded chiado L1 RPC          $L1"
echo "   L1→L2 front (Inbound)           http://localhost:18999"
echo "   L2→L1 front (Outbound)          http://localhost:18998"
echo
echo " Deployment (rollupId ${EEZ_ROLLUP_ID:-1}, deploy block ${EEZ_REGISTRY_DEPLOY_BLOCK:-?}):"
printf "   %-20s %s\n" "EEZ registry"      "$EEZ_REGISTRY_ADDRESS"
printf "   %-20s %s\n" "Rollup manager"    "${EEZ_ROLLUP_MANAGER_ADDRESS:-?}"
printf "   %-20s %s\n" "ECDSA proof system" "${EEZ_ECDSA_PROOF_SYSTEM_ADDRESS:-?}"
printf "   %-20s %s\n" "L1 bridge sender"  "${EEZ_L1_BRIDGE_SENDER:-?}"
if [[ -n "${EEZ_BLOCKSCOUT_URL:-}" ]]; then
    printf "   Blockscout: %s/address/%s\n" "${EEZ_BLOCKSCOUT_URL%/}" "$EEZ_REGISTRY_ADDRESS"
fi
echo
echo " Next:"
echo "   ${COMPOSE[*]} logs -f eez-node          # follow the node"
echo "   EEZ_WAVE_COUNT=5 bash scripts/xchain-test.sh   # exercise cross-chain"
echo "   bash scripts/teardown-chiado.sh                # stop the stack"
echo "════════════════════════════════════════════════════════════════"
