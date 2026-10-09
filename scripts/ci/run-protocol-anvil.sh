#!/usr/bin/env bash
# Run every pinned upstream Anvil scenario once, with bounded lifetime.
set -euo pipefail
cd "${1:-$(dirname "${BASH_SOURCE[0]}")/../../eez-core-protocol}"
scenario_timeout="${EEZ_ANVIL_SCENARIO_TIMEOUT:-300}"
suite_timeout="${EEZ_ANVIL_SUITE_TIMEOUT:-2400}"
for value in "$scenario_timeout" "$suite_timeout"; do
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || { echo "Timeout must be a positive integer" >&2; exit 1; }
done
mapfile -t scenarios < <(find script/e2e/scenarios -mindepth 3 -name 'E2E*.s.sol' -not -path '*/shared/*' | sort)
(( ${#scenarios[@]} )) || { echo "No protocol scenarios found" >&2; exit 1; }
declare -A names=()
for scenario in "${scenarios[@]}"; do
    name="$(basename "$(dirname "$scenario")")"
    [[ -z "${names[$name]+x}" ]] || { echo "Duplicate scenario name: $name" >&2; exit 1; }
    names[$name]=1
done
mkdir -p tmp
logs="$(mktemp -d tmp/e2e-ci-XXXXXXXX)"
export ETH_TIMEOUT="${ETH_TIMEOUT:-120}" ETH_RPC_TIMEOUT="${ETH_RPC_TIMEOUT:-30}"
deadline=$((SECONDS + suite_timeout))
passed=0
child=""
stop_child() {
    # GNU timeout creates a process group. Stop only this scenario and its
    # descendants, including stray children after success or cancellation.
    if [[ -n "$child" ]]; then
        kill -KILL -- "-$child" 2>/dev/null || true
        wait "$child" 2>/dev/null || true
        child=""
    fi
}
trap stop_child EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "Running ${#scenarios[@]} scenarios serially; logs: $logs"
for index in "${!scenarios[@]}"; do
    scenario="${scenarios[$index]}"
    name="$(basename "$(dirname "$scenario")")"
    remaining=$((deadline - SECONDS))
    if (( remaining <= 0 )); then
        printf '%s\tNOT_RUN\t124\t0\n' "$name" >> "$logs/results.tsv"
        continue
    fi
    limit=$scenario_timeout
    (( remaining >= limit )) || limit=$remaining
    started=$SECONDS
    status=0
    echo "[$name] starting"
    L1_PORT=$(( ${BASE_PORT:-18545} + 2 * index )) \
    L2_PORT=$(( ${BASE_PORT:-18545} + 2 * index + 1 )) \
    L1_CHAIN_ID=$(( ${BASE_CHAIN_ID:-41337} + 2 * index )) \
    L2_CHAIN_ID=$(( ${BASE_CHAIN_ID:-41337} + 2 * index + 1 )) \
        timeout --kill-after=5s "$limit" bash script/e2e/lib/local-scenario.sh "$scenario" >"$logs/$name.log" 2>&1 &
    child=$!
    wait "$child" || status=$?
    stop_child
    case "$status" in
        0) result=PASS; passed=$((passed + 1)) ;;
        124|137) result=TIMEOUT; echo 'CI scenario deadline exceeded' >> "$logs/$name.log" ;;
        *) result=FAIL ;;
    esac
    printf '%s\t%s\t%s\t%s\n' "$name" "$result" "$status" "$((SECONDS - started))" >> "$logs/results.tsv"
    echo "[$name] $result ($((SECONDS - started))s)"
    [[ "$result" == PASS ]] || tail -n 25 "$logs/$name.log"
done
echo "RESULT: $passed/${#scenarios[@]} passed; $((${#scenarios[@]} - passed)) failed/timed out/not run"
(( passed == ${#scenarios[@]} ))
