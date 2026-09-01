#!/usr/bin/env bash
# Correlate independently verified blob-DA evidence with Blobscan's indexed
# transaction metadata, KZG commitments, and PostgreSQL-backed blob bytes.
set -euo pipefail

KURTOSIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-blob}"
TIMEOUT="${EEZ_BLOBSCAN_TIMEOUT:-300}"
SUMMARY_FILE="${1:?usage: verify-blobscan.sh BLOB_DA_SUMMARY [OUTPUT_FILE]}"
OUTPUT_FILE="${2:-$(dirname "$SUMMARY_FILE")/blobscan.json}"

for tool in cast curl jq kurtosis sha256sum xxd; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done
[[ "$TIMEOUT" =~ ^[0-9]+$ ]] || { echo "EEZ_BLOBSCAN_TIMEOUT must be seconds" >&2; exit 1; }
[[ -f "$SUMMARY_FILE" ]] || { echo "blob-DA summary not found: $SUMMARY_FILE" >&2; exit 1; }
jq -e '.verified == true and (.verified_blob_batches | length > 0)' "$SUMMARY_FILE" >/dev/null || {
    echo "blob-DA summary is not verified or contains no batches: $SUMMARY_FILE" >&2
    exit 1
}

temp_dir="$(mktemp -d /tmp/eez-blobscan-verify.XXXXXX)"
trap 'rm -rf -- "$temp_dir"' EXIT

export KURTOSIS_ENCLAVE="$ENCLAVE"
source "$KURTOSIS_DIR/ports.sh" >/dev/null
BLOBSCAN_API="${BLOBSCAN_API:-${EEZ_DEVNET_BLOBSCAN_API:-}}"
BLOBSCAN_WEB="${BLOBSCAN_WEB:-${EEZ_DEVNET_BLOBSCAN:-}}"
L1_RPC="${L1_RPC:-$EEZ_DEVNET_L1_RPC}"
[[ -n "$BLOBSCAN_API" && -n "$BLOBSCAN_WEB" ]] || {
    echo "Blobscan endpoints are not available in enclave '$ENCLAVE'" >&2
    exit 1
}

http_url() {
    case "$1" in
        http://*|https://*) printf '%s\n' "$1" ;;
        *) printf 'http://%s\n' "$1" ;;
    esac
}

BEACON_URL="${EEZ_L1_BEACON_URL:-$(http_url "$(kurtosis port print "$ENCLAVE" cl-1-lighthouse-reth http)")}"

[[ "$(curl -fsS "$BLOBSCAN_API/healthcheck" | jq -r '.')" == "yay!" ]] || {
    echo "Blobscan API healthcheck failed" >&2
    exit 1
}
[[ "$(curl -fsS -o /dev/null -w '%{http_code}' "$BLOBSCAN_WEB/")" == "200" ]] || {
    echo "Blobscan web healthcheck failed" >&2
    exit 1
}

l1_chain_id="$(cast chain-id --rpc-url "$L1_RPC")"
expected_chain_ids="$(jq -c '[.verified_blob_batches[].l1_chain_id] | unique' "$SUMMARY_FILE")"
[[ "$expected_chain_ids" == "[$l1_chain_id]" ]] || {
    echo "live L1 chain ID $l1_chain_id does not match evidence $expected_chain_ids" >&2
    exit 1
}

transactions_evidence='[]'
blobs_evidence='[]'
batch_count="$(jq -r '.verified_blob_batches | length' "$SUMMARY_FILE")"

for ((batch_position = 0; batch_position < batch_count; batch_position++)); do
    batch="$(jq -c --argjson position "$batch_position" '.verified_blob_batches[$position]' "$SUMMARY_FILE")"
    tx_hash="$(jq -er '.l1_transaction_hash | ascii_downcase' <<<"$batch")"
    block_number="$(jq -er '.l1_block_number' <<<"$batch")"
    beacon_slot="$(jq -er '.beacon_slot' <<<"$batch")"
    blob_count="$(jq -er '.blob_count' <<<"$batch")"
    expected_hashes="$(jq -c '[.blob_versioned_hashes[] | ascii_downcase] | sort' <<<"$batch")"

    tx_file="$temp_dir/tx-$batch_position.json"
    deadline=$((SECONDS + TIMEOUT))
    while (( SECONDS < deadline )); do
        if curl -fsS "$BLOBSCAN_API/transactions/$tx_hash" -o "$tx_file" 2>/dev/null &&
            jq -e --arg tx "$tx_hash" --argjson block "$block_number" \
                '(.hash | ascii_downcase) == $tx and .blockNumber == $block' \
                "$tx_file" >/dev/null 2>&1; then
            break
        fi
        sleep 2
    done
    if ! jq -e --arg tx "$tx_hash" --argjson block "$block_number" \
        '(.hash | ascii_downcase) == $tx and .blockNumber == $block' \
        "$tx_file" >/dev/null 2>&1; then
        echo "Blobscan did not index posting transaction $tx_hash within ${TIMEOUT}s" >&2
        exit 1
    fi

    observed_hashes="$(jq -c '[.blobs[].versionedHash | ascii_downcase] | sort' "$tx_file")"
    observed_blob_count="$(jq -r '.blobs | length' "$tx_file")"
    if [[ "$observed_hashes" != "$expected_hashes" || "$observed_blob_count" != "$blob_count" ]]; then
        echo "Blobscan blob set mismatch for posting transaction $tx_hash" >&2
        echo "  expected: $expected_hashes" >&2
        echo "  observed: $observed_hashes" >&2
        exit 1
    fi

    tx_item="$(
        jq -cn \
            --arg tx_hash "$tx_hash" \
            --argjson block_number "$block_number" \
            --argjson beacon_slot "$beacon_slot" \
            --argjson blob_count "$blob_count" \
            '{transaction_hash:$tx_hash,block_number:$block_number,
              beacon_slot:$beacon_slot,blob_count:$blob_count,indexed:true}'
    )"
    transactions_evidence="$(jq -c --argjson item "$tx_item" '. + [$item]' <<<"$transactions_evidence")"

    sidecars_file="$temp_dir/beacon-blobs-$batch_position.json"
    curl -fsS "$BEACON_URL/eth/v1/beacon/blobs/$beacon_slot" -o "$sidecars_file" ||
        curl -fsS "$BEACON_URL/eth/v1/beacon/blob_sidecars/$beacon_slot" -o "$sidecars_file"

    for ((blob_position = 0; blob_position < blob_count; blob_position++)); do
        versioned_hash="$(jq -er --argjson position "$blob_position" '.blob_versioned_hashes[$position] | ascii_downcase' <<<"$batch")"
        commitment="$(jq -er --argjson position "$blob_position" '.kzg_commitments[$position] | ascii_downcase' <<<"$batch")"
        blob_index="$(jq -er --argjson position "$blob_position" '.blob_indices[$position] | tostring' <<<"$batch")"

        detail_file="$temp_dir/detail-$batch_position-$blob_position.json"
        curl -fsS "$BLOBSCAN_API/blobs/$versioned_hash" -o "$detail_file"
        jq -e \
            --arg hash "$versioned_hash" \
            --arg commitment "$commitment" \
            --arg tx "$tx_hash" \
            '(.versionedHash | ascii_downcase) == $hash and
             (.commitment | ascii_downcase) == $commitment and
             .size == 131072 and
             any(.dataStorageReferences[]; .storage == "postgres") and
             any(.transactions[]; (.txHash | ascii_downcase) == $tx)' \
            "$detail_file" >/dev/null || {
                echo "Blobscan metadata mismatch for $versioned_hash" >&2
                exit 1
            }

        indexed_blob="$(curl -fsS "$BLOBSCAN_API/blobs/$versioned_hash/data" | jq -er '.')"
        beacon_blob="$(
            jq -er --arg blob "${indexed_blob,,}" \
                '.data[] | (if type == "object" then .blob else . end) |
                 select(ascii_downcase == $blob)' \
                "$sidecars_file" | head -n 1
        )"
        [[ -n "$beacon_blob" ]] || {
            echo "Blobscan bytes are absent from the canonical Beacon response for $versioned_hash" >&2
            exit 1
        }
        beacon_blob_sha256="$(printf '%s' "${beacon_blob#0x}" | xxd -r -p | sha256sum | awk '{print $1}')"
        indexed_blob_sha256="$(printf '%s' "${indexed_blob#0x}" | xxd -r -p | sha256sum | awk '{print $1}')"
        if [[ "$indexed_blob_sha256" != "$beacon_blob_sha256" ]]; then
            echo "Blobscan bytes do not match beacon sidecar for $versioned_hash" >&2
            exit 1
        fi

        storage_url="$(
            jq -er '.dataStorageReferences[] | select(.storage == "postgres") | .url' \
                "$detail_file" | head -n 1
        )"
        encoded_storage_url="$(printf '%s' "$storage_url" | jq -sRr @uri)"
        web_blob_file="$temp_dir/web-blob-$batch_position-$blob_position.bin"
        web_status="$(
            curl -sS -o "$web_blob_file" -w '%{http_code}' \
                "$BLOBSCAN_WEB/api/blob-data?storage=postgres&url=$encoded_storage_url"
        )"
        if [[ "$web_status" != "200" ]]; then
            echo "Blobscan web proxy returned HTTP $web_status for $versioned_hash" >&2
            exit 1
        fi
        web_blob_size="$(wc -c <"$web_blob_file" | tr -d ' ')"
        web_blob_sha256="$(sha256sum "$web_blob_file" | awk '{print $1}')"
        if [[ "$web_blob_size" != "131072" || "$web_blob_sha256" != "$beacon_blob_sha256" ]]; then
            echo "Blobscan web bytes do not match beacon sidecar for $versioned_hash" >&2
            echo "  expected: size=131072 sha256=$beacon_blob_sha256" >&2
            echo "  observed: size=$web_blob_size sha256=$web_blob_sha256" >&2
            exit 1
        fi

        blob_item="$(
            jq -cn \
                --arg versioned_hash "$versioned_hash" \
                --arg commitment "$commitment" \
                --arg transaction_hash "$tx_hash" \
                --arg blob_sha256 "$indexed_blob_sha256" \
                --argjson beacon_index "$blob_index" \
                '{versioned_hash:$versioned_hash,commitment:$commitment,
                  transaction_hash:$transaction_hash,beacon_index:$beacon_index,
                  blob_sha256:$blob_sha256,postgres_storage:true,
                  data_matches_beacon_sidecar:true,
                  web_proxy_matches_beacon_sidecar:true}'
        )"
        blobs_evidence="$(jq -c --argjson item "$blob_item" '. + [$item]' <<<"$blobs_evidence")"
    done
done

mkdir -p "$(dirname "$OUTPUT_FILE")"
temp_output="$temp_dir/blobscan.json"
jq -n \
    --arg enclave "$ENCLAVE" \
    --arg api_url "$BLOBSCAN_API" \
    --arg web_url "$BLOBSCAN_WEB" \
    --argjson l1_chain_id "$l1_chain_id" \
    --argjson transactions "$transactions_evidence" \
    --argjson blobs "$blobs_evidence" \
    '{verified:true,enclave:$enclave,l1_chain_id:$l1_chain_id,
      api_url:$api_url,web_url:$web_url,transactions:$transactions,blobs:$blobs}' \
    >"$temp_output"
mv "$temp_output" "$OUTPUT_FILE"

echo "==> Blobscan verification passed"
echo "    transactions: $(jq -r '.transactions | length' "$OUTPUT_FILE")"
echo "    blobs:        $(jq -r '.blobs | length' "$OUTPUT_FILE")"
echo "    evidence:     $OUTPUT_FILE"
