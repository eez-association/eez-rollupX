#!/usr/bin/env bash
# Static regression check for the post-Fulu Chiado deployment profile.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

for tool in docker jq; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

# Lighthouse v8 reads bootstrap_nodes.yaml for custom networks. The older
# boot_enr.yaml filename alone silently leaves peer discovery without seeds.
[[ -s configs/chiado/bootstrap_nodes.yaml ]] || {
    echo "Chiado Lighthouse bootstrap_nodes.yaml is missing or empty" >&2
    exit 1
}
cmp -s configs/chiado/bootstrap_nodes.yaml configs/chiado/bootnodes.yaml

# Compose v2 validates service env_file paths even with --no-env-resolution.
# Render against an empty fixture, never the checkout's deployment bindings.
config_dir="$(mktemp -d)"
trap 'rm -f -- "$config_dir/deployments.env"; rmdir -- "$config_dir"' EXIT
touch "$config_dir/deployments.env"

config="$(
    docker compose \
        --project-directory "$config_dir" \
        --env-file "$REPO/.env.chiado.example" \
        -f "$REPO/docker-compose.chiado-node.yml" \
        config --no-env-resolution --format json
)"

jq -e '
    .services["eez-node"].environment.EEZ_L1_CHAIN_ID == "10200" and
    .services["eez-node"].environment.EEZ_L1_BLOCK_TIME_MS == "5000" and
    .services["eez-node"].environment.EEZ_PROOF_TIME_MS == "1500" and
    .services["eez-node"].environment.EEZ_SUBMISSION_SLACK_MS == "700" and
    .services["eez-node"].environment.EEZ_BUNDLE_SUBMISSION_LEAD_MS == "7500" and
    .services["eez-node"].environment.EEZ_L1_BEACON_URL == "http://127.0.0.1:6152/" and
    .services["eez-node"].environment.EEZ_L1_BLOB_SIDECAR_ENCODING == "eip7594" and
    .services.lighthouse.image == "sigp/lighthouse:v8.1.3" and
    (.services.lighthouse.command | index("--semi-supernode") != null) and
    (.services.lighthouse.command | index("--supernode") == null) and
    (.services.lighthouse.command | index("--http-port=6152") != null)
' >/dev/null <<<"$config"

bash -n scripts/chiado-up.sh
echo "Chiado post-Fulu blob deployment configuration: PASS"
