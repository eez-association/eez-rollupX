// Uses the repository's existing Playwright/Chromium verification convention.
// Input is a private file for an owned synthetic Grafana fixture, never live config.
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
  const report = { scope: 'actual browser with synthetic SQL/metric producers', checks: [], page_errors: [], sql_queries: [] };
  page.on('pageerror', error => report.page_errors.push(error.message));
  const queryFailures = [];
  const pendingQueries = new Set();
  const pendingRequests = new Set();
  let lastQueryActivity = Date.now();
  let completedQueries = 0;
  page.on('request', request => {
    pendingRequests.add(request);
    if (new URL(request.url()).pathname === '/api/ds/query') {
      pendingQueries.add(request);
      lastQueryActivity = Date.now();
    }
  });
  page.on('requestfinished', request => pendingRequests.delete(request));
  page.on('requestfailed', request => {
    pendingRequests.delete(request);
    if (pendingQueries.delete(request)) {
      queryFailures.push({ error: request.failure()?.errorText || 'Datasource request failed' });
      lastQueryActivity = Date.now();
    }
  });
  // Grafana maintains background/live traffic. Completion means the visible
  // panels' datasource requests have finished, including response validation;
  // an idle browser network is neither necessary nor a data assertion.
  async function settleQueries(since) {
    const deadline = Date.now() + 30000;
    while (Date.now() < deadline) {
      if (completedQueries > since && pendingQueries.size === 0 && Date.now() - lastQueryActivity >= 500) {
        assert.deepEqual(queryFailures, []);
        return;
      }
      await page.waitForTimeout(50);
    }
    throw new Error(`Datasource queries did not settle: completed=${completedQueries - since}, pending=${pendingQueries.size}`);
  }
  page.on('response', async response => {
    if (new URL(response.url()).pathname !== '/api/ds/query') return;
    try {
      const body = await response.json();
      for (const [ref, result] of Object.entries(body.results || {})) {
        if (result.error) queryFailures.push({ ref, error: result.error });
        for (const frame of result.frames || []) {
          const sql = frame.schema?.meta?.executedQueryString;
          if (sql && report.sql_queries.length < 250) report.sql_queries.push({
            sql, rows: frame.data?.values?.[0]?.length || 0 });
        }
      }
    } catch (error) { queryFailures.push({ error: String(error) }); }
    finally {
      pendingQueries.delete(response.request());
      completedQueries++;
      lastQueryActivity = Date.now();
    }
  });
  const variables = new URLSearchParams({ from: '100000', to: '200000', refresh: '',
    'var-environment': 'fixture', 'var-deployment': 'canonical-outcomes',
    'var-chain': '$__all', 'var-rollup': '$__all', 'var-instance': '$__all' });
  const dashboard = (uid, panel) => `${input.url}/d/${uid}?${variables}${panel ? `&viewPanel=${panel}` : ''}`;
  try {
    await page.goto(`${input.url}/login`, { waitUntil: 'domcontentloaded' });
    await page.getByLabel('Email or username').fill('admin');
    await page.getByLabel('Password', { exact: true }).fill(input.password);
    await page.getByRole('button', { name: 'Log in', exact: true }).click();
    await page.waitForURL(url => !url.pathname.startsWith('/login'));
    report.checks.push('private_fixture_login');
    // Native metrics exist now; SQL history intentionally uses 1970 fixtures.
    // Test the live activity presentation at a current range, then exercise
    // historical SQL drill-downs without pretending old metric samples exist.
    const liveVariables = new URLSearchParams(variables);
    liveVariables.set('from', 'now-5m');
    liveVariables.set('to', 'now');
    const beforeLive = completedQueries;
    await page.goto(`${input.url}/d/eez-cross-chain?${liveVariables}`, { waitUntil: 'domcontentloaded' });
    const activity = page.getByRole('region', { name: 'Cross-chain activity · selected range end', exact: true });
    await activity.getByText('Idle / unexercised', { exact: true }).waitFor({ state: 'visible' });
    const retained = page.getByRole('region', { name: 'Last canonical success · retained selected history', exact: true });
    await retained.getByText('inbound', { exact: true }).waitFor({ state: 'visible' });
    await settleQueries(beforeLive);
    await page.screenshot({ path: path.join(input.output, 'current-activity.png') });
    report.checks.push('quiet_direction_is_idle_and_retained_success_is_separate');
    for (const [uid, title] of [['eez-cross-chain', 'Cross-chain'], ['eez-settlement', 'Settlement'],
        ['eez-blobs', 'Blobs'], ['eez-gas', 'Gas']]) {
      const before = completedQueries;
      const response = await page.goto(dashboard(uid), { waitUntil: 'domcontentloaded' });
      assert.equal(response.status(), 200);
      await page.getByText(new RegExp(title)).first().waitFor({ state: 'visible', timeout: 30000 });
      await page.waitForFunction(() => document.querySelectorAll('[data-testid*="panel"]').length > 0,
        null, { timeout: 30000 });
      await settleQueries(before);
      await page.screenshot({ path: path.join(input.output, `${uid}.png`) });
      report.checks.push(`${uid}_renders`);
    }
    const source = '0x' + BigInt(1002).toString(16).padStart(64, '0');
    for (const [link, target] of [['Request timeline', 'eez-cross-chain'],
        ['Related attempts', 'eez-settlement'], ['Source receipt and explorer', 'eez-gas']]) {
      const before = completedQueries;
      await page.goto(dashboard('eez-cross-chain'), { waitUntil: 'domcontentloaded' });
      await page.getByText('Requests · distinct retained admissions', { exact: true }).scrollIntoViewIfNeeded();
      // The table clips long hashes; their text box extends under adjacent
      // columns. Click the visible beginning, as a user would, without force.
      await page.getByText(source, { exact: true }).first().click({ timeout: 30000, position: { x: 5, y: 5 } });
      await settleQueries(before);
      const beforeLink = completedQueries;
      await page.getByText(link, { exact: true }).last().click({ timeout: 10000 });
      await page.waitForURL(url => url.pathname.includes(target) && url.searchParams.get('var-source_hash') === source);
      const navigated = new URL(page.url());
      assert.equal(navigated.searchParams.get('var-environment'), 'fixture');
      assert.equal(navigated.searchParams.get('var-deployment'), 'canonical-outcomes');
      const milliseconds = value => /^\d+$/.test(value) ? Number(value) : Date.parse(value);
      assert.equal(milliseconds(navigated.searchParams.get('from')), 100000);
      assert.equal(milliseconds(navigated.searchParams.get('to')), 200000);
      const detail = {
        'Request timeline': ['Request timeline · optional source hash', 'attempt_generation'],
        'Related attempts': ['Attempt membership · optional source hash', 'AS reported_blob_count'],
        'Source receipt and explorer': ['Receipt detail · optional transaction hash', 'eez.canonical_receipt_facts']
      }[link];
      await page.getByText(detail[0], { exact: true }).scrollIntoViewIfNeeded();
      await settleQueries(beforeLink);
      assert(report.sql_queries.some(query => query.rows > 0 && query.sql.includes(source) && query.sql.includes(detail[1])),
        `${link} must return retained data for the selected source`);
      await page.screenshot({ path: path.join(input.output, `link-${target}-${link.replaceAll(' ', '_')}.png`) });
      report.checks.push(`request_link_${target}_${link.replaceAll(' ', '_')}_preserves_scope_and_time`);
    }
    assert.deepEqual(report.page_errors, []);
    assert.deepEqual(queryFailures, []);
    report.status = 'passed';
  } catch (error) {
    report.status = 'failed';
    report.error = String(error);
    await page.screenshot({ path: path.join(input.output, 'failure.png') }).catch(() => {});
    throw error;
  } finally {
    report.query_failures = queryFailures;
    report.pending_requests = [...pendingRequests].map(request => ({ path: new URL(request.url()).pathname, method: request.method() }));
    fs.writeFileSync(path.join(input.output, 'browser.json'), JSON.stringify(report, null, 2) + '\n');
    await browser.close();
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
