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
if [[ "$suite" == cross-chain ]]; then
    # Live observer acceptance uses only an owned, network-disabled PostgreSQL
    # container and private Unix socket. The real native loss-alert gate also
    # needs pinned Prometheus/Grafana. The observer comes from the sealed
    # runtime payload selected below, never a consumer-side rebuild.
    bash scripts/ci/check-docker.sh
    python3 - <<'PYTHON'
import json, subprocess
images = json.load(open('observability/images.lock.json'))
for prefix in ('postgres:', 'prom/prometheus:', 'grafana/grafana:'):
    image, digest = next((name, digest) for name, digest in images.items() if name.startswith(prefix))
    subprocess.run(['docker', 'pull', image + '@' + digest], check=True)
PYTHON
    export EEZ_TEST_QUOTA_IMAGE="eez-native-quota:${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}"
    docker build --tag "$EEZ_TEST_QUOTA_IMAGE" --file observability/tests/spool-quota/Dockerfile observability/tests/spool-quota
fi
forge build --root contracts
python3 scripts/ci/native-tests.py run "$directory" --suite "$suite" "${partition[@]}" 2>&1 \
    | tee "$PWD/artifacts/$suite-e2e/nextest.log"
