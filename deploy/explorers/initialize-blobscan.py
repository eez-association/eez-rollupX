#!/usr/bin/env python3
"""Initialize an empty Blobscan cursor and preserve its configured history boundary."""
import argparse
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import time

from configuration import read_env, request, resolve_origin, rpc


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b'=')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--env-file', type=Path, help='Otherwise read BLOBSCAN_SECRET_KEY from the environment')
    parser.add_argument('--start-block', type=int, required=True)
    parser.add_argument('--start-slot', type=int, required=True)
    parser.add_argument('--start-block-hash', required=True)
    parser.add_argument('--origin-file', type=Path, required=True)
    parser.add_argument('--l1-rpc', default='http://127.0.0.1:18645')
    parser.add_argument('--beacon', default='http://127.0.0.1:6152')
    parser.add_argument('--api', default='http://127.0.0.1:18101')
    args = parser.parse_args()
    if args.start_block < 1 or args.start_slot < 1 or not re.fullmatch(r'0x[0-9a-fA-F]{64}', args.start_block_hash):
        parser.error('Prepare a valid Blobscan history boundary with prepare.py --start-block')
    expected = dict(chainId=10200, startBlock=args.start_block, startSlot=args.start_slot,
                    startBlockHash=args.start_block_hash.lower())
    if int(rpc(args.l1_rpc, 'eth_chainId', []), 16) != 10200:
        parser.error('Blobscan requires Chiado L1 chain 10200')
    if args.origin_file.exists() and json.loads(args.origin_file.read_text()) != expected:
        parser.error('History boundary differs from the initialized archive; use a new Compose project')
    state = request(args.api + '/blockchain-sync-state')
    if state['lastUpperSyncedSlot'] is not None:
        if not args.origin_file.exists():
            parser.error('Existing cursor has no saved origin; validate/migrate the archive before adopting it')
        if state['lastUpperSyncedSlot'] < args.start_slot - 1:
            parser.error('Existing cursor is below the saved history boundary')
        print('Keeping existing Blobscan cursor:', state['lastUpperSyncedSlot'])
        return
    if any(state.values()):
        parser.error('Partially initialized database; refusing to replace its cursor')
    try:
        actual = resolve_origin(args.start_block, args.l1_rpc, args.beacon)
    except (ValueError, OSError) as exc:
        parser.error(str(exc))
    if actual != expected:
        parser.error('Configured history boundary disagrees with canonical execution/beacon data')
    values = read_env(args.env_file) if args.env_file else os.environ
    secret = values.get('BLOBSCAN_SECRET_KEY')
    if not secret:
        parser.error('Set BLOBSCAN_SECRET_KEY or provide --env-file')
    body = b'.'.join([b64(b'{"alg":"HS512","typ":"JWT"}'),
                      b64(json.dumps({'exp': int(time.time()) + 60}).encode())])
    token = body + b'.' + b64(hmac.digest(secret.encode(), body, hashlib.sha512))
    # Save before the API write so a crash after setting the cursor is resumable.
    # An empty database with the same saved origin can safely retry this operation.
    args.origin_file.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.origin_file.with_suffix('.tmp')
    temporary.write_text(json.dumps(expected, indent=2) + '\n')
    temporary.replace(args.origin_file)
    request(args.api + '/blockchain-sync-state',
            {'lastUpperSyncedSlot': args.start_slot - 1}, method='PUT',
            headers={'Authorization': 'Bearer ' + token.decode()})
    print(json.dumps(expected, indent=2))


if __name__ == '__main__':
    main()
