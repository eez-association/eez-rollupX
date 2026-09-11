#!/usr/bin/env bash
# Optional job-private containerd image store on GitHub-hosted Linux only.
# Never reconfigure/restart/prune a homelab daemon. Select one daemon BEFORE any
# candidate build; all exports, local FROM dependencies and pushes then use it.
set -euo pipefail
[[ "${RUNNER_ENVIRONMENT:-}" == github-hosted && "${RUNNER_OS:-}" == Linux ]] || exit 0

has_containerd_store() {
    timeout 15s docker info --format '{{json .DriverStatus}}' | jq -e \
        'any(.[]?; . == ["driver-type", "io.containerd.snapshotter.v1"])' >/dev/null
}

case "${1:-}" in
    prepare)
        : "${GITHUB_OUTPUT:?}"
        echo 'configure=false' >> "$GITHUB_OUTPUT"
        if [[ "${EEZ_RUNTIME_CACHE:-true}" != true || "${EEZ_PUBLISH_IMAGES:-false}" != true \
              || "${EEZ_HOSTED_CONTAINERD:-true}" != true ]]; then
            exit 0
        fi
        if has_containerd_store; then
            echo "Docker already supports registry layer caching"
            exit 0
        fi
        original_host="${DOCKER_HOST:-$(docker context inspect --format '{{.Endpoints.docker.Host}}')}"
        # Only a VM-local daemon is eligible. Do not replace remote endpoints.
        [[ "$original_host" =~ ^unix:///[^[:space:]]+$ ]] || exit 0
        version="$(docker version --format '{{.Server.Version}}')"
        [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 0
        printf 'original-host=%s\nversion=v%s\nconfigure=true\n' "$original_host" "$version" >> "$GITHUB_OUTPUT"
        ;;
    select)
        : "${GITHUB_ENV:?}"
        : "${EEZ_ORIGINAL_DOCKER_HOST:?}"
        [[ "$EEZ_ORIGINAL_DOCKER_HOST" =~ ^unix:///[^[:space:]]+$ ]] || exit 1
        # DOCKER_CONTEXT overrides DOCKER_HOST. The build intentionally selects
        # the default docker-driver builder against this explicit socket.
        export DOCKER_CONTEXT=""
        export DOCKER_HOST="${EEZ_CACHE_DOCKER_HOST:-}"
        if [[ "${EEZ_DOCKER_SETUP_OUTCOME:-}" == success && "$DOCKER_HOST" =~ ^unix:///[^[:space:]]+$ ]] \
            && has_containerd_store; then
            echo "Using job-private Docker with GHCR layer caching"
        else
            export DOCKER_HOST="$EEZ_ORIGINAL_DOCKER_HOST"
            echo "::warning::Optional containerd setup unavailable; keeping the original Docker daemon"
            timeout 15s docker info >/dev/null
        fi
        printf 'DOCKER_CONTEXT=\nDOCKER_HOST=%s\n' "$DOCKER_HOST" >> "$GITHUB_ENV"
        ;;
    *) echo "Usage: hosted-docker-cache.sh prepare|select" >&2; exit 2 ;;
esac
