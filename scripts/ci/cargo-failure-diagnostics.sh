#!/usr/bin/env bash
# Focused diagnostics for missing Cargo dependency artifacts in CI.
set -u

echo "CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
echo "CARGO_INCREMENTAL=$CARGO_INCREMENTAL"
echo "RUSTC_WRAPPER=${RUSTC_WRAPPER:-}"

find "$CARGO_TARGET_DIR/debug/deps" \
    -maxdepth 1 \
    -name 'libversion_check*' \
    -ls 2>/dev/null || true

find "$CARGO_TARGET_DIR/debug/.fingerprint" \
    -maxdepth 2 \
    -iname '*version_check*' \
    -ls 2>/dev/null || true

if command -v sccache >/dev/null 2>&1; then
    sccache --show-stats || true
fi

ps -ef | grep -E '[c]argo|[r]ustc|[s]ccache|[c]lean|[r]m ' || true
