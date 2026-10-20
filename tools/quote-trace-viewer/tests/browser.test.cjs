const { test } = require("node:test");
const assert = require("node:assert/strict");
const { chromium } = require("playwright");
const { pathToFileURL } = require("node:url");
const path = require("node:path");
const fs = require("node:fs/promises");
const { fixture, bytes, revert } = require("./fixture.cjs");

test("imports local JSON, navigates nested failures and exports without network access", async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const errors = [], requests = [];
    page.on("pageerror", err => errors.push(err.message));
    page.on("request", request => { if (!request.url().startsWith("file:")) requests.push(request.url()); });
    await page.goto(pathToFileURL(path.join(__dirname, "../index.html")).href);
    const doc = fixture(), frame = doc.result.diagnostics.trace.runs[0].frames[1];
    frame.output = bytes(revert('<img src="https://example.invalid" onerror="globalThis.compromised=true">'));
    await page.locator("#file").setInputFiles({ name: "trace.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(doc)) });
    await page.locator("#workspace").waitFor({ state: "visible" });
    assert.equal(await page.locator("#quote-status").textContent(), "Quote available");
    assert.match(await page.locator("#frame-detail").textContent(), /Failure inside a successful simulation/);
    assert.match(await page.locator("#frame-detail").textContent(), /<img src=/);
    assert.equal(await page.locator("#frame-detail img").count(), 0);
    assert.equal(await page.evaluate(() => globalThis.compromised), undefined);
    await page.locator("#expand").click();
    await page.locator('[data-frame="2"]').click();
    assert.match(await page.locator("#frame-detail").textContent(), /rolled back by an ancestor/);
    await page.locator("#collapse").click();
    assert.equal(await page.locator('[role="treeitem"]').count(), 1);
    await page.locator("#search").fill("Dependency");
    await page.locator("#search").fill(frame.to);
    assert.equal(await page.locator('[role="treeitem"]').count(), 2);
    const downloaded = page.waitForEvent("download");
    await page.locator("#download").click();
    assert.equal((await downloaded).suggestedFilename(), "eez-quote-trace.json");

    doc.result.diagnostics.trace.truncated = true;
    await page.locator("#file").setInputFiles({ name: "partial.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(doc)) });
    await page.locator("#warning").waitFor({ state: "visible" });
    assert.equal(await page.locator("#warning").isVisible(), true);
    await page.locator("#file").setInputFiles({ name: "invalid.json", mimeType: "application/json", buffer: Buffer.from('{"result":{}}') });
    await page.locator("#error").waitFor({ state: "visible" });
    assert.equal(await page.locator("#workspace").isVisible(), false);
    assert.match(await page.locator("#error").textContent(), /No supported call trace/);

    await page.locator("#file").setInputFiles(path.join(__dirname, "../examples/nested-revert.json"));
    await page.locator("#workspace").waitFor({ state: "visible" });
    assert.equal(await page.locator("#quote-status").textContent(), "Quote failed");
    assert.match(await page.locator("#quote-errors").textContent(), /descendants are not supported/);
    await page.locator('[data-frame="0"]').click();
    assert.match(await page.locator("#frame-detail").textContent(), /Succeeded in simulation/);

    const partial = fixture().result.diagnostics.trace;
    partial.partial = true;
    partial.failure = { code: "poster_insufficient_funds", message: "Posting wallet cannot fund settlement" };
    const failed = { jsonrpc: "2.0", error: { code: -32004, message: partial.failure.message,
      data: { ...partial.failure, trace: partial } } };
    await page.locator("#file").setInputFiles({ name: "funding-failure.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(failed)) });
    await page.waitForFunction(() => document.getElementById("quote-errors").textContent.includes("poster_insufficient_funds"));
    assert.equal(await page.locator("#quote-status").textContent(), "Quote failed");
    assert.match(await page.locator("#warning").textContent(), /do not establish a valid fee quote/);
    assert((await page.locator('[role="treeitem"]').count()) > 0);

    // When supplied, validate real process-test exports with the same UI.
    const directory = process.env.EEZ_TEST_LOG_DIR && path.join(process.env.EEZ_TEST_LOG_DIR, "quote-traces");
    if (directory) {
      const names = (await fs.readdir(directory)).filter(name => name.endsWith(".json"));
      assert(names.length > 0, "expected actual quote process-test exports");
      for (const name of names) {
        // Wait for this import, rather than the previous file's visible tree.
        await page.reload();
        await page.locator("#file").setInputFiles(path.join(directory, name));
        await page.locator("#workspace").waitFor({ state: "visible" });
        assert.equal(await page.locator("#error").isVisible(), false, name);
        assert((await page.locator('[role="treeitem"]').count()) > 0, name);
      }
      await fs.mkdir(path.join(__dirname, "../test-results"), { recursive: true });
      await page.screenshot({ path: path.join(__dirname, "../test-results/viewer.png"), fullPage: true });
    }
    assert.deepEqual(errors, []);
    assert.deepEqual(requests, []);
  } finally { await browser.close(); }
});

test("embedded viewer supports keyboard import and ignores superseded file reads", async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    const url = pathToFileURL(path.join(__dirname, "../index.html"));
    url.search = "?embedded=1";
    await page.goto(url.href);
    assert.equal(await page.locator("body > header").isVisible(), false);
    await page.locator("#file").focus();
    const chooser = page.waitForEvent("filechooser");
    await page.keyboard.press("Enter");
    await (await chooser).setFiles(path.join(__dirname, "../examples/nested-revert.json"));
    await page.locator("#workspace").waitFor({ state: "visible" });
    assert.equal(await page.locator("#quote-status").textContent(), "Quote failed");

    await page.evaluate(() => {
      const original = File.prototype.text;
      globalThis.pendingReads = [];
      File.prototype.text = function () {
        if (!this.name.startsWith("delayed")) return original.call(this);
        return new Promise((resolve, reject) => globalThis.pendingReads.push({ resolve, reject }));
      };
    });
    const delayed = name => page.locator("#file").setInputFiles({
      name, mimeType: "application/json", buffer: Buffer.from("{}"),
    });
    await delayed("delayed-first.json");
    await page.waitForFunction(() => globalThis.pendingReads.length === 1);
    const latest = fixture();
    latest.result.diagnostics.trace.truncated = true;
    await page.locator("#file").setInputFiles({
      name: "latest.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(latest)),
    });
    await page.locator("#warning").waitFor({ state: "visible" });
    await page.evaluate(text => globalThis.pendingReads[0].resolve(text), JSON.stringify(fixture()));
    assert.equal(await page.locator("#warning").isVisible(), true, "an older file must not replace a newer import");

    await delayed("delayed-second.json");
    await page.waitForFunction(() => globalThis.pendingReads.length === 2);
    await page.getByText("Paste JSON instead", { exact: true }).click();
    await page.locator("#json").fill(JSON.stringify(fixture()));
    await page.locator("#load").click();
    await page.evaluate(() => globalThis.pendingReads[1].reject(new Error("old read failed")));
    assert.equal(await page.locator("#workspace").isVisible(), true);
    assert.equal(await page.locator("#warning").isVisible(), false);
    assert.equal(await page.locator("#error").isVisible(), false, "a superseded read error must not replace pasted input");
  } finally { await browser.close(); }
});
