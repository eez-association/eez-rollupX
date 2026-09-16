const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

function requiredUrl(name) {
  const value = process.env[name];
  assert(value, `${name} must be set`);
  const url = new URL(value);
  assert(['http:', 'https:'].includes(url.protocol), `${name} must be an HTTP(S) URL`);
  return url.href.replace(/\/$/, '');
}

async function main() {
  const publicUrl = requiredUrl('EEZ_PUBLIC_URL');
  const l2ExplorerUrl = requiredUrl('EEZ_L2_EXPLORER_URL');
  const blobscanUrl = requiredUrl('EEZ_BLOBSCAN_URL');
  const output = process.argv[2] || 'artifacts/public-explorer-monitor';
  fs.mkdirSync(output, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  const report = { errors: [], failed: [], links: [] };
  try {
    const page = await browser.newPage({ viewport: { width: 1600, height: 1100 } });
    page.on('pageerror', e => report.errors.push(e.message));
    page.on('console', m => { if (m.type() === 'error') report.errors.push(m.text()); });
    page.on('requestfailed', r => report.failed.push({ url: r.url(), error: r.failure()?.errorText }));
    const response = await page.goto(`${publicUrl}/monitor/`, { waitUntil: 'domcontentloaded' });
    assert.equal(response.status(), 200);
    await page.locator('#resource-links a').filter({ hasText: 'L2 Blockscout' }).waitFor();
    report.links = await page.locator('#resource-links a').evaluateAll(links => links.map(a => ({ label: a.textContent, href: a.href })));
    assert(report.links.some(a => a.label === 'L2 Blockscout' && a.href === `${l2ExplorerUrl}/`));
    assert(report.links.some(a => a.label === 'Blobscan' && a.href === `${blobscanUrl}/`));
    assert(report.links.some(a => a.label === 'L1 Blockscout' && a.href === 'https://gnosis-chiado.blockscout.com/'));

    const snapshotResponse = await page.request.get(`${publicUrl}/monitor/api/snapshot`);
    assert.equal(snapshotResponse.status(), 200);
    const snapshot = await snapshotResponse.json();
    assert.equal(snapshot.configuration.explorers.l2, l2ExplorerUrl);
    assert.equal(snapshot.configuration.explorers.blobscan, blobscanUrl);
    assert.equal(snapshot.configuration.settlementPolicy.maxUnsettledL2Blocks, 1800);
    assert.equal(snapshot.configuration.settlementPolicy.pureL2Mode, 'always');
    report.configuration = snapshot.configuration;

    // Follow a safe L2 block through the link rendered by the real monitor.
    const blockLink = page.locator('#l2-safe a');
    report.l2BlockHref = await blockLink.getAttribute('href');
    assert(report.l2BlockHref.startsWith(`${l2ExplorerUrl}/block/`));
    const [blockPage] = await Promise.all([page.waitForEvent('popup'), blockLink.click()]);
    await blockPage.waitForLoadState('domcontentloaded');
    const blockNumber = Number(new URL(report.l2BlockHref).pathname.split('/').pop());
    await blockPage.getByRole('heading', { name: `Block #${blockNumber}`, exact: true }).waitFor();
    const indexedResponse = await blockPage.request.get(`${l2ExplorerUrl}/api/v2/blocks/${blockNumber}`);
    assert.equal(indexedResponse.status(), 200);
    const indexed = await indexedResponse.json();
    const canonicalResponse = await page.request.post(`${publicUrl}/rpc/l2`, {
      data: { jsonrpc: '2.0', id: 1, method: 'eth_getBlockByNumber', params: [`0x${blockNumber.toString(16)}`, false] },
    });
    const canonical = await canonicalResponse.json();
    assert.equal(indexed.hash, canonical.result.hash);
    report.openedBlock = { number: blockNumber, hash: indexed.hash };
    await blockPage.close();

    // Check the archive integration using a finalized historical two-blob post.
    const tx = '0x73d0ad1c2c67c824f51a146d6b5d28dd6c6ae55e0eddfbf68119695a67b2398b';
    const decodeResponse = await page.request.get(`${publicUrl}/monitor/api/blob-decode?tx=${tx}`);
    assert.equal(decodeResponse.status(), 200, await decodeResponse.text());
    const decoded = await decodeResponse.json();
    assert.equal(decoded.transactionHash, tx);
    assert.equal(decoded.blobVersionedHashes.length, 2);
    assert.equal(decoded.structuralDecodeOnly, true);
    report.decode = { transactionHash: tx, blobCount: decoded.blobVersionedHashes.length, structuralDecodeOnly: decoded.structuralDecodeOnly };

    const blobLinks = page.locator(`a[href^="${blobscanUrl}/blob/"]`);
    assert(await blobLinks.count() > 0);
    report.blobHref = await blobLinks.last().getAttribute('href');
    const blobResponse = await page.request.get(report.blobHref);
    assert.equal(blobResponse.status(), 200);
    assert.equal(report.errors.length, 0, JSON.stringify(report.errors));
    assert.equal(report.failed.length, 0, JSON.stringify(report.failed));
    await page.screenshot({ path: path.join(output, 'monitor.png'), fullPage: true });
    report.passed = true;
    console.log(JSON.stringify(report));
  } finally {
    fs.writeFileSync(path.join(output, 'monitor.json'), JSON.stringify(report, null, 2) + '\n');
    await browser.close();
  }
}

main().catch(e => { console.error(e); process.exitCode = 1; });
