#!/usr/bin/env node
// Authorized Chiado workload. Uses the existing UI's pinned viem installation
// for in-process signing so spawning cast cannot limit the measured arrival rate.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, appendFileSync, existsSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { feeQuoteTransaction } from '../sdk/l2-fees.mjs';

const require = createRequire(resolve(process.env.EEZ_LOAD_UI_ROOT, 'package.json'));
const { encodeFunctionData, decodeFunctionResult, parseAbi, encodeAbiParameters,
  keccak256, toHex, concat, getCreate2Address, padHex, stringToHex } = require('viem');
const { privateKeyToAccount } = require('viem/accounts');
const out = process.env.EEZ_LOAD_OUTPUT;
assert(out && process.env.SOURCE_PK, 'Output and dedicated test funder required');
const root = process.cwd();
const L1 = 'http://127.0.0.1:18645', L2 = 'http://127.0.0.1:18688';
const F1 = 'http://127.0.0.1:18999', F2 = 'http://127.0.0.1:18998';
const EXT = 'https://rpc.chiadochain.net';
const registry = process.env.EEZ_REGISTRY_ADDRESS;
const manager = process.env.EEZL2_ADDRESS;
const funder = privateKeyToAccount(process.env.SOURCE_PK);
const kinds = ['nested', 'safe_create', 'safe_nested', 'safe_transfer'];
const actorCount = 64, actorFunding = 14_000_000_000_000_000n;
const maxSourceFee = 10_000_000_000_000_000n;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const json = value => JSON.stringify(value, (_, v) => typeof v === 'bigint' ? v.toString() : v);
const save = (name, value) => writeFileSync(`${out}/${name}`, json(value) + '\n', { mode: 0o600 });
const record = (name, value) => appendFileSync(`${out}/${name}.jsonl`, json(value) + '\n', { mode: 0o600 });
const load = name => JSON.parse(readFileSync(`${out}/${name}`, 'utf8'));
const progress = (message, detail = {}) => console.log(json({ at: new Date().toISOString(), message, ...detail }));
const call = (signature, args = []) => encodeFunctionData({ abi: parseAbi([`function ${signature}`]), args });
let rpcId = 0;
let externalTurn = Promise.resolve();
async function response(url, method, params = [], timeout = 15000) {
  // The public provider corroborates evidence; it is never the load target.
  if (url === EXT) {
    const turn = externalTurn;
    externalTurn = turn.then(() => sleep(250));
    await turn;
  }
  const res = await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' },
    body: json({ jsonrpc: '2.0', id: ++rpcId, method, params }), signal: AbortSignal.timeout(timeout) });
  assert(res.ok, `RPC HTTP ${res.status}`);
  return res.json();
}
async function rpc(url, method, params = []) {
  const res = await response(url, method, params);
  assert(!res.error, `${method}: ${json(res.error)}`);
  return res.result;
}
async function until(label, fn, seconds = 240) {
  const end = performance.now() + seconds * 1000;
  while (performance.now() < end) {
    const r = await fn();
    if (r) return r;
    await sleep(1000);
  }
  throw new Error(`Timed out: ${label}`);
}
async function receipt(url, hash) {
  const r = await until(`receipt ${hash}`, () => rpc(url, 'eth_getTransactionReceipt', [hash]));
  assert.equal(r.status, '0x1', `reverted ${hash}`);
  return r;
}
async function read(url, to, signature, args = [], tag = 'latest') {
  const abi = parseAbi([`function ${signature}`]);
  return decodeFunctionResult({ abi, data: await rpc(url, 'eth_call', [{ to, data: encodeFunctionData({ abi, args }) }, tag]) });
}
function identity() {
  return JSON.parse(execFileSync('docker', ['inspect', 'eez-node-chiado', 'eez-proof-signer-chiado'], { encoding: 'utf8' }))
    .map(d => ({ name: d.Name, id: d.Id, image: d.Image, started: d.State.StartedAt,
      restarts: d.RestartCount, running: d.State.Running,
      source: d.Config.Labels['org.opencontainers.image.revision'], protocol: d.Config.Labels['io.eez.protocol.revision'] }));
}
let state = existsSync(`${out}/private-state.json`) ? load('private-state.json') : {
  seed: (BigInt(Date.now()) << 32n).toString(), owner: toHex(randomBytes(32)), setup: {},
  actors: Array.from({ length: actorCount }, (_, i) => ({ key: toHex(randomBytes(32)), kind: kinds[i % 4], nonce: 0, safeNonce: 0 })),
  quoteKey: toHex(randomBytes(32)), fixtures: {},
};
const checkpoint = () => save('private-state.json', state);
checkpoint();
const owner = privateKeyToAccount(state.owner);
const qActor = privateKeyToAccount(state.quoteKey);
const setupOwner = (to = '0x0000000000000000000000000000000000000000', data = '0x') =>
  call('setup(address[],uint256,address,bytes,address,address,uint256,address)',
    [[owner.address], 1n, to, data, '0x0000000000000000000000000000000000000000', '0x0000000000000000000000000000000000000000', 0n, '0x0000000000000000000000000000000000000000']);

// A setup hash is retained before waiting. Resuming observes the identical hash;
// it never funds/deploys twice after an ambiguous timeout.
async function ordinary(key, url, to, data = '0x', value = 0n, gas = 7_000_000n) {
  if (state.setup[key]) return receipt(url === F1 ? L1 : url, state.setup[key]);
  const chainId = Number(BigInt(await rpc(url, 'eth_chainId')));
  const nonce = Number(BigInt(await rpc(url, 'eth_getTransactionCount', [funder.address, 'pending'])));
  const block = await rpc(url, 'eth_getBlockByNumber', ['latest', false]);
  const tip = chainId === 10200 ? 1_000_000_000n : 10_000_000n;
  const raw = await funder.signTransaction({ chainId, nonce, to, data, value, gas,
    maxPriorityFeePerGas: tip, maxFeePerGas: 2n * BigInt(block.baseFeePerGas) + tip });
  const hash = keccak256(raw);
  state.setup[key] = hash; checkpoint();
  record('setup', { key, hash, chainId, to, value });
  assert.equal(await rpc(url, 'eth_sendRawTransaction', [raw]), hash);
  return receipt(url === F1 ? L1 : url, hash);
}
async function deploy(key, url, bytecode, args = '0x') {
  if (state.fixtures[key]) return state.fixtures[key];
  const r = await ordinary(key, url, undefined, concat([bytecode, args]));
  assert(r.contractAddress); state.fixtures[key] = r.contractAddress; checkpoint();
  return r.contractAddress;
}
async function proxy(key, url, mgr, target, rid) {
  if (state.fixtures[key]) return state.fixtures[key];
  const address = await read(url, mgr, 'computeCrossChainProxyAddress(address,uint64) view returns (address)', [target, rid]);
  if (await rpc(url, 'eth_getCode', [address, 'latest']) === '0x')
    await ordinary(key, url, mgr, call('createCrossChainProxy(address,uint64)', [target, rid]));
  state.fixtures[key] = address; checkpoint(); return address;
}
const artifact = name => JSON.parse(readFileSync(`${root}/eez-core-protocol/out/CounterContracts.sol/${name}.json`)).bytecode.object;
function createData(initializer, salt) {
  return call('createProxyWithNonce(address,bytes,uint256)', [state.fixtures.singleton, initializer, salt]);
}
function safeAddress(initializer, salt) {
  return getCreate2Address({ from: state.fixtures.factory,
    salt: keccak256(encodeAbiParameters([{ type: 'bytes32' }, { type: 'uint256' }], [keccak256(initializer), salt])),
    bytecode: concat([state.fixtures.proxyCode, padHex(state.fixtures.singleton, { size: 32 })]) });
}
function creation(id, nested) {
  const salt = BigInt(state.seed) + BigInt(id) * 10n;
  let initializer = setupOwner();
  const addresses = [];
  if (nested) {
    const packed = [];
    for (let i = 1; i <= 4; i++) {
      const data = createData(initializer, salt + BigInt(i));
      packed.push(concat(['0x00', state.fixtures.factory, padHex('0x0', { size: 32 }),
        toHex(BigInt((data.length - 2) / 2), { size: 32 }), data]));
      addresses.push(safeAddress(initializer, salt + BigInt(i)));
    }
    initializer = setupOwner(state.fixtures.multisend, call('multiSend(bytes)', [concat(packed)]));
  }
  addresses.push(safeAddress(initializer, salt));
  return { to: state.fixtures.factoryProxy, data: createData(initializer, salt), safes: addresses };
}
async function transfer(actor) {
  const zero = '0x0000000000000000000000000000000000000000';
  const types = { SafeTx: [
    { name: 'to', type: 'address' }, { name: 'value', type: 'uint256' }, { name: 'data', type: 'bytes' },
    { name: 'operation', type: 'uint8' }, { name: 'safeTxGas', type: 'uint256' }, { name: 'baseGas', type: 'uint256' },
    { name: 'gasPrice', type: 'uint256' }, { name: 'gasToken', type: 'address' }, { name: 'refundReceiver', type: 'address' }, { name: 'nonce', type: 'uint256' }] };
  const message = { to: actor.recipient, value: 1n, data: '0x', operation: 0, safeTxGas: 0n, baseGas: 0n,
    gasPrice: 0n, gasToken: zero, refundReceiver: zero, nonce: BigInt(actor.safeNonce) };
  const signature = await owner.signTypedData({ domain: { chainId: 10200, verifyingContract: actor.safe },
    primaryType: 'SafeTx', types, message });
  return { to: actor.safeProxy, data: call('execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes) returns (bool)',
    [message.to, 1n, '0x', 0, 0n, 0n, 0n, zero, zero, signature]), safe: actor.safe,
    safeNonce: actor.safeNonce, recipient: actor.recipient };
}
async function intent(kind, id, actor) {
  const payload = kind === 'nested' ? { to: state.fixtures.counterProxy, data: call('incrementProxy()') }
    : kind === 'safe_transfer' ? await transfer(actor)
    : creation(id, kind === 'safe_nested');
  const account = privateKeyToAccount(actor.key);
  return { payload, request: { from: account.address, to: payload.to, data: payload.data, value: '0x0' } };
}
async function preflight() {
  assert.equal(BigInt(await rpc(L1, 'eth_chainId')), 10200n);
  assert.equal(BigInt(await rpc(EXT, 'eth_chainId')), 10200n);
  assert.equal(BigInt(await rpc(L2, 'eth_chainId')), 906969n);
  assert.notEqual(funder.address.toLowerCase(), '0x2ea2a5a1fe1ba4f4134c993e0c9a700706031e52');
  const ids = identity();
  assert.equal(ids[0].image, 'sha256:712c027cd6ee482adb449d53783dc75353e5feca2b0d8ab50962875a0d5cac09');
  assert.equal(ids[1].image, 'sha256:00cdbf0a414b3bfeaaf028c38901d84c7286fbae778cffc2d60c9d6687303250');
  assert(ids.every(d => d.running && d.source === 'b2a82536e87d2543ce9839eb3ba15a9ceefdaefc' && d.protocol === '9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f'));
  const queue = await rpc(F2, 'eez_getCrossChainQueueStatus');
  assert(['queued', 'inFlight', 'blocked', 'unknown'].every(k => queue[k] === 0), 'Shared queue not idle');
  for (const url of [L1, L2]) {
    const h = await rpc(url, 'eth_getBlockByNumber', ['latest', false]);
    assert(Date.now() / 1000 - Number(BigInt(h.timestamp)) < 30, 'Chain stale');
    assert.equal(await rpc(url, 'eth_getTransactionCount', [funder.address, 'latest']),
      await rpc(url, 'eth_getTransactionCount', [funder.address, 'pending']), 'Funder has pending transactions');
  }
  save('identity.json', { runtime: ids, viem: require('viem/package.json').version, at: Date.now(),
    source: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
    funder: funder.address, actorCount, actorFunding, quoteTypes: ['0x0', '0x2'] });
}
async function prepare() {
  await preflight();
  progress('Preparing isolated workload fixtures');
  const balance = BigInt(await rpc(L2, 'eth_getBalance', [funder.address, 'latest']));
  if (balance < 1_000_000_000_000_000_000n && !state.setup.bridge) {
    assert(BigInt(await rpc(L1, 'eth_getBalance', [funder.address, 'latest'])) > 1_250_000_000_000_000_000n, 'Insufficient test funding');
    const p = await proxy('fundingProxy', L1, registry, funder.address, 1n);
    const before = BigInt(await rpc(L2, 'eth_getBalance', [funder.address, 'latest']));
    await ordinary('bridge', F1, p, '0x', 1_100_000_000_000_000_000n, 500_000n);
    await until('test funds on L2', async () => BigInt(await rpc(L2, 'eth_getBalance', [funder.address, 'latest'])) >= before + 1_100_000_000_000_000_000n);
    progress('Bridged 1.1 test xDAI for the bounded workload');
  }
  const fixtures = JSON.parse(readFileSync('crates/eez-node-e2e/tests/fixtures/safe-1.4.1/bytecode.json')).contracts;
  for (const [key, name] of [['singleton', 'Safe'], ['factory', 'SafeProxyFactory'], ['multisend', 'MultiSend']]) {
    const address = await deploy(key, L1, fixtures[name].bytecode);
    const code = await rpc(L1, 'eth_getCode', [address, 'latest']);
    if (fixtures[name].runtimeHash) assert.equal(keccak256(code), fixtures[name].runtimeHash);
  }
  state.fixtures.proxyCode = await read(L1, state.fixtures.factory, 'proxyCreationCode() pure returns (bytes)'); checkpoint();
  await proxy('factoryProxy', L2, manager, state.fixtures.factory, 0n);
  const counter = await deploy('counter', L2, artifact('Counter'));
  const callback = await proxy('callback', L1, registry, counter, 1n);
  const cap = await deploy('cap', L1, artifact('CounterAndProxy'), encodeAbiParameters([{ type: 'address' }], [callback]));
  await proxy('counterProxy', L2, manager, cap, 0n);
  // Setup is deliberately outside measured intervals. Sequential setup keeps
  // nonce ownership explicit; load signing later happens entirely in-process.
  for (let i = 0; i < state.actors.length; i++) {
    const actor = state.actors[i]; const a = privateKeyToAccount(actor.key);
    await ordinary(`fund-${i}`, L2, a.address, '0x', actorFunding, 21000n);
    if (actor.kind === 'safe_transfer') await prepareSafe(actor, i);
    if (i % 8 === 7) progress('Test actors prepared', { count: i + 1 });
  }
  await ordinary('fund-quote', L2, qActor.address, '0x', actorFunding, 21000n);
  state.quoteActor ??= { key: state.quoteKey, safeNonce: 0 };
  await prepareSafe(state.quoteActor, 1000);
  const lastSetup = await receipt(L2, state.setup['safe-proxy-1000']);
  await until('fixture canonical settlement', async () => {
    const safe = await rpc(L2, 'eth_getBlockByNumber', ['safe', false]);
    const current = await rpc(L2, 'eth_getTransactionReceipt', [lastSetup.transactionHash]);
    return current && BigInt(safe.number) >= BigInt(current.blockNumber);
  });
  state.prepared = true; checkpoint();
  save('fixtures.json', { ...state.fixtures, owner: owner.address,
    quoteActor: qActor.address, actors: state.actors.map(a => ({ address: privateKeyToAccount(a.key).address, kind: a.kind, safe: a.safe, recipient: a.recipient })) });
  progress('Workload fixtures ready');
}
async function prepareSafe(actor, index) {
  const init = setupOwner(), salt = BigInt(state.seed) + 50_000_000n + BigInt(index);
  actor.safe = safeAddress(init, salt);
  actor.recipient ??= privateKeyToAccount(toHex(randomBytes(32))).address;
  await ordinary(`safe-${index}`, L1, state.fixtures.factory, createData(init, salt));
  await ordinary(`safe-fund-${index}`, L1, actor.safe, '0x', 10000n, 100000n);
  actor.safeProxy = await proxy(`safe-proxy-${index}`, L2, manager, actor.safe, 0n);
  checkpoint();
}

let stopped = null, monitoring = true, monitorError = null;
const accepted = [], outstanding = new Set(), stages = [];
let txSequence = 0, quoteSequence = 0, budgetReserved = 0n;
async function timedQuote(request, stage, kind, usage = 'load') {
  const start = performance.now(), at = Date.now();
  let result, code = 'ok', error;
  try {
    const r = await response(F2, 'eez_quoteFee', [request, { transactionTypes: ['0x0', '0x2'] }]);
    if (r.error) { code = r.error.data?.code ?? String(r.error.code); error = r.error; }
    else {
      result = r.result;
      if (result.options.some(x => x.error)) { code = result.options.find(x => x.error).error.code; error = result.options; }
    }
  } catch (e) { code = e.name; error = e.message; }
  const row = { at, stage, kind, usage, ms: performance.now() - start, code, error };
  record('quotes', row);
  return { result, row };
}
async function submit(stage, kind, actor) {
  actor.busy = true;
  const id = ++txSequence, start = Date.now();
  const account = privateKeyToAccount(actor.key);
  try {
    const { payload, request } = await intent(kind, id, actor);
    const { result: quote, row } = await timedQuote(request, stage, kind, 'submission');
    if (row.code !== 'ok') { record('submissions', { id, stage, kind, start, code: `quote:${row.code}` }); actor.busy = false; return; }
    const type = Math.floor((id - 1) / 4) % 2 ? '0x0' : '0x2';
    const tx = feeQuoteTransaction({ quote, transaction: request, type, chainId: toHex(906969),
      keccak256: s => keccak256(stringToHex(s)), maxTotalFee: maxSourceFee });
    assert.equal(Number(BigInt(tx.nonce)), actor.nonce, 'Sender nonce drift');
    const maxFee = BigInt(tx.gas) * BigInt(type === '0x0' ? tx.gasPrice : tx.maxFeePerGas);
    if (budgetReserved + maxFee > 800_000_000_000_000_000n) { stopped = 'bounded_fee_budget'; actor.busy = false; return; }
    assert(BigInt(await rpc(L2, 'eth_getBalance', [account.address, 'latest'])) >= maxFee, 'Test actor balance below quoted maximum');
    const transaction = { chainId: 906969, nonce: actor.nonce, to: tx.to, data: tx.data, value: 0n, gas: BigInt(tx.gas),
      ...(type === '0x0' ? { type: 'legacy', gasPrice: BigInt(tx.gasPrice) } : { type: 'eip1559', maxFeePerGas: BigInt(tx.maxFeePerGas), maxPriorityFeePerGas: BigInt(tx.maxPriorityFeePerGas) }) };
    const raw = await account.signTransaction(transaction), hash = keccak256(raw);
    const entry = { id, stage, kind, start, sentAt: Date.now(), actorIndex: state.actors.indexOf(actor), hash, payload, type, transaction, quote };
    record('signed', { ...entry, raw });
    const begin = performance.now();
    let res;
    try { res = await response(F2, 'eth_sendRawTransaction', [raw]); }
    catch (e) { stopped = 'ambiguous_submission'; entry.ambiguous = true; accepted.push(entry); entry.actor = actor; record('submissions', { id, stage, kind, hash, code: e.name, ms: performance.now() - begin }); return; }
    const code = res.error?.data?.code ?? (res.error ? String(res.error.code) : 'ok');
    record('submissions', { id, stage, kind, hash, start, at: Date.now(), ms: performance.now() - begin, code, error: res.error });
    if (res.error) { actor.busy = false; return; }
    assert.equal(res.result, hash);
    budgetReserved += maxFee;
    entry.actor = actor; entry.acceptedAt = Date.now(); accepted.push(entry);
  } catch (e) {
    actor.busy = false; stopped = `client_validation:${e.message}`;
    record('submissions', { id, stage, kind, start, code: 'client_validation', error: e.message });
  }
}
async function observe() {
  while (monitoring) {
    const at = Date.now();
    try {
      const [queue, head, safe] = await Promise.all([rpc(F2, 'eez_getCrossChainQueueStatus'),
        rpc(L1, 'eth_getBlockByNumber', ['latest', false]), rpc(L2, 'eth_getBlockByNumber', ['safe', false])]);
      const active = accepted.filter(e => !e.settledAt);
      record('observations', { at, queue, l1: { number: head.number, hash: head.hash, timestamp: head.timestamp },
        safe: { number: safe.number, hash: safe.hash }, pendingTestSources: active.length });
      if (Date.now() / 1000 - Number(BigInt(head.timestamp)) > 45) stopped ??= 'stale_l1';
      if (queue.queued + queue.inFlight > 64) stopped ??= 'backlog_bound';
      const correlations = new Map();
      for (let offset = 0; offset < active.length; offset += 8) await Promise.all(active.slice(offset, offset + 8).map(async e => {
        const r = await rpc(L2, 'eth_getTransactionReceipt', [e.hash]);
        if (!r || BigInt(r.blockNumber) > BigInt(safe.number)) return;
        assert.equal(r.status, '0x1', `source reverted ${e.hash}`);
        if (!correlations.has(r.blockHash)) correlations.set(r.blockHash, rpc(F2, 'eez_getSettlementByL2Block', [r.blockHash]));
        const settlement = await correlations.get(r.blockHash);
        if (!settlement?.canonicalL2) return;
        const ownership = await rpc(F2, 'eez_getCrossChainTransaction', [e.hash]);
        if (!ownership?.ownership?.canonical) return;
        assert.equal(ownership.ownership.reason, 'Committed');
        const block = await rpc(L2, 'eth_getBlockByNumber', [r.blockNumber, false]);
        if (block.hash !== r.blockHash) return;
        e.receipt = r; e.settlement = settlement; e.block = block; e.settledAt = Date.now();
        e.actor.nonce++; if (e.kind === 'safe_transfer') e.actor.safeNonce++;
        e.actor.busy = false;
        record('settlements', { id: e.id, stage: e.stage, kind: e.kind, hash: e.hash, sentAt: e.sentAt,
          observedAt: e.settledAt, latencyMs: e.settledAt - e.sentAt, blockNumber: r.blockNumber,
          blockHash: r.blockHash, postHash: settlement.l1TransactionHash, gasUsed: r.gasUsed });
      }));
      checkpoint();
    } catch (e) { monitorError = e.message; stopped ??= 'observation_failure'; record('observation-errors', { at, error: e.message }); }
    await sleep(1000);
  }
}
async function arrivalStream(rate, seconds, fn, label) {
  const start = performance.now(), count = Math.round(rate * seconds);
  for (let i = 0; i < count && !stopped; i++) {
    const due = start + i * 1000 / rate;
    await sleep(Math.max(0, due - performance.now()));
    const lag = performance.now() - due;
    if (lag > 250 || outstanding.size >= 128) { record('generator-skips', { at: Date.now(), label, lag, outstanding: outstanding.size }); continue; }
    const p = fn(i).catch(e => {
      stopped ??= 'generator_failure';
      record('generator-errors', { at: Date.now(), label, error: e.message });
    }).finally(() => outstanding.delete(p)); outstanding.add(p);
  }
  await sleep(Math.max(0, start + seconds * 1000 - performance.now()));
}
async function drain() {
  await Promise.allSettled([...outstanding]);
  await until('canonical drain', async () => accepted.every(e => e.settledAt), 300);
  const q = await rpc(F2, 'eez_getCrossChainQueueStatus');
  assert(['queued', 'inFlight', 'blocked', 'unknown'].every(k => q[k] === 0), 'Queue not empty after drain');
}
async function run() {
  assert(state.prepared, 'Prepare first'); assert(!existsSync(`${out}/run-start.json`), 'A measured run cannot be resumed or silently repeated');
  // The measured nested-Safe quote is about twice the simple-call price.
  // Give those actors a larger reserve from the already bridged test budget.
  let lastFunding;
  for (let i = 0; i < state.actors.length; i++) {
    const actor = state.actors[i];
    if (actor.kind !== 'safe_nested') continue;
    lastFunding = await ordinary(`nested-reserve-${i}`, L2, privateKeyToAccount(actor.key).address, '0x', 8_000_000_000_000_000n, 21000n);
    actor.funding = (actorFunding + 8_000_000_000_000_000n).toString(); checkpoint();
  }
  await until('fixture canonical settlement', async () => {
    const r = await rpc(L2, 'eth_getTransactionReceipt', [lastFunding.transactionHash]);
    const safe = await rpc(L2, 'eth_getBlockByNumber', ['safe', false]);
    return r && BigInt(safe.number) >= BigInt(r.blockNumber);
  });
  await preflight(); save('run-start.json', { at: Date.now() });
  const observer = observe();
  try {
    // Each real path must settle once before the offered-rate intervals start.
    for (const kind of kinds) {
      const actor = state.actors.find(a => a.kind === kind);
      const n = accepted.length;
      await submit('canary', kind, actor);
      assert.equal(accepted.length, n + 1, `${kind} canary not accepted`);
      await drain(); assert(!stopped, stopped);
      progress('Canary canonically settled', { kind });
    }
    const cursors = Object.fromEntries(kinds.map(k => [k, 0]));
    for (const [quoteRate, txRate, seconds] of [[10, .5, 60], [25, 1, 60], [50, 2, 60], [100, 4, 60], [25, 8, 30]]) {
      if (stopped) break;
      const name = `q${quoteRate}-tx${txRate}`, begin = Date.now();
      const stage = { name, quoteRate, txRate, durationSeconds: seconds, begin }; stages.push(stage);
      progress('Starting measured stage', stage);
      await Promise.all([
        arrivalStream(quoteRate, seconds, async i => {
          const kind = kinds[i % 4];
          const { request } = await intent(kind, 100_000 + (++quoteSequence), state.quoteActor);
          await timedQuote(request, name, kind);
        }, `${name}:quote`),
        arrivalStream(txRate, seconds, async i => {
          const kind = kinds[i % 4];
          const pool = state.actors.filter(a => a.kind === kind);
          let actor;
          for (let step = 0; step < pool.length; step++) {
            const index = cursors[kind]++ % pool.length;
            if (!pool[index].busy) { actor = pool[index]; break; }
          }
          if (!actor) { record('generator-skips', { at: Date.now(), label: `${name}:actor`, kind }); return; }
          await submit(name, kind, actor);
        }, `${name}:transaction`),
      ]);
      stage.end = Date.now(); stage.stopped = stopped;
      await drain(); stage.drained = Date.now(); save('stages.json', stages);
      progress('Stage drained', { name, accepted: accepted.filter(e => e.stage === name).length, stopped });
    }
  } finally {
    await Promise.allSettled([...outstanding]);
    save('stages.json', stages);
    try { await drain(); } catch (e) { save('drain-error.json', { error: e.message }); }
    monitoring = false; await observer;
    save('accepted.json', accepted.map(({ actor, ...entry }) => entry));
    save('run-end.json', { at: Date.now(), stopped, monitorError, accepted: accepted.length,
      settled: accepted.filter(e => e.settledAt).length, budgetReserved });
    assert.deepEqual(identity(), load('identity.json').runtime, 'Runtime identity changed');
  }
}

async function verify() {
  const entries = load('accepted.json');
  assert(entries.length > 0 && entries.every(e => e.settledAt), 'Every admitted source must settle');
  const l1 = await rpc(L1, 'eth_getBlockByNumber', ['latest', false]);
  const l2 = await rpc(L2, 'eth_getBlockByNumber', ['safe', false]);
  assert.equal((await until('independent L1 snapshot availability', () => rpc(EXT, 'eth_getBlockByNumber', [l1.number, false]), 60)).hash, l1.hash);
  const posts = new Map(), sourceEvidence = [], semantic = [];
  for (let offset = 0; offset < entries.length; offset += 8) {
    await Promise.all(entries.slice(offset, offset + 8).map(async e => {
      const r = await rpc(L2, 'eth_getTransactionReceipt', [e.hash]);
      assert.equal(r.status, '0x1');
      assert.equal(r.blockHash, e.receipt.blockHash, 'Source receipt changed since measured settlement');
      assert(BigInt(r.blockNumber) <= BigInt(l2.number));
      const b = await rpc(L2, 'eth_getBlockByNumber', [r.blockNumber, false]);
      assert.equal(b.hash, r.blockHash); assert(b.transactions.includes(e.hash));
      const ownership = await rpc(F2, 'eez_getCrossChainTransaction', [e.hash]);
      assert(ownership.ownership.canonical && ownership.ownership.reason === 'Committed');
      const settlement = await rpc(F2, 'eez_getSettlementByL2Block', [r.blockHash]);
      assert(settlement.canonicalL2 && settlement.l1TransactionHash === e.settlement.l1TransactionHash);
      const tip = BigInt(r.gasUsed) * (BigInt(r.effectiveGasPrice) - BigInt(b.baseFeePerGas));
      const item = { hash: e.hash, kind: e.kind, stage: e.stage, type: e.type, receipt: r, tip: tip.toString(), post: settlement.l1TransactionHash };
      sourceEvidence.push(item);
      if (!posts.has(item.post)) posts.set(item.post, { hash: item.post, settlement, sources: [] });
      posts.get(item.post).sources.push(item);
    }));
  }
  save('canonical-sources.json', sourceEvidence);
  const eventTopic = keccak256(stringToHex('L2ExecutionPerformed(uint64,bytes32,uint256)'));
  const postEvidence = [];
  for (const post of posts.values()) {
    const [r, other] = await Promise.all([rpc(L1, 'eth_getTransactionReceipt', [post.hash]), rpc(EXT, 'eth_getTransactionReceipt', [post.hash])]);
    assert(r && other && r.status === '0x1' && r.type === '0x3');
    for (const k of ['transactionHash', 'blockHash', 'blockNumber', 'gasUsed', 'effectiveGasPrice', 'blobGasUsed', 'blobGasPrice', 'status']) assert.equal(r[k], other[k], `Independent posting ${k}`);
    const localBlock = await rpc(L1, 'eth_getBlockByNumber', [r.blockNumber, false]);
    assert.equal(localBlock.hash, r.blockHash);
    assert.equal((await rpc(EXT, 'eth_getBlockByNumber', [r.blockNumber, false])).hash, r.blockHash);
    const query = { blockHash: r.blockHash, address: registry, topics: [eventTopic, toHex(1n, { size: 32 })] };
    const [localEvents, remoteEvents] = await Promise.all([rpc(L1, 'eth_getLogs', [query]), rpc(EXT, 'eth_getLogs', [query])]);
    const fields = ['address', 'topics', 'data', 'blockHash', 'blockNumber', 'transactionHash', 'transactionIndex', 'logIndex'];
    const project = logs => logs.map(log => fields.map(k => log[k]));
    assert(localEvents.length && !localEvents.some(log => log.removed));
    assert.deepEqual(project(localEvents), project(remoteEvents));
    const last = localEvents.at(-1);
    assert.equal(last.data.slice(0, 66), post.settlement.l2Range.lastBlockHash, 'Incomplete canonical consumption');
    assert.equal((await rpc(EXT, 'eth_getTransactionReceipt', [last.transactionHash])).status, '0x1');
    const terminal = await rpc(L2, 'eth_getBlockByNumber', [post.settlement.l2Range.lastBlockNumber, false]);
    assert.equal(terminal.hash, post.settlement.l2Range.lastBlockHash);
    const cost = BigInt(r.gasUsed) * BigInt(r.effectiveGasPrice) + BigInt(r.blobGasUsed) * BigInt(r.blobGasPrice);
    const paid = post.sources.reduce((sum, e) => sum + BigInt(e.tip), 0n);
    const result = { hash: post.hash, blockTimestamp: localBlock.timestamp, receipt: r, sourceCount: post.sources.length,
      postingAndBlobCost: cost, collectedTestTips: paid, covered: paid >= cost,
      firstL2: post.settlement.l2Range.firstBlockNumber, lastL2: post.settlement.l2Range.lastBlockNumber,
      lastL2Hash: terminal.hash, events: localEvents };
    postEvidence.push(result);
    save('canonical-postings.json', postEvidence);
    assert(paid >= cost, `Collected test tips below whole posting/blob cost: ${post.hash}`);
  }
  const nested = entries.filter(e => e.kind === 'nested').length;
  for (const [url, address, signature] of [[L2, state.fixtures.counter, 'counter() view returns (uint256)'],
    [L1, state.fixtures.cap, 'counter() view returns (uint256)'], [L1, state.fixtures.cap, 'targetCounter() view returns (uint256)']]) {
    const tag = url === L2 ? l2.number : l1.number;
    assert.equal(await read(url, address, signature, [], tag), BigInt(nested));
    if (url === L1) assert.equal(await read(EXT, address, signature, [], tag), BigInt(nested));
    semantic.push({ check: signature, address, expected: nested, block: tag });
  }
  const created = entries.flatMap(e => e.payload.safes ?? []);
  for (let offset = 0; offset < created.length; offset += 6) {
    await Promise.all(created.slice(offset, offset + 6).map(async address => {
      assert.deepEqual((await read(L1, address, 'getOwners() view returns (address[])', [], l1.number)).map(x => x.toLowerCase()), [owner.address.toLowerCase()]);
      assert.deepEqual((await read(EXT, address, 'getOwners() view returns (address[])', [], l1.number)).map(x => x.toLowerCase()), [owner.address.toLowerCase()]);
      assert.equal(await read(L1, address, 'getThreshold() view returns (uint256)', [], l1.number), 1n);
      assert.equal((await read(L1, address, 'masterCopy() view returns (address)', [], l1.number)).toLowerCase(), state.fixtures.singleton.toLowerCase());
    }));
  }
  semantic.push({ check: 'created Safe owners, threshold and implementation', count: created.length });
  for (let i = 0; i < state.actors.length; i++) {
    const actor = state.actors[i], own = entries.filter(e => e.actorIndex === i);
    const address = privateKeyToAccount(actor.key).address;
    const fee = sourceEvidence.filter(e => own.some(o => o.hash === e.hash)).reduce((v, e) => v + BigInt(e.receipt.gasUsed) * BigInt(e.receipt.effectiveGasPrice), 0n);
    assert.equal(BigInt(await rpc(L2, 'eth_getBalance', [address, l2.number])), BigInt(actor.funding ?? actorFunding) - fee, 'Exact actor gas accounting');
    assert.equal(BigInt(await rpc(L2, 'eth_getTransactionCount', [address, l2.number])), BigInt(own.length));
    if (actor.kind !== 'safe_transfer') continue;
    for (const url of [L1, EXT]) {
      assert.equal(await read(url, actor.safe, 'nonce() view returns (uint256)', [], l1.number), BigInt(own.length));
      assert.equal(BigInt(await rpc(url, 'eth_getBalance', [actor.recipient, l1.number])), BigInt(own.length));
      assert.equal(BigInt(await rpc(url, 'eth_getBalance', [actor.safe, l1.number])), 10000n - BigInt(own.length));
    }
    semantic.push({ check: 'Safe transfer nonce and exact balances', safe: actor.safe, recipient: actor.recipient, transfers: own.length });
  }
  assert.equal((await rpc(L1, 'eth_getBlockByNumber', [l1.number, false])).hash, l1.hash);
  assert.equal((await until('independent L1 snapshot availability', () => rpc(EXT, 'eth_getBlockByNumber', [l1.number, false]), 60)).hash, l1.hash);
  assert.equal((await rpc(L2, 'eth_getBlockByNumber', [l2.number, false])).hash, l2.hash);
  assert.deepEqual(identity(), load('identity.json').runtime);
  const q = await rpc(F2, 'eez_getCrossChainQueueStatus');
  assert(['queued', 'inFlight', 'blocked', 'unknown'].every(k => q[k] === 0));
  save('verification.json', { pass: true, at: Date.now(), sources: entries.length, postings: postEvidence.length,
    createdSafes: created.length, nestedRoundTrips: nested, semantic, independentL1: true, independentL2Replay: false,
    postingAndBlobCost: postEvidence.reduce((sum, p) => sum + BigInt(p.postingAndBlobCost), 0n),
    collectedTestTips: sourceEvidence.reduce((sum, e) => sum + BigInt(e.tip), 0n), l1Snapshot: l1.hash, l2Snapshot: l2.hash });
  progress('Canonical sources, independent L1 settlement, semantics and exact fee accounting verified', { sources: entries.length, postings: postEvidence.length, createdSafes: created.length });
}

const mode = process.argv[2];
try {
  if (mode === 'prepare') await prepare();
  else if (mode === 'run') await run();
  else if (mode === 'verify') await verify();
  else throw new Error('Choose prepare, run or verify');
} catch (e) { save(`${mode}-failure.json`, { at: Date.now(), error: e.message, stack: e.stack }); progress('Stopped with retained evidence', { error: e.message }); process.exitCode = 1; }
