/* Shared by the browser and Node tests. No network or DOM access. */
(function (root) {
  "use strict";
  const MAX_IMPORT_BYTES = 8 * 1024 * 1024;
  const statuses = new Set(["success", "revert", "halt", "incomplete"]);
  const quantity = value => typeof value === "string" && /^0x(?:0|[1-9a-f][0-9a-f]{0,63})$/i.test(value);
  const object = value => value !== null && typeof value === "object" && !Array.isArray(value);
  const requireValue = (condition, message) => { if (!condition) throw new Error(message); };
  const string = (value, max = 512) => typeof value === "string" && value.length <= max;
  const address = value => value === null || (typeof value === "string" && /^0x[0-9a-f]{40}$/i.test(value));

  function bytes(value) {
    requireValue(object(value) && typeof value.hex === "string" &&
      /^0x(?:[0-9a-f]{2}){0,4096}$/i.test(value.hex) &&
      Number.isSafeInteger(value.length) && value.length >= 0 &&
      typeof value.truncated === "boolean", "Invalid captured bytes");
    const captured = (value.hex.length - 2) / 2;
    requireValue(value.length >= captured && value.truncated === (value.length > captured),
      "Captured byte length does not match its truncation flag");
  }

  function parse(text) {
    requireValue(typeof text === "string" && text.length <= MAX_IMPORT_BYTES &&
      new TextEncoder().encode(text).length <= MAX_IMPORT_BYTES, "Import exceeds 8 MiB");
    const document = JSON.parse(text);
    const result = document?.result ?? document;
    const trace = result?.diagnostics?.trace ?? (result?.format ? result : null);
    if (!trace && object(document?.error)) {
      const message = typeof document.error.message === "string"
        ? document.error.message.slice(0, 512) : "Request failed";
      throw new Error(`No call trace was returned. RPC error: ${message}`);
    }
    requireValue(trace?.format === "eez-quote-call-trace" && trace.version === 1,
      "No supported call trace found. Request eez_debugQuoteFee with trace: \"calls\".");
    requireValue(object(trace.context) && object(trace.request) &&
      quantity(trace.context.l1ChainId) && quantity(trace.context.l2ChainId),
    "Invalid trace context");
    requireValue(Array.isArray(trace.runs) && trace.runs.length <= 128 &&
      typeof trace.truncated === "boolean", "Invalid trace runs");
    let count = 0;
    for (const [index, run] of trace.runs.entries()) {
      requireValue(object(run) && run.id === index && string(run.phase, 64) &&
        Number.isSafeInteger(run.pass) && run.pass >= 0 &&
        string(run.purpose, 64) && ["0x0", "0x2"].includes(run.type) &&
        quantity(run.chainId) && object(run.transaction) && typeof run.complete === "boolean" &&
        Array.isArray(run.frames), "Invalid execution run");
      count += run.frames.length;
      requireValue(count <= 4096, "Trace exceeds 4096 frames");
      bytes(run.transaction.input);
      for (const [id, frame] of run.frames.entries()) {
        requireValue(object(frame) && frame.id === id &&
          (frame.parentId === null || (Number.isInteger(frame.parentId) && frame.parentId >= 0 && frame.parentId < id)) &&
          quantity(frame.chainId) && statuses.has(frame.status) &&
          string(frame.kind, 32) && typeof frame.crossChain === "boolean" &&
          typeof frame.executionRoot === "boolean" && typeof frame.isStatic === "boolean",
        "Invalid call frame or parent");
        requireValue(address(frame.from) && address(frame.to) && address(frame.codeAddress) &&
          quantity(frame.value) && quantity(frame.gas) &&
          (frame.gasUsed === undefined || quantity(frame.gasUsed)) &&
          (frame.result === undefined || string(frame.result, 64)), "Invalid frame fields");
        requireValue(frame.crossChain === (frame.parentId !== null &&
          frame.chainId !== run.frames[frame.parentId].chainId), "Invalid cross-chain link");
        bytes(frame.input);
        if (frame.status !== "incomplete") bytes(frame.output);
      }
    }
    return { document, result, trace };
  }

  function indexRun(run) {
    const children = run.frames.map(() => []);
    const roots = [], depth = [], unwound = [], incomplete = [], rootId = [];
    for (const frame of run.frames) {
      const { id, parentId } = frame;
      if (parentId === null) {
        roots.push(id); depth[id] = 0; rootId[id] = id;
      } else {
        children[parentId].push(id);
        depth[id] = depth[parentId] + 1;
        rootId[id] = rootId[parentId];
      }
      unwound[id] = ["revert", "halt"].includes(frame.status) ||
        (parentId !== null && unwound[parentId]);
      incomplete[id] = frame.status === "incomplete" ||
        (parentId !== null && incomplete[parentId]);
    }
    return { children, roots, depth, unwound, incomplete, rootId };
  }

  function decodeError(frame) {
    if (!["revert", "halt"].includes(frame.status)) return null;
    if (frame.status === "halt") return frame.result ?? "Execution halted";
    const output = frame.output;
    if (!output || output.truncated) return "Revert data incomplete";
    const data = output.hex.slice(2);
    if (!data) return "Revert without data";
    const selector = data.slice(0, 8).toLowerCase();
    if (selector === "08c379a0" && data.length >= 136) {
      const offset = BigInt("0x" + data.slice(8, 72));
      const length = BigInt("0x" + data.slice(72, 136));
      if (offset === 32n && length <= 4096n && 136n + length * 2n <= BigInt(data.length)) {
        try {
          const hex = data.slice(136, 136 + Number(length) * 2);
          return "Error: " + new TextDecoder("utf-8", { fatal: true }).decode(
            Uint8Array.from(hex.match(/../g) ?? [], byte => parseInt(byte, 16)));
        } catch { /* Preserve malformed bytes without inventing a message. */ }
      }
    }
    if (selector === "4e487b71" && data.length === 72) {
      const code = BigInt("0x" + data.slice(8));
      const meanings = {
        1: "assertion failed", 17: "arithmetic overflow / underflow", 18: "division by zero",
        33: "invalid enum conversion", 34: "invalid storage byte array", 49: "pop on empty array",
        50: "array index out of bounds", 65: "memory allocation overflow", 81: "uninitialized function",
      };
      return `Panic 0x${code.toString(16)}${meanings[code.toString()] ? ": " + meanings[code.toString()] : ""}`;
    }
    return data.length >= 8 ? `Unknown error selector 0x${selector}` : "Unrecognized revert data";
  }

  function chooseRun(model) {
    const runs = model.trace.runs;
    if (!runs.length) return -1;
    const failed = (model.result.options ?? []).filter(option => option.error).at(-1);
    if (failed) {
      const detail = model.result.diagnostics?.options?.find(option => option.type === failed.type);
      const stage = detail?.error?.details?.stage;
      const phase = ["source_execution", "native_replay"].includes(stage) ? "fee_simulation" : stage;
      const typed = runs.filter(run => run.type === failed.type);
      // A terminal effects/validation error can follow successful execution.
      // Do not promote an earlier, intentionally failed gas probe as its cause.
      const terminal = typed.filter(run => run.phase === phase).at(-1) ?? typed.at(-1);
      if (terminal) return terminal.id;
    }
    const composition = runs.filter(run => run.purpose === "composition");
    return (composition.at(-1) ?? runs.at(-1)).id;
  }

  function disposition(frame, index, run) {
    if (index.incomplete[frame.id]) return "Effects unknown: execution incomplete";
    if (["revert", "halt"].includes(frame.status)) {
      return run.frames[index.rootId[frame.id]].status === "success"
        ? "Failure inside a successful simulation"
        : "Failed frame";
    }
    return index.unwound[frame.id] ? "Effects rolled back by an ancestor" : "Succeeded in simulation";
  }

  const api = { MAX_IMPORT_BYTES, parse, indexRun, decodeError, chooseRun, disposition,
    decimal: value => quantity(value) ? BigInt(value).toLocaleString("en-US") : "—" };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.EEZQuoteTrace = api;
})(globalThis);
