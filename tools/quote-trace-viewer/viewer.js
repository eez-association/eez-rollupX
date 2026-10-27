/* Imported content and host annotations are text. This frame never fetches. */
(() => {
  "use strict";
  const api = globalThis.EEZQuoteTrace;
  if (new URLSearchParams(location.search).get("embedded") === "1") {
    document.body.classList.add("embedded");
  }
  const $ = id => document.getElementById(id);
  const el = (tag, text, className) => {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  };
  let model, run, index, selected, expanded = new Set();
  let importSequence = 0;
  let host, traceSequence = 0, decoded = new Map(), annotationBytes = 0, annotationLimited = false, redrawPending = false;
  const annotation = frame => decoded.get(frame.id);
  const frameError = frame => annotation(frame)?.error ?? api.decodeError(frame);
  const notifyRun = () => {
    if (host && model) host.postMessage({ type: "run", sequence: traceSequence,
      context: model.trace.context, run: run ?? null });
  };
  const short = address => address ? `${address.slice(0, 10)}…${address.slice(-6)}` : "—";
  const chain = id => id === model.trace.context.l1ChainId ? "L1" :
    id === model.trace.context.l2ChainId ? "L2" : api.decimal(id);
  const error = message => { $("error").textContent = message; $("error").hidden = false; };

  function load(text) {
    traceSequence++;
    decoded.clear();
    try {
      const parsed = api.parse(text);
      model = parsed;
      $("error").hidden = true;
      $("workspace").hidden = false;
      document.body.classList.add("has-trace");
      document.body.classList.remove("import-open");
      $("show-import").hidden = !document.body.classList.contains("embedded");
      $("show-import").setAttribute("aria-expanded", "false");
      $("download").hidden = false;
      const options = Array.isArray(model.result.options) ? model.result.options : [];
      const failures = options.filter(option => option.error);
      const failure = model.trace.failure;
      $("quote-status").textContent = failure ? "Quote failed" : !options.length ? "Outcome not included" :
        failures.length === options.length ? "Quote failed" : failures.length ? "Mixed results" : "Quote available";
      $("quote-detail").textContent = options.map(option => `${option.type === "0x0" ? "Legacy" : "EIP-1559"}: ${option.error?.code ?? "available"}`).join(" · ");
      $("quote-errors").hidden = !failure && failures.length === 0;
      $("quote-diagnostics").hidden = $("quote-errors").hidden;
      $("quote-errors").textContent = failure ? `${failure.code}: ${failure.message}` : failures.map(option => {
        const detail = model.result.diagnostics?.options?.find(item => item.type === option.type)?.error?.details;
        return `${option.type}: ${detail?.stage ? detail.stage + ": " : ""}${detail?.message ?? option.error.message ?? option.error.code}`;
      }).join("\n");
      $("networks").textContent = `L1 ${api.decimal(model.trace.context.l1ChainId)} ↔ L2 ${api.decimal(model.trace.context.l2ChainId)}`;
      const frames = model.trace.runs.reduce((sum, item) => sum + item.frames.length, 0);
      const count = model.trace.runs.length;
      $("capture").textContent = `${frames} call${frames === 1 ? "" : "s"} · ${count} run${count === 1 ? "" : "s"}`;
      const capture = api.captureSummary(model.trace);
      $("capture-detail").textContent = model.trace.partial ? "Quote interrupted" : model.trace.truncated
        ? model.trace.omittedFrames === 0 && model.trace.omittedRuns === 0 && capture.shortened.length
          ? `${capture.shortened.length} field${capture.shortened.length === 1 ? "" : "s"} shortened · call tree captured` : "Partial capture"
        : "Within capture limits";
      $("warning").hidden = !model.trace.truncated && !model.trace.partial;
      $("warning").textContent = [
        model.trace.partial ? "The quote failed. These calls show the work completed before the failure; they do not establish a valid fee quote." : "",
        model.trace.truncated ? capture.text : "",
      ].filter(Boolean).join(" ");
      $("context").textContent = JSON.stringify({ context: model.trace.context, request: model.trace.request }, null, 2);
      $("runs").replaceChildren();
      for (const item of model.trace.runs) {
        const outcome = item.frames.find(frame => frame.parentId === null)?.status ?? "no frames";
        const option = el("option", `#${item.id + 1} · ${item.type === "0x0" ? "Legacy" : "1559"} · ${item.phase} ${item.pass} / ${item.purpose} · ${outcome}`);
        option.value = String(item.id);
        $("runs").append(option);
      }
      const chosen = api.chooseRun(model);
      $("runs").value = String(chosen);
      selectRun(chosen);
    } catch (failure) {
      model = null;
      run = null;
      document.body.classList.remove("has-trace");
      $("show-import").hidden = true;
      $("workspace").hidden = true;
      $("download").hidden = true;
      error(failure.message);
      host?.postMessage({ type: "invalid", sequence: traceSequence, message: failure.message });
    }
  }

  function selectRun(id) {
    traceSequence++;
    decoded.clear();
    annotationBytes = 0; annotationLimited = false;
    run = model.trace.runs[id];
    $("decoding-status").hidden = !host;
    $("decoding-status").textContent = "Looking up verified contract ABIs…";
    $("failure-summary").hidden = !run;
    $("frame-detail").replaceChildren(el("p", "No execution frames were captured.", "muted"));
    $("frame-id").textContent = "";
    $("tree").replaceChildren();
    $("search").value = "";
    $("execution").textContent = JSON.stringify(run?.transaction ?? {}, null, 2);
    if (!run) { $("quote-diagnostics").open = true; $("run-info").textContent = "Execution did not reach a traceable run."; notifyRun(); return; }
    index = api.indexRun(run);
    expanded = new Set(run.frames.filter(frame => index.depth[frame.id] < 4 || frame.crossChain).map(frame => frame.id));
    const failedRoot = run.frames.find(frame => frame.parentId === null &&
      ["revert", "halt"].includes(frame.status));
    $("quote-diagnostics").open = !failedRoot || Boolean(model.trace.partial);
    selected = failedRoot?.id ?? run.frames.find(frame => ["revert", "halt"].includes(frame.status) &&
      !index.children[frame.id].some(id => ["revert", "halt"].includes(run.frames[id].status)))?.id ?? 0;
    let parent = run.frames[selected]?.parentId;
    while (parent !== null && parent !== undefined) { expanded.add(parent); parent = run.frames[parent].parentId; }
    $("run-info").textContent = `${run.phase} ${run.pass} · ${run.purpose} · transaction gas limit ${api.decimal(run.transaction.gas)}. ${
      run.complete ? "Execution hooks completed." : "Execution interrupted or no frame was reached."
    } ${run.purpose === "conditional_setup" || run.purpose === "settlement" ? "Proof execution is assumed or metered for estimation; this is not proof verification." : ""}`;
    drawTree(); drawDetail(); drawSummary(); notifyRun();
  }

  function focusFrame(id) {
    selected = id;
    let parent = run.frames[id]?.parentId;
    while (parent !== null && parent !== undefined) { expanded.add(parent); parent = run.frames[parent].parentId; }
    $("search").value = "";
    drawTree(); drawDetail();
    $("tree").querySelector(`[data-frame="${id}"]`)?.scrollIntoView({ block: "nearest" });
  }

  function drawSummary() {
    if (!run) return;
    const summary = api.failureSummary(run), box = $("failure-summary");
    box.classList.toggle("error", summary.failed.length > 0);
    box.replaceChildren(el("strong", summary.title), el("p", summary.detail));
    const links = el("div", undefined, "summary-actions");
    for (const id of summary.failed.slice(0, 6)) {
      const frame = run.frames[id];
      const message = (frameError(frame) ?? frame.status).replace(/\s+/g, " ");
      const button = el("button", `#${id} · ${annotation(frame)?.method ?? frame.kind} · ${message.length > 160 ? message.slice(0, 160) + "…" : message}`);
      button.addEventListener("click", () => focusFrame(id)); links.append(button);
    }
    if (summary.lastChild !== undefined) {
      const button = el("button", `Inspect last child #${summary.lastChild}`);
      button.addEventListener("click", () => focusFrame(summary.lastChild)); links.append(button);
    }
    box.append(links);
  }

  function drawTree() {
    if (!run) return;
    const search = $("search").value.toLowerCase();
    const include = run.frames.map(frame => !search ||
      [frame.to, frame.from, frame.codeAddress, frame.kind, frame.input.hex, frameError(frame),
        annotation(frame)?.name, annotation(frame)?.method, annotation(frame)?.arguments].join(" ").toLowerCase().includes(search));
    for (let i = include.length - 1; i >= 0; i--) {
      if (include[i] && run.frames[i].parentId !== null) include[run.frames[i].parentId] = true;
    }
    const visible = [], fragment = document.createDocumentFragment();
    for (const frame of run.frames) {
      visible[frame.id] = include[frame.id] && (frame.parentId === null ||
        (visible[frame.parentId] && (search || expanded.has(frame.parentId))));
      if (!visible[frame.id]) continue;
      const hasChildren = index.children[frame.id].length > 0;
      const button = el("button", undefined, "call");
      button.setAttribute("role", "treeitem");
      button.setAttribute("aria-level", String(index.depth[frame.id] + 1));
      button.setAttribute("aria-selected", String(selected === frame.id));
      if (hasChildren) button.setAttribute("aria-expanded", String(Boolean(search || expanded.has(frame.id))));
      // Numeric indentation comes only from the validated parent graph.
      button.style.paddingLeft = `${6 + Math.min(index.depth[frame.id], 18) * 12}px`;
      button.dataset.frame = String(frame.id);
      const arrow = el("span", hasChildren ? (expanded.has(frame.id) || search ? "▾" : "▸") : "·", "arrow");
      const badge = el("span", chain(frame.chainId), "chain" + (chain(frame.chainId) === "L2" ? " l2" : ""));
      const description = el("span", undefined, "description");
      const info = annotation(frame);
      description.append(el("span", `#${frame.id} ${frame.crossChain ? "↳ " : ""}${frame.kind} ${info?.name ?? short(frame.to)}${info?.method ? "." + info.method : ""}`, "title"));
      description.append(el("span", frameError(frame) ??
        `${frame.input.hex.slice(0, 10)} · ${api.decimal(frame.gasUsed)} gas${index.unwound[frame.id] ? " · rolled back" : ""}`, "sub"));
      const rolledBack = frame.status === "success" && index.unwound[frame.id];
      button.append(arrow, badge, description, el("span", rolledBack ? "returned · rolled back" : frame.status,
        "state " + (rolledBack ? "rolled-back" : frame.status)));
      button.addEventListener("click", event => {
        if (hasChildren && event.target === arrow) {
          if (expanded.has(frame.id)) expanded.delete(frame.id); else expanded.add(frame.id);
        }
        selected = frame.id; drawTree(); drawDetail();
      });
      button.addEventListener("keydown", event => {
        if (!hasChildren || !["ArrowLeft", "ArrowRight"].includes(event.key)) return;
        event.preventDefault();
        if (event.key === "ArrowRight") expanded.add(frame.id); else expanded.delete(frame.id);
        drawTree(); $("tree").querySelector(`[data-frame="${frame.id}"]`)?.focus();
      });
      fragment.append(button);
    }
    $("tree").replaceChildren(fragment);
  }

  function drawDetail() {
    const frame = run?.frames[selected];
    if (!frame) return;
    const container = $("frame-detail"), list = el("dl");
    $("frame-id").textContent = `#${frame.id}`;
    container.replaceChildren(
      el("strong", `${frame.kind} · ${chain(frame.chainId)}`),
      el("p", api.disposition(frame, index, run), index.incomplete[frame.id] ? "muted" : index.unwound[frame.id] ? "bad" : "good"));
    const reason = frameError(frame);
    if (reason) container.append(el("p", reason, "notice error"));
    const info = annotation(frame);
    if (info) {
      container.append(el("h2", info.name ?? "Contract decoding"), el("p", info.note, "muted"));
      for (const [label, value] of [["Function", info.signature], ["Arguments", info.arguments],
        ["Return values", info.returns]]) {
        if (value) container.append(el("h2", label), el("pre", value));
      }
    }
    for (const [name, value] of [
      ["Sender (msg.sender)", frame.from], ["Target / storage", frame.to], ["Code address", frame.codeAddress],
      ["Chain ID", api.decimal(frame.chainId)], ["Value (wei)", api.decimal(frame.value)],
      ["Gas forwarded", api.decimal(frame.gas)], ["Gas used (inclusive)", api.decimal(frame.gasUsed)],
      ["Static", frame.isStatic ? "Yes" : "No"], ["EVM result", frame.result ?? "Not observed"],
      ["Cross-chain entry", frame.crossChain ? "Yes — linked to suspended caller" : "No"],
    ]) list.append(el("dt", name), el("dd", value ?? "—"));
    container.append(list);
    for (const [label, data] of [["Input / initcode", frame.input], ["Output / revert data", frame.output]]) {
      const heading = el("div", undefined, "data-label");
      heading.append(el("h2", label), el("span", data ? `${data.length} bytes${data.truncated ? " · truncated" : ""}` : "Not observed"));
      container.append(heading, el("pre", data?.hex ?? "Execution did not return an outcome."));
    }
  }

  async function file(file) {
    if (!file) return;
    const sequence = ++importSequence;
    traceSequence++;
    host?.postMessage({ type: "import-started", sequence: traceSequence });
    $("workspace").hidden = true;
    $("download").hidden = true;
    if (file.size > api.MAX_IMPORT_BYTES) { error("Import exceeds 8 MiB"); return; }
    try {
      const text = await file.text();
      // A newer file selection or paste owns the displayed result.
      if (sequence === importSequence) load(text);
    } catch (failure) {
      if (sequence === importSequence) error(failure.message);
    }
  }
  $("file").addEventListener("change", event => file(event.target.files[0]));
  $("load").addEventListener("click", () => { importSequence++; load($("json").value); });
  $("show-import").addEventListener("click", () => {
    const open = document.body.classList.toggle("import-open");
    $("show-import").setAttribute("aria-expanded", String(open));
  });
  $("runs").addEventListener("change", () => selectRun(Number($("runs").value)));
  $("search").addEventListener("input", drawTree);
  $("expand").addEventListener("click", () => { if (run) { expanded = new Set(run.frames.map(frame => frame.id)); drawTree(); } });
  $("collapse").addEventListener("click", () => { expanded.clear(); drawTree(); });
  $("download").addEventListener("click", () => {
    const url = URL.createObjectURL(new Blob([JSON.stringify(model.document, null, 2)], { type: "application/json" }));
    const anchor = el("a"); anchor.href = url; anchor.download = "eez-quote-trace.json"; anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  const drop = document.querySelector(".import");
  drop.addEventListener("dragover", event => { event.preventDefault(); drop.classList.add("drag"); });
  drop.addEventListener("dragleave", () => drop.classList.remove("drag"));
  drop.addEventListener("drop", event => { event.preventDefault(); drop.classList.remove("drag"); file(event.dataTransfer.files[0]); });

  // The opaque iframe receives a private port only from its same-site parent.
  // No arbitrary RPC/URL commands cross this channel. The parent supplies text
  // annotations; they never change the captured frame status or exported JSON.
  window.addEventListener("message", event => {
    if (host || parent === window || !document.body.classList.contains("embedded") ||
      event.source !== parent || event.origin !== new URL(location.href).origin ||
      event.data?.type !== "eez-quote-connect" || event.data?.version !== 1 || event.ports.length !== 1) return;
    host = event.ports[0];
    host.onmessage = ({ data }) => {
      if (data?.type === "load" && typeof data.text === "string" &&
        (data.expectedSequence === undefined || data.expectedSequence === traceSequence)) {
        importSequence++; load(data.text);
      } else if (data?.type === "annotations" && model && run && data.sequence === traceSequence && data.runId === run.id) {
        if (!Array.isArray(data.frames) || data.frames.length > 4096) return;
        const allowed = ["name", "method", "signature", "arguments", "returns", "error", "note"];
        const incoming = [], ids = new Set();
        let size = 0;
        for (const item of data.frames) {
          if (!item || !Number.isInteger(item.id) || !run.frames[item.id] || ids.has(item.id)) return;
          ids.add(item.id);
          const clean = {};
          for (const key of allowed) {
            if (item[key] !== undefined) {
              if (typeof item[key] !== "string" || item[key].length > 32768) return;
              size += item[key].length;
              if (size > 1024 * 1024) return;
              clean[key] = item[key];
            }
          }
          incoming.push([item.id, clean]);
        }
        const replaced = incoming.reduce((sum, [id]) => sum + Object.values(decoded.get(id) ?? {}).reduce((n, value) => n + value.length, 0), 0);
        if (annotationBytes - replaced + size > 4 * 1024 * 1024) {
          annotationLimited = true;
          $("decoding-status").textContent = "Decoded display limit reached; inspect remaining calls as raw bytes.";
          return;
        }
        annotationBytes += size - replaced;
        for (const [id, item] of incoming) decoded.set(id, item);
        // Many frames can share one ABI. Coalesce their annotations into one
        // paint instead of rebuilding a 4096-row tree once per decoded call.
        if (!redrawPending) {
          redrawPending = true;
          requestAnimationFrame(() => { redrawPending = false; if (run && model) { drawTree(); drawDetail(); drawSummary(); } });
        }
      } else if (data?.type === "decoding-status" && data.sequence === traceSequence && run?.id === data.runId &&
        typeof data.text === "string" && data.text.length <= 512) {
        $("decoding-status").hidden = false;
        $("decoding-status").textContent = data.text + (annotationLimited ? " Decoded display limit reached; remaining raw bytes are retained." : "");
      }
    };
    host.postMessage({ type: "ready", version: 1, sequence: traceSequence });
    notifyRun();
  });
})();
