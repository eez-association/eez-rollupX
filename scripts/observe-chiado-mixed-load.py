"""Read-only receipt history for this run's cross-chain hashes; no resubmission."""
import argparse
import json
import re
import time
import urllib.request
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--evidence", type=Path, required=True)
root = parser.parse_args().evidence
seen = {}
last = {}
started = time.monotonic()
with (root / 'cross-chain-receipt-history.jsonl').open('x', buffering=1) as output:
    while time.monotonic() - started < 3600:
        text = (root / 'live-waves.log').read_text()
        for side, kind, arg, txhash in re.findall(
                r'tx side=(in|out) kind=(\w+) arg=(\d+) hash=(0x[0-9a-fA-F]{64})', text):
            seen.setdefault(txhash, dict(side=side, kind=kind, argument=arg))
        for side, port in [('in', 18645), ('out', 18688)]:
            hashes = [h for h, item in seen.items() if item['side'] == side]
            for offset in range(0, len(hashes), 50):
                group = hashes[offset:offset+50]
                payload = [dict(jsonrpc='2.0', id=h, method='eth_getTransactionReceipt',
                                params=[h]) for h in group]
                try:
                    req = urllib.request.Request(f'http://127.0.0.1:{port}',
                        json.dumps(payload).encode(), {'Content-Type': 'application/json'})
                    with urllib.request.urlopen(req, timeout=10) as response:
                        rows = json.load(response)
                    assert {row['id'] for row in rows} == set(group)
                    for row in rows:
                        assert 'error' not in row
                        h, receipt = row['id'], row['result']
                        identity = None if receipt is None else (receipt['blockHash'], receipt['status'])
                        if h not in last or last[h] != identity:
                            output.write(json.dumps(dict(at=time.time(), hash=h,
                                **seen[h], previous=last.get(h), receipt=receipt))+'\n')
                        last[h] = identity
                except Exception as exc:
                    output.write(json.dumps(dict(at=time.time(), observationError=type(exc).__name__))+'\n')
        if (root / 'live-mixed.exit').exists():
            break
        time.sleep(5)
    print(json.dumps(dict(observedHashes=len(seen), currentlyReceipted=sum(v is not None for v in last.values()))))
