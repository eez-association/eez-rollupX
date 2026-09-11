#!/usr/bin/env bash
set -euo pipefail

# ARC starts Docker in a per-pod sidecar. Check it before expensive setup.
for attempt in {1..6}; do
    if timeout 5s docker info >/dev/null 2>&1; then
        echo "Docker daemon is ready"
        exit 0
    fi
    if (( attempt < 6 )); then sleep 5; fi
done
timeout 5s docker version || true
echo "::error::Docker daemon unavailable. ARC runners need a ready per-pod DinD daemon and its socket mounted in the runner container. Check DOCKER_HOST and docker info inside the runner."
exit 1
