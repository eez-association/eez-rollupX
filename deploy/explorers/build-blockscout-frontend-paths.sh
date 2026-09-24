#!/usr/bin/env bash
# Build a separate Blockscout frontend image for the /explorer public path.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
work_dir="$(mktemp -d /tmp/eez-blockscout-paths.XXXXXXXX)"
trap 'rm -rf -- "$work_dir"' EXIT
revision=95feb0ba3245c67b1ced38e71a38569951add266
curl --fail --location --retry 3 \
  "https://codeload.github.com/blockscout/frontend/tar.gz/$revision" \
  --output "$work_dir/source.tar.gz"
mkdir "$work_dir/source"
tar -xzf "$work_dir/source.tar.gz" --strip-components=1 -C "$work_dir/source"
patch --batch -p1 -d "$work_dir/source" < "$script_dir/blockscout-frontend-paths.patch"
docker build \
  --build-arg "GIT_COMMIT_SHA=$revision" \
  --build-arg GIT_TAG=v2.3.5 \
  -t eez-blockscout-frontend:2.3.5-paths "$work_dir/source"
