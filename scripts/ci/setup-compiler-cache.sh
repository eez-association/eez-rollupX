#!/usr/bin/env bash
set -euo pipefail
: "${GITHUB_ENV:?}"
backend=none
mode="${EEZ_COMPILER_CACHE_MODE:-native}"
enabled="${EEZ_COMPILER_CACHE_ENABLED:-true}"
[[ "$mode" == native || "$mode" == docker ]]
[[ "$enabled" == true || "$enabled" == false ]]
sccache_bin=""
if [[ "$enabled" == true ]]; then
    if [[ "${RUNNER_ENVIRONMENT:-}" == self-hosted && -n "${SCCACHE_REDIS_ENDPOINT:-}" ]]; then
        backend=redis
    elif [[ "${RUNNER_ENVIRONMENT:-}" == github-hosted && -n "${ACTIONS_RESULTS_URL:-}" && -n "${ACTIONS_RUNTIME_TOKEN:-}" ]]; then
        backend=gha
    fi
    if [[ "$mode" == native ]]; then
        if [[ "${RUNNER_ENVIRONMENT:-}" == self-hosted ]]; then
            # Preserve the runner's supported sccache installation/backend.
            candidate="${RUSTC_WRAPPER:-sccache}"
            if [[ "${candidate##*/}" == sccache ]] && command -v "$candidate" >/dev/null 2>&1; then
                sccache_bin="$(command -v "$candidate")"
                [[ "$backend" != none ]] || backend=inherited
            fi
        elif [[ "$backend" == gha ]]; then
            : "${RUNNER_TEMP:?}"
            tools_dir="$(mktemp -d "$RUNNER_TEMP/eez-sccache-bin.XXXXXXXX")"
            if bash scripts/ci/install-sccache.sh "$tools_dir"; then
                sccache_bin="$tools_dir/sccache"
            else
                echo "::warning::sccache installation unavailable; compiling without it"
            fi
        fi
        [[ -n "$sccache_bin" ]] || backend=none
    fi
fi
printf 'EEZ_COMPILER_CACHE=%s\nEEZ_SCCACHE_BIN=%s\n' "$backend" "$sccache_bin" >> "$GITHUB_ENV"
echo "Compiler cache: $backend ($mode); Cargo target stays job-local"
