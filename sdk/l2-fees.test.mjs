import { test } from "node:test";
import assert from "node:assert/strict";
import { requestFeeQuote, feeQuoteTransaction, sendFeeQuotedTransaction } from "./l2-fees.mjs";

function compactFixture(type = "0x2") {
  const source = { type:"0x2",chainId:"0x1",from:`0x${"11".repeat(20)}`,to:`0x${"22".repeat(20)}`,nonce:"0x0",gas:"0x186a0",value:"0x0",data:"0x1234" };
  const transaction = { from: source.from, to: source.to, data: source.data };
  // Independent cast keccak vectors of the normalized JSON; no server fixture
  // determines which destination/calldata the SDK is allowed to send.
  const hashes = { "0x0":"0x873184abbd3d864413844c1aad934f779432aba75064a5ed1c7060547c249712",
    "0x2":"0xb027810c300a8577d3d15e441e23fad18defd8cf8f1485f2c42dfcaa015b3c31" };
  const keccak256 = text => {
    for (const type of ["0x0", "0x2"]) {
      const normalized = { ...source, type };
      const expected = JSON.stringify(Object.fromEntries(Object.keys(normalized).sort().map(k => [k, normalized[k]])));
      if (text === expected) return hashes[type];
    }
    return `0x${"00".repeat(32)}`;
  };
  const options = ["0x0", "0x2"].map(type => ({ type, gas:source.gas, requestHash:hashes[type], estimatedFee:"0x4ce78",
    ...(type === "0x0" ? {gasPrice:"0xf"} : {maxPriorityFeePerGas:"0x5",maxFeePerGas:"0xf"}) }));
  const quote = { version:1, chainId:"0x1",nonce:"0x0",expiresAt:`0x${(Math.floor(Date.now()/1000)+30).toString(16)}`,options };
  return { transaction, quote, type, chainId:"0x1", keccak256, maxTotalFee:1500000n };
}

test("compact quotes use one RPC round trip and preserve requested type selection", async () => {
  const args = compactFixture(); let calls = 0;
  const rpc = async (method, params) => {
    calls++; assert.equal(method, "eez_quoteFee");
    assert.deepEqual(params, [args.transaction, {transactionTypes:["0x0","0x2"]}]);
    return args.quote;
  };
  assert.equal(await requestFeeQuote({rpc,transaction:args.transaction}), args.quote);
  assert.equal(calls, 1);
  await assert.rejects(requestFeeQuote({rpc, transaction:args.transaction,transactionTypes:[]}), /transactionTypes/);
  await assert.rejects(requestFeeQuote({rpc:async () => ({...args.quote,options:[args.quote.options[0]]}),transaction:args.transaction}), /requested transaction types/);
});

test("compact options bind resolved gas, nonce and type without accepting replacement transaction", () => {
  for (const type of ["0x0","0x2"]) {
    const args = compactFixture(type);
    const tx = feeQuoteTransaction(args);
    assert.equal(args.quote.recoveryMode, undefined);
    for (const recoveryMode of ["advisory", "enforced_tariff"]) {
      assert.deepEqual(feeQuoteTransaction({...args, quote:{...args.quote,diagnostics:{recoveryMode}}}), tx);
    }
    assert.equal(tx.type, type); assert.equal(tx.gas, "0x186a0"); assert.equal(tx.nonce,"0x0");
    assert.equal(tx.to,args.transaction.to); assert.equal(tx.data,args.transaction.data);
    assert.equal("gasPrice" in tx, type === "0x0"); assert.equal("maxFeePerGas" in tx,type === "0x2");
    assert.throws(() => feeQuoteTransaction({...args, maxTotalFee:1499999n}), /app limit/);
    assert.throws(() => feeQuoteTransaction({...args, chainId:"0x2"}), /chain mismatch/);
    assert.throws(() => feeQuoteTransaction({...args, transaction:{...args.transaction,nonce:"0x1"}}), /changed nonce/);
    assert.throws(() => feeQuoteTransaction({...args, transaction:{...args.transaction,gas:"0x186a1"}}), /changed gas/);
    assert.throws(() => feeQuoteTransaction({...args, transaction:{...args.transaction,data:"0x5678"}}), /binding mismatch/);
    for (const mutate of [q => q.expiresAt="0x0", q => q.nonce="0x1", q => q.options[0].type="0x2",
      q => q.options.find(o=>o.type===type).gas="0x1000001",
      q => q.options.find(o=>o.type===type).to=args.transaction.to,
      q => q.options.find(o=>o.type===type).estimatedFee="0xffffffffffff",
      q => q.options.find(o=>o.type===type).error={code:"unsupported_transaction"}]) {
      const bad = compactFixture(type); mutate(bad.quote); assert.throws(() => feeQuoteTransaction(bad));
    }
  }
});

test("compact wallet send checks account and nonce then sends exactly one selected transaction", async () => {
  const args = compactFixture("0x0"); let sent = 0;
  const wallet = {request:async ({method,params}) => {
    if (method === "eth_sendTransaction") { sent++; assert.deepEqual(params,[feeQuoteTransaction(args)]); return "0xhash"; }
    return {eth_chainId:1,eth_accounts:[args.transaction.from],eth_getTransactionCount:"0x00"}[method];
  }};
  assert.equal(await sendFeeQuotedTransaction({...args,wallet}),"0xhash"); assert.equal(sent,1);
  await assert.rejects(sendFeeQuotedTransaction({...args,wallet:{request:async ({method}) => {
    assert.notEqual(method,"eth_sendTransaction");
    return {eth_chainId:"0x1",eth_accounts:[args.transaction.from],eth_getTransactionCount:"0x1"}[method];
  }}}), /nonce changed/);
});

test("compact gas profile accepts measured Falcon limits and rejects malformed quantities", () => {
  for (const type of ["0x0", "0x2"]) {
    for (const gas of [21_000n, 6_050_247n, 6_037_754n, 16_777_216n, 20_999n, 16_777_217n]) {
      const args = compactFixture(type);
      const option = args.quote.options.find(option => option.type === type);
      option.gas = `0x${gas.toString(16)}`;
      // Isolate resource-bound checking; the separate binding tests use fixed
      // independently computed Keccak vectors, including transaction mutations.
      args.keccak256 = () => option.requestHash;
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
        eth_accounts:[changed === "account" ? args.transaction.to : args.transaction.from],eth_getTransactionCount:"0x0"}[method];
    }};
    await assert.rejects(sendFeeQuotedTransaction({...args,wallet}), /different chain|account changed|expired/);
  }
});

test("advisory quotes allow thirty seconds but reject expiry and implausible clock skew", () => {
  const now = 2_000_000_000_000;
  for (const type of ["0x0", "0x2"]) {
    for (const remaining of [30, 1, 0, -1, 32]) {
      const args = compactFixture(type);
      args.quote.expiresAt = `0x${(BigInt(now / 1000) + BigInt(remaining)).toString(16)}`;
      if (remaining > 0 && remaining <= 30) assert.equal(feeQuoteTransaction({...args, now}).type, type);
      else assert.throws(() => feeQuoteTransaction({...args, now}), /expired or invalid clock/);
    }
  }
});
