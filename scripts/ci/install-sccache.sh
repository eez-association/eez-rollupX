#!/usr/bin/env bash
# A static binary works both on hosted Ubuntu and inside the Bookworm builder.
# Callers treat a failed download as an uncached build, not a CI dependency.
set -euo pipefail
destination="${1:?destination directory}"
mkdir -p "$destination"
download="$(mktemp -d "${TMPDIR:-/tmp}/eez-sccache-download.XXXXXXXX")"
archive=sccache-v0.17.0-x86_64-unknown-linux-musl
curl --fail --silent --show-error --location --retry 2 --connect-timeout 10 --max-time 90 \
    "https://github.com/mozilla/sccache/releases/download/v0.17.0/$archive.tar.gz" \
    -o "$download/sccache.tar.gz"
printf '%s  %s\n' 67c4a96dd237c1f518f6b36083f270f9976d516f1e57fce891755ea782e50006 \
    "$download/sccache.tar.gz" | sha256sum --check --status
tar -xzf "$download/sccache.tar.gz" -C "$destination" --strip-components=1 "$archive/sccache"
chmod 755 "$destination/sccache"
