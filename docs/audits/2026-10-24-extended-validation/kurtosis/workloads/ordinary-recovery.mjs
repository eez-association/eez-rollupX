// Separate signer-outage control: 24 ordinary transfers in 72 seconds.
// Head age is measured by the supervisor; baseline liveness is observational.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync, appendFileSync, mkdirSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
const require = createRequire(resolve(process.env.EEZ_LOAD_UI_ROOT, 'package.json'));
const { keccak256 } = require('viem');
const { privateKeyToAccount } = require('viem/accounts');
const config = JSON.parse(readFileSync(process.env.EEZ_LOAD_CONFIG, 'utf8'));
const out = process.env.EEZ_LOAD_OUTPUT;
assert(out && process.env.SOURCE_PK && process.env.KURTOSIS_ENCLAVE === config.enclave);
assert(config.chain_id === 696990, 'This audit accepts only the private fixture chain');
const account = privateKeyToAccount(process.env.SOURCE_PK);
delete process.env.SOURCE_PK;
assert.equal(account.address.toLowerCase(), config.sender.toLowerCase());
assert.notEqual(config.sender.toLowerCase(), config.recipient.toLowerCase());
const stages = [{ rate: 1 / 3, seconds: 72 }];
const maxConcurrent = 1, requests = new Set();
const maxFee = 100_000_000n, tip = 10_000_000n, gas = 21_000n, value = 1n;
const planned = stages.reduce((sum, stage) => sum + stage.rate * stage.seconds, 0);
const duration = stages.reduce((sum, stage) => sum + stage.seconds, 0);
const json = obj => JSON.stringify(obj, (_, v) => typeof v === 'bigint' ? v.toString() : v);
const save = (name, obj) => writeFileSync(`${out}/${name}`, json(obj) + '\n', { mode: 0o600 });
const event = (kind, data = {}) => appendFileSync(`${out}/events.jsonl`, json({ at: Date.now() / 1000, kind, ...data }) + '\n', { mode: 0o600 });
const sleep = ms => new Promise(resolve => setTimeout(resolve, Math.max(0, ms)));
mkdirSync(out);
let id = 0;
async function batch(url, calls) {
  const ids = calls.map(() => ++id);
  const body = calls.map(([method, params], index) => ({ jsonrpc: '2.0', id: ids[index], method, params }));
  const response = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: AbortSignal.timeout(10000) });
  assert(response.ok, `HTTP ${response.status}`);
  const rows = await response.json();
  assert(Array.isArray(rows) && rows.length === calls.length);
  const byId = new Map(rows.map(row => [row.id, row]));
  assert.equal(byId.size, calls.length);
  return ids.map((key, index) => {
    const row = byId.get(key);
    assert(row && !row.error, `${calls[index][0]}: ${json(row?.error)}`);
    return row.result;
  });
}
async function rpc(url, method, params = []) { return (await batch(url, [[method, params]]))[0]; }
function memory() {
  const text = readFileSync('/proc/meminfo', 'utf8');
  return Number(text.match(/^MemAvailable:\s+(\d+) kB$/m)[1]) * 1024;
}
function assertQueue(q, idle = false) {
  for (const key of ['queued', 'inFlight', 'blocked', 'unknown']) assert(Number.isSafeInteger(q[key]) && q[key] >= 0);
  if (idle) assert(['queued', 'inFlight', 'blocked', 'unknown'].every(key => q[key] === 0));
}
const l2 = config.l2, front = config.front;
assert(memory() >= 12 * 1024 ** 3, 'Insufficient initial shared-host memory');
assert.equal(await rpc(l2, 'eth_chainId'), `0x${config.chain_id.toString(16)}`);
assert.equal((await rpc(l2, 'eth_getBlockByNumber', ['0x0', false])).hash, config.genesis);
const before = await rpc(l2, 'eth_getBlockByNumber', ['safe', false]);
const firstNonce = Number(BigInt(await rpc(l2, 'eth_getTransactionCount', [account.address, before.number])));
assert.equal(Number(BigInt(await rpc(l2, 'eth_getTransactionCount', [account.address, 'pending']))), firstNonce);
const balanceBefore = BigInt(await rpc(l2, 'eth_getBalance', [account.address, before.number]));
const recipientBefore = BigInt(await rpc(l2, 'eth_getBalance', [config.recipient, before.number]));
assert.equal(await rpc(l2, 'eth_getCode', [config.recipient, before.number]), '0x');
assert(balanceBefore >= BigInt(planned) * (gas * maxFee + value));
assert(BigInt(before.baseFeePerGas) + tip <= maxFee);
const startQueue = await rpc(front, 'eez_getCrossChainQueueStatus');
assertQueue(startQueue, true); // This separate control begins after all mixed waves settle.
const meta = { runtime_source: config.runtime_source, enclave: config.enclave, stages, planned,
  duration_seconds: duration, sender: account.address, recipient: config.recipient, chain_id: config.chain_id,
  first_nonce: firstNonce, before, balance_before: balanceBefore, recipient_before: recipientBefore,
  gas, max_fee_per_gas: maxFee, priority_fee_per_gas: tip, value, maximum_gas_budget_wei: BigInt(planned) * gas * maxFee,
  viem: require('viem/package.json').version, initial_queue: startQueue, pending_cap: 128, concurrent_rpc_cap: maxConcurrent, broadcasts_retried: 0 };
save('metadata.json', meta);
const accepted = [], firstSeen = new Map(), observations = [], canonicalObserved = new Map();
let latestNonce = firstNonce, nextNonce = firstNonce, highestSeen = Number(BigInt(before.number));
let stop = false, fatal = null, skippedSchedule = 0, withheld = 0, offered = 0;
let began = performance.now(), wallStart = Date.now() / 1000;
meta.started_at = wallStart;
save('metadata.json', meta);
function fail(error) {
  fatal ??= String(error);
  event('stopped', { error: fatal });
  save('stop.json', { at: Date.now() / 1000, error: fatal });
  stop = true;
}
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => fail(`received ${signal}`));
const observer = (async () => {
  let lastQueue = -Infinity;
  while (!stop) {
    const sampled = performance.now();
    try {
      const [head, safe, nonce] = await batch(l2, [['eth_getBlockByNumber', ['latest', false]], ['eth_getBlockByNumber', ['safe', false]], ['eth_getTransactionCount', [account.address, 'latest']]]);
      latestNonce = Number(BigInt(nonce));
      const height = Number(BigInt(head.number));
      assert(height <= highestSeen + 128, 'Unexpected head jump during measured phase');
      const missing = [];
      for (let n = highestSeen + 1; n < height; n++) missing.push(['eth_getBlockByNumber', [`0x${n.toString(16)}`, false]]);
      const headers = [...(missing.length ? await batch(l2, missing) : []), head];
      for (const b of headers) {
        assert(b);
        if (canonicalObserved.has(b.number) && canonicalObserved.get(b.number) !== b.hash) event('unsafe_replacement', { number: b.number, before: canonicalObserved.get(b.number), after: b.hash });
        if (canonicalObserved.get(b.number) !== b.hash) appendFileSync(`${out}/observed-headers.jsonl`, json(b) + '\n', { mode: 0o600 });
        canonicalObserved.set(b.number, b.hash);
        for (const hash of b.transactions) if (!firstSeen.has(hash)) firstSeen.set(hash, Date.now() / 1000);
      }
      highestSeen = Math.max(highestSeen, height);
      const available = memory();
      const row = { at: Date.now() / 1000, elapsed: (performance.now() - began) / 1000,
        head_number: height, head_hash: head.hash, head_timestamp: Number(BigInt(head.timestamp)), safe_number: Number(BigInt(safe.number)), safe_hash: safe.hash,
        latest_nonce: latestNonce, accepted: accepted.length, pending: nextNonce - latestNonce, outstanding_rpc: requests.size, mem_available_bytes: available };
      if (sampled - lastQueue >= 5000) {
        row.queue = await rpc(front, 'eez_getCrossChainQueueStatus'); assertQueue(row.queue); lastQueue = sampled;
      }
      observations.push(row); appendFileSync(`${out}/observations.jsonl`, json(row) + '\n', { mode: 0o600 });
      assert(available >= 5 * 1024 ** 3, 'Shared-host memory safety threshold reached');
      // The supervisor records liveness separately for each protocol revision.
      assert(BigInt(head.baseFeePerGas) + tip <= maxFee, 'Source fee cap insufficient; no automatic price increase');
    } catch (error) { fail(error); }
    await sleep(1000 - (performance.now() - sampled));
  }
})();
try {
  let offset = 0;
  for (let stage = 0; stage < stages.length && !stop; stage++) {
    const { rate, seconds } = stages[stage], count = rate * seconds, spacing = 1000 / rate;
    const stageStart = began + offset * 1000, stageEnd = stageStart + seconds * 1000;
    event('stage_started', { stage, rate, seconds });
    for (let tick = 0; tick < count && !stop; tick++) {
      const scheduled = stageStart + tick * spacing;
      await sleep(scheduled - performance.now());
      offered++;
      if (performance.now() >= stageEnd || performance.now() - scheduled >= spacing) {
        skippedSchedule++; event('scheduler_missed_offer', { stage, tick, lag_ms: performance.now() - scheduled }); continue;
      }
      if (nextNonce - latestNonce >= 128 || requests.size >= maxConcurrent) {
        withheld++; event('backpressure', { stage, tick, pending: nextNonce - latestNonce, outstanding_rpc: requests.size }); continue;
      }
      if (existsSync(`${out}/../stop-producers.json`)) throw new Error('Supervisor requested stop');
      const nonce = nextNonce;
      const raw = await account.signTransaction({ type: 'eip1559', chainId: config.chain_id, to: config.recipient, nonce, gas, maxFeePerGas: maxFee, maxPriorityFeePerGas: tip, value });
      const hash = keccak256(raw), intent = { hash, nonce, stage, scheduled_at: wallStart + (scheduled - began) / 1000, submitted_at: Date.now() / 1000 };
      event('intent', { ...intent, raw }); // Persist before the only submission.
      nextNonce++;
      const request = rpc(l2, 'eth_sendRawTransaction', [raw]).then(actual => {
        assert.equal(actual, hash);
        accepted.push(intent); event('accepted', intent);
        if (accepted.length % 300 === 0) console.log(json({ elapsed: (performance.now() - began) / 1000, stage, accepted: accepted.length, offered, skippedSchedule, withheld, outstanding_rpc: requests.size }));
      }).catch(error => { event('submission_failed', { hash, nonce, error: String(error) }); fail(error); }).finally(() => requests.delete(request));
      requests.add(request);
    }
    if (!stop) await sleep(stageEnd - performance.now());
    offset += seconds;
  }
} catch (error) { fail(error); }
finally {
  stop = true; await Promise.allSettled([...requests]); await observer;
  accepted.sort((a, b) => a.nonce - b.nonce);
  Object.assign(meta, { ended_at: Date.now() / 1000, elapsed_seconds: (performance.now() - began) / 1000, accepted: accepted.length, offered, skipped_schedule: skippedSchedule, withheld_backpressure: withheld, error: fatal });
  save('metadata.json', meta); save('accepted.json', accepted); save('first-seen.json', Object.fromEntries(firstSeen));
}
assert(accepted.every((tx, index) => tx.nonce === firstNonce + index), 'Accepted nonce gap; no replacement or resubmission is authorized');
assert(accepted.length, 'No accepted ordinary transfers');
console.log('Load ended; checking every receipt and canonical safe coverage.');
let receipts, safe;
const deadline = performance.now() + 600_000;
while (true) {
  receipts = [];
  for (let offset = 0; offset < accepted.length; offset += 100) receipts.push(...await batch(l2, accepted.slice(offset, offset + 100).map(t => ['eth_getTransactionReceipt', [t.hash]])));
  safe = await rpc(l2, 'eth_getBlockByNumber', ['safe', false]);
  assert(receipts.filter(Boolean).every(r => r.status === '0x1'));
  if (receipts.every(r => r && BigInt(r.blockNumber) <= BigInt(safe.number))) break;
  save('receipts-at-drain.json', receipts);
  assert(performance.now() < deadline, 'Canonical receipt/settlement drain timeout; no resubmission');
  await sleep(3000);
}
save('receipts.json', receipts);
const heights = [...new Set(receipts.map(r => r.blockNumber))];
const headers = [];
for (let offset = 0; offset < heights.length; offset += 100) headers.push(...await batch(l2, heights.slice(offset, offset + 100).map(n => ['eth_getBlockByNumber', [n, false]])));
const byNumber = new Map(headers.map(b => [b.number, b]));
let cost = 0n;
for (let index = 0; index < accepted.length; index++) {
  const r = receipts[index], tx = accepted[index], b = byNumber.get(r.blockNumber);
  assert.equal(r.transactionHash, tx.hash); assert.equal(r.blockHash, b.hash); assert(b.transactions.includes(tx.hash));
  assert.equal(BigInt(r.gasUsed), gas); assert.equal(r.type, '0x2');
  const expectedPrice = BigInt(b.baseFeePerGas) + tip < maxFee ? BigInt(b.baseFeePerGas) + tip : maxFee;
  assert.equal(BigInt(r.effectiveGasPrice), expectedPrice); cost += gas * expectedPrice;
}
const terminal = headers.reduce((a, b) => BigInt(a.number) > BigInt(b.number) ? a : b);
const afterNonce = BigInt(await rpc(l2, 'eth_getTransactionCount', [account.address, terminal.number]));
const balanceAfter = BigInt(await rpc(l2, 'eth_getBalance', [account.address, terminal.number]));
const recipientAfter = BigInt(await rpc(l2, 'eth_getBalance', [config.recipient, terminal.number]));
assert.equal(afterNonce, BigInt(firstNonce + accepted.length));
assert.equal(balanceBefore - balanceAfter, cost + value * BigInt(accepted.length));
assert.equal(recipientAfter - recipientBefore, value * BigInt(accepted.length));
assert.equal((await rpc(l2, 'eth_getBlockByNumber', [before.number, false])).hash, before.hash);
save('canonical-receipt-blocks.json', headers);
const summary = { verified_receipts_and_accounting: true, runtime_source: config.runtime_source,
  planned, offered, accepted: accepted.length, skipped_schedule: skippedSchedule, withheld_backpressure: withheld,
  elapsed_seconds: meta.elapsed_seconds, error: fatal, gas_cost_wei: cost, value_wei: value * BigInt(accepted.length),
  exact_nonce: true, exact_sender_balance: true, exact_recipient_balance: true, safe, terminal,
  per_stage: stages.map((s, i) => ({ ...s, accepted: accepted.filter(t => t.stage === i).length })),
  authenticated_blob_verification: 'pending separate collector', independent_replay: 'pending separate follower' };
save('summary.json', summary); console.log(json(summary));
assert(!fatal, fatal); assert.equal(offered, planned); assert(meta.elapsed_seconds >= duration);
