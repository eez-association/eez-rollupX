#!/usr/bin/env bash
# One measured build operation. Never retries Cargo or cleans an active target.
# Each operation owns its sccache server: no cross-job stops or counter resets.
set -uo pipefail
# BuildKit can expose absent optional secrets as empty environment variables.
# sccache 0.17 rejects these two variables being present together, even for
# non-Redis backends. Preserve explicit values, including a TTL of zero.
for cache_expiry_var in SCCACHE_REDIS_TTL SCCACHE_REDIS_EXPIRATION; do
    if [[ -z "${!cache_expiry_var:-}" ]]; then
        unset "$cache_expiry_var"
    fi
done
name="${1:?metric name}"
shift
[[ "$name" =~ ^[a-z0-9-]+$ && "$#" -gt 0 ]] || exit 2
metrics="${EEZ_BUILD_METRICS_DIR:-artifacts/build-metrics}"
mkdir -p "$metrics" || exit 1
backend="${EEZ_COMPILER_CACHE:-none}"
cache_bin="${EEZ_SCCACHE_BIN:-sccache}"
server_started=false
case "$backend" in
    none|inherited) ;;
    redis) [[ -n "${SCCACHE_REDIS_ENDPOINT:-}" ]] || backend=none ;;
    gha) [[ -n "${ACTIONS_RESULTS_URL:-}" && -n "${ACTIONS_RUNTIME_TOKEN:-}" ]] || backend=none ;;
    *) echo "Invalid compiler cache backend" >&2; exit 2 ;;
esac
if [[ "$backend" != none ]] && command -v "$cache_bin" >/dev/null 2>&1; then
    # Short names keep Unix socket paths within Linux's 108-byte limit even
    # when a runner supplies a fairly long temporary-directory prefix.
    session="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/scc.XXXXXXXX")" || exit 1
    export SCCACHE_SERVER_UDS="$session/s"
    export SCCACHE_IDLE_TIMEOUT=0 SCCACHE_IGNORE_SERVER_IO_ERROR=1
    case "$backend" in
        # The hosted backend uses ACTIONS_RESULTS_URL (API v2). OpenDAL in
        # sccache 0.17 otherwise silently selects the retired v1 API, including
        # inside BuildKit where the runner's API-version flag is not inherited.
        gha) export SCCACHE_GHA_ENABLED=on ACTIONS_CACHE_SERVICE_V2=true ;;
        redis) unset SCCACHE_GHA_ENABLED ;;
        inherited) ;;
        *) echo "Invalid compiler cache backend" >&2; exit 2 ;;
    esac
    # Backend errors may contain credential-bearing URLs. Never print them.
    if timeout 20s "$cache_bin" --start-server >/dev/null 2>&1; then
        export RUSTC_WRAPPER="$cache_bin"
        server_started=true
    else
        timeout 5s "$cache_bin" --stop-server >/dev/null 2>&1 || true
        echo "::warning::Compiler cache unavailable; running the build uncached"
        export RUSTC_WRAPPER=""
    fi
else
    export RUSTC_WRAPPER=""
fi
echo "==> $name: compiler cache=$backend, active=$server_started"
started=$SECONDS
if [[ -x /usr/bin/time ]]; then
    /usr/bin/time -v -o "$metrics/$name.time.txt" "$@"
    status=$?
else
    "$@"
    status=$?
fi
elapsed=$((SECONDS - started))
printf '{"elapsed_seconds":%d,"exit_code":%d,"cache_active":%s}\n' \
    "$elapsed" "$status" "$server_started" > "$metrics/$name.json"
echo "==> $name: elapsed=${elapsed}s, exit=$status"
if [[ "$server_started" == true ]]; then
    # Store counters only. Cache locations, URLs, credentials and environment
    # details are deliberately excluded from artifacts and job summaries.
    timeout 15s "$cache_bin" --show-stats --stats-format json 2>/dev/null \
        | python3 -c 'import json,sys
stats = json.load(sys.stdin)["stats"]
keys = "compile_requests requests_executed requests_not_cacheable requests_not_compile cache_hits cache_misses cache_errors cache_timeouts cache_read_errors cache_write_errors cache_writes compilations compile_fails non_cacheable_compilations not_cached cache_write_duration cache_read_hit_duration compiler_write_duration".split()
print(json.dumps({k: stats[k] for k in keys if k in stats}, indent=2))' \
        > "$metrics/$name.sccache.json" 2>/dev/null || true
    timeout 10s "$cache_bin" --stop-server >/dev/null 2>&1 || true
    # Also visible in failed Docker RUN logs, whose filesystem cannot be exported.
    python3 - "$metrics/$name.sccache.json" <<'PY' || true
import json, sys
with open(sys.argv[1]) as stream:
    stats = json.load(stream)
count = lambda key: sum(stats.get(key, {}).get("counts", {}).values())
print(f"sccache: hits={count('cache_hits')} misses={count('cache_misses')} "
      f"non-cacheable={stats.get('requests_not_cacheable', 0)} "
      f"read-errors={stats.get('cache_read_errors', 0)} write-errors={stats.get('cache_write_errors', 0)}")
PY
fi
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '\nBuild `%s`: %ss, exit %s, compiler cache %s (active: %s). Counters are in the step log; diagnostic artifacts are failure-only.\n' \
        "$name" "$elapsed" "$status" "$backend" "$server_started" >> "$GITHUB_STEP_SUMMARY"
fi
exit "$status"
