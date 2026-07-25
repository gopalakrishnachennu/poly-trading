#!/usr/bin/env python3
"""Intraday strategy analysis — buy low, sell high within a window.

The current paper trader only takes buy-and-hold-to-resolution bets.
Binary tokens are tradable continuously though, so we can enter at
0.30 and exit at 0.60 within the same market and never care whether
the market resolves UP or DOWN.

This works on ALL captured markets (not just the 115 that resolve
cleanly at 0/1), so we get a much bigger sample.

For each market:
  - Compute intra-window price range and volatility
  - Simulate mean-reversion, momentum, and range-trading strategies
  - Compare against buy-and-hold-to-resolution
"""
from __future__ import annotations
import glob, json, statistics
from collections import defaultdict


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
                    "bid_sz": bids[0][1] / DOLLAR if bids else 0.0,
                    "ask_sz": asks[0][1] / DOLLAR if asks else 0.0,
                }
                if tok == markets[cond]["up_tok"]:
                    markets[cond]["up_ticks"].append(tick)
                elif tok == markets[cond]["down_tok"]:
                    markets[cond]["down_ticks"].append(tick)
    return markets


def prepare(market: dict) -> dict | None:
    """Sort ticks and filter to those inside the window."""
    for side in ("up_ticks", "down_ticks"):
        market[side].sort(key=lambda t: t["t_ms"])
        market[side] = [t for t in market[side]
                        if market["start_ms"] <= t["t_ms"] <= market["end_ms"]]
    if len(market["up_ticks"]) < 4 or len(market["down_ticks"]) < 4:
        return None
    return market


def path_stats(ticks: list[dict]) -> dict:
    """Range, volatility, number of significant moves in a price path."""
    prices = [t["ask_px"] for t in ticks if t.get("ask_px") is not None
              and 0.02 < t["ask_px"] < 0.98]
    if len(prices) < 4: return {}
    high = max(prices); low = min(prices); open_ = prices[0]; close = prices[-1]
    range_ = high - low
    # count significant moves: >= 3¢ in either direction across neighboring ticks
    moves = sum(1 for a, b in zip(prices, prices[1:]) if abs(a - b) >= 0.03)
    # avg absolute tick-to-tick change
    diffs = [abs(a - b) for a, b in zip(prices, prices[1:])]
    return {
        "open": open_, "high": high, "low": low, "close": close,
        "range": range_, "moves_3c": moves,
        "avg_move": statistics.fmean(diffs) if diffs else 0.0,
        "n_ticks": len(prices),
    }


# ---------------- intraday strategies ------------------------------------- #

def sim_mean_reversion(ticks: list[dict], buy_at: float, sell_at: float,
                       stake: float = 10.0, side_name: str = "UP") -> dict:
    """Buy when ask <= buy_at, sell when bid >= sell_at within same market.

    One position at a time. Position closes on the FIRST subsequent tick
    where bid >= sell_at. If never reached, close at end of window at
    the last available bid (mark to market).
    """
    if not ticks: return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    bets = 0; wins = 0; pnl = 0.0; total_stake = 0.0
    in_pos = False; entry_ask = None; entry_i = None
    for i, tick in enumerate(ticks):
        ask = tick.get("ask_px"); bid = tick.get("bid_px")
        if ask is None or bid is None: continue
        if not in_pos:
            if ask <= buy_at and 0.02 < ask < 0.98:
                in_pos = True; entry_ask = ask; entry_i = i
                bets += 1; total_stake += stake
        else:
            if bid >= sell_at:
                # sell at bid — realistic exit; you take the bid, not the mid
                shares = stake / entry_ask
                exit_value = shares * bid
                pnl += exit_value - stake
                if bid > entry_ask: wins += 1
                in_pos = False; entry_ask = None
    # close residual position at last observable bid (mark to market)
    if in_pos and entry_ask is not None:
        last_bid = None
        for t in reversed(ticks):
            if t.get("bid_px") is not None:
                last_bid = t["bid_px"]; break
        if last_bid is not None:
            shares = stake / entry_ask
            pnl += shares * last_bid - stake
            if last_bid > entry_ask: wins += 1
    return {"bets": bets, "wins": wins, "pnl": pnl, "stake": total_stake}


def sim_momentum(ticks: list[dict], move_threshold: float = 0.05,
                 window_ticks: int = 3, stake: float = 10.0) -> dict:
    """Buy after a rapid move up; exit at first pause."""
    if len(ticks) < window_ticks + 2:
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    bets = 0; wins = 0; pnl = 0.0; total_stake = 0.0
    in_pos = False; entry_ask = None
    for i in range(window_ticks, len(ticks)):
        ask = ticks[i].get("ask_px"); bid = ticks[i].get("bid_px")
        prev_ask = ticks[i - window_ticks].get("ask_px")
        if ask is None or bid is None or prev_ask is None: continue
        if not (0.05 < ask < 0.95): continue
        if not in_pos:
            move = ask - prev_ask
            if move >= move_threshold:
                in_pos = True; entry_ask = ask
                bets += 1; total_stake += stake
        else:
            # Exit as soon as price pauses (no further rise in next tick) or bid < entry
            if bid < entry_ask * 0.98:  # small stop
                shares = stake / entry_ask
                pnl += shares * bid - stake
                in_pos = False
    if in_pos and entry_ask is not None:
        last_bid = None
        for t in reversed(ticks):
            if t.get("bid_px") is not None:
                last_bid = t["bid_px"]; break
        if last_bid is not None:
            pnl += stake / entry_ask * last_bid - stake
            if last_bid > entry_ask: wins += 1
    return {"bets": bets, "wins": wins, "pnl": pnl, "stake": total_stake}


def sim_range_trade(ticks: list[dict], stake: float = 10.0) -> dict:
    """Detect an oscillating range then buy near range-low, sell near high.

    Cheap approximation: look at the min/max of first 30% of ticks; if
    range is meaningful (>= 8¢), trade the reversal inside that band.
    """
    if len(ticks) < 20:
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    first_third = ticks[: len(ticks) // 3]
    prices = [t.get("ask_px") for t in first_third
              if t.get("ask_px") is not None and 0.05 < t["ask_px"] < 0.95]
    if len(prices) < 5:
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    lo, hi = min(prices), max(prices)
    if hi - lo < 0.08:
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    buy_at = lo + 0.02  # slightly above range-low (fillable)
    sell_at = hi - 0.02
    if sell_at <= buy_at:
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    return sim_mean_reversion(ticks[len(ticks)//3:], buy_at, sell_at, stake)


def sim_buy_hold(ticks: list[dict], side_name: str = "UP",
                 stake: float = 10.0, entry_at_frac: float = 0.15) -> dict:
    """Reference: single buy at entry_at_frac into the window, hold to close."""
    if len(ticks) < 4:
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    entry = ticks[int(len(ticks) * entry_at_frac)]
    ask = entry.get("ask_px")
    if ask is None or not (0.05 < ask < 0.95):
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    close_bid = None
    for t in reversed(ticks):
        if t.get("bid_px") is not None:
            close_bid = t["bid_px"]; break
    if close_bid is None:
        return {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    shares = stake / ask
    pnl = shares * close_bid - stake
    return {"bets": 1, "wins": 1 if close_bid > ask else 0,
            "pnl": pnl, "stake": stake}


def combine(results: list[dict]) -> dict:
    tot = {"bets": 0, "wins": 0, "pnl": 0.0, "stake": 0.0}
    for r in results:
        for k in tot:
            tot[k] += r.get(k, 0)
    tot["roi_pct"] = (tot["pnl"] / tot["stake"] * 100) if tot["stake"] else 0.0
    tot["win_rate"] = tot["wins"] / max(1, tot["bets"])
    return tot


def main() -> None:
    all_markets = []
    for sub, label in (("research-5m","5m"),("research-15m","15m"),("research-1h","1h")):
        for path in glob.glob(f"var/{sub}/*.jsonl"):
            m = load_tape(path)
            for k, mk in m.items():
                mk["window"] = label
                prep = prepare(mk)
                if prep: all_markets.append(prep)

    per_win = defaultdict(list)
    for m in all_markets: per_win[m["window"]].append(m)
    print(f"markets loaded (with usable ticks): {len(all_markets)}")
    for w in ("5m", "15m", "1h"):
        print(f"  {w}: {len(per_win[w])} markets")
    print()

    print("=" * 78)
    print("STEP 1 — Do intraday prices actually MOVE?")
    print("           (If not, intraday trading has no fuel.)")
    print("=" * 78)
    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        stats_up = [path_stats(m["up_ticks"]) for m in markets]
        stats_up = [s for s in stats_up if s]
        if not stats_up: continue
        ranges = [s["range"] for s in stats_up]
        moves = [s["moves_3c"] for s in stats_up]
        avg = [s["avg_move"] for s in stats_up]
        print(f"  {window}:  n={len(stats_up):>3}")
        print(f"       range (high-low) — median {statistics.median(ranges)*100:5.1f}¢  "
              f"mean {statistics.fmean(ranges)*100:5.1f}¢")
        print(f"       3¢+ moves        — median {statistics.median(moves):>3.0f}   "
              f"mean {statistics.fmean(moves):>4.1f}")
        print(f"       avg tick-to-tick — median {statistics.median(avg)*100:5.2f}¢")
    print()

    print("=" * 78)
    print("STEP 2 — Baseline: single-entry buy-and-hold (bet on outcome)")
    print("=" * 78)
    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        # Buy UP once, hold to close
        results = [sim_buy_hold(m["up_ticks"], "UP") for m in markets]
        r = combine(results)
        print(f"  {window} BUY-HOLD UP  bets={r['bets']:>4} win={r['win_rate']*100:5.1f}% "
              f"pnl={r['pnl']:+8.2f} roi={r['roi_pct']:+6.2f}%")
    print()

    print("=" * 78)
    print("STEP 3 — Mean reversion:  buy UP <= 0.30, sell UP >= 0.50")
    print("           (No outcome bet — just catch the swing.)")
    print("=" * 78)
    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        for (b, s) in [(0.30, 0.50), (0.25, 0.40), (0.40, 0.60), (0.20, 0.35)]:
            results = [sim_mean_reversion(m["up_ticks"], b, s) for m in markets]
            r = combine(results)
            if r["bets"] > 0:
                print(f"  {window} buy<={b:.2f}/sell>={s:.2f}  bets={r['bets']:>4} "
                      f"win={r['win_rate']*100:5.1f}% pnl={r['pnl']:+8.2f} "
                      f"roi={r['roi_pct']:+6.2f}%")
        print()

    print("=" * 78)
    print("STEP 4 — Momentum:  buy UP after a >=5¢ rise in 3 ticks, stop at 2% loss")
    print("=" * 78)
    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        for thresh in (0.03, 0.05, 0.08):
            results = [sim_momentum(m["up_ticks"], move_threshold=thresh) for m in markets]
            r = combine(results)
            if r["bets"] > 0:
                print(f"  {window} momentum {thresh*100:.0f}¢/3-tick  bets={r['bets']:>4} "
                      f"win={r['win_rate']*100:5.1f}% pnl={r['pnl']:+8.2f} "
                      f"roi={r['roi_pct']:+6.2f}%")
        print()

    print("=" * 78)
    print("STEP 5 — Range trading:  detect early-window range, buy low sell high")
    print("=" * 78)
    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        results = [sim_range_trade(m["up_ticks"]) for m in markets]
        r = combine(results)
        print(f"  {window} range-trade  bets={r['bets']:>4} "
              f"win={r['win_rate']*100:5.1f}% pnl={r['pnl']:+8.2f} "
              f"roi={r['roi_pct']:+6.2f}%")
    print()

    print("=" * 78)
    print("STEP 6 — SAME strategies but on DOWN token (symmetric check)")
    print("           If mean-reversion pays on UP but not DOWN, it is bias-driven.")
    print("=" * 78)
    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        for (b, s) in [(0.30, 0.50), (0.25, 0.40)]:
            results = [sim_mean_reversion(m["down_ticks"], b, s) for m in markets]
            r = combine(results)
            if r["bets"] > 0:
                print(f"  {window} DOWN buy<={b:.2f}/sell>={s:.2f}  bets={r['bets']:>4} "
                      f"win={r['win_rate']*100:5.1f}% pnl={r['pnl']:+8.2f} "
                      f"roi={r['roi_pct']:+6.2f}%")
        print()

    print("=" * 78)
    print("STEP 7 — Round-trip spread cost check")
    print("           Each mean-reversion trade pays (ask-bid) spread on entry+exit.")
    print("           If spread eats the target, no strategy survives.")
    print("=" * 78)
    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        spreads = []
        for m in markets:
            for t in m["up_ticks"]:
                a, b = t.get("ask_px"), t.get("bid_px")
                if a and b and 0.05 < a < 0.95 and 0.05 < b < 0.95:
                    spreads.append(a - b)
        if spreads:
            print(f"  {window} bid-ask spread on UP  n={len(spreads):>5} "
                  f"median={statistics.median(spreads)*100:.2f}¢ "
                  f"mean={statistics.fmean(spreads)*100:.2f}¢ "
                  f"90th={sorted(spreads)[int(len(spreads)*0.9)]*100:.2f}¢")


if __name__ == "__main__":
    main()
