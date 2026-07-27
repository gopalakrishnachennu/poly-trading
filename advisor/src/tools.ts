import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { createReadStream } from "node:fs";
import { readdir, readFile, stat, writeFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { errorResult, fetchLivePayload, textResult } from "./api.js";
import {
  findDeathPatterns,
  rankKillKeep,
  rejectReasonBreakdown,
  expectedRiskBps,
  stakeTierFromPerf,
  summarizeCloses,
} from "./analytics.js";
import { loadDayCloses, readJournalLines, readJsonFile } from "./journal.js";
import { bpsToPctText, microsToUsdText } from "./money.js";
import {
  advisorRoot,
  ensureAdvisorDirs,
  journalPathForDay,
  paperDir,
  paths,
  statusFile,
  todayUtc,
} from "./paths.js";
import { ADVISOR_SYSTEM_PROMPT, ALLOWED_ACTIONS, CURRENT_POLICY } from "./policy.js";
import {
  dirSizeBytes,
  exportReport,
  getPattern,
  getSuggestion,
  listPatterns,
  listSuggestions,
  savePattern,
  saveSuggestion,
  searchPatterns,
  setPatternStatus,
  updateSuggestion,
} from "./store.js";
import type { ClosedTrade, StrategyRow, SuggestionAction } from "./types.js";
import { draftSpecDelta, requireCitations, validateProposal } from "./validate.js";
import {
  edgeVsSizeReport,
  whatIfCapRiskBps,
  whatIfDisableSetups,
  whatIfMinSample,
} from "./whatif.js";
import { writeSnapshotOnce } from "./features.js";

async function liveOrThrow() {
  const r = await fetchLivePayload();
  if (!r.payload) throw new Error(r.error ?? "no live payload");
  return r;
}

function closesFromPayload(payload: Awaited<ReturnType<typeof liveOrThrow>>["payload"]): ClosedTrade[] {
  const a = payload?.analytics;
  return (a?.day_closes ?? a?.recent_closed ?? []) as ClosedTrade[];
}

export function registerAllTools(server: McpServer): void {
  // ─── Module A: Data ───────────────────────────────────────────────
  server.registerTool("get_status", {
    description: "Live paper worker status: live/halted, version, assets, age",
    inputSchema: {},
  }, async () => {
    const r = await liveOrThrow();
    const s = r.payload!;
    return textResult({
      source: r.source,
      live: s.live,
      available: s.available,
      halted: s.halted,
      status_age_ms: s.status_age_ms,
      strategy_version: (s.status as { strategy_version?: string } | null)?.strategy_version
        ?? (s as { strategy_version?: string }).strategy_version,
      paper_only: true,
      real_money: false,
      live_trading_authority: false,
      assets_evaluated: (s.status as { assets_evaluated?: string[] } | null)?.assets_evaluated,
      open_positions: s.ledger?.open_positions?.length ?? s.status?.open_positions,
      markets_seen: (s.status as { markets_seen?: number } | null)?.markets_seen,
      error: r.error,
    });
  });

  server.registerTool("get_capital", {
    description: "Principal, tradable bankroll, withdrawn, withdraw progress (Spec 093)",
    inputSchema: {},
  }, async () => {
    const { payload, source } = await liveOrThrow();
    const p = payload!;
    return textResult({
      source,
      capital: p.capital,
      tradable_bankroll_micros: p.tradable_bankroll_micros,
      tradable_bankroll_usd: microsToUsdText(p.tradable_bankroll_micros as number),
      net_realized_micros: p.net_realized_micros,
      net_realized_usd: microsToUsdText(p.net_realized_micros as number),
      withdraw_target_micros: p.withdraw_target_micros,
      withdraw_progress_bps: p.withdraw_progress_bps,
      withdraw_progress: bpsToPctText(p.withdraw_progress_bps as number),
    });
  });

  server.registerTool("get_health", {
    description: "Recovery, journal errors, halt signals",
    inputSchema: {},
  }, async () => {
    const { payload, source, error } = await liveOrThrow();
    const p = payload!;
    const status = p.status as {
      journal_errors?: string[];
      recovery?: unknown;
      settled_on_recovery?: number;
    } | null;
    return textResult({
      source,
      halted: p.halted,
      journal_errors: status?.journal_errors ?? p.status?.journal_errors ?? [],
      recovery: p.recovery ?? status?.recovery,
      settled_on_recovery: p.settled_on_recovery ?? status?.settled_on_recovery,
      api_warning: error,
      status_file: statusFile(),
      paper_dir: paperDir(),
      advisor_root: advisorRoot(),
    });
  });

  server.registerTool("get_open_book", {
    description: "Open positions with stake/mark/unrealized",
    inputSchema: {
      asset: z.string().optional(),
      window: z.string().optional(),
    },
  }, async ({ asset, window }) => {
    const { payload } = await liveOrThrow();
    let rows = payload!.ledger?.open_positions ?? [];
    if (asset) rows = rows.filter((r) => String(r.asset) === asset);
    if (window) rows = rows.filter((r) => String(r.window) === window);
    return textResult({ count: rows.length, open_positions: rows });
  });

  server.registerTool("get_setup_scoreboard", {
    description: "Ranked setups by expectancy / PF / WR",
    inputSchema: {
      book: z.string().optional(),
      min_closes: z.number().int().nonnegative().optional(),
      sort: z.enum(["expectancy", "pf", "wr", "pnl"]).optional(),
    },
  }, async ({ book, min_closes = 0, sort = "expectancy" }) => {
    const { payload } = await liveOrThrow();
    let rows = [...(payload!.ledger?.by_strategy ?? [])] as StrategyRow[];
    if (book) rows = rows.filter((r) => String(r.book).toUpperCase().includes(book.toUpperCase()));
    rows = rows.filter((r) => r.totals.closed_trades >= min_closes);
    rows.sort((a, b) => {
      if (sort === "wr") return (b.win_rate_bps ?? 0) - (a.win_rate_bps ?? 0);
      if (sort === "pf") return (b.quality?.profit_factor_bps ?? 0) - (a.quality?.profit_factor_bps ?? 0);
      if (sort === "pnl") return b.totals.realized_pnl_micros - a.totals.realized_pnl_micros;
      return (b.quality?.expectancy_micros ?? 0) - (a.quality?.expectancy_micros ?? 0);
    });
    return textResult({ sort, min_closes, count: rows.length, by_strategy: rows });
  });

  server.registerTool("get_setup_detail", {
    description: "Full attribution for one setup_id plus recent closes",
    inputSchema: { setup_id: z.string() },
  }, async ({ setup_id }) => {
    const { payload } = await liveOrThrow();
    const row = (payload!.ledger?.by_strategy ?? []).find((r) => r.setup_id === setup_id);
    const closes = closesFromPayload(payload).filter((c) => c.setup_id === setup_id);
    return textResult({
      setup: row ?? null,
      closes_summary: summarizeCloses(closes),
      recent_closes: closes.slice(-30),
    });
  });

  server.registerTool("get_rolls", {
    description: "Totals by window and by asset",
    inputSchema: {},
  }, async () => {
    const { payload } = await liveOrThrow();
    return textResult({
      by_window: payload!.ledger?.by_window ?? [],
      by_asset: payload!.ledger?.by_asset ?? [],
      active_core: payload!.ledger?.active_core,
      shadow_experiment: payload!.ledger?.shadow_experiment,
    });
  });

  server.registerTool("get_day_diagnostics", {
    description: "Day expectancy, avg win/loss, PF, ROI on stake",
    inputSchema: { date: z.string().optional() },
  }, async ({ date }) => {
    const day = date ?? todayUtc();
    if (!date) {
      const { payload } = await liveOrThrow();
      const diag = payload!.analytics?.diagnostics;
      if (diag) return textResult({ date: day, diagnostics: diag, source: "api" });
    }
    const closes = await loadDayCloses(day);
    return textResult({ date: day, diagnostics: summarizeCloses(closes), source: "journal" });
  });

  server.registerTool("get_recent_closes", {
    description: "Closed blotter rows",
    inputSchema: {
      limit: z.number().int().positive().max(500).optional(),
      setup_id: z.string().optional(),
      date: z.string().optional(),
    },
  }, async ({ limit = 50, setup_id, date }) => {
    let closes: ClosedTrade[];
    if (date) closes = await loadDayCloses(date);
    else {
      const { payload } = await liveOrThrow();
      closes = closesFromPayload(payload);
      if (!closes.length) closes = await loadDayCloses(todayUtc());
    }
    if (setup_id) closes = closes.filter((c) => c.setup_id === setup_id);
    return textResult({ count: closes.length, closes: closes.slice(-limit).reverse() });
  });

  server.registerTool("get_recent_decisions", {
    description: "Opens/exits/rejects with stake_tier and reasons",
    inputSchema: {
      limit: z.number().int().positive().max(500).optional(),
      action: z.string().optional(),
    },
  }, async ({ limit = 80, action }) => {
    const { payload } = await liveOrThrow();
    let rows = [...(payload!.analytics?.recent_decisions ?? [])];
    if (!rows.length) {
      const j = await readJournalLines(todayUtc(), { limit: 2000 });
      rows = j.lines;
    }
    if (action) rows = rows.filter((r) => String(r.action) === action);
    return textResult({ count: rows.length, decisions: rows.slice(0, limit) });
  });

  server.registerTool("get_live_tape", {
    description: "Last N decisions from local 1s tape buffer",
    inputSchema: { limit: z.number().int().positive().max(500).optional() },
  }, async ({ limit = 50 }) => {
    await ensureAdvisorDirs();
    const rows: unknown[] = [];
    try {
      const rl = createInterface({
        input: createReadStream(paths.tapeLatest(), { encoding: "utf8" }),
        crlfDelay: Infinity,
      });
      const buf: string[] = [];
      for await (const line of rl) {
        if (line.trim()) buf.push(line);
      }
      for (const line of buf.slice(-limit)) {
        try {
          rows.push(JSON.parse(line));
        } catch {
          /* skip */
        }
      }
    } catch {
      return textResult({
        count: 0,
        decisions: [],
        note: "tape empty — start snapshot daemon: npm run snapshot",
      });
    }
    return textResult({ count: rows.length, decisions: rows.reverse() });
  });

  server.registerTool("get_decisions_since", {
    description: "Journal decisions since from_ns (advisor delta)",
    inputSchema: {
      from_ns: z.number(),
      date: z.string().optional(),
      limit: z.number().int().positive().optional(),
    },
  }, async ({ from_ns, date, limit = 500 }) => {
    const j = await readJournalLines(date ?? todayUtc(), { from_ns, limit });
    return textResult(j);
  });

  server.registerTool("get_journal_slice", {
    description: "Raw journal window for a UTC day",
    inputSchema: {
      date: z.string(),
      from_ns: z.number().optional(),
      to_ns: z.number().optional(),
      limit: z.number().int().positive().max(5000).optional(),
    },
  }, async ({ date, from_ns, to_ns, limit = 200 }) => {
    return textResult(await readJournalLines(date, { from_ns, to_ns, limit }));
  });

  server.registerTool("get_day_archive", {
    description: "Stored day summary archive if present",
    inputSchema: { date: z.string() },
  }, async ({ date }) => {
    const path = join(paths.archivesDir(), `${date}.json`);
    const data = await readJsonFile(path);
    return data ? textResult({ path, archive: data }) : errorResult(`no archive for ${date}`);
  });

  server.registerTool("list_snapshots", {
    description: "Index of 1s snapshots for a day",
    inputSchema: {
      date: z.string().optional(),
      limit: z.number().int().positive().optional(),
    },
  }, async ({ date, limit = 100 }) => {
    const day = date ?? todayUtc();
    const dir = paths.snapshots(day);
    try {
      const files = (await readdir(dir)).filter((f) => f.endsWith(".json")).sort().reverse();
      return textResult({ day, count: files.length, files: files.slice(0, limit) });
    } catch {
      return textResult({ day, count: 0, files: [], note: "no snapshots yet" });
    }
  });

  server.registerTool("get_snapshot", {
    description: "Load one stored snapshot by filename or path",
    inputSchema: {
      date: z.string().optional(),
      file: z.string(),
    },
  }, async ({ date, file }) => {
    const path = file.includes("/") ? file : join(paths.snapshots(date ?? todayUtc()), file);
    const data = await readJsonFile(path);
    return data ? textResult({ path, snapshot: data }) : errorResult(`missing ${path}`);
  });

  server.registerTool("get_feature_snapshot", {
    description: "Latest LLM-sized feature rollup (preferred default feed)",
    inputSchema: {},
  }, async () => {
    const data = await readJsonFile(paths.featuresLatest());
    if (data) return textResult(data);
    const written = await writeSnapshotOnce();
    if (!written.ok) return errorResult(written.error ?? "feature snapshot unavailable");
    return textResult(await readJsonFile(paths.featuresLatest()));
  });

  // ─── Module B: Analytics ──────────────────────────────────────────
  server.registerTool("rank_kill_keep", {
    description: "Kill/demote/keep/promote lists from setup expectancy",
    inputSchema: { min_closes: z.number().int().nonnegative().optional() },
  }, async ({ min_closes = 5 }) => {
    const { payload } = await liveOrThrow();
    return textResult(rankKillKeep((payload!.ledger?.by_strategy ?? []) as StrategyRow[], min_closes));
  });

  server.registerTool("find_death_patterns", {
    description: "Losing clusters: setup×window×ask×hold×tier",
    inputSchema: { date: z.string().optional() },
  }, async ({ date }) => {
    let c: ClosedTrade[] = [];
    if (date) {
      c = await loadDayCloses(date);
    } else {
      const { payload } = await liveOrThrow();
      c = closesFromPayload(payload);
      if (!c.length) c = await loadDayCloses(todayUtc());
    }
    return textResult({ patterns: findDeathPatterns(c), sample: c.length });
  });

  server.registerTool("compare_days", {
    description: "Compare two UTC day archives / journals",
    inputSchema: { date_a: z.string(), date_b: z.string() },
  }, async ({ date_a, date_b }) => {
    const a = await loadDayCloses(date_a);
    const b = await loadDayCloses(date_b);
    return textResult({
      date_a,
      date_b,
      a: summarizeCloses(a),
      b: summarizeCloses(b),
      delta_realized_pnl_micros: summarizeCloses(b).realized_pnl_micros - summarizeCloses(a).realized_pnl_micros,
    });
  });

  server.registerTool("streak_report", {
    description: "Streaks from scoreboard",
    inputSchema: { setup_id: z.string().optional() },
  }, async ({ setup_id }) => {
    const { payload } = await liveOrThrow();
    let rows = (payload!.ledger?.by_strategy ?? []) as StrategyRow[];
    if (setup_id) rows = rows.filter((r) => r.setup_id === setup_id);
    return textResult(rows.map((r) => ({
      setup_id: r.setup_id,
      current_streak: r.totals.current_streak,
      best_win_streak: r.totals.best_win_streak,
      worst_loss_streak: r.totals.worst_loss_streak,
    })));
  });

  server.registerTool("exposure_report", {
    description: "Open stake by asset/window/setup vs caps",
    inputSchema: {},
  }, async () => {
    const { payload } = await liveOrThrow();
    const opens = payload!.ledger?.open_positions ?? [];
    const by: Record<string, number> = {};
    for (const o of opens) {
      const k = `${o.asset}|${o.window}|${o.setup_id}`;
      by[k] = (by[k] ?? 0) + Number(o.stake_micros ?? 0);
    }
    return textResult({
      open_count: opens.length,
      bankroll_micros: payload!.tradable_bankroll_micros,
      max_market_exposure_bps: CURRENT_POLICY.max_market_exposure_bps,
      by_key_stake_micros: by,
      opens,
    });
  });

  server.registerTool("stake_tier_audit", {
    description: "Check opens/closes against Spec 093 tier expectations",
    inputSchema: { date: z.string().optional() },
  }, async ({ date }) => {
    const { payload } = await liveOrThrow();
    const scoreboard = (payload!.ledger?.by_strategy ?? []) as StrategyRow[];
    const perf = new Map(scoreboard.map((s) => [s.setup_id, s]));
    const closes = date ? await loadDayCloses(date) : closesFromPayload(payload);
    const c = closes.length ? closes : await loadDayCloses(date ?? todayUtc());
    const mismatches: unknown[] = [];
    for (const row of c) {
      if (row.stake_tier == null && row.risk_bps == null) continue;
      const s = perf.get(row.setup_id ?? "");
      const closed = s?.totals.closed_trades ?? 0;
      const expected = stakeTierFromPerf(closed, s?.win_rate_bps ?? null);
      if (row.stake_tier && row.stake_tier !== expected) {
        // note: tier at open time may differ from end-of-day scoreboard
        mismatches.push({
          condition_id: row.condition_id,
          setup_id: row.setup_id,
          recorded_tier: row.stake_tier,
          end_of_day_implied_tier: expected,
          risk_bps: row.risk_bps,
        });
      }
    }
    return textResult({
      policy: CURRENT_POLICY.stake_tiers_bps,
      checked: c.length,
      mismatches_vs_eod_scoreboard: mismatches.slice(0, 50),
      note: "Tier at open can legitimately differ from end-of-day implied tier.",
    });
  });

  server.registerTool("reject_reason_breakdown", {
    description: "Why entries were blocked",
    inputSchema: { limit: z.number().int().positive().optional() },
  }, async ({ limit = 400 }) => {
    const { payload } = await liveOrThrow();
    let rows = (payload!.analytics?.recent_decisions ?? []) as Record<string, unknown>[];
    if (rows.length < 50) {
      rows = (await readJournalLines(todayUtc(), { limit })).lines;
    }
    return textResult({ reasons: rejectReasonBreakdown(rows.slice(0, limit)) });
  });

  server.registerTool("what_if_disable_setups", {
    description: "Replay day PnL if listed setups never closed/traded",
    inputSchema: {
      setup_ids: z.array(z.string()),
      date: z.string().optional(),
    },
  }, async ({ setup_ids, date }) => {
    const closes = await loadDayCloses(date ?? todayUtc());
    return textResult(whatIfDisableSetups(closes, setup_ids));
  });

  server.registerTool("what_if_cap_tier", {
    description: "Approximate replay with max risk bps cap",
    inputSchema: {
      max_bps: z.number().int().positive(),
      date: z.string().optional(),
    },
  }, async ({ max_bps, date }) => {
    const closes = await loadDayCloses(date ?? todayUtc());
    return textResult(whatIfCapRiskBps(closes, max_bps));
  });

  server.registerTool("what_if_min_sample", {
    description: "Replay skipping trades until setup had N prior closes",
    inputSchema: {
      min_closes: z.number().int().nonnegative(),
      date: z.string().optional(),
    },
  }, async ({ min_closes, date }) => {
    const closes = await loadDayCloses(date ?? todayUtc());
    return textResult(whatIfMinSample(closes, min_closes));
  });

  server.registerTool("edge_vs_size_report", {
    description: "Where big stakes hurt vs help",
    inputSchema: { date: z.string().optional() },
  }, async ({ date }) => {
    const closes = await loadDayCloses(date ?? todayUtc());
    return textResult(edgeVsSizeReport(closes));
  });

  // ─── Module C: Patterns ───────────────────────────────────────────
  server.registerTool("list_patterns", {
    description: "List pattern library records",
    inputSchema: { status: z.string().optional() },
  }, async ({ status }) => textResult(await listPatterns(status)));

  server.registerTool("get_pattern", {
    description: "Get one pattern by id",
    inputSchema: { pattern_id: z.string() },
  }, async ({ pattern_id }) => {
    const p = await getPattern(pattern_id);
    return p ? textResult(p) : errorResult("pattern not found");
  });

  server.registerTool("save_pattern", {
    description: "Create/update a local pattern (draft by default)",
    inputSchema: {
      id: z.string().optional(),
      title: z.string(),
      hypothesis: z.string(),
      rules: z.array(z.string()),
      evidence: z.string(),
      kill_criteria: z.string(),
      status: z.enum(["draft", "shadow", "active", "killed"]).optional(),
      linked_suggestion_id: z.string().optional(),
      notes: z.string().optional(),
    },
  }, async (args) => textResult(await savePattern(args)));

  server.registerTool("set_pattern_status", {
    description: "Transition pattern draft→shadow→active→killed",
    inputSchema: {
      pattern_id: z.string(),
      status: z.enum(["draft", "shadow", "active", "killed"]),
    },
  }, async ({ pattern_id, status }) => {
    const p = await setPatternStatus(pattern_id, status);
    return p ? textResult(p) : errorResult("pattern not found");
  });

  server.registerTool("search_patterns", {
    description: "Text search pattern library",
    inputSchema: { query: z.string() },
  }, async ({ query }) => textResult(await searchPatterns(query)));

  server.registerTool("pattern_performance", {
    description: "Heuristic performance note for a pattern (linked setup rules)",
    inputSchema: {
      pattern_id: z.string(),
      from_date: z.string().optional(),
    },
  }, async ({ pattern_id, from_date }) => {
    const p = await getPattern(pattern_id);
    if (!p) return errorResult("pattern not found");
    const closes = await loadDayCloses(from_date ?? todayUtc());
    const setupHints = p.rules
      .map((r) => {
        const m = r.match(/setup[_\s-]?id[:\s]+([a-z0-9_]+)/i);
        return m?.[1];
      })
      .filter(Boolean) as string[];
    const filtered = setupHints.length
      ? closes.filter((c) => setupHints.includes(c.setup_id ?? ""))
      : closes;
    return textResult({
      pattern: p,
      setup_hints: setupHints,
      day: from_date ?? todayUtc(),
      summary: summarizeCloses(filtered),
      note: "Tag setup ids in rules as 'setup_id: foo' for tighter attribution.",
    });
  });

  // ─── Module D: Suggestions ────────────────────────────────────────
  server.registerTool("save_suggestion", {
    description: "Persist one advisor run locally",
    inputSchema: {
      model: z.string(),
      verdict: z.string(),
      actions: z.array(z.object({
        action: z.string(),
        setup_id: z.string().optional(),
        window: z.string().optional(),
        asset: z.string().optional(),
        detail: z.string().optional(),
        evidence: z.string().optional(),
      })),
      raw: z.string().optional(),
      note: z.string().optional(),
    },
  }, async (args) => {
    const v = validateProposal({ actions: args.actions as SuggestionAction[] });
    const rec = await saveSuggestion(args);
    return textResult({ suggestion: rec, validation: v });
  });

  server.registerTool("list_suggestions", {
    description: "List saved suggestions",
    inputSchema: {
      limit: z.number().int().positive().optional(),
      status: z.string().optional(),
    },
  }, async ({ limit = 30, status }) => textResult(await listSuggestions(limit, status)));

  server.registerTool("get_suggestion", {
    description: "Get suggestion by id",
    inputSchema: { id: z.string() },
  }, async ({ id }) => {
    const s = await getSuggestion(id);
    return s ? textResult(s) : errorResult("not found");
  });

  server.registerTool("accept_suggestion", {
    description: "Mark suggestion accepted (ready for spec)",
    inputSchema: { id: z.string(), note: z.string().optional() },
  }, async ({ id, note }) => {
    const s = await updateSuggestion(id, { status: "accepted", note });
    return s ? textResult(s) : errorResult("not found");
  });

  server.registerTool("reject_suggestion", {
    description: "Reject suggestion with reason",
    inputSchema: { id: z.string(), reason: z.string() },
  }, async ({ id, reason }) => {
    const s = await updateSuggestion(id, { status: "rejected", reject_reason: reason });
    return s ? textResult(s) : errorResult("not found");
  });

  server.registerTool("suggestion_diff_summary", {
    description: "Human summary of proposed actions",
    inputSchema: { id: z.string() },
  }, async ({ id }) => {
    const s = await getSuggestion(id);
    if (!s) return errorResult("not found");
    return textResult({
      id: s.id,
      verdict: s.verdict,
      status: s.status,
      summary: s.actions.map((a, i) => `${i + 1}. ${a.action} ${a.setup_id ?? ""} — ${a.detail ?? a.evidence ?? ""}`),
    });
  });

  // ─── Module E: Specs ──────────────────────────────────────────────
  server.registerTool("get_current_policy", {
    description: "Canonical Spec 093 stake/withdraw policy",
    inputSchema: {},
  }, async () => textResult(CURRENT_POLICY));

  server.registerTool("draft_spec_delta", {
    description: "Draft markdown spec from suggestion or pattern",
    inputSchema: {
      suggestion_id: z.string().optional(),
      pattern_id: z.string().optional(),
    },
  }, async ({ suggestion_id, pattern_id }) => {
    let actions: SuggestionAction[] = [];
    let source = "";
    if (suggestion_id) {
      const s = await getSuggestion(suggestion_id);
      if (!s) return errorResult("suggestion not found");
      actions = s.actions;
      source = suggestion_id;
    } else if (pattern_id) {
      const p = await getPattern(pattern_id);
      if (!p) return errorResult("pattern not found");
      actions = [{ action: "TIGHTEN_ENTRY", detail: p.title, evidence: p.evidence }];
      source = pattern_id;
    } else {
      return errorResult("provide suggestion_id or pattern_id");
    }
    const md = draftSpecDelta(actions, source);
    const path = await exportReport(`spec-delta-${source}`, md, "md");
    return textResult({ path, markdown: md });
  });

  server.registerTool("validate_proposal_guards", {
    description: "Validate proposal against paper-only action menu",
    inputSchema: {
      actions: z.array(z.object({
        action: z.string(),
        setup_id: z.string().optional(),
        evidence: z.string().optional(),
        detail: z.string().optional(),
      })),
      raise_stake: z.boolean().optional(),
      live_trading: z.boolean().optional(),
    },
  }, async (args) => textResult(validateProposal(args)));

  server.registerTool("list_open_spec_work", {
    description: "Accepted suggestions not yet implemented",
    inputSchema: {},
  }, async () => textResult(await listSuggestions(100, "accepted")));

  server.registerTool("mark_implemented", {
    description: "Close loop after shipping a spec",
    inputSchema: {
      suggestion_id: z.string(),
      spec_id: z.string(),
      commit: z.string().optional(),
    },
  }, async ({ suggestion_id, spec_id, commit }) => {
    const s = await updateSuggestion(suggestion_id, {
      status: "implemented",
      spec_id,
      commit,
    });
    return s ? textResult(s) : errorResult("not found");
  });

  // ─── Module F: Ops ────────────────────────────────────────────────
  server.registerTool("ops_overview", {
    description: "One-shot ops: live, halt, bankroll, opens, day expectancy",
    inputSchema: {},
  }, async () => {
    const { payload, source, error } = await liveOrThrow();
    const p = payload!;
    const diag = p.analytics?.diagnostics as { expectancy_micros?: number } | undefined;
    return textResult({
      source,
      live: p.live,
      halted: p.halted,
      status_age_ms: p.status_age_ms,
      bankroll_usd: microsToUsdText(p.tradable_bankroll_micros as number),
      withdraw_progress: bpsToPctText(p.withdraw_progress_bps as number),
      open_positions: p.ledger?.open_positions?.length ?? 0,
      day_expectancy_micros: diag?.expectancy_micros ?? null,
      api_warning: error,
    });
  });

  server.registerTool("diagnose_halt", {
    description: "Explain halt / recovery / lock issues",
    inputSchema: {},
  }, async () => {
    const { payload } = await liveOrThrow();
    const errors = (payload!.status as { journal_errors?: string[] } | null)?.journal_errors
      ?? [];
    const lockPath = join(paperDir(), "writer.lock");
    let lock: unknown = null;
    try {
      lock = { path: lockPath, ...(JSON.parse(await readFile(lockPath, "utf8")) as object) };
    } catch {
      lock = { path: lockPath, present: false };
    }
    const steps = [
      "Confirm only one btc-window-paper worker (writer.lock).",
      "If journal_errors non-empty: do not delete journal; inspect last lines.",
      "Restart with scripts/detach.py after clearing stale lock only if PID is dead.",
      "Re-check get_health and ops_overview before advising strategy changes.",
    ];
    return textResult({
      halted: payload!.halted,
      journal_errors: errors,
      recovery: payload!.recovery,
      lock,
      safe_next_steps: steps,
    });
  });

  server.registerTool("disk_usage", {
    description: "Advisor + paper journal disk usage",
    inputSchema: {},
  }, async () => {
    await ensureAdvisorDirs();
    const bytes = {
      advisor_root: await dirSizeBytes(advisorRoot()),
      snapshots: await dirSizeBytes(join(advisorRoot(), "snapshots")),
      suggestions: await dirSizeBytes(paths.suggestionsDir()),
      patterns: await dirSizeBytes(paths.patternsDir()),
      paper_dir: await dirSizeBytes(paperDir()),
    };
    return textResult({
      bytes,
      advice: "Keep ~1h of 1s snapshots; archive daily summaries; journals are source of truth.",
    });
  });

  server.registerTool("writer_lock_status", {
    description: "Single-writer lock status for paper worker",
    inputSchema: {},
  }, async () => {
    const lockPath = join(paperDir(), "writer.lock");
    try {
      const raw = await readFile(lockPath, "utf8");
      const st = await stat(lockPath);
      return textResult({ present: true, path: lockPath, mtime_ms: st.mtimeMs, content: raw });
    } catch {
      return textResult({ present: false, path: lockPath });
    }
  });

  server.registerTool("restart_playbook", {
    description: "Safe restart steps (text only — MCP does not kill processes)",
    inputSchema: {},
  }, async () => textResult({
    steps: [
      "1. Check writer_lock_status — if PID alive, do not start a second worker.",
      "2. Stop the living worker only (pkill -f target/release/btc-window-paper) if intentional.",
      "3. Remove stale writer.lock only when PID is dead.",
      "4. python3 scripts/detach.py var/log/btc-window-paper.log ./target/release/btc-window-paper",
      "5. Optionally start advisor snapshot: cd advisor && npm run snapshot",
      "6. Verify ops_overview live=true and halted=false.",
    ],
    mcp_will_not: ["kill processes", "place orders", "edit engine config"],
  }));

  server.registerTool("data_freshness", {
    description: "Ages of status, tape, features",
    inputSchema: {},
  }, async () => {
    const { payload } = await liveOrThrow();
    const ages: Record<string, number | null> = {
      status_age_ms: payload!.status_age_ms ?? null,
    };
    for (const [name, path] of [
      ["features", paths.featuresLatest()],
      ["tape_meta", paths.tapeMeta()],
      ["status_file", statusFile()],
    ] as const) {
      try {
        ages[`${name}_mtime_age_ms`] = Date.now() - (await stat(path)).mtimeMs;
      } catch {
        ages[`${name}_mtime_age_ms`] = null;
      }
    }
    return textResult(ages);
  });

  // ─── Module G: Reports ────────────────────────────────────────────
  server.registerTool("daily_briefing", {
    description: "3 killers, 2 keepers, capital, one next action",
    inputSchema: { date: z.string().optional() },
  }, async ({ date }) => {
    const { payload } = await liveOrThrow();
    const ranked = rankKillKeep((payload!.ledger?.by_strategy ?? []) as StrategyRow[], 5);
    const diag = payload!.analytics?.diagnostics;
    const briefing = {
      date: date ?? todayUtc(),
      verdict: payload!.halted
        ? "HALTED"
        : (diag as { expectancy_micros?: number } | undefined)?.expectancy_micros != null
          && ((diag as { expectancy_micros: number }).expectancy_micros < 0)
          ? "LOSING"
          : "FLAT_OR_IMPROVING",
      capital_usd: microsToUsdText(payload!.tradable_bankroll_micros as number),
      withdraw_progress: bpsToPctText(payload!.withdraw_progress_bps as number),
      killers: ranked.kill.slice(0, 3),
      demote: ranked.demote.slice(0, 3),
      keepers: ranked.keep.slice(0, 2),
      promote: ranked.promote.slice(0, 2),
      diagnostics: diag,
      next_action: ranked.kill[0]
        ? `Consider DISABLE_SETUP ${(ranked.kill[0] as { setup_id: string }).setup_id} after what_if_disable_setups`
        : "NO_CHANGE — monitor expectancy",
    };
    return textResult(briefing);
  });

  server.registerTool("trade_explain", {
    description: "Plain-English explanation for a condition_id",
    inputSchema: { condition_id: z.string(), date: z.string().optional() },
  }, async ({ condition_id, date }) => {
    const j = await readJournalLines(date ?? todayUtc(), { limit: 20_000 });
    const rows = j.lines.filter((l) => String(l.condition_id) === condition_id);
    if (!rows.length) return errorResult("condition not found in journal");
    const open = rows.find((r) => String(r.action) === "open" || String(r.action) === "open_reversed");
    const close = [...rows].reverse().find((r) =>
      ["exit", "settle", "time_stop", "hard_stop"].includes(String(r.action))
    );
    return textResult({
      condition_id,
      narrative: [
        open
          ? `Opened ${open.asset} ${open.window} setup=${open.setup_id} side=${open.side} stake_tier=${open.stake_tier ?? "?"} risk_bps=${open.risk_bps ?? "?"} ask=${open.ask_micros} reason=${open.reason}`
          : "No open record found.",
        close
          ? `Closed action=${close.action} outcome=${close.outcome} pnl_micros=${close.realized_pnl_micros} hold_secs=${close.hold_secs} reason=${close.reason}`
          : "Still open or close not in day journal.",
      ].join("\n"),
      records: rows,
    });
  });

  server.registerTool("loss_review", {
    description: "Worst trades and shared causes",
    inputSchema: {
      date: z.string().optional(),
      top_n: z.number().int().positive().optional(),
    },
  }, async ({ date, top_n = 15 }) => {
    const closes = await loadDayCloses(date ?? todayUtc());
    const worst = [...closes].sort((a, b) => a.realized_pnl_micros - b.realized_pnl_micros).slice(0, top_n);
    return textResult({
      worst,
      death_patterns: findDeathPatterns(closes).slice(0, 10),
      summary: summarizeCloses(closes),
    });
  });

  server.registerTool("withdraw_progress_report", {
    description: "Distance to next $500 checkpoint",
    inputSchema: {},
  }, async () => {
    const { payload } = await liveOrThrow();
    return textResult({
      capital: payload!.capital,
      tradable_bankroll_usd: microsToUsdText(payload!.tradable_bankroll_micros as number),
      net_realized_usd: microsToUsdText(payload!.net_realized_micros as number),
      withdraw_target_usd: microsToUsdText(payload!.withdraw_target_micros as number),
      progress: bpsToPctText(payload!.withdraw_progress_bps as number),
      progress_bps: payload!.withdraw_progress_bps,
    });
  });

  server.registerTool("export_day_report", {
    description: "Write markdown/json day report under var/poly-advisor/reports",
    inputSchema: {
      date: z.string().optional(),
      format: z.enum(["md", "json"]).optional(),
    },
  }, async ({ date, format = "md" }) => {
    const day = date ?? todayUtc();
    const { payload } = await liveOrThrow();
    const ranked = rankKillKeep((payload!.ledger?.by_strategy ?? []) as StrategyRow[], 5);
    const closes = await loadDayCloses(day);
    const summary = summarizeCloses(closes);
    if (format === "json") {
      const body = JSON.stringify({ day, summary, ranked, capital: payload!.capital }, null, 2);
      const path = await exportReport(`day-${day}`, body, "json");
      return textResult({ path });
    }
    const md = [
      `# Day report ${day}`,
      ``,
      `- Bankroll: ${microsToUsdText(payload!.tradable_bankroll_micros as number)}`,
      `- Withdraw progress: ${bpsToPctText(payload!.withdraw_progress_bps as number)}`,
      `- Closes: ${summary.sample_size} WR=${bpsToPctText(summary.win_rate_bps)} Exp=${microsToUsdText(summary.expectancy_micros)}`,
      `- Realized: ${microsToUsdText(summary.realized_pnl_micros)}`,
      ``,
      `## Kill`,
      ...ranked.kill.slice(0, 5).map((k) => `- ${JSON.stringify(k)}`),
      ``,
      `## Keep`,
      ...ranked.keep.slice(0, 5).map((k) => `- ${JSON.stringify(k)}`),
    ].join("\n");
    const path = await exportReport(`day-${day}`, md, "md");
    return textResult({ path, preview: md.slice(0, 2000) });
  });

  // ─── Module H: Guardrails ─────────────────────────────────────────
  server.registerTool("assert_paper_only", {
    description: "Confirm paper-only / no live authority from live status",
    inputSchema: {},
  }, async () => {
    const { payload } = await liveOrThrow();
    const status = payload!.status as {
      paper_only?: boolean;
      real_money?: boolean;
      live_trading_authority?: boolean;
      paper_week_capital_authority?: boolean;
    } | null;
    const checks = {
      paper_only: status?.paper_only === true || payload!.live !== false,
      real_money_false: status?.real_money === false || status?.real_money == null,
      live_trading_authority_false: status?.live_trading_authority === false || status?.live_trading_authority == null,
      paper_week_capital_authority_false:
        status?.paper_week_capital_authority === false || status?.paper_week_capital_authority == null,
    };
    const ok = Object.values(checks).every(Boolean);
    return textResult({ ok, checks, allowed_actions: ALLOWED_ACTIONS });
  });

  server.registerTool("forbid_live_order_tools", {
    description: "Meta: this MCP has zero place/cancel/sign tools",
    inputSchema: {},
  }, async () => textResult({
    place_order: false,
    cancel_order: false,
    sign: false,
    set_live_authority: false,
    statement: "Poly Advisor MCP is research-only (ADR 0098 / Spec 094).",
  }));

  server.registerTool("proposal_risk_score", {
    description: "Score whether a proposal may worsen losses",
    inputSchema: {
      actions: z.array(z.object({
        action: z.string(),
        setup_id: z.string().optional(),
        evidence: z.string().optional(),
      })),
      raise_stake: z.boolean().optional(),
    },
  }, async (args) => textResult(validateProposal(args)));

  server.registerTool("require_citations", {
    description: "Verify suggestion actions cite numbers",
    inputSchema: { suggestion_id: z.string() },
  }, async ({ suggestion_id }) => {
    const s = await getSuggestion(suggestion_id);
    if (!s) return errorResult("not found");
    return textResult(requireCitations(s.actions));
  });

  // Convenience: force a snapshot write
  server.registerTool("capture_snapshot_now", {
    description: "Write one 1s snapshot + feature rollup now",
    inputSchema: {},
  }, async () => textResult(await writeSnapshotOnce()));

  // Silence unused helper in tree
  void expectedRiskBps;
  void journalPathForDay;
  void writeFile;
}
