#!/usr/bin/env bash
# Reclaim only preinstalled, unused SDKs on disposable GitHub-hosted runners.
# Never prune Docker or delete Cargo outputs; another build may be using them.
set -euo pipefail

if [[ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]]; then
    echo "Skipping hosted SDK cleanup on ${RUNNER_ENVIRONMENT:-an unknown runner}"
    exit 0
fi

df -h .
sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup
df -h .
free -h
