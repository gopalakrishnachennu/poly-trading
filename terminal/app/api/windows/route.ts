import { readFile } from "node:fs/promises";
import { join } from "node:path";

// Detailed live view of the short-window (5m/15m) BTC/ETH markets from the
// read-only capture snapshot, merged with the 1h markets from the projection
// gateway. Every market is emitted in one uniform shape — full complementary
// book depth plus pair economics — so the terminal can render the same detail
// it shows for the hourly markets.
// Read-only observation: no order, credential, wallet or execution path.
export const dynamic = "force-dynamic";

type Level = { price_micros: string; quantity_micros: string };
type MarketBook = { bids: Level[]; asks: Level[]; best_bid_micros: string; best_ask_micros: string; tick_size_micros: string };
type Market = {
  asset: string;
  window: "5m" | "15m" | "1h";
  cond: string;
  start_ms: number;
  end_ms: number;
  t_ms: number;
  reference_micros: string | null;
  target_micros: string | null;
  up_book: MarketBook;
  down_book: MarketBook;
  pair: { buy_pair_cost_micros: string; raw_gap_micros: string; executable_quantity_micros: string };
};

const API = (process.env.NEXT_PUBLIC_TERMINAL_API_URL ?? "http://127.0.0.1:8088").replace(/\/+$/, "");
const NO_LEVELS: Level[] = [];

function bestMicros(levels: Level[], fallback: string): string {
  return levels.length > 0 ? levels[0].price_micros : fallback;
}

// buy the complete set: pay both best asks; the redeemed set is always worth $1.
function pairEconomics(upAsks: Level[], downAsks: Level[]) {
  const upAsk = upAsks[0], downAsk = downAsks[0];
  if (!upAsk || !downAsk) {
    return { buy_pair_cost_micros: "0", raw_gap_micros: "0", executable_quantity_micros: "0" };
  }
  const cost = BigInt(upAsk.price_micros) + BigInt(downAsk.price_micros);
  const qty = BigInt(upAsk.quantity_micros) < BigInt(downAsk.quantity_micros) ? upAsk.quantity_micros : downAsk.quantity_micros;
  return { buy_pair_cost_micros: cost.toString(), raw_gap_micros: (1_000_000n - cost).toString(), executable_quantity_micros: qty };
}

// short-capture rows are [price_micros, size_micros] pairs, best-first.
function toLevels(rows: [number, number][] | undefined): Level[] {
  if (!Array.isArray(rows)) return NO_LEVELS;
  return rows.map(([price, size]) => ({ price_micros: String(price), quantity_micros: String(size) }));
}

function book(bids: Level[], asks: Level[], tick: string): MarketBook {
  return { bids, asks, best_bid_micros: bestMicros(bids, "0"), best_ask_micros: bestMicros(asks, "0"), tick_size_micros: tick };
}

type GatewayAsset = {
  asset: string; start_time_ms: number; end_time_ms: number; condition_id: string;
  reference_price_micros: string; target_price_micros: string;
  up_book: MarketBook; down_book: MarketBook;
  pair: { buy_pair_cost_micros: string; raw_gap_micros: string; executable_quantity_micros: string };
};

async function gatewayAssets(): Promise<Map<string, GatewayAsset>> {
  const map = new Map<string, GatewayAsset>();
  try {
    const response = await fetch(`${API}/api/v1/terminal/snapshot`, { cache: "no-store" });
    if (!response.ok) return map;
    const snap = await response.json() as { mode?: string; assets?: GatewayAsset[] };
    if (snap.mode !== "ready" || !Array.isArray(snap.assets)) return map;
    for (const asset of snap.assets) map.set(asset.asset, asset);
  } catch { /* gateway unavailable */ }
  return map;
}

export async function GET() {
  const path = process.env.POLY_WINDOWS_SNAPSHOT_PATH
    ?? join(process.cwd(), "..", "var", "windows-latest.json");

  const gateway = await gatewayAssets();
  const markets: Market[] = [];

  // Short windows (5m/15m) from the read-only capture snapshot.
  let shortAgeMs: number | null = null;
  let shortLive = false;
  try {
    const raw = await readFile(path, "utf8");
    const snap = JSON.parse(raw) as { generated_at_ms: number; markets: Record<string, unknown>[] };
    shortAgeMs = Date.now() - snap.generated_at_ms;
    if (shortAgeMs <= 30_000 && Array.isArray(snap.markets)) {
      for (const raw of snap.markets) {
        const m = raw as {
          asset: string; window: "5m" | "15m"; cond: string; start_ms: number; end_ms: number; t_ms: number;
          up_bids?: [number, number][]; up_asks?: [number, number][]; down_bids?: [number, number][]; down_asks?: [number, number][];
        };
        const upBids = toLevels(m.up_bids), upAsks = toLevels(m.up_asks);
        const downBids = toLevels(m.down_bids), downAsks = toLevels(m.down_asks);
        markets.push({
          asset: m.asset, window: m.window, cond: m.cond, start_ms: m.start_ms, end_ms: m.end_ms, t_ms: m.t_ms,
          reference_micros: gateway.get(m.asset)?.reference_price_micros ?? null, target_micros: null,
          up_book: book(upBids, upAsks, "10000"), down_book: book(downBids, downAsks, "10000"),
          pair: pairEconomics(upAsks, downAsks),
        });
      }
      shortLive = markets.length > 0;
    }
  } catch { /* capture not running */ }

  // Hourly (1h) from the projection gateway.
  for (const asset of gateway.values()) {
    markets.push({
      asset: asset.asset, window: "1h", cond: asset.condition_id, start_ms: asset.start_time_ms, end_ms: asset.end_time_ms, t_ms: Date.now(),
      reference_micros: asset.reference_price_micros, target_micros: asset.target_price_micros,
      up_book: asset.up_book, down_book: asset.down_book, pair: asset.pair,
    });
  }

  const order = { "5m": 0, "15m": 1, "1h": 2 } as const;
  markets.sort((a, b) => a.asset.localeCompare(b.asset) || order[a.window] - order[b.window]);

  return Response.json({
    available: true,
    generated_at_ms: Date.now(),
    short_capture_age_ms: shortAgeMs,
    short_capture_live: shortLive,
    gateway_ready: gateway.size > 0,
    markets,
    note: "Observation only. Read-only public data; no order, credential or execution authority.",
  });
}
