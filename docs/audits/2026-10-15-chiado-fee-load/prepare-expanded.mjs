import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,appendFileSync,existsSync} from 'node:fs';
import {createRequire} from 'node:module';
import {randomBytes} from 'node:crypto';
const require=createRequire(`${process.env.EEZ_LOAD_UI_ROOT}/package.json`);
const {privateKeyToAccount}=require('viem/accounts');
const {encodeFunctionData,decodeFunctionResult,parseAbi,encodeAbiParameters,concat,keccak256,toHex,padHex}=require('viem');
const base=process.env.EEZ_LOAD_BASE, out=process.env.EEZ_LOAD_OUTPUT, root=process.cwd();
assert(JSON.parse(readFileSync(`${base}/verification.json`)).pass);
const old=JSON.parse(readFileSync(`${base}/private-state.json`));
const funder=privateKeyToAccount(process.env.SOURCE_PK), owner=privateKeyToAccount(old.owner);
const L1='http://127.0.0.1:18645',L2='http://127.0.0.1:18688',F2='http://127.0.0.1:18998';
const zero='0x0000000000000000000000000000000000000000';
const stringify=x=>JSON.stringify(x,(_,v)=>typeof v==='bigint'?v.toString():v);
const save=(name,x)=>writeFileSync(`${out}/${name}`,stringify(x)+'\n',{mode:0o600});
const progress=(message,x={})=>console.log(stringify({at:new Date().toISOString(),message,...x}));
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
let id=0;
async function rpc(url,method,params=[]){const r=await(await fetch(url,{method:'POST',headers:{'content-type':'application/json'},body:stringify({jsonrpc:'2.0',id:++id,method,params}),signal:AbortSignal.timeout(20000)})).json();assert(!r.error,stringify(r.error));return r.result;}
async function receipt(url,hash){for(let i=0;i<240;i++){const r=await rpc(url,'eth_getTransactionReceipt',[hash]);if(r){assert.equal(r.status,'0x1',hash);return r;}await sleep(1000);}throw Error('Receipt timeout '+hash);}
const call=(sig,args=[])=>encodeFunctionData({abi:parseAbi(['function '+sig]),args});
async function read(url,to,sig,args=[]){const abi=parseAbi(['function '+sig]);return decodeFunctionResult({abi,data:await rpc(url,'eth_call',[{to,data:encodeFunctionData({abi,args})},'latest'])});}
const state=existsSync(`${out}/private-state.json`)?JSON.parse(readFileSync(`${out}/private-state.json`)):{
 seed:(BigInt(Date.now())<<32n).toString(),owner:old.owner,quoteKey:old.quoteKey,quoteActor:old.quoteActor,
 fixtures:{...old.fixtures},setup:{},initialNested:0,prepared:false,
 actors:Array.from({length:256},(_,i)=>({key:toHex(randomBytes(32)),kind:['nested','safe_create','safe_nested','safe_transfer'][i%4],nonce:0,safeNonce:0,
 funding:[1000000000000000n,1200000000000000n,2800000000000000n,1000000000000000n][i%4].toString()}))};
const checkpoint=()=>save('private-state.json',state);checkpoint();
async function send(key,account,url,to,data,value,gas){
 if(state.setup[key])return receipt(url,state.setup[key]);
 const h=await rpc(url,'eth_getBlockByNumber',['latest',false]);const chainId=Number(BigInt(await rpc(url,'eth_chainId')));
 const nonce=Number(BigInt(await rpc(url,'eth_getTransactionCount',[account.address,'pending'])));
 const tip=chainId===10200?1000000000n:10000000n;
 const raw=await account.signTransaction({chainId,nonce,to,data,value,gas,maxPriorityFeePerGas:tip,maxFeePerGas:BigInt(h.baseFeePerGas)*2n+tip});
 const hash=keccak256(raw);state.setup[key]=hash;checkpoint();appendFileSync(`${out}/setup.jsonl`,stringify({key,hash,chainId,to,value})+'\n',{mode:0o600});
 assert.equal(await rpc(url,'eth_sendRawTransaction',[raw]),hash);return receipt(url,hash);
}
const ordinary=(key,url,to,data='0x',value=0n,gas=16500000n)=>send(key,funder,url,to,data,value,gas);
const pack=items=>concat(items.map(({to,data='0x',value=0n})=>concat(['0x00',to,toHex(value,{size:32}),toHex((data.length-2)/2,{size:32}),data])));
async function multisend(key,url,safe,multisend,items,value=0n){
 if(state.setup[key])return receipt(url,state.setup[key]);
 const chainId=Number(BigInt(await rpc(url,'eth_chainId')));const nonce=await read(url,safe,'nonce() view returns (uint256)');
 const data=call('multiSend(bytes)',[pack(items)]);
 const hash=await read(url,safe,'getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256) view returns (bytes32)',[multisend,0n,data,1,0n,0n,0n,zero,zero,nonce]);
 const signature=await owner.sign({hash});
 return ordinary(key,url,safe,call('execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes) returns (bool)',[multisend,0n,data,1,0n,0n,0n,zero,zero,signature]),value);
}
async function deploy(key,url,bytecode){if(state.fixtures[key])return state.fixtures[key];const r=await ordinary(key,url,undefined,bytecode,0n,7000000n);state.fixtures[key]=r.contractAddress;checkpoint();return r.contractAddress;}
const setup=()=>call('setup(address[],uint256,address,bytes,address,address,uint256,address)',[[owner.address],1n,zero,'0x',zero,zero,0n,zero]);
function prediction(factory,singleton,code,init,salt){return getCreate2Address({from:factory,salt:keccak256(encodeAbiParameters([{type:'bytes32'},{type:'uint256'}],[keccak256(init),salt])),bytecode:concat([code,padHex(singleton,{size:32})])});}
const {getCreate2Address}=require('viem');
const queue=await rpc(F2,'eez_getCrossChainQueueStatus');assert(['queued','inFlight','blocked','unknown'].every(k=>queue[k]===0));
progress('Reusing unused test balances after successful primary verification');
for(let offset=0;offset<old.actors.length;offset+=8)await Promise.all(old.actors.slice(offset,offset+8).map(async(a,j)=>{
 const key=`recover-${offset+j}`;if(state.setup[key])return receipt(L2,state.setup[key]);
 const account=privateKeyToAccount(a.key);const nonce=await rpc(L2,'eth_getTransactionCount',[account.address,'latest']);
 assert.equal(nonce,await rpc(L2,'eth_getTransactionCount',[account.address,'pending']));
 assert.equal(Number(BigInt(nonce)),a.nonce);
 const h=await rpc(L2,'eth_getBlockByNumber',['latest',false]);const fee=(BigInt(h.baseFeePerGas)*2n+10000000n)*21000n;
 const balance=BigInt(await rpc(L2,'eth_getBalance',[account.address,'latest']));assert(balance>fee);
 return send(key,account,L2,funder.address,'0x',balance-fee,21000n);
}));
const needed=state.actors.reduce((n,a)=>n+BigInt(a.funding),0n);
assert(BigInt(await rpc(L2,'eth_getBalance',[funder.address,'latest']))>needed+1000000000000000n,'Insufficient reused test funds');
progress('Recovered unused test funds; creating 64 new transfer Safes in one Safe MultiSend');
const init=setup(),items=[];
for(let i=0;i<state.actors.length;i++){
 const a=state.actors[i];if(a.kind!=='safe_transfer')continue;
 const salt=BigInt(state.seed)+50000000n+BigInt(i);a.safe=prediction(state.fixtures.factory,state.fixtures.singleton,state.fixtures.proxyCode,init,salt);
 a.recipient??=privateKeyToAccount(toHex(randomBytes(32))).address;
 items.push({to:state.fixtures.factory,data:call('createProxyWithNonce(address,bytes,uint256)',[state.fixtures.singleton,init,salt])},{to:a.safe,value:10000n});
}checkpoint();
await multisend('expanded-safes',L1,old.quoteActor.safe,state.fixtures.multisend,items,640000n);
state.quoteActor.safeNonce=Number(await read(L1,old.quoteActor.safe,'nonce() view returns (uint256)'));checkpoint();
const fixture=JSON.parse(readFileSync(`${root}/crates/eez-node-e2e/tests/fixtures/safe-1.4.1/bytecode.json`)).contracts;
for(const [key,name]of[['l2Singleton','Safe'],['l2Factory','SafeProxyFactory'],['l2MultiSend','MultiSend']]){
 const a=await deploy(key,L2,fixture[name].bytecode);if(fixture[name].runtimeHash)assert.equal(keccak256(await rpc(L2,'eth_getCode',[a,'latest'])),fixture[name].runtimeHash);
}
const code=await read(L2,state.fixtures.l2Factory,'proxyCreationCode() pure returns (bytes)');const salt=BigInt(state.seed)+100000000n;
const distributor=prediction(state.fixtures.l2Factory,state.fixtures.l2Singleton,code,init,salt);
await ordinary('l2-distributor',L2,state.fixtures.l2Factory,call('createProxyWithNonce(address,bytes,uint256)',[state.fixtures.l2Singleton,init,salt]));
const proxies=[];
for(const a of state.actors){if(a.kind!=='safe_transfer')continue;a.safeProxy=await read(L2,process.env.EEZL2_ADDRESS,'computeCrossChainProxyAddress(address,uint64) view returns (address)',[a.safe,0n]);proxies.push({to:process.env.EEZL2_ADDRESS,data:call('createCrossChainProxy(address,uint64)',[a.safe,0n])});}
checkpoint();
for (let offset=0;offset<proxies.length;offset+=16) await multisend(`expanded-proxies-${offset}`,L2,distributor,state.fixtures.l2MultiSend,proxies.slice(offset,offset+16));
const funded=await multisend('expanded-fund',L2,distributor,state.fixtures.l2MultiSend,state.actors.map(a=>({to:privateKeyToAccount(a.key).address,value:BigInt(a.funding)})),needed);
for(let i=0;i<240;i++){
 const r=await rpc(L2,'eth_getTransactionReceipt',[funded.transactionHash]);const safe=await rpc(L2,'eth_getBlockByNumber',['safe',false]);
 if(r&&BigInt(safe.number)>=BigInt(r.blockNumber))break;
 assert(i<239,'Expanded funding did not settle');await sleep(1000);
}
state.initialNested=Number(await read(L2,state.fixtures.counter,'counter() view returns (uint256)'));
state.prepared=true;checkpoint();
save('fixtures.json',{...state.fixtures,owner:owner.address,initialNested:state.initialNested,actors:state.actors.map(a=>({address:privateKeyToAccount(a.key).address,kind:a.kind,safe:a.safe,recipient:a.recipient,funding:a.funding}))});
progress('256 independent actors ready; no additional L1 to L2 funding', {funded:needed});
