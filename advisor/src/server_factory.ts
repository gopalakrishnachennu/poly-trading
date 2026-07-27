import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { readFile } from "node:fs/promises";
import { ensureAdvisorDirs, paths } from "./paths.js";
import { ADVISOR_SYSTEM_PROMPT, CURRENT_POLICY } from "./policy.js";
import { registerAllTools } from "./tools.js";
import { listPatterns, listSuggestions } from "./store.js";
import { readJsonFile } from "./journal.js";
import { fetchLivePayload } from "./api.js";

/** Build a fully wired Poly Advisor MCP server (tools + resources). */
export async function createAdvisorServer(): Promise<McpServer> {
  await ensureAdvisorDirs();
  try {
    const { writeFile } = await import("node:fs/promises");
    await writeFile(paths.advisorPrompt(), ADVISOR_SYSTEM_PROMPT, "utf8");
  } catch {
    /* non-fatal */
  }

  const server = new McpServer({
    name: "poly-advisor",
    version: "0.1.0",
  });

  registerAllTools(server);

  server.registerResource(
    "status",
    "resource://poly/status",
    { description: "Latest live paper status payload", mimeType: "application/json" },
    async () => {
      const r = await fetchLivePayload();
      return {
        contents: [{
          uri: "resource://poly/status",
          mimeType: "application/json",
          text: JSON.stringify(r.payload ?? { error: r.error }, null, 2),
        }],
      };
    },
  );

  server.registerResource(
    "features",
    "resource://poly/features",
    { description: "Latest LLM feature rollup", mimeType: "application/json" },
    async () => {
      const data = await readJsonFile(paths.featuresLatest());
      return {
        contents: [{
          uri: "resource://poly/features",
          mimeType: "application/json",
          text: JSON.stringify(data ?? { note: "run capture_snapshot_now or snapshot daemon" }, null, 2),
        }],
      };
    },
  );

  server.registerResource(
    "scoreboard",
    "resource://poly/scoreboard",
    { description: "by_strategy scoreboard", mimeType: "application/json" },
    async () => {
      const r = await fetchLivePayload();
      return {
        contents: [{
          uri: "resource://poly/scoreboard",
          mimeType: "application/json",
          text: JSON.stringify(r.payload?.ledger?.by_strategy ?? [], null, 2),
        }],
      };
    },
  );

  server.registerResource(
    "policy",
    "resource://poly/policy",
    { description: "Spec 093 stake/withdraw policy", mimeType: "application/json" },
    async () => ({
      contents: [{
        uri: "resource://poly/policy",
        mimeType: "application/json",
        text: JSON.stringify(CURRENT_POLICY, null, 2),
      }],
    }),
  );

  server.registerResource(
    "advisor-prompt",
    "resource://poly/advisor-prompt",
    { description: "System prompt for Claude/ChatGPT advisor runs", mimeType: "text/markdown" },
    async () => {
      let text = ADVISOR_SYSTEM_PROMPT;
      try {
        text = await readFile(paths.advisorPrompt(), "utf8");
      } catch {
        /* use constant */
      }
      return {
        contents: [{
          uri: "resource://poly/advisor-prompt",
          mimeType: "text/markdown",
          text,
        }],
      };
    },
  );

  server.registerResource(
    "patterns",
    "resource://poly/patterns",
    { description: "Pattern library index", mimeType: "application/json" },
    async () => ({
      contents: [{
        uri: "resource://poly/patterns",
        mimeType: "application/json",
        text: JSON.stringify(await listPatterns(), null, 2),
      }],
    }),
  );

  server.registerResource(
    "suggestions-latest",
    "resource://poly/suggestions/latest",
    { description: "Latest saved suggestion", mimeType: "application/json" },
    async () => {
      const latest = await readJsonFile(`${paths.suggestionsDir()}/latest.json`);
      const fallback = (await listSuggestions(1))[0] ?? null;
      return {
        contents: [{
          uri: "resource://poly/suggestions/latest",
          mimeType: "application/json",
          text: JSON.stringify(latest ?? fallback ?? { note: "none" }, null, 2),
        }],
      };
    },
  );

  return server;
}
