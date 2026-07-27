#!/usr/bin/env node
/**
 * Poly Advisor MCP — Streamable HTTP for ChatGPT Pro (remote MCP).
 *
 * ChatGPT cannot spawn local stdio. It connects to an HTTPS MCP URL.
 * Run this on the laptop, then tunnel with cloudflared/ngrok.
 *
 * Auth: Authorization: Bearer <POLY_ADVISOR_HTTP_TOKEN>
 * Spec 094 / ADR 0098 — still paper research only; no order tools.
 */
import { randomBytes } from "node:crypto";
import type { Request, Response } from "express";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { createMcpExpressApp } from "@modelcontextprotocol/sdk/server/express.js";
import { createAdvisorServer } from "./server_factory.js";

const host = process.env.POLY_ADVISOR_HTTP_HOST ?? "127.0.0.1";
const port = Number(process.env.POLY_ADVISOR_HTTP_PORT ?? 8787);
const token = process.env.POLY_ADVISOR_HTTP_TOKEN ?? randomBytes(24).toString("hex");

/** Hostnames allowed in the Host header (localhost + Cloudflare/ngrok tunnel host). */
function allowedHosts(): string[] {
  const hosts = new Set<string>(["127.0.0.1", "localhost", "[::1]"]);
  const pub = process.env.POLY_ADVISOR_PUBLIC_HOST?.trim();
  if (pub) {
    hosts.add(pub.replace(/^https?:\/\//, "").replace(/\/.*$/, ""));
  }
  for (const h of (process.env.POLY_ADVISOR_ALLOWED_HOSTS ?? "").split(",")) {
    const t = h.trim();
    if (t) hosts.add(t);
  }
  return [...hosts];
}

function authorize(req: Request): boolean {
  const header = String(req.headers.authorization ?? "");
  if (header === `Bearer ${token}`) return true;
  const alt = String(req.headers["x-poly-advisor-token"] ?? "");
  return alt === token;
}

async function main() {
  const hosts = allowedHosts();
  const app = createMcpExpressApp({ host, allowedHosts: hosts });

  app.get("/healthz", (_req: Request, res: Response) => {
    res.json({
      ok: true,
      service: "poly-advisor-http",
      paper_only: true,
      live_orders: false,
    });
  });

  app.post("/mcp", async (req: Request, res: Response) => {
    if (!authorize(req)) {
      res.status(401).json({ error: "unauthorized — Bearer POLY_ADVISOR_HTTP_TOKEN required" });
      return;
    }
    const server = await createAdvisorServer();
    try {
      const transport = new StreamableHTTPServerTransport({
        sessionIdGenerator: undefined,
      });
      await server.connect(transport);
      await transport.handleRequest(req, res, req.body);
      res.on("close", () => {
        void transport.close();
        void server.close();
      });
    } catch (error) {
      console.error("MCP HTTP error:", error);
      if (!res.headersSent) {
        res.status(500).json({
          jsonrpc: "2.0",
          error: { code: -32603, message: "Internal server error" },
          id: null,
        });
      }
    }
  });

  app.get("/mcp", (_req: Request, res: Response) => {
    res.status(405).json({
      jsonrpc: "2.0",
      error: { code: -32000, message: "Method not allowed — use POST /mcp" },
      id: null,
    });
  });

  app.listen(port, host, () => {
    console.error(`[poly-advisor-http] listening http://${host}:${port}/mcp`);
    console.error(`[poly-advisor-http] health  http://${host}:${port}/healthz`);
    console.error(`[poly-advisor-http] allowed_hosts ${hosts.join(",")}`);
    console.error(`[poly-advisor-http] token   ${token}`);
    console.error("[poly-advisor-http] For ChatGPT: tunnel this port with HTTPS, then paste URL + Bearer token.");
  });
}

main().catch((err) => {
  console.error("poly-advisor HTTP failed:", err);
  process.exit(1);
});
