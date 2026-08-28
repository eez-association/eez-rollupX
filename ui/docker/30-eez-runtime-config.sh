#!/bin/sh
set -eu

test -f /out/deployments.env
set -a
# shellcheck disable=SC1091
. /out/deployments.env
if [ -f /demo/demo.env ]; then
    . /demo/demo.env
fi
set +a

: "${EEZ_UI_DEMO_ENABLED:=false}"
: "${EEZ_UI_DEMO_ACCOUNT_ADDRESS:=}"
: "${EEZ_UI_DEMO_PRIVATE_KEY:=}"
: "${EEZ_UI_DEMO_BRIDGE_ADDRESS:=}"
: "${EEZ_UI_DEMO_TOKEN_ADDRESS:=}"
: "${EEZ_UI_DEMO_POOL_ADDRESS:=}"
: "${EEZ_UI_DEMO_EXECUTOR_L1:=}"
: "${EEZ_UI_DEMO_EXECUTOR_L2:=}"
: "${EEZ_UI_DEMO_WRAPPED_TOKEN_L2:=}"
: "${EEZ_UI_DEMO_NFT_L2:=}"
export EEZ_UI_DEMO_ENABLED EEZ_UI_DEMO_ACCOUNT_ADDRESS EEZ_UI_DEMO_PRIVATE_KEY
export EEZ_UI_DEMO_BRIDGE_ADDRESS EEZ_UI_DEMO_TOKEN_ADDRESS EEZ_UI_DEMO_POOL_ADDRESS
export EEZ_UI_DEMO_EXECUTOR_L1 EEZ_UI_DEMO_EXECUTOR_L2
export EEZ_UI_DEMO_WRAPPED_TOKEN_L2 EEZ_UI_DEMO_NFT_L2

envsubst < /opt/eez/config.template.json > /usr/share/nginx/html/config.json
