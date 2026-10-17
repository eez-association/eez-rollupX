#!/usr/bin/env node
// Real PBS qualification: ordered native settlement, required user source and
// an independently observed builder payment. Restricted to a disposable devnet.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { feeQuoteTransaction, sendFeeQuotedTransaction } from "../../../sdk/l2-fees.mjs";

const env = name => { assert(process.env[name], `${name} is required`); return process.env[name]; };
const l1 = env("EEZ_DEVNET_L1_RPC"), l2 = env("EEZ_DEVNET_L2_RPC"), front = env("EEZ_DEVNET_L1_FRONT");
const registry = env("EEZ_REGISTRY_ADDRESS"), key = env("EEZ_TEST_FEE_KEY"), output = env("EEZ_FEE_EVIDENCE_FILE");
const zero = "0x0000000000000000000000000000000000000000";
const hex = value => `0x${BigInt(value).toString(16)}`;
const evidence = { sources: [], fixtures: {} };
const checkpoint = () => writeFileSync(output, `${JSON.stringify(evidence, null, 2)}\n`);
let id = 0;
function cast(...args) {
  try { return execFileSync("cast", args, { encoding: "utf8", timeout: 30_000, stdio: ["ignore", "pipe", "pipe"] }).trim(); }
  catch { throw new Error(`cast ${args[0]} failed`); }
}
const from = cast("wallet", "address", "--private-key", key);
async function request(url, method, params = []) {
  const response = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }), signal: AbortSignal.timeout(20_000) });
  assert(response.ok, `RPC HTTP ${response.status}`);
  return response.json();
}
async function rpc(url, method, params = []) {
  const value = await request(url, method, params);
  assert(!value.error, `${method}: ${JSON.stringify(value.error)}`);
  return value.result;
}
async function until(label, check, seconds = 180) {
  const deadline = Date.now() + seconds * 1000;
  while (Date.now() < deadline) {
    const value = await check();
    if (value !== undefined && value !== null && value !== false) return value;
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  throw new Error(`Timed out: ${label}`);
}
const retryable = result => ["stale_context", "quote_unavailable", "quote_busy"].includes(result.error?.data?.code);
function sign(tx, signingKey = key) {
  const args = ["mktx", "--private-key", signingKey, "--chain", String(BigInt(tx.chainId)), "--nonce", String(BigInt(tx.nonce)),
    "--gas-limit", String(BigInt(tx.gas)), "--value", String(BigInt(tx.value ?? "0x0"))];
  if (tx.type === "0x0") args.push("--legacy", "--gas-price", String(BigInt(tx.gasPrice)));
  else args.push("--gas-price", String(BigInt(tx.maxFeePerGas)), "--priority-gas-price", String(BigInt(tx.maxPriorityFeePerGas)));
  if (tx.to) args.push(tx.to, tx.data); else args.push("--create", tx.data);
  return cast(...args);
}
async function receipt(url, hash) {
  const result = await until(`receipt ${hash}`, () => rpc(url, "eth_getTransactionReceipt", [hash]));
  assert.equal(result.status, "0x1", `reverted ${hash}`);
  return result;
}
async function ordinary(url, to, data = "0x", value = "0x0") {
  const [chainId, nonce, parent] = await Promise.all([rpc(url, "eth_chainId"),
    rpc(url, "eth_getTransactionCount", [from, "pending"]), rpc(url, "eth_getBlockByNumber", ["latest", false])]);
  const tip = 1_000_000_000n;
  const raw = sign({ to, data, value, chainId, nonce, gas: hex(8_000_000), maxPriorityFeePerGas: hex(tip),
    maxFeePerGas: hex(2n * BigInt(parent.baseFeePerGas) + tip) });
  return receipt(url, await rpc(url, "eth_sendRawTransaction", [raw]));
}
async function deploy(url, bytecode, args = "0x") {
  const result = await ordinary(url, null, bytecode + args.slice(2));
  assert(result.contractAddress);
  return result.contractAddress;
}
const artifact = name => JSON.parse(readFileSync(`contracts/out/${name}`, "utf8")).bytecode.object;
async function proxy(target) {
  const encoded = await rpc(l1, "eth_call", [{ to: registry,
    data: cast("calldata", "computeCrossChainProxyAddress(address,uint64)", target, "1") }, "latest"]);
  await ordinary(l1, registry, cast("calldata", "createCrossChainProxy(address,uint64)", target, "1"));
  return `0x${encoded.slice(-40)}`;
}
async function read(url, to, signature, block = "latest") {
  return rpc(url, "eth_call", [{ to, data: cast("calldata", signature) }, block]);
}
const quoteArgs = (quote, transaction, type) => ({ quote, transaction, type, chainId: hex(7331),
  maxTotalFee: 10n ** 18n, keccak256: text => cast("keccak", text) });
async function quote(transaction) {
  return until("L1 quote", async () => {
    const result = await request(front, "eez_debugQuoteFee", [transaction]);
    if (retryable(result)) return null;
    assert(!result.error, JSON.stringify(result.error));
    assert.deepEqual(result.result.options.map(o => o.type), ["0x0", "0x2"]);
    for (const option of result.result.options) assert(!option.error, JSON.stringify(option));
    return result.result;
  }, 60);
}

assert.equal(BigInt(await rpc(l1, "eth_chainId")), 7331n, "requires private Kurtosis L1");
assert.equal(BigInt(await rpc(l2, "eth_chainId")), 696990n, "requires private Kurtosis L2");
await until("initial canonical settlement", async () =>
  BigInt((await rpc(l2, "eth_getBlockByNumber", ["safe", false])).number) > 0n, 600);
const target = await deploy(l2, artifact("Value.sol/Value.json"), cast("abi-encode", "f(uint256)", "0"));
const targetProxy = await proxy(target);
const inner = await deploy(l1, artifact("NestedSetterWrapper.sol/NestedSetterInner.json"), cast("abi-encode", "f(address)", targetProxy));
const outer = await deploy(l1, artifact("NestedSetterWrapper.sol/NestedSetterOuter.json"), cast("abi-encode", "f(address)", inner));
evidence.fixtures = { target, targetProxy, inner, outer }; checkpoint();
console.log("Nested L1 → L2 fixture deployed");

async function paidSource(name, to, data, type, verify, rejectUnderpayment = false) {
  const transaction = { chainId: hex(7331), from, to, data, value: "0x0" };
  let quoted = await quote(transaction);
  if (rejectUnderpayment) {
    const tx = feeQuoteTransaction(quoteArgs(quoted, transaction, "0x2"));
    const raw = sign({ ...tx, maxPriorityFeePerGas: "0x0" });
    const rejected = await until("underfunded rejection", async () => {
      const r = await request(front, "eth_sendRawTransaction", [raw]);
      if (retryable(r)) return null;
      assert.equal(r.error?.data?.code, "l1_tip_underfunded", JSON.stringify(r));
      return r;
    }, 30);
    assert.equal(await rpc(front, "eez_getCrossChainTransaction", [cast("keccak", raw)]), null);
    evidence.underfunded = rejected; checkpoint();
    quoted = await quote(transaction);
  }
  let signatures = 0;
  const entry = { name, type, transaction, quote: quoted };
  const wallet = { request: async ({ method, params = [] }) => {
    if (method === "eth_accounts") return [from];
    if (method === "eth_sendTransaction") {
      signatures++;
      const raw = sign(params[0]); entry.hash = cast("keccak", raw);
      evidence.sources.push(entry); checkpoint();
      return until("signed admission", async () => {
        const r = await request(front, "eth_sendRawTransaction", [raw]);
        if (retryable(r)) return null;
        assert(!r.error, JSON.stringify(r)); return r.result;
      }, 30);
    }
    return rpc(front, method, params);
  } };
  const hash = await sendFeeQuotedTransaction({ wallet, ...quoteArgs(quoted, transaction, type) });
  assert.equal(hash, entry.hash); assert.equal(signatures, 1);
  console.log(`${name} admitted: ${hash}`);
  const ownership = await until("canonical source consumption", async () => {
    const r = await rpc(front, "eez_getCrossChainTransaction", [hash]);
    return r?.ownership?.canonical ? r : null;
  });
  assert.equal(ownership.ownership.reason, "Committed");
  const source = await receipt(l1, hash);
  const block = await rpc(l1, "eth_getBlockByNumber", [source.blockNumber, true]);
  assert.equal(block.hash, source.blockHash);
  const sourceIndex = Number(BigInt(source.transactionIndex));
  const posts = block.transactions.slice(0, sourceIndex).filter(t => t.to?.toLowerCase() === registry.toLowerCase() && t.type === "0x3");
  assert.equal(posts.length, 1, "one ordered native blob post before source");
  const post = await receipt(l1, posts[0].hash);
  assert(BigInt(post.blobGasUsed) > 0n);
  const detail = quoted.diagnostics.options.find(o => o.transaction.type === type);
  const recipient = detail.estimate.refundRecipient.toLowerCase();
  const payments = block.transactions.slice(sourceIndex + 1).filter(t => t.from.toLowerCase() === block.miner.toLowerCase()
    && t.to?.toLowerCase() === recipient && t.input === "0x" && BigInt(t.value) > 0n);
  assert.equal(payments.length, 1, "one unambiguous builder-funded payout after source");
  const payout = await receipt(l1, payments[0].hash);
  assert.equal(BigInt(payout.gasUsed), 21_000n);
  const actualCost = BigInt(post.gasUsed) * BigInt(post.effectiveGasPrice) + BigInt(post.blobGasUsed) * BigInt(post.blobGasPrice);
  const policy = detail.policy.assumptions;
  const subtotal = actualCost + BigInt(policy.proving_cost_l1) + BigInt(policy.operations_cost_l1) + BigInt(policy.treasury_cost_l1);
  const minimum = subtotal + (subtotal * BigInt(policy.risk_bps) + 9999n) / 10_000n;
  const income = BigInt(payments[0].value);
  assert(income >= minimum, "received refund must cover measured post/blob/service costs and risk");
  const balance = async block => BigInt(await rpc(l1, "eth_getBalance", [recipient, block]));
  assert.equal(await balance(source.blockNumber) - await balance(hex(BigInt(source.blockNumber) - 1n)), income);
  await verify();
  entry.ownership = ownership; entry.sourceReceipt = source; entry.postReceipt = post; entry.payoutReceipt = payout;
  entry.postHash = posts[0].hash; entry.payoutHash = payments[0].hash; entry.refundRecipient = recipient;
  entry.actualPostingCostL1 = String(actualCost); entry.minimumL1 = String(minimum); entry.receivedRefundL1 = String(income);
  entry.grossUserTipL1 = String(BigInt(source.gasUsed) * (BigInt(source.effectiveGasPrice) - BigInt(block.baseFeePerGas)));
  entry.builder = block.miner; checkpoint();
  console.log(`${name}: canonical effects and refund verified; income=${income}, cost=${actualCost}`);
}

for (const [index, type] of ["0x0", "0x2"].entries()) {
  const value = BigInt(781 + index);
  await paidSource(`nested-${type}`, outer, cast("calldata", "setViaInner(uint256)", String(value)), type, async () => {
    await until("canonical nested remote effect", async () => BigInt(await read(l2, target, "value()", "safe")) === value);
    assert.equal(BigInt(await read(l1, inner, "completedProxyCalls()")), BigInt(index + 1));
  }, index === 0);
}

const fixtures = JSON.parse(readFileSync("crates/eez-node-e2e/tests/fixtures/safe-1.4.1/bytecode.json", "utf8")).contracts;
const singleton = await deploy(l2, fixtures.Safe.bytecode);
const factory = await deploy(l2, fixtures.SafeProxyFactory.bytecode);
const factoryProxy = await proxy(factory);
const initializer = cast("calldata", "setup(address[],uint256,address,bytes,address,address,uint256,address)", `[${from}]`, "1", zero, "0x", zero, zero, "0", zero);
const creationCode = cast("abi-decode", "f()(bytes)", await read(l2, factory, "proxyCreationCode()"));
const initHash = cast("keccak", creationCode + singleton.slice(2).padStart(64, "0"));
const safes = [];
for (const [index, type] of ["0x0", "0x2"].entries()) {
  const salt = String(100 + index);
  const saltHash = cast("keccak", cast("abi-encode", "f(bytes32,uint256)", cast("keccak", initializer), salt));
  const safe = `0x${cast("keccak", `0xff${factory.slice(2)}${saltHash.slice(2)}${initHash.slice(2)}`).slice(-40)}`;
  await paidSource(`safe-create-${type}`, factoryProxy,
    cast("calldata", "createProxyWithNonce(address,bytes,uint256)", singleton, initializer, salt), type, async () => {
      await until("canonical Safe deployment", async () => (await rpc(l2, "eth_getCode", [safe, "safe"])) !== "0x");
      assert.equal(BigInt(await read(l2, safe, "getThreshold()", "safe")), 1n);
      const owners = cast("abi-decode", "f()(address[])", await read(l2, safe, "getOwners()", "safe"));
      assert.equal(owners.toLowerCase(), `[${from.toLowerCase()}]`);
      safes.push(safe);
    });
}
Object.assign(evidence.fixtures, { singleton, factory, factoryProxy, safes });

// Public fixture keys, used only after the private-chain identity checks above.
// Independent senders let quotes use actual current nonces while a concurrent
// wave reaches the scheduler before its next preparation opportunity.
const actors = [101, 102, 103, 104].map(n => {
  const signingKey = `0x${n.toString(16).padStart(64, "0")}`;
  return { signingKey, from: cast("wallet", "address", "--private-key", signingKey) };
});
for (const actor of actors) await ordinary(l1, actor.from, "0x", hex(10n ** 18n));
evidence.batches = [];
for (let wave = 0; wave < 2; wave++) {
  const batch = { sources: [] };
  evidence.batches.push(batch); checkpoint();
  const beforeCalls = BigInt(await read(l1, inner, "completedProxyCalls()"));
  // Prepare fees ahead of the boundary; do not depend on per-gas priority order.
  const prepared = await Promise.all(actors.map(async (actor, index) => {
    const type = index % 2 ? "0x2" : "0x0";
    const value = BigInt(900 + wave * actors.length + index);
    const transaction = { chainId: hex(7331), from: actor.from, to: outer,
      data: cast("calldata", "setViaInner(uint256)", String(value)), value: "0x0" };
    const quoted = await quote(transaction);
    const tx = feeQuoteTransaction(quoteArgs(quoted, transaction, type));
    const raw = sign(tx, actor.signingKey);
    const source = { transaction, type, quote: quoted, hash: cast("keccak", raw) };
    batch.sources.push(source); checkpoint();
    return { raw, source };
  }));
  const head = await rpc(l1, "eth_blockNumber");
  await until("next L1 slot before concurrent submission", async () => (await rpc(l1, "eth_blockNumber")) !== head);
  await Promise.all(prepared.map(async ({ raw, source }) => {
    const result = await until("batched source admission", async () => {
      const r = await request(front, "eth_sendRawTransaction", [raw]);
      if (retryable(r)) return null;
      assert(!r.error, JSON.stringify(r)); return r.result;
    }, 30);
    assert.equal(result, source.hash);
  }));
  for (const source of batch.sources) {
    source.ownership = await until("batched canonical consumption", async () => {
      const r = await rpc(front, "eez_getCrossChainTransaction", [source.hash]);
      return r?.ownership?.canonical ? r : null;
    });
    assert.equal(source.ownership.ownership.reason, "Committed");
    source.receipt = await receipt(l1, source.hash); checkpoint();
  }
  const first = batch.sources[0].receipt;
  assert(batch.sources.every(s => s.receipt.blockHash === first.blockHash), "all sources must share one L1 block");
  const block = await rpc(l1, "eth_getBlockByNumber", [first.blockNumber, true]);
  const posts = block.transactions.filter(t => t.to?.toLowerCase() === registry.toLowerCase() && t.type === "0x3");
  assert.equal(posts.length, 1, "multiple L1 groups must share one native post");
  const post = await receipt(l1, posts[0].hash);
  const ordered = batch.sources.toSorted((a, b) => Number(BigInt(a.receipt.transactionIndex) - BigInt(b.receipt.transactionIndex)));
  assert(BigInt(post.transactionIndex) < BigInt(ordered[0].receipt.transactionIndex));
  for (const source of ordered) source.grossUserTipL1 = String(BigInt(source.receipt.gasUsed)
    * (BigInt(source.receipt.effectiveGasPrice) - BigInt(block.baseFeePerGas)));
  const payer = ordered.reduce((a, b) => BigInt(b.grossUserTipL1) > BigInt(a.grossUserTipL1) ? b : a);
  const detail = payer.quote.diagnostics.options.find(o => o.transaction.type === payer.type);
  const recipient = detail.estimate.refundRecipient.toLowerCase();
  const payments = block.transactions.filter(t => t.from.toLowerCase() === block.miner.toLowerCase()
    && t.to?.toLowerCase() === recipient && t.input === "0x" && BigInt(t.value) > 0n);
  assert.equal(payments.length, 1, "one selected-payer refund per shared post");
  const payout = await receipt(l1, payments[0].hash);
  assert(BigInt(payout.transactionIndex) > BigInt(payer.receipt.transactionIndex));
  const payoutCost = BigInt(payout.gasUsed) * BigInt(payout.effectiveGasPrice);
  const expected = BigInt(payer.grossUserTipL1) * BigInt(detail.estimate.refundPercent) / 100n - payoutCost;
  const income = BigInt(payments[0].value);
  assert.equal(income, expected, "refund must come from the largest measured total tip");
  const cost = BigInt(post.gasUsed) * BigInt(post.effectiveGasPrice) + BigInt(post.blobGasUsed) * BigInt(post.blobGasPrice);
  if (wave === 0) assert(income < cost, "first batch must exercise an actual shortfall");
  assert.equal(BigInt(await read(l1, inner, "completedProxyCalls()")), beforeCalls + BigInt(actors.length));
  const lastValue = BigInt(`0x${ordered.at(-1).transaction.data.slice(-64)}`);
  await until("batched nested effects", async () => BigInt(await read(l2, target, "value()", "safe")) === lastValue);
  Object.assign(batch, { postHash: posts[0].hash, postReceipt: post, payoutHash: payments[0].hash,
    payoutReceipt: payout, selectedPayer: payer.hash, actualPostingCostL1: String(cost), receivedRefundL1: String(income),
    shortfallL1: String(cost > income ? cost - income : 0n), surplusL1: String(income - cost),
    expectedL2Value: String(lastValue) }); checkpoint();
  console.log(`Shared batch ${wave}: ${actors.length} nested sources, payer=${payer.hash}, refund=${income}, cost=${cost}`);
  // Allow canonical observation before the next wave or final quote check.
  await new Promise(resolve => setTimeout(resolve, 15_000));
}
// Quotes remain available after an observed shortfall. Missing-payment
// continuation is independently covered by the process adapter regressions.
await new Promise(resolve => setTimeout(resolve, 15_000));
await quote({ chainId: hex(7331), from, to: outer, data: cast("calldata", "setViaInner(uint256)", "783") });
evidence.completed = true; checkpoint();
console.log("Legacy/EIP-1559, nested/Safe execution, multi-user batching, largest-payer refunds and subsidized continuation passed");
