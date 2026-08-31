#!/usr/bin/env bash
# Prove that the configured held-transaction cap spills mixed L1→L2 and L2→L1
# traffic across Sync blocks without loss, duplication, nonce gaps, or eviction.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KURTOSIS_DIR="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$KURTOSIS_DIR/../.." && pwd)"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-crosschain-limit}"
ARGS_FILE="${KURTOSIS_ARGS_FILE:-$KURTOSIS_DIR/crosschain-limit-args.yaml}"
CAP="${EEZ_EXPECTED_MAX_USER_TXS_PER_BUNDLE:-3}"
WAVES="${EEZ_LIMIT_WAVE_COUNT:-3}"
RESULT_ROOT="${EEZ_LIMIT_RESULT_DIR:-$REPO/artifacts/kurtosis-crosschain-limit}"
ANALYZE_RUN_DIR="${EEZ_LIMIT_ANALYZE_RUN_DIR:-}"

for tool in awk jq kurtosis sed tee; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done
[[ "$CAP" =~ ^[1-9][0-9]*$ ]] || { echo "CAP must be a positive integer" >&2; exit 1; }
[[ "$WAVES" =~ ^[1-9][0-9]*$ ]] || { echo "WAVES must be a positive integer" >&2; exit 1; }
[[ -f "$ARGS_FILE" ]] || { echo "Kurtosis args not found: $ARGS_FILE" >&2; exit 1; }

configured_cap="$(grep -E '^[[:space:]]*max_user_txs_per_bundle:' "$ARGS_FILE" | head -1 \
    | sed -E 's/^[^:]*:[[:space:]]*//; s/[[:space:]]*#.*$//; s/^"//; s/"$//')"
[[ "$configured_cap" == "$CAP" ]] || {
    echo "expected cap $CAP but $ARGS_FILE configures ${configured_cap:-nothing}" >&2
    exit 1
}

if [[ -n "$ANALYZE_RUN_DIR" ]]; then
    RUN_DIR="$ANALYZE_RUN_DIR"
else
    run_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    RUN_DIR="$RESULT_ROOT/$run_stamp"
fi
mkdir -p "$RUN_DIR"
NODE_LOG="$RUN_DIR/node-after.log"
TRAFFIC_LOG="$RUN_DIR/node-traffic-window.log"
WAVE_LOG="$RUN_DIR/cross-chain-wave.log"
RAW_NODE_LOG="$RUN_DIR/node-after.raw.log"
log_follower_pid=""

stop_log_follower() {
    if [[ -n "$log_follower_pid" ]]; then
        kill "$log_follower_pid" 2>/dev/null || true
        wait "$log_follower_pid" 2>/dev/null || true
        log_follower_pid=""
    fi
}
trap stop_log_follower EXIT

if [[ -n "$ANALYZE_RUN_DIR" ]]; then
    [[ -f "$NODE_LOG" && -f "$WAVE_LOG" ]] || {
        echo "replay evidence is incomplete in $RUN_DIR" >&2
        exit 1
    }
    wave_status="$(jq -r '.traffic.semantic_wave_exit_code // 1' "$RUN_DIR/result.json" 2>/dev/null || echo 1)"
    echo "==> re-analyzing preserved cross-chain cap evidence in $RUN_DIR"
else
    # Follow the service during the full test. Snapshot-style `service logs`
    # keeps only a recent window, which can discard the first (and deepest)
    # drain before a deliberately backlogged test finishes.
    timeout "${EEZ_LIMIT_LOG_FOLLOW_TIMEOUT_SECS:-900}s" \
        kurtosis service logs -f "$ENCLAVE" eez-node >"$RAW_NODE_LOG" 2>&1 &
    log_follower_pid=$!
    sleep 2

    echo "==> submitting $((WAVES * 8)) mixed cross-chain transactions against cap=$CAP"
    set +e
    env \
        KURTOSIS_ENCLAVE="$ENCLAVE" \
        KURTOSIS_ARGS_FILE="$ARGS_FILE" \
        EEZ_WAVE_MODE=mixed \
        EEZ_WAVE_COUNT="$WAVES" \
        EEZ_WAIT_BETWEEN_WAVES=0 \
        EEZ_WAVE_GAP_SECS=0 \
        EEZ_INCLUDE_REVERTS=0 \
        EEZ_NODE_LOG="$RUN_DIR/wave-node.log" \
        EEZ_PROOF_SIGNER_LOG="$RUN_DIR/wave-proof-signer.log" \
        bash "$HERE/cross-chain-wave.sh" 2>&1 | tee "$WAVE_LOG"
    wave_status="${PIPESTATUS[0]}"
    set -e

    stop_log_follower
    sed 's/\x1b\[[0-9;]*m//g' "$RAW_NODE_LOG" >"$NODE_LOG"
fi
# Exclude builder warm-up retries and empty drains. A fresh isolated enclave
# has no positive held-pool drain before this test's submitted traffic.
awk '/drained held pool at Sync slot/ && /drained_count=[1-9][0-9]*/ { traffic = 1 } traffic' \
    "$NODE_LOG" >"$TRAFFIC_LOG"

read -r drain_events positive_drains full_drains max_pool max_drained cap_violations total_drained < <(
    awk -v cap="$CAP" '
        /drained held pool at Sync slot/ {
            pool = $0; sub(/^.*pool_len_before=/, "", pool); sub(/[^0-9].*$/, "", pool)
            drained = $0; sub(/^.*drained_count=/, "", drained); sub(/[^0-9].*$/, "", drained)
            if (pool == "" || drained == "") next
            pool += 0
            drained += 0
            events++
            if (drained > 0) positive++
            if (drained == cap) full++
            if (pool > max_pool) max_pool = pool
            if (drained > max_drained) max_drained = drained
            if (drained > cap) violations++
            total += drained
        }
        END { print events+0, positive+0, full+0, max_pool+0, max_drained+0, violations+0, total+0 }
    ' "$TRAFFIC_LOG"
)

evictions="$(grep -Ec 'event_name="eez\.composer\.[^"]*(evicted|eviction)"|; evicting| evicting' "$TRAFFIC_LOG" || true)"
compose_failures="$(grep -Ec 'event_name="eez\.composer\.cc_compose\.failed"|cross-chain compose failed' "$TRAFFIC_LOG" || true)"
bundle_drops="$(grep -Fc 'bundle dropped' "$TRAFFIC_LOG" || true)"
overflow_observed=false
(( max_pool > CAP )) && overflow_observed=true

assertions_ok=1
(( wave_status == 0 )) || { echo "    ✗ mixed semantic wave failed"; assertions_ok=0; }
(( drain_events > 0 )) || { echo "    ✗ no Sync-slot drain telemetry found"; assertions_ok=0; }
(( positive_drains >= 2 )) || { echo "    ✗ traffic did not spill across multiple positive drains"; assertions_ok=0; }
(( full_drains > 0 )) || { echo "    ✗ no drain exercised the configured cap"; assertions_ok=0; }
(( max_pool > CAP )) || { echo "    ✗ no pool overflow was observed (max pool=$max_pool)"; assertions_ok=0; }
(( cap_violations == 0 )) || { echo "    ✗ observed $cap_violations drain(s) above cap=$CAP"; assertions_ok=0; }
(( evictions == 0 )) || { echo "    ✗ observed $evictions held-transaction eviction event(s)"; assertions_ok=0; }
(( compose_failures == 0 )) || { echo "    ✗ observed $compose_failures composition failure event(s)"; assertions_ok=0; }

result=fail
(( assertions_ok )) && result=pass

jq -n \
    --arg result "$result" \
    --arg enclave "$ENCLAVE" \
    --arg root_commit "$(git -C "$REPO" rev-parse HEAD)" \
    --arg protocol_commit "$(git -C "$REPO/eez-core-protocol" rev-parse HEAD)" \
    --argjson configured_cap "$CAP" \
    --argjson submitted_cross_chain_txs "$((WAVES * 8))" \
    --argjson semantic_wave_exit_code "$wave_status" \
    --argjson drain_events "$drain_events" \
    --argjson positive_drains "$positive_drains" \
    --argjson full_drains "$full_drains" \
    --argjson maximum_pool_depth "$max_pool" \
    --argjson maximum_drain "$max_drained" \
    --argjson cap_violations "$cap_violations" \
    --argjson total_drain_attempts "$total_drained" \
    --argjson evictions "$evictions" \
    --argjson composition_failures "$compose_failures" \
    --argjson bundle_drop_retries "$bundle_drops" \
    --argjson overflow_observed "$overflow_observed" \
    '{
        result: $result,
        enclave: $enclave,
        commits: {root: $root_commit, protocol: $protocol_commit},
        configured_cap: $configured_cap,
        traffic: {
            mode: "mixed",
            directions: ["L1-to-L2", "L2-to-L1"],
            submitted_cross_chain_txs: $submitted_cross_chain_txs,
            semantic_wave_exit_code: $semantic_wave_exit_code
        },
        drain_evidence: {
            events: $drain_events,
            positive_events: $positive_drains,
            full_cap_events: $full_drains,
            maximum_pool_depth: $maximum_pool_depth,
            maximum_drain: $maximum_drain,
            cap_violations: $cap_violations,
            total_drain_attempts: $total_drain_attempts,
            overflow_observed: $overflow_observed
        },
        health: {
            evictions: $evictions,
            composition_failures: $composition_failures,
            bundle_drop_retries: $bundle_drop_retries
        }
    }' >"$RUN_DIR/result.json"

echo
echo "==> cross-chain cap evidence"
echo "    result:              $result"
echo "    configured cap:      $CAP"
echo "    submitted txs:       $((WAVES * 8)) (mixed directions)"
echo "    maximum pool depth:  $max_pool"
echo "    maximum drain:       $max_drained"
echo "    positive drains:     $positive_drains"
echo "    cap violations:      $cap_violations"
echo "    evictions:           $evictions"
echo "    compose failures:    $compose_failures"
echo "    evidence:            $RUN_DIR/result.json"

(( assertions_ok )) || exit 1
