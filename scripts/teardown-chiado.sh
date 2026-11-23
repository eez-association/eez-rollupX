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
if [[ -n "$left" ]]; then
    echo "ERROR: cleanup incomplete. still running=[$left]"
    exit 1
fi
echo "✓ chiado stack torn down (L1/L2 datadirs on disk are untouched)"
