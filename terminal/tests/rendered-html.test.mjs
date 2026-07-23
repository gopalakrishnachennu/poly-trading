import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function render() {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}`);
  const { default: worker } = await import(workerUrl.href);
  return worker.fetch(
    new Request("http://localhost/", { headers: { accept: "text/html" } }),
    { ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) } },
    { waitUntil() {}, passThroughOnException() {} },
  );
}

test("server render fails closed before the public gateway is available", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);
  const html = await response.text();
  assert.match(html, /Poly Terminal — Event Markets Control/);
  assert.match(html, /GLOBAL NO_TRADE/);
  assert.match(html, /awaiting read-only projection gateway/);
  assert.match(html, /NO SIMULATED PAIRS/);
  assert.match(html, /Available paper cash<\/span><b>UNAVAILABLE/);
  assert.match(html, /NO EXTERNAL ORDERS/);
  assert.doesNotMatch(html, /SIMULATED FEED|DEMO PROJECTION|\$10,000\.00|\$9,800\.00/);
});

test("client source enforces the projection authority and freshness contract", async () => {
  const page = await readFile(new URL("../app/page.tsx", import.meta.url), "utf8");
  assert.match(page, /item\.no_trade !== true/);
  assert.match(page, /item\.credentials_present !== false/);
  assert.match(page, /item\.authenticated_transport_present !== false/);
  assert.match(page, /item\.order_submission_present !== false/);
  assert.match(page, /item\.financial_authority_present !== false/);
  assert.match(page, /names !== "BTC,ETH"/);
  assert.match(page, /projection exceeded client freshness budget/);
  assert.match(page, /projection clock is ahead of client/);
  assert.match(page, /projection sequence regressed/);
  assert.match(page, /book\.received_at_ms >= book\.source_timestamp_ms/);
  assert.match(page, /validQuantity\(\(level as Partial<Level>\)\.quantity_micros, false\)/);
  assert.match(page, /9_223_372_036_854_775_807n/);
  assert.doesNotMatch(page, /setSnapshot\(null\)/);
  assert.match(page, /Keep the last verified frame visible/);
  assert.match(page, /next\.mode === "ready" \|\| current === null/);
  assert.match(page, /next\.mode === "ready" \? "" : next\.reason/);
  assert.doesNotMatch(page, /118642\.37|3641\.82|btcBook|ethBook|SIMULATED FEED/);
});
