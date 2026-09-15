#!/usr/bin/env python3
"""Test-only Engine API driver for the existing embedded Ethereum L1 fixture.

Uses the node's real payload builder, execution, transaction pool and Engine
API. A file barrier can replace an unfinalized ancestor while the composer
keeps its in-memory queue. No state override or fabricated receipt is used.
"""
import argparse
import base64
import hashlib
import hmac
import json
from pathlib import Path
import time
import urllib.parse
import urllib.request


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--rpc', required=True)
    parser.add_argument('--engine', required=True)
    parser.add_argument('--jwt', type=Path, required=True)
    parser.add_argument('--control', type=Path, required=True)
    parser.add_argument('--genesis-time', type=int, required=True)
    parser.add_argument('--block-seconds', type=int, required=True)
    args = parser.parse_args()
    for endpoint in (args.rpc, args.engine):
        if urllib.parse.urlparse(endpoint).hostname != '127.0.0.1':
            raise ValueError('native fixture only accepts loopback endpoints')
    secret = bytes.fromhex(args.jwt.read_text().strip().removeprefix('0x'))

    def b64(data):
        return base64.urlsafe_b64encode(data).rstrip(b'=')

    def rpc(method, params, engine=False):
        headers = {'Content-Type': 'application/json'}
        if engine:
            unsigned = b64(b'{"alg":"HS256","typ":"JWT"}') + b'.' + b64(json.dumps({'iat': int(time.time())}).encode())
            token = unsigned + b'.' + b64(hmac.digest(secret, unsigned, 'sha256'))
            headers['Authorization'] = 'Bearer ' + token.decode()
        request = urllib.request.Request(args.engine if engine else args.rpc,
            json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params}).encode(), headers)
        with urllib.request.urlopen(request, timeout=10) as response:
            result = json.load(response)
        if 'error' in result:
            raise RuntimeError(method + ': ' + json.dumps(result['error']))
        return result['result']

    if int(rpc('eth_chainId', []), 16) != 1337:
        raise ValueError('native consensus driver requires the test chain ID')
    head = rpc('eth_getBlockByNumber', ['latest', False])
    finalized = rpc('eth_getBlockByNumber', ['finalized', False])
    final_hash = finalized['hash'] if finalized else rpc('eth_getBlockByNumber', ['0x0', False])['hash']
    zero = '0x' + '00' * 32

    def fcu(block_hash, attributes=None):
        result = rpc('engine_forkchoiceUpdatedV3', [{
            'headBlockHash': block_hash, 'safeBlockHash': final_hash,
            'finalizedBlockHash': final_hash}, attributes], True)
        if result['payloadStatus']['status'] != 'VALID':
            raise RuntimeError('invalid forkchoice: ' + json.dumps(result))
        return result

    def write(name, value):
        temporary = args.control / (name + '.new')
        temporary.write_text(json.dumps(value))
        temporary.replace(args.control / name)

    def build(parent, timestamp, competing=False):
        attributes = {'timestamp': hex(timestamp),
            'prevRandao': '0x' + '01' * 32 if competing else zero,
            'suggestedFeeRecipient': '0x0000000000000000000000000000000000000000',
            'withdrawals': [], 'parentBeaconBlockRoot': zero}
        payload_id = fcu(parent['hash'], attributes)['payloadId']
        if payload_id is None:
            raise RuntimeError('client did not start building on the current head')
        return rpc('engine_getPayloadV5', [payload_id], True)

    def adopt(built):
        payload = built['executionPayload']
        commitments = (built.get('blobsBundle') or {}).get('commitments', [])
        hashes = ['0x01' + hashlib.sha256(bytes.fromhex(c.removeprefix('0x'))).hexdigest()[2:] for c in commitments]
        result = rpc('engine_newPayloadV4', [payload, hashes, zero, built.get('executionRequests', [])], True)
        if result['status'] != 'VALID':
            raise RuntimeError('invalid built payload: ' + json.dumps(result))
        fcu(payload['blockHash'])
        return rpc('eth_getBlockByHash', [payload['blockHash'], False])

    fcu(head['hash'])
    write('ready.json', {'head': head['hash'], 'number': head['number']})
    alternatives = {}
    while True:
        request_path = args.control / 'reorg.json'
        if request_path.exists():
            pending = json.loads(request_path.read_text())
            request_path.rename(args.control / 'reorg.used.json')
            ancestor = rpc('eth_getBlockByHash', [pending['hash'], False])
            if not ancestor or int(ancestor['number'], 16) != pending['number']:
                raise ValueError('reorg ancestor identity mismatch')
            if finalized and int(ancestor['number'], 16) <= int(finalized['number'], 16):
                raise ValueError('cannot replace finalized fixture history')
            pending['previousHead'] = {'hash': head['hash'], 'number': head['number']}
            # Reth correctly ignores FCU rewinds to an older canonical ancestor.
            # Import a real sibling payload instead, then select that fork.
            # It was built on this parent before the later settlement arrived.
            head = adopt(alternatives[pending['hash']])
            write('reorg-result.json', {**pending,
                'replacement': {'number': head['number'], 'hash': head['hash']}})
        now = int(time.time())
        timestamp = args.genesis_time + ((now - args.genesis_time) // args.block_seconds) * args.block_seconds
        if timestamp <= int(head['timestamp'], 16):
            time.sleep(0.05)
            continue
        head = adopt(build(head, timestamp))
        # Build a competing successor immediately, before the next submission
        # window. Its execution and roots come from the real payload builder;
        # no account/state/receipt override or hand-built block is involved.
        alternatives[head['hash']] = build(head, timestamp + args.block_seconds, competing=True)
        print(json.dumps({'number': head['number'], 'hash': head['hash']}), flush=True)


if __name__ == '__main__':
    main()
