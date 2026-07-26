import { readFile } from "node:fs/promises";
import { join } from "node:path";

// Serves the paper intraday mean-reversion tracker's report, written by
// `python3 scripts/live_paper_intraday.py`.
//
// Companion to /api/live-paper.  These are SIMULATED entries/exits on the
// UP/DOWN token itself (buy low, sell higher within the same market) — NOT
// buy-and-hold-to-resolution bets.  Paper only; no credential, wallet,
// signer or order path.  Nothing here can move money.
export const dynamic = "force-dynamic";

export async function GET() {
  const path = process.env.POLY_LIVE_PAPER_INTRADAY_REPORT_PATH
    ?? join(process.cwd(), "..", "var", "live-paper", "intraday-report.json");
  try {
    const raw = await readFile(path, "utf8");
    const report = JSON.parse(raw) as {
      paper_only?: boolean; live?: boolean; intraday?: boolean;
    };
    if (report.paper_only !== true || report.live !== true || report.intraday !== true) {
      return Response.json(
        { available: false, reason: "report is not a live intraday paper report" },
        { status: 409 },
      );
    }
    return Response.json({
      available: true,
      strategy_status: "RETIRED_NEGATIVE_OOS",
      strategy_reason: "Historical paper P&L is negative. New simulated entries are disabled pending an independent unseen-session, cost-aware promotion decision.",
      report,
    });
  } catch {
    return Response.json({
      available: false,
      reason: "intraday tracker not running — start: python3 scripts/live_paper_intraday.py",
    });
  }
}
