#!/usr/bin/env python3
"""Regime analysis — does the intraday mean-reversion signal depend on
what the underlying BTC/ETH price is doing?

Adapted from Vibe-Trading's "factor" concept: instead of running 452
generic quant factors on data they weren't designed for, we compute a
few regime signals from OUR data and check if strategy performance
splits by regime.  If yes, we can only trade in the favourable regime.

Regime features computed per market at OPEN:
  - underlying_momentum:  price change of BTC/ETH reference in the
                          preceding 60s vs the 5m average
  - intra_vol:            realised volatility over the window
                          (a proxy for how much price moved)
  - hour_bucket:          which 4-hour block of UTC day

Then splits the mean-reversion strategy results by each regime.

Read-only.  Uses only stdlib.
"""
from __future__ import annotations
import glob, json, math, statistics
from collections import defaultdict
from datetime import datetime, timezone


DOLLAR = 1_000_000


def load_tape(path: str) -> dict:
    markets: dict[str, dict] = {}
    with open(path) as h:
        for line in h:
            try: row = json.loads(line)
            except ValueError: continue
            if row.get("k") == "sys":
                p = row.get("payload", {})
                if p.get("event_type") != "market_identity": continue
                markets[p["condition_id"]] = {
                    "cond": p["condition_id"], "asset": p["asset"],
                    "start_ms": p["start_time_ms"], "end_ms": p["end_time_ms"],
                    "up_tok": p["up_token_id"], "down_tok": p["down_token_id"],
                    "up_ticks": [], "down_ticks": [],
                }
            elif row.get("k") == "book":
                cond = row.get("cond")
                if cond not in markets: continue
                tok = row.get("tok")
                bids = row.get("bids") or []
                asks = row.get("asks") or []
                t_ms = row.get("t", 0) // 1_000_000
                tick = {
                    "t_ms": t_ms,
                    "bid_px": bids[0][0] / DOLLAR if bids else None,
                    "ask_px": asks[0][0] / DOLLAR if asks else None,
                }
                if tok == markets[cond]["up_tok"]:
                    markets[cond]["up_ticks"].append(tick)
                elif tok == markets[cond]["down_tok"]:
                    markets[cond]["down_ticks"].append(tick)
    return markets


def prepare(m: dict):
    for side in ("up_ticks", "down_ticks"):
        m[side].sort(key=lambda t: t["t_ms"])
        m[side] = [t for t in m[side]
                   if m["start_ms"] <= t["t_ms"] <= m["end_ms"]]
    if len(m["up_ticks"]) < 4 or len(m["down_ticks"]) < 4:
        return None
    return m


def implied_underlying(m: dict) -> list[tuple[int, float]]:
    """Reconstruct a proxy for the underlying price direction from the binary.

    The binary token's UP-ask is a probability P(UP wins).  When the underlying
    moves up during a window, P(UP wins) rises.  So the CHANGE in UP-ask over
    time is a signed proxy for the underlying price movement (in probability
    units, not $), suitable for computing momentum / vol regime features.
    """
    return [(t["t_ms"], t["ask_px"]) for t in m["up_ticks"]
            if t.get("ask_px") is not None and 0.02 < t["ask_px"] < 0.98]


def momentum(series: list[tuple[int, float]], lookback_ms: int = 60_000) -> float | None:
    """Sign of P(UP) drift in the last lookback_ms of the series prefix."""
    if len(series) < 4: return None
    latest_t = series[-1][0]
    start_t = latest_t - lookback_ms
    window = [s for s in series if s[0] >= start_t]
    if len(window) < 3: return None
    return window[-1][1] - window[0][1]


def intra_vol(series: list[tuple[int, float]]) -> float:
    prices = [p for _, p in series]
    if len(prices) < 4: return 0.0
    return statistics.pstdev(prices)


def sim_mean_reversion(ticks: list[dict], buy_at: float, sell_at: float,
                       stake: float = 10.0) -> tuple[float, float, int]:
    if not ticks: return 0.0, 0.0, 0
    pnl = 0.0; stake_used = 0.0; bets = 0
    in_pos = False; entry_ask = None
    for t in ticks:
        a, b = t.get("ask_px"), t.get("bid_px")
        if a is None or b is None: continue
        if not in_pos:
            if a <= buy_at and 0.02 < a < 0.98:
                in_pos = True; entry_ask = a
                bets += 1; stake_used += stake
        else:
            if b >= sell_at:
                pnl += stake / entry_ask * b - stake
                in_pos = False; entry_ask = None
    if in_pos and entry_ask is not None:
        last_bid = None
        for t in reversed(ticks):
            if t.get("bid_px") is not None:
                last_bid = t["bid_px"]; break
        if last_bid is not None:
            pnl += stake / entry_ask * last_bid - stake
    return pnl, stake_used, bets


def slice_report(sliced: list[dict], side_key: str, buy: float, sell: float,
                 label: str) -> None:
    pnl = 0.0; stk = 0.0; bets = 0
    for m in sliced:
        p, s, b = sim_mean_reversion(m[side_key], buy, sell)
        pnl += p; stk += s; bets += b
    roi = pnl / stk * 100 if stk else 0.0
    print(f"  {label:<40} n={len(sliced):>3}  bets={bets:>4}  "
          f"pnl={pnl:+8.2f}  roi={roi:+6.2f}%")


def main() -> None:
    all_markets = []
    for sub, label in (("research-5m", "5m"), ("research-15m", "15m"),
                       ("research-1h", "1h")):
        for path in glob.glob(f"var/{sub}/*.jsonl"):
            for m in load_tape(path).values():
                m["window"] = label
                p = prepare(m)
                if p: all_markets.append(p)

    per_win = defaultdict(list)
    for m in all_markets: per_win[m["window"]].append(m)

    # Attach regime features to each market
    for m in all_markets:
        series = implied_underlying(m)
        m["momentum_60s"] = momentum(series, 60_000)
        m["intra_vol"] = intra_vol(series)
        m["hour_bucket"] = datetime.fromtimestamp(m["start_ms"]/1000, tz=timezone.utc).hour // 4

    for window in ("15m", "1h"):
        markets = per_win[window]
        if len(markets) < 10: continue
        print("=" * 78)
        print(f"WINDOW = {window}  ({len(markets)} markets)   "
              f"strategy: buy UP<=0.25 / sell UP>=0.40")
        print("=" * 78)

        print("\n  ── SPLIT BY UNDERLYING MOMENTUM (last-60s P(UP) drift) ──")
        for lo, hi, label in [
            (-1.0, -0.02, "STRONG DOWN drift (< -2¢/60s)"),
            (-0.02, -0.005, "mild DOWN drift"),
            (-0.005, 0.005, "flat (near zero)"),
            (0.005, 0.02, "mild UP drift"),
            (0.02, 1.0, "STRONG UP drift (> +2¢/60s)"),
        ]:
            sliced = [m for m in markets if m["momentum_60s"] is not None
                      and lo <= m["momentum_60s"] < hi]
            if sliced:
                slice_report(sliced, "up_ticks", 0.25, 0.40, label)

        print("\n  ── SPLIT BY INTRA-WINDOW VOLATILITY (P(UP) stdev) ──")
        vols = [m["intra_vol"] for m in markets if m["intra_vol"] > 0]
        if vols:
            vols.sort()
            terts = [vols[len(vols)//3], vols[2*len(vols)//3]]
            for lo, hi, label in [
                (0.0, terts[0], f"LOW vol (<= {terts[0]:.3f})"),
                (terts[0], terts[1], f"MID vol"),
                (terts[1], 1.0, f"HIGH vol (>= {terts[1]:.3f})"),
            ]:
                sliced = [m for m in markets if lo <= m["intra_vol"] < hi]
                if sliced:
                    slice_report(sliced, "up_ticks", 0.25, 0.40, label)

        print("\n  ── SPLIT BY HOUR-BUCKET (UTC) ──")
        for hb in range(6):
            sliced = [m for m in markets if m["hour_bucket"] == hb]
            if len(sliced) >= 5:
                slice_report(sliced, "up_ticks", 0.25, 0.40,
                             f"UTC {hb*4:02d}:00 - {hb*4+4:02d}:00")

        print()

    # Also: is DOWN-token mean-rev profitable in DOWN-drift regimes?
    print("=" * 78)
    print("SYMMETRY CHECK — DOWN token in DOWN-drift regime  (window=15m)")
    print("=" * 78)
    markets = per_win["15m"]
    for lo, hi, label in [
        (-1.0, -0.02, "STRONG DOWN drift"),
        (-0.02, -0.005, "mild DOWN drift"),
        (-0.005, 0.005, "flat"),
        (0.005, 0.02, "mild UP drift"),
        (0.02, 1.0, "STRONG UP drift"),
    ]:
        sliced = [m for m in markets if m["momentum_60s"] is not None
                  and lo <= m["momentum_60s"] < hi]
        if sliced:
            slice_report(sliced, "down_ticks", 0.25, 0.40, f"DOWN token, {label}")


if __name__ == "__main__":
    main()
