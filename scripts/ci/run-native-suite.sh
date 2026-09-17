#!/usr/bin/env bash
# Identical execution entrypoint for separate jobs and no-publication fallback.
set -euo pipefail
suite="${1:?embedded or cross-chain}"
directory="${2:?verified E2E payload directory}"
[[ "$suite" == embedded || "$suite" == cross-chain ]] || exit 2
partition=()
if [[ -n "${3:-}" ]]; then
    partition=(--partition "$3")
fi
export CARGO_INCREMENTAL=0
source "$(dirname "${BASH_SOURCE[0]}")/init-cargo-target.sh"
export EEZ_TEST_LOG_DIR="$PWD/artifacts/$suite-e2e/logs"
mkdir -p "$EEZ_TEST_LOG_DIR"
forge build --root contracts
python3 scripts/ci/native-tests.py run "$directory" --suite "$suite" "${partition[@]}" 2>&1 \
    | tee "$PWD/artifacts/$suite-e2e/nextest.log"
