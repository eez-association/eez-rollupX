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
    const trace = result?.diagnostics?.trace ?? document?.error?.data?.trace ?? (result?.format ? result : null);
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
    for (const key of ["omittedFrames", "omittedRuns"]) {
      requireValue(trace[key] === undefined || (Number.isSafeInteger(trace[key]) && trace[key] >= 0),
        "Invalid omission count");
    }
    requireValue(trace.partial === undefined || typeof trace.partial === "boolean", "Invalid partial trace flag");
    if (trace.failure !== undefined) {
      requireValue(object(trace.failure) && string(trace.failure.code, 64) &&
        string(trace.failure.message, 512), "Invalid trace failure");
    }
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
        if (frame.status !== "incomplete" || frame.output !== undefined) bytes(frame.output);
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
    if (model.trace.partial) return (runs.findLast(run => run.frames.length) ?? runs.at(-1)).id;
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

  function captureSummary(trace) {
    const shortened = [];
    let frames = 0;
    const record = (data, runId, frameId, field) => {
      if (data?.truncated) shortened.push({ runId, frameId, field,
        captured: (data.hex.length - 2) / 2, length: data.length });
    };
    for (const run of trace.runs) {
      frames += run.frames.length;
      record(run.transaction.input, run.id, null, "transaction input");
      for (const frame of run.frames) {
        record(frame.input, run.id, frame.id, frame.kind.startsWith("CREATE") ? "constructor input" : "input");
        record(frame.output, run.id, frame.id, "output");
      }
    }
    const counts = trace.omittedFrames === 0 && trace.omittedRuns === 0
      ? `All ${frames} recorded calls captured; no calls or runs omitted.`
      : `${frames} calls captured; ${trace.omittedFrames ?? "unknown"} calls and ${trace.omittedRuns ?? "unknown"} runs omitted.`;
    const details = shortened.slice(0, 6).map(item =>
      `Run #${item.runId + 1}${item.frameId === null ? "" : `, call #${item.frameId}`}: ${item.field} shortened (${item.captured} of ${item.length} bytes).`);
    if (shortened.length > 6) details.push(`${shortened.length - 6} more shortened fields.`);
    if (trace.truncated) details.push(trace.omittedFrames === 0 && trace.omittedRuns === 0 && shortened.length
      ? "Shortened bytes cannot be reconstructed." : "Missing bytes or calls cannot be reconstructed.");
    return { shortened, text: [counts, ...details].join(" ") };
  }

  function failureSummary(run) {
    const index = indexRun(run);
    const failed = run.frames.filter(frame => ["revert", "halt"].includes(frame.status));
    const roots = index.roots.map(id => run.frames[id]);
    const failedRoots = roots.filter(frame => ["revert", "halt"].includes(frame.status));
    const incomplete = !run.complete || run.frames.some(frame => frame.status === "incomplete");
    const rolledBack = run.frames.filter(frame => frame.status === "success" && index.unwound[frame.id]).length;
    // Entry IDs order calls by entry, not by return. Only a direct child of a
    // completed failed frame can be described as its last recorded child call.
    const focus = failedRoots[0] ?? failed[0];
    const lastChild = focus && run.frames[index.children[focus.id].at(-1)];
    const onlyRoots = failed.length > 0 && failed.length === failedRoots.length;
    const title = incomplete ? "Execution evidence is incomplete" : failedRoots.length
      ? `${failedRoots.slice(0, 6).map(frame => `#${frame.id}`).join(", ")}${failedRoots.length > 6 ? " and more" : ""} failed in this run`
      : failed.length ? "Nested failure inside a successful simulation" : "No recorded EVM failure in this run";
    const detail = [
      onlyRoots ? "No child revert or halt is recorded. A parent can fail after a child call returns; internal Solidity checks are not separate calls." : "",
      !failed.length ? "Quote-level validation or operational errors are reported above the call tree." : "",
      rolledBack ? `${rolledBack} returned call${rolledBack === 1 ? " has" : "s have"} effects rolled back by a failed ancestor.` : "",
      lastChild ? `Last recorded child of #${focus.id}: #${lastChild.id} ${lastChild.status === "success"
        ? lastChild.output && !lastChild.output.truncated && lastChild.output.length === 0 ? "returned no data" : "returned"
        : lastChild.status}. This does not establish the cause.` : "",
    ].filter(Boolean).join(" ");
    return { title, detail, failed: failed.map(frame => frame.id), focus: focus?.id, lastChild: lastChild?.id };
  }

  const api = { MAX_IMPORT_BYTES, parse, indexRun, decodeError, chooseRun, disposition, captureSummary, failureSummary,
    decimal: value => quantity(value) ? BigInt(value).toLocaleString("en-US") : "—" };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.EEZQuoteTrace = api;
})(globalThis);
