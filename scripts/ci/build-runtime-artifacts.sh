#!/usr/bin/env bash
# Build immutable runtime outputs once. Publish by digest, or use the images
# in the same job for no-write CI.
set -Eeuo pipefail
# Report the phase, not the failing command (which might contain credentials).
phase=initialization
trap 'status=$?; echo "::error::Runtime build failed during $phase (line $LINENO, exit $status)" >&2; exit "$status"' ERR

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"
: "${GITHUB_REPOSITORY:?}"
: "${GITHUB_SHA:?}"
: "${GITHUB_RUN_ID:?}"
: "${GITHUB_RUN_ATTEMPT:?}"
: "${RUNNER_TEMP:?}"
: "${EEZ_RUNTIME_OUTPUT_DIR:?}"
: "${EEZ_NATIVE_TEST_OUTPUT_DIR:?}"
publish="${EEZ_PUBLISH_IMAGES:-false}"
[[ "$publish" == true || "$publish" == false ]] || { echo "EEZ_PUBLISH_IMAGES must be true or false" >&2; exit 1; }
cache="${EEZ_RUNTIME_CACHE:-true}"
[[ "$cache" == true || "$cache" == false ]] || { echo "EEZ_RUNTIME_CACHE must be true or false" >&2; exit 1; }

root_commit="$(git rev-parse HEAD)"
protocol_commit="$(git -C eez-core-protocol rev-parse HEAD)"
pinned_protocol="$(git ls-files -s eez-core-protocol | awk '{print $2}')"
[[ "$root_commit" == "$GITHUB_SHA" && "$protocol_commit" == "$pinned_protocol" ]] || {
    echo "Runtime build must use the exact workflow revision and pinned protocol submodule" >&2
    exit 1
}
if [[ "$publish" == true ]]; then
    git diff --quiet HEAD || { echo "Refusing to publish a modified checkout" >&2; exit 1; }
fi

# The Docker driver resolves the just-built node image used by Dockerfile.deploy.
# Select it explicitly; a docker-container builder cannot see local image tags.
driver="$(docker buildx inspect default | awk '$1 == "Driver:" {print $2}')"
[[ "$driver" == docker ]] || { echo "The default Buildx builder must use the docker driver" >&2; exit 1; }
[[ "$(docker info --format '{{.Architecture}}')" == x86_64 ]] || { echo "Runtime builds currently require an amd64 Docker host" >&2; exit 1; }

mkdir -p "$EEZ_RUNTIME_OUTPUT_DIR"
[[ -z "$(ls -A "$EEZ_RUNTIME_OUTPUT_DIR")" ]] || { echo "Runtime output directory must be empty" >&2; exit 1; }
output="$(cd "$EEZ_RUNTIME_OUTPUT_DIR" && pwd)"
mkdir -p "$EEZ_NATIVE_TEST_OUTPUT_DIR"
[[ -z "$(ls -A "$EEZ_NATIVE_TEST_OUTPUT_DIR")" ]] || { echo "Test output directory must be empty" >&2; exit 1; }
test_output="$(cd "$EEZ_NATIVE_TEST_OUTPUT_DIR" && pwd)"
build_dir="$(mktemp -d "$RUNNER_TEMP/eez-runtime-build.XXXXXXXX")"
prefix="ghcr.io/${GITHUB_REPOSITORY,,}"
tag="sha-$root_commit-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"
node_image="$prefix-node:$tag"
signer_image="$prefix-proof-signer:$tag"
deploy_image="$prefix-deploy:$tag"
source_url="https://github.com/$GITHUB_REPOSITORY"
lock_sha="$(sha256sum Cargo.lock | awk '{print $1}')"

# Registry cache export needs the containerd image store with the Docker driver.
# Do not switch drivers or restart Docker: deploy's local node-image dependency
# and hosted runners with the classic store must keep working. Fork/read-only
# jobs retain the credential-free same-job path and never write this cache.
runtime_cache_from=(); runtime_cache_to=()
deploy_cache_from=(); deploy_cache_to=()
cache_status="disabled (opt-out, read-only event or missing ref)"
if [[ "$cache" == true && "$publish" == true && "${GITHUB_REF:-}" == refs/* ]]; then
    if docker info --format '{{json .DriverStatus}}' | jq -e \
        'any(.[]?; . == ["driver-type", "io.containerd.snapshotter.v1"])' >/dev/null; then
        cache_scopes=("$GITHUB_REF")
        default_ref="refs/heads/${EEZ_DEFAULT_BRANCH:-main}"
        [[ "$GITHUB_REF" == "$default_ref" ]] || cache_scopes+=("$default_ref")
        for scope in "${cache_scopes[@]}"; do
            # Mutable cache tags are separate from immutable candidate tags.
            # Import one source per image: overlapping BuildKit records from the
            # default branch can mask a matching ref-scoped compile layer.
            scope_hash="$(printf '%s' "$scope" | sha256sum | cut -c1-16)"
            cache_tag="buildcache-v1-linux-amd64-$scope_hash"
            runtime_ref="$prefix-node:$cache_tag"
            deploy_ref="$prefix-deploy:$cache_tag"
            if [[ ${#runtime_cache_from[@]} -eq 0 ]] &&
                docker buildx imagetools inspect "$runtime_ref" >/dev/null 2>&1; then
                runtime_cache_from=(--cache-from "type=registry,ref=$runtime_ref")
            fi
            if [[ ${#deploy_cache_from[@]} -eq 0 ]] &&
                docker buildx imagetools inspect "$deploy_ref" >/dev/null 2>&1; then
                deploy_cache_from=(--cache-from "type=registry,ref=$deploy_ref")
            fi
            if [[ "$scope" == "$GITHUB_REF" ]]; then
                runtime_cache_to=(--cache-to "type=registry,ref=$runtime_ref,mode=max,ignore-error=true")
                deploy_cache_to=(--cache-to "type=registry,ref=$deploy_ref,mode=max,ignore-error=true")
            fi
        done
        cache_status="GHCR (one ref-scoped source or default-branch fallback, best-effort export)"
    else
        cache_status="disabled (Docker driver lacks the containerd image store)"
    fi
fi
echo "==> BuildKit cache: $cache_status"

# Resolve moving base tags once. Every output must use the same toolchain/base,
# even if a registry tag changes between binary export and image packaging.
resolve_base() {
    local tagged="$1" repository="${1%:*}" digest
    digest="$(docker buildx imagetools inspect "$tagged" --format '{{.Manifest.Digest}}')"
    [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "Invalid base digest for $tagged" >&2; return 1; }
    docker pull --platform linux/amd64 "$repository@$digest" >&2
    printf '%s@%s\n' "$repository" "$digest"
}
rust_base="$(resolve_base rust:1.94.0-bookworm)"
runtime_base="$(resolve_base debian:bookworm-slim)"
build=(docker buildx build --builder default --platform linux/amd64 --progress plain --provenance=false
       --build-arg "RUNTIME_IMAGE=$runtime_base")
runtime_build=("${build[@]}" "${runtime_cache_from[@]}" -f Dockerfile.runtime --build-arg "RUST_IMAGE=$rust_base")
# Only the backend name is a build argument. Credential values travel through
# BuildKit secret mounts, never argv, layer history, metadata or GHCR caches.
compiler_cache="${EEZ_COMPILER_CACHE:-none}"
[[ "$compiler_cache" == none || "$compiler_cache" == redis || "$compiler_cache" == gha ]] || {
    echo "Invalid runtime compiler cache backend" >&2; exit 1;
}
runtime_build+=(--build-arg "EEZ_COMPILER_CACHE=$compiler_cache")
if [[ "$compiler_cache" == gha ]]; then
    cache_secrets=(ACTIONS_RESULTS_URL ACTIONS_RUNTIME_TOKEN)
elif [[ "$compiler_cache" == redis ]]; then
    cache_secrets=(SCCACHE_REDIS_ENDPOINT SCCACHE_REDIS_USERNAME SCCACHE_REDIS_PASSWORD
                   SCCACHE_REDIS_DB SCCACHE_REDIS_KEY_PREFIX SCCACHE_REDIS_TTL
                   SCCACHE_REDIS_EXPIRATION SCCACHE_REDIS_RW_MODE)
else
    cache_secrets=()
fi
for secret in "${cache_secrets[@]}"; do
    [[ -z "${!secret:-}" ]] || runtime_build+=(--secret "id=$secret,env=$secret")
done
labels=(--label "org.opencontainers.image.source=$source_url"
        --label "org.opencontainers.image.revision=$root_commit"
        --label "io.eez.protocol.revision=$protocol_commit"
        --label "io.eez.build.profile=release")

phase=compilation
echo "==> compiling runtime binaries, then release tests (linux/amd64, same target)"
"${runtime_build[@]}" --target binaries \
    --output "type=local,dest=$build_dir/binaries" \
    --metadata-file "$output/binaries-build.json" .

cp "$build_dir/binaries/cargo-timing.html" "$output/cargo-timing.html"
if [[ -d "$build_dir/binaries/build-metrics" ]]; then
    cp -R "$build_dir/binaries/build-metrics" "$output/build-metrics"
fi
tar -czf "$output/runtime-binaries.tar.gz" -C "$build_dir/binaries" \
    bin rustc-version.txt cargo-version.txt
cp -R "$build_dir/binaries/native-tests/." "$test_output/"
phase="release test identity validation"
python3 scripts/ci/native-tests.py seal "$test_output" --runtime-archive "$output/runtime-binaries.tar.gz"

# These targets share the exact cached builder stage above: no second Cargo run.
phase=packaging
echo "==> packaging runtime images"
"${runtime_build[@]}" --target node "${labels[@]}" \
    --load -t "$node_image" --metadata-file "$output/node-build.json" .
"${runtime_build[@]}" --target signer "${labels[@]}" \
    "${runtime_cache_to[@]}" \
    --load -t "$signer_image" --metadata-file "$output/signer-build.json" .
"${build[@]}" -f testing/kurtosis/Dockerfile.deploy "${labels[@]}" \
    "${deploy_cache_from[@]}" "${deploy_cache_to[@]}" \
    --build-arg "EEZ_NODE_IMAGE=$node_image" --load -t "$deploy_image" \
    --metadata-file "$output/deploy-build.json" .

images='{}'
for role in node signer deploy; do
    phase="identity validation for $role"
    case "$role" in
        node) image="$node_image" ;;
        signer) image="$signer_image" ;;
        deploy) image="$deploy_image" ;;
    esac
    image_id="$(docker image inspect --format '{{.Id}}' "$image")"
    [[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]] || {
        echo "Invalid image build identity for $role" >&2; exit 1;
    }
    reference="$image"
    if [[ "$publish" == true ]]; then
        phase="publication of $role"
        docker push "$image"
        repository="${image%:*}"
        reference="$(docker image inspect --format '{{json .RepoDigests}}' "$image" | jq -er \
            --arg prefix "$repository@sha256:" \
            'map(select(startswith($prefix))) | unique | if length == 1 then .[0] else error("missing/ambiguous pushed digest") end')"
        [[ "$reference" =~ @sha256:[0-9a-f]{64}$ ]] || { echo "Invalid pushed image digest" >&2; exit 1; }
    fi
    images="$(jq -c --arg role "$role" --arg reference "$reference" --arg id "$image_id" \
        '. + {($role): {reference: $reference, image_id: $id}}' <<<"$images")"
done

phase="artifact export"
binary_sha="$(sha256sum "$output/runtime-binaries.tar.gz" | awk '{print $1}')"
jq -n --arg root "$root_commit" --arg protocol "$protocol_commit" --arg lock "$lock_sha" \
    --argjson published "$publish" --argjson images "$images" \
    --arg binary_sha "$binary_sha" \
    --arg rust_base "$rust_base" --arg runtime_base "$runtime_base" \
    --rawfile rustc "$build_dir/binaries/rustc-version.txt" \
    --rawfile cargo "$build_dir/binaries/cargo-version.txt" \
    '{schema: 2, root_commit: $root, protocol_commit: $protocol, cargo_lock_sha256: $lock,
      platform: "linux/amd64", profile: "release", rustflags: "-C link-arg=-fuse-ld=mold",
      rustc: $rustc, cargo: $cargo, base_images: {rust: $rust_base, runtime: $runtime_base},
      published: $published, images: $images, binaries_sha256: $binary_sha}' > "$output/manifest.json"

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
        printf '### Runtime candidate\n\nRevision: `%s` · protocol: `%s` · linux/amd64 · release\n\n' "$root_commit" "$protocol_commit"
        jq -r '.images | to_entries[] | "- \(.key): `\(.value.reference)`"' "$output/manifest.json"
        printf '\nRuntime payload: binaries and manifest only. Build metadata and Cargo timings are uploaded separately on failure.\n'
        printf '\nBuildKit cache: %s.\n' "$cache_status"
    } >> "$GITHUB_STEP_SUMMARY"
fi
echo "Runtime artifacts ready: $output"
