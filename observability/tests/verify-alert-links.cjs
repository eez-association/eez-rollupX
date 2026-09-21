// Actual Chromium navigation of URLs delivered in the owned Grafana webhook.
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
  const report = { checks: [], page_errors: [], request_errors: [], status: 'failed', scope: 'actual browser follows delivered webhook links' };
  page.on('pageerror', error => report.page_errors.push(error.message));
  page.on('requestfailed', request => report.request_errors.push({ path: new URL(request.url()).pathname, error: request.failure()?.errorText }));
  try {
    await page.goto(input.url + '/login', { waitUntil: 'domcontentloaded' });
    await page.getByLabel('Email or username').fill('admin');
    await page.getByLabel('Password', { exact: true }).fill(input.password);
    await page.getByRole('button', { name: 'Log in', exact: true }).click();
    await page.waitForURL(url => !url.pathname.startsWith('/login'));
    report.checks.push('private_login_required');
    for (const [index, alert] of input.alerts.entries()) {
      const runbook = alert.annotations.runbook_url;
      assert.equal(new URL(runbook).origin, input.url);
      // Render the delivered link as a webhook receiver would, then click it.
      // A receiver is a separate document, not a replaced live Grafana SPA DOM.
      await page.goto('about:blank');
      await page.setContent('<a id="runbook">Open alert runbook</a>');
      await page.locator('#runbook').evaluate((link, href) => { link.href = href; }, runbook);
      await Promise.all([page.waitForURL(url => url.pathname === '/d/eez-runbooks'), page.locator('#runbook').click()]);
      const protocol = new URL(runbook).searchParams.get('viewPanel') === '2';
      await page.getByText(protocol ? /Stage-2 alerts are opt-in/ : /This stack is private operational tooling/)
        .first().waitFor({ state: 'visible', timeout: 30000 });
      await page.screenshot({ path: path.join(input.output, `runbook-${index}.png`) });
      report.checks.push(protocol ? 'protocol_runbook_content_from_delivered_link' : 'operations_runbook_content_from_delivered_link');
      const uid = new URL(alert.dashboardURL).pathname.split('/')[2];
      const response = await context.request.get(input.url + '/api/dashboards/uid/' + uid);
      assert.equal(response.status(), 200);
      const dashboard = await response.json();
      assert.equal(dashboard.dashboard.uid, uid);
      await page.goto('about:blank');
      await page.setContent('<a id="dashboard">Open alert dashboard</a>');
      await page.locator('#dashboard').evaluate((link, href) => { link.href = href; }, alert.dashboardURL);
      await Promise.all([page.waitForURL(url => url.pathname.startsWith('/d/' + uid)), page.locator('#dashboard').click()]);
      await page.getByText(dashboard.dashboard.title, { exact: true }).first().waitFor({ state: 'visible' });
      report.checks.push('matching_dashboard_' + uid);
    }
    report.status = 'passed';
  } catch (error) {
    report.error = String(error);
    report.failed_url = page.url();
    report.body = (await page.locator('body').innerText().catch(() => '')).slice(0, 16000);
    await page.screenshot({ path: path.join(input.output, 'failure.png') }).catch(() => {});
    throw error;
  } finally {
    fs.writeFileSync(path.join(input.output, 'browser.json'), JSON.stringify(report, null, 2) + '\n');
    await browser.close();
  }
}
main().catch(error => { console.error(error); process.exit(1); });
