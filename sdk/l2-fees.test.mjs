import { test } from "node:test";
import assert from "node:assert/strict";
import { quotedTransaction, requestL2FeeQuote, sendQuotedTransaction } from "./l2-fees.mjs";

function fixture() {
  const intent = { type:"0x2",chainId:"0x1",from:`0x${"11".repeat(20)}`,to:`0x${"22".repeat(20)}`,nonce:"0x0",gas:"0x186a0",value:"0x0",data:"0x1234" };
  const quote = { version:1,mode:"advisory",intent:{...intent},transaction:{...intent,maxPriorityFeePerGas:"0x5",maxFeePerGas:"0xf"},
    issuedAt:Math.floor(Date.now()/1000),expiresAt:Math.floor(Date.now()/1000)+10,
    context:{l2ChainId:1,rollupId:1},policy:{id:`0x${"ab".repeat(32)}`,assumptions:{l2_chain_id:1,rollup_id:1}},
    estimate:{basis:"isolated_native_l2_replay_and_projected_l1_post",sourceGasUsed:21000,l2BaseFeePerGas:"10",grossL2Tip:"105000",requiredL2Tip:"100001"} };
  return {intent,quote,maxTotalFee:1500000n};
}

function legacyFixture() {
  const args = fixture();
  args.intent.type = "0x0";
  args.quote.intent = { ...args.intent };
  args.quote.transaction = { ...args.intent, gasPrice: "0xf" };
  return args;
}

test("legacy quotes count only gasPrice minus base fee and preserve the fee ceiling", async () => {
  const args = legacyFixture();
  assert.deepEqual(quotedTransaction(args), args.quote.transaction);
  assert.throws(() => quotedTransaction({ ...args, maxTotalFee: 1499999n }), /app limit/);
  for (const mutate of [
    q => q.transaction.gasPrice = "0x9",
    q => q.transaction.maxFeePerGas = "0xf",
    q => q.transaction.maxPriorityFeePerGas = "0x5",
    q => q.transaction.type = "0x2",
    q => q.estimate.grossL2Tip = "315000", // Incorrectly includes the burned base fee.
  ]) {
    const bad = legacyFixture();
    mutate(bad.quote);
    assert.throws(() => quotedTransaction(bad));
  }
  let sent = 0;
  const wallet = { request: async ({ method, params }) => {
    if (method === "eth_sendTransaction") {
      sent++;
      assert.deepEqual(params, [args.quote.transaction]);
      return "0xhash";
    }
    return { eth_chainId: "0x1", eth_accounts: [args.intent.from], eth_getTransactionCount: "0x0", eth_getBlockByNumber: { baseFeePerGas: "0xa" } }[method];
  } };
  assert.equal(await sendQuotedTransaction({ ...args, wallet }), "0xhash");
  assert.equal(sent, 1);
  wallet.request = async ({ method }) => {
    assert.notEqual(method, "eth_sendTransaction");
    return { eth_chainId: "0x1", eth_accounts: [args.intent.from], eth_getTransactionCount: "0x0", eth_getBlockByNumber: { baseFeePerGas: "0xb" } }[method];
  };
  await assert.rejects(sendQuotedTransaction({ ...args, wallet }), /Base fee changed/);
});

test("custom RPC quotes use the same validation and never silently fall back", async () => {
  for (const args of [fixture(), legacyFixture()]) {
    const rpc = async (method, params) => {
      assert.equal(method, "eez_quoteL2ToL1Fee");
      assert.deepEqual(params, [args.intent]);
      return args.quote;
    };
    assert.equal(await requestL2FeeQuote({ ...args, rpc }), args.quote);
    await assert.rejects(requestL2FeeQuote({ ...args, rpc, url: "https://example.invalid" }), /either/);
    await assert.rejects(requestL2FeeQuote({ ...args, rpc: async () => { throw new Error("quotes_disabled"); } }), /quotes_disabled/);
    await assert.rejects(requestL2FeeQuote({ ...args, rpc: async () => ({ ...args.quote, expiresAt: 0 }) }), /expired/);
  }
});

test("returns only the exact intent and ordinary fee fields",()=>{
  const args=fixture(); const tx=quotedTransaction(args);
  assert.deepEqual(tx,args.quote.transaction); assert.notEqual(tx,args.quote.transaction);
  assert.throws(()=>quotedTransaction({...args,maxTotalFee:1499999n}),/app limit/);
});

test("rejects intent changes, extra authority, expiry and inconsistent pricing",()=>{
  const mutations=[q=>q.transaction.to=`0x${"33".repeat(20)}`,q=>q.transaction.gas="0x186a1",
    q=>q.transaction.authorizationList=[],q=>q.intent.nonce="0x1",q=>q.context.l2ChainId=2,
    q=>q.estimate.sourceGasUsed=0,q=>q.estimate.grossL2Tip="1",q=>q.estimate.requiredL2Tip="105001",
    q=>q.transaction.maxFeePerGas="0xe",q=>q.expiresAt=q.issuedAt,q=>q.mode="guaranteed"];
  for(const mutate of mutations){const args=fixture();mutate(args.quote);assert.throws(()=>quotedTransaction(args));}
  const args=fixture();assert.throws(()=>quotedTransaction({...args,now:args.quote.expiresAt*1000}),/expired/);
  assert.throws(()=>quotedTransaction({...args,expectedPolicyId:`0x${"cd".repeat(32)}`}),/policy/);
});

test("fetches without requesting a wallet signature",async()=>{
  const args=fixture();let called=0;
  const quote=await requestL2FeeQuote({...args,url:"https://example.invalid/eez/fees/l2-to-l1",fetchImpl:async(url,req)=>{
    called++;assert.equal(req.method,"POST");assert.deepEqual(JSON.parse(req.body),args.intent);return {ok:true,json:async()=>args.quote};
  }});assert.equal(called,1);assert.equal(quote,args.quote);
});

test("one normal wallet submission after chain, account and nonce checks",async()=>{
  const args=fixture();const methods=[];
  const wallet={request:async({method,params})=>{methods.push(method);switch(method){
    case "eth_chainId":return "0x1";case "eth_accounts":return [args.intent.from];case "eth_getTransactionCount":return "0x0";
    case "eth_getBlockByNumber":return {baseFeePerGas:"0xa"};
    case "eth_sendTransaction":assert.deepEqual(params,[args.quote.transaction]);return "0xhash";
    default:throw new Error(`Unexpected ${method}`);
  }}};
  assert.equal(await sendQuotedTransaction({...args,wallet}),"0xhash");
  assert.equal(methods.filter(m=>m==="eth_sendTransaction").length,1);
});

test("wrong wallet chain, nonce or changed base fee cannot submit",async()=>{
  for(const wrong of ["eth_chainId","eth_getTransactionCount","eth_getBlockByNumber"]){
    const args=fixture();let sent=false;const wallet={request:async({method})=>{
      if(method==="eth_sendTransaction"){sent=true;return "0xhash";}
      if(method===wrong)return method==="eth_getBlockByNumber"?{baseFeePerGas:"0xb"}:"0x2";
      return {eth_chainId:"0x1",eth_getTransactionCount:"0x0",eth_accounts:[args.intent.from],eth_getBlockByNumber:{baseFeePerGas:"0xa"}}[method];
    }};await assert.rejects(sendQuotedTransaction({...args,wallet}));assert.equal(sent,false);
  }
});
