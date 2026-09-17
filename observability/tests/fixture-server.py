#!/usr/bin/env python3
"""Disposable RPC, metrics and notification receiver. Used only by smoke-stack.py."""
import http.server
import json
import threading
import time


class Fixture(http.server.BaseHTTPRequestHandler):
    mode = 'healthy'
    events = []
    lock = threading.Lock()
    genesis = '0x' + 'aa' * 32
    metric_lines = None

    def log_message(self, *args):
        pass

    def send(self, data, status=200, content_type='application/json'):
        body = data if isinstance(data, bytes) else json.dumps(data).encode()
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == '/metrics':
            if self.mode == 'metrics_error':
                self.send({'error': 'fixture metrics unavailable'}, status=503)
                return
            if self.metric_lines is not None:
                now = time.time()
                body = '\n'.join(self.metric_lines).replace('$NOW', str(now)).replace('$HEAD', str(int(now))) + '\n'
                self.send(body.encode(), content_type='text/plain; version=0.0.4')
            else:
                self.send(b'node_memory_MemAvailable_bytes 10737418240\nnode_filesystem_avail_bytes{fstype="ext4",mountpoint="/fixture"} 107374182400\n', content_type='text/plain; version=0.0.4')
        elif self.path == '/events':
            with self.lock:
                self.send(self.events)
        elif self.path == '/heartbeat-status':
            with self.lock:
                times = [event['time'] for event in self.events if event['path'] == '/heartbeat']
            self.send({'age_seconds': time.monotonic() - max(times) if times else None, 'count': len(times)})
        else:
            self.send({'ok': True})

    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path == '/control':
            assert data['mode'] in ('healthy', 'stale', 'error', 'metrics_error')
            type(self).mode = data['mode']
            if 'metric_lines' in data:
                assert data['metric_lines'] is None or isinstance(data['metric_lines'], list) and all(isinstance(line, str) for line in data['metric_lines'])
                type(self).metric_lines = data['metric_lines']
            self.send({'ok': True})
            return
        if self.path in ('/alerts', '/heartbeat'):
            with self.lock:
                self.events.append({'path': self.path, 'time': time.monotonic(), 'data': data})
            self.send({'ok': True})
            return
        method = data['method']
        if method == 'eth_chainId':
            result = '0x1'
        elif method == 'eth_blockNumber':
            result = '0x64'
        elif method == 'eth_getBlockByNumber':
            result = {'hash': self.genesis, 'number': '0x0' if data['params'][0] == '0x0' else '0x64', 'timestamp': hex(int(time.time()))}
        elif method == 'eez_getCrossChainQueueStatus':
            if self.mode == 'error':
                self.send({'error': 'fixture unavailable'}, status=503)
                return
            result = {'queued': 3, 'ready': 1, 'blocked': 1, 'unknown': 1, 'inFlight': 1,
                      'oldestPendingAgeMs': 12000, 'oldestServiceAgeMs': None,
                      'evaluatedAtMs': int(time.time() * 1000) - (120000 if self.mode == 'stale' else 0),
                      'sourceState': {c: {'number': 100, 'hash': self.genesis} for c in ('l1', 'l2')}}
        else:
            raise AssertionError('Unexpected RPC method')
        self.send({'jsonrpc': '2.0', 'id': data['id'], 'result': result})


http.server.ThreadingHTTPServer(('0.0.0.0', 9000), Fixture).serve_forever()
