#!/usr/bin/env bash
# Authorized shared-Chiado workload; see docs/runbooks/chiado-mixed-validation.md.
set -euo pipefail
set +x
umask 077
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
: "${STRESS_DEPLOYMENT_ROOT:?Select the existing deployment checkout}"
: "${EEZ_MIXED_OUTPUT_DIR:?Select a new private evidence directory}"
: "${EEZ_TEST_FUND_AMOUNT_ETH:?Explicit test actor funding is required}"
: "${STRESS_NODE_IMAGE_ID:?}"
: "${STRESS_SIGNER_IMAGE_ID:?}"
: "${STRESS_POSTER_ADDRESS:?}"
: "${STRESS_SOURCE_REVISION:?}"
: "${STRESS_FOLLOWER_CONTAINER:?}"
: "${STRESS_FOLLOWER_IMAGE:?}"
: "${STRESS_SENDER_ADDRESS:?}"
: "${STRESS_L2_CHAIN_ID:?}"
export RAYON_NUM_THREADS=2 TOKIO_WORKER_THREADS=2
OUT="$EEZ_MIXED_OUTPUT_DIR"
python3 -E -c 'import os; from scripts.settlement_evidence import check_funding; check_funding(os.environ["EEZ_TEST_FUND_AMOUNT_ETH"])'
mkdir "$OUT"
# Separate worktrees still share the same funder. Coordinate both known locks.
exec 200>"$STRESS_DEPLOYMENT_ROOT/eez-core-protocol/tmp/.eez-network-e2e.lock"
flock -n 200
if [[ "$REPO" != "$STRESS_DEPLOYMENT_ROOT" ]]; then
    exec 201>"$REPO/eez-core-protocol/tmp/.eez-network-e2e.lock"
    flock -n 201
fi
wave_pid='' observer_pid=''
STARTED=$(date -u +%FT%TZ)
finish() {
    local rc=$?
    trap - EXIT
    if [[ -n "$wave_pid" ]] && kill -0 "$wave_pid" 2>/dev/null; then
        kill -TERM -- "-$wave_pid" 2>/dev/null || true
        wait "$wave_pid" || true
    fi
    if [[ ! -f "$OUT/live-mixed.exit" ]]; then printf '%s\n' "$rc" >"$OUT/live-mixed.exit"; fi
    printf '%s\n' "$rc" >"$OUT/run.exit"
    if [[ -n "$observer_pid" ]]; then wait "$observer_pid" || true; fi
    docker logs --since "$STARTED" eez-node-chiado >"$OUT/live-node.log" 2>&1 || true
    docker logs --since "$STARTED" eez-proof-signer-chiado >"$OUT/live-signer.log" 2>&1 || true
}
trap finish EXIT
set -a
source "$STRESS_DEPLOYMENT_ROOT/eez-core-protocol/chain.env"
source "$STRESS_DEPLOYMENT_ROOT/deployments.env"
set +a
export EEZ_USE_LOCAL_DEPLOYMENTS=1 EEZ_DEPLOYMENTS_FILE="$STRESS_DEPLOYMENT_ROOT/deployments.env"
export EEZ_DEVNET_L1_RPC=http://127.0.0.1:18645 EEZ_DEVNET_L2_RPC=http://127.0.0.1:18688
export EEZ_DEVNET_L1_FRONT=http://127.0.0.1:18999 EEZ_DEVNET_L2_FRONT=http://127.0.0.1:18998
export L1="$EEZ_DEVNET_L1_RPC" L2="$EEZ_DEVNET_L2_RPC" L1F="$EEZ_DEVNET_L1_FRONT" L2F="$EEZ_DEVNET_L2_FRONT"
export EEZ_NODE_CONTAINER=eez-node-chiado EEZ_SIGNER_CONTAINER=eez-proof-signer-chiado
export EEZ_FUND_FROM_KEY="$SOURCE_PK" EEZ_L1_SETUP_KEY="$SOURCE_PK" EEZ_L2_SETUP_KEY="$SOURCE_PK"
export EEZ_WAVE_MODE=mixed EEZ_WAVE_COUNT=30 EEZ_WAVE_GAP_SECS=30 EEZ_INCLUDE_REVERTS=0
export EEZ_NODE_LOG="$OUT/waves-node.log" EEZ_PROOF_SIGNER_LOG="$OUT/waves-signer.log"
export STRESS_OUTPUT_DIR="$OUT/live-mixed-ordinary" STRESS_DURATION_SECONDS=900
# Profiles can override inherited variables; validate the effective funding too.
python3 -E -c 'import os; from scripts.settlement_evidence import check_funding; check_funding(os.environ["EEZ_TEST_FUND_AMOUNT_ETH"])'
python3 -E - <<'PY'
import json, os, subprocess
from pathlib import Path
from scripts.settlement_evidence import check_funding, cast_with_key
node, signer, follower = json.loads(subprocess.check_output([
    'docker', 'inspect', 'eez-node-chiado', 'eez-proof-signer-chiado', os.environ['STRESS_FOLLOWER_CONTAINER']]))
assert all(c['State']['Running'] for c in (node, signer, follower))
assert node['Image'] == os.environ['STRESS_NODE_IMAGE_ID']
assert signer['Image'] == os.environ['STRESS_SIGNER_IMAGE_ID']
assert node['Config']['Labels']['org.opencontainers.image.revision'] == os.environ['STRESS_SOURCE_REVISION']
protocol = subprocess.check_output(['git', 'rev-parse', 'HEAD:eez-core-protocol'], text=True).strip()
assert all(c['Config']['Labels']['io.eez.protocol.revision'] == protocol for c in (node, signer))
assert signer['Config']['Labels']['org.opencontainers.image.revision'] == os.environ['STRESS_SOURCE_REVISION']
assert follower['Config']['Image'] == os.environ['STRESS_FOLLOWER_IMAGE']
assert '@sha256:' in os.environ['STRESS_FOLLOWER_IMAGE'], 'Pin follower by digest'
assert follower['Config']['Entrypoint'] == ['/usr/local/bin/eez-follower']
assert not any(v.startswith(('EEZ_SEQUENCER_RPC=', 'EEZ_L1_POSTER_KEY=', 'EEZ_PROOF_SIGNER_KEY='))
               for v in follower['Config']['Env'])
args = follower['Config']['Cmd']
assert not any(a.startswith('--sequencer-rpc') for a in args)
assert '--disable-discovery' in args
for flag in ('--max-outbound-peers', '--max-inbound-peers'):
    assert args[args.index(flag)+1] == '0'
assert os.environ['STRESS_POSTER_ADDRESS'].lower() != os.environ['STRESS_SENDER_ADDRESS'].lower()
sender = cast_with_key(['wallet','address'], os.environ['SOURCE_PK'])
assert sender.lower() == os.environ['STRESS_SENDER_ADDRESS'].lower()
funding = check_funding(os.environ['EEZ_TEST_FUND_AMOUNT_ETH'])
fee = int(os.environ.get('STRESS_MAX_FEE_WEI', '100000000'))
assert 10_000_000 <= fee <= 100_000_000
for port, reserve in ((18645, funding), (18688, funding + 4500*21000*fee)):
    balance = int(subprocess.check_output(['cast','balance',sender,'--rpc-url',f'http://127.0.0.1:{port}']))
    assert balance > reserve, 'Source funds insufficient even before setup gas'
for port, expected in ((18645, 10200), (18688, int(os.environ['STRESS_L2_CHAIN_ID'])),
                       (19288, int(os.environ['STRESS_L2_CHAIN_ID']))):
    assert int(subprocess.check_output(['cast','chain-id','--rpc-url',f'http://127.0.0.1:{port}']), 10) == expected
Path(os.environ['EEZ_MIXED_OUTPUT_DIR'], 'identity.json').write_text(json.dumps([
    dict(name=c['Name'], id=c['Id'], image=c['Image'], started=c['State']['StartedAt'],
         restarts=c['RestartCount'], labels=c['Config']['Labels']) for c in (node, signer, follower)], indent=2))
PY
cast rpc eez_getCrossChainQueueStatus --rpc-url "$L2F" \
    | python3 -E -c 'import json,sys; from scripts.settlement_evidence import check_queue; check_queue(json.load(sys.stdin), idle=True)'
jq -n --argjson l1 "$(cast rpc eth_getBlockByNumber latest false --rpc-url "$L1")" \
    --argjson l2 "$(cast rpc eth_getBlockByNumber latest false --rpc-url "$L2")" \
    '{l1:$l1,l2:$l2}' >"$OUT/initial-blocks.json"
# Setup consumes the ordinary sender's nonce. Start ordinary load only after
# funding/deployment ends; mixed (not mixed-pure) never uses it for filler.
setsid timeout --foreground 2700s bash testing/kurtosis/scripts/cross-chain-wave.sh >"$OUT/live-waves.log" 2>&1 &
wave_pid=$!
python3 -E scripts/observe-chiado-mixed-load.py --evidence "$OUT" >"$OUT/observer.log" 2>&1 &
observer_pid=$!
deadline=$((SECONDS+900))
until grep -q '==> firing 30 wave(s)' "$OUT/live-waves.log"; do
    kill -0 "$wave_pid" 2>/dev/null || { wait "$wave_pid"; exit 1; }
    (( SECONDS < deadline )) || { echo 'Wave setup timed out'; exit 1; }
    sleep 1
done
echo 'Mixed waves running; starting 900 seconds of ordinary load at 5 tx/s.'
python3 -E -u scripts/chiado-ordinary-stress.py >"$OUT/live-ordinary.log" 2>&1
wait "$wave_pid"
wave_pid=''
# This records only the original workload outcome. Independent verification
# has its own exit marker so failures can never overwrite the workload result.
printf '0\n' >"$OUT/live-mixed.exit"
wait "$observer_pid"
observer_pid=''
unset SOURCE_PK EEZ_FUND_FROM_KEY EEZ_L1_SETUP_KEY EEZ_L2_SETUP_KEY
verification_rc=0
python3 -E scripts/verify-chiado-mixed-load.py --evidence "$OUT" --waves 30 \
    >"$OUT/independent-verification.log" 2>&1 || verification_rc=$?
printf '%s\n' "$verification_rc" >"$OUT/independent-verification.exit"
if (( verification_rc != 0 )); then
    echo 'Independent verification failed; preserve all evidence.'
    exit "$verification_rc"
fi
echo 'Complete mixed workload and independent verification passed.'
