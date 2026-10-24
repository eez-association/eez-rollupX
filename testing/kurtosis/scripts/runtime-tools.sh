#!/usr/bin/env bash
# Explicit artifact overrides fail closed; local development can still compile.
resolve_runtime_tool() {
    local package="$1" example="$2" prebuilt="${3:-}" repo target binary
    local features=()
    if [[ -n "$prebuilt" ]]; then
        [[ -x "$prebuilt" ]] || { echo "runtime tool is not executable at $prebuilt" >&2; return 1; }
        printf '%s\n' "$prebuilt"
        return 0
    fi
    if [[ "${GITHUB_ACTIONS:-false}" == true ]]; then
        echo "CI requires a verified $example tool; refusing Cargo fallback" >&2
        return 1
    fi
    repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
    target="${CARGO_TARGET_DIR:-$repo/target}"
    [[ "$target" == /* ]] || target="$repo/$target"
    if [[ "$package/$example" == eez-l1/verify_blob_evidence ]]; then
        features=(--features chain-spec)
    fi
    env CARGO_TARGET_DIR="$target" cargo build --locked --manifest-path "$repo/Cargo.toml" \
        -p "$package" --example "$example" "${features[@]}" >&2 || return 1
    binary="$target/debug/examples/$example"
    [[ -x "$binary" ]] || { echo "runtime tool was not built at $binary" >&2; return 1; }
    printf '%s\n' "$binary"
}
