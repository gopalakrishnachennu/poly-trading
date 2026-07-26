#!/usr/bin/env python3
"""Per-second feature builder for Polymarket contracts.

Reads:
  - Polymarket L2 captures (var/research-{5m,15m,1h}/*.jsonl in existing schema)
  - Binance spot events (var/research-binance/*.jsonl in profit_engine schema)

Emits one row per (contract_condition_id, second) where the contract is live
and Polymarket has fresh top-of-book on both YES and NO tokens.

Row shape (JSON lines, easy to convert to Parquet later):
    {
      "t_ms":                1785030000000,   # second boundary (ms)
      "contract_cond":       "0x...",          # 64-char Polymarket condition_id
      "asset":               "BTC" | "ETH",
      "window":              "5m" | "15m" | "1h",
      "start_ms":            1785029700000,
      "end_ms":              1785030000000,
      "seconds_remaining":   293,
      "session_progress":    0.245,            # fraction of window elapsed

      // Polymarket top-of-book (YES = UP, NO = DOWN in existing captures)
      "poly_up_bid":         0.42, "poly_up_ask": 0.43,
      "poly_up_bid_sz":      120.0, "poly_up_ask_sz": 80.0,
      "poly_down_bid":       0.56, "poly_down_ask": 0.57,
      "poly_down_bid_sz":    95.0, "poly_down_ask_sz": 210.0,
      "poly_spread":         0.010,            // round-trip complete-set cost
      "poly_up_microprice":  0.4243,
      "poly_up_obi_1":       0.20,             // depth-1 order-book imbalance
      "poly_up_last_price":  0.42,

      // Cross-exchange BTC state (Binance spot as anchor)
      "binance_bid":         62431.10, "binance_ask": 62431.20,
      "binance_mid":         62431.15,
      "binance_return_1s":  -0.00001, "binance_return_5s": +0.00003,
      "binance_return_60s": +0.00021,
      "binance_realized_vol_60s": 0.00018,

      // Distance-to-threshold z-score (from plan §5)
      // For "up or down" contracts, threshold = opening_ref (captured at start)
      "threshold_ref":       62410.50,
      "distance_to_threshold": 20.65,
      "z_distance":          0.87,             // distance / (vol * sqrt(T-t))
      "reference_age_ms":    2100
    }

Feature-quality flags:
    "quality_flags":  ["ok"]                    # or ["stale_binance", "empty_side", ...]

Writes JSONL (upgradeable to Parquet trivially).  Read-only, pure Python.

Usage:
    python3 -m scripts.profit_engine.features.one_second \
        --polymarket 'var/research-15m/*.jsonl' \
        --binance 'var/research-binance/*.jsonl' \
        --out var/features/features-15m.jsonl
"""
from __future__ import annotations
import argparse, glob, json, math, os, statistics, sys
from collections import defaultdict, deque


DOLLAR = 1_000_000


# ------------------------- loaders (streaming) -------------------------- #

def stream_polymarket(paths: list[str]):
    """Yield (t_ms, kind, payload) from every Polymarket capture line."""
    for path in paths:
        with open(path) as h:
            for line in h:
                try: row = json.loads(line)
                except ValueError: continue
                if row.get("k") == "sys":
                    p = row.get("payload") or {}
                    if p.get("event_type") == "market_identity":
                        yield (p["start_time_ms"], "identity", p)
                elif row.get("k") == "book":
                    t_ms = row.get("t", 0) // 1_000_000
                    yield (t_ms, "book", row)


def stream_binance(paths: list[str]):
    for path in paths:
        with open(path) as h:
            for line in h:
                try: row = json.loads(line)
                except ValueError: continue
                yield (row["t_recv_ms"], row)


# ------------------------- feature helpers ------------------------------ #

def microprice(bid: float, ask: float, bid_sz: float, ask_sz: float) -> float:
    tot = bid_sz + ask_sz
    if tot <= 0: return (bid + ask) / 2
    return (ask * bid_sz + bid * ask_sz) / tot


def obi(bid_sz: float, ask_sz: float) -> float:
    tot = bid_sz + ask_sz
    if tot <= 0: return 0.0
    return (bid_sz - ask_sz) / tot


def realized_vol(returns: list[float]) -> float:
    if len(returns) < 2: return 0.0
    return statistics.pstdev(returns)


def z_distance(spot: float, threshold: float, vol_per_s: float,
               seconds_remaining: float) -> float | None:
    if vol_per_s <= 0 or seconds_remaining <= 0: return None
    denom = spot * vol_per_s * math.sqrt(seconds_remaining)
    if denom <= 0: return None
    return (spot - threshold) / denom


# ------------------------- Polymarket per-market state ------------------ #

class ContractState:
    __slots__ = ("cond", "asset", "window", "start_ms", "end_ms",
                 "up_tok", "down_tok",
                 "up_bid", "up_ask", "up_bid_sz", "up_ask_sz",
                 "down_bid", "down_ask", "down_bid_sz", "down_ask_sz",
                 "up_last_seen_ms", "down_last_seen_ms", "opening_ref")

    def __init__(self, ident: dict, asset: str, window: str):
        self.cond = ident["condition_id"]
        self.asset = asset; self.window = window
        self.start_ms = ident["start_time_ms"]; self.end_ms = ident["end_time_ms"]
        self.up_tok = ident["up_token_id"]; self.down_tok = ident["down_token_id"]
        self.up_bid = self.up_ask = None
        self.up_bid_sz = self.up_ask_sz = 0.0
        self.down_bid = self.down_ask = None
        self.down_bid_sz = self.down_ask_sz = 0.0
        self.up_last_seen_ms = self.down_last_seen_ms = 0
        self.opening_ref: float | None = None  # set from first Binance mid inside window

    def apply_book(self, row: dict, t_ms: int) -> None:
        tok = row.get("tok"); bids = row.get("bids") or []; asks = row.get("asks") or []
        if tok == self.up_tok:
            self.up_bid = bids[0][0] / DOLLAR if bids else self.up_bid
            self.up_ask = asks[0][0] / DOLLAR if asks else self.up_ask
            self.up_bid_sz = bids[0][1] / DOLLAR if bids else 0.0
            self.up_ask_sz = asks[0][1] / DOLLAR if asks else 0.0
            self.up_last_seen_ms = t_ms
        elif tok == self.down_tok:
            self.down_bid = bids[0][0] / DOLLAR if bids else self.down_bid
            self.down_ask = asks[0][0] / DOLLAR if asks else self.down_ask
            self.down_bid_sz = bids[0][1] / DOLLAR if bids else 0.0
            self.down_ask_sz = asks[0][1] / DOLLAR if asks else 0.0
            self.down_last_seen_ms = t_ms


# ------------------------- rolling Binance state ------------------------ #

class BinanceRolling:
    """Keep the last N ticks per symbol so we can compute short-horizon
    returns and realised vol at any timestamp without look-ahead."""

    def __init__(self, symbol: str, max_len: int = 3600):
        self.symbol = symbol
        self.buf: deque[tuple[int, float, float, float]] = deque(maxlen=max_len)
        # (t_ms, bid, ask, mid)

    def push(self, row: dict) -> None:
        self.buf.append((row["t_recv_ms"], row["bid"], row["ask"], row["mid"]))

    def state_at(self, t_ms: int) -> dict:
        """Return snapshot: latest at-or-before t_ms + horizon returns."""
        # Walk from the right to find the latest tick at or before t_ms
        latest = None
        for tick in reversed(self.buf):
            if tick[0] <= t_ms:
                latest = tick; break
        if latest is None:
            return {"available": False}

        def mid_at_or_before(target_ms: int) -> float | None:
            for tick in reversed(self.buf):
                if tick[0] <= target_ms:
                    return tick[3]
            return None

        mid_now = latest[3]
        def ret(dt_ms: int) -> float | None:
            past = mid_at_or_before(t_ms - dt_ms)
            return (mid_now / past - 1) if past and past > 0 else None

        # realised vol over last 60s from second-diff mid changes
        recent = [tick for tick in self.buf if t_ms - 60_000 <= tick[0] <= t_ms]
        returns = []
        for a, b in zip(recent, recent[1:]):
            if a[3] > 0: returns.append(math.log(b[3] / a[3]))
        return {
            "available": True,
            "bid": latest[1], "ask": latest[2], "mid": mid_now,
            "return_1s": ret(1000), "return_5s": ret(5000),
            "return_60s": ret(60_000),
            "realized_vol_60s": realized_vol(returns),
            "reference_age_ms": t_ms - latest[0],
        }


# ------------------------- main builder --------------------------------- #

def infer_asset_window(path: str) -> tuple[str, str] | None:
    # var/research-15m/btc-2026-07-25.jsonl -> ("BTC", "15m")
    base = os.path.basename(path)
    parent = os.path.basename(os.path.dirname(path))
    if not parent.startswith("research-"): return None
    window = parent.removeprefix("research-")
    if window not in ("5m", "15m", "1h"): return None
    for asset_prefix, asset in (("btc-", "BTC"), ("eth-", "ETH")):
        if base.startswith(asset_prefix): return (asset, window)
    return None


def underlying_symbols_for(asset: str) -> tuple[str, ...]:
    """Return every acceptable exchange symbol for an asset's underlying.

    Feature builder is exchange-agnostic — Binance BTCUSDT and Coinbase BTCUSD
    both count as BTC underlying.  When both are present we prefer the one
    with more ticks (later can weight by recency / spread quality).
    """
    return {
        "BTC": ("BTCUSDT", "BTCUSD"),
        "ETH": ("ETHUSDT", "ETHUSD"),
    }[asset]


def build_features(polymarket_paths: list[str], binance_paths: list[str],
                   out_path: str) -> dict:
    # Group Polymarket files by (asset, window) so we can label rows
    poly_groups: dict[tuple[str, str], list[str]] = defaultdict(list)
    for p in polymarket_paths:
        info = infer_asset_window(p)
        if info: poly_groups[info].append(p)
    if not poly_groups:
        return {"rows_written": 0, "reason": "no polymarket files matched"}

    # Pre-load ALL external-exchange ticks into per-symbol rolling buffers.
    # Files can be from any collector (binance_spot / coinbase_spot / …) —
    # routing is by the `symbol` field inside each event, not the filename.
    binance: dict[str, BinanceRolling] = {}
    for path in binance_paths:
        for _, row in stream_binance([path]):
            sym = row.get("symbol")
            if not sym: continue
            roll = binance.setdefault(sym, BinanceRolling(sym, max_len=3600 * 8))
            roll.push(row)
    for sym, roll in binance.items():
        print(f"  loaded {len(roll.buf)} external ticks for {sym}", file=sys.stderr)

    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    rows_written = 0
    with open(out_path, "w") as out:
        for (asset, window), paths in sorted(poly_groups.items()):
            # Pick the first available exchange for this asset (prefer richer).
            roll = None
            for candidate in underlying_symbols_for(asset):
                cand = binance.get(candidate)
                if cand and (roll is None or len(cand.buf) > len(roll.buf)):
                    roll = cand
            # Play the Polymarket tape in order.  For each new market, produce
            # a row every whole second while the market is live.
            events = sorted(
                (e for p in paths for e in stream_polymarket([p])),
                key=lambda e: e[0]
            )
            state: ContractState | None = None
            last_emit_second: int = 0
            for t_ms, kind, payload in events:
                if kind == "identity":
                    state = ContractState(payload, asset, window)
                    last_emit_second = 0
                    continue
                if state is None: continue
                state.apply_book(payload, t_ms)

                # Emit at most one row per whole second while inside window.
                sec = t_ms // 1000
                if sec == last_emit_second: continue
                if not (state.start_ms <= t_ms < state.end_ms): continue
                if state.up_ask is None or state.down_ask is None: continue
                if state.up_bid is None or state.down_bid is None: continue

                bx = roll.state_at(t_ms) if roll else {"available": False}
                # Binance/Coinbase features are optional — the plan requires
                # them but until the external collector has accumulated days
                # of ticks the Polymarket-only rows are still useful (fewer
                # features, but still ~500 usable per market for training the
                # baseline logistic).
                if bx.get("available"):
                    if state.opening_ref is None:
                        state.opening_ref = bx["mid"]
                    dist = bx["mid"] - state.opening_ref
                    zd = z_distance(bx["mid"], state.opening_ref,
                                    bx["realized_vol_60s"],
                                    (state.end_ms - t_ms) / 1000)
                else:
                    bx = {
                        "bid": None, "ask": None, "mid": None,
                        "return_1s": None, "return_5s": None, "return_60s": None,
                        "realized_vol_60s": None, "reference_age_ms": None,
                    }
                    dist = None; zd = None

                row = {
                    "t_ms": sec * 1000,
                    "contract_cond": state.cond,
                    "asset": asset, "window": window,
                    "start_ms": state.start_ms, "end_ms": state.end_ms,
                    "seconds_remaining": max(0, (state.end_ms - t_ms) // 1000),
                    "session_progress": round(
                        (t_ms - state.start_ms) / max(1, state.end_ms - state.start_ms), 6),
                    "poly_up_bid": state.up_bid, "poly_up_ask": state.up_ask,
                    "poly_up_bid_sz": state.up_bid_sz, "poly_up_ask_sz": state.up_ask_sz,
                    "poly_down_bid": state.down_bid, "poly_down_ask": state.down_ask,
                    "poly_down_bid_sz": state.down_bid_sz, "poly_down_ask_sz": state.down_ask_sz,
                    "poly_spread": round((state.up_ask + state.down_ask) - 1.0, 6),
                    "poly_up_microprice": round(microprice(
                        state.up_bid, state.up_ask, state.up_bid_sz, state.up_ask_sz), 6),
                    "poly_up_obi_1": round(obi(state.up_bid_sz, state.up_ask_sz), 6),
                    "binance_bid": bx["bid"], "binance_ask": bx["ask"], "binance_mid": bx["mid"],
                    "binance_return_1s": bx["return_1s"], "binance_return_5s": bx["return_5s"],
                    "binance_return_60s": bx["return_60s"],
                    "binance_realized_vol_60s": bx["realized_vol_60s"],
                    "reference_age_ms": bx["reference_age_ms"],
                    "threshold_ref": state.opening_ref,
                    "distance_to_threshold": dist,
                    "z_distance": zd,
                }
                out.write(json.dumps(row, separators=(",", ":")) + "\n")
                rows_written += 1
                last_emit_second = sec
    return {"rows_written": rows_written, "output": out_path}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--polymarket", nargs="+", required=True,
                    help="glob(s) of Polymarket capture jsonl files")
    ap.add_argument("--binance", nargs="+", required=True,
                    help="glob(s) of Binance capture jsonl files (may be empty)")
    ap.add_argument("--out", required=True, help="output jsonl path")
    args = ap.parse_args()

    poly_paths: list[str] = []
    for pat in args.polymarket: poly_paths.extend(sorted(glob.glob(pat)))
    binance_paths: list[str] = []
    for pat in args.binance: binance_paths.extend(sorted(glob.glob(pat)))

    print(f"polymarket files: {len(poly_paths)}, binance files: {len(binance_paths)}",
          file=sys.stderr)
    result = build_features(poly_paths, binance_paths, args.out)
    print(json.dumps(result, indent=2))
    return 0 if result.get("rows_written", 0) > 0 else 1


if __name__ == "__main__":
    sys.exit(main())
