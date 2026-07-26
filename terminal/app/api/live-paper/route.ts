// The historical directional simulator is intentionally not an active paper
// authority. It used a separate float bankroll and bypassed the core-backed
// paper ledger/risk/reconciliation path. The Paper Week API is the only active
// paper authority; this endpoint exists solely to make retirement explicit.
export const dynamic = "force-dynamic";

export async function GET() {
  return Response.json({
    available: false,
    strategy_status: "RETIRED_LEGACY_BYPASS",
    reason: "Legacy directional paper simulator is retired because it bypasses the core-backed fixed-point ledger, scenario-risk, reservation and reconciliation path. Use Paper Week instead.",
  });
}
