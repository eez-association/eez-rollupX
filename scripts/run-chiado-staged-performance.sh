#!/usr/bin/env bash
# Run the staged workload on an already deployed Chiado rollup.
set -euo pipefail
set +x
umask 077

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROTOCOL="$ROOT/eez-core-protocol"
CHECK_ONLY=false
VERIFY=false
while (($#)); do
case "$1" in
    --check) CHECK_ONLY=true; shift ;;
    --verify) VERIFY=true; shift ;;
    --help|-h)
        cat <<'HELP'
Usage: bash scripts/run-chiado-staged-performance.sh [--check] [--verify]

Runs the 14 requested targets (770 jobs / 870 source transactions by default) with
--no-verify by default. --verify enables per-job semantic assertions after mining.
--check only checks readiness; it does not fund, build or send.
DEVNET_ENV defaults to eez-core-protocol/chain.env. The profile must contain
the deployed RPCs, ingress URLs, registry, manager and a funded SOURCE_PK.

Defaults: PREPARE_PARALLEL=200, FUND_ETH=0.005, FLOOR_ETH=0.005,
SEND_WORKERS=28, EEZ_STAGED_L2_CHAIN_ID=696990.
EEZ_STAGED_SCALE multiplies every target count (default 1; 4 gives 3,080 jobs
and 3,480 source transactions).
Environment overrides are passed through the existing profile and runner.
The full run locks the shared worker pool. Do not launch the upstream runner
directly while this helper is active; upstream only locks its funding step.
Only fresh runs are supported. Saved RPC URLs do not identify a network after
replacement; this helper does not resume or resend old signed transactions.
HELP
        exit 0 ;;
    *) echo "Unknown argument: $1 (use --help)" >&2; exit 2 ;;
esac
done
for tool in python3 cast forge curl jq bc flock timeout; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

export DEVNET_ENV="${DEVNET_ENV:-$PROTOCOL/chain.env}"
[[ -f "$DEVNET_ENV" ]] || { echo "Missing DEVNET_ENV profile: $DEVNET_ENV" >&2; exit 1; }
DEVNET_ENV="$(realpath "$DEVNET_ENV")"
export PREPARE_PARALLEL="${PREPARE_PARALLEL:-200}"
export FUND_ETH="${FUND_ETH:-0.005}" FLOOR_ETH="${FLOOR_ETH:-0.005}"
export SEND_WORKERS="${SEND_WORKERS:-28}"
export RAYON_NUM_THREADS="${RAYON_NUM_THREADS:-2}" TOKIO_WORKER_THREADS="${TOKIO_WORKER_THREADS:-2}"
export EEZ_STAGED_L2_CHAIN_ID="${EEZ_STAGED_L2_CHAIN_ID:-696990}"
export EEZ_STAGED_SCALE="${EEZ_STAGED_SCALE:-1}"
# Same working directory and profile precedence as the upstream entrypoint.
cd "$PROTOCOL"
set -a
# shellcheck disable=SC1090
source "$DEVNET_ENV"
set +a
set +x
# Publish the selected CLI mode for the read-only preflight summary.
export EEZ_STAGED_VERIFY="$VERIFY"
# Validate the effective profile value before deriving keys, locking wallets or
# querying the network. Avoid arithmetic on an unbounded shell integer.
[[ "$EEZ_STAGED_SCALE" =~ ^([1-9]|10)$ ]] || {
    echo "EEZ_STAGED_SCALE must be an integer from 1 to 10" >&2; exit 2;
}
: "${SOURCE_PK:?SOURCE_PK must be configured explicitly in DEVNET_ENV}"
export SOURCE_ADDRESS
SOURCE_ADDRESS="$(cast wallet address --private-key "$SOURCE_PK" 2>/dev/null)" || {
    echo "Could not derive the funding address from the configured SOURCE_PK." >&2; exit 1;
}
# The runner sources its profile again; the preflight needs only public values.
unset SOURCE_PK
mkdir -p tmp
# FD 200 is distinct from the upstream faucet lock's FD 9. It stays open in
# the upstream process and its workers until this complete run has exited.
exec 200>tmp/.eez-network-e2e.lock
flock -n 200 || { echo "Another run holds the shared worker-pool lock." >&2; exit 1; }

python3 - <<'PY'
import csv
from decimal import Decimal, InvalidOperation
import json
import os
from pathlib import Path
import re
import sys
import time
import urllib.request


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def rpc_batch(endpoint, queries):
    payload = [{'jsonrpc': '2.0', 'id': i, 'method': m, 'params': p}
               for i, (m, p) in enumerate(queries)]
    request = urllib.request.Request(endpoint, json.dumps(payload).encode(),
                                    {'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            rows = json.load(response)
    except Exception as error:
        raise RuntimeError('RPC unavailable: ' + type(error).__name__) from None
    require(isinstance(rows, list) and len(rows) == len(payload), 'Incomplete RPC batch')
    by_id = {row['id']: row for row in rows}
    require(set(by_id) == set(range(len(payload))), 'Mismatched RPC response IDs')
    require(all('result' in row and 'error' not in row for row in rows), 'RPC rejected a readiness query')
    return [by_id[i]['result'] for i in range(len(payload))]


def rpc(endpoint, method, params=None):
    return rpc_batch(endpoint, [(method, params or [])])[0]


def tokens(value):
    return format(Decimal(value) / 10**18, 'f')


def funding_requirement(balances, target, floor, chunk):
    # Match the pinned orchestrator's _plan_funding / _topup_faucet:
    # only deficient workers and nonempty chunks consume funding reserves.
    missing = [target - balance for balance in balances
               if balance < floor and balance < target]
    if not missing:
        return 0, 0
    return (sum(missing) + ((len(missing) + chunk - 1) // chunk) * 5 * 10**16
            + 10**17), len(missing)


def check():
    for name in ['L1_RPC', 'L1_FRONT', 'L2_RPC', 'L2_FRONT', 'ROLLUPS', 'MANAGER_L2']:
        require(os.environ.get(name), 'Missing profile setting: ' + name)
    for name in ['ROLLUPS', 'MANAGER_L2', 'SOURCE_ADDRESS']:
        require(re.fullmatch(r'0x[0-9a-fA-F]{40}', os.environ[name]), 'Invalid address: ' + name)
    for name in ['PREPARE_PARALLEL', 'SEND_WORKERS', 'EEZ_STAGED_L2_CHAIN_ID']:
        require(re.fullmatch(r'[1-9][0-9]*', os.environ[name]), 'Expected a positive integer: ' + name)
    for process in Path('/proc').glob('[0-9]*/cmdline'):
        try:
            args = process.read_bytes().split(b'\0')
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
        require(not any(Path(os.fsdecode(arg)).as_posix().endswith(
                            ('run/network/staged.sh', 'run/network/parallel.sh'))
                        for arg in args if arg),
                'An upstream network runner is already active; wait for it to exit')
    amount, floor = Decimal(os.environ['FUND_ETH']), Decimal(os.environ['FLOOR_ETH'])
    require(amount.is_finite() and floor.is_finite() and 0 < floor <= amount,
            'Require 0 < FLOOR_ETH <= FUND_ETH')
    require(amount * 10**18 == int(amount * 10**18), 'FUND_ETH has more than 18 decimal places')
    chunk = int(os.environ.get('MULTISEND_BATCH', '100'))
    require(chunk > 0, 'MULTISEND_BATCH must be positive')
    scale = int(os.environ['EEZ_STAGED_SCALE'])
    jobs = 770 * scale
    faucet_text = Path('script/e2e/run/faucet.txt').read_text()
    match = re.search(r'^address:\s*(0x[0-9a-fA-F]{40})\s*$', faucet_text, re.M)
    require(match, 'The existing faucet file is missing its public address')
    faucet, source = match[1], os.environ['SOURCE_ADDRESS']
    with Path('script/e2e/run/wallet-pool.csv').open() as stream:
        workers = [row[0] for row in csv.reader(stream)][:jobs]
    require(len(workers) == jobs and len({a.lower() for a in workers}) == jobs
            and all(re.fullmatch(r'0x[0-9a-fA-F]{40}', a) for a in workers),
            f'This helper requires {jobs} existing unique worker wallets')
    # The extra source reserve is an estimate, not a cost cap. Worker balances
    # are read per chain under the shared lock, as in the pinned runner.
    reserve = 10**16
    queue = rpc(os.environ['L2_FRONT'], 'eez_getCrossChainQueueStatus')
    require(all(queue.get(k) == 0 for k in ['queued', 'inFlight', 'blocked', 'unknown']),
            'Cross-chain queue is not empty; let current work settle first')
    ready = True
    print(f'Profile: {os.environ["DEVNET_ENV"]}')
    print(f'Workload: {jobs} jobs / {870 * scale} sources; preparation={os.environ["PREPARE_PARALLEL"]}, '
          f'send workers={os.environ["SEND_WORKERS"]}; semantic verifier '
          + ('enabled' if os.environ['EEZ_STAGED_VERIFY'] == 'true' else 'disabled'))
    print(f'Funding source: {source}; faucet: {faucet}')
    for layer, chain_id, contract in [('L1', 10200, 'ROLLUPS'),
                                      ('L2', int(os.environ['EEZ_STAGED_L2_CHAIN_ID']), 'MANAGER_L2')]:
        endpoint, front = os.environ[layer + '_RPC'], os.environ[layer + '_FRONT']
        require(int(rpc(endpoint, 'eth_chainId'), 16) == chain_id
                and int(rpc(front, 'eth_chainId'), 16) == chain_id, layer + ' RPC/ingress chain mismatch')
        require(rpc(endpoint, 'eth_syncing') is False, layer + ' is still syncing')
        head = rpc(endpoint, 'eth_getBlockByNumber', ['latest', False])
        require(head and abs(time.time() - int(head['timestamp'], 16)) <= 60,
                layer + ' head is stale or the local clock is incorrect')
        require(rpc(endpoint, 'eth_getCode', [os.environ[contract], 'latest']) not in [None, '0x', '0x0'],
                layer + ' configured contract has no deployed code')
        if layer == 'L2':
            safe = rpc(endpoint, 'eth_getBlockByNumber', ['safe', False])
            require(safe is not None, 'L2 has no safe endpoint')
            print(f'L2 safe={int(safe["number"], 16)}, latest={int(head["number"], 16)} '
                  '(setup history may still be unsettled when the measured wave starts)')
        addresses = workers + [source, faucet]
        balances = []
        for offset in range(0, len(addresses), 100):
            group = addresses[offset:offset + 100]
            latest = rpc_batch(endpoint, [('eth_getTransactionCount', [a, 'latest']) for a in group])
            pending = rpc_batch(endpoint, [('eth_getTransactionCount', [a, 'pending']) for a in group])
            ingress = rpc_batch(front, [('eth_getTransactionCount', [a, 'pending']) for a in group])
            require(latest == pending == ingress, layer + ' has pending or mismatched worker/funding nonces')
            balances.extend(int(v, 16) for v in rpc_batch(endpoint,
                [('eth_getBalance', [a, 'latest']) for a in group]))
        source_balance, faucet_balance = balances[-2:]
        required, deficient = funding_requirement(balances[:-2], int(amount * 10**18),
                                                 int(floor * 10**18), chunk)
        topup = max(0, required - faucet_balance)
        shortfall = max(0, topup + reserve - source_balance) if topup else 0
        print(f'{layer}: {deficient}/{jobs} workers need funding; '
              f'upfront faucet requirement={tokens(required)} xDAI (includes gas/deployment reserve)')
        print(f'{layer} chain {chain_id}: source={tokens(source_balance)}, faucet={tokens(faucet_balance)}, '
              f'additional source funding needed={tokens(shortfall)} xDAI (includes 0.01 gas reserve)')
        ready = ready and shortfall == 0
    require(ready, 'Funding is insufficient; top up the funding source on the indicated layer and recheck')
    print('Readiness passed: queue empty, worker nonces aligned, funding available.')


try:
    check()
except (RuntimeError, OSError, ValueError, InvalidOperation) as error:
    print('Preflight failed: ' + str(error), file=sys.stderr)
    sys.exit(1)
PY

$CHECK_ONLY && exit 0
# Larger packed batches can exceed Linux's single-argument limit when the pinned
# verifier passes hex calldata to Forge. Preserve its checks through file input.
source "$ROOT/scripts/chiado-forge.sh"
echo "Starting upstream staged runner; its run directory contains timings.csv and job-timings.csv."
verify_args=(--no-verify)
$VERIFY && verify_args=()
exec bash script/e2e/run/network/staged.sh "${verify_args[@]}" \
    "counter:$((100 * EEZ_STAGED_SCALE))" "counterL2:$((100 * EEZ_STAGED_SCALE))" \
    "nestedCounter:$((50 * EEZ_STAGED_SCALE))" "nestedCounterL2:$((50 * EEZ_STAGED_SCALE))" \
    "multi-call-nested:$((50 * EEZ_STAGED_SCALE))" "multi-call-nestedL2:$((50 * EEZ_STAGED_SCALE))" \
    "multi-call-twice:$((50 * EEZ_STAGED_SCALE))" "multi-call-twiceL2:$((50 * EEZ_STAGED_SCALE))" \
    "counter-multi-tx:$((50 * EEZ_STAGED_SCALE))" "reentrant:$((20 * EEZ_STAGED_SCALE))" \
    "revertCounter:$((50 * EEZ_STAGED_SCALE))" "revertCounterL2:$((50 * EEZ_STAGED_SCALE))" \
    "revertFromOtherChain:$((50 * EEZ_STAGED_SCALE))" "revertFromOtherChainL2:$((50 * EEZ_STAGED_SCALE))"
