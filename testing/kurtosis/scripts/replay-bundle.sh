#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 CAPTURE_DIRECTORY [BUILDER_RPC_URL]" >&2
  exit 2
fi

capture_dir=$1
request_file="$capture_dir/request.json"
metadata_file="$capture_dir/metadata.json"

if [[ ! -f "$request_file" || ! -f "$metadata_file" ]]; then
  echo "capture must contain request.json and metadata.json: $capture_dir" >&2
  exit 2
fi

builder_rpc=${2:-$(jq -er '.builderRpcUrl' "$metadata_file")}
curl --fail-with-body --silent --show-error \
  -H 'content-type: application/json' \
  --data-binary "@$request_file" \
  "$builder_rpc" | jq .
