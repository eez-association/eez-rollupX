const { test } = require("node:test");
const assert = require("node:assert/strict");
const api = require("../model.js");
const { fixture, bytes, revert } = require("./fixture.cjs");
const parse = doc => api.parse(JSON.stringify(doc));

test("imports partial traces from operational RPC failures and standalone exports", () => {
  const trace = fixture().result.diagnostics.trace;
  trace.partial = true;
  trace.failure = { code: "poster_insufficient_funds", message: "Posting wallet cannot fund settlement" };
  const doc = { jsonrpc: "2.0", error: { code: -32004, message: trace.failure.message,
    data: { ...trace.failure, trace } } };
  for (const value of [doc, trace]) {
    const model = parse(value);
    assert.equal(model.trace.partial, true);
    assert.equal(model.trace.failure.code, "poster_insufficient_funds");
    assert.equal(api.chooseRun(model), 0);
    assert.equal(model.result.options, undefined);
  }
  trace.failure.message = { unexpected: "object" };
  assert.throws(() => parse(doc), /Invalid trace failure/);
});

test("imports the recorded Composer nested revert without conflating quote and execution failure", () => {
  const model = parse(require("../examples/nested-revert.json"));
  assert.equal(model.result.options[0].error.code, "unsupported_transaction");
  const run = model.trace.runs[api.chooseRun(model)], index = api.indexRun(run);
  assert.equal(run.frames[0].status, "success");
  const remote = run.frames.find(frame => frame.crossChain && frame.status === "revert");
  assert(remote);
  assert.equal(api.disposition(remote, index, run), "Failure inside a successful simulation");
  const callback = run.frames.find(frame => frame.crossChain && frame.status === "success");
  assert(callback);
  assert.equal(api.disposition(callback, index, run), "Effects rolled back by an ancestor");
});

test("accepts RPC envelopes, results and trace exports with exact cross-chain parents", () => {
  const doc = fixture();
  for (const value of [doc, doc.result, doc.result.diagnostics.trace]) {
    const model = parse(value), run = model.trace.runs[0], index = api.indexRun(run);
    assert.deepEqual(index.children, [[1], [2], []]);
    assert.deepEqual(index.depth, [0, 1, 2]);
    assert.deepEqual(index.unwound, [false, true, true]);
    assert.equal(api.disposition(run.frames[1], index, run), "Failure inside a successful simulation");
    assert.equal(api.disposition(run.frames[2], index, run), "Effects rolled back by an ancestor");
  }
});

test("decodes standard errors and panic without guessing unknown custom error ABIs", () => {
  const frame = fixture().result.diagnostics.trace.runs[0].frames[1];
  assert.equal(api.decodeError(frame), "Error: Dependency rejected");
  frame.output = bytes("0x4e487b71" + "11".padStart(64, "0"));
  assert.equal(api.decodeError(frame), "Panic 0x11: arithmetic overflow / underflow");
  frame.output = bytes("0xdeadbeef");
  assert.equal(api.decodeError(frame), "Unknown error selector 0xdeadbeef");
  frame.output = bytes("0x08c379a0" + "ff".repeat(64));
  assert.equal(api.decodeError(frame), "Unknown error selector 0x08c379a0");
  frame.output = bytes("0x");
  assert.equal(api.decodeError(frame), "Revert without data");
  frame.output = { hex: "0x12", length: 100, truncated: true };
  assert.equal(api.decodeError(frame), "Revert data incomplete");
  frame.status = "halt"; frame.result = "OutOfGas";
  assert.equal(api.decodeError(frame), "OutOfGas");
});

test("preserves hostile text as data and keeps 256-bit quantities exact", () => {
  const frame = fixture().result.diagnostics.trace.runs[0].frames[1];
  const text = '<img src=x onerror="globalThis.compromised=true">';
  frame.output = bytes(revert(text));
  assert.equal(api.decodeError(frame), "Error: " + text);
  assert.equal(api.decimal("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff").replaceAll(",", ""),
    ((1n << 256n) - 1n).toString());
});

test("rejects cycles, forward parents, fabricated cross-chain edges and malformed data", () => {
  const mutations = [
    frame => { frame.parentId = 1; }, frame => { frame.parentId = -1; },
    frame => { frame.id = 9; }, frame => { frame.status = "probably fine"; },
    frame => { frame.chainId = "javascript:alert(1)"; }, frame => { frame.crossChain = false; },
    frame => { frame.input.length = 999; }, frame => { frame.output.hex = "0x123"; },
    frame => { frame.gasUsed = "0x00"; }, frame => { frame.to = "<script>"; },
  ];
  for (const mutate of mutations) {
    const doc = fixture(); mutate(doc.result.diagnostics.trace.runs[0].frames[1]);
    assert.throws(() => parse(doc));
  }
});

test("bounds import and rejects missing/unsupported schemas", () => {
  assert.throws(() => api.parse(" ".repeat(api.MAX_IMPORT_BYTES + 1)), /8 MiB/);
  assert.throws(() => api.parse("{"), /JSON/);
  assert.throws(() => parse({ result: { diagnostics: {} } }), /No supported call trace/);
  assert.throws(() => parse({ jsonrpc: "2.0", error: { code: -32000, message: "Quote context expired" } }),
    /No call trace was returned. RPC error: Quote context expired/);
  const doc = fixture(); doc.result.diagnostics.trace.version = 2;
  assert.throws(() => parse(doc), /No supported call trace/);
  doc.result.diagnostics.trace.version = 1;
  doc.result.diagnostics.trace.runs = Array(129).fill(doc.result.diagnostics.trace.runs[0]);
  assert.throws(() => parse(doc), /Invalid trace runs/);
});

test("chooses successful final composition over failed gas probes; terminal failures win", () => {
  const doc = fixture(), trace = doc.result.diagnostics.trace;
  const probe = structuredClone(trace.runs[0]);
  probe.phase = "gas_estimation"; probe.frames[0].status = "revert";
  trace.runs.unshift(probe); trace.runs[1].id = 1;
  assert.equal(api.chooseRun(parse(doc)), 1);
  doc.result.options[0].error = { code: "unsupported_transaction" };
  assert.equal(api.chooseRun(parse(doc)), 1, "an earlier probe is not automatically the cause");
  doc.result.diagnostics.options = [{ type: "0x2", error: { details: { stage: "gas_estimation" } } }];
  assert.equal(api.chooseRun(parse(doc)), 0);
});

test("marks interrupted ancestry unknown and preserves separate delegatecall addresses", () => {
  const doc = fixture(), frames = doc.result.diagnostics.trace.runs[0].frames;
  frames[0].status = "incomplete"; delete frames[0].output;
  frames[2].kind = "DELEGATECALL"; frames[2].codeAddress = frames[1].to;
  const model = parse(doc), run = model.trace.runs[0];
  assert.equal(api.disposition(run.frames[2], api.indexRun(run), run), "Effects unknown: execution incomplete");
  assert.notEqual(run.frames[2].codeAddress, run.frames[2].to);
});

test("indexes deep traces iteratively without recursive traversal", () => {
  const doc = fixture(), run = doc.result.diagnostics.trace.runs[0];
  const prototype = structuredClone(run.frames[0]);
  run.frames = Array.from({ length: 4096 }, (_, id) => ({
    ...prototype, id, parentId: id ? id - 1 : null, crossChain: false,
  }));
  assert.equal(api.indexRun(parse(doc).trace.runs[0]).depth[4095], 4095);
  run.frames.push({ ...prototype, id: 4096, parentId: 4095 });
  assert.throws(() => parse(doc), /4096 frames/);
});

test("explains a parent-only revert without blaming its last successful child", () => {
  const run = fixture().result.diagnostics.trace.runs[0];
  run.frames[0].status = "revert";
  run.frames[0].output = bytes(revert("GPv2: not a contract"));
  for (const frame of run.frames.slice(1)) { frame.status = "success"; frame.output = bytes("0x"); }
  run.frames[2].parentId = 0;
  const summary = api.failureSummary(run);
  assert.equal(summary.focus, 0);
  assert.equal(summary.lastChild, 2);
  assert.deepEqual(summary.failed, [0]);
  assert.match(summary.detail, /No child revert or halt is recorded/);
  assert.match(summary.detail, /2 returned calls have effects rolled back/);
  assert.match(summary.detail, /#2 returned no data/);
  assert.match(summary.detail, /does not establish the cause/);
  run.complete = false;
  assert.match(api.failureSummary(run).title, /incomplete/);
});

test("distinguishes field truncation from omitted calls and runs", () => {
  const trace = fixture().result.diagnostics.trace;
  trace.omittedFrames = 0; trace.omittedRuns = 0; trace.truncated = true;
  trace.runs[0].frames[2].kind = "CREATE2";
  trace.runs[0].frames[2].input = { hex: "0x1234", length: 7, truncated: true };
  let summary = api.captureSummary(trace);
  assert.match(summary.text, /no calls or runs omitted/);
  assert.match(summary.text, /call #2: constructor input shortened \(2 of 7 bytes\)/);
  assert.equal(summary.shortened.length, 1);
  trace.omittedFrames = 5; trace.omittedRuns = 2;
  summary = api.captureSummary(trace);
  assert.match(summary.text, /5 calls and 2 runs omitted/);
  assert.doesNotMatch(summary.text, /no calls or runs omitted/);
  trace.omittedFrames = -1;
  assert.throws(() => parse(trace), /Invalid omission count/);
});
