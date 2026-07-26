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
  // Rollover-cached: the gateway is momentarily stale (empty book side at
  // resolution) and this frame is the last-good one held over the blip. Still
  // within its own end_time_ms; not served past resolution. Frontend renders a
  // ROLLOVER indicator instead of dropping the row entirely.
  stale?: boolean;
  stale_age_ms?: number;
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
type GatewayAssets = Map<"BTC" | "ETH", GatewayAsset>;

// Hard cap on the gateway fetch — without it, an unresponsive gateway
// (SIGSTOP, network hang) blocks every /api/windows request indefinitely and
// wedges the whole terminal. On timeout we fall through to the hourly cache.
const GATEWAY_FETCH_TIMEOUT_MS = 2_500;

function isMicros(value: unknown): value is string {
  return typeof value === "string" && /^-?\d+$/.test(value);
}

function isBook(value: unknown): value is MarketBook {
  if (!value || typeof value !== "object") return false;
  const book = value as Partial<MarketBook>;
  const levelsOkay = (levels: unknown) => Array.isArray(levels) && levels.length > 0 && levels.every((level) =>
    Boolean(level) && typeof level === "object" && isMicros((level as Level).price_micros) &&
    isMicros((level as Level).quantity_micros));
  return levelsOkay(book.bids) && levelsOkay(book.asks) && isMicros(book.best_bid_micros) &&
    isMicros(book.best_ask_micros) && isMicros(book.tick_size_micros);
}

function asAtomicGatewayAssets(value: unknown): GatewayAssets | undefined {
  if (!Array.isArray(value) || value.length !== 2) return undefined;
  const assets: GatewayAssets = new Map();
  for (const candidate of value) {
    if (!candidate || typeof candidate !== "object") return undefined;
    const asset = candidate as Partial<GatewayAsset>;
    const startMs = asset.start_time_ms;
    const endMs = asset.end_time_ms;
    if ((asset.asset !== "BTC" && asset.asset !== "ETH") || assets.has(asset.asset) ||
        typeof startMs !== "number" || typeof endMs !== "number" ||
        !Number.isSafeInteger(startMs) || !Number.isSafeInteger(endMs) || startMs >= endMs ||
        typeof asset.condition_id !== "string" ||
        !isMicros(asset.reference_price_micros) || !isMicros(asset.target_price_micros) ||
        !isBook(asset.up_book) || !isBook(asset.down_book) || !asset.pair ||
        !isMicros(asset.pair.buy_pair_cost_micros) || !isMicros(asset.pair.raw_gap_micros) ||
        !isMicros(asset.pair.executable_quantity_micros)) return undefined;
    assets.set(asset.asset, asset as GatewayAsset);
  }
  return assets.size === 2 && assets.has("BTC") && assets.has("ETH") ? assets : undefined;
}

async function gatewayAssets(): Promise<GatewayAssets | undefined> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), GATEWAY_FETCH_TIMEOUT_MS);
  try {
    const response = await fetch(`${API}/api/v1/terminal/snapshot`, {
      cache: "no-store", signal: controller.signal,
    });
    if (!response.ok) return undefined;
    const snap = await response.json() as { mode?: string; assets?: GatewayAsset[] };
    if (snap.mode !== "ready") return undefined;
    return asAtomicGatewayAssets(snap.assets);
  } catch { /* gateway unavailable, timeout, or malformed body */ return undefined; }
  finally { clearTimeout(timer); }
}

// The last-good hourly frame is held across the top-of-hour rollover blip as
// one BTC+ETH pair. Per-asset caching could combine a fresh BTC market with a
// stale ETH market, which is not a coherent terminal frame. It is never served
// past either market's own end_time_ms and is capped at 90 s of staleness.
const HOURLY_CACHE_TTL_MS = 90_000;
type CachedHourlyPair = { assets: GatewayAssets; cached_at_ms: number };
let hourlyCache: CachedHourlyPair | undefined;

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
          reference_micros: gateway?.get(m.asset as "BTC" | "ETH")?.reference_price_micros ?? null, target_micros: null,
          up_book: book(upBids, upAsks, "10000"), down_book: book(downBids, downAsks, "10000"),
          pair: pairEconomics(upAsks, downAsks),
        });
      }
      shortLive = markets.length > 0;
    }
  } catch { /* capture not running */ }

  // Hourly (1h) from the projection gateway — with rollover-blip carry-over.
  const now = Date.now();
  if (gateway) {
    hourlyCache = { assets: gateway, cached_at_ms: now };
    for (const asset of gateway.values()) {
      markets.push({
        asset: asset.asset, window: "1h", cond: asset.condition_id, start_ms: asset.start_time_ms, end_ms: asset.end_time_ms, t_ms: now,
        reference_micros: asset.reference_price_micros, target_micros: asset.target_price_micros,
        up_book: asset.up_book, down_book: asset.down_book, pair: asset.pair,
      });
    }
  } else if (hourlyCache) {
    const ageMs = now - hourlyCache.cached_at_ms;
    const cacheUsable = ageMs <= HOURLY_CACHE_TTL_MS && [...hourlyCache.assets.values()].every((asset) => now < asset.end_time_ms);
    if (!cacheUsable) {
      hourlyCache = undefined;
    } else {
      for (const asset of hourlyCache.assets.values()) {
        markets.push({
          asset: asset.asset, window: "1h", cond: asset.condition_id, start_ms: asset.start_time_ms, end_ms: asset.end_time_ms, t_ms: now,
          reference_micros: asset.reference_price_micros, target_micros: asset.target_price_micros,
          up_book: asset.up_book, down_book: asset.down_book, pair: asset.pair, stale: true, stale_age_ms: ageMs,
        });
      }
    }
  }

  const order = { "5m": 0, "15m": 1, "1h": 2 } as const;
  markets.sort((a, b) => a.asset.localeCompare(b.asset) || order[a.window] - order[b.window]);

  return Response.json({
    available: true,
    generated_at_ms: Date.now(),
    short_capture_age_ms: shortAgeMs,
    short_capture_live: shortLive,
    gateway_ready: gateway !== undefined,
    markets,
    note: "Observation only. Read-only public data; no order, credential or execution authority.",
  });
}
