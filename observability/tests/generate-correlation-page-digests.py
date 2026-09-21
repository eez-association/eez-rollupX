#!/usr/bin/env python3
"""Print deterministic public RPC-fixture Keccak vectors using installed Foundry cast.

Generate: python3 observability/tests/generate-correlation-page-digests.py > observability/tests/fixtures/correlation-page-digests.json
No RPC, key, Docker or chain access; this is offline hashing only.
"""
import json
import subprocess


def generate():
    values = {}
    for l1_number in (2, 3):  # canonical and deliberately wrong-anchor evidence
        for fork in (False, True):
            l1_hash = 112 if fork else 102
            for last_hash in (212 if fork else 202, 999, 201):  # valid, wrong hash, duplicate
                for index in range(32):
                    data = b'EEZ_CORRELATION_PAGE_V1'
                    for value, width in ((l1_number,8),(l1_hash,32),(1002+index,32),(index,8),(2,8),
                                         (1,8),(201,32),(2,8),(last_hash,32)):
                        data += value.to_bytes(width,'big')
                    value = subprocess.check_output(['cast','keccak','0x'+data.hex()],text=True,timeout=5).strip()
                    if len(value) != 66 or not value.startswith('0x'):
                        raise ValueError('invalid cast digest')
                    values[f'{l1_number}:{l1_hash}:{last_hash}:{index}'] = value
    return {'algorithm':'Keccak-256','domain':'EEZ_CORRELATION_PAGE_V1',
            'generator':'observability/tests/generate-correlation-page-digests.py; cast keccak, offline only',
            'encoding':'domain || l1_number:u64be || l1_hash:bytes32 || tx_hash:bytes32 || tx_index:u64be || count:u64be || (number:u64be || hash:bytes32)*',
            'digests':values}


if __name__ == '__main__':
    print(json.dumps(generate(),indent=2,sort_keys=True))
