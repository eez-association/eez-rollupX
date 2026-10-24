"""Repeat the retained owned-enclave L1 pauses with the exact recovery sender."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import urllib.request

root = Path.cwd()
config = json.loads(Path(os.environ['EEZ_LOAD_CONFIG']).read_text())
plan = json.loads((root / 'docs/audits/2026-10-24-extended-validation/kurtosis/l1-fault-plan.json').read_text())
case = next(row for row in plan['cases'] if row['name'] == os.environ['EEZ_L1_FAULT'])
assert config['enclave'] == os.environ['KURTOSIS_ENCLAVE'] == plan['enclave']
assert config['enclave_uuid'] == plan['enclave_uuid'] and config['runtime_source'] == plan['runtime_source']
assert config['chain_id'] == 696990
handoff = json.loads(Path(os.environ['EEZ_FINAL_RUNTIME_HANDOFF']).read_text())
assert handoff['pr_head'] == config['runtime_source']
out = Path(os.environ['EEZ_L1_FAULT_OUTPUT'])
out.mkdir(exist_ok=False)
history = '0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077'
node = config['containers'][0]
target = case['container_id']
record = {'verified': False, 'case': case, 'runtime_source': config['runtime_source'],
    'enclave': config['enclave'], 'enclave_uuid': config['enclave_uuid'],
    'ordinary_transfers': 24, 'maximum_gas_reserve_wei': '50400000000000',
    'arbitrary_wall_clock_sla': None, 'samples': []}


def save():
    (out / 'result.json').write_text(json.dumps(record, indent=2) + '\n')


def command(*args, timeout=30):
    return subprocess.check_output(args, text=True, timeout=timeout).strip()


def inspect(container, service):
    row = json.loads(command('docker', 'inspect', container))[0]
    assert row['Id'] == container
    assert row['Config']['Labels']['com.kurtosistech.enclave-id'] == config['enclave_uuid']
    assert row['Config']['Labels']['com.kurtosistech.id'] == service
    assert row['State']['Running'] and not row['State']['OOMKilled']
    return row


def memory():
    return int(next(row.split()[1] for row in Path('/proc/meminfo').read_text().splitlines()
                    if row.startswith('MemAvailable:'))) * 1024


def rpc(url, method, *params):
    request = urllib.request.Request(url, json.dumps({'jsonrpc': '2.0', 'id': 1,
        'method': method, 'params': params}).encode(), {'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=5) as response:
        row = json.load(response)
    assert 'error' not in row, (method, row.get('error'))
    return row['result']


current_selector = command('cast', 'calldata', 'current()')
hash_selector = command('cast', 'sig', 'getL1BlockHash(uint64)')
registry_selector = command('cast', 'calldata', 'rollups(uint64)', '1')


def origin(tag):
    value = rpc(config['l2'], 'eth_call', {'to': history, 'data': current_selector}, tag)
    assert value.startswith('0x') and len(value) == 130
    return {'number': int(value[2:66], 16), 'hash': '0x' + value[66:]}


def embedded_head():
    # Reuse the retained diagnostic method; this short-lived helper publishes no
    # host ports and reads only the owned node's loopback execution RPC.
    code = "import json,urllib.request;r=urllib.request.Request('http://127.0.0.1:18545',json.dumps({'jsonrpc':'2.0','id':1,'method':'eth_getBlockByNumber','params':['latest',False]}).encode(),{'Content-Type':'application/json'});x=json.load(urllib.request.urlopen(r,timeout=5));assert 'error' not in x;print(json.dumps(x['result']))"
    inspect(node, 'eez-node')
    return json.loads(command('docker', 'run', '--rm', '--network', 'container:' + node,
        '--entrypoint', 'python3', handoff['images']['deploy'], '-c', code, timeout=20))


def sample(phase):
    began = time.monotonic()
    latest = rpc(config['l2'], 'eth_getBlockByNumber', 'latest', False)
    safe = rpc(config['l2'], 'eth_getBlockByNumber', 'safe', False)
    l1 = rpc(config['l1'], 'eth_getBlockByNumber', 'latest', False)
    nonce = rpc(config['l2'], 'eth_getTransactionCount', config['sender'], latest['number'])
    selected = origin(latest['hash'])
    recheck = rpc(config['l2'], 'eth_getBlockByNumber', latest['number'], False)
    row = {'at': time.time(), 'phase': phase, 'latest': latest, 'safe': safe, 'l1': l1,
        'sender_nonce': int(nonce, 16), 'origin': selected,
        'sample_hash_stable': recheck['hash'] == latest['hash'],
        'available_memory_bytes': memory(), 'read_duration_seconds': time.monotonic() - began}
    record['samples'].append(row); save()
    assert row['available_memory_bytes'] >= 5 * 1024**3
    return row


def interrupted(signum, frame):
    raise InterruptedError(f'Owned L1 control received signal {signum}')


for signum in [signal.SIGTERM, signal.SIGINT]:
    signal.signal(signum, interrupted)
assert memory() >= 12 * 1024**3
assert rpc(config['l2'], 'eth_getBlockByNumber', '0x0', False)['hash'] == config['genesis']
assert int(rpc(config['l1'], 'eth_chainId'), 16) == 7331
assert rpc(config['l1'], 'eth_getBlockByNumber', '0x0', False)['hash'] == config['l1_genesis']
assert int(rpc(config['l2'], 'eth_chainId'), 16) == config['chain_id']
before_service = inspect(target, case['service'])
assert not before_service['State']['Paused']
record['service_before'] = {'id': before_service['Id'], 'name': before_service['Name'],
    'image': before_service['Image'], 'state': before_service['State']}
record['embedded_before'] = embedded_head()
script = root / 'artifacts/extended-validation/tools/ordinary-recovery.mjs'
record['sender_sha256'] = hashlib.sha256(script.read_bytes()).hexdigest()
assert record['sender_sha256'] == '6528e4ed7ffa0775ef21919912f54bf1dc0375bff08f8af8506e2c2cdb83074c'
deadline = time.monotonic() + 180
while True:
    before = sample('before')
    value = rpc(config['l1'], 'eth_call', {'to': os.environ['EEZ_REGISTRY_ADDRESS'],
        'data': registry_selector}, before['l1']['number'])
    assert len(value) == 194
    if '0x' + value[66:130] == before['safe']['hash']:
        break
    assert time.monotonic() < deadline, 'Safe and Registry commitment did not converge'
    time.sleep(1)
record['before'] = before
env = dict(os.environ)
env['SOURCE_PK'] = Path(env['EEZ_ORDINARY_KEY_FILE']).read_text().strip()
env['EEZ_LOAD_OUTPUT'] = str(out / 'ordinary')
stream = (out / 'ordinary.log').open('x')
process = None
requested = False
paused = False
try:
    (out / 'operation-intent.json').write_text(json.dumps({k: v for k, v in record.items()
        if k != 'samples'}, indent=2) + '\n')
    requested = True
    subprocess.run(['docker', 'pause', target], check=True, stdout=subprocess.DEVNULL)
    paused = True
    record['paused_at'] = time.time(); save()
    deadline = time.monotonic() + case['settling_seconds']
    while time.monotonic() < deadline:
        sample('settling')
        time.sleep(min(1, max(0, deadline - time.monotonic())))
    record['embedded_during_start'] = embedded_head()
    start = sample('start'); record['measurement_start'] = start; save()
    deadline = time.monotonic() + case['measurement_seconds']
    process = subprocess.Popen(['node', str(script)], env=env, cwd=root, stdout=stream,
        stderr=subprocess.STDOUT, start_new_session=True)
    del env['SOURCE_PK']
    try:
        while time.monotonic() < deadline:
            assert process.poll() is None, 'Ordinary sender exited during the measured pause'
            sample('paused')
            time.sleep(min(1, max(0, deadline - time.monotonic())))
        end = sample('end'); record['measurement_end'] = end; save()
        record['embedded_during_end'] = embedded_head(); save()
        capture_deadline = time.monotonic() + 30
        blocks = []
        block = rpc(config['l2'], 'eth_getBlockByHash', end['latest']['hash'], True)
        while int(block['number'], 16) > int(start['latest']['number'], 16):
            assert time.monotonic() < capture_deadline and len(blocks) < 256, 'Unsafe capture bound exceeded'
            selected = origin(block['hash'])
            parent_selected = origin(block['parentHash'])
            transactions = block['transactions']
            repeated = bool(transactions and transactions[0]['type'] == '0x45'
                and (transactions[0].get('to') or '').lower() == history and selected == parent_selected)
            blocks.append({'number': int(block['number'], 16), 'hash': block['hash'],
                'parent_hash': block['parentHash'], 'timestamp': int(block['timestamp'], 16),
                'origin': selected, 'randao': block['mixHash'], 'repeated_history': repeated,
                'history_transaction': transactions[0] if repeated else None})
            block = rpc(config['l2'], 'eth_getBlockByHash', block['parentHash'], True)
            assert block
        assert block['hash'] == start['latest']['hash'], 'Observed unsafe branch replaced before capture'
        blocks.reverse(); record['unsafe_blocks'] = blocks; save()
    finally:
        if paused:
            assert inspect(target, case['service'])['State']['Paused']
            subprocess.run(['docker', 'unpause', target], check=True, stdout=subprocess.DEVNULL)
            paused = False
            record['restored_at'] = time.time(); save()

    rows = [r for r in record['samples'] if r['phase'] in ('start', 'paused', 'end')]
    assert all(r['sample_hash_stable'] for r in rows), 'Unstable sample requires separate diagnosis'
    assert end['at'] - start['at'] >= 72
    a, b = record['embedded_during_start'], record['embedded_during_end']
    assert a['number'] == b['number'] and a['hash'] == b['hash'], 'Embedded L1 did not remain fixed'
    if case['name'] == 'missed_l1_slots':
        assert all(r['l1']['hash'] == start['l1']['hash'] and r['l1']['number'] == start['l1']['number'] for r in rows)
        assert all(r['origin'] == start['origin'] and r['latest']['mixHash'] == start['latest']['mixHash'] for r in rows)
    else:
        assert int(end['l1']['number'], 16) >= int(start['l1']['number'], 16) + 4
        assert all(r['origin']['number'] <= int(a['number'], 16) for r in rows)
    parent_hash = start['latest']['hash']; timestamp = int(start['latest']['timestamp'], 16)
    for block in blocks:
        assert block['parent_hash'] == parent_hash and block['timestamp'] == timestamp + 2
        l1 = rpc(config['l1'], 'eth_getBlockByNumber', hex(block['origin']['number']), False)
        assert l1['hash'] == block['origin']['hash'] and l1['mixHash'] == block['randao']
        parent_hash, timestamp = block['hash'], block['timestamp']
    assert blocks and any(block['repeated_history'] for block in blocks)
    nonce_progress = max(row['sender_nonce'] for row in rows) - start['sender_nonce']
    assert nonce_progress > 0, 'No ordinary execution progress during the pause'
    changes = [rows[0]]
    for row in rows[1:]:
        if row['latest']['hash'] != changes[-1]['latest']['hash']:
            changes.append(row)
    record['production'] = {'blocks': len(blocks), 'sender_nonce_progress': nonce_progress,
        'repeated_history_blocks': [r['number'] for r in blocks if r['repeated_history']],
        'maximum_head_age_seconds': max(r['at'] - int(r['latest']['timestamp'], 16) for r in rows),
        'maximum_sample_gap_seconds': max(y['at'] - x['at'] for x, y in zip(rows, rows[1:])),
        'maximum_observed_commit_gap_seconds': max((y['at'] - x['at'] for x, y in zip(changes, changes[1:])), default=None)}
    save()
    deadline = time.monotonic() + 660
    while process.poll() is None:
        assert time.monotonic() < deadline, 'Ordinary canonical recovery exceeded eleven minutes'
        sample('recovery'); time.sleep(1)
    assert process.returncode == 0
    summary = json.loads((out / 'ordinary/summary.json').read_text())
    assert summary['accepted'] == summary['offered'] == summary['planned'] == 24
    assert summary['verified_receipts_and_accounting'] and summary['error'] is None
    while True:
        after = sample('canonical')
        if (int(after['safe']['number'], 16) >= int(end['latest']['number'], 16)
                and origin(after['safe']['hash'])['number'] > int(a['number'], 16)):
            break
        assert time.monotonic() < deadline, 'Canonical history recovery exceeded eleven minutes'
        time.sleep(1)
    selected = origin(after['safe']['hash'])
    canonical = rpc(config['l1'], 'eth_getBlockByNumber', hex(selected['number']), False)
    assert canonical['hash'] == selected['hash'] and canonical['mixHash'] == after['safe']['mixHash']
    history_rows = []
    previous_hash = None
    for n in range(start['origin']['number'], selected['number'] + 1):
        l1 = rpc(config['l1'], 'eth_getBlockByNumber', hex(n), False)
        stored = rpc(config['l2'], 'eth_call', {'to': history, 'data': hash_selector + format(n, '064x')}, after['safe']['hash'])
        assert stored == l1['hash']
        if previous_hash:
            assert l1['parentHash'] == previous_hash
        history_rows.append({'number': n, 'hash': l1['hash'], 'timestamp': int(l1['timestamp'], 16)})
        previous_hash = l1['hash']
    changes = []
    for block in blocks:
        canonical = rpc(config['l2'], 'eth_getBlockByNumber', hex(block['number']), False)
        if canonical['hash'] != block['hash']:
            changes.append({'number': block['number'], 'before': block['hash'], 'after': canonical['hash']})
    record['recovery'] = {'safe': after['safe'], 'selected_origin': selected,
        'verified_history': history_rows, 'unsafe_replacements': changes,
        'embedded_l1': embedded_head(), 'ordinary_summary': summary}
    record['verified'] = True
except BaseException as error:
    record['error'] = str(error)
    raise
finally:
    if requested and inspect(target, case['service'])['State']['Paused']:
        subprocess.run(['docker', 'unpause', target], check=True, stdout=subprocess.DEVNULL)
        record['restored_at'] = time.time()
    if process and process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=12)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL); process.wait()
    after_service = inspect(target, case['service'])
    record['service_after'] = {'id': after_service['Id'], 'name': after_service['Name'],
        'image': after_service['Image'], 'state': after_service['State']}
    record['restored_running'] = not after_service['State']['Paused']
    record['same_service_process'] = all(after_service['State'][key] == before_service['State'][key]
        for key in ['Pid', 'StartedAt'])
    record['ordinary_exit'] = process.poll() if process else None
    record['ended_at'] = time.time(); save(); stream.close()
    assert record['restored_running'] and record['same_service_process']
print(json.dumps({'verified': record['verified'], 'case': case['name'],
    'production': record['production'], 'safe': record['recovery']['safe']['number']}))
