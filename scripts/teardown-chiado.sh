#!/usr/bin/env bash
# Tear down the dockerized chiado stack (docker-compose.chiado-node.yml):
# eez-node, eez-proof-signer, lighthouse. Idempotent — safe to call even if
# nothing is running.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"; cd "$REPO" || exit 1
[[ -f .env ]] || { echo "✗ .env missing — nothing to tear down (or wrong dir)"; exit 1; }

COMPOSE=(docker compose -f docker-compose.chiado-node.yml)
"${COMPOSE[@]}" down || {
    echo "ERROR: compose teardown failed"; exit 1; }

# Verify cleanup in the same project even when another Chiado stack is running.
left="$("${COMPOSE[@]}" ps --all --quiet eez-node eez-proof-signer lighthouse)" || {
    echo "ERROR: cannot verify container cleanup"; exit 1; }
# A project rename can make both down and ps select an empty project while the
# previous stack still owns this checkout's bind mounts.
previous="$(docker ps --all --quiet \
    --filter "label=com.docker.compose.project.working_dir=$REPO" \
    --filter "label=com.docker.compose.project.config_files=$REPO/docker-compose.chiado-node.yml")" || {
    echo "ERROR: cannot verify checkout cleanup"; exit 1; }
if [[ -n "$left" || -n "$previous" ]]; then
    echo "ERROR: cleanup incomplete. remaining container IDs: $left $previous"
    exit 1
fi
echo "✓ chiado stack torn down (L1/L2 datadirs on disk are untouched)"
