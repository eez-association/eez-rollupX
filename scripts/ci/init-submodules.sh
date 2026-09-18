#!/usr/bin/env bash
# Retry transient transport failures without changing the pinned gitlinks.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"
for attempt in 1 2 3; do
    if git -c protocol.version=2 submodule update --init --recursive --depth=1 --checkout; then
        exit 0
    fi
    if [[ "$attempt" -lt 3 ]]; then
        echo "::warning::Pinned submodule fetch failed (attempt $attempt/3); retrying"
        sleep "$((attempt * 5))"
    fi
done
echo "::error::Pinned submodule checkout failed after three attempts" >&2
exit 1
