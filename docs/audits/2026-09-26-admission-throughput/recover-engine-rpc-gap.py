"""One-off bounded Engine backfill; imported payloads do not choose fork choice."""
import base64,datetime,fcntl,hashlib,hmac,json,os,subprocess,sys,time,urllib.request
from pathlib import Path
os.umask(0o077);root=Path.cwd();out=root/'artifacts/admission-throughput/engine-rpc-gap';out.mkdir(exist_ok=True)
mode=sys.argv[1];assert mode in ['prepare','import']
env=json.loads(subprocess.check_output(['bash','-c','set +x; set -a; source data/.env; python3 -c \'import os,json; print(json.dumps(dict(os.environ)))\'']))
external=env['EEZ_L1_RPC_URL'];local='http://127.0.0.1:18645';engine='http://127.0.0.1:18551'
secret=bytes.fromhex((root/'data/jwt.hex').read_text().strip().removeprefix('0x'))
def request(url,value=None):
 headers={'Content-Type':'application/json'}
 if url==engine:
  def b64(x):return base64.urlsafe_b64encode(x).rstrip(b'=')
  unsigned=b64(b'{"alg":"HS256","typ":"JWT"}')+b'.'+b64(json.dumps({'iat':int(time.time())}).encode())
  headers['Authorization']='Bearer '+(unsigned+b'.'+b64(hmac.digest(secret,unsigned,'sha256'))).decode()
 req=urllib.request.Request(url,None if value is None else json.dumps(value).encode(),headers)
 try:
  with urllib.request.urlopen(req,timeout=30) as f:return json.load(f)
 except Exception as e:raise RuntimeError('Transport '+type(e).__name__) from None
def rpc(url,m,p=[]):
 r=request(url,{'jsonrpc':'2.0','id':1,'method':m,'params':p})
 if 'error' in r:raise RuntimeError('RPC rejected '+m+': '+str(r['error'].get('code')))
 return r['result']
def cast(command,value):return subprocess.check_output(['cast',command,value],text=True).strip()
assert rpc(local,'eth_chainId')==rpc(external,'eth_chainId')=='0x27d8'
assert rpc(local,'eth_getBlockByNumber',['0x0',False])['hash']==rpc(external,'eth_getBlockByNumber',['0x0',False])['hash']=='0xada44fd8d2ecab8b08f256af07ad3e777f17fb434f8f8e678b312f576212ba9a'
if mode=='prepare':
 assert not (out/'prepared.json').exists()
 start=rpc(local,'eth_getBlockByNumber',['latest',False]);beacon=request('http://127.0.0.1:6152/eth/v2/beacon/blocks/head')
 tip=beacon['data']['message']['body']['execution_payload'];first=int(start['number'],16);last=int(tip['block_number']);assert 0<last-first<=512
 assert rpc(external,'eth_getBlockByNumber',[start['number'],False])['hash']==start['hash']
 previous=start['hash'];payloads=[]
 fields={'parentHash':'parentHash','feeRecipient':'miner','stateRoot':'stateRoot','receiptsRoot':'receiptsRoot','logsBloom':'logsBloom','prevRandao':'mixHash','blockNumber':'number','gasLimit':'gasLimit','gasUsed':'gasUsed','timestamp':'timestamp','extraData':'extraData','baseFeePerGas':'baseFeePerGas','blockHash':'hash','withdrawals':'withdrawals','blobGasUsed':'blobGasUsed','excessBlobGas':'excessBlobGas'}
 for offset in range(first+1,last+1,10):
  numbers=list(range(offset,min(offset+10,last+1)));queries=[]
  for n in numbers:
   queries.extend([{'jsonrpc':'2.0','id':n*2,'method':'eth_getBlockByNumber','params':[hex(n),True]},{'jsonrpc':'2.0','id':n*2+1,'method':'debug_getRawBlock','params':[hex(n)]}])
  rows=request(external,queries);by_id={r['id']:r for r in rows};assert len(by_id)==len(queries)
  for n in numbers:
   block=by_id[n*2]['result'];raw=by_id[n*2+1]['result'];body=json.loads(cast('from-rlp',raw));assert len(body)==4 and len(body[0])==21 and body[2]==[]
   assert cast('to-rlp',json.dumps(body))==raw
   assert cast('keccak',cast('to-rlp',json.dumps(body[0])))==block['hash']
   assert int(block['number'],16)==n and block['parentHash']==previous
   assert block['requestsHash']=='0x'+hashlib.sha256(b'').hexdigest(),'Nonempty execution requests require a verified request encoder'
   txs=[cast('to-rlp',json.dumps(t)) if isinstance(t,list) else t for t in body[1]]
   assert len(txs)==len(block['transactions'])
   assert [cast('keccak',t) for t in txs]==[t['hash'] for t in block['transactions']]
   payload={k:block[v] for k,v in fields.items()};payload['transactions']=txs
   hashes=[h for t in block['transactions'] for h in t.get('blobVersionedHashes',[])]
   assert int(block['blobGasUsed'],16)==len(hashes)*131072
   payloads.append({'number':n,'hash':block['hash'],'params':[payload,hashes,block['parentBeaconBlockRoot'],[]]});previous=block['hash']
  print('Verified raw canonical bodies',len(payloads),'/',last-first,flush=True)
 assert previous==tip['block_hash'],'Canonical execution target disagrees with local Beacon head'
 record={'prepared_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'start_block':first,'start_hash':start['hash'],'end_block':last,'end_hash':previous,'beacon_slot':beacon['data']['message']['slot'],'payloads':payloads}
 (out/'prepared.json').write_text(json.dumps(record)+'\n');print('Prepared',len(payloads),'payloads; no Engine mutation performed',flush=True)
else:
 lock=(root/'eez-core-protocol/tmp/.eez-network-e2e.lock').open('r');fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 assert not (out/'import-results.json').exists()
 record=json.loads((out/'prepared.json').read_text());payloads=record['payloads'];assert 0<len(payloads)<=512
 assert rpc(external,'eth_getBlockByNumber',[hex(record['end_block']),False])['hash']==record['end_hash']
 assert rpc(local,'eth_getBlockByNumber',[hex(record['start_block']),False])['hash']==record['start_hash']
 results=[]
 for p in payloads:
  result=rpc(engine,'engine_newPayloadV4',p['params']);results.append({'number':p['number'],'hash':p['hash'],'result':result})
  (out/'import-results.json').write_text(json.dumps(results,indent=2)+'\n')
  assert result['status']=='VALID' and result['latestValidHash']==p['hash'],result
  if len(results)%20==0:print('Engine validated',len(results),'/',len(payloads),flush=True)
 print('All',len(results),'payloads validated; Lighthouse retains fork-choice ownership',flush=True)
