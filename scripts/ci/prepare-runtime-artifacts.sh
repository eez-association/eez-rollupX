#!/usr/bin/env bash
# Consume exactly the build job's candidate. Never rebuild a missing artifact.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_dir="${1:?usage: prepare-runtime-artifacts.sh ARTIFACT_DIRECTORY}"
artifact_dir="$(cd "$artifact_dir" && pwd)"
manifest="$artifact_dir/manifest.json"
: "${GITHUB_ENV:?}"
: "${GITHUB_PATH:?}"
: "${GITHUB_SHA:?}"
: "${GITHUB_REPOSITORY:?}"
: "${RUNNER_TEMP:?}"
: "${EEZ_EXPECT_PUBLISHED:?}"

[[ "$(uname -s)/$(uname -m)" == Linux/x86_64 ]] || { echo "Runtime artifacts require Linux amd64" >&2; exit 1; }
jq -e --arg root "$(git -C "$REPO" rev-parse HEAD)" \
    --arg workflow_sha "$GITHUB_SHA" \
    --arg protocol "$(git -C "$REPO/eez-core-protocol" rev-parse HEAD)" \
    --arg lock "$(sha256sum "$REPO/Cargo.lock" | awk '{print $1}')" \
    --argjson published "$EEZ_EXPECT_PUBLISHED" '
    .schema == 2 and .root_commit == $root and .root_commit == $workflow_sha and
    .protocol_commit == $protocol and .cargo_lock_sha256 == $lock and
    .platform == "linux/amd64" and .profile == "release" and .published == $published and
    (.images | keys) == ["deploy", "node", "signer"] and
    (.binaries_sha256 | test("^[0-9a-f]{64}$"))
' "$manifest" >/dev/null || { echo "Runtime manifest does not match this checkout/build contract" >&2; exit 1; }

verify_archive() {
    local checksum="$1" filename="$2"
    [[ "$checksum" =~ ^[0-9a-f]{64}$ ]] || { echo "Invalid archive checksum" >&2; exit 1; }
    printf '%s  %s\n' "$checksum" "$artifact_dir/$filename" | sha256sum --check --status || {
        echo "Runtime archive checksum mismatch: $filename" >&2; return 1;
    }
}
verify_archive "$(jq -er .binaries_sha256 "$manifest")" runtime-binaries.tar.gz

prefix="ghcr.io/${GITHUB_REPOSITORY,,}"
for role in node signer deploy; do
    reference="$(jq -er --arg role "$role" '.images[$role].reference' "$manifest")"
    case "$role" in
        node) repository="$prefix-node"; env_name=EEZ_NODE_IMAGE ;;
        signer) repository="$prefix-proof-signer"; env_name=EEZ_PROOF_SIGNER_IMAGE ;;
        deploy) repository="$prefix-deploy"; env_name=EEZ_DEPLOY_IMAGE ;;
    esac
    if [[ "$EEZ_EXPECT_PUBLISHED" == true ]]; then
        [[ "$reference" == "$repository@sha256:"* && "$reference" =~ @sha256:[0-9a-f]{64}$ ]] || {
            echo "Expected a repository-bound image digest, got $reference" >&2; exit 1;
        }
        # Private registry credentials live in the runner, not in Kurtosis's API
        # container. Pre-pull into this job's daemon before starting the enclave.
        docker pull --platform linux/amd64 "$reference"
    else
        [[ "$reference" == "$repository:sha-$GITHUB_SHA-"* && "$reference" != *$'\n'* ]] || {
            echo "Unexpected local candidate reference" >&2; exit 1;
        }
        [[ "$(docker image inspect --format '{{.Id}}' "$reference")" == \
            "$(jq -er --arg role "$role" '.images[$role].image_id' "$manifest")" ]] || {
            echo "Local candidate bytes changed for $role" >&2; exit 1;
        }
    fi
    # The digest pull / same-daemon image ID binds its bytes to the producer.
    # Check portable OCI identity too; local .Id is NOT portable between classic
    # Docker (config ID) and the containerd store (manifest/index ID).
    docker image inspect "$reference" | jq -e \
        --arg root "$GITHUB_SHA" --arg source "https://github.com/$GITHUB_REPOSITORY" \
        --arg protocol "$(jq -er .protocol_commit "$manifest")" '
        .[0] | .Os == "linux" and .Architecture == "amd64" and
        .Config.Labels["org.opencontainers.image.revision"] == $root and
        .Config.Labels["org.opencontainers.image.source"] == $source and
        .Config.Labels["io.eez.protocol.revision"] == $protocol and
        .Config.Labels["io.eez.build.profile"] == "release"
    ' >/dev/null || { echo "Image $role does not match the candidate's identity/platform" >&2; exit 1; }
    printf '%s=%s\n' "$env_name" "$reference" >> "$GITHUB_ENV"
done

binary_dir="$(mktemp -d "$RUNNER_TEMP/eez-runtime-bin.XXXXXXXX")"
# Only regular runtime files belong in this archive; no links or paths outside
# the extraction directory. This also rejects accidentally exporting target/.
python3 - "$artifact_dir/runtime-binaries.tar.gz" "$binary_dir" <<'PY'
import sys
import tarfile

allowed = {"rustc-version.txt", "cargo-version.txt"} | {
    "bin/" + name for name in (
        "eez-composer", "eez-follower", "eez-proof-signer", "eez-observer",
        "eez-genesis-state-root", "eez-genesis-block-hash",
        "e2e-causal-verify", "verify_blob_evidence",
    )
}
with tarfile.open(sys.argv[1], "r:gz") as archive:
    names = set()
    for member in archive.getmembers():
        if member.isdir() and member.name == "bin":
            continue
        if not member.isfile() or member.name not in allowed or member.name in names:
            raise SystemExit("Unexpected runtime archive entry: " + member.name)
        names.add(member.name)
    if names != allowed:
        raise SystemExit("Runtime archive is incomplete")
    archive.extractall(sys.argv[2], filter="data")
PY
for binary in eez-composer eez-follower eez-proof-signer eez-observer eez-genesis-state-root \
    eez-genesis-block-hash e2e-causal-verify verify_blob_evidence; do
    [[ -x "$binary_dir/bin/$binary" ]] || { echo "Runtime archive is missing executable $binary" >&2; exit 1; }
done
printf '%s\n' "$binary_dir/bin" >> "$GITHUB_PATH"
{
    printf 'EEZ_CAUSAL_VERIFIER_BIN=%s/bin/e2e-causal-verify\n' "$binary_dir"
    printf 'EEZ_BLOB_VERIFIER_BIN=%s/bin/verify_blob_evidence\n' "$binary_dir"
    printf 'EEZ_SKIP_NODE_BUILD=1\nEEZ_SKIP_PROOF_SIGNER_BUILD=1\nEEZ_SKIP_DEPLOY_BUILD=1\n'
} >> "$GITHUB_ENV"
echo "Prepared exact runtime artifacts for $GITHUB_SHA (no compilation)"
