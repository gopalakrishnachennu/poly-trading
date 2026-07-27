/** Integer-micros helpers — never use binary float for P&L math (Spec 094). */

export function microsToUsdText(micros: number | null | undefined): string {
  if (micros === null || micros === undefined || !Number.isFinite(micros)) {
    return "n/a";
  }
  const sign = micros < 0 ? "-" : "";
  const abs = Math.abs(Math.trunc(micros));
  const whole = Math.floor(abs / 1_000_000);
  const frac = String(abs % 1_000_000).padStart(6, "0");
  return `${sign}$${whole}.${frac.slice(0, 2)}`;
}

export function bpsToPctText(bps: number | null | undefined): string {
  if (bps === null || bps === undefined || !Number.isFinite(bps)) return "n/a";
  const whole = Math.trunc(bps / 100);
  const frac = Math.abs(bps % 100);
  return `${whole}.${String(frac).padStart(2, "0")}%`;
}

/** Average of integer micros using integer division (trunc toward zero). */
export function avgMicros(sum: number, count: number): number | null {
  if (count <= 0) return null;
  return Math.trunc(sum / count);
}

export function profitFactorBps(grossWin: number, grossLossAbs: number): number | null {
  if (grossLossAbs <= 0) {
    return grossWin > 0 ? 999_999 : null;
  }
  return Math.trunc((grossWin * 10_000) / grossLossAbs);
}

export function winRateBps(wins: number, closed: number): number | null {
  if (closed <= 0) return null;
  return Math.trunc((wins * 10_000) / closed);
}

export function expectancyMicros(
  wins: number,
  losses: number,
  grossWin: number,
  grossLossAbs: number,
): number | null {
  const closed = wins + losses;
  if (closed <= 0) return null;
  const avgWin = wins > 0 ? Math.trunc(grossWin / wins) : 0;
  const avgLoss = losses > 0 ? Math.trunc(grossLossAbs / losses) : 0;
  // E = p_win * avg_win - p_loss * avg_loss  (integer)
  return Math.trunc((wins * avgWin - losses * avgLoss) / closed);
}

export function sum(nums: number[]): number {
  let s = 0;
  for (const n of nums) s += Math.trunc(n);
  return s;
}
