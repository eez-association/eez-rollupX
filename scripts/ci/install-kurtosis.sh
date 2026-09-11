#!/usr/bin/env bash
set -euo pipefail
echo "deb [trusted=yes] https://sdk.kurtosis.com/kurtosis-cli-release-artifacts/ /" \
    | sudo tee /etc/apt/sources.list.d/kurtosis.list
sudo apt-get update
sudo apt-get install -y bc kurtosis-cli jq openssl python3 util-linux
kurtosis engine start
