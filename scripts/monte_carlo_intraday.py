#!/usr/bin/env python3
"""Monte Carlo bootstrap validation of the intraday mean-reversion signal.

The +11.9% ROI on 15m markets from scripts/analyze_intraday.py might be
sample luck.  This bootstraps N times: draw a random sample of markets
WITH REPLACEMENT of the same size as the original, re-run the strategy,
record the ROI. The distribution of ROIs tells us:
  - What is P(ROI > 0) under sampling noise?
  - What is the 5th percentile ROI (worst plausible day)?
  - What is the SD across resamples?

If P(ROI > 0) > 95% and the 5th percentile is still positive, we can
say the finding is robust to sampling noise. If P(ROI > 0) is only
70-80%, we cannot commit to it yet.

Read-only, no changes.
"""
from __future__ import annotations
import argparse, glob, json, math, random, statistics
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
                }
                if tok == markets[cond]["up_tok"]:
                    markets[cond]["up_ticks"].append(tick)
                elif tok == markets[cond]["down_tok"]:
                    markets[cond]["down_ticks"].append(tick)
    return markets


def prepare(m: dict) -> dict | None:
    for side in ("up_ticks", "down_ticks"):
        m[side].sort(key=lambda t: t["t_ms"])
        m[side] = [t for t in m[side]
                   if m["start_ms"] <= t["t_ms"] <= m["end_ms"]]
    if len(m["up_ticks"]) < 4 or len(m["down_ticks"]) < 4:
        return None
    return m


def sim_mean_reversion(ticks: list[dict], buy_at: float, sell_at: float,
                       stake: float = 10.0) -> tuple[float, float, int, int]:
    """Returns (pnl, stake_used, bets, wins). One-position-at-a-time on ticks."""
    if not ticks:
        return 0.0, 0.0, 0, 0
    pnl = 0.0; stake_used = 0.0; bets = 0; wins = 0
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
                shares = stake / entry_ask
                pnl += shares * b - stake
                if b > entry_ask: wins += 1
                in_pos = False; entry_ask = None
    if in_pos and entry_ask is not None:
        last_bid = None
        for t in reversed(ticks):
            if t.get("bid_px") is not None:
                last_bid = t["bid_px"]; break
        if last_bid is not None:
            shares = stake / entry_ask
            pnl += shares * last_bid - stake
            if last_bid > entry_ask: wins += 1
    return pnl, stake_used, bets, wins


def run_strategy(markets: list[dict], buy_at: float, sell_at: float,
                 side: str = "up_ticks") -> dict:
    tot_pnl = 0.0; tot_stake = 0.0; tot_bets = 0; tot_wins = 0
    for m in markets:
        p, s, b, w = sim_mean_reversion(m[side], buy_at, sell_at)
        tot_pnl += p; tot_stake += s; tot_bets += b; tot_wins += w
    return {"pnl": tot_pnl, "stake": tot_stake, "bets": tot_bets, "wins": tot_wins,
            "roi_pct": (tot_pnl / tot_stake * 100) if tot_stake else 0.0}


def bootstrap(markets: list[dict], n_iter: int, buy_at: float, sell_at: float,
              side: str) -> dict:
    rng = random.Random(42)
    rois = []; pnls = []; positive_rois = 0
    n = len(markets)
    for _ in range(n_iter):
        sample = [markets[rng.randrange(n)] for _ in range(n)]  # resample w/ replacement
        r = run_strategy(sample, buy_at, sell_at, side)
        rois.append(r["roi_pct"]); pnls.append(r["pnl"])
        if r["roi_pct"] > 0: positive_rois += 1
    rois.sort()
    def pct(p): return rois[max(0, int(p * len(rois)) - 1)]
    return {
        "n_iter": n_iter,
        "roi_mean": statistics.fmean(rois),
        "roi_median": statistics.median(rois),
        "roi_sd": statistics.pstdev(rois),
        "roi_p05": pct(0.05), "roi_p25": pct(0.25),
        "roi_p75": pct(0.75), "roi_p95": pct(0.95),
        "p_positive": positive_rois / n_iter,
        "pnl_mean": statistics.fmean(pnls),
        "pnl_sd": statistics.pstdev(pnls),
    }


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--iter", type=int, default=5000, help="bootstrap iterations")
    ap.add_argument("--buy",  type=float, default=0.25)
    ap.add_argument("--sell", type=float, default=0.40)
    args = ap.parse_args()

    all_markets = []
    for sub, label in (("research-5m","5m"),("research-15m","15m"),("research-1h","1h")):
        for path in glob.glob(f"var/{sub}/*.jsonl"):
            for m in load_tape(path).values():
                m["window"] = label
                p = prepare(m)
                if p: all_markets.append(p)

    per_win = defaultdict(list)
    for m in all_markets: per_win[m["window"]].append(m)
    print(f"loaded: {len(all_markets)} markets  "
          f"(5m={len(per_win['5m'])}, 15m={len(per_win['15m'])}, 1h={len(per_win['1h'])})")
    print(f"strategy: buy UP/DOWN token when ask<={args.buy}, "
          f"sell when bid>={args.sell}")
    print(f"bootstrap iterations: {args.iter}")
    print()

    for window in ("5m", "15m", "1h"):
        markets = per_win[window]
        if len(markets) < 10:
            print(f"=== {window}: only {len(markets)} markets — skip ===")
            continue
        print(f"=== {window}  ({len(markets)} markets) ===")

        for side_label, side_key in (("UP", "up_ticks"), ("DOWN", "down_ticks")):
            actual = run_strategy(markets, args.buy, args.sell, side_key)
            boot = bootstrap(markets, args.iter, args.buy, args.sell, side_key)
            print(f"  {side_label} token:")
            print(f"    ACTUAL sample:  bets={actual['bets']:>4} pnl={actual['pnl']:+8.2f} "
                  f"roi={actual['roi_pct']:+6.2f}%")
            print(f"    BOOTSTRAP:      P(roi>0)={boot['p_positive']*100:5.1f}%  "
                  f"roi mean={boot['roi_mean']:+.2f}% (sd {boot['roi_sd']:.2f})  "
                  f"median {boot['roi_median']:+.2f}%")
            print(f"    ROI PERCENTILES:  5th={boot['roi_p05']:+6.2f}%  "
                  f"25th={boot['roi_p25']:+6.2f}%  "
                  f"75th={boot['roi_p75']:+6.2f}%  95th={boot['roi_p95']:+6.2f}%")
            verdict = ("ROBUST"    if boot["p_positive"] > 0.95 and boot["roi_p05"] > 0
                       else "STRONG"   if boot["p_positive"] > 0.90
                       else "SUGGESTIVE" if boot["p_positive"] > 0.75
                       else "WEAK — could be sample luck")
            print(f"    VERDICT: {verdict}")
            print()


if __name__ == "__main__":
    main()
