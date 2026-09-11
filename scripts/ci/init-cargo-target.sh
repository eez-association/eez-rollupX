#!/usr/bin/env bash
# Create a fresh Cargo target outside RUNNER_TEMP. Some ARC installations share
# that mount, allowing another runner's lifecycle cleanup to remove active files.
set -euo pipefail

: "${GITHUB_ENV:?GITHUB_ENV must be set by GitHub Actions}"
: "${GITHUB_WORKSPACE:?GITHUB_WORKSPACE must be set by GitHub Actions}"

CARGO_TARGET_DIR="$(mktemp -d "$GITHUB_WORKSPACE/.cargo-target.XXXXXXXXXX")"
export CARGO_TARGET_DIR
printf 'CARGO_TARGET_DIR=%s\n' "$CARGO_TARGET_DIR" >> "$GITHUB_ENV"
printf 'Initialized CARGO_TARGET_DIR=%s\n' "$CARGO_TARGET_DIR"
