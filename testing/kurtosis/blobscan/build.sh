#!/usr/bin/env bash
# Build chain-specific Blobscan API and web images from a pinned upstream tree.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BLOBSCAN_REPOSITORY="https://github.com/Blobscan/blobscan.git"
BLOBSCAN_COMMIT="424d5e512d57ff4ac42509ef93f3845ead392155"
BLOBSCAN_VERSION="1.40.3"
BUILD_POSTGRES_IMAGE="postgres:alpine@sha256:d3e1620b530c944afa6e887d22eb899824da68e19c52024bf98f5220c88a65b2"
BUILD_MIGRATOR_IMAGE="blossomlabs/blobscan-api@sha256:2edefdf2c959e6d0b574953e17a5a522135a8bf53931ab5bbef4707f6e7dd32c"

CHAIN_ID="${1:?usage: build.sh CHAIN_ID API_IMAGE WEB_IMAGE}"
API_IMAGE="${2:?usage: build.sh CHAIN_ID API_IMAGE WEB_IMAGE}"
WEB_IMAGE="${3:?usage: build.sh CHAIN_ID API_IMAGE WEB_IMAGE}"
BUILD_COMPONENT="${EEZ_BLOBSCAN_BUILD_COMPONENT:-all}"

for tool in docker git rg shuf ss; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done
if [[ ! "$CHAIN_ID" =~ ^[1-9][0-9]*$ ]]; then
    echo "invalid Blobscan chain ID: $CHAIN_ID" >&2
    exit 1
fi
case "$BUILD_COMPONENT" in
    all|api|web) ;;
    *)
        echo "EEZ_BLOBSCAN_BUILD_COMPONENT must be all, api, or web" >&2
        exit 1
        ;;
esac

BUILD_ROOT="$(mktemp -d -t eez-blobscan-build.XXXXXX)"
BUILD_DB_CONTAINER=""
cleanup() {
    if [[ -n "$BUILD_DB_CONTAINER" ]]; then
        docker stop "$BUILD_DB_CONTAINER" >/dev/null 2>&1 || true
    fi
    rm -rf -- "$BUILD_ROOT"
}
trap cleanup EXIT

SOURCE_DIR="$BUILD_ROOT/blobscan"
echo "==> fetching Blobscan $BLOBSCAN_VERSION ($BLOBSCAN_COMMIT)"
git clone --filter=blob:none --no-checkout "$BLOBSCAN_REPOSITORY" "$SOURCE_DIR"
git -C "$SOURCE_DIR" fetch --depth 1 origin "$BLOBSCAN_COMMIT"
git -C "$SOURCE_DIR" checkout --detach "$BLOBSCAN_COMMIT"

ACTUAL_COMMIT="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
if [[ "$ACTUAL_COMMIT" != "$BLOBSCAN_COMMIT" ]]; then
    echo "Blobscan source mismatch: expected $BLOBSCAN_COMMIT, got $ACTUAL_COMMIT" >&2
    exit 1
fi

git -C "$SOURCE_DIR" apply --check "$HERE/eez-devnet.patch"
git -C "$SOURCE_DIR" apply "$HERE/eez-devnet.patch"
sed -i "s/__EEZ_BLOBSCAN_CHAIN_ID__/$CHAIN_ID/g" \
    "$SOURCE_DIR/packages/chains/src/chains/devnet.ts"
if rg -n '__EEZ_BLOBSCAN_CHAIN_ID__' "$SOURCE_DIR" >/dev/null; then
    echo "Blobscan chain-ID placeholder was not fully replaced" >&2
    exit 1
fi

# Prisma's typed-SQL generator introspects a live database during the upstream
# image build. Bind a disposable PostgreSQL instance only to host loopback and
# let BuildKit reach it through its supported host-network mode.
BUILD_DB_PORT=""
for _ in $(seq 1 60); do
    candidate_port="$(shuf --input-range=20000-60000 --head-count=1)"
    if ! ss --tcp --listening --numeric | awk '{print $4}' | grep -Eq ":${candidate_port}$"; then
        BUILD_DB_PORT="$candidate_port"
        break
    fi
done
if [[ -z "$BUILD_DB_PORT" ]]; then
    echo "could not reserve a free loopback port for the Blobscan build database" >&2
    exit 1
fi
BUILD_DB_CONTAINER="$(
    docker run --detach --rm \
        --network host \
        --env POSTGRES_USER=blobscan \
        --env POSTGRES_PASSWORD=blobscan \
        --env POSTGRES_DB=blobscan \
        "$BUILD_POSTGRES_IMAGE" \
        -c "listen_addresses=127.0.0.1" \
        -c "port=$BUILD_DB_PORT"
)"
for _ in $(seq 1 60); do
    if docker exec "$BUILD_DB_CONTAINER" pg_isready --host 127.0.0.1 --port "$BUILD_DB_PORT" --username blobscan --dbname blobscan >/dev/null 2>&1; then
        break
    fi
    sleep 1
done
if ! docker exec "$BUILD_DB_CONTAINER" pg_isready --host 127.0.0.1 --port "$BUILD_DB_PORT" --username blobscan --dbname blobscan >/dev/null 2>&1; then
    echo "temporary Blobscan build database did not become ready" >&2
    exit 1
fi
BUILD_DATABASE_URL="postgresql://blobscan:blobscan@127.0.0.1:$BUILD_DB_PORT/blobscan"

echo "==> preparing Blobscan build schema"
docker run --rm \
    --network host \
    --env "DATABASE_URL=$BUILD_DATABASE_URL" \
    --env "DIRECT_URL=$BUILD_DATABASE_URL" \
    --entrypoint /bin/sh \
    "$BUILD_MIGRATOR_IMAGE" \
    -c './node_modules/prisma/build/index.js migrate deploy --schema schema.prisma'

build_args=(
    --build-arg "BUILD_TIMESTAMP=1970-01-01T00:00:00Z"
    --build-arg "GIT_COMMIT=$BLOBSCAN_COMMIT-eez-chain-$CHAIN_ID"
    --build-arg "NEXT_PUBLIC_BLOBSCAN_RELEASE=$BLOBSCAN_VERSION-eez-chain-$CHAIN_ID"
    --build-arg "DATABASE_URL=$BUILD_DATABASE_URL"
    --build-arg "DIRECT_URL=$BUILD_DATABASE_URL"
)

if [[ "$BUILD_COMPONENT" != "web" ]]; then
    echo "==> building $API_IMAGE"
    docker build \
        --network host \
        "${build_args[@]}" \
        --target api \
        -t "$API_IMAGE" \
        "$SOURCE_DIR"
fi

if [[ "$BUILD_COMPONENT" != "api" ]]; then
    echo "==> building $WEB_IMAGE"
    docker build \
        --network host \
        "${build_args[@]}" \
        --target web \
        -t "$WEB_IMAGE" \
        "$SOURCE_DIR"
fi

echo "==> Blobscan images ready for chain $CHAIN_ID"
