#!/usr/bin/env bash
set -euo pipefail

# Replay a captured eth_sendBundle request against one relay. Version 1
# captures recorded the single relay URL; version 2 captures list per-relay
# outcomes by position and host only (URLs may carry credentials), so pass the
# relay URL explicitly.

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

if [[ $# -eq 2 ]]; then
  builder_rpc=$2
elif ! builder_rpc=$(jq -er '.builderRpcUrl' "$metadata_file"); then
  echo "capture records no relay URL (metadata version 2+); pass BUILDER_RPC_URL" >&2
  jq -c '.builders // []' "$metadata_file" >&2
  exit 2
fi
curl --fail-with-body --silent --show-error \
  -H 'content-type: application/json' \
  --data-binary "@$request_file" \
  "$builder_rpc" | jq .
