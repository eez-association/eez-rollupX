import { test } from "node:test";
import assert from "node:assert/strict";
import { requestFeeQuote, feeQuoteTransaction, sendFeeQuotedTransaction } from "./l2-fees.mjs";

function compactFixture(type = "0x2") {
  const old = fixture();
  const intent = { from: old.intent.from, to: old.intent.to, data: old.intent.data };
  // Independent cast keccak vectors of the normalized JSON; no server fixture
  // determines which destination/calldata the SDK is allowed to send.
  const hashes = { "0x0":"0x873184abbd3d864413844c1aad934f779432aba75064a5ed1c7060547c249712",
    "0x2":"0xb027810c300a8577d3d15e441e23fad18defd8cf8f1485f2c42dfcaa015b3c31" };
  const keccak256 = text => {
    for (const type of ["0x0", "0x2"]) {
      const normalized = { ...old.intent, type };
      const expected = JSON.stringify(Object.fromEntries(Object.keys(normalized).sort().map(k => [k, normalized[k]])));
      if (text === expected) return hashes[type];
    }
    return `0x${"00".repeat(32)}`;
  };
  const options = ["0x0", "0x2"].map(type => ({ type, gas:old.intent.gas, intentHash:hashes[type], estimatedFee:"0x4ce78",
    ...(type === "0x0" ? {gasPrice:"0xf"} : {maxPriorityFeePerGas:"0x5",maxFeePerGas:"0xf"}) }));
  const quote = { version:1, chainId:"0x1",nonce:"0x0",expiresAt:`0x${old.quote.expiresAt.toString(16)}`,recoveryMode:"enforced_tariff",options };
  return { intent, quote, type, chainId:"0x1", keccak256, maxTotalFee:1500000n };
}

test("compact quotes use one RPC round trip and preserve requested type selection", async () => {
  const args = compactFixture(); let calls = 0;
  const rpc = async (method, params) => {
    calls++; assert.equal(method, "eez_quoteFee");
    assert.deepEqual(params, [args.intent, {transactionTypes:["0x0","0x2"]}]);
    return args.quote;
  };
  assert.equal(await requestFeeQuote({rpc,intent:args.intent}), args.quote);
  assert.equal(calls, 1);
  await assert.rejects(requestFeeQuote({rpc, intent:args.intent,transactionTypes:[]}), /transactionTypes/);
  await assert.rejects(requestFeeQuote({rpc:async () => ({...args.quote,options:[args.quote.options[0]]}),intent:args.intent}), /requested transaction types/);
});

test("compact options bind resolved gas, nonce and type without accepting replacement intent", () => {
  for (const type of ["0x0","0x2"]) {
    const args = compactFixture(type);
    const tx = feeQuoteTransaction(args);
    assert.equal(tx.type, type); assert.equal(tx.gas, "0x186a0"); assert.equal(tx.nonce,"0x0");
    assert.equal(tx.to,args.intent.to); assert.equal(tx.data,args.intent.data);
    assert.equal("gasPrice" in tx, type === "0x0"); assert.equal("maxFeePerGas" in tx,type === "0x2");
    assert.throws(() => feeQuoteTransaction({...args, maxTotalFee:1499999n}), /app limit/);
    assert.throws(() => feeQuoteTransaction({...args, chainId:"0x2"}), /chain mismatch/);
    assert.throws(() => feeQuoteTransaction({...args, intent:{...args.intent,nonce:"0x1"}}), /changed nonce/);
    assert.throws(() => feeQuoteTransaction({...args, intent:{...args.intent,gas:"0x186a1"}}), /changed gas/);
    assert.throws(() => feeQuoteTransaction({...args, intent:{...args.intent,data:"0x5678"}}), /binding mismatch/);
    for (const mutate of [q => q.expiresAt="0x0", q => q.nonce="0x1", q => q.options[0].type="0x2",
      q => q.options.find(o=>o.type===type).gas="0x1000001",
      q => q.options.find(o=>o.type===type).to=args.intent.to,
      q => q.options.find(o=>o.type===type).estimatedFee="0xffffffffffff",
      q => q.options.find(o=>o.type===type).error={code:"unsupported_intent"}]) {
      const bad = compactFixture(type); mutate(bad.quote); assert.throws(() => feeQuoteTransaction(bad));
    }
  }
});

test("compact wallet send checks account and nonce then sends exactly one selected transaction", async () => {
  const args = compactFixture("0x0"); let sent = 0;
  const wallet = {request:async ({method,params}) => {
    if (method === "eth_sendTransaction") { sent++; assert.deepEqual(params,[feeQuoteTransaction(args)]); return "0xhash"; }
    return {eth_chainId:1,eth_accounts:[args.intent.from],eth_getTransactionCount:"0x00"}[method];
  }};
  assert.equal(await sendFeeQuotedTransaction({...args,wallet}),"0xhash"); assert.equal(sent,1);
  await assert.rejects(sendFeeQuotedTransaction({...args,wallet:{request:async ({method}) => {
    assert.notEqual(method,"eth_sendTransaction");
    return {eth_chainId:"0x1",eth_accounts:[args.intent.from],eth_getTransactionCount:"0x1"}[method];
  }}}), /nonce changed/);
});

function fixture() {
  const intent = { type:"0x2",chainId:"0x1",from:`0x${"11".repeat(20)}`,to:`0x${"22".repeat(20)}`,nonce:"0x0",gas:"0x186a0",value:"0x0",data:"0x1234" };
  const quote = { version:1,mode:"advisory",intent:{...intent},transaction:{...intent,maxPriorityFeePerGas:"0x5",maxFeePerGas:"0xf"},
    issuedAt:Math.floor(Date.now()/1000),expiresAt:Math.floor(Date.now()/1000)+10,
    context:{l2ChainId:1,rollupId:1},policy:{id:`0x${"ab".repeat(32)}`,assumptions:{l2_chain_id:1,rollup_id:1}},
    estimate:{basis:"isolated_native_l2_replay_and_projected_l1_post",sourceGasUsed:21000,l2BaseFeePerGas:"10",grossL2Tip:"105000",requiredL2Tip:"100001"} };
  return {intent,quote,maxTotalFee:1500000n};
}

test("compact gas profile accepts measured Falcon limits and rejects malformed quantities", () => {
  for (const type of ["0x0", "0x2"]) {
    for (const gas of [21_000n, 6_050_247n, 6_037_754n, 16_777_216n, 20_999n, 16_777_217n]) {
      const args = compactFixture(type);
      const option = args.quote.options.find(option => option.type === type);
      option.gas = `0x${gas.toString(16)}`;
      // Isolate resource-bound checking; the separate binding tests use fixed
      // independently computed Keccak vectors, including intent mutations.
      args.keccak256 = () => option.intentHash;
      args.maxTotalFee = gas * 15n;
      if (gas >= 21_000n && gas <= 16_777_216n) {
        assert.equal(feeQuoteTransaction(args).gas, option.gas);
        assert.throws(() => feeQuoteTransaction({...args,maxTotalFee:args.maxTotalFee-1n}), /app limit/);
      } else assert.throws(() => feeQuoteTransaction(args), /Gas outside quote profile/);
    }
    for (const value of ["0x00", "0x", 0, "1", null]) {
      const args = compactFixture(type); args.quote.nonce = value;
      assert.throws(() => feeQuoteTransaction(args), /canonical hex quantity/);
    }
  }
});

test("compact sends reject changed wallet chain/account and expire during preflight", async () => {
  for (const changed of ["chain", "account", "expiry"]) {
    const args = compactFixture();
    const wallet = { request: async ({method}) => {
      assert.notEqual(method,"eth_sendTransaction");
      if (changed === "expiry") args.quote.expiresAt = "0x0";
      return {eth_chainId: changed === "chain" ? "0x2" : "0x1",
        eth_accounts:[changed === "account" ? args.intent.to : args.intent.from],eth_getTransactionCount:"0x0"}[method];
    }};
    await assert.rejects(sendFeeQuotedTransaction({...args,wallet}), /different chain|account changed|expired/);
  }
});
