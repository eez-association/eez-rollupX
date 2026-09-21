#!/usr/bin/env python3
"""Deterministic fork/receipt source. No transactions are submitted anywhere."""
import http.server
import json
import pathlib
import threading
import time


POST_CALL = pathlib.Path('/post-calldata.hex').read_text().strip()
MULTI_ROLLUP_CALL = pathlib.Path('/multi-rollup-post-calldata.hex').read_text().strip()
BLOB_FIXTURES = json.loads(pathlib.Path('/blob-carriers.json').read_text())
PAGE_DIGESTS = json.loads(pathlib.Path('/correlation-page-digests.json').read_text())['digests']

def digest(number):
    return '0x' + f'{number:064x}'


class RPC(http.server.BaseHTTPRequestHandler):
    funding_base_time = None
    funding_balance = 591216
    funding_balance_error = False
    blob_mode = 'disabled'
    blob_requests = 0
    multi_rollup = False
    protocol_logs = 'missing'
    phase = 'missing'
    finalize = False
    correlation = 'none'
    post_success = False
    post_count = 1
    source_unselected = False
    source_receipt_missing = False
    source_cost_missing = False
    source_hash_mismatch = False
    source_receipt_failure = False
    source_receipt_delay_ms = 0
    lock = threading.Lock()

    def log_message(self, *args):
        pass

    def send(self, value):
        body = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == '/fixture/blob-requests':
            self.send({'requests': self.blob_requests})
            return
        if self.path == '/eth/v1/beacon/genesis':
            self.send({'data': {'genesis_time': '100'}})
            return
        if self.path == '/eth/v1/config/spec':
            self.send({'data': {'SECONDS_PER_SLOT': '5'}})
            return
        if self.path in ('/eth/v1/beacon/blobs/2', '/eth/v1/beacon/blob_sidecars/2'):
            with self.lock:
                type(self).blob_requests += 1
                mode = self.blob_mode
            legacy = '/blob_sidecars/' in self.path
            status = 410 if mode == 'pruned' else 404 if mode in ('disabled', 'unavailable', 'missing_metadata') or (mode == 'legacy' and not legacy) else 200
            if status != 200:
                self.send_response(status)
                self.end_headers()
                return
            blobs = [blob for fixture in BLOB_FIXTURES for blob in fixture['blobs']]
            commitments = [commitment for fixture in BLOB_FIXTURES for commitment in fixture['commitments']]
            if mode == 'corrupt':
                blobs[0] = blobs[0][:-2] + ('01' if blobs[0][-2:] != '01' else '02')
            self.send({'data': [{'blob':blob,'kzg_commitment':commitment} for blob,commitment in zip(blobs,commitments)] if legacy else blobs})
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path == '/control':
            with self.lock:
                if 'funding_base_time' in request:
                    assert request['funding_base_time'] is None or type(request['funding_base_time']) is int and request['funding_base_time'] > 0
                    type(self).funding_base_time = request['funding_base_time']
                if 'funding_balance' in request:
                    assert type(request['funding_balance']) is int and 0 <= request['funding_balance'] < 2**256
                    type(self).funding_balance = request['funding_balance']
                if 'funding_balance_error' in request:
                    assert type(request['funding_balance_error']) is bool
                    type(self).funding_balance_error = request['funding_balance_error']
                if 'blob_mode' in request:
                    assert request['blob_mode'] in ('disabled','valid','legacy','unavailable','pruned','corrupt','missing_metadata')
                    type(self).blob_mode = request['blob_mode']
                if 'protocol_logs' in request:
                    assert request['protocol_logs'] in ('missing','valid','malformed','conflict','third','limit')
                    type(self).protocol_logs = request['protocol_logs']
                if 'phase' in request:
                    assert request['phase'] in ('missing', 'healthy', 'fork', 'contradiction', 'wrong_identity', 'mid_fetch', 'pruned', 'wrong_type', 'invalid_post', 'unverified_post', 'transaction_unavailable', 'l2_fork')
                    type(self).phase = request['phase']
                if 'finalize' in request:
                    type(self).finalize = request['finalize']
                if 'correlation' in request:
                    assert request['correlation'] in ('none','valid','missing','wrong_anchor','wrong_hash','duplicate','reported_unfinalized')
                    type(self).correlation = request['correlation']
                if 'post_count' in request:
                    assert type(request['post_count']) is int and 1 <= request['post_count'] <= 32
                    type(self).post_count = request['post_count']
                if 'source_receipt_delay_ms' in request:
                    assert type(request['source_receipt_delay_ms']) is int and 0 <= request['source_receipt_delay_ms'] <= 2000
                    type(self).source_receipt_delay_ms = request['source_receipt_delay_ms']
                for field in ('multi_rollup','post_success','source_unselected','source_receipt_missing','source_cost_missing','source_hash_mismatch','source_receipt_failure'):
                    if field in request:
                        assert type(request[field]) is bool
                        setattr(type(self), field, request[field])
            self.send({'ok': True})
            return
        chain = 1 if self.path == '/l1' else 2
        with self.lock:
            funding_base_time, funding_balance, funding_balance_error = self.funding_base_time, self.funding_balance, self.funding_balance_error
            blob_mode = self.blob_mode
            protocol_logs = self.protocol_logs
            post_call = MULTI_ROLLUP_CALL if self.multi_rollup else POST_CALL
            phase, finalize = self.phase, self.finalize
            correlation, post_success, post_count = self.correlation, self.post_success, self.post_count
            source_unselected, source_receipt_missing = self.source_unselected, self.source_receipt_missing
            source_cost_missing, source_hash_mismatch = self.source_cost_missing, self.source_hash_mismatch
            source_receipt_failure, source_receipt_delay_ms = self.source_receipt_failure, self.source_receipt_delay_ms
        fork = phase == 'fork' or (phase == 'l2_fork' and chain == 2)
        def block_hash(height):
            return digest(chain * 100 + height + ((1000000 if funding_base_time else 10) if fork and height >= 2 else 0))
        def transaction(height, index=0):
            carrier = BLOB_FIXTURES[index % len(BLOB_FIXTURES)] if blob_mode != 'disabled' and chain == 1 and height == 2 else None
            selected_call = carrier['calldata'] if carrier else post_call
            tx = {'hash': digest(chain * 1000 + height + index), 'blockHash': block_hash(height),
                'blockNumber': hex(height), 'transactionIndex': hex(index), 'from': '0x' + ('cc' if source_unselected and chain == 1 and height == 2 else 'aa') * 20,
                'to': '0x' + 'bb' * 20, 'nonce': hex(height - 1 + index), 'type': '0x3' if height == 2 else '0x2', 'input': (selected_call[:10] if phase == 'invalid_post' else selected_call + '00' * 32 if phase == 'unverified_post' else selected_call) if height == 2 else '0x12345678'}
            if carrier and blob_mode != 'missing_metadata':
                tx['blobVersionedHashes'] = carrier['blob_hashes']
            return tx
        method, params = request['method'], request['params']
        latest_height = 3 + max(0, int(time.time()) - funding_base_time) // 15 if funding_base_time else 3
        if method == 'eth_chainId':
            value = hex(chain + (10 if phase == 'wrong_identity' else 0))
        elif method == 'eth_getBlockByNumber':
            selector, full = params
            height = latest_height if selector == 'latest' else (3 if finalize else 1) if selector == 'finalized' else int(selector, 16)
            if selector == 'finalized' and chain == 2:
                raise AssertionError('L2 RPC finality is not settlement evidence')
            timestamp = funding_base_time + ({0:-3610,1:-3600,2:-15}.get(height, (height - 3) * 15)) if funding_base_time else 100 + height * 5
            value = None if height > latest_height else {'number': hex(height), 'hash': block_hash(height),
                'parentHash': digest(0) if height == 0 else block_hash(height - 1), 'timestamp': hex(timestamp),
                'transactions': [(transaction(height, index) if full else transaction(height, index)['hash']) for index in range(post_count if chain == 1 and height == 2 else 1)] if height in (1, 2) else []}
            if value:
                blob_count = sum(len(BLOB_FIXTURES[index % len(BLOB_FIXTURES)]['blobs'])
                                 if blob_mode != 'disabled' and chain == 1 else 1
                                 for index in range(len(value['transactions']))) if height == 2 else 0
                value.update(gasUsed=hex(len(value['transactions']) * (22000 if height == 2 else 21000)),
                             gasLimit=hex(30000000), blobGasUsed=hex(131072 * blob_count), excessBlobGas='0x0')
            if value and not full and chain == 1 and height == 2 and source_hash_mismatch:
                value['transactions'] = [digest(9999)]
            if phase == 'pruned' and height == 2 and full:
                value = None
        elif method == 'eth_getBalance':
            assert funding_base_time is not None, 'balance fixture must be explicitly enabled'
            assert len(params) == 2 and params[1].get('requireCanonical') is True
            known = any(block_hash(height) == params[1].get('blockHash') for height in range(latest_height + 1))
            if funding_balance_error or not known:
                self.send({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32603,'message':'fixture balance unavailable'}})
                return
            value = hex(funding_balance)
        elif method == 'eth_getTransactionByHash':
            height = int(params[0], 16) - chain * 1000
            index = height - 2 if chain == 1 and 2 <= height < 2 + post_count else 0
            if index:
                height = 2
            value = transaction(height, index) if height in (1,2) and phase != 'transaction_unavailable' else None
        elif method == 'eth_getTransactionReceipt':
            if chain == 1 and source_receipt_delay_ms:
                time.sleep(source_receipt_delay_ms / 1000)
            if chain == 1 and source_receipt_failure and int(params[0],16) == 1002:
                self.send({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32603,'message':'fixture source receipt failure'}})
                return
            height = int(params[0], 16) - chain * 1000
            index = height - 2 if chain == 1 and 2 <= height < 2 + post_count else 0
            if index:
                height = 2
            if phase == 'mid_fetch' and height == 2 and chain == 1:
                with self.lock:
                    type(self).phase = 'fork'
                fork = True
            if height not in (1, 2) or ((phase == 'missing' or (chain == 1 and source_receipt_missing)) and height == 2):
                value = None
            else:
                tx = transaction(height, index)
                value = {key: tx[key] for key in ('hash', 'blockHash', 'blockNumber', 'transactionIndex', 'type')}
                value['transactionHash'] = value.pop('hash')
                value.update(status='0x0' if height == 2 and not (chain == 1 and post_success) else '0x1', gasUsed=hex(22000 if height == 2 else 21000),
                    effectiveGasPrice=hex((12 if fork else 9) if height == 2 else 7))
                if phase == 'wrong_type' and height == 2:
                    value['type'] = '0x2'
                if height == 2:
                    value.update(blobGasUsed=hex(131072 * (len(BLOB_FIXTURES[index % len(BLOB_FIXTURES)]['blobs']) if blob_mode != 'disabled' and chain == 1 else 1)), blobGasPrice='0x3')
                    if chain == 1 and source_cost_missing:
                        value.pop('blobGasPrice')
                if protocol_logs != 'missing':
                    value['logs'] = []
                    if chain == 1 and height == 2 and post_success:
                        base = {key:value[key] for key in ('blockHash','blockNumber','transactionHash','transactionIndex')}
                        base.update(address='0x' + 'bb'*20,removed=False)
                        root = {**base,'logIndex':hex(index*3),'topics':['0xc0b8d01bb696973c75a58d377245b23ce450af0ed8c4cf808cd0a3d85c6f3da1',digest(1)],'data':digest(202 if protocol_logs not in ('conflict','third') else 999 if protocol_logs=='conflict' else 998)}
                        def consumed(offset):
                            return {**base,'logIndex':hex(index*3+offset),'topics':['0xa17dc82da628b280737819918ee433b966773121ff974c081d8cbac4c6199d7d',digest(777),digest(1),digest(offset)],'data':'0x'}
                        value['logs'] = [root,consumed(1),consumed(2)]
                        if protocol_logs == 'malformed':
                            value['logs'][1]['blockHash'] = digest(999)
                        if protocol_logs == 'limit':
                            value['logs'] = [{**root,'logIndex':hex(i)} for i in range(4097)]
        elif method in ('eez_getSettledL2RangesByL1Block', 'eez_getSettledL2RangePage'):
            assert chain == 2
            l1_hash = digest(102 + (10 if phase == 'fork' else 0))
            value = []
            paged = method == 'eez_getSettledL2RangePage'
            selector = params[0]['l1BlockHash'] if paged else params[0].get('blockHash')
            if selector == l1_hash and correlation not in ('none','missing'):
                blocks = [{'number':'0x1','hash':digest(201)}, {'number':'0x2','hash':digest(212 if phase == 'fork' else 202)}]
                if correlation == 'wrong_hash':
                    blocks[1]['hash'] = digest(999)
                elif paged and correlation == 'duplicate':
                    blocks[1]['hash'] = blocks[0]['hash']
                for index in range(post_count):
                    value.append({'l1BlockNumber':'0x3' if correlation == 'wrong_anchor' else '0x2',
                        'l1BlockHash':l1_hash, 'l1TransactionHash':digest(1002 + index), 'l1TransactionIndex':hex(index),
                        'l2Range':{'firstBlockNumber':'0x1','firstBlockHash':blocks[0]['hash'],
                            'lastBlockNumber':'0x2','lastBlockHash':blocks[1]['hash'],'blockCount':'0x2'},
                        'l2Blocks':blocks, 'canonicalL2':True, 'l2Finalized':correlation != 'reported_unfinalized'})
                if correlation == 'duplicate':
                    value.append(value[0])
            if paged:
                query = params[0]
                assert set(query) == {'l1BlockHash','l1TransactionHash','offset','limit'}
                assert type(query['offset']) is int and 0 <= query['offset'] < 65536
                assert type(query['limit']) is int and 1 <= query['limit'] <= 1024
                matched = [v for v in value if v['l1TransactionHash'] == query['l1TransactionHash']]
                value = matched[0] if matched else None
                if value is not None:
                    offset, limit = query['offset'], query['limit']
                    blocks = value['l2Blocks']
                    assert offset < len(blocks)
                    key = ':'.join(str(v) for v in (int(value['l1BlockNumber'],16),int(l1_hash,16),int(blocks[-1]['hash'],16),int(value['l1TransactionIndex'],16)))
                    end = min(offset + limit,len(blocks))
                    value.update(offset=offset,nextOffset=end if end<len(blocks) else None,
                                 evidenceDigest=PAGE_DIGESTS[key],l2Blocks=blocks[offset:end])
        else:
            self.send({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': 'unsupported fixture method'}})
            return
        self.send({'jsonrpc': '2.0', 'id': request['id'], 'result': value})


http.server.ThreadingHTTPServer(('0.0.0.0', 9000), RPC).serve_forever()
