#!/usr/bin/env python3
"""Deterministic fork/receipt source. No transactions are submitted anywhere."""
import http.server
import json
import threading


def digest(number):
    return '0x' + f'{number:064x}'


class RPC(http.server.BaseHTTPRequestHandler):
    phase = 'missing'
    finalize = False
    lock = threading.Lock()

    def log_message(self, *args):
        pass

    def send(self, value):
        body = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path == '/control':
            with self.lock:
                if 'phase' in request:
                    assert request['phase'] in ('missing', 'healthy', 'fork', 'contradiction', 'wrong_identity', 'mid_fetch', 'pruned', 'wrong_type')
                    type(self).phase = request['phase']
                if 'finalize' in request:
                    type(self).finalize = request['finalize']
            self.send({'ok': True})
            return
        chain = 1 if self.path == '/l1' else 2
        with self.lock:
            phase, finalize = self.phase, self.finalize
        fork = phase == 'fork'
        def block_hash(height):
            return digest(chain * 100 + height + (10 if fork and height >= 2 else 0))
        def transaction(height):
            return {'hash': digest(chain * 1000 + height), 'blockHash': block_hash(height),
                'blockNumber': hex(height), 'transactionIndex': '0x0', 'from': '0x' + 'aa' * 20,
                'to': '0x' + 'bb' * 20, 'nonce': hex(height - 1), 'type': '0x3' if height == 2 else '0x2', 'input': '0x12345678'}
        method, params = request['method'], request['params']
        if method == 'eth_chainId':
            value = hex(chain + (10 if phase == 'wrong_identity' else 0))
        elif method == 'eth_getBlockByNumber':
            selector, full = params
            height = 3 if selector == 'latest' else (3 if finalize else 1) if selector == 'finalized' else int(selector, 16)
            if selector == 'finalized' and chain == 2:
                raise AssertionError('L2 RPC finality is not settlement evidence')
            value = None if height > 3 else {'number': hex(height), 'hash': block_hash(height),
                'parentHash': digest(0) if height == 0 else block_hash(height - 1), 'timestamp': hex(100 + height * 5),
                'transactions': [transaction(height)] if full and height in (1, 2) else []}
            if phase == 'pruned' and height == 2 and full:
                value = None
        elif method == 'eth_getTransactionReceipt':
            height = int(params[0], 16) - chain * 1000
            assert height in (1, 2)
            if phase == 'mid_fetch' and height == 2 and chain == 1:
                with self.lock:
                    type(self).phase = 'fork'
                fork = True
            if phase == 'missing' and height == 2:
                value = None
            else:
                tx = transaction(height)
                value = {key: tx[key] for key in ('hash', 'blockHash', 'blockNumber', 'transactionIndex', 'type')}
                value['transactionHash'] = value.pop('hash')
                value.update(status='0x0' if height == 2 else '0x1', gasUsed=hex(22000 if height == 2 else 21000),
                    effectiveGasPrice=hex((12 if fork else 9) if height == 2 else 7))
                if phase == 'wrong_type' and height == 2:
                    value['type'] = '0x2'
                if height == 2:
                    value.update(blobGasUsed=hex(131072), blobGasPrice='0x3')
        else:
            self.send({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': 'unsupported fixture method'}})
            return
        self.send({'jsonrpc': '2.0', 'id': request['id'], 'result': value})


http.server.ThreadingHTTPServer(('0.0.0.0', 9000), RPC).serve_forever()
