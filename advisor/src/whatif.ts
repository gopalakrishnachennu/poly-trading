import { summarizeCloses } from "./analytics.js";
import type { ClosedTrade } from "./types.js";

export function whatIfDisableSetups(closes: ClosedTrade[], setupIds: string[]) {
  const ban = new Set(setupIds);
  const kept = closes.filter((c) => !ban.has(c.setup_id ?? ""));
  const removed = closes.filter((c) => ban.has(c.setup_id ?? ""));
  return {
    baseline: summarizeCloses(closes),
    after: summarizeCloses(kept),
    removed: summarizeCloses(removed),
    disabled_setups: setupIds,
    delta_realized_pnl_micros:
      summarizeCloses(kept).realized_pnl_micros - summarizeCloses(closes).realized_pnl_micros,
  };
}

/** Cap stake by scaling realized PnL roughly with stake ratio (integer). */
export function whatIfCapRiskBps(closes: ClosedTrade[], maxRiskBps: number) {
  const scaled: ClosedTrade[] = closes.map((c) => {
    const risk = c.risk_bps ?? null;
    if (risk == null || risk <= maxRiskBps || risk <= 0) return c;
    const scaledPnl = Math.trunc((c.realized_pnl_micros * maxRiskBps) / risk);
    const scaledStake = c.stake_micros != null
      ? Math.trunc((c.stake_micros * maxRiskBps) / risk)
      : c.stake_micros;
    return { ...c, realized_pnl_micros: scaledPnl, stake_micros: scaledStake, risk_bps: maxRiskBps };
  });
  return {
    max_risk_bps: maxRiskBps,
    baseline: summarizeCloses(closes),
    after: summarizeCloses(scaled),
    note: "Linear stake/PnL scale — approximate counterfactual, not a full replay.",
  };
}

export function whatIfMinSample(closes: ClosedTrade[], minCloses: number) {
  const counts = new Map<string, number>();
  // First pass chronological counts of closes per setup before each trade
  const ordered = [...closes].sort((a, b) => a.observed_at_ns - b.observed_at_ns);
  const kept: ClosedTrade[] = [];
  const skipped: ClosedTrade[] = [];
  for (const c of ordered) {
    const id = c.setup_id ?? "unknown";
    const prior = counts.get(id) ?? 0;
    if (prior < minCloses) {
      skipped.push(c);
    } else {
      kept.push(c);
    }
    counts.set(id, prior + 1);
  }
  return {
    min_closes_before_trade_counts: minCloses,
    baseline: summarizeCloses(closes),
    after: summarizeCloses(kept),
    skipped: summarizeCloses(skipped),
    note: "Treats early closes as 'would not have traded' until setup had min prior closes that day.",
  };
}

export function edgeVsSizeReport(closes: ClosedTrade[]) {
  const byTier = new Map<string, ClosedTrade[]>();
  for (const c of closes) {
    const tier = c.stake_tier ?? (c.risk_bps != null && c.risk_bps >= 800 ? "high_bps" : "unknown");
    const arr = byTier.get(tier) ?? [];
    arr.push(c);
    byTier.set(tier, arr);
  }
  const tiers = [...byTier.entries()].map(([tier, rows]) => ({
    tier,
    ...summarizeCloses(rows),
  }));
  const highWrLosses = closes.filter((c) => {
    // large stakes that lost
    return (c.risk_bps ?? 0) >= 600 && c.realized_pnl_micros < 0;
  });
  return {
    by_tier: tiers,
    large_stake_losses: summarizeCloses(highWrLosses),
    warning:
      "If high tiers show negative expectancy, Spec 093 win-rate sizing is amplifying a selection problem — prefer DISABLE/CAP_TIER over bigger bets.",
  };
}
