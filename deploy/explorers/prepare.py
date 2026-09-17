#!/usr/bin/env python3
"""Prepare explorer credentials and settings for an existing deployment."""
import argparse
import json
from pathlib import Path
import secrets
import shlex
from urllib.parse import urlsplit

from configuration import read_env, resolve_origin


def endpoint_url(value):
    try:
        url = urlsplit(value)
        if url.scheme not in ('http', 'https') or not url.hostname:
            raise ValueError
        _ = url.port
    except ValueError:
        raise argparse.ArgumentTypeError('Expected an HTTP(S) node URL with a valid hostname and port') from None
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--env-file', type=Path, default=Path('.env.explorers'))
    parser.add_argument('--genesis', type=Path, help='Actual L2 genesis for Blockscout')
    parser.add_argument('--start-block', type=int, help='First retained Chiado L1 block for Blobscan')
    parser.add_argument('--l2-rpc', type=endpoint_url, help='L2 RPC URL reachable from explorer containers')
    parser.add_argument('--l1-rpc', type=endpoint_url, help='Chiado L1 RPC URL reachable from this host and explorer containers')
    parser.add_argument('--beacon', type=endpoint_url, help='Beacon API URL reachable from this host and explorer containers')
    args = parser.parse_args()
    env_file = args.env_file.resolve()
    existing = read_env(env_file) if env_file.exists() else {}
    updates = {}
    if args.genesis and not args.l2_rpc:
        parser.error('--genesis requires --l2-rpc with a container-reachable node hostname')
    if args.start_block is not None and not (args.l1_rpc and args.beacon):
        parser.error('--start-block requires --l1-rpc and --beacon with container-reachable node hostnames')
    if args.genesis:
        genesis = args.genesis.resolve(strict=True)
        chain_id = json.loads(genesis.read_text())['config']['chainId']
        if type(chain_id) is not int or not 0 < chain_id <= 2**53 - 1:
            parser.error('Genesis chainId must be a positive safe integer')
        updates.update(L2_GENESIS=str(genesis), L2_CHAIN_ID=str(chain_id), L2_RPC_URL=args.l2_rpc)
    if args.start_block is not None:
        if existing.get('BLOBSCAN_START_BLOCK') not in (None, str(args.start_block)):
            parser.error('Existing history boundary differs; use a new env file and Compose project')
        try:
            origin = resolve_origin(args.start_block, args.l1_rpc, args.beacon)
        except (ValueError, OSError) as exc:
            parser.error(str(exc))
        updates.update(BLOBSCAN_START_BLOCK=str(origin['startBlock']),
                       BLOBSCAN_START_SLOT=str(origin['startSlot']),
                       BLOBSCAN_START_BLOCK_HASH=origin['startBlockHash'],
                       BLOBSCAN_L1_RPC_URL=args.l1_rpc, BLOBSCAN_BEACON_URL=args.beacon)
    for key, value in updates.items():
        if key in existing and existing[key] != value:
            parser.error(f'Existing {key} differs; use a new env file and Compose project')
    if existing:
        updates = {key: value for key, value in updates.items() if key not in existing}
        if updates:
            with env_file.open('a') as output:
                output.write('\n' + ''.join(f'{k}={shlex.quote(v)}\n' for k, v in updates.items()))
        print(f'Keeping existing explorer credentials and selection: {env_file}')
        return
    if not updates:
        parser.error('Provide --genesis for Blockscout and/or --start-block for Blobscan')
    profiles = (['blockscout'] if args.genesis else []) + (['blobscan'] if args.start_block is not None else [])
    values = {
        'COMPOSE_FILE': 'docker-compose.explorers.yml',
        'COMPOSE_PROFILES': ','.join(profiles),
        'BLOCKSCOUT_DB_PASSWORD': secrets.token_hex(24),
        'BLOCKSCOUT_SECRET_KEY_BASE': secrets.token_hex(48),
        'BLOBSCAN_DB_PASSWORD': secrets.token_hex(24),
        'BLOBSCAN_SECRET_KEY': secrets.token_hex(48),
        **updates,
    }
    env_file.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with env_file.open('x') as output:
        env_file.chmod(0o600)
        output.write('# Select blockscout, blobscan, or blockscout,blobscan.\n')
        output.write(''.join(f'{k}={shlex.quote(v)}\n' for k, v in values.items()))
        output.write('\n# Optional published port overrides.\n'
                     '# BLOCKSCOUT_WEB_PORT=18110\n# BLOCKSCOUT_API_PORT=18111\n'
                     '# BLOBSCAN_WEB_PORT=18100\n# BLOBSCAN_API_PORT=18101\n')
    print(f'Prepared explorer configuration: {env_file}')


if __name__ == '__main__':
    main()
