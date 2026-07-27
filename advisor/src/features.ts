import { mkdir, writeFile, readdir, unlink } from "node:fs/promises";
import { join } from "node:path";
import { fetchLivePayload } from "./api.js";
import { ensureAdvisorDirs, paths, todayUtc } from "./paths.js";
import { microsToUsdText, bpsToPctText } from "./money.js";
import type { PaperPayload, StrategyRow } from "./types.js";

function compactFeatures(payload: PaperPayload) {
  const ledger = payload.ledger;
  const strategies = (ledger?.by_strategy ?? []) as StrategyRow[];
  const top = [...strategies]
    .sort((a, b) => (a.quality?.expectancy_micros ?? a.total_pnl_micros) - (b.quality?.expectancy_micros ?? b.total_pnl_micros))
    .slice(0, 12)
    .map((s) => ({
      setup_id: s.setup_id,
      book: s.book,
      closed: s.totals.closed_trades,
      wr_bps: s.win_rate_bps,
      exp_micros: s.quality?.expectancy_micros ?? null,
      pf_bps: s.quality?.profit_factor_bps ?? null,
      pnl_micros: s.totals.realized_pnl_micros,
      streak: s.totals.current_streak ?? null,
    }));

  const diag = payload.analytics?.diagnostics ?? null;
  return {
    captured_at: new Date().toISOString(),
    live: payload.live ?? null,
    halted: payload.halted ?? null,
    status_age_ms: payload.status_age_ms ?? null,
    capital: {
      tradable_bankroll_micros: payload.tradable_bankroll_micros ?? null,
      tradable_bankroll_usd: microsToUsdText(payload.tradable_bankroll_micros as number | null),
      withdraw_progress_bps: payload.withdraw_progress_bps ?? null,
      withdraw_progress: bpsToPctText(payload.withdraw_progress_bps as number | null),
      withdrawn_micros: payload.capital?.withdrawn_micros ?? null,
    },
    diagnostics: diag,
    open_positions: ledger?.open_positions?.length ?? 0,
    active_core: ledger?.active_core ?? null,
    shadow_experiment: ledger?.shadow_experiment ?? null,
    setup_focus: top,
    last_actions: (payload.status as { last_actions?: unknown } | null)?.last_actions ?? null,
  };
}

async function pruneSnapshots(dayDir: string, keep = 3_600): Promise<void> {
  // keep ~1 hour of 1s snaps by default
  try {
    const files = (await readdir(dayDir)).filter((f) => f.endsWith(".json")).sort();
    if (files.length <= keep) return;
    const drop = files.slice(0, files.length - keep);
    for (const f of drop) {
      await unlink(join(dayDir, f)).catch(() => undefined);
    }
  } catch {
    /* empty */
  }
}

export async function writeSnapshotOnce(): Promise<{ ok: boolean; path?: string; error?: string }> {
  await ensureAdvisorDirs();
  const { ok, payload, error, source } = await fetchLivePayload();
  if (!ok || !payload) {
    return { ok: false, error: error ?? "no payload" };
  }

  const day = todayUtc();
  const dayDir = paths.snapshots(day);
  await mkdir(dayDir, { recursive: true });
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const snapPath = join(dayDir, `${stamp}.json`);

  const slim = {
    source,
    captured_at: new Date().toISOString(),
    live: payload.live,
    halted: payload.halted,
    status_age_ms: payload.status_age_ms,
    capital: payload.capital,
    tradable_bankroll_micros: payload.tradable_bankroll_micros,
    net_realized_micros: payload.net_realized_micros,
    withdraw_progress_bps: payload.withdraw_progress_bps,
    ledger_open: payload.ledger?.open_positions ?? [],
    by_strategy: payload.ledger?.by_strategy ?? [],
    diagnostics: payload.analytics?.diagnostics ?? null,
    last_decisions: (payload.analytics?.recent_decisions ?? []).slice(0, 20),
  };
  await writeFile(snapPath, `${JSON.stringify(slim)}\n`, "utf8");
  await pruneSnapshots(dayDir);

  const features = compactFeatures(payload);
  await writeFile(paths.featuresLatest(), `${JSON.stringify(features, null, 2)}\n`, "utf8");

  // Append newest decisions to tape
  const { appendFile } = await import("node:fs/promises");
  for (const d of (payload.analytics?.recent_decisions ?? []).slice(0, 5)) {
    await appendFile(paths.tapeLatest(), `${JSON.stringify({ ...d, _captured_at: features.captured_at })}\n`, "utf8");
  }
  await writeFile(
    paths.tapeMeta(),
    `${JSON.stringify({ updated_at: features.captured_at, source }, null, 2)}\n`,
    "utf8",
  );

  // Day archive summary
  await mkdir(paths.archivesDir(), { recursive: true });
  await writeFile(
    join(paths.archivesDir(), `${day}.json`),
    `${JSON.stringify({
      day,
      updated_at: features.captured_at,
      capital: features.capital,
      diagnostics: features.diagnostics,
      setup_focus: features.setup_focus,
      active_core: features.active_core,
    }, null, 2)}\n`,
    "utf8",
  );

  return { ok: true, path: snapPath };
}
