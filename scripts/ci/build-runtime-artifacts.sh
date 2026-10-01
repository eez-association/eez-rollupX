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
state_cache="${EEZ_CARGO_STATE_CACHE:-false}"
[[ "$state_cache" == true || "$state_cache" == false ]] || { echo "EEZ_CARGO_STATE_CACHE must be true or false" >&2; exit 1; }
# Optional resource overrides for the retained-state compile cap; see
# resolve_build_cap. Unset or empty selects the Docker daemon's own values.
cpus_override="${EEZ_CI_BUILD_CPUS:-}"
memory_override="${EEZ_CI_BUILD_MEMORY:-}"
[[ -z "$cpus_override" || "$cpus_override" =~ ^[1-9][0-9]{0,2}$ ]] || {
    echo "EEZ_CI_BUILD_CPUS must be a positive whole number of CPUs" >&2; exit 1;
}
[[ -z "$memory_override" || "$memory_override" =~ ^[1-9][0-9]{0,15}$ ]] || {
    echo "EEZ_CI_BUILD_MEMORY must be a byte count, such as the downward API's limits.memory" >&2; exit 1;
}
operation="${1:-build}"
[[ "$operation" == build || "$operation" == prepare-state || "$operation" == export-state ]] || exit 2

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

# Resolve a moving tag once, shared by all outputs in this job.
resolve_base() {
    local tagged="$1" repository="${1%:*}" digest reference
    if [[ "$tagged" =~ @sha256:[0-9a-f]{64}$ ]]; then
        reference="$tagged"
    else
        digest="$(docker buildx imagetools inspect "$tagged" --format '{{.Manifest.Digest}}')"
        [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "Invalid base digest for $tagged" >&2; return 1; }
        reference="$repository@$digest"
    fi
    if ! docker image inspect "$reference" >/dev/null 2>&1; then
        docker pull --platform linux/amd64 "$reference" >&2
    fi
    printf '%s\n' "$reference"
}

# Size the retained-state compile cap from the Docker daemon that runs the build.
# CPUs: all of the daemon's CPUs. Memory: its MemTotal rounded down to whole GiB,
# minus a fixed 4 GiB for the daemon, BuildKit and the runner, so Cargo fails at
# its own cgroup limit instead of the node OOM-killing the job. Whole GiB keeps
# the value, and the BuildKit cache identity derived from it, stable across
# nominally identical nodes whose kernels reserve slightly different memory.
# Under ARC's DinD sidecar, `docker info` reports the node, not the sidecar's
# Kubernetes limits. That is correct only when the scale set gives this job the
# whole node. Otherwise the runner must export the sidecar's limits (downward API
# resourceFieldRef limits.cpu and limits.memory, default divisor) as
# EEZ_CI_BUILD_CPUS/EEZ_CI_BUILD_MEMORY; overrides replace the detected values
# and may only lower them. Cargo inside the nested build container cannot see an
# outer quota, so CARGO_BUILD_JOBS is passed explicitly from the same CPU count.
resolve_build_cap() {
    local cpus memory
    cpus="$(docker info --format '{{.NCPU}}')"
    memory="$(docker info --format '{{.MemTotal}}')"
    [[ "$cpus" =~ ^[1-9][0-9]{0,3}$ && "$memory" =~ ^[1-9][0-9]{0,15}$ ]] || {
        echo "Docker reported invalid NCPU/MemTotal: $cpus/$memory" >&2; exit 1;
    }
    cpu_source="auto: docker info NCPU"
    memory_source="auto: docker info MemTotal $memory bytes"
    if [[ -n "$cpus_override" ]]; then
        (( cpus_override <= cpus )) || {
            echo "EEZ_CI_BUILD_CPUS=$cpus_override exceeds the Docker daemon's $cpus CPUs" >&2; exit 1;
        }
        cpus="$cpus_override" cpu_source="override: EEZ_CI_BUILD_CPUS"
    fi
    if [[ -n "$memory_override" ]]; then
        (( memory_override <= memory )) || {
            echo "EEZ_CI_BUILD_MEMORY=$memory_override exceeds the Docker daemon's $memory bytes" >&2; exit 1;
        }
        memory="$memory_override" memory_source="override: EEZ_CI_BUILD_MEMORY $memory_override bytes"
    fi
    build_cpus="$cpus"
    build_memory_gib=$(( memory / 1073741824 - 4 ))
    (( build_memory_gib >= 8 )) || {
        echo "Cargo state builds require at least 8 GiB after 4 GiB headroom; $memory bytes available ($memory_source)" >&2
        exit 1
    }
}

# Docker enforces the limits; build-runtime.sh verifies them in its cgroup and
# sets CARGO_BUILD_JOBS. The build arguments are part of build-offline's
# BuildKit cache identity, so every state build in one job (restore check,
# compilation, packaging, export) uses the cap recorded once in plan.json.
set_build_cap() {
    build_cap=(--resource cpu-period=100000 --resource "cpu-quota=$(( build_cpus * 100000 ))"
               --resource "memory=${build_memory_gib}g" --resource "memory-swap=${build_memory_gib}g"
               --build-arg "EEZ_BUILD_CPUS=$build_cpus"
               --build-arg "EEZ_BUILD_MEMORY_BYTES=$(( build_memory_gib * 1073741824 ))")
    state_build+=("${build_cap[@]}")
    echo "==> Cargo state compile cap: $build_cpus CPUs ($cpu_source), ${build_memory_gib} GiB ($memory_source, rounded down to whole GiB, minus 4 GiB headroom)"
}

# Keep the cache path stable for Actions, but never reuse a local job directory.
# Only transport/ belongs to the cache; the authoritative checkout and plan do not.
state_dir="$REPO/.ci-cargo-state"
inputs=(Cargo.toml Cargo.lock crates contracts/test/PostBatchGasPins.t.sol
        .config/nextest.toml scripts/ci/native-tests.py scripts/ci/with-compiler-cache.sh)
state_build=(docker buildx build --builder default --platform linux/amd64 --progress plain
             --provenance=false -f Dockerfile.runtime --build-arg EEZ_COMPILER_CACHE=none
             --build-arg BUILD_STAGE=build-offline)
if [[ "$operation" == prepare-state ]]; then
    [[ "$state_cache" == true ]]
    git diff --quiet HEAD -- "${inputs[@]}" || { echo 'Cargo state requires clean compilation inputs' >&2; exit 1; }
    [[ -z "$(git ls-files --others --exclude-standard -- "${inputs[@]}")" ]] || {
        echo 'Cargo state requires tracked compilation inputs' >&2; exit 1;
    }
    resolve_build_cap
    [[ "$(df -PB1 "$REPO" | awk 'NR == 2 {print $4}')" -ge 53687091200 ]] || {
        echo 'Cargo state requires 50 GiB free disk; no automatic cleanup' >&2; exit 1;
    }
    mkdir "$state_dir"
    set_build_cap
    rust_base="$(resolve_base "${RUST_IMAGE:-rust:1.95.0-bookworm}")"
    "${state_build[@]}" --build-arg "RUST_IMAGE=$rust_base" --target build-contract \
        --output "type=local,dest=$state_dir/contract" .
    compatibility="$( { printf '%s\n' "$rust_base"; cat "$state_dir/contract/toolchain.txt";
        sha256sum Dockerfile.runtime scripts/ci/native-tests.py .config/nextest.toml;
    } | sha256sum | cut -d' ' -f1)"
    source="$(git ls-tree -r HEAD -- "${inputs[@]}" | sha256sum | cut -d' ' -f1)"
    jq -n --arg root "$root_commit" --arg protocol "$protocol_commit" --arg rust "$rust_base" \
        --arg compatibility "$compatibility" --arg source "$source" \
        --argjson cpus "$build_cpus" --argjson memory_gib "$build_memory_gib" \
        --arg cpu_source "$cpu_source" --arg memory_source "$memory_source" \
        '{root:$root, protocol:$protocol, rust:$rust, compatibility:$compatibility, source:$source,
          build_cap:{cpus:$cpus, memory_gib:$memory_gib, cpu_source:$cpu_source, memory_source:$memory_source}}' \
        > "$state_dir/plan.json"
    prefix="cargo-oci-v1-linux-amd64-$compatibility-"
    printf 'key=%s%s\nprefix=%s\n' "$prefix" "$source" "$prefix" >> "${GITHUB_OUTPUT:?}"
    exit 0
fi

# OCI blobs are opaque Docker inputs, not a tar archive extracted beside source.
# Validate the bounded layout/config before BuildKit reads and verifies its blobs.
state_digest() {
    local layout="$1" compatibility="$2" digest manifest config
    [[ -d "$layout" && ! -L "$layout" ]] || return 1
    [[ -z "$(find "$layout" ! -type d ! -type f -print -quit)" ]] || return 1
    [[ "$(du -sb "$layout" | cut -f1)" -le 4294967296 ]] || return 1
    [[ "$(stat -c %s "$layout/index.json")" -le 1048576 ]] || return 1
    digest="$(jq -er '.manifests | select(length == 1) | .[0].digest | select(test("^sha256:[0-9a-f]{64}$"))' "$layout/index.json")" || return 1
    manifest="$layout/blobs/sha256/${digest#sha256:}"
    [[ "$(stat -c %s "$manifest")" -le 1048576 ]] || return 1
    config="$(jq -er '.config.digest | select(test("^sha256:[0-9a-f]{64}$"))' "$manifest")" || return 1
    config="$layout/blobs/sha256/${config#sha256:}"
    [[ "$(stat -c %s "$config")" -le 1048576 ]] || return 1
    jq -e --arg compatibility "$compatibility" '
        .os == "linux" and .architecture == "amd64" and
        .config.Labels["io.eez.cargo.compatibility"] == $compatibility
    ' "$config" >/dev/null || return 1
    printf '%s\n' "$digest"
}

if [[ "$state_cache" == true ]]; then
    jq -e --arg root "$root_commit" --arg protocol "$protocol_commit" \
        '.root == $root and .protocol == $protocol' "$state_dir/plan.json" >/dev/null
    git diff --quiet HEAD -- "${inputs[@]}" || exit 1
    rust_base="$(jq -er .rust "$state_dir/plan.json")"
    compatibility="$(jq -er .compatibility "$state_dir/plan.json")"
    build_cpus="$(jq -er '.build_cap.cpus | tostring' "$state_dir/plan.json")"
    build_memory_gib="$(jq -er '.build_cap.memory_gib | tostring' "$state_dir/plan.json")"
    cpu_source="$(jq -er .build_cap.cpu_source "$state_dir/plan.json")"
    memory_source="$(jq -er .build_cap.memory_source "$state_dir/plan.json")"
    [[ "$build_cpus" =~ ^[1-9][0-9]{0,3}$ && "$build_memory_gib" =~ ^[1-9][0-9]{0,6}$ ]]
    set_build_cap
    state_build+=(--build-arg "RUST_IMAGE=$rust_base")
fi
if [[ "$operation" == export-state ]]; then
    [[ "$state_cache" == true && "${GITHUB_EVENT_NAME:-}" == push &&
       "${GITHUB_REF:-}" == "refs/heads/${EEZ_DEFAULT_BRANCH:-main}" ]] || {
        echo 'Only default-branch pushes may save Cargo state' >&2; exit 1;
    }
    jq -e -s --arg root "$root_commit" --arg protocol "$protocol_commit" '
        length == 2 and .[0] == .[1] and
        .[0].root_commit == $root and .[0].protocol_commit == $protocol
    ' "$EEZ_NATIVE_TEST_OUTPUT_DIR/manifest.json" "$EEZ_NATIVE_TEST_OUTPUT_DIR/producer-passed.json" >/dev/null
    previous="$(jq -er '.previous // ""' "$state_dir/plan.json")"
    [[ -z "$previous" ]] || state_build+=(--build-context "previous-state=$previous")
    "${state_build[@]}" --target cargo-state \
        --label "io.eez.cargo.compatibility=$compatibility" \
        --output "type=oci,tar=false,compression=zstd,compression-level=1,dest=$state_dir/next" .
    state_digest "$state_dir/next" "$compatibility" >/dev/null
    # Keep restored blobs available until the export finishes. Neither directory
    # can contain the authoritative checkout; both are owned by this job.
    [[ ! -e "$state_dir/transport" ]] || mv "$state_dir/transport" "$state_dir/restored"
    mv "$state_dir/next" "$state_dir/transport"
    printf 'upload=true\n' >> "${GITHUB_OUTPUT:?}"
    exit 0
fi

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
                # Large binaries and already-compressed test archives do not
                # justify gzip's CPU cost. Existing cache blobs remain reusable.
                runtime_cache_to=(--cache-to "type=registry,ref=$runtime_ref,mode=min,compression=zstd,compression-level=1,ignore-error=true")
                deploy_cache_to=(--cache-to "type=registry,ref=$deploy_ref,mode=max,ignore-error=true")
            fi
        done
        cache_status="GHCR (one ref-scoped source or default-branch fallback, best-effort export)"
    else
        cache_status="disabled (Docker driver lacks the containerd image store)"
    fi
fi
echo "==> BuildKit cache: $cache_status"

[[ "$state_cache" == true ]] || rust_base="$(resolve_base "${RUST_IMAGE:-rust:1.95.0-bookworm}")"
runtime_base="$(resolve_base "${RUNTIME_IMAGE:-debian:bookworm-slim}")"
build=(docker buildx build --builder default --platform linux/amd64 --progress plain --provenance=false
       --build-arg "RUNTIME_IMAGE=$runtime_base")
runtime_build=("${build[@]}" "${runtime_cache_from[@]}" -f Dockerfile.runtime --build-arg "RUST_IMAGE=$rust_base")
# Only the backend name is a build argument. Credential values travel through
# BuildKit secret mounts, never argv, layer history, metadata or GHCR caches.
compiler_cache="${EEZ_COMPILER_CACHE:-none}"
[[ "$state_cache" == false ]] || compiler_cache=none
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

if [[ "$state_cache" == true ]]; then
    runtime_build+=(--build-arg BUILD_STAGE=build-offline "${build_cap[@]}")
    previous=""
    if digest="$(state_digest "$state_dir/transport" "$compatibility" 2>/dev/null)"; then
        candidate="oci-layout://$state_dir/transport@$digest"
        # Only restoration can fall back. A Cargo or test failure is never retried.
        if timeout 600 "${state_build[@]}" --build-context "previous-state=$candidate" \
            --target checked-state --output type=cacheonly .; then
            previous="$candidate"
            runtime_build+=(--build-context "previous-state=$previous")
        else
            echo '::warning::Cargo state rejected before compilation; building cold'
        fi
    fi
    jq --arg previous "$previous" '. + {previous:$previous}' "$state_dir/plan.json" > "$state_dir/selected.json"
    mv "$state_dir/selected.json" "$state_dir/plan.json"
    echo "==> Docker Cargo state: $([[ -n "$previous" ]] && echo hit || echo miss)"
fi

phase=compilation
echo "==> compiling runtime binaries, then release tests (linux/amd64, same target)"
# Cache only the flat executable output, not Cargo state or its intermediate
# layers. Node and signer both consume this same cached binaries stage.
"${runtime_build[@]}" --target binaries "${runtime_cache_to[@]}" \
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
