import { readFile } from "node:fs/promises";
import { join } from "node:path";

// Live grid of the short-window (5m/15m) BTC/ETH markets from the read-only
// capture snapshot, merged with the 1h markets from the projection gateway.
// Read-only observation: no order, credential, wallet or execution path.
export const dynamic = "force-dynamic";

type Market = {
  asset: string;
  window: "5m" | "15m" | "1h";
  cond: string;
  end_ms: number;
  up_bid: number | null;
  up_ask: number | null;
  down_bid: number | null;
  down_ask: number | null;
  t_ms: number;
};

const API = (process.env.NEXT_PUBLIC_TERMINAL_API_URL ?? "http://127.0.0.1:8088").replace(/\/+$/, "");

function micros(value: string): number {
  const n = Number(value);
  return Number.isFinite(n) ? n / 1_000_000 : NaN;
}

async function hourlyFromGateway(): Promise<Market[]> {
  try {
    const response = await fetch(`${API}/api/v1/terminal/snapshot`, { cache: "no-store" });
    if (!response.ok) return [];
    const snap = await response.json() as {
      mode?: string;
      assets?: { asset: string; end_time_ms: number; condition_id: string;
        up_book: { best_bid_micros: string; best_ask_micros: string };
        down_book: { best_bid_micros: string; best_ask_micros: string } }[];
    };
    if (snap.mode !== "ready" || !Array.isArray(snap.assets)) return [];
    return snap.assets.map((a) => ({
      asset: a.asset, window: "1h" as const, cond: a.condition_id.slice(0, 12), end_ms: a.end_time_ms,
      up_bid: micros(a.up_book.best_bid_micros), up_ask: micros(a.up_book.best_ask_micros),
      down_bid: micros(a.down_book.best_bid_micros), down_ask: micros(a.down_book.best_ask_micros),
      t_ms: Date.now(),
    }));
  } catch {
    return [];
  }
}

export async function GET() {
  const path = process.env.POLY_WINDOWS_SNAPSHOT_PATH
    ?? join(process.cwd(), "..", "var", "windows-latest.json");

  let short: Market[] = [];
  let shortAgeMs: number | null = null;
  try {
    const raw = await readFile(path, "utf8");
    const snap = JSON.parse(raw) as { generated_at_ms: number; markets: Market[] };
    shortAgeMs = Date.now() - snap.generated_at_ms;
    // Only trust the short-window snapshot if the capture is actually live.
    if (shortAgeMs <= 30_000 && Array.isArray(snap.markets)) short = snap.markets;
  } catch { /* capture not running */ }

  const hourly = await hourlyFromGateway();
  const markets = [...short, ...hourly];
  const order = { "5m": 0, "15m": 1, "1h": 2 } as const;
  markets.sort((a, b) => a.asset.localeCompare(b.asset) || order[a.window] - order[b.window]);

  return Response.json({
    available: true,
    generated_at_ms: Date.now(),
    short_capture_age_ms: shortAgeMs,
    short_capture_live: short.length > 0,
    markets,
    note: "Observation only. Read-only public data; no order, credential or execution authority.",
  });
}
