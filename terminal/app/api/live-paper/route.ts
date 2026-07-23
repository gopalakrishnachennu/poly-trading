import { readFile } from "node:fs/promises";
import { join } from "node:path";

// Serves the live directional paper trader's report, written by
// `python3 scripts/live_paper_trader.py`.
//
// These are simulated positions taken against the live tape in real time. The
// trader holds no credential, wallet, signer or transport and places no order;
// nothing in this path can move money.
export const dynamic = "force-dynamic";

export async function GET() {
  const path = process.env.POLY_LIVE_PAPER_REPORT_PATH
    ?? join(process.cwd(), "..", "var", "live-paper", "report.json");
  try {
    const raw = await readFile(path, "utf8");
    const report = JSON.parse(raw) as { paper_only?: boolean; live?: boolean };
    if (report.paper_only !== true || report.live !== true) {
      return Response.json({ available: false, reason: "report is not a live paper report" }, { status: 409 });
    }
    return Response.json({ available: true, report });
  } catch {
    return Response.json({
      available: false,
      reason: "live paper trader not running — start: python3 scripts/live_paper_trader.py",
    });
  }
}
