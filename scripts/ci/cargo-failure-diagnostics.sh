#!/usr/bin/env bash
# Best-effort diagnostics for missing outputs, disk pressure and runner shutdowns.
set -u

echo "CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-}"
echo "CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-}"
echo "CARGO_PROFILE_DEV_DEBUG=${CARGO_PROFILE_DEV_DEBUG:-}"
echo "CARGO_PROFILE_TEST_DEBUG=${CARGO_PROFILE_TEST_DEBUG:-}"
echo "CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-}"
echo "RUSTC_WRAPPER=${RUSTC_WRAPPER:-}"

df -h . "${RUNNER_TEMP:-.}" || true
df -i . || true
free -h || true
for metric in memory.current memory.peak memory.max memory.events; do
    if [[ -r "/sys/fs/cgroup/$metric" ]]; then
        printf '\n%s:\n' "$metric"
        cat "/sys/fs/cgroup/$metric" || true
    fi
done

if [[ -n "${CARGO_TARGET_DIR:-}" && ! -d "$CARGO_TARGET_DIR" ]]; then
    echo "::error::Cargo target directory disappeared during the job: $CARGO_TARGET_DIR"
fi

if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    find "$CARGO_TARGET_DIR/debug/deps" \
        -maxdepth 1 \
        -name 'libversion_check*' \
        -ls 2>/dev/null || true

    find "$CARGO_TARGET_DIR/debug/.fingerprint" \
        -maxdepth 2 \
        -iname '*version_check*' \
        -ls 2>/dev/null || true
fi

if command -v sccache >/dev/null 2>&1; then
    timeout 15s sccache --show-stats || true
fi

ps -ef | grep -E '[c]argo|[r]ustc|[s]ccache|[c]lean|[r]m ' || true
