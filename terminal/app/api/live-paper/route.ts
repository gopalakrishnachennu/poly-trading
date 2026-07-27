// The historical directional simulator is intentionally not an active paper
// authority. It used a separate float bankroll and bypassed the core-backed
// paper ledger/risk/reconciliation path. The Paper Week API is the only active
// paper authority; this endpoint exists solely to make retirement explicit.
export const dynamic = "force-dynamic";

export async function GET() {
  return Response.json({
    available: false,
    strategy_status: "DISABLED",
    reason: "Legacy directional paper simulator is DISABLED. Active directional paper evaluation is btc_window_engine_v1 (Positions → BTC WINDOW PAPER). Pair capital remains Paper Week only.",
  });
}
