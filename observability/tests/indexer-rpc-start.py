#!/usr/bin/env python3
"""Start the existing simulated RPC with one persisted, validated control state."""
import pathlib
import runpy
import threading
import time
import urllib.error
import urllib.request


def main():
    control = pathlib.Path('/fixture-control.json').read_bytes()
    server = threading.Thread(target=runpy.run_path, args=('/rpc.py',), daemon=True)
    server.start()
    deadline = time.monotonic() + 20
    while server.is_alive() and time.monotonic() < deadline:
        try:
            request = urllib.request.Request('http://127.0.0.1:9000/control', data=control,
                                             headers={'Content-Type': 'application/json'})
            with urllib.request.urlopen(request, timeout=2) as response:
                if response.status != 200:
                    raise RuntimeError('Simulated RPC rejected its persisted control state')
                break
        except (urllib.error.URLError, TimeoutError):
            time.sleep(0.1)
    else:
        raise RuntimeError('Simulated RPC did not accept its persisted control state')
    server.join()
    raise RuntimeError('Simulated RPC stopped unexpectedly')


if __name__ == '__main__':
    main()
