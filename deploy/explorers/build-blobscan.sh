#!/usr/bin/env bash
# Build the pinned upstream release plus the explicit Chiado compatibility patch.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
work_dir="$(mktemp -d /tmp/eez-blobscan-build.XXXXXXXX)"
build_db="eez-blobscan-build-$$"
cleanup() {
  docker rm -f "$build_db" >/dev/null 2>&1 || true
  rm -rf -- "$work_dir"
}
trap cleanup EXIT

revision=424d5e512d57ff4ac42509ef93f3845ead392155
curl --fail --location --retry 3 \
  "https://codeload.github.com/Blobscan/blobscan/tar.gz/$revision" \
  --output "$work_dir/source.tar.gz"
mkdir "$work_dir/source"
tar -xzf "$work_dir/source.tar.gz" --strip-components=1 -C "$work_dir/source"
patch --batch -p1 -d "$work_dir/source" < "$script_dir/blobscan-chiado.patch"

# Prisma's typed SQL generator needs a migrated database even at build time.
# These are disposable credentials for an empty schema, never runtime secrets.
docker run --rm -d --name "$build_db" -p 127.0.0.1::5432 \
  -e POSTGRES_USER=build -e POSTGRES_PASSWORD=build -e POSTGRES_DB=blobscan \
  --tmpfs /var/lib/postgresql/data:rw,size=1g \
  postgres@sha256:3d0f7584ed7d04e27fa050d6683a74746608faf21f202be78460d679cc56461f >/dev/null
build_port="$(docker port "$build_db" 5432/tcp | cut -d: -f2)"
[[ "$build_port" =~ ^[0-9]+$ ]]
build_url="postgresql://build:build@127.0.0.1:$build_port/blobscan"
ready=false
for attempt in {1..30}; do
  if docker exec "$build_db" pg_isready -U build -d blobscan >/dev/null; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]]
docker run --rm --network host \
  -e "DATABASE_URL=$build_url" -e "DIRECT_URL=$build_url" \
  --entrypoint /bin/sh \
  blossomlabs/blobscan-api@sha256:2edefdf2c959e6d0b574953e17a5a522135a8bf53931ab5bbef4707f6e7dd32c \
  -ec 'node node_modules/prisma/build/index.js migrate deploy --schema schema.prisma'

build_args=(--network host --progress plain \
  --build-arg NEXT_PUBLIC_BLOBSCAN_RELEASE=1.40.3-chiado-configurable \
  --build-arg "GIT_COMMIT=$revision" \
  --build-arg "DATABASE_URL=$build_url" --build-arg "DIRECT_URL=$build_url")
for target in web api; do
  docker build "${build_args[@]}" --target "$target" \
    -t "eez-blobscan-$target:1.40.3-chiado-configurable" "$work_dir/source"
done
docker build "${build_args[@]}" --target web-builder \
  -t eez-blobscan-build-check:chiado "$work_dir/source"
docker run --rm --network none \
  -e SKIP_ENV_VALIDATION= -e NODE_ENV=production \
  -e SECRET_KEY=profile-test-secret \
  -e DATABASE_URL=postgresql://test:test@127.0.0.1/test \
  -e DIRECT_URL=postgresql://test:test@127.0.0.1/test \
  -e STATS_SYNCER_LOWEST_SLOT=30000000 \
  -e PUBLIC_NETWORK_NAME=chiado -e PUBLIC_BLOBSCAN_START_BLOCK=25000000 \
  --mount "type=bind,src=$script_dir/check-chiado-profile.ts,dst=/app/check-chiado-profile.ts,readonly" \
  eez-blobscan-build-check:chiado pnpm exec tsx /app/check-chiado-profile.ts
docker run --rm --network none \
  -e SKIP_ENV_VALIDATION= -e NODE_ENV=production -e CHAIN_ID=10200 \
  -e PUBLIC_NETWORK_NAME=chiado -e BLOBSCAN_API_BASE_URL=http://blobscan-api:3001 \
  -e DATABASE_URL=postgresql://test:test@127.0.0.1/test \
  -e DIRECT_URL=postgresql://test:test@127.0.0.1/test \
  --mount "type=bind,src=$script_dir/check-blob-proxy.ts,dst=/app/check-blob-proxy.ts,readonly" \
  eez-blobscan-build-check:chiado pnpm exec tsx \
  --tsconfig /app/apps/web/tsconfig.json /app/check-blob-proxy.ts
