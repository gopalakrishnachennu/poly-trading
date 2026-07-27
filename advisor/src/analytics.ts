import {
  avgMicros,
  expectancyMicros,
  profitFactorBps,
  sum,
  winRateBps,
} from "./money.js";
import type { ClosedTrade, StrategyRow } from "./types.js";

export function summarizeCloses(closes: ClosedTrade[]) {
  const wins = closes.filter((c) => c.outcome === "win" || c.realized_pnl_micros > 0);
  const losses = closes.filter((c) => c.outcome === "loss" || c.realized_pnl_micros < 0);
  const grossWin = sum(wins.map((c) => Math.max(0, c.realized_pnl_micros)));
  const grossLossAbs = sum(losses.map((c) => Math.abs(Math.min(0, c.realized_pnl_micros))));
  const stakes = closes.map((c) => c.stake_micros ?? 0).filter((s) => s > 0);
  const realized = sum(closes.map((c) => c.realized_pnl_micros));
  const hold = closes.map((c) => c.hold_secs ?? 0).filter((h) => h > 0);
  return {
    sample_size: closes.length,
    wins: wins.length,
    losses: losses.length,
    realized_pnl_micros: realized,
    gross_win_micros: grossWin,
    gross_loss_micros: grossLossAbs,
    avg_win_micros: avgMicros(grossWin, wins.length),
    avg_loss_micros: losses.length ? -avgMicros(grossLossAbs, losses.length)! : null,
    expectancy_micros: expectancyMicros(wins.length, losses.length, grossWin, grossLossAbs),
    profit_factor_bps: profitFactorBps(grossWin, grossLossAbs),
    win_rate_bps: winRateBps(wins.length, closes.length),
    avg_stake_micros: avgMicros(sum(stakes), stakes.length),
    avg_hold_secs: avgMicros(sum(hold), hold.length),
    best_trade_micros: closes.length ? Math.max(...closes.map((c) => c.realized_pnl_micros)) : null,
    worst_trade_micros: closes.length ? Math.min(...closes.map((c) => c.realized_pnl_micros)) : null,
    roi_on_stake_bps: stakes.length
      ? Math.trunc((realized * 10_000) / Math.max(1, sum(stakes)))
      : null,
  };
}

export function rankKillKeep(
  rows: StrategyRow[],
  minCloses = 5,
): {
  kill: unknown[];
  demote: unknown[];
  keep: unknown[];
  promote: unknown[];
} {
  const kill: unknown[] = [];
  const demote: unknown[] = [];
  const keep: unknown[] = [];
  const promote: unknown[] = [];

  for (const row of rows) {
    const closed = row.totals.closed_trades;
    if (closed < minCloses) continue;
    const exp = row.quality?.expectancy_micros
      ?? (row.totals.realized_pnl_micros !== 0 && closed > 0
        ? Math.trunc(row.totals.realized_pnl_micros / closed)
        : null);
    const pf = row.quality?.profit_factor_bps ?? null;
    const wr = row.win_rate_bps;
    const avgWin = row.quality?.avg_win_micros ?? null;
    const avgLoss = row.quality?.avg_loss_micros ?? null;
    const item = {
      setup_id: row.setup_id,
      book: row.book,
      closed_trades: closed,
      win_rate_bps: wr,
      expectancy_micros: exp,
      profit_factor_bps: pf,
      realized_pnl_micros: row.totals.realized_pnl_micros,
      avg_win_micros: avgWin,
      avg_loss_micros: avgLoss,
      current_streak: row.totals.current_streak ?? null,
    };

    const fatLoss = avgWin != null && avgLoss != null && Math.abs(avgLoss) >= avgWin;
    if ((exp != null && exp < 0 && closed >= 8) || (pf != null && pf < 8_000 && closed >= 10)) {
      kill.push({ ...item, suggested: "DISABLE_SETUP", why: "negative expectancy or weak PF" });
    } else if (fatLoss || (exp != null && exp < 0)) {
      demote.push({ ...item, suggested: "DEMOTE_TO_SHADOW", why: "avg loss dominates or soft negative edge" });
    } else if (
      closed >= 20
      && exp != null
      && exp > 0
      && pf != null
      && pf >= 12_000
      && String(row.book).toUpperCase().includes("SHADOW")
    ) {
      promote.push({ ...item, suggested: "PROMOTE_TO_CORE", why: "sample+edge+PF gates" });
    } else if (exp != null && exp >= 0) {
      keep.push({ ...item, suggested: "NO_CHANGE", why: "non-negative expectancy" });
    } else {
      demote.push({ ...item, suggested: "CAP_TIER", why: "insufficient edge clarity" });
    }
  }

  const byExp = (a: unknown, b: unknown) => {
    const ae = (a as { expectancy_micros?: number | null }).expectancy_micros ?? 0;
    const be = (b as { expectancy_micros?: number | null }).expectancy_micros ?? 0;
    return ae - be;
  };
  kill.sort(byExp);
  demote.sort(byExp);
  keep.sort((a, b) => byExp(b, a));
  promote.sort((a, b) => byExp(b, a));
  return { kill, demote, keep, promote };
}

export function findDeathPatterns(closes: ClosedTrade[]) {
  type Bucket = {
    key: string;
    setup_id: string;
    window: string;
    asset: string;
    tier: string;
    hold_bucket: string;
    ask_bucket: string;
    n: number;
    wins: number;
    losses: number;
    realized_pnl_micros: number;
  };
  const map = new Map<string, Bucket>();

  for (const c of closes) {
    const hold = c.hold_secs ?? 0;
    const hold_bucket = hold < 30 ? "lt30s" : hold < 120 ? "30s-2m" : hold < 600 ? "2m-10m" : "ge10m";
    const ask = c.ask_micros ?? c.effective_cost_micros ?? 0;
    const ask_bucket = ask >= 800_000 ? "high_ask" : ask >= 600_000 ? "mid_ask" : "low_ask";
    const tier = c.stake_tier ?? "unknown";
    const setup_id = c.setup_id ?? "unknown";
    const key = `${setup_id}|${c.window}|${c.asset}|${tier}|${hold_bucket}|${ask_bucket}`;
    const b = map.get(key) ?? {
      key,
      setup_id,
      window: c.window,
      asset: c.asset,
      tier,
      hold_bucket,
      ask_bucket,
      n: 0,
      wins: 0,
      losses: 0,
      realized_pnl_micros: 0,
    };
    b.n += 1;
    if (c.outcome === "win" || c.realized_pnl_micros > 0) b.wins += 1;
    if (c.outcome === "loss" || c.realized_pnl_micros < 0) b.losses += 1;
    b.realized_pnl_micros += c.realized_pnl_micros;
    map.set(key, b);
  }

  return [...map.values()]
    .filter((b) => b.n >= 3 && b.realized_pnl_micros < 0)
    .sort((a, b) => a.realized_pnl_micros - b.realized_pnl_micros)
    .slice(0, 40);
}

export function rejectReasonBreakdown(decisions: Record<string, unknown>[]) {
  const counts = new Map<string, number>();
  for (const d of decisions) {
    const reasons = Array.isArray(d.rejection_reasons)
      ? d.rejection_reasons.map(String)
      : d.reason
        ? [String(d.reason)]
        : [];
    if (String(d.action) !== "no_trade" && reasons.length === 0) continue;
    for (const r of reasons) {
      counts.set(r, (counts.get(r) ?? 0) + 1);
    }
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 40);
}

export function stakeTierFromPerf(closed: number, winRateBps: number | null): string {
  if (closed < 5) return "unproven";
  if (winRateBps == null) return "unproven";
  if (winRateBps >= 6_500) return "strong";
  if (winRateBps >= 5_500) return "good";
  if (winRateBps >= 4_500) return "average";
  return "weak";
}

export function expectedRiskBps(tier: string, experimental: boolean): number {
  const core: Record<string, number> = {
    unproven: 200,
    weak: 150,
    average: 300,
    good: 600,
    strong: 1_000,
  };
  let bps = core[tier] ?? 200;
  if (experimental) bps = Math.min(300, Math.max(150, Math.trunc(bps / 2)));
  return bps;
}
