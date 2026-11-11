#!/usr/bin/env bash
# Fee-aware load exercise; selected wallets are shared across both checkouts.
set -euo pipefail
set +x
umask 077
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
: "${EEZ_LOAD_DEPLOYMENT_ROOT:?Select the existing Chiado deployment}"
: "${EEZ_LOAD_OUTPUT:?Select a private evidence directory}"
: "${EEZ_LOAD_UI_ROOT:?Select the UI checkout containing pinned viem}"
mkdir -p "$ROOT/eez-core-protocol/tmp"
exec 200>"$EEZ_LOAD_DEPLOYMENT_ROOT/eez-core-protocol/tmp/.eez-network-e2e.lock"
flock -n 200
if [[ "$ROOT" != "$EEZ_LOAD_DEPLOYMENT_ROOT" ]]; then
    exec 201>"$ROOT/eez-core-protocol/tmp/.eez-network-e2e.lock"
    flock -n 201
fi
set -a
source "$EEZ_LOAD_DEPLOYMENT_ROOT/eez-core-protocol/chain.env"
source "$EEZ_LOAD_DEPLOYMENT_ROOT/deployments.env"
set +a
STARTED=$(date -u +%FT%TZ)
finish() {
    local rc=$?
    trap - EXIT
    printf '%s\n' "$rc" > "$EEZ_LOAD_OUTPUT/run.exit"
    docker logs --since "$STARTED" eez-node-chiado > "$EEZ_LOAD_OUTPUT/node.log" 2>&1 || true
    docker logs --since "$STARTED" eez-proof-signer-chiado > "$EEZ_LOAD_OUTPUT/signer.log" 2>&1 || true
}
trap finish EXIT
node scripts/chiado-fee-load.mjs prepare
node scripts/chiado-fee-load.mjs run
node scripts/chiado-fee-load.mjs verify
