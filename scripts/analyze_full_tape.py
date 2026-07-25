#!/usr/bin/env python3
"""Full-tape analysis over every captured market — not just paper bets.

Reads all var/research-{5m,15m,1h}/*.jsonl. For each RESOLVED market
(one whose end_time_ms has passed and has ticks either side of it),
computes what actually happened and what would have been tradable.

Answers the questions the bets-only analysis could not:
  1. Baseline — how often does UP win, per window / asset / hour-of-day?
  2. Does underlying price momentum (last N seconds of reference)
     predict the resolution?
  3. Does book depth (up_size vs down_size at entry) predict?
  4. What is the average time-to-move — does the market decide early
     (advantage: bet late) or drift throughout (advantage: bet early)?
  5. If we ladder N bets across a window at equal price points, does
     the average return beat one-shot at any single decision time?

Read-only, no changes. Prints a compact report.
"""
from __future__ import annotations
import glob, json, os, statistics, math
from collections import defaultdict
from datetime import datetime, timezone


DOLLAR = 1_000_000


def load_tape(path: str) -> dict:
    """Return {condition_id: {"start_ms","end_ms","asset","ticks":[...]}}."""
    markets: dict[str, dict] = {}
    up_tok_to_cond: dict[str, str] = {}
    down_tok_to_cond: dict[str, str] = {}
    with open(path) as h:
        for line in h:
            try: row = json.loads(line)
            except ValueError: continue
            if row.get("k") == "sys":
                p = row.get("payload", {})
                if p.get("event_type") != "market_identity":
                    continue
                cond = p["condition_id"]
                markets[cond] = {
                    "cond": cond, "asset": p["asset"],
                    "start_ms": p["start_time_ms"], "end_ms": p["end_time_ms"],
                    "up_tok": p["up_token_id"], "down_tok": p["down_token_id"],
                    "ticks": [],
                }
                up_tok_to_cond[p["up_token_id"]] = cond
                down_tok_to_cond[p["down_token_id"]] = cond
            elif row.get("k") == "book":
                cond = row.get("cond")
                if cond not in markets:
                    continue
                tok = row.get("tok")
                bids = row.get("bids") or []
                asks = row.get("asks") or []
                t_ms = row.get("t", 0) // 1_000_000
                side = "up" if tok == markets[cond]["up_tok"] else "down"
                markets[cond]["ticks"].append({
                    "t_ms": t_ms, "side": side,
                    "bid_px": bids[0][0] / DOLLAR if bids else None,
                    "ask_px": asks[0][0] / DOLLAR if asks else None,
                    "bid_sz": bids[0][1] / DOLLAR if bids else 0.0,
                    "ask_sz": asks[0][1] / DOLLAR if asks else 0.0,
                })
    return markets


def resolve(market: dict) -> dict | None:
    """Compute what actually happened on this market."""
    ticks = market["ticks"]
    if len(ticks) < 4:
        return None  # too little data to resolve honestly
    ticks.sort(key=lambda x: x["t_ms"])
    end_ms = market["end_ms"]
    # keep only ticks inside the market's own window
    inside = [t for t in ticks if market["start_ms"] <= t["t_ms"] <= end_ms + 5000]
    if len(inside) < 4:
        return None
    up_ticks = [t for t in inside if t["side"] == "up"]
    down_ticks = [t for t in inside if t["side"] == "down"]
    if not up_ticks or not down_ticks:
        return None
    # implied Up probability from up_ask + down_ask (complete-set arbitrage)
    def implied_up(t_up, t_down):
        if t_up.get("ask_px") is None or t_down.get("ask_px") is None:
            return None
        return t_up["ask_px"]  # ask is the price you pay to buy Up
    open_up = up_ticks[0]
    open_down = down_ticks[0]
    close_up = up_ticks[-1]
    close_down = down_ticks[-1]
    # Outcome = the side whose ask crashed to ~0 by close (loser goes to 0, winner to 1)
    close_up_ask = close_up.get("ask_px")
    close_down_ask = close_down.get("ask_px")
    if close_up_ask is None or close_down_ask is None:
        return None
    if close_up_ask <= 0.05 and close_down_ask >= 0.90:
        up_won = False
    elif close_down_ask <= 0.05 and close_up_ask >= 0.90:
        up_won = True
    elif close_up_ask + close_down_ask > 1.5:
        return None  # book decayed weirdly, can't resolve
    else:
        # ambiguous close — skip
        return None
    return {
        "cond": market["cond"], "asset": market["asset"],
        "start_ms": market["start_ms"], "end_ms": market["end_ms"],
        "open_up_ask": open_up.get("ask_px"),
        "open_down_ask": open_down.get("ask_px"),
        "open_up_bid_sz": open_up.get("bid_sz", 0),
        "open_up_ask_sz": open_up.get("ask_sz", 0),
        "open_down_bid_sz": open_down.get("bid_sz", 0),
        "open_down_ask_sz": open_down.get("ask_sz", 0),
        "up_won": up_won,
        "n_ticks": len(inside),
        "up_ticks": up_ticks,
        "down_ticks": down_ticks,
    }


def wilson(k: int, n: int) -> tuple[float, float, float]:
    if n == 0: return (0.0, 0.0, 0.0)
    p = k / n
    z = 1.96
    denom = 1 + z*z/n
    centre = (p + z*z/(2*n)) / denom
    margin = z * math.sqrt(p*(1-p)/n + z*z/(4*n*n)) / denom
    return (p, max(0.0, centre - margin), min(1.0, centre + margin))


def print_headline(label: str, wins: int, n: int, extra: str = ""):
    if n == 0:
        print(f"  {label:<40} — 0 markets —")
        return
    p, lo, hi = wilson(wins, n)
    marker = " *" if (lo > 0.50 or hi < 0.50) else "  "
    print(f"  {label:<40} n={n:>4} up_wins={p*100:5.1f}% "
          f"[{lo*100:4.1f},{hi*100:4.1f}]{marker}{extra}")


def main() -> None:
    all_resolved: list[dict] = []
    for window_sub, window_label, window_sec in (
        ("research-5m", "5m", 300), ("research-15m", "15m", 900),
        ("research-1h", "1h", 3600),
    ):
        markets_this_window: list[dict] = []
        for path in glob.glob(f"var/{window_sub}/*.jsonl"):
            markets = load_tape(path)
            for m in markets.values():
                m["window"] = window_label
                m["window_sec"] = window_sec
                res = resolve(m)
                if res:
                    res["window"] = window_label
                    res["window_sec"] = window_sec
                    all_resolved.append(res)
                    markets_this_window.append(res)
        print(f"loaded {window_label:<3}: {len(markets_this_window)} resolved markets")

    print(f"\ntotal resolved markets: {len(all_resolved)}")
    print()

    print("=" * 80)
    print("Q1  BASELINE — how often does UP win?  (bias vs 50%?)")
    print("=" * 80)
    by_win_asset: dict[tuple, list] = defaultdict(list)
    for r in all_resolved:
        by_win_asset[(r["window"], r["asset"])].append(r)
        by_win_asset[(r["window"], "*")].append(r)
    for key in sorted(by_win_asset):
        wins = sum(1 for r in by_win_asset[key] if r["up_won"])
        print_headline(f"{key[0]}/{key[1]}", wins, len(by_win_asset[key]))
    print()

    print("=" * 80)
    print("Q2  MOMENTUM — does the OPEN price predict UP winning?")
    print("     open_up_ask > 0.5 → market thinks UP.  Does market get it right?")
    print("=" * 80)
    price_buckets = [(0.00, 0.20), (0.20, 0.40), (0.40, 0.50),
                     (0.50, 0.60), (0.60, 0.80), (0.80, 1.01)]
    for window in ("5m", "15m", "1h"):
        window_markets = [r for r in all_resolved if r["window"] == window]
        for lo, hi in price_buckets:
            sub = [r for r in window_markets
                   if r["open_up_ask"] is not None and lo <= r["open_up_ask"] < hi]
            wins = sum(1 for r in sub if r["up_won"])
            if len(sub) >= 5:
                # EV of buying UP at this open price: prob_win / open_up_ask - 1
                avg_open = statistics.fmean(r["open_up_ask"] for r in sub) if sub else 0
                if avg_open > 0 and len(sub) > 0:
                    ev_up = (wins / len(sub)) / avg_open - 1
                    ev_down = ((len(sub) - wins) / len(sub)) / max(1e-6, 1 - avg_open) - 1
                    ev_note = f"  EV(buy UP)={ev_up*100:+5.1f}%  EV(buy DOWN)={ev_down*100:+5.1f}%"
                else:
                    ev_note = ""
                print_headline(f"{window}  open_up_ask [{lo:.2f},{hi:.2f})",
                               wins, len(sub), ev_note)
        print()

    print("=" * 80)
    print("Q3  BOOK DEPTH — does open size imbalance predict?")
    print("     (up_bid_sz - down_bid_sz) / total.  +ve = more capital wants UP.")
    print("=" * 80)
    for window in ("5m", "15m", "1h"):
        markets = [r for r in all_resolved if r["window"] == window]
        for lo, hi in [(-1.01, -0.30), (-0.30, -0.10), (-0.10, 0.10),
                       (0.10, 0.30), (0.30, 1.01)]:
            sub = []
            for r in markets:
                total = r["open_up_bid_sz"] + r["open_down_bid_sz"]
                if total <= 0: continue
                imb = (r["open_up_bid_sz"] - r["open_down_bid_sz"]) / total
                if lo <= imb < hi:
                    sub.append(r)
            wins = sum(1 for r in sub if r["up_won"])
            if len(sub) >= 5:
                print_headline(f"{window}  bid_imb [{lo:+.2f},{hi:+.2f})", wins, len(sub))
        print()

    print("=" * 80)
    print("Q4  HOUR-OF-DAY — is there a time-of-day when UP-bias exists?")
    print("     (UTC hour when the market opened)")
    print("=" * 80)
    for hour in range(24):
        hour_markets = [r for r in all_resolved
                        if datetime.fromtimestamp(r["start_ms"]/1000, tz=timezone.utc).hour == hour]
        wins = sum(1 for r in hour_markets if r["up_won"])
        if len(hour_markets) >= 10:
            print_headline(f"UTC {hour:02d}:00", wins, len(hour_markets))
    print()

    print("=" * 80)
    print("Q5  LADDER SIM — buy UP at 4 equally-spaced points across the window")
    print("     vs one shot at midpoint.  On resolved markets only.")
    print("=" * 80)
    def simulate_strategy(markets: list[dict], strategy: str, stake_each: float = 10.0) -> dict:
        """strategy: 'oneshot_mid', 'ladder4', 'oneshot_late'."""
        pnl = 0.0; total_stake = 0.0; wins = 0; bets = 0
        for r in markets:
            ups = r["up_ticks"]
            if len(ups) < 5: continue
            entry_ticks_uphill = []
            n = len(ups)
            if strategy == "oneshot_mid":
                entry_ticks_uphill = [ups[n // 2]]
            elif strategy == "oneshot_late":
                entry_ticks_uphill = [ups[int(n * 0.85)]]
            elif strategy == "ladder4":
                for pct in (0.25, 0.45, 0.65, 0.85):
                    entry_ticks_uphill.append(ups[int(n * pct)])
            elif strategy == "oneshot_early":
                entry_ticks_uphill = [ups[int(n * 0.15)]]
            for tick in entry_ticks_uphill:
                px = tick.get("ask_px")
                if px is None or not (0.05 < px < 0.95):
                    continue  # skip degenerate prices
                stake = stake_each
                # Directional rule: bet on the side priced UNDER 0.5 (contrarian to favorite)
                # This gives us BOTH long and short exposures across the sample.
                if px < 0.5:
                    side_up = True; price = px
                else:
                    # bet DOWN instead of UP at this tick
                    down_tick = min(r["down_ticks"], key=lambda t: abs(t["t_ms"] - tick["t_ms"]))
                    dp = down_tick.get("ask_px")
                    if dp is None or not (0.05 < dp < 0.95): continue
                    side_up = False; price = dp
                shares = stake / price
                won = (side_up and r["up_won"]) or ((not side_up) and (not r["up_won"]))
                pnl += (shares - stake) if won else (-stake)
                total_stake += stake
                bets += 1
                if won: wins += 1
        return {"strategy": strategy, "bets": bets, "wins": wins,
                "pnl": pnl, "stake": total_stake,
                "roi_pct": (pnl / total_stake * 100) if total_stake else 0.0}
    for window in ("5m", "15m", "1h"):
        markets = [r for r in all_resolved if r["window"] == window]
        print(f"\n  {window} markets ({len(markets)} resolved):")
        for strat in ("oneshot_early", "oneshot_mid", "oneshot_late", "ladder4"):
            r = simulate_strategy(markets, strat)
            print(f"    {strat:<15} bets={r['bets']:>4} win={r['wins']/max(1,r['bets'])*100:5.1f}% "
                  f"pnl={r['pnl']:+9.2f}  roi={r['roi_pct']:+6.2f}%")


if __name__ == "__main__":
    main()
