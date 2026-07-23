import { readFile } from "node:fs/promises";
import { join } from "node:path";

// Serves the offline betting-engine report written by
// `python3 scripts/hourly_engine.py --json var/engine-report.json`.
//
// This is BACKTEST evidence over captured data, not live trading: the report is
// a static file produced by an offline run. It carries no order, signer or
// capital authority, and nothing here can place a bet.
export const dynamic = "force-dynamic";

export async function GET() {
  const path = process.env.POLY_ENGINE_REPORT_PATH
    ?? join(process.cwd(), "..", "var", "engine-report.json");
  try {
    const raw = await readFile(path, "utf8");
    const report = JSON.parse(raw) as { paper_only?: boolean };
    if (report.paper_only !== true) {
      return Response.json({ available: false, reason: "report is not marked paper_only" }, { status: 409 });
    }
    return Response.json({ available: true, report });
  } catch {
    return Response.json({
      available: false,
      reason: "no engine report yet — run: python3 scripts/hourly_engine.py --json var/engine-report.json",
    });
  }
}
