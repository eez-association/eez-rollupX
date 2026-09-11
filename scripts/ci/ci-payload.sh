#!/usr/bin/env bash
# Immutable, same-revision GHCR handoff. Never execute a payload container.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
operation="${1:?publish or fetch}"
kind="${2:?runtime or tests}"
[[ "$kind" == runtime || "$kind" == tests ]] || { echo "Invalid payload kind" >&2; exit 1; }
: "${GITHUB_REPOSITORY:?}" "${GITHUB_SHA:?}" "${RUNNER_TEMP:?}"
repository="ghcr.io/${GITHUB_REPOSITORY,,}-ci-$kind"
protocol="$(git -C "$REPO/eez-core-protocol" rev-parse HEAD)"
source_url="https://github.com/$GITHUB_REPOSITORY"
container_id=""
trap 'if [[ -n "$container_id" ]]; then docker rm "$container_id" >/dev/null || true; fi' EXIT

case "$operation" in
    publish)
        directory="${3:?payload directory}"
        : "${GITHUB_RUN_ID:?}" "${GITHUB_RUN_ATTEMPT:?}" "${GITHUB_OUTPUT:?}"
        # Narrow the build context: no successful-run diagnostics or credentials.
        context="$(mktemp -d "$RUNNER_TEMP/eez-ci-payload.XXXXXXXX")"
        cp "$directory/manifest.json" "$context/manifest.json"
        if [[ "$kind" == runtime ]]; then
            cp "$directory/runtime-binaries.tar.gz" "$context/"
        else
            cp -R "$directory/target" "$context/target"
        fi
        tag="$repository:sha-$GITHUB_SHA-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"
        docker buildx build --builder default --platform linux/amd64 --provenance=false \
            --file "$REPO/Dockerfile.ci-payload" --load --tag "$tag" \
            --label "org.opencontainers.image.source=$source_url" \
            --label "org.opencontainers.image.revision=$GITHUB_SHA" \
            --label "io.eez.protocol.revision=$protocol" \
            --label "io.eez.ci.payload=$kind" "$context"
        docker push "$tag"
        reference="$(docker image inspect --format '{{json .RepoDigests}}' "$tag" | jq -er \
            --arg prefix "$repository@sha256:" \
            'map(select(startswith($prefix))) | unique | if length == 1 then .[0] else error("ambiguous payload digest") end')"
        [[ "$reference" =~ @sha256:[0-9a-f]{64}$ ]] || { echo "Invalid payload digest" >&2; exit 1; }
        printf '%s-ref=%s\n' "$kind" "$reference" >> "$GITHUB_OUTPUT"
        ;;
    fetch)
        reference="${3:?immutable payload reference}"
        directory="${4:?empty output directory}"
        [[ "$reference" == "$repository@sha256:"* && "$reference" =~ @sha256:[0-9a-f]{64}$ ]] || {
            echo "Expected a repository-bound immutable payload digest" >&2; exit 1;
        }
        mkdir -p "$directory"
        [[ -z "$(ls -A "$directory")" ]] || { echo "Payload directory must be empty" >&2; exit 1; }
        docker pull --platform linux/amd64 "$reference"
        docker image inspect "$reference" | jq -e --arg source "$source_url" --arg root "$GITHUB_SHA" \
            --arg protocol "$protocol" --arg kind "$kind" '
            .[0] | .Os == "linux" and .Architecture == "amd64" and
            .Config.Labels["org.opencontainers.image.source"] == $source and
            .Config.Labels["org.opencontainers.image.revision"] == $root and
            .Config.Labels["io.eez.protocol.revision"] == $protocol and
            .Config.Labels["io.eez.ci.payload"] == $kind' >/dev/null || {
                echo "Payload identity/platform mismatch" >&2; exit 1;
            }
        container_id="$(docker create "$reference" /not-executed)"
        [[ "$container_id" =~ ^[0-9a-f]{64}$ ]] || { echo "Invalid payload container ID" >&2; exit 1; }
        docker cp "$container_id:/payload/." "$directory/"
        ;;
    *) echo "Expected publish or fetch" >&2; exit 1 ;;
esac
