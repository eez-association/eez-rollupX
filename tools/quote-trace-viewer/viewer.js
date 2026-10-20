/* All imported content is rendered as text. Files never leave this page. */
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
  const short = address => address ? `${address.slice(0, 10)}…${address.slice(-6)}` : "—";
  const chain = id => id === model.trace.context.l1ChainId ? "L1" :
    id === model.trace.context.l2ChainId ? "L2" : api.decimal(id);
  const error = message => { $("error").textContent = message; $("error").hidden = false; };

  function load(text) {
    try {
      const parsed = api.parse(text);
      model = parsed;
      $("error").hidden = true;
      $("workspace").hidden = false;
      $("download").hidden = false;
      const options = Array.isArray(model.result.options) ? model.result.options : [];
      const failures = options.filter(option => option.error);
      $("quote-status").textContent = !options.length ? "Outcome not included" :
        failures.length === options.length ? "Quote failed" : failures.length ? "Mixed results" : "Quote available";
      $("quote-detail").textContent = options.map(option => `${option.type === "0x0" ? "Legacy" : "EIP-1559"}: ${option.error?.code ?? "available"}`).join(" · ");
      $("quote-errors").hidden = failures.length === 0;
      $("quote-errors").textContent = failures.map(option => {
        const detail = model.result.diagnostics?.options?.find(item => item.type === option.type)?.error?.details;
        return `${option.type}: ${detail?.stage ? detail.stage + ": " : ""}${detail?.message ?? option.error.message ?? option.error.code}`;
      }).join("\n");
      $("networks").textContent = `L1 ${api.decimal(model.trace.context.l1ChainId)} ↔ L2 ${api.decimal(model.trace.context.l2ChainId)}`;
      const frames = model.trace.runs.reduce((sum, item) => sum + item.frames.length, 0);
      const count = model.trace.runs.length;
      $("capture").textContent = `${frames} call${frames === 1 ? "" : "s"} · ${count} run${count === 1 ? "" : "s"}`;
      $("capture-detail").textContent = model.trace.truncated ? "Partial capture" : "Within capture limits";
      $("warning").hidden = !model.trace.truncated;
      $("warning").textContent = "This trace is truncated. Missing calls or bytes cannot be reconstructed. Inspect the available evidence without assuming the omitted execution succeeded.";
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
      $("workspace").hidden = true;
      $("download").hidden = true;
      error(failure.message);
    }
  }

  function selectRun(id) {
    run = model.trace.runs[id];
    $("frame-detail").replaceChildren(el("p", "No execution frames were captured.", "muted"));
    $("frame-id").textContent = "";
    $("tree").replaceChildren();
    $("search").value = "";
    $("execution").textContent = JSON.stringify(run?.transaction ?? {}, null, 2);
    if (!run) { $("run-info").textContent = "Execution did not reach a traceable run."; return; }
    index = api.indexRun(run);
    expanded = new Set(run.frames.filter(frame => index.depth[frame.id] < 4 || frame.crossChain).map(frame => frame.id));
    const failedRoot = run.frames.find(frame => frame.parentId === null &&
      ["revert", "halt"].includes(frame.status));
    selected = failedRoot?.id ?? run.frames.find(frame => ["revert", "halt"].includes(frame.status) &&
      !index.children[frame.id].some(id => ["revert", "halt"].includes(run.frames[id].status)))?.id ?? 0;
    let parent = run.frames[selected]?.parentId;
    while (parent !== null && parent !== undefined) { expanded.add(parent); parent = run.frames[parent].parentId; }
    $("run-info").textContent = `${run.phase} ${run.pass} · ${run.purpose} · transaction gas limit ${api.decimal(run.transaction.gas)}. ${
      run.complete ? "Execution hooks completed." : "Execution interrupted or no frame was reached."
    } ${run.purpose === "conditional_setup" || run.purpose === "settlement" ? "Proof execution is assumed or metered for estimation; this is not proof verification." : ""}`;
    drawTree(); drawDetail();
  }

  function drawTree() {
    if (!run) return;
    const search = $("search").value.toLowerCase();
    const include = run.frames.map(frame => !search ||
      [frame.to, frame.from, frame.codeAddress, frame.kind, frame.input.hex, api.decodeError(frame)].join(" ").toLowerCase().includes(search));
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
      description.append(el("span", `${frame.crossChain ? "↳ " : ""}${frame.kind} ${short(frame.to)}`, "title"));
      description.append(el("span", api.decodeError(frame) ??
        `${frame.input.hex.slice(0, 10)} · ${api.decimal(frame.gasUsed)} gas${index.unwound[frame.id] ? " · rolled back" : ""}`, "sub"));
      button.append(arrow, badge, description, el("span", frame.status, "state " + frame.status));
      button.addEventListener("click", event => {
        if (hasChildren && (event.target === arrow || selected === frame.id)) {
          if (expanded.has(frame.id)) expanded.delete(frame.id); else expanded.add(frame.id);
        }
        selected = frame.id; drawTree(); drawDetail();
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
      el("p", api.disposition(frame, index, run), index.unwound[frame.id] ? "bad" : "good"));
    const decoded = api.decodeError(frame);
    if (decoded) container.append(el("p", decoded, "notice error"));
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
})();
