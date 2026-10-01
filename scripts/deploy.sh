#!/usr/bin/env bash
#
# Deploys EEZ + ECDSAProofSystem + Rollup manager and registers the rollup.
#
# Reads from .env (poster key, proof signer key, RPC url, etc.).
# Writes deployments.env with the addresses + rollupId + deploy block.
# `eez-node` auto-loads BOTH .env and deployments.env (via dotenvy at
# startup), so a successful run of this script leaves the workspace
# ready for `make run-node`.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${EEZ_ENV_FILE:-$REPO/.env}"
OUT_FILE="${EEZ_DEPLOYMENTS_FILE:-$REPO/deployments.env}"

# Timestamp marker — used at the end to find broadcast/ entries produced
# by *this* run (vs stale ones from prior chain ids or earlier deploys).
START_MARKER="$(mktemp -t eez-deploy.XXXXXX)"
GENESIS_TIMESTAMP_TMP=""
cleanup() {
    rm -f "$START_MARKER"
    [[ -z "$GENESIS_TIMESTAMP_TMP" ]] || rm -f "$GENESIS_TIMESTAMP_TMP"
}
trap cleanup EXIT

# ── Inputs from .env ──────────────────────────────────────────────────
[[ -f "$ENV_FILE" ]] || { echo "deploy: $ENV_FILE not found — copy .env.example to .env first" >&2; exit 1; }
# shellcheck disable=SC1090
source "$ENV_FILE"

: "${EEZ_L1_RPC_URL:?EEZ_L1_RPC_URL not set in .env}"
: "${EEZ_L1_POSTER_KEY:?EEZ_L1_POSTER_KEY not set in .env}"
: "${EEZ_PROOF_SIGNER_KEY:?EEZ_PROOF_SIGNER_KEY not set in .env}"

configured_initial_block_hash="${EEZ_INITIAL_BLOCK_HASH:-}"

# Deploy/own key, decoupled from the composer's poster so posting can't advance
# the deployer's nonce or shift the CREATE addresses. Defaults to the poster.
EEZ_DEPLOY_KEY="${EEZ_DEPLOY_KEY:-$EEZ_L1_POSTER_KEY}"

# Derive addresses from keys.
AUTHORIZED_SIGNER="$(cast wallet address --private-key "$EEZ_PROOF_SIGNER_KEY")"
PROOF_SYSTEM_VKEY="0x000000000000000000000000${AUTHORIZED_SIGNER#0x}"
OWNER="$(cast wallet address --private-key "$EEZ_DEPLOY_KEY")"
POSTER_ADDRESS="$(cast wallet address --private-key "$EEZ_L1_POSTER_KEY")"
# EEZL2 sweeps ether found at a proxy address during deployment to this L2
# account. It is baked into the genesis predeploy; default to the L1 poster.
EEZ_L2_RECOVERY_ADDRESS="${EEZ_L2_RECOVERY_ADDRESS:-$POSTER_ADDRESS}"
EEZ_L2_SYSTEM_ADDRESS="0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076"
if [[ "${AUTHORIZED_SIGNER,,}" == "${EEZ_L2_SYSTEM_ADDRESS,,}" ]]; then
    echo "deploy: proof attestation address must differ from the reserved native system address" >&2
    exit 1
fi

echo "deploy: RPC                  = $EEZ_L1_RPC_URL"
echo "deploy: deployer / owner     = $OWNER"
echo "deploy: authorized signer    = $AUTHORIZED_SIGNER"
echo "deploy: L2 system address    = $EEZ_L2_SYSTEM_ADDRESS"
echo "deploy: L2 recovery address  = $EEZ_L2_RECOVERY_ADDRESS"
echo

CONTRACTS="$REPO/contracts"
RPC="--rpc-url $EEZ_L1_RPC_URL"
KEY="--private-key $EEZ_DEPLOY_KEY"

# Each Foundry deploy script logs `KEY=VALUE` lines via `console.log`.
# `forge script --silent` still emits these via the script's stdout
# stream; we capture and grep them out per step.

# Helper: parse `KEY=value` line out of forge output, lowercase the
# value (cast prints checksummed; lowercase keeps comparisons easy).
extract() {
    local key="$1" out="$2"
    grep -oE "${key}=0x[0-9a-fA-F]+" <<< "$out" | tail -1 | cut -d= -f2 | tr '[:upper:]' '[:lower:]'
}
extract_uint() {
    local key="$1" out="$2"
    grep -oE "${key}=[0-9]+" <<< "$out" | tail -1 | cut -d= -f2
}

# Runs a forge script command, capturing combined stdout+stderr. On
# non-zero exit (e.g., RPC down, gas insufficient, simulation revert),
# prints the captured output before bailing — without this wrapper,
# `set -e` would abort the `OUT=$(...)` assignment and forge's error
# message would be lost.
run_forge() {
    local label="$1"; shift
    local extra=()
    # On flaky public RPCs (chiado endpoints), forge's pre-broadcast
    # simulation refetches state at a block the RPC hasn't finalized
    # yet → "block not found". Smokes set EEZ_DEPLOY_SKIP_SIMULATION=1
    # to bypass that phase. Local dev L1 leaves it unset.
    if [[ "${EEZ_DEPLOY_SKIP_SIMULATION:-0}" == "1" ]]; then
        extra+=(--skip-simulation)
    fi
    if ! OUT="$(cd "$CONTRACTS" && "$@" "${extra[@]}" 2>&1)"; then
        echo "$OUT" >&2
        echo "deploy: $label failed (forge non-zero exit)" >&2
        exit 1
    fi
}

compute_genesis_state_root() {
    local genesis="$1"
    if command -v eez-genesis-state-root >/dev/null 2>&1; then
        eez-genesis-state-root "$genesis"
    else
        (
            cd "$REPO"
            cargo run --quiet --locked --package eez-node --example genesis_state_root -- "$genesis"
        )
    fi
}

compute_genesis_block_hash() {
    local genesis="$1"
    if command -v eez-genesis-block-hash >/dev/null 2>&1; then
        eez-genesis-block-hash "$genesis"
    else
        (
            cd "$REPO"
            cargo run --quiet --locked --package eez-node --example genesis_block_hash -- "$genesis"
        )
    fi
}

# Render and check both genesis tools before spending deployment gas. This
# fresh registry will register rollup 1. Pin a finalized reference before the
# first broadcast; deployment must not change genesis afterward.
EXPECTED_ROLLUP_ID=1
GENESIS_OUT="${EEZ_GENESIS_OUT:-$REPO/datadir/genesis.json}"
GENESIS_PROFILE_OUT="${EEZ_GENESIS_PROFILE_OUT:-${GENESIS_OUT%.json}.profile.json}"
# genesis.json is the minimal production base (predeploys only, no ETH).
# Development networks select genesis.dev.json, which funds public test keys.
GENESIS_BASE="${EEZ_GENESIS_BASE:-$REPO/genesis.json}"

echo "      rendering L2 genesis (rollupId=$EXPECTED_ROLLUP_ID, system=$EEZ_L2_SYSTEM_ADDRESS, recovery=$EEZ_L2_RECOVERY_ADDRESS)"
"$REPO/scripts/update-eezl2-genesis.sh" --render \
    --rollup-id "$EXPECTED_ROLLUP_ID" \
    --system-address "$EEZ_L2_SYSTEM_ADDRESS" \
    --recovery-address "$EEZ_L2_RECOVERY_ADDRESS" \
    --base "$GENESIS_BASE" \
    --output "$GENESIS_OUT" \
    --profile-output "$GENESIS_PROFILE_OUT"

EEZ_L2_GENESIS_STATE_ROOT="$(jq -er '.genesisStateRoot' "$GENESIS_PROFILE_OUT")"
EEZ_L2_EEZL2_CODE_HASH="$(jq -er '.runtimeCodeHash' "$GENESIS_PROFILE_OUT")"

# One explicit new-network profile. The finalized L1 reference fixes the initial
# seed and slot phase; it is not inferred from the later deployment receipt.
# EEZ_L1_REFERENCE=genesis pins L1 block zero instead. It is final by
# definition, so a fresh private devnet need not wait for its first finalized
# checkpoint. Genesis inherits the reference timestamp, so this is rejected on
# public networks whose block zero would leave years of L2 blocks to catch up.
REFERENCE_CHAIN_ID="$(cast chain-id --rpc-url "$EEZ_L1_RPC_URL")"
case "${EEZ_L1_REFERENCE-finalized}" in
    finalized) REFERENCE_BLOCK=finalized ;;
    genesis)
        case "$REFERENCE_CHAIN_ID" in
            1|10200)
                echo "deploy: EEZ_L1_REFERENCE=genesis is only for private devnets" >&2
                exit 1
                ;;
        esac
        REFERENCE_BLOCK=0
        ;;
    *)
        echo "deploy: EEZ_L1_REFERENCE must be 'finalized' or 'genesis'" >&2
        exit 1
        ;;
esac
case "$REFERENCE_CHAIN_ID" in
    1)
        : "${EEZ_L1_BLOCK_TIME_MS=12000}" "${EEZ_L2_BLOCK_TIME_MS=2000}"
        ;;
    10200)
        : "${EEZ_L1_BLOCK_TIME_MS=5000}" "${EEZ_L2_BLOCK_TIME_MS=1000}"
        ;;
    *)
        : "${EEZ_L1_BLOCK_TIME_MS:?private L1 genesis requires an explicit block cadence}"
        : "${EEZ_L2_BLOCK_TIME_MS:?private L2 genesis requires an explicit block cadence}"
        ;;
esac
GENESIS_TIMESTAMP_TMP="$(mktemp "${GENESIS_OUT}.tmp.XXXXXX")"
cast block "$REFERENCE_BLOCK" --rpc-url "$EEZ_L1_RPC_URL" --json | python3 -c '
import json, re, sys
base, output, chain, l1_ms, l2_ms = sys.argv[1:]
chain, l1_ms, l2_ms = int(chain), int(l1_ms), int(l2_ms)
if chain <= 0 or l1_ms <= 0 or l2_ms <= 0 or l2_ms % 1000 or l1_ms % l2_ms:
    raise SystemExit("deploy: invalid whole-second genesis cadence")
step, positions = l2_ms // 1000, l1_ms // l2_ms
if positions < 2 or positions > 0xffffffff or step > 0xffffffffffffffff:
    raise SystemExit("deploy: invalid Sync interval")
if (chain == 1 and (step, positions) != (2, 6)) or (chain == 10200 and (step, positions) != (1, 5)):
    raise SystemExit("deploy: cadence conflicts with the settlement network profile")
reference = json.load(sys.stdin)
if not isinstance(reference, dict):
    raise SystemExit("deploy: finalized reference is unavailable")
def integer(name):
    value = reference[name]
    value = int(value, 0) if isinstance(value, str) else value
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= 0xffffffffffffffff:
        raise SystemExit("deploy: invalid finalized reference " + name)
    return value
def hash32(name):
    value = reference[name]
    if not isinstance(value, str) or re.fullmatch(r"0x[0-9a-fA-F]{64}", value) is None:
        raise SystemExit("deploy: invalid finalized reference " + name)
    return value
number, timestamp = integer("number"), integer("timestamp")
hash_value, seed = hash32("hash"), hash32("mixHash")
if int(hash_value, 16) == 0:
    raise SystemExit("deploy: zero finalized reference hash")
with open(base) as source:
    genesis = json.load(source)
genesis["timestamp"], genesis["mixHash"] = hex(timestamp), seed
genesis["config"]["rollupX"] = {
    "blockIntervalSeconds": step, "syncIntervalBlocks": positions,
    "l1Reference": {"chainId": chain, "number": number, "hash": hash_value,
                    "timestamp": timestamp, "prevRandao": seed}}
with open(output, "w") as destination:
    json.dump(genesis, destination, indent=2)
    destination.write("\n")
' "$GENESIS_OUT" "$GENESIS_TIMESTAMP_TMP" "$REFERENCE_CHAIN_ID" "$EEZ_L1_BLOCK_TIME_MS" "$EEZ_L2_BLOCK_TIME_MS"
chmod --reference="$GENESIS_OUT" "$GENESIS_TIMESTAMP_TMP"
mv "$GENESIS_TIMESTAMP_TMP" "$GENESIS_OUT"
GENESIS_TIMESTAMP_TMP=""
compute_genesis_block_hash "$GENESIS_OUT" >/dev/null

# ── 1/4 DeployEEZ ────────────────────────────────────────────────────
echo "[1/4] DeployEEZ"
run_forge "DeployEEZ" forge script script/DeployEEZ.s.sol:DeployEEZ \
    --sig "run(address)" "$OWNER" $RPC $KEY --broadcast
EEZ_REGISTRY_ADDRESS="$(extract EEZ "$OUT")"
[[ -n "$EEZ_REGISTRY_ADDRESS" ]] || { echo "$OUT" >&2; echo "deploy: failed to capture EEZ address" >&2; exit 1; }
EEZ_REGISTRY_DEPLOY_BLOCK="$(cast block-number --rpc-url "$EEZ_L1_RPC_URL")"
echo "      EEZ        = $EEZ_REGISTRY_ADDRESS"
echo "      deployBlock= $EEZ_REGISTRY_DEPLOY_BLOCK"

# ── 2/4 DeployECDSAProofSystem ───────────────────────────────────────
echo "[2/4] DeployECDSAProofSystem(authorizedSigner=$AUTHORIZED_SIGNER)"
run_forge "DeployECDSAProofSystem" forge script script/DeployECDSAProofSystem.s.sol:DeployECDSAProofSystem \
    --sig "run(address)" "$AUTHORIZED_SIGNER" $RPC $KEY --broadcast
EEZ_ECDSA_PROOF_SYSTEM_ADDRESS="$(extract ECDSA_PS "$OUT")"
[[ -n "$EEZ_ECDSA_PROOF_SYSTEM_ADDRESS" ]] || { echo "$OUT" >&2; echo "deploy: failed to capture ECDSA_PS address" >&2; exit 1; }
echo "      ECDSA_PS   = $EEZ_ECDSA_PROOF_SYSTEM_ADDRESS"

# ── 3/4 DeployRollup ────────────────────────────────────────────────
echo "[3/4] DeployRollup"
run_forge "DeployRollup" forge script script/DeployRollup.s.sol:DeployRollup \
    --sig "run(address,address,address,address)" \
    "$EEZ_REGISTRY_ADDRESS" "$EEZ_ECDSA_PROOF_SYSTEM_ADDRESS" "$AUTHORIZED_SIGNER" "$OWNER" \
    $RPC $KEY --broadcast
EEZ_ROLLUP_MANAGER_ADDRESS="$(extract ROLLUP_CONTRACT "$OUT")"
[[ -n "$EEZ_ROLLUP_MANAGER_ADDRESS" ]] || { echo "$OUT" >&2; echo "deploy: failed to capture ROLLUP_CONTRACT address" >&2; exit 1; }
echo "      rollupMgr  = $EEZ_ROLLUP_MANAGER_ADDRESS"

# ── Finalize deployment-bound L2 genesis ────────────────────────────
# Confirm the registry is empty before registering the rendered rollup 1.
ROLLUP_COUNTER="$(cast call "$EEZ_REGISTRY_ADDRESS" "rollupCounter()(uint256)" \
    --rpc-url "$EEZ_L1_RPC_URL" | awk '{print $1}')"
[[ "$ROLLUP_COUNTER" == "0" ]] || {
    echo "deploy: fresh EEZ registry unexpectedly has rollupCounter=$ROLLUP_COUNTER" >&2
    exit 1
}
# The finalized-reference genesis was fully checked before any broadcast.
# Registration commits that same document, without deployment-time mutation.
FINAL_STATE_ROOT="$(compute_genesis_state_root "$GENESIS_OUT")"
if [[ "${FINAL_STATE_ROOT,,}" != "${EEZ_L2_GENESIS_STATE_ROOT,,}" ]]; then
    echo "deploy: finalized genesis state root differs from the generated profile" >&2
    echo "profile:   $EEZ_L2_GENESIS_STATE_ROOT" >&2
    echo "finalized: $FINAL_STATE_ROOT" >&2
    exit 1
fi
EEZ_INITIAL_BLOCK_HASH="$(compute_genesis_block_hash "$GENESIS_OUT")"
if [[ -n "$configured_initial_block_hash" \
    && "${configured_initial_block_hash,,}" != "${EEZ_INITIAL_BLOCK_HASH,,}" ]]
then
    echo "deploy: configured EEZ_INITIAL_BLOCK_HASH does not match the rendered L2 genesis" >&2
    echo "configured: $configured_initial_block_hash" >&2
    echo "rendered:   $EEZ_INITIAL_BLOCK_HASH" >&2
    exit 1
fi
echo "      blockHash  = $EEZ_INITIAL_BLOCK_HASH"
echo "      stateRoot  = $EEZ_L2_GENESIS_STATE_ROOT"
echo "      runtimeHash= $EEZ_L2_EEZL2_CODE_HASH"
echo "      genesis.ts = $(jq -r .timestamp "$GENESIS_OUT") (finalized L1 reference)"

# ── 4/4 RegisterRollup ──────────────────────────────────────────────
echo "[4/4] RegisterRollup(initialBlockHash=$EEZ_INITIAL_BLOCK_HASH)"
run_forge "RegisterRollup" forge script script/RegisterRollup.s.sol:RegisterRollup \
    --sig "run(address,address,bytes32)" \
    "$EEZ_REGISTRY_ADDRESS" "$EEZ_ROLLUP_MANAGER_ADDRESS" "$EEZ_INITIAL_BLOCK_HASH" \
    $RPC $KEY --broadcast
EEZ_ROLLUP_ID="$(extract_uint L2_ROLLUP_ID "$OUT")"
[[ -n "$EEZ_ROLLUP_ID" ]] || { echo "$OUT" >&2; echo "deploy: failed to capture L2_ROLLUP_ID" >&2; exit 1; }
[[ "$EEZ_ROLLUP_ID" == "$EXPECTED_ROLLUP_ID" ]] || {
    echo "deploy: registry assigned rollupId=$EEZ_ROLLUP_ID; generated EEZL2 expects $EXPECTED_ROLLUP_ID" >&2
    exit 1
}
echo "      rollupId   = $EEZ_ROLLUP_ID"

# ── Write deployments.env ───────────────────────────────────────────
cat > "$OUT_FILE" <<EOF
# Generated by scripts/deploy.sh — do not edit by hand. Gitignored.
# Sourced automatically at eez-node startup (alongside .env) via dotenvy.

EEZ_REGISTRY_ADDRESS=$EEZ_REGISTRY_ADDRESS
EEZ_REGISTRY_DEPLOY_BLOCK=$EEZ_REGISTRY_DEPLOY_BLOCK
EEZ_ECDSA_PROOF_SYSTEM_ADDRESS=$EEZ_ECDSA_PROOF_SYSTEM_ADDRESS
EEZ_PROOF_SYSTEM_KIND=real
EEZ_PROOF_SYSTEM=$EEZ_ECDSA_PROOF_SYSTEM_ADDRESS
EEZ_VKEY=$PROOF_SYSTEM_VKEY
EEZ_ATTESTER_ADDRESS=$AUTHORIZED_SIGNER
EEZ_ROLLUP_MANAGER_ADDRESS=$EEZ_ROLLUP_MANAGER_ADDRESS
EEZ_ROLLUP_ID=$EEZ_ROLLUP_ID
EEZ_INITIAL_BLOCK_HASH=$EEZ_INITIAL_BLOCK_HASH
EEZ_L2_GENESIS_STATE_ROOT=$EEZ_L2_GENESIS_STATE_ROOT
EEZ_L2_GENESIS_PATH=$GENESIS_OUT
EEZ_L2_GENESIS_PROFILE_PATH=$GENESIS_PROFILE_OUT

# EEZL2 predeploy baked into genesis.json.
EEZL2_ADDRESS=0x4200000000000000000000000000000000000007
# Reserved native system identity; no L2 system key is used.
EEZ_L2_SYSTEM_ADDRESS=$EEZ_L2_SYSTEM_ADDRESS
EEZ_L2_RECOVERY_ADDRESS=$EEZ_L2_RECOVERY_ADDRESS
EEZ_L2_EEZL2_CODE_HASH=$EEZ_L2_EEZL2_CODE_HASH
EOF

echo
echo "deploy: wrote $OUT_FILE"

# ── Optional: Blockscout verification ────────────────────────────────
# Delegated to scripts/verify-blockscout.sh (best-effort; no-op unless
# EEZ_BLOCKSCOUT_URL is set). START_MARKER (created at script start)
# scopes it to THIS run's fresh CREATE entries so a re-run doesn't
# re-submit stale addresses. Never fails the deploy.
echo
EEZ_BLOCKSCOUT_URL="${EEZ_BLOCKSCOUT_URL:-}" EEZ_CONTRACTS_DIR="$CONTRACTS" \
EEZ_L1_RPC_URL="$EEZ_L1_RPC_URL" \
    "$REPO/scripts/verify-blockscout.sh" "$START_MARKER" || true

echo
echo "deploy: ready. \`make run-node\` will pick these up automatically."
