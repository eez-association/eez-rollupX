// Regression for a mined ordinary L1 nonce overtaking the completed settlement snapshot.
// Uses only the private Kurtosis chain IDs and funded public fixture wallets.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';

const { EEZ_DEVNET_L1_RPC: l1, EEZ_DEVNET_L2_RPC: l2, EEZ_DEVNET_L1_FRONT: front,
  EEZ_REGISTRY_ADDRESS: registry, EEZ_TEST_FEE_KEY: key } = process.env;
assert(l1 && l2 && front && registry && key);
const output = process.env.EEZ_NONCE_SNAPSHOT_EVIDENCE;
assert(output, 'Set EEZ_NONCE_SNAPSHOT_EVIDENCE to the evidence output path');
const evidence = { source: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  protocol: execFileSync('git', ['-C', 'eez-core-protocol', 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  baseline: [], rounds: [], controls: {} };
const save = () => writeFileSync(output, JSON.stringify(evidence, null, 2) + '\n');
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const hex = n => `0x${BigInt(n).toString(16)}`;
const cast = (...args) => execFileSync('cast', args, { encoding: 'utf8', timeout: 30000,
  stdio: ['ignore', 'pipe', 'pipe'] }).trim();
const from = cast('wallet', 'address', '--private-key', key);
const companionKey = process.env.EEZ_COMPANION_TEST_KEY;
assert(companionKey);
const companionFrom = cast('wallet', 'address', '--private-key', companionKey);
assert.notEqual(from, companionFrom);
let id = 0;
async function request(url, method, params = []) {
  const response = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: ++id, method, params }), signal: AbortSignal.timeout(15000) });
  assert(response.ok, `HTTP ${response.status}`);
  return response.json();
}
async function rpc(url, method, params = []) {
  const response = await request(url, method, params);
  assert(!response.error, `${method}: ${JSON.stringify(response.error)}`);
  return response.result;
}
async function until(label, fn, timeout = 120000, interval = 100) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await fn();
    if (value) return value;
    await pause(interval);
  }
  throw new Error(`Timed out: ${label}`);
}
async function ordinary(url, to, data = '0x', gas = 21000) {
  const [chain, nonce, head] = await Promise.all([rpc(url, 'eth_chainId'),
    rpc(url, 'eth_getTransactionCount', [from, 'latest']), rpc(url, 'eth_getBlockByNumber', ['latest', false])]);
  const tip = 1000000000n;
  const args = ['mktx', '--private-key', key, '--chain', String(BigInt(chain)), '--nonce', String(BigInt(nonce)),
    '--gas-limit', String(gas), '--gas-price', String(2n * BigInt(head.baseFeePerGas) + tip),
    '--priority-gas-price', String(tip), '--value', '0'];
  args.push(...(to ? [to, data] : ['--create', data]));
  const hash = await rpc(url, 'eth_sendRawTransaction', [cast(...args)]);
  const receipt = await until('ordinary receipt', () => rpc(url, 'eth_getTransactionReceipt', [hash]), 120000, 40);
  assert.equal(receipt.status, '0x1');
  return receipt;
}
async function quote(transaction, method = 'eez_debugQuoteFee') {
  const started = new Date().toISOString(), begin = performance.now();
  const response = await request(front, method, [transaction, { transactionTypes: ['0x0', '0x2'] }]);
  return { started, finished: new Date().toISOString(), milliseconds: performance.now() - begin,
    transaction, response };
}
const usable = q => q.response.result?.options?.every(option => !option.error);
assert.equal(await rpc(l1, 'eth_chainId'), hex(7331));
assert.equal(await rpc(l2, 'eth_chainId'), hex(696990));
await until('L2 started', async () => BigInt((await rpc(l2, 'eth_getBlockByNumber', ['latest', false])).number) > 0n, 120000, 500);
const bytecode = JSON.parse(readFileSync('contracts/out/Value.sol/Value.json')).bytecode.object;
const created = await ordinary(l2, null, bytecode + cast('abi-encode', 'f(uint256)', '0').slice(2), 3000000);
const target = created.contractAddress;
const encoded = await rpc(l1, 'eth_call', [{ to: registry,
  data: cast('calldata', 'computeCrossChainProxyAddress(address,uint64)', target, '1') }, 'latest']);
const proxy = `0x${encoded.slice(-40)}`;
const setup = await ordinary(l1, registry, cast('calldata', 'createCrossChainProxy(address,uint64)', target, '1'), 3000000);
const transaction = { from, to: proxy, data: cast('calldata', 'setValue(uint256)', '42'), value: '0x0' };
evidence.fixture = { from, target, proxy, created, setup, transaction }; save();
await until('stable initial quote', async () => {
  const nonce = await rpc(l1, 'eth_getTransactionCount', [from, 'latest']);
  const q = await quote({ ...transaction, nonce }); evidence.baseline.push(q); save();
  assert.notEqual(q.response.error?.data?.code, 'invalid_transaction', JSON.stringify(q));
  return usable(q);
}, 120000, 300);
console.log('Fixture has a successful baseline cross-chain quote');
for (let round = 0; round < 6; round++) {
  const headBefore = await rpc(l1, 'eth_blockNumber');
  await until('fresh source slot', async () => (await rpc(l1, 'eth_blockNumber')) !== headBefore, 30000, 40);
  const companionTransaction = { ...transaction, from: companionFrom,
    nonce: await rpc(l1, 'eth_getTransactionCount', [companionFrom, 'latest']),
    data: cast('calldata', 'setValue(uint256)', String(84 + round)) };
  const companionQuote = await until('companion quote', async () => {
    const q = await quote(companionTransaction);
    return usable(q) && q;
  }, 15000, 150);
  const option = companionQuote.response.result.options.find(option => option.type === '0x2');
  const raw = cast('mktx', '--private-key', companionKey, '--chain', '7331',
    '--nonce', String(BigInt(companionTransaction.nonce)), '--gas-limit', String(BigInt(option.gas)),
    '--gas-price', String(BigInt(option.maxFeePerGas)), '--priority-gas-price', String(BigInt(option.maxPriorityFeePerGas)),
    proxy, companionTransaction.data);
  const companionHash = cast('keccak', raw);
  await until('companion admission', async () => {
    const r = await request(front, 'eth_sendRawTransaction', [raw]);
    if (['stale_context', 'quote_unavailable', 'quote_busy'].includes(r.error?.data?.code)) return false;
    assert(!r.error, JSON.stringify(r.error)); assert.equal(r.result, companionHash); return true;
  }, 15000, 150);
  const receipt = await ordinary(l1, from);
  const [nodeNonce, frontNonce, head] = await Promise.all([
    rpc(l1, 'eth_getTransactionCount', [from, 'latest']), rpc(front, 'eth_getTransactionCount', [from, 'latest']),
    rpc(l1, 'eth_getBlockByNumber', ['latest', false])]);
  assert.equal(nodeNonce, frontNonce);
  const item = { round, receiptObservedAt: new Date().toISOString(), receipt, nodeNonce, frontNonce,
    latestBlock: { number: head.number, hash: head.hash },
    companion: { transaction: companionTransaction, quote: companionQuote, hash: companionHash }, observations: [] };
  evidence.rounds.push(item);
  const quotes = await Promise.all(['eez_quoteFee', 'eez_debugQuoteFee'].flatMap(method =>
    [quote({ ...transaction, nonce: nodeNonce }, method), quote(transaction, method)]));
  item.observations.push(...quotes); save();
  for (const q of quotes) {
    if (q.response.error) {
      assert(['stale_context', 'quote_unavailable', 'quote_busy', 'quote_timeout'].includes(q.response.error.data?.code), JSON.stringify(q));
    } else {
      assert(usable(q), JSON.stringify(q));
      assert.equal(q.response.result.nonce, nodeNonce, 'never return a spent omitted nonce');
      assert.equal(q.response.result.options.length, 2);
    }
  }
  const [explicit, omitted] = quotes;
  console.log(JSON.stringify({ round, block: receipt.blockNumber, nodeNonce,
    explicit: explicit.response.error || explicit.response.result?.nonce,
    omitted: omitted.response.error || omitted.response.result?.nonce }));
  await until('same explicit request succeeds', async () => {
    const q = await quote({ ...transaction, nonce: nodeNonce }); item.observations.push(q); save();
    return usable(q);
  }, 15000, 150);
  item.companion.receipt = await until('companion settled', () => rpc(l1, 'eth_getTransactionReceipt', [companionHash]), 120000);
  assert.equal(item.companion.receipt.status, '0x1');
  item.companion.sameL1Block = item.companion.receipt.blockHash === receipt.blockHash;
  save();
  const controls = {
    realFutureNonce: await quote({ ...transaction, nonce: hex(BigInt(nodeNonce) + 1n) }),
    currentNonce: await quote({ ...transaction, nonce: nodeNonce }),
    omittedNonce: await quote(transaction),
    sourceNonceAfter: await rpc(l1, 'eth_getTransactionCount', [from, 'latest']),
    targetValue: await rpc(l2, 'eth_call', [{ to: target, data: cast('calldata', 'value()') }, 'latest']),
  };
  item.controls = controls; save();
  assert.equal(controls.realFutureNonce.response.error?.data?.code, 'invalid_transaction');
  assert(controls.realFutureNonce.response.error.message.includes('queued nonce prefix has a gap'));
  assert(usable(controls.currentNonce), JSON.stringify(controls.currentNonce));
  assert(usable(controls.omittedNonce), JSON.stringify(controls.omittedNonce));
  assert.equal(controls.currentNonce.response.result.nonce, nodeNonce);
  assert.equal(controls.omittedNonce.response.result.nonce, nodeNonce);
  assert.equal(controls.sourceNonceAfter, nodeNonce, 'quotes must not consume a nonce');
  assert.equal(BigInt(controls.targetValue), BigInt(84 + round), 'only companion executes on L2');
}
assert(evidence.rounds.some(round => round.companion.sameL1Block), 'must overlap ordinary source and cross-chain settlement');
evidence.passed = true; save();
console.log('PASS: explicit/omitted nonces, both RPC methods and fee encodings, genuine gap and canonical effects.');
