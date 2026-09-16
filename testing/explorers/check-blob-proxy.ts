import assert from "node:assert/strict";
import handler from "./apps/web/src/pages/api/blob-data";

async function main() {
  const hash = "0x01" + "23".repeat(31);
  const allowed = `http://blobscan-api:3001/blobs/${hash}/data`;
  const bytes = Buffer.alloc(131072, 0x80);
  const calls: { url: string; redirect?: RequestRedirect }[] = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url: String(url), redirect: init?.redirect });
    return new Response(JSON.stringify("0x" + bytes.toString("hex")), {
      headers: { "content-type": "application/json" },
    });
  };

  async function request(query: Record<string, unknown>, method = "GET") {
    const result = { status: 200, headers: {} as Record<string, string>, body: undefined as unknown };
    const response = {
      status(code: number) { result.status = code; return this; },
      setHeader(name: string, value: string) { result.headers[name] = value; return this; },
      json(body: unknown) { result.body = body; return this; },
      send(body: unknown) { result.body = body; return this; },
    };
    await handler({ query, method } as never, response as never);
    return result;
  }

  const valid = await request({ storage: "postgres", url: allowed });
  assert.equal(valid.status, 200);
  assert.equal(valid.headers["Content-Type"], "application/octet-stream");
  assert.deepEqual(valid.body, bytes);
  assert.deepEqual(calls, [{ url: allowed, redirect: "error" }]);

  for (const query of [
    {}, { storage: "postgres" }, { url: allowed },
    { storage: ["postgres", "s3"], url: allowed },
    { storage: "postgres", url: [allowed, allowed] },
    { storage: "s3", url: allowed },
    { storage: "postgres", url: "http://127.0.0.1:2019/config/" },
    { storage: "postgres", url: "http://169.254.169.254/latest/meta-data/" },
    { storage: "postgres", url: "http://blobscan-api:3001/blockchain-sync-state" },
    { storage: "postgres", url: allowed.replace("blobscan-api:3001", "example.com") },
    { storage: "postgres", url: allowed.replace("http:", "file:") },
    { storage: "postgres", url: allowed.replace("http://", "http://user:pass@") },
    { storage: "postgres", url: allowed + "?redirect=http://example.com" },
    { storage: "postgres", url: allowed + "#fragment" },
    { storage: "postgres", url: allowed.replace(hash, "bad-hash") },
    { storage: "postgres", url: allowed.replace("/data", "/%2e%2e/admin") },
  ]) {
    assert.equal((await request(query)).status, 400, JSON.stringify(query));
  }
  const post = await request({ storage: "postgres", url: allowed }, "POST");
  assert.equal(post.status, 405);
  assert.equal(post.headers.Allow, "GET");
  assert.equal(calls.length, 1, "Rejected inputs must never cause an outbound fetch");
  console.log("Blob download: exact bytes, archive-only URLs, no redirects, and rejected requests without fetches passed");
}

main().catch(error => { console.error(error); process.exitCode = 1; });
