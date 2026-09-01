#!/usr/bin/env bash
# Static regression check for the post-Fulu Chiado deployment profile.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

for tool in docker jq; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

config="$(
    docker compose \
        --env-file .env.chiado.example \
        -f docker-compose.chiado-node.yml \
        config --no-env-resolution --format json
)"

jq -e '
    .services["eez-node"].environment.EEZ_L1_CHAIN_ID == "10200" and
    .services["eez-node"].environment.EEZ_L1_BLOCK_TIME_MS == "5000" and
    .services["eez-node"].environment.EEZ_DA_MODE == "blob" and
    .services["eez-node"].environment.EEZ_L1_BEACON_URL == "http://127.0.0.1:6152/" and
    .services["eez-node"].environment.EEZ_L1_BLOB_SIDECAR_ENCODING == "eip7594" and
    .services.lighthouse.image == "sigp/lighthouse:v8.1.3" and
    (.services.lighthouse.command | index("--supernode") != null) and
    (.services.lighthouse.command | index("--http-port=6152") != null)
' >/dev/null <<<"$config"

bash -n scripts/chiado-up.sh
echo "Chiado post-Fulu blob deployment configuration: PASS"
