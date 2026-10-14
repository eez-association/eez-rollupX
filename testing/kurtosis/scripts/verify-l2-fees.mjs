#!/usr/bin/env node
// Disposable Kurtosis-only fee exercise. Requires prebuilt integration contracts
// and a funded fixture key; never accepts a public-network chain ID.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { requestL2FeeQuote, sendQuotedTransaction } from "../../../sdk/l2-fees.mjs";

const env = name => {
  assert(process.env[name], `${name} is required`);
  return process.env[name];
};
const l1 = env("EEZ_DEVNET_L1_RPC");
const l2 = env("EEZ_DEVNET_L2_RPC");
const front = env("EEZ_DEVNET_L2_FRONT");
const manager = env("EEZL2_ADDRESS");
const fixtureKey = env("EEZ_TEST_FEE_KEY");
const output = env("EEZ_FEE_EVIDENCE_FILE");
const maxTotalFee = 10n ** 17n;
const quoteUrl = process.env.EEZ_TEST_FEE_QUOTE_URL ?? `${front}/eez/fees/l2-to-l1`;
let id = 0;
const hex = value => `0x${BigInt(value).toString(16)}`;
function cast(...args) {
  try {
    return execFileSync("cast", args, { encoding: "utf8", timeout: 30_000, stdio: ["ignore", "pipe", "pipe"] }).trim();
  } catch {
    // Do not propagate child-process argument arrays containing a signing key.
    throw new Error(`cast ${args[0]} failed`);
  }
}
async function request(url, method, params = []) {
  const response = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }), signal: AbortSignal.timeout(15_000) });
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
const address = key => cast("wallet", "address", "--private-key", key);
function sign(tx, key) {
  const args = ["mktx", "--private-key", key, "--chain", String(BigInt(tx.chainId)),
    "--nonce", String(BigInt(tx.nonce)), "--gas-limit", String(BigInt(tx.gas)),
    "--gas-price", String(BigInt(tx.maxFeePerGas)), "--priority-gas-price", String(BigInt(tx.maxPriorityFeePerGas)),
    "--value", String(BigInt(tx.value))];
  if (tx.to) args.push(tx.to, tx.data);
  else args.push("--create", tx.data);
  return cast(...args);
}
async function receipt(url, hash) {
  const result = await until(`receipt ${hash}`, () => rpc(url, "eth_getTransactionReceipt", [hash]));
  assert.equal(result.status, "0x1", `reverted ${hash}`);
  return result;
}
async function ordinary(url, key, to, data = "0x", value = "0x0") {
  const from = address(key);
  const [chainId, nonce, parent] = await Promise.all([
    rpc(url, "eth_chainId"), rpc(url, "eth_getTransactionCount", [from, "pending"]), rpc(url, "eth_getBlockByNumber", ["latest", false]),
  ]);
  const tip = 1_000_000_000n;
  const raw = sign({ to, data, value, chainId, nonce, gas: "0x2dc6c0", maxPriorityFeePerGas: hex(tip), maxFeePerGas: hex(2n * BigInt(parent.baseFeePerGas) + tip) }, key);
  return receipt(url, await rpc(url, "eth_sendRawTransaction", [raw]));
}
async function deploy(url, artifact, constructor) {
  const code = JSON.parse(readFileSync(`contracts/out/${artifact}`, "utf8")).bytecode.object;
  const result = await ordinary(url, fixtureKey, null, code + constructor.replace(/^0x/, ""));
  assert(result.contractAddress, "deployment address missing");
  return result.contractAddress;
}
async function readUint(url, to, signature) {
  return BigInt(await rpc(url, "eth_call", [{ to, data: cast("calldata", signature) }, "latest"]));
}
async function getQuote(intent) {
  return until("available fee quote", async () => {
    try { return await requestL2FeeQuote({ url: quoteUrl, intent, maxTotalFee }); }
    catch (error) {
      if (/Fee quote (stale_context|quote_unavailable|quote_busy)/.test(error.message)) return null;
      throw error;
    }
  }, 60);
}
async function submitRaw(raw) {
  return until("signed fee admission", async () => {
    const result = await request(front, "eth_sendRawTransaction", [raw]);
    if (["stale_context", "quote_unavailable", "quote_busy"].includes(result.error?.data?.code)) return null;
    assert(!result.error, JSON.stringify(result.error));
    return result.result;
  }, 30);
}

assert.equal(BigInt(await rpc(l1, "eth_chainId")), 7331n, "fixture requires private Kurtosis L1");
assert.equal(BigInt(await rpc(l2, "eth_chainId")), 696990n, "fixture requires private Kurtosis L2");
const fixtures = [];
for (let index = 0; index < 2; index++) {
  const key = `0x${randomBytes(32).toString("hex")}`;
  const from = address(key);
  await ordinary(l2, fixtureKey, from, "0x", hex(10n ** 18n));
  const target = await deploy(l1, "Value.sol/Value.json", cast("abi-encode", "f(uint256)", "0"));
  const proxy = `0x${(await rpc(l2, "eth_call", [{ to: manager, data: cast("calldata", "computeCrossChainProxyAddress(address,uint64)", target, "0") }, "latest"])).slice(-40)}`;
  await ordinary(l2, fixtureKey, manager, cast("calldata", "createCrossChainProxy(address,uint64)", target, "0"));
  const inner = await deploy(l2, "NestedSetterWrapper.sol/NestedSetterInner.json", cast("abi-encode", "f(address)", proxy));
  const outer = await deploy(l2, "NestedSetterWrapper.sol/NestedSetterOuter.json", cast("abi-encode", "f(address)", inner));
  const intent = { type: "0x2", chainId: hex(696990), from, to: outer, nonce: "0x0", gas: "0x124f80", value: "0x0", data: cast("calldata", "setViaInner(uint256)", String(123 + index)) };
  assert.equal(BigInt(intent.chainId), 696990n);
  fixtures.push({ key, target, proxy, inner, intent, expected: BigInt(123 + index) });
}
console.log("Two independently funded nested fixtures deployed");

const first = fixtures[0];
const initialQuote = await getQuote(first.intent);
const badRaw = sign({ ...initialQuote.transaction, maxPriorityFeePerGas: "0x0" }, first.key);
const rejected = await until("underpayment rejection", async () => {
  const result = await request(front, "eth_sendRawTransaction", [badRaw]);
  if (["stale_context", "quote_unavailable", "quote_busy"].includes(result.error?.data?.code)) return null;
  assert.equal(result.error?.data?.code, "l2_tip_underfunded", JSON.stringify(result));
  return result;
}, 30);
assert.equal(await rpc(front, "eez_getCrossChainTransaction", [cast("keccak", badRaw)]), null);
assert.equal(BigInt(await rpc(l2, "eth_getTransactionCount", [first.intent.from, "latest"])), 0n);
assert.equal(await readUint(l1, first.target, "value()"), 0n);
console.log("Underpaid signed source rejected without queue ownership or execution");

const evidence = { rejected, sources: [], postings: [] };
// Quote/send independent nonces promptly. Canonical receipt evidence below
// records whether scheduling batched them; this script does not assume it did.
for (const fixture of fixtures) {
  const quote = await getQuote(fixture.intent);
  let signatures = 0;
  const wallet = { request: async ({ method, params = [] }) => {
    if (method === "eth_accounts") return [fixture.intent.from];
    if (method === "eth_sendTransaction") { signatures++; return submitRaw(sign(params[0], fixture.key)); }
    return rpc(front, method, params);
  } };
  const hash = await sendQuotedTransaction({ wallet, quote, intent: fixture.intent, maxTotalFee });
  assert.equal(signatures, 1);
  evidence.sources.push({ hash, quote, target: fixture.target, proxy: fixture.proxy, inner: fixture.inner, expected: String(fixture.expected) });
}
for (const source of evidence.sources) {
  const paid = await receipt(l2, source.hash);
  await until("canonical L1 target state", async () => await readUint(l1, source.target, "value()") === BigInt(source.expected));
  assert.equal(await readUint(l2, source.inner, "completedProxyCalls()"), 1n);
  const block = await rpc(l2, "eth_getBlockByHash", [paid.blockHash, false]);
  assert.equal(block.miner.toLowerCase(), source.quote.policy.assumptions.beneficiary.toLowerCase());
  const tip = BigInt(paid.gasUsed) * (BigInt(paid.effectiveGasPrice) - BigInt(block.baseFeePerGas));
  assert(tip >= BigInt(source.quote.estimate.requiredL2Tip), "actual tip must cover the quoted tariff");
  const ownership = await until("canonical queue disposition", async () => {
    const status = await rpc(front, "eez_getCrossChainTransaction", [source.hash]);
    return status?.ownership?.canonical ? status : null;
  });
  assert.equal(ownership.ownership.reason, "Committed");
  source.receipt = paid;
  source.grossL2Tip = tip.toString();
  source.ownership = ownership;
  source.settlement = await until("source settlement correlation", async () => {
    const settlement = await rpc(front, "eez_getSettlementByL2Block", [paid.blockHash]);
    return settlement?.canonicalL2 ? settlement : null;
  });
}
const ceil = (value, denominator) => (value + denominator - 1n) / denominator;
for (const hash of new Set(evidence.sources.map(source => source.settlement.l1TransactionHash))) {
  const sources = evidence.sources.filter(source => source.settlement.l1TransactionHash === hash);
  const post = await receipt(l1, hash);
  assert.equal(post.type, "0x3", "settlement must use native blob DA");
  assert(BigInt(post.blobGasUsed) > 0n);
  const cost = BigInt(post.gasUsed) * BigInt(post.effectiveGasPrice) + BigInt(post.blobGasUsed) * BigInt(post.blobGasPrice);
  const policy = sources[0].quote.policy.assumptions;
  const subtotal = cost + BigInt(policy.proving_cost_l1) + BigInt(policy.operations_cost_l1) + BigInt(policy.treasury_cost_l1);
  const requiredL1 = subtotal + ceil(subtotal * BigInt(policy.risk_bps), 10_000n) + ceil(subtotal * BigInt(policy.profit_bps), 10_000n);
  const requiredL2 = ceil(requiredL1 * BigInt(policy.l1_per_l2_denominator), BigInt(policy.l1_per_l2_numerator));
  const paid = sources.reduce((sum, source) => sum + BigInt(source.grossL2Tip), 0n);
  assert(paid >= requiredL2, "pooled receipts must cover the complete canonical post/blob tariff");
  assert.equal((await rpc(l1, "eth_getBlockByNumber", [post.blockNumber, false])).hash, post.blockHash);
  evidence.postings.push({ hash, receipt: post, actualPostingCostL1: String(cost), requiredL2: String(requiredL2), paidL2: String(paid) });
}
for (const source of evidence.sources) {
  assert.equal((await rpc(l2, "eth_getBlockByNumber", [source.receipt.blockNumber, false])).hash, source.receipt.blockHash);
}
evidence.sameNativeBlock = evidence.sources[0].receipt.blockHash === evidence.sources[1].receipt.blockHash;
writeFileSync(output, `${JSON.stringify(evidence, null, 2)}\n`);
console.log(`Fee receipt, target state and canonical disposition checks passed; same native block: ${evidence.sameNativeBlock}`);
