#!/usr/bin/env python3
"""Exercise only local mock RPCs and a temporary exporter process."""
import argparse
import http.server
import json
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True, type=pathlib.Path)
args = parser.parse_args()
fail_queue = threading.Event()
genesis = '0x' + 'aa' * 32


class Rpc(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        method = request['method']
        if method == 'eth_chainId':
            result = '0x1'
        elif method == 'eth_getBlockByNumber':
            result = {'number': '0x0' if request['params'][0] == '0x0' else '0xa',
                      'hash': genesis, 'timestamp': hex(int(time.time()))}
        elif method == 'eez_getCrossChainQueueStatus':
            if fail_queue.is_set():
                self.send_response(503)
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            result = {'queued': 3, 'inFlight': 1, 'ready': 1, 'blocked': 1, 'unknown': 1,
                      'evaluatedAtMs': int(time.time() * 1000),
                      'sourceState': {c: {'number': 10, 'hash': genesis} for c in ('l1', 'l2')},
                      'oldestPendingAgeMs': 2000, 'oldestServiceAgeMs': None}
        else:
            raise AssertionError(f'Unexpected RPC method: {method}')
        body = json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


rpc = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Rpc)
threading.Thread(target=rpc.serve_forever, daemon=True).start()
with socket.socket() as reservation:
    reservation.bind(('127.0.0.1', 0))
    metrics_port = reservation.getsockname()[1]


def get(path):
    with urllib.request.urlopen(f'http://127.0.0.1:{metrics_port}/{path}', timeout=2) as response:
        return response.headers, response.read().decode()


def until(predicate, seconds=35):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError('Exporter stopped unexpectedly')
        try:
            headers, text = get('metrics')
            if predicate(text):
                return headers, text
        except OSError:
            pass
        time.sleep(0.1)
    raise AssertionError('Expected metrics were not observed before deadline')


try:
    with tempfile.TemporaryDirectory(prefix='eez-observer-smoke-') as directory:
        config = pathlib.Path(directory) / 'config.json'
        endpoint = {'url': f'http://127.0.0.1:{rpc.server_port}', 'chain_id': 1, 'genesis_hash': genesis}
        config.write_text(json.dumps({'l1': endpoint, 'l2': endpoint, 'listen': f'127.0.0.1:{metrics_port}'}))
        with (pathlib.Path(directory) / 'observer.log').open('w') as log:
            process = subprocess.Popen([str(args.binary.resolve()), 'exporter', '--config', str(config)], stdout=log, stderr=log)
            try:
                headers, metrics = until(lambda s: 'eez_observer_queue_evaluation_valid 1\n' in s)
                assert headers['Content-Type'].startswith('text/plain; version=0.0.4')
                assert 'eez_observer_queue_transactions{state="queued_ready"} 1\n' in metrics
                assert not any(line.startswith('eez_observer_queue_oldest_age_seconds{kind="serviceable"}') for line in metrics.splitlines())
                assert 'process is running' in get('healthz')[1]
                fail_queue.set()
                _, failed = until(lambda s: 'eez_observer_collection_error{source="queue",reason="http"} 1\n' in s)
                assert not any(line.startswith('eez_observer_queue_transactions{') for line in failed.splitlines())
                assert 'eez_observer_collection_last_success_timestamp_seconds{source="queue"}' in failed
                assert 'eez_observer_chain_head_number{source="l1"} 10\n' in failed
                fail_queue.clear()
                until(lambda s: 'eez_observer_queue_evaluation_valid 1\n' in s and 'reason="http"' not in s)
                print('PASS: HTTP metrics/health, null semantics, isolated RPC failure and recovery')
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                    raise AssertionError('Exporter failed bounded graceful shutdown')
finally:
    rpc.shutdown()
    rpc.server_close()
