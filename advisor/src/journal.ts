import { createReadStream } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createInterface } from "node:readline";
import { journalPathForDay, todayUtc } from "./paths.js";
import type { ClosedTrade } from "./types.js";

const CLOSE_ACTIONS = new Set(["exit", "settle", "time_stop", "hard_stop", "hedge_reduce"]);

export async function readJournalLines(
  day = todayUtc(),
  opts: { from_ns?: number; to_ns?: number; limit?: number; action?: string } = {},
): Promise<{ path: string; lines: Record<string, unknown>[]; truncated: boolean }> {
  const path = journalPathForDay(day);
  const lines: Record<string, unknown>[] = [];
  let truncated = false;
  try {
    await stat(path);
  } catch {
    return { path, lines: [], truncated: false };
  }
  const rl = createInterface({ input: createReadStream(path, { encoding: "utf8" }), crlfDelay: Infinity });
  for await (const line of rl) {
    if (!line.trim()) continue;
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(line) as Record<string, unknown>;
    } catch {
      continue;
    }
    const ns = Number(obj.observed_at_ns ?? 0);
    if (opts.from_ns !== undefined && ns < opts.from_ns) continue;
    if (opts.to_ns !== undefined && ns > opts.to_ns) continue;
    if (opts.action && String(obj.action) !== opts.action) continue;
    lines.push(obj);
    if (opts.limit !== undefined && lines.length >= opts.limit) {
      truncated = true;
      break;
    }
  }
  return { path, lines, truncated };
}

export function decisionToClosed(d: Record<string, unknown>): ClosedTrade | null {
  const action = String(d.action ?? "");
  const pnl = d.realized_pnl_micros;
  if (typeof pnl !== "number") return null;
  const isClose = CLOSE_ACTIONS.has(action) || d.outcome === "win" || d.outcome === "loss";
  if (!isClose && action !== "settle") return null;
  if (!CLOSE_ACTIONS.has(action) && d.outcome == null && action !== "settle") return null;
  return {
    condition_id: String(d.condition_id ?? ""),
    asset: String(d.asset ?? ""),
    window: String(d.window ?? ""),
    side: (d.side as string | null) ?? null,
    setup_id: (d.setup_id as string | null) ?? null,
    book: String(d.book ?? ""),
    action,
    outcome: (d.outcome as "win" | "loss" | null) ?? (pnl > 0 ? "win" : pnl < 0 ? "loss" : null),
    stake_micros: typeof d.stake_micros === "number" ? d.stake_micros : null,
    realized_pnl_micros: Math.trunc(pnl),
    hold_secs: typeof d.hold_secs === "number" ? d.hold_secs : null,
    effective_cost_micros: typeof d.effective_cost_micros === "number" ? d.effective_cost_micros : null,
    ask_micros: typeof d.ask_micros === "number" ? d.ask_micros : null,
    settlement_source: (d.settlement_source as string | null) ?? null,
    reason: String(d.reason ?? ""),
    observed_at_ns: Number(d.observed_at_ns ?? 0),
    stake_tier: (d.stake_tier as string | null) ?? null,
    risk_bps: typeof d.risk_bps === "number" ? d.risk_bps : null,
  };
}

export async function loadDayCloses(day = todayUtc()): Promise<ClosedTrade[]> {
  const { lines } = await readJournalLines(day);
  const out: ClosedTrade[] = [];
  for (const line of lines) {
    const c = decisionToClosed(line);
    if (c && (c.outcome === "win" || c.outcome === "loss" || c.action === "settle" || c.action === "exit")) {
      // Prefer accounting closes with outcome
      if (c.outcome === "win" || c.outcome === "loss") out.push(c);
    }
  }
  return out;
}

export async function readJsonFile<T>(path: string): Promise<T | null> {
  try {
    const raw = await readFile(path, "utf8");
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}
