// Requires Playwright and its Chromium browser; this only reads the explorers.
const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

const output = process.argv[2] || 'artifacts/explorer-browser';
const blockscoutOrigin = new URL(process.env.EEZ_BLOCKSCOUT_BROWSER_URL || 'http://127.0.0.1:18110').origin;
const blobscanOrigin = new URL(process.env.EEZ_BLOBSCAN_BROWSER_URL || 'http://127.0.0.1:18100').origin;
const blobHash = '0x018412fe07f4ed2ff77ae4c5412c63a217a61063986c3028a456f54905fb34ee';
const txHash = '0x73d0ad1c2c67c824f51a146d6b5d28dd6c6ae55e0eddfbf68119695a67b2398b';
const expectedSha = 'a4c781c1b72cf8af4b929aa70bd1fcbfa119528ec2c84df35a0f8c5cc1b2c589';

async function main() {
  fs.mkdirSync(output, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  const results = [];
  try {
    for (const [name, url] of [
      ['blockscout', `${blockscoutOrigin}/`],
      ['blobscan-home', `${blobscanOrigin}/`],
      ['blobscan-transaction', `${blobscanOrigin}/tx/${txHash}`],
      ['blobscan-blob', `${blobscanOrigin}/blob/${blobHash}`],
    ]) {
      const page = await browser.newPage({ viewport: { width: 1600, height: 1050 } });
      const errors = [], failed = [], responses = [], sockets = [];
      page.on('pageerror', e => errors.push(e.message));
      page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
      page.on('requestfailed', r => failed.push({ url: r.url(), error: r.failure()?.errorText }));
      page.on('response', r => {
        if (r.url().includes('/api/')) responses.push({ url: r.url(), status: r.status() });
      });
      page.on('websocket', s => {
        const entry = { url: s.url(), joins: 0, blocks: 0, errors: [] };
        sockets.push(entry);
        s.on('socketerror', e => entry.errors.push(String(e)));
        s.on('framereceived', ({ payload }) => {
          try {
            const f = JSON.parse(payload.toString());
            if (f[3] === 'phx_reply' && f[4]?.status === 'ok') entry.joins++;
            if (f[3] === 'new_block') entry.blocks++;
          } catch { /* Ignore non-JSON control frames. */ }
        });
      });
      const response = await page.goto(url, { waitUntil: 'domcontentloaded' });
      await page.waitForTimeout(name === 'blockscout' ? 18000 : 8000);
      const text = await page.locator('body').innerText();
      const result = { name, url, status: response.status(), errors, failed, responses, sockets };
      assert.equal(result.status, 200);
      assert.equal(errors.length, 0, JSON.stringify(errors));
      assert.equal(failed.length, 0, JSON.stringify(failed));
      assert(responses.length && responses.every(r => r.status < 400));
      if (name === 'blockscout') {
        result.env = await page.evaluate(() => ({
          chainId: window.__envs.NEXT_PUBLIC_NETWORK_ID,
          apiHost: window.__envs.NEXT_PUBLIC_API_HOST,
        }));
        assert.equal(result.env.chainId, '6290');
        assert(sockets.some(s => s.joins > 0 && s.blocks > 0 && !s.errors.length));
        assert(sockets.every(s => s.url.startsWith(blockscoutOrigin.replace(/^http/, 'ws') + '/')));
      } else {
        assert(text.includes('Chiado'));
        assert(!text.includes('$0.00'), 'Unavailable exchange prices must not become zero-dollar fees');
        assert(!text.includes('Blobscan is migrating to IPFS'));
        assert(text.includes('Not supplied by beacon API (Fulu)'));
        assert(!text.includes('0xc0' + '00'.repeat(47)), 'Do not present the Fulu placeholder as a usable proof');
        if (name === 'blobscan-home') {
          assert(!text.includes('No Chart Data'), 'Completed-day statistics should be initialized');
        }
        if (name === 'blobscan-transaction') {
          assert(text.includes('256 KiB (2 blobs)'));
          assert(text.includes('0.000262144 xDAI'));
        }
        if (name === 'blobscan-blob') {
          const href = await page.locator('a[href^="/api/blob-data"]').first().getAttribute('href');
          const download = await page.request.get(new URL(href, url).href);
          const bytes = await download.body();
          const native = await page.request.get(`http://127.0.0.1:18101/blobs/${blobHash}/data`);
          const nativeHex = await native.json();
          result.download = {
            status: download.status(), bytes: bytes.length,
            sha256: createHash('sha256').update(bytes).digest('hex'),
          };
          assert.equal(download.status(), 200);
          assert.equal(bytes.length, 131072);
          assert.equal(result.download.sha256, expectedSha);
          assert.deepEqual(bytes, Buffer.from(nativeHex.slice(2), 'hex'));
          assert(text.includes(nativeHex.slice(0, 128)), 'Raw display must contain actual blob bytes');
        }
      }
      result.textExcerpt = text.slice(0, 5000);
      await page.screenshot({ path: path.join(output, name + '.png') });
      results.push(result);
      console.log(JSON.stringify({ name, passed: true, download: result.download, sockets }));
      await page.close();
    }
  } finally {
    fs.writeFileSync(path.join(output, 'browser.json'), JSON.stringify(results, null, 2) + '\n');
    await browser.close();
  }
}

main().catch(e => { console.error(e); process.exitCode = 1; });
