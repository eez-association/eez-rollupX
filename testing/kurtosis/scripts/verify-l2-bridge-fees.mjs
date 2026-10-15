#!/usr/bin/env node
// Receipt-backed first/repeated bridge pricing on a disposable private network.
// The inbound fixture funds real L1 escrow before either outbound withdrawal.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { feeQuoteTransaction, sendFeeQuotedTransaction } from "../../../sdk/l2-fees.mjs";

const env = name => { assert(process.env[name], `${name} is required`); return process.env[name]; };
const l1 = env("EEZ_DEVNET_L1_RPC"), l2 = env("EEZ_DEVNET_L2_RPC");
const inbound = env("EEZ_DEVNET_L1_FRONT"), outbound = env("EEZ_DEVNET_L2_FRONT");
const bridgeL1 = env("EEZ_L1_BRIDGE_SENDER"), bridgeL2 = env("EEZ_L2_BRIDGE_RECEIVER");
const funder = env("EEZ_TEST_FEE_KEY"), output = env("EEZ_BRIDGE_FEE_EVIDENCE_FILE");
const hex = n => `0x${BigInt(n).toString(16)}`;
const ceil = (n, d) => (n + d - 1n) / d;
function cast(...args) {
  try { return execFileSync("cast", args, { encoding: "utf8", timeout: 30_000, stdio: ["ignore", "pipe", "pipe"] }).trim(); }
  catch { throw new Error(`cast ${args[0]} failed`); }
}
let id = 0;
async function request(url, method, params = []) {
  const response = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }), signal: AbortSignal.timeout(15_000) });
  assert(response.ok, `RPC HTTP ${response.status}`);
  return response.json();
}
async function rpc(url, method, params = []) {
  const response = await request(url, method, params);
  assert(!response.error, `${method}: ${JSON.stringify(response.error)}`);
  return response.result;
}
async function until(label, fn, seconds = 180) {
  const deadline = Date.now() + seconds * 1000;
  while (Date.now() < deadline) {
    const value = await fn();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  throw new Error(`Timed out: ${label}`);
}
function sign(tx, key) {
  const flags = tx.type === "0x0" ? ["--legacy", "--gas-price", String(BigInt(tx.gasPrice))]
    : ["--gas-price", String(BigInt(tx.maxFeePerGas)), "--priority-gas-price", String(BigInt(tx.maxPriorityFeePerGas))];
  return cast("mktx", "--private-key", key, "--chain", String(BigInt(tx.chainId)),
    "--nonce", String(BigInt(tx.nonce)), "--gas-limit", String(BigInt(tx.gas)),
    ...flags, "--value", String(BigInt(tx.value)), tx.to, tx.data);
}
async function receipt(url, hash) {
  const result = await until("receipt", () => rpc(url, "eth_getTransactionReceipt", [hash]));
  assert.equal(result.status, "0x1");
  return result;
}
async function ordinary(url, to, data, value) {
  const from = cast("wallet", "address", "--private-key", funder);
  const chainId = await rpc(url, "eth_chainId");
  const nonce = await rpc(url, "eth_getTransactionCount", [from, "pending"]);
  const block = await rpc(url, "eth_getBlockByNumber", ["latest", false]);
  const raw = sign({ chainId, nonce, to, data, value, gas: "0x2dc6c0",
    maxPriorityFeePerGas: hex(1_000_000_000n), maxFeePerGas: hex(2n * BigInt(block.baseFeePerGas) + 1_000_000_000n) }, funder);
  const hash = await rpc(url, "eth_sendRawTransaction", [raw]);
  await receipt(url, hash);
  return hash;
}
assert.equal(BigInt(await rpc(l1, "eth_chainId")), 7331n, "private Kurtosis L1 required");
assert.equal(BigInt(await rpc(l2, "eth_chainId")), 696990n, "private Kurtosis L2 required");
await until("initial canonical settlement", async () =>
  BigInt((await rpc(l2, "eth_getBlockByNumber", ["safe", false])).number) > 0n, 600);
const key = `0x${randomBytes(32).toString("hex")}`;
const user = cast("wallet", "address", "--private-key", key);
await ordinary(l2, user, "0x", hex(10n ** 18n));
const inboundHash = await ordinary(inbound, bridgeL1, cast("calldata", "bridgeEther(uint64,address)", "1", user), hex(10n ** 16n));
await until("inbound escrow settlement", async () => (await rpc(outbound, "eez_getCrossChainTransaction", [inboundHash]))?.ownership?.canonical);
const evidence = { inboundHash, sources: [] };
for (const type of ["0x2", "0x0"]) {
  const value = 10n ** 15n;
  const transaction = { type, chainId: hex(696990), from: user, to: bridgeL2,
    nonce: await rpc(l2, "eth_getTransactionCount", [user, "latest"]), value: hex(value),
    data: cast("calldata", "bridgeEther(uint64,address)", "0", user) };
  const beforeL1 = BigInt(await rpc(l1, "eth_getBalance", [user, "latest"]));
  const beforeL2 = BigInt(await rpc(l2, "eth_getBalance", [user, "latest"]));
  const maxTotalFee = 10n ** 17n;
  const quote = await until("bridge quote", async () => {
    const response = await request(outbound, "eez_debugQuoteFee", [transaction]);
    if (["stale_context", "quote_unavailable", "quote_busy"].includes(response.error?.data?.code)) return null;
    assert(!response.error, JSON.stringify(response.error));
    feeQuoteTransaction({quote:response.result,transaction,type,chainId:transaction.chainId,maxTotalFee,keccak256:text=>cast("keccak",text)});
    return response.result;
  });
  const detail = quote.diagnostics.options[0];
  assert.equal(detail.estimate.basis, "isolated_native_l2_and_l1_settlement");
  assert.equal(detail.policy.assumptions.risk_bps, 2000);
  for (const field of ["proving_cost_l1", "operations_cost_l1", "treasury_cost_l1"]) assert.equal(detail.policy.assumptions[field], "0");
  assert.equal(detail.policy.assumptions.profit_bps, 0);
  assert.equal(detail.policy.assumptions.l1_per_l2_numerator, "1");
  assert.equal(detail.policy.assumptions.l1_per_l2_denominator, "1");
  const projectedCost = BigInt(detail.estimate.l1PostingCost);
  assert.equal(BigInt(detail.estimate.requiredL2Tip), projectedCost + ceil(projectedCost * 2000n, 10_000n));
  let signatures = 0;
  const wallet = { request: async ({ method, params = [] }) => {
    if (method === "eth_accounts") return [user];
    if (method === "eth_sendTransaction") {
      signatures++;
      const raw = sign(params[0], key);
      return until("signed bridge admission", async () => {
        const response = await request(outbound, "eth_sendRawTransaction", [raw]);
        if (["stale_context", "quote_unavailable", "quote_busy"].includes(response.error?.data?.code)) return null;
        assert(!response.error, JSON.stringify(response.error));
        return response.result;
      });
    }
    return rpc(outbound, method, params);
  } };
  const hash = await sendFeeQuotedTransaction({wallet,quote,transaction,type,chainId:transaction.chainId,maxTotalFee,keccak256:text=>cast("keccak",text)});
  assert.equal(signatures, 1);
  await until("canonical bridge settlement", async () => (await rpc(outbound, "eez_getCrossChainTransaction", [hash]))?.ownership?.canonical);
  const paid = await receipt(l2, hash);
  const settlement = await rpc(outbound, "eez_getSettlementByL2Block", [paid.blockHash]);
  assert(settlement.canonicalL2);
  const post = await receipt(l1, settlement.l1TransactionHash);
  const block = await rpc(l2, "eth_getBlockByHash", [paid.blockHash, false]);
  const fee = BigInt(paid.gasUsed) * BigInt(paid.effectiveGasPrice);
  const tip = BigInt(paid.gasUsed) * (BigInt(paid.effectiveGasPrice) - BigInt(block.baseFeePerGas));
  const cost = BigInt(post.gasUsed) * BigInt(post.effectiveGasPrice) + BigInt(post.blobGasUsed) * BigInt(post.blobGasPrice);
  assert.equal(BigInt(await rpc(l1, "eth_getBalance", [user, "latest"])) - beforeL1, value);
  assert.equal(beforeL2 - BigInt(await rpc(l2, "eth_getBalance", [user, "latest"])), value + fee);
  assert.equal(BigInt(paid.gasUsed), BigInt(detail.estimate.sourceGasUsed));
  const gasDifference = BigInt(detail.estimate.l1Gas) - BigInt(post.gasUsed);
  assert(gasDifference >= -1000n && gasDifference <= 1000n, `settlement gas differs: ${gasDifference}`);
  const tariff = cost + ceil(cost * 2000n, 10_000n);
  assert(tip >= tariff, "actual payment must cover settlement plus the 20% tariff");
  assert(tip <= ceil(tariff * 102n, 100n), "fee exceeds measured cost plus margin by more than 2%");
  evidence.sources.push({ hash, quote, receipt: paid, settlement, post,
    l1Cost: String(cost), requiredL2Tip: String(tariff), grossL2Tip: String(tip), gasDifference: String(gasDifference) });
  writeFileSync(output, JSON.stringify(evidence, null, 2) + "\n");
  console.log(`${type} bridge settled: quoted L1 gas ${detail.estimate.l1Gas}, actual ${BigInt(post.gasUsed)}, tip/cost ${Number(tip * 10000n / cost) / 10000}`);
}
