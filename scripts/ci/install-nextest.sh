#!/usr/bin/env bash
# Same pinned executable in the Bookworm producer and Ubuntu/ARC consumers.
# Do not install into the runner's shared CARGO_HOME or fall back to compilation.
set -euo pipefail
destination="${1:?destination directory}"
[[ "$(uname -s)/$(uname -m)" == Linux/x86_64 ]] || { echo 'nextest requires Linux amd64' >&2; exit 1; }
mkdir -p "$destination"
download="$(mktemp -d "${TMPDIR:-/tmp}/eez-nextest-download.XXXXXXXX")"
curl --fail --silent --show-error --location --retry 2 --connect-timeout 10 --max-time 90 \
    https://get.nexte.st/0.9.143/linux -o "$download/nextest.tar.gz"
# SHA-256 of the official 0.9.143 x86_64 Linux GNU release.
printf '%s  %s\n' 66786b9abe23920d022a182d1416b1bbc8130dd4872a9553d76985a1708dcd1e \
    "$download/nextest.tar.gz" | sha256sum --check --status
tar -xzf "$download/nextest.tar.gz" -C "$destination" cargo-nextest
chmod 755 "$destination/cargo-nextest"
"$destination/cargo-nextest" nextest --version
