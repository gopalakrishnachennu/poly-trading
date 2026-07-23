import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import test from "node:test";

const terminalRoot = new URL("..", import.meta.url);
const nextBin = new URL("node_modules/.bin/next", terminalRoot).pathname;

// Boot the production build on a loopback port, fetch the shell, then stop it.
// Standard Next.js server-renders the client shell, so the initial HTML must
// already be fail-closed before any projection frame arrives from the gateway.
async function render() {
  const port = 3100 + (process.pid % 800);
  const server = spawn(nextBin, ["start", "--hostname", "127.0.0.1", "--port", String(port)], {
    cwd: terminalRoot.pathname,
    stdio: ["ignore", "ignore", "inherit"],
    env: { ...process.env, NODE_ENV: "production" },
  });

  try {
    const base = `http://127.0.0.1:${port}`;
    const deadline = Date.now() + 30_000;
    for (;;) {
      try {
        const response = await fetch(`${base}/`, { headers: { accept: "text/html" } });
        if (response.status === 200) {
          return { status: response.status, headers: response.headers, html: await response.text() };
        }
      } catch {
        // Server not accepting connections yet; retry until the deadline.
      }
      if (Date.now() > deadline) throw new Error("next start did not become ready within 30s");
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  } finally {
    server.kill("SIGTERM");
  }
}

test("server render fails closed before the public gateway is available", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);
  const html = response.html;
  assert.match(html, /Poly Terminal — Event Markets Control/);
  assert.match(html, /GLOBAL NO_TRADE/);
  assert.match(html, /awaiting read-only projection gateway/);
  // Default MARKETS workspace must render fail-closed: no market identity and no book.
  assert.match(html, /NO COMPLETE CURRENT BTC\/ETH PROJECTION/);
  assert.match(html, /AUTHORITATIVE BOOK UNAVAILABLE/);
  assert.match(html, /NO EXTERNAL ORDERS/);
  assert.doesNotMatch(html, /SIMULATED FEED|DEMO PROJECTION|\$10,000\.00|\$9,800\.00/);
});

test("client source enforces the projection authority and freshness contract", async () => {
  const page = await readFile(new URL("app/page.tsx", terminalRoot), "utf8");
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
