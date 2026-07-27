import { createReadStream } from "node:fs";
import { open, readFile, readdir, stat } from "node:fs/promises";
import { createInterface } from "node:readline";
import { join } from "node:path";

// Serves the multi-asset window paper bridge status written by
// `btc-window-paper` (Spec 090 / 091, ADR 0094 / 0095). The worker owns the
// book: stake, marks, realized results and attribution are read from its
// ledger, never recomputed here. Proposal-only; no live authority.
export const dynamic = "force-dynamic";

type StatusAction = {
  condition_id: string;
  asset: string;
  window: string;
  action: string;
  reason: string;
  book: string;
};

type BookTotals = {
  closed_trades: number;
  wins: number;
  losses: number;
  realized_pnl_micros: number;
  open_positions: number;
  open_stake_micros: number;
  unrealized_pnl_micros: number;
};

type StrategyStats = {
  setup_id: string;
  book: string;
  totals: BookTotals;
  win_rate_bps: number | null;
  total_pnl_micros: number;
  last_observed_at_ns: number;
};

type OpenPositionView = {
  condition_id: string;
  asset: string;
  window: string;
  side: string;
  setup_id: string;
  book: string;
  position_state: string;
  quantity_micros: number;
  entry_ask_micros: number;
  effective_cost_micros: number;
  bid_micros: number;
  stake_micros: number;
  mark_value_micros: number;
  unrealized_pnl_micros: number;
  realized_pnl_micros: number;
  total_pnl_micros: number;
  hold_secs: number;
  entry_observed_at_ns: number;
};

type LedgerSnapshot = {
  active_core: BookTotals;
  shadow_experiment: BookTotals;
  by_strategy: StrategyStats[];
  by_asset: [string, BookTotals][];
  by_window: [string, BookTotals][];
  open_positions: OpenPositionView[];
};

type BridgeStatus = {
  paper_only: boolean;
  real_money: boolean;
  strategy_version: string;
  live_trading_authority: boolean;
  paper_week_capital_authority: boolean;
  observed_at_ns: number;
  markets_seen: number;
  decisions_this_tick: number;
  open_positions: number;
  assets_evaluated: string[];
  settlements_this_tick: number;
  ledger: LedgerSnapshot;
  recovery?: RecoverySummary;
  settled_on_recovery?: number;
  journal_errors?: string[];
  capital?: CapitalState;
  tradable_bankroll_micros?: number;
  net_realized_micros?: number;
  withdraw_target_micros?: number;
  withdraw_progress_bps?: number;
  last_actions: StatusAction[];
  reject_reasons: string[];
};

/** Shared $1000 paper capital and 50% profit withdraw checkpoints (Spec 093). */
type CapitalState = {
  principal_micros: number;
  profit_withdraw_bps: number;
  withdrawn_micros: number;
  withdraw_count: number;
  last_withdraw_micros: number;
  last_withdraw_at_ns: number;
};

/** What boot recovery rebuilt from the journal and checkpoint (Spec 092). */
type RecoverySummary = {
  journal_path: string;
  journal_records_read: number;
  closes_replayed: number;
  pre_accounting_closes_skipped: number;
  positions_restored: number;
  checkpoint_present: boolean;
};

type DecisionRecord = {
  condition_id: string;
  asset: string;
  window: "5m" | "15m" | "1h";
  elapsed_secs: number;
  observed_at_ns: number;
  action: string;
  reason: string;
  rejection_reasons: string[];
  setup_id: string | null;
  side: "up" | "down" | null;
  ask_micros: number | null;
  bid_micros: number | null;
  edge_micros: number | null;
  effective_cost_micros: number | null;
  quantity_micros: number | null;
  stake_micros: number | null;
  mark_value_micros: number | null;
  unrealized_pnl_micros: number | null;
  realized_pnl_micros: number | null;
  hold_secs: number | null;
  outcome: "win" | "loss" | null;
  settlement_source: string | null;
  position_state: string;
  book: string;
};

type WindowMetrics = {
  key: string;
  asset: string;
  window: "5m" | "15m" | "1h";
  mode: "CORE CANDIDATE" | "SHADOW / EXPERIMENTAL";
  decisions: number;
  entries: number;
  exits: number;
  settlements: number;
  wins: number;
  losses: number;
  no_trade: number;
  realized_pnl_micros: number;
  open_positions: number;
  open_stake_micros: number;
  unrealized_pnl_micros: number;
  last_action: DecisionRecord | null;
  latest_signal: DecisionRecord | null;
  top_reject_reasons: [string, number][];
};

/** One closed paper trade, for the blotter and expectancy maths. */
type ClosedTradeView = {
  condition_id: string;
  asset: string;
  window: "5m" | "15m" | "1h";
  side: "up" | "down" | null;
  setup_id: string | null;
  book: string;
  action: string;
  outcome: "win" | "loss" | null;
  stake_micros: number | null;
  realized_pnl_micros: number;
  hold_secs: number | null;
  effective_cost_micros: number | null;
  settlement_source: string | null;
  reason: string;
  observed_at_ns: number;
};

type BookDiagnostics = {
  /** Always "day" — computed from every accounting close in today's journal. */
  scope: "day";
  sample_size: number;
  avg_win_micros: number | null;
  avg_loss_micros: number | null;
  expectancy_micros: number | null;
  profit_factor_bps: number | null;
  avg_hold_secs: number | null;
  best_trade_micros: number | null;
  worst_trade_micros: number | null;
  avg_stake_micros: number | null;
  roi_on_stake_bps: number | null;
};

const WINDOWS = ["5m", "15m", "1h"] as const;
// `DecisionAction` serializes snake_case; the status file's `last_actions`
// uses Rust debug formatting. Journal records are the snake_case form.
const ENTRY_ACTIONS = new Set(["open", "open_reversed"]);
const EMPTY_TOTALS: BookTotals = {
  closed_trades: 0,
  wins: 0,
  losses: 0,
  realized_pnl_micros: 0,
  open_positions: 0,
  open_stake_micros: 0,
  unrealized_pnl_micros: 0,
};

function isBookTotals(value: unknown): value is BookTotals {
  if (!value || typeof value !== "object") return false;
  const t = value as Partial<BookTotals>;
  return typeof t.closed_trades === "number"
    && typeof t.wins === "number"
    && typeof t.losses === "number"
    && typeof t.realized_pnl_micros === "number"
    && typeof t.open_stake_micros === "number"
    && typeof t.unrealized_pnl_micros === "number";
}

function isLedger(value: unknown): value is LedgerSnapshot {
  if (!value || typeof value !== "object") return false;
  const l = value as Partial<LedgerSnapshot>;
  return isBookTotals(l.active_core)
    && isBookTotals(l.shadow_experiment)
    && Array.isArray(l.by_strategy)
    && Array.isArray(l.by_asset)
    && Array.isArray(l.by_window)
    && Array.isArray(l.open_positions);
}

function isBridgeStatus(value: unknown): value is BridgeStatus {
  if (!value || typeof value !== "object") return false;
  const s = value as Partial<BridgeStatus>;
  return s.paper_only === true
    && s.real_money === false
    && typeof s.strategy_version === "string"
    && s.live_trading_authority === false
    && s.paper_week_capital_authority === false
    && typeof s.observed_at_ns === "number"
    && typeof s.markets_seen === "number"
    && typeof s.decisions_this_tick === "number"
    && typeof s.open_positions === "number"
    && Array.isArray(s.assets_evaluated)
    && isLedger(s.ledger)
    && Array.isArray(s.last_actions)
    && Array.isArray(s.reject_reasons);
}

function isDecisionRecord(value: unknown): value is DecisionRecord {
  if (!value || typeof value !== "object") return false;
  const record = value as Partial<DecisionRecord>;
  return typeof record.condition_id === "string"
    && (record.window === "5m" || record.window === "15m" || record.window === "1h")
    && typeof record.elapsed_secs === "number"
    && typeof record.observed_at_ns === "number"
    && typeof record.action === "string"
    && typeof record.reason === "string"
    && Array.isArray(record.rejection_reasons)
    && typeof record.position_state === "string"
    && typeof record.book === "string";
}

type JournalSlice = {
  decisions: DecisionRecord[];
  /** Every accounting close in today's journal, newest first. */
  closed: ClosedTradeView[];
  /** Entry counts per asset-window for the whole day. */
  entry_counts: Record<string, number>;
  journal_day: string | null;
};

type JournalCacheEntry = {
  path: string;
  atMs: number;
  value: JournalSlice;
  inflight: Promise<JournalSlice> | null;
};

/** Avoid re-parsing a 100MB+ day journal on every Positions poll. */
const JOURNAL_CACHE_TTL_MS = 5_000;
const TAPE_TAIL_BYTES = 2_000_000;
const TAPE_DECISIONS = 200;
let journalCache: JournalCacheEntry | null = null;

async function readTailUtf8(path: string, maxBytes: number): Promise<string> {
  const handle = await open(path, "r");
  try {
    const meta = await handle.stat();
    const size = meta.size;
    if (size <= 0) return "";
    const start = size > maxBytes ? size - maxBytes : 0;
    const length = size - start;
    const buf = Buffer.alloc(length);
    await handle.read(buf, 0, length, start);
    let text = buf.toString("utf8");
    // Drop a partial first line when we seek mid-file.
    if (start > 0) {
      const nl = text.indexOf("\n");
      if (nl >= 0) text = text.slice(nl + 1);
    }
    return text;
  } finally {
    await handle.close();
  }
}

async function scanJournalFile(path: string, journal_day: string): Promise<JournalSlice> {
  const parseLine = (line: string): DecisionRecord | null => {
    try {
      const parsed = JSON.parse(line) as unknown;
      if (!isDecisionRecord(parsed)) return null;
      return { ...parsed, asset: parsed.asset ?? "BTC" };
    } catch {
      return null;
    }
  };

  const tapeRaw = await readTailUtf8(path, TAPE_TAIL_BYTES);
  const decisions = tapeRaw
    .split("\n")
    .filter(Boolean)
    .flatMap((line) => {
      const record = parseLine(line);
      return record ? [record] : [];
    })
    .slice(-TAPE_DECISIONS)
    .reverse();

  const closed: ClosedTradeView[] = [];
  const entry_counts: Record<string, number> = {};
  const stream = createReadStream(path, { encoding: "utf8" });
  const rl = createInterface({ input: stream, crlfDelay: Infinity });
  try {
    for await (const line of rl) {
      if (!line) continue;
      const isClose = line.includes("\"action\":\"exit\"") || line.includes("\"action\":\"settle\"");
      const isOpen = line.includes("\"action\":\"open\"") || line.includes("\"action\":\"open_reversed\"");
      if (!isClose && !isOpen) continue;
      const record = parseLine(line);
      if (!record) continue;
      if (isOpen && ENTRY_ACTIONS.has(record.action)) {
        const key = `${record.asset}-${record.window}`;
        entry_counts[key] = (entry_counts[key] ?? 0) + 1;
      }
      const trade = closedFromDecision(record);
      if (trade) closed.push(trade);
    }
  } finally {
    rl.close();
    stream.destroy();
  }
  closed.sort((a, b) => b.observed_at_ns - a.observed_at_ns);

  return { decisions, closed, entry_counts, journal_day };
}

async function readJournalSlice(directory: string): Promise<JournalSlice> {
  let names: string[];
  try {
    names = await readdir(directory);
  } catch {
    return { decisions: [], closed: [], entry_counts: {}, journal_day: null };
  }
  const latest = names
    .filter((name) => /^decisions-\d{4}-\d{2}-\d{2}\.jsonl$/.test(name))
    .sort()
    .at(-1);
  if (!latest) return { decisions: [], closed: [], entry_counts: {}, journal_day: null };

  const journal_day = latest.slice("decisions-".length, -".jsonl".length);
  const path = join(directory, latest);
  const now = Date.now();
  if (
    journalCache
    && journalCache.path === path
    && journalCache.value.closed.length + journalCache.value.decisions.length > 0
    && now - journalCache.atMs < JOURNAL_CACHE_TTL_MS
  ) {
    return journalCache.value;
  }
  if (journalCache && journalCache.path === path && journalCache.inflight) {
    return journalCache.inflight;
  }

  const inflight = scanJournalFile(path, journal_day)
    .then((value) => {
      journalCache = { path, atMs: Date.now(), value, inflight: null };
      return value;
    })
    .catch((err: unknown) => {
      if (journalCache?.path === path) journalCache.inflight = null;
      throw err;
    });
  journalCache = {
    path,
    atMs: journalCache?.path === path ? journalCache.atMs : 0,
    value: journalCache?.path === path
      ? journalCache.value
      : { decisions: [], closed: [], entry_counts: {}, journal_day },
    inflight,
  };
  return inflight;
}

function emptyMetrics(asset: string, window: "5m" | "15m" | "1h"): WindowMetrics {
  return {
    key: `${asset}-${window}`,
    asset,
    window,
    mode: window === "1h" ? "SHADOW / EXPERIMENTAL" : "CORE CANDIDATE",
    decisions: 0,
    entries: 0,
    exits: 0,
    settlements: 0,
    wins: 0,
    losses: 0,
    no_trade: 0,
    realized_pnl_micros: 0,
    open_positions: 0,
    open_stake_micros: 0,
    unrealized_pnl_micros: 0,
    last_action: null,
    latest_signal: null,
    top_reject_reasons: [],
  };
}

function isCloseAction(action: string): boolean {
  return action === "exit" || action === "settle";
}

function closedFromDecision(decision: DecisionRecord): ClosedTradeView | null {
  if (!isCloseAction(decision.action)) return null;
  if (typeof decision.realized_pnl_micros !== "number") return null;
  return {
    condition_id: decision.condition_id,
    asset: decision.asset,
    window: decision.window,
    side: decision.side,
    setup_id: decision.setup_id,
    book: decision.book,
    action: decision.action,
    outcome: decision.outcome,
    stake_micros: decision.stake_micros,
    realized_pnl_micros: decision.realized_pnl_micros,
    hold_secs: decision.hold_secs,
    effective_cost_micros: decision.effective_cost_micros,
    settlement_source: decision.settlement_source,
    reason: decision.reason,
    observed_at_ns: decision.observed_at_ns,
  };
}

/// Expectancy and profit-factor from every accounting close in today's journal.
function diagnosticsFromCloses(closes: ClosedTradeView[]): BookDiagnostics {
  if (closes.length === 0) {
    return {
      scope: "day",
      sample_size: 0,
      avg_win_micros: null,
      avg_loss_micros: null,
      expectancy_micros: null,
      profit_factor_bps: null,
      avg_hold_secs: null,
      best_trade_micros: null,
      worst_trade_micros: null,
      avg_stake_micros: null,
      roi_on_stake_bps: null,
    };
  }
  const wins = closes.filter((c) => c.realized_pnl_micros > 0);
  const losses = closes.filter((c) => c.realized_pnl_micros < 0);
  const winSum = wins.reduce((s, c) => s + c.realized_pnl_micros, 0);
  const lossSum = losses.reduce((s, c) => s + c.realized_pnl_micros, 0);
  const holdSamples = closes.filter((c) => typeof c.hold_secs === "number") as Array<ClosedTradeView & { hold_secs: number }>;
  const stakeSamples = closes.filter((c) => typeof c.stake_micros === "number" && c.stake_micros > 0) as Array<ClosedTradeView & { stake_micros: number }>;
  const realized = closes.reduce((s, c) => s + c.realized_pnl_micros, 0);
  const stake = stakeSamples.reduce((s, c) => s + c.stake_micros, 0);
  const grossWins = winSum;
  const grossLosses = Math.abs(lossSum);
  return {
    scope: "day",
    sample_size: closes.length,
    avg_win_micros: wins.length ? Math.round(winSum / wins.length) : null,
    avg_loss_micros: losses.length ? Math.round(lossSum / losses.length) : null,
    expectancy_micros: Math.round(realized / closes.length),
    profit_factor_bps: grossLosses === 0
      ? (grossWins > 0 ? 999_900 : null)
      : Math.round((grossWins * 10_000) / grossLosses),
    avg_hold_secs: holdSamples.length
      ? Math.round(holdSamples.reduce((s, c) => s + c.hold_secs, 0) / holdSamples.length)
      : null,
    best_trade_micros: closes.reduce((best, c) => Math.max(best, c.realized_pnl_micros), Number.NEGATIVE_INFINITY),
    worst_trade_micros: closes.reduce((worst, c) => Math.min(worst, c.realized_pnl_micros), Number.POSITIVE_INFINITY),
    avg_stake_micros: stakeSamples.length ? Math.round(stake / stakeSamples.length) : null,
    roi_on_stake_bps: stake > 0 ? Math.round((realized * 10_000) / stake) : null,
  };
}

/// Roll the tape up per asset/window and count why entries were refused.
///
/// Realized results and open risk come from the worker's ledger, not the tape:
/// the tape is a bounded window and older closes scroll out of it.
function analytics(
  decisions: DecisionRecord[],
  closed: ClosedTradeView[],
  entryCounts: Record<string, number>,
  ledger: LedgerSnapshot,
  journalDay: string | null,
) {
  const metrics = new Map<string, WindowMetrics>();
  const rejects = new Map<string, Map<string, number>>();

  for (const [key, totals] of ledger.by_window ?? []) {
    const dash = key.indexOf("-");
    const asset = key.slice(0, dash);
    const window = key.slice(dash + 1);
    if (window !== "5m" && window !== "15m" && window !== "1h") continue;
    const metric = emptyMetrics(asset, window);
    metric.realized_pnl_micros = totals.realized_pnl_micros;
    metric.wins = totals.wins;
    metric.losses = totals.losses;
    metric.open_positions = totals.open_positions;
    metric.open_stake_micros = totals.open_stake_micros;
    metric.unrealized_pnl_micros = totals.unrealized_pnl_micros;
    metrics.set(key, metric);
  }

  for (const decision of [...decisions].reverse()) {
    const key = `${decision.asset}-${decision.window}`;
    const metric = metrics.get(key) ?? emptyMetrics(decision.asset, decision.window);
    metric.decisions += 1;
    metric.last_action = decision;

    if (ENTRY_ACTIONS.has(decision.action)) {
      metric.latest_signal = decision;
    } else if (decision.action === "no_trade") {
      metric.no_trade += 1;
      const counts = rejects.get(key) ?? new Map<string, number>();
      counts.set(decision.reason, (counts.get(decision.reason) ?? 0) + 1);
      rejects.set(key, counts);
    }
    metrics.set(key, metric);
  }

  // Entry/exit/settle counts come from the wider blotter scan so hold/locked
  // spam in the short tape cannot zero out the window cards.
  for (const [key, count] of Object.entries(entryCounts)) {
    const dash = key.indexOf("-");
    const asset = key.slice(0, dash);
    const window = key.slice(dash + 1);
    if (window !== "5m" && window !== "15m" && window !== "1h") continue;
    const metric = metrics.get(key) ?? emptyMetrics(asset, window);
    metric.entries = count;
    metrics.set(key, metric);
  }
  for (const trade of closed) {
    const key = `${trade.asset}-${trade.window}`;
    const metric = metrics.get(key) ?? emptyMetrics(trade.asset, trade.window);
    if (trade.action === "exit") metric.exits += 1;
    if (trade.action === "settle") metric.settlements += 1;
    metrics.set(key, metric);
  }

  for (const position of ledger.open_positions) {
    const key = `${position.asset}-${position.window}`;
    if (!metrics.has(key)) {
      metrics.set(key, emptyMetrics(position.asset, position.window as "5m" | "15m" | "1h"));
    }
  }

  for (const [key, counts] of rejects) {
    const metric = metrics.get(key);
    if (!metric) continue;
    metric.top_reject_reasons = [...counts.entries()]
      .sort((left, right) => right[1] - left[1])
      .slice(0, 3);
  }

  // Blotter UI shows the newest closes; diagnostics and E/X/S use the full day.
  const recentClosed = closed.slice(0, 100);
  const assets = [...new Set([...metrics.values()].map((m) => m.asset))].sort();
  const ordered = assets.flatMap((asset) =>
    WINDOWS.map((window) => metrics.get(`${asset}-${window}`) ?? emptyMetrics(asset, window)));

  return {
    assets,
    windows: ordered,
    recent_decisions: decisions.slice(0, 200),
    recent_closed: recentClosed,
    day_closes: closed.length,
    journal_day: journalDay,
    diagnostics: diagnosticsFromCloses(closed),
    journal_decisions_loaded: decisions.length,
  };
}

/// Combined book across ACTIVE_CORE and SHADOW, for the portfolio ribbon only.
/// The per-book totals stay separate everywhere a decision would be made.
function portfolio(ledger: LedgerSnapshot) {
  const core = ledger.active_core ?? EMPTY_TOTALS;
  const shadow = ledger.shadow_experiment ?? EMPTY_TOTALS;
  const closed = core.closed_trades + shadow.closed_trades;
  const wins = core.wins + shadow.wins;
  const realized = core.realized_pnl_micros + shadow.realized_pnl_micros;
  const unrealized = core.unrealized_pnl_micros + shadow.unrealized_pnl_micros;
  return {
    closed_trades: closed,
    wins,
    losses: core.losses + shadow.losses,
    win_rate_bps: closed === 0 ? null : Math.round((wins * 10_000) / closed),
    realized_pnl_micros: realized,
    unrealized_pnl_micros: unrealized,
    total_pnl_micros: realized + unrealized,
    open_positions: core.open_positions + shadow.open_positions,
    open_stake_micros: core.open_stake_micros + shadow.open_stake_micros,
  };
}

export async function GET() {
  const path = process.env.POLY_BTC_WINDOW_PAPER_STATUS_PATH
    ?? join(process.cwd(), "..", "var", "btc-window-paper", "status.json");
  try {
    const [raw, meta] = await Promise.all([readFile(path, "utf8"), stat(path)]);
    const status = JSON.parse(raw) as unknown;
    if (!isBridgeStatus(status)) {
      return Response.json({
        available: false,
        reason: "window paper status contract invalid — rebuild and restart btc-window-paper",
      }, { status: 409 });
    }
    const ageMs = Date.now() - meta.mtimeMs;
    const live = ageMs <= 15_000;
    const journal = await readJournalSlice(join(path, ".."));
    // A durability failure halts the worker, so it must outrank staleness in the
    // banner: the operator needs to know the journal is incomplete, not just old.
    const halted = (status.journal_errors ?? []).length > 0;
    return Response.json({
      available: true,
      live,
      halted,
      status_age_ms: Math.max(0, Math.round(ageMs)),
      strategy_status: halted
        ? "HALTED_JOURNAL_NOT_DURABLE"
        : live ? "ACTIVE_PAPER_BRIDGE" : "STALE_OR_STOPPED",
      strategy_reason: halted
        ? `Journal write failed, so the worker halted rather than trade against an incomplete record: ${(status.journal_errors ?? []).join("; ")}`
        : live
          ? `${status.strategy_version} is evaluating ${status.assets_evaluated.join(" / ") || "no"} 5m/15m/1h observation feeds in paper mode. No live trading or Paper Week capital authority.`
          : "Status file is older than 15s — start or restart btc-window-paper / the managed stack.",
      recovery: status.recovery ?? null,
      settled_on_recovery: status.settled_on_recovery ?? 0,
      capital: status.capital ?? null,
      tradable_bankroll_micros: status.tradable_bankroll_micros ?? null,
      net_realized_micros: status.net_realized_micros ?? null,
      withdraw_target_micros: status.withdraw_target_micros ?? null,
      withdraw_progress_bps: status.withdraw_progress_bps ?? null,
      status,
      portfolio: portfolio(status.ledger),
      ledger: status.ledger,
      analytics: analytics(
        journal.decisions,
        journal.closed,
        journal.entry_counts,
        status.ledger,
        journal.journal_day,
      ),
    });
  } catch {
    return Response.json({
      available: false,
      live: false,
      reason: "btc-window-paper not running — start managed stack or: cargo run -p btc-window-strategy --bin btc-window-paper",
    });
  }
}
