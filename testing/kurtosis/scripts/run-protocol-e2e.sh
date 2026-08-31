#!/usr/bin/env bash
# Run eez-core-protocol scenarios against the CI enclave and produce a strict,
# machine-readable evidence bundle. The default discovers and runs every
# upstream E2E script at the pinned submodule commit.
set -euo pipefail

KURTOSIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$KURTOSIS_DIR/../.." && pwd)"
PROTOCOL="$REPO/eez-core-protocol"
ENCLAVE="${KURTOSIS_ENCLAVE:-eez-ci}"
RESULT_DIR="${EEZ_CI_RESULT_DIR:-$REPO/artifacts/kurtosis-e2e}/protocol-e2e"

UNSUPPORTED_TARGETS=()

for tool in bash bc cargo cast find forge git jq kurtosis sha256sum; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

expected_protocol="$(git -C "$REPO" ls-files -s eez-core-protocol | awk '{print $2}')"
actual_protocol="$(git -C "$PROTOCOL" rev-parse HEAD)"
if [[ -z "$expected_protocol" || "$actual_protocol" != "$expected_protocol" ]]; then
    echo "eez-core-protocol is not at the commit pinned by this checkout" >&2
    echo "expected: ${expected_protocol:-missing}; found: $actual_protocol" >&2
    echo "Run: git submodule update --init --recursive eez-core-protocol" >&2
    exit 1
fi

causal_target_dir="${CARGO_TARGET_DIR:-$REPO/target}"
[[ "$causal_target_dir" == /* ]] || causal_target_dir="$REPO/$causal_target_dir"
env CARGO_TARGET_DIR="$causal_target_dir" \
    cargo build --manifest-path "$REPO/Cargo.toml" -p eez-protocol --example e2e-causal-verify
CAUSAL_VERIFIER="$causal_target_dir/debug/examples/e2e-causal-verify"
[[ -x "$CAUSAL_VERIFIER" ]] || {
    echo "causal verifier was not built at $CAUSAL_VERIFIER" >&2
    exit 1
}

source "$KURTOSIS_DIR/ports.sh" >/dev/null
L1_RPC="${L1_RPC:-$EEZ_DEVNET_L1_RPC}"
L2_RPC="${L2_RPC:-$EEZ_DEVNET_L2_RPC}"
L1_FRONT="${L1_FRONT:-$EEZ_DEVNET_L1_FRONT}"
L2_FRONT="${L2_FRONT:-$EEZ_DEVNET_L2_FRONT}"

mkdir -p "$RESULT_DIR"
deploy_dir="$(mktemp -d /tmp/eez-protocol-deployments.XXXXXX)"
trap 'rm -rf "$deploy_dir"' EXIT

kurtosis files download "$ENCLAVE" eez-deployments "$deploy_dir" >/dev/null
set -a
source "$deploy_dir/deployments.env"
set +a

ROLLUPS="${ROLLUPS:-$EEZ_REGISTRY_ADDRESS}"
MANAGER_L2="${MANAGER_L2:-${EEZL2_ADDRESS:-0x4200000000000000000000000000000000000007}}"
ROLLUP_ID="${EEZ_ROLLUP_ID:-1}"

# Hardhat account #2, shared with the earlier cross-chain wave.
PK="${PK:-0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a}"
SENDER="$(cast wallet address --private-key "$PK")"

echo "==> protocol E2E endpoints"
echo "    protocol:     $actual_protocol"
echo "    L1 RPC/front: $L1_RPC / $L1_FRONT"
echo "    L2 RPC/front: $L2_RPC / $L2_FRONT"
echo "    registry:     $ROLLUPS"

wait_for_protocol_ready() {
    local timeout="${EEZ_PROTOCOL_READY_TIMEOUT:-300}"
    local deadline=$((SECONDS + timeout))
    local safe_json safe_number_hex safe_number safe_hash rollup_data registry_root

    echo "==> waiting for the first L2 safe block hash to settle on L1"
    while (( SECONDS < deadline )); do
        if safe_json="$(cast block safe --json --rpc-url "$L2_RPC" 2>/dev/null)" &&
            safe_number_hex="$(jq -er '.number' <<<"$safe_json" 2>/dev/null)" &&
            safe_number="$(cast to-dec "$safe_number_hex" 2>/dev/null)" &&
            safe_hash="$(jq -er '.hash' <<<"$safe_json" 2>/dev/null)" &&
            rollup_data="$(cast call "$ROLLUPS" \
                'rollups(uint64)(address,bytes32,uint256)' "$ROLLUP_ID" \
                --rpc-url "$L1_RPC" 2>/dev/null)"; then
            registry_root="$(sed -n '2p' <<<"$rollup_data" | tr -d '[:space:]')"
            if [[ "$safe_number" =~ ^[0-9]+$ ]] &&
                (( 10#$safe_number > 0 )) &&
                [[ "${registry_root,,}" == "${safe_hash,,}" ]]; then
                echo "    ready: safe L2 block $safe_number, hash $safe_hash"
                return 0
            fi
        fi
        sleep 3
    done

    echo "protocol network did not become settlement-ready within ${timeout}s" >&2
    echo "last safe block/hash: ${safe_number:-unavailable} / ${safe_hash:-unavailable}" >&2
    echo "last registry root: ${registry_root:-unavailable}" >&2
    return 1
}

wait_for_protocol_ready

(
    cd "$PROTOCOL"
    bash script/e2e/run/prepare-network.sh \
        --l1-rpc "$L1_RPC" \
        --l1-front "$L1_FRONT" \
        --l2-rpc "$L2_RPC" \
        --pk "$PK" \
        --rollups "$ROLLUPS"
)

# Independently enforce the balance required by the scenario deployments.
min_l2_balance=10000000000000000
l2_balance="$(cast balance "$SENDER" --rpc-url "$L2_RPC")"
if (( $(echo "$l2_balance < $min_l2_balance" | bc) )); then
    echo "L2 balance for $SENDER is $l2_balance wei, below the $min_l2_balance wei minimum" >&2
    exit 1
fi

env_file="$deploy_dir/chain.env"
cat >"$env_file" <<ENV
L1_RPC=$L1_RPC
L1_FRONT=$L1_FRONT
L2_RPC=$L2_RPC
L2_FRONT=$L2_FRONT
ROLLUPS=$ROLLUPS
MANAGER_L2=$MANAGER_L2
PK=$PK
ENV

# Discover the upstream suite from the pinned submodule. Scenario basenames are
# the network runner's result keys, so require them to be globally unique.
mapfile -t all_scenario_paths < <(
    find "$PROTOCOL/script/e2e" -mindepth 3 -name 'E2E*.s.sol' -not -path '*/shared/*' -print \
        | sed "s#^$PROTOCOL/##" \
        | sort
)
(( ${#all_scenario_paths[@]} > 0 )) || { echo "no upstream protocol E2Es discovered" >&2; exit 1; }

declare -A all_path_by_name=()
all_scenario_names=()
for path in "${all_scenario_paths[@]}"; do
    name="$(basename "$(dirname "$path")")"
    if [[ -n "${all_path_by_name[$name]+x}" ]]; then
        echo "duplicate upstream scenario name '$name': ${all_path_by_name[$name]} and $path" >&2
        exit 1
    fi
    all_path_by_name["$name"]="$path"
    all_scenario_names+=("$name")
done

if [[ -n "${EEZ_PROTOCOL_SCENARIOS:-}" ]]; then
    read -r -a targets <<<"$EEZ_PROTOCOL_SCENARIOS"
else
    targets=(all)
fi

selected_paths=()
for target in "${targets[@]}"; do
    matches=()
    if [[ "$target" == "all" ]]; then
        matches=("${all_scenario_paths[@]}")
    elif [[ -d "$PROTOCOL/script/e2e/$target" ]]; then
        mapfile -t matches < <(
            find "$PROTOCOL/script/e2e/$target" -name 'E2E*.s.sol' -not -path '*/shared/*' -print \
                | sed "s#^$PROTOCOL/##" \
                | sort
        )
    else
        for path in "${all_scenario_paths[@]}"; do
            [[ "$(basename "$(dirname "$path")")" == "$target" ]] && matches+=("$path")
        done
    fi
    (( ${#matches[@]} > 0 )) || { echo "no upstream scenario matches '$target'" >&2; exit 1; }
    selected_paths+=("${matches[@]}")
done

declare -A selected_path_by_name=()
selected_names=()
for path in "${selected_paths[@]}"; do
    name="$(basename "$(dirname "$path")")"
    if [[ -n "${selected_path_by_name[$name]+x}" ]]; then
        echo "scenario '$name' was selected more than once; each scenario must run exactly once" >&2
        exit 1
    fi
    selected_path_by_name["$name"]="$path"
    selected_names+=("$name")
done

suite_complete=false
(( ${#selected_names[@]} == ${#all_scenario_names[@]} )) && suite_complete=true
echo "==> discovered ${#all_scenario_names[@]} upstream E2Es; selected ${#selected_names[@]} (complete=$suite_complete)"

json_array() {
    jq -cn --args '$ARGS.positional' "$@"
}

manifest_json="$({
    for name in "${all_scenario_names[@]}"; do
        path="${all_path_by_name[$name]}"
        has_network_driver=false
        grep -qE 'contract ExecuteNetwork(L2)? ' "$PROTOCOL/$path" && has_network_driver=true
        direction=L1_to_L2
        grep -qE 'contract ExecuteNetworkL2 ' "$PROTOCOL/$path" && direction=L2_to_L1
        jq -cn --arg name "$name" --arg path "$path" --arg direction "$direction" \
            --argjson has_network_driver "$has_network_driver" \
            '{name: $name, path: $path, direction: $direction,
              has_network_driver: $has_network_driver}'
    done
} | jq -s .)"

evidence_dir="$RESULT_DIR/evidence"
mkdir -p "$evidence_dir"
find "$evidence_dir" -maxdepth 1 -type f \( -name '*.json' -o -name '*.log' \) -delete
rm -f "$RESULT_DIR/summary.json" "$RESULT_DIR/manifest.json"
jq -n --arg protocol_commit "$actual_protocol" --argjson scenarios "$manifest_json" \
    '{protocol_commit: $protocol_commit, scenarios: $scenarios}' >"$RESULT_DIR/manifest.json"

run_log="$RESULT_DIR/network-sequential.log"
rm -rf "$PROTOCOL/tmp/e2e-network"
status=0
(
    cd "$PROTOCOL"
    DEVNET_ENV="$env_file" \
        bash script/e2e/run/network-sequential.sh "${targets[@]}"
) >"$run_log" 2>&1 || status=$?
cat "$run_log"
if [[ -d "$PROTOCOL/tmp/e2e-network" ]]; then
    cp -R "$PROTOCOL/tmp/e2e-network/." "$RESULT_DIR/"
fi

# Parse the raw upstream result stream once. Later compatibility verification
# updates an in-memory verdict; it never appends a second RESULT line.
declare -A result_count=()
declare -A raw_verdict=()
declare -A final_verdict=()
declare -A verification_mode=()
unexpected_results=()
while read -r name verdict; do
    result_count["$name"]=$(( ${result_count[$name]:-0} + 1 ))
    raw_verdict["$name"]="$verdict"
    if [[ -z "${selected_path_by_name[$name]+x}" ]]; then
        unexpected_results+=("$name")
    fi
done < <(sed -n 's/^RESULT \([^:]*\): \([A-Z]*\).*/\1 \2/p' "$run_log")

accounting_ok=true
raw_failure_count=0
for name in "${selected_names[@]}"; do
    [[ "${raw_verdict[$name]:-}" == FAIL ]] && raw_failure_count=$((raw_failure_count + 1))
done
if (( status == 0 && raw_failure_count > 0 )); then
    echo "upstream runner exited successfully while reporting $raw_failure_count failed scenario(s)" >&2
    accounting_ok=false
elif (( status != 0 && raw_failure_count == 0 )); then
    echo "upstream runner exited $status without reporting a failed scenario" >&2
    accounting_ok=false
fi
missing_results=()
duplicate_results=()
for name in "${selected_names[@]}"; do
    count="${result_count[$name]:-0}"
    if (( count == 0 )); then
        missing_results+=("$name")
        final_verdict["$name"]=FAIL
        verification_mode["$name"]=missing-result
        accounting_ok=false
    elif (( count > 1 )); then
        duplicate_results+=("$name")
        final_verdict["$name"]=FAIL
        verification_mode["$name"]=duplicate-result
        accounting_ok=false
    else
        final_verdict["$name"]="${raw_verdict[$name]}"
        case "${raw_verdict[$name]}" in
            PASS) verification_mode["$name"]=upstream-direct ;;
            FAIL) verification_mode["$name"]=upstream-failed ;;
            SKIP) verification_mode["$name"]=upstream-skipped ;;
            *)
                final_verdict["$name"]=FAIL
                verification_mode["$name"]=invalid-result
                accounting_ok=false
                ;;
        esac
    fi
done
if (( ${#unexpected_results[@]} > 0 )); then
    accounting_ok=false
fi

compat_overrides=0
accept_static_counter_l2_static_pool() {
    local name=staticCounterL2
    local log="$RESULT_DIR/$name.log"
    local compat_log="$RESULT_DIR/$name.compat.log"
    local evidence="$evidence_dir/$name.json"

    [[ "${result_count[$name]:-0}" == 1 && "${raw_verdict[$name]:-}" == FAIL ]] || return 1
    if ! L1_RPC="$L1_RPC" L2_RPC="$L2_RPC" ROLLUPS="$ROLLUPS" MANAGER_L2="$MANAGER_L2" \
        ROLLUP_ID="$ROLLUP_ID" PROTOCOL_COMMIT="$actual_protocol" CAUSAL_VERIFIER="$CAUSAL_VERIFIER" \
        bash "$KURTOSIS_DIR/scripts/verify-static-counter-l2.sh" "$log" "$evidence" \
        >"$compat_log" 2>&1; then
        cat "$compat_log" >&2
        return 1
    fi
    cat "$compat_log" | tee -a "$run_log"
    echo "COMPAT $name: PASS — strict static-only pool evidence written to $evidence" | tee -a "$run_log"
    final_verdict["$name"]=PASS
    verification_mode["$name"]=external-static-pool
    compat_overrides=$((compat_overrides + 1))
}

accept_top_level_static_counter() {
    local name=topLevelStaticCounter
    local log="$RESULT_DIR/$name.log"
    local compat_log="$RESULT_DIR/$name.compat.log"
    local evidence="$evidence_dir/$name.json"

    [[ "${result_count[$name]:-0}" == 1 && "${raw_verdict[$name]:-}" == FAIL ]] || return 1
    if ! L1_RPC="$L1_RPC" L1_FRONT="$L1_FRONT" L2_RPC="$L2_RPC" \
        ROLLUPS="$ROLLUPS" PK="$PK" PROTOCOL_COMMIT="$actual_protocol" \
        bash "$KURTOSIS_DIR/scripts/verify-top-level-static-counter.sh" "$log" "$evidence" \
        >"$compat_log" 2>&1; then
        cat "$compat_log" >&2
        return 1
    fi
    cat "$compat_log" | tee -a "$run_log"
    echo "COMPAT $name: PASS — strict view-only static evidence written to $evidence" | tee -a "$run_log"
    final_verdict["$name"]=PASS
    verification_mode["$name"]=external-view-only-static
    compat_overrides=$((compat_overrides + 1))
}

if [[ -n "${selected_path_by_name[staticCounterL2]+x}" ]]; then
    accept_static_counter_l2_static_pool || true
fi
if [[ -n "${selected_path_by_name[topLevelStaticCounter]+x}" ]]; then
    accept_top_level_static_counter || true
fi

write_non_pass_evidence() {
    local name="$1" result="$2" mode="$3" message="$4"
    local path="${selected_path_by_name[$name]}" log="$RESULT_DIR/$name.log"
    local log_sha256=""
    [[ -f "$log" ]] && log_sha256="$(sha256sum "$log" | awk '{print $1}')"
    jq -n --argjson schema_version 1 --arg scenario "$name" --arg scenario_path "$path" \
        --arg protocol_commit "$actual_protocol" --arg result "$result" \
        --arg verification_mode "$mode" --arg error "$message" \
        --arg log "$log" --arg log_sha256 "$log_sha256" \
        '{schema_version: $schema_version, scenario: $scenario, scenario_path: $scenario_path,
          protocol_commit: $protocol_commit, result: $result, verification_mode: $verification_mode,
          error: $error, source_log: {path: $log, sha256: $log_sha256}}' \
        >"$evidence_dir/$name.json"
}

# A direct upstream PASS is necessary but not sufficient: independently query
# all user, settlement, and table-load receipts and bind them to the source log.
for name in "${selected_names[@]}"; do
    if [[ "${final_verdict[$name]}" == PASS && "${verification_mode[$name]}" == upstream-direct ]]; then
        evidence_log="$evidence_dir/$name.collect.log"
        if ! L1_RPC="$L1_RPC" L2_RPC="$L2_RPC" ROLLUPS="$ROLLUPS" MANAGER_L2="$MANAGER_L2" \
            ROLLUP_ID="$ROLLUP_ID" PROTOCOL_COMMIT="$actual_protocol" CAUSAL_VERIFIER="$CAUSAL_VERIFIER" \
            bash "$KURTOSIS_DIR/scripts/collect-protocol-e2e-evidence.sh" \
                "$name" "$PROTOCOL/${selected_path_by_name[$name]}" "$RESULT_DIR/$name.log" \
                "$evidence_dir/$name.json" >"$evidence_log" 2>&1; then
            cat "$evidence_log" >&2
            final_verdict["$name"]=FAIL
            verification_mode["$name"]=evidence-invalid
            write_non_pass_evidence "$name" FAIL evidence-invalid \
                "upstream passed, but independent receipt/table evidence collection failed"
        else
            rm -f "$evidence_log"
        fi
    elif [[ "${final_verdict[$name]}" != PASS ]]; then
        write_non_pass_evidence "$name" "${final_verdict[$name]}" "${verification_mode[$name]}" \
            "scenario did not produce a complete passing evidence bundle"
    fi
done

passed=()
failed=()
skipped=()
for name in "${selected_names[@]}"; do
    case "${final_verdict[$name]}" in
        PASS) passed+=("$name") ;;
        FAIL) failed+=("$name") ;;
        SKIP) skipped+=("$name") ;;
    esac
done

evidence_index="$evidence_dir/index.json"
{
    for name in "${selected_names[@]}"; do
        jq -c . "$evidence_dir/$name.json"
    done
} | jq -s . >"$evidence_index"
all_evidence_complete=false
if [[ "$(jq '[.[] | select(.result == "PASS")] | length' "$evidence_index")" \
    == "${#selected_names[@]}" ]]; then
    all_evidence_complete=true
fi
causally_paired="$(jq '[.[] | select(
    .result == "PASS" and .causal_graph.verified == true and
    ((.claims.no_destination_delivery_required // false) == false)
) | .scenario]' "$evidence_index")"
causal_no_destination="$(jq '[.[] | select(
    .result == "PASS" and .causal_graph.verified == true and
    (.claims.no_destination_delivery_required == true)
) | .scenario]' "$evidence_index")"
causal_missing="$(jq '[.[] | select(
    .result != "PASS" or .causal_graph.verified != true
) | .scenario]' "$evidence_index")"
causal_coverage_complete=false
[[ "$(jq 'length' <<<"$causal_missing")" == 0 ]] && causal_coverage_complete=true

jq -n \
    --argjson schema_version 3 \
    --arg protocol_commit "$actual_protocol" \
    --argjson discovered_count "${#all_scenario_names[@]}" \
    --argjson selected_count "${#selected_names[@]}" \
    --argjson suite_complete "$suite_complete" \
    --argjson accounting_ok "$accounting_ok" \
    --argjson all_evidence_complete "$all_evidence_complete" \
    --argjson causal_coverage_complete "$causal_coverage_complete" \
    --argjson causally_paired "$causally_paired" \
    --argjson causal_no_destination "$causal_no_destination" \
    --argjson causal_missing "$causal_missing" \
    --argjson upstream_exit_status "$status" \
    --argjson compat_overrides "$compat_overrides" \
    --argjson passed "$(json_array ${passed[@]+"${passed[@]}"})" \
    --argjson failed "$(json_array ${failed[@]+"${failed[@]}"})" \
    --argjson skipped "$(json_array ${skipped[@]+"${skipped[@]}"})" \
    --argjson targets "$(json_array ${targets[@]+"${targets[@]}"})" \
    --argjson missing_results "$(json_array ${missing_results[@]+"${missing_results[@]}"})" \
    --argjson duplicate_results "$(json_array ${duplicate_results[@]+"${duplicate_results[@]}"})" \
    --argjson unexpected_results "$(json_array ${unexpected_results[@]+"${unexpected_results[@]}"})" \
    --argjson unsupported "$(json_array ${UNSUPPORTED_TARGETS[@]+"${UNSUPPORTED_TARGETS[@]}"})" \
    --slurpfile scenarios "$evidence_index" \
    '{schema_version: $schema_version, protocol_commit: $protocol_commit,
      targets: $targets,
      suite: {discovered: $discovered_count, selected: $selected_count, complete: $suite_complete},
      accounting: {valid: $accounting_ok, missing: $missing_results,
                   duplicates: $duplicate_results, unexpected: $unexpected_results},
      upstream_exit_status: $upstream_exit_status, compatibility_overrides: $compat_overrides,
      evidence_complete: $all_evidence_complete,
      causal_coverage: {complete: $causal_coverage_complete, paired: $causally_paired,
                        no_destination_required: $causal_no_destination,
                        missing: $causal_missing},
      passed: $passed, failed: $failed, skipped: $skipped, unsupported: $unsupported,
      scenarios: $scenarios[0]}' >"$RESULT_DIR/summary.json"

echo "==> protocol E2E result: ${#passed[@]} passed, ${#failed[@]} failed, ${#skipped[@]} skipped"
echo "    suite accounting: complete=$suite_complete valid=$accounting_ok evidence=$all_evidence_complete"
echo "    causal coverage: complete=$causal_coverage_complete paired=$(jq 'length' <<<"$causally_paired") no-destination=$(jq 'length' <<<"$causal_no_destination")"
echo "    summary: $RESULT_DIR/summary.json"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
        echo "### Protocol network E2E"
        echo
        echo "- Upstream scenarios: ${#all_scenario_names[@]} discovered / ${#selected_names[@]} selected"
        echo "- Complete suite: $suite_complete"
        echo "- Result accounting valid: $accounting_ok"
        echo "- Complete evidence bundles: $all_evidence_complete"
        echo "- Complete causal coverage: $causal_coverage_complete"
        echo "- Causally paired scenarios: $(jq 'length' <<<"$causally_paired")"
        echo "- View-only/no-destination scenarios: $(jq 'length' <<<"$causal_no_destination")"
        echo "- Passed: ${#passed[@]}"
        echo "- Failed: ${#failed[@]}"
        echo "- Skipped: ${#skipped[@]}"
    } >>"$GITHUB_STEP_SUMMARY"
fi

[[ "$accounting_ok" == true ]]
[[ "$all_evidence_complete" == true ]]
[[ "$causal_coverage_complete" == true ]]
(( ${#failed[@]} == 0 && ${#skipped[@]} == 0 ))
