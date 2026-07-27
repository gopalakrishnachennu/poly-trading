#!/usr/bin/env node
/**
 * Poly Advisor MCP — stdio server (Spec 094 / ADR 0098).
 * For Claude Desktop / Cursor.
 */
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createAdvisorServer } from "./server_factory.js";

async function main() {
  const server = await createAdvisorServer();
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("poly-advisor MCP failed:", err);
  process.exit(1);
});
