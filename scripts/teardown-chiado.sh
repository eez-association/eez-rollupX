#!/usr/bin/env bash
# Tear down the dockerized chiado stack (docker-compose.chiado-node.yml):
# eez-node, eez-proof-signer, lighthouse. Idempotent — safe to call even if
# nothing is running.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"; cd "$REPO" || exit 1
[[ -f .env ]] || { echo "✗ .env missing — nothing to tear down (or wrong dir)"; exit 1; }

docker compose -f docker-compose.chiado-node.yml down || {
    echo "ERROR: compose teardown failed"; exit 1; }

# A failing `docker ps` would leave `left` empty and read as a clean teardown.
left="$(docker ps --filter 'name=eez-node-chiado' --filter 'name=eez-proof-signer-chiado' --filter 'name=eez-chiado-lighthouse' --format '{{.Names}}')" || {
    echo "ERROR: cannot verify container cleanup"; exit 1; }
if [[ -n "$left" ]]; then
    echo "ERROR: cleanup incomplete. still running=[$left]"
    exit 1
fi
echo "✓ chiado stack torn down (L1/L2 datadirs on disk are untouched)"
