#!/usr/bin/env bash
# Independently execute the exact range verified by verify-blob-da.sh.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
EVIDENCE="$(realpath "${1:?usage: verify-policy-replay.sh path/to/summary.json}")"
ENCLAVE="$(jq -er .enclave "$EVIDENCE")"
export KURTOSIS_ENCLAVE="$ENCLAVE"
source "$HERE/ports.sh" >/dev/null
source "$HERE/scripts/lib.sh"
TEMP="$(mktemp -d /tmp/eez-policy-replay.XXXXXX)"
trap 'rm -rf "$TEMP"' EXIT
mkdir -p "$TEMP/deployment"
load_deployment_artifact "$TEMP/deployment"
if [[ "${EEZ_USE_LOCAL_DEPLOYMENTS:-0}" == "1" ]]; then
    export EEZ_KURTOSIS_L2_GENESIS="${EEZ_L2_GENESIS:?set the deployed L2 genesis path}"
    [[ -r "$EEZ_KURTOSIS_L2_GENESIS" ]] || { echo "L2 genesis is not readable" >&2; exit 1; }
    export EEZ_L1_CHAIN_CONFIG="${EEZ_L1_CHAIN_CONFIG:?set the deployed L1 chain profile}"
    export EEZ_L1_BEACON_URL="${EEZ_L1_BEACON_URL:?set the deployed Beacon API}"
    export EEZ_L2_BLOCK_TIME_MS="${EEZ_L2_BLOCK_TIME_MS:?set the deployed L2 cadence}"
    export EEZ_L1_CHAIN_ID="$(cast chain-id --rpc-url "$EEZ_DEVNET_L1_RPC")"
else
    kurtosis files download "$ENCLAVE" el_cl_genesis_data "$TEMP/network" >/dev/null
    export EEZ_KURTOSIS_L2_GENESIS="$TEMP/deployment/l2-genesis.json"
    export EEZ_L1_CHAIN_CONFIG="$TEMP/network/genesis.json"
    export EEZ_L1_BEACON_URL="$(kurtosis port print "$ENCLAVE" cl-1-lighthouse-reth http)"
    export EEZ_L1_CHAIN_ID="$(jq -er .config.chainId "$EEZ_L1_CHAIN_CONFIG")"
    export EEZ_L2_BLOCK_TIME_MS="${EEZ_L2_BLOCK_TIME_MS:-2000}"
fi
# Use exactly the deployment selected by the caller or enclave.
set -a
source "$TEMP/deployment/deployments.env"
set +a
export EEZ_KURTOSIS_POLICY_EVIDENCE="$EVIDENCE"
export EEZ_L1_RPC_URL="$EEZ_DEVNET_L1_RPC"
[[ "$EEZ_L1_BEACON_URL" == http* ]] || export EEZ_L1_BEACON_URL="http://$EEZ_L1_BEACON_URL"
export EEZ_ROLLUP_ID="$(jq -er .rollup_id "$EVIDENCE")"
# Preserve the enclave's actual timing; only the existing proof/slack windows
# are explicit test values (no proving occurs in this L1-derived follower).
export EEZ_L1_BLOCK_TIME_MS="$(curl -fsS "$EEZ_L1_BEACON_URL/eth/v1/config/spec" | jq -er '.data.SECONDS_PER_SLOT | tonumber * 1000')"
export EEZ_PROOF_TIME_MS="${EEZ_PROOF_TIME_MS:-4000}"
export EEZ_SUBMISSION_SLACK_MS="${EEZ_SUBMISSION_SLACK_MS:-500}"
cd "$REPO"
cargo test --locked -p eez-node-e2e --test kurtosis_policy_replay -- --ignored --test-threads=1
