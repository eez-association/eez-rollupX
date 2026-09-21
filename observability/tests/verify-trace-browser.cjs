// Owned retention fixture: follow the actual SQL trace link after Tempo expiry.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

async function main() {
  const input = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  fs.mkdirSync(input.output, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1050 } });
  const page = await context.newPage();
  const report = { checks: [], trace_responses: [], page_errors: [] };
  page.on('pageerror', error => report.page_errors.push(error.message));
  context.on('page', opened => opened.on('pageerror', error => report.page_errors.push(error.message)));
  context.on('response', async response => {
    const pathname = new URL(response.url()).pathname;
    if (pathname !== '/api/ds/query') return;
    const request = response.request().postDataJSON();
    if (!request?.queries?.some(query => query.queryType === 'traceId' && query.query === input.expired_trace)) return;
    try {
      const body = await response.json();
      report.trace_responses.push({ path: pathname, status: response.status(), results: body.results });
    } catch (error) { report.page_errors.push(String(error)); }
  });
  let tracePage;
  try {
    await page.goto(`${input.url}/login`, { waitUntil: 'domcontentloaded' });
    await page.getByLabel('Email or username').fill('admin');
    await page.getByLabel('Password', { exact: true }).fill(input.password);
    await page.getByRole('button', { name: 'Log in', exact: true }).click();
    await page.waitForURL(url => !url.pathname.startsWith('/login'));
    const variables = new URLSearchParams({ from: 'now-15m', to: 'now',
      'var-environment': 'devnet', 'var-deployment': input.deployment,
      'var-chain': '$__all', 'var-rollup': '$__all', 'var-instance': 'fixture-node' });
    await page.goto(`${input.url}/d/eez-cross-chain?${variables}`, { waitUntil: 'domcontentloaded' });
    const timeline = page.getByRole('region', { name: 'Request timeline · optional source hash', exact: true });
    await timeline.scrollIntoViewIfNeeded();
    await timeline.getByText('31'.repeat(16), { exact: true }).waitFor({ state: 'visible' });
    report.checks.push('all_numeric_epoch_is_rendered_exactly_without_unit_scaling');
    // The same ID also appears in the timeline, where it has no Tempo link.
    // The pinned PanelChrome renders a named section for each actual panel.
    const tracePanel = page.getByRole('region', { name: 'Retained trace references · optional backend', exact: true });
    await tracePanel.scrollIntoViewIfNeeded();
    const popup = page.waitForEvent('popup', { timeout: 30000 });
    popup.catch(() => {}); // Retain diagnostics if locating the source cell fails first.
    await tracePanel.getByText(input.expired_trace, { exact: true }).click({ position: { x: 5, y: 5 } });
    const menu = page.getByText('Trace (optional; may be unavailable)', { exact: true }).last();
    const openedDirectly = await Promise.race([
      popup.then(() => true), menu.waitFor({ state: 'visible', timeout: 10000 }).then(() => false)
    ]);
    if (!openedDirectly) await menu.click();
    tracePage = await popup;
    await tracePage.waitForURL(url => url.pathname === '/explore');
    const panes = JSON.parse(new URL(tracePage.url()).searchParams.get('panes'));
    assert.equal(panes.trace.datasource, 'eez-tempo');
    assert.equal(panes.trace.queries[0].query, input.expired_trace);
    const unavailable = `failed to get trace with id: ${input.expired_trace} Status: Not Found`;
    await tracePage.getByText(unavailable, { exact: true }).waitFor({ timeout: 30000 });
    assert(report.trace_responses.length > 0, 'browser must issue the exact backend trace-ID query');
    assert(report.trace_responses.some(response => response.results?.A?.error === unavailable &&
      response.results.A.frames.length === 0), 'exact trace must be unavailable, not another query failure');
    report.checks.push('retained_sql_trace_link_opens_exact_expired_trace_and_displays_unavailable');
    await tracePanel.getByText(input.expired_trace, { exact: true }).waitFor({ state: 'visible' });
    report.checks.push('original_durable_sql_reference_remains_visible_after_trace_expiry');
    assert.deepEqual(report.page_errors, []);
    await tracePage.screenshot({ path: path.join(input.output, 'expired-trace.png') });
    report.status = 'passed';
  } catch (error) {
    report.status = 'failed';
    report.error = String(error);
    const active = tracePage || page;
    report.visible_text = (await active.locator('body').innerText().catch(() => '')).slice(0, 10000);
    await active.screenshot({ path: path.join(input.output, 'failure.png') }).catch(() => {});
    throw error;
  } finally {
    fs.writeFileSync(path.join(input.output, 'browser.json'), JSON.stringify(report, null, 2) + '\n');
    await browser.close();
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
