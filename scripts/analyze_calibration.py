#!/usr/bin/env python3
"""Is the market itself mispriced? Calibration and favourite-longshot analysis.

Every strategy so far has tried to out-predict the market. This asks a different
and much more promising question: does the market's OWN price carry a systematic
bias we can harvest without predicting anything?

The classic prediction-market edge is the favourite-longshot bias: bettors
overpay for unlikely outcomes, so longshots are systematically overpriced and
favourites underpriced. If that exists here and is bigger than the spread, then
mechanically backing favourites is profitable with no model at all.

Method
------
For each resolved hourly market we sample the book at several minutes-to-expiry.
At each sample we record the market's implied probability for Up (the mid) and
the realised outcome. Then:

  1. Calibration: bucket by implied probability, compare to realised frequency.
     A well-calibrated market sits on the diagonal; deviations are the edge.
  2. Cost floor: measure the actual half-spread you pay to enter. Any bias must
     exceed this to be harvestable.
  3. Harvest test: simulate mechanically buying the favourite (or the longshot)
     at the ask, across thresholds, and report P&L per $1 staked.

Usage
-----
  python3 scripts/analyze_calibration.py
  python3 scripts/analyze_calibration.py --asset BTC --decision-min 20
"""
from __future__ import annotations

import argparse
import glob
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hourly_engine import load_markets  # noqa: E402

DECISIONS = [45.0, 30.0, 20.0, 10.0, 5.0]


def wilson(k: int, n: int) -> tuple[float, float]:
    """95% Wilson interval for a binomial proportion — honest error bars."""
    if n == 0:
        return (0.0, 1.0)
    z = 1.96
    p = k / n
    d = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / d
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return (max(0.0, centre - half), min(1.0, centre + half))


def sample_at(market, minutes: float):
    """Book state closest to `minutes` before expiry, or None."""
    target = minutes * 60_000
    best = min(market.obs, key=lambda o: abs(o.tau_ms - target))
    return best if abs(best.tau_ms - target) <= 150_000 else None


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--glob", action="append")
    ap.add_argument("--asset", choices=["BTC", "ETH"])
    ap.add_argument("--min-span-min", type=float, default=20.0)
    ap.add_argument("--decision-min", type=float,
                    help="analyse only this minutes-to-expiry (default: several)")
    args = ap.parse_args()

    patterns = args.glob or ["var/paper-campaign/paper-*.jsonl", "var/research-capture/*.jsonl"]
    paths = sorted({p for pat in patterns for p in glob.glob(pat)})
    markets = load_markets(paths, asset_filter=args.asset, min_span_min=args.min_span_min)
    if not markets:
        print("No usable markets. Capture first.")
        return

    decisions = [args.decision_min] if args.decision_min else DECISIONS
    rows = []          # (implied_up, up_ask, down_ask, up_bid, down_bid, outcome_up)
    for m in markets:
        for minutes in decisions:
            o = sample_at(m, minutes)
            if o is None or not (0 < o.up_ask < 1) or not (0 < o.down_ask < 1):
                continue
            implied = (o.up_bid + o.up_ask) / 2
            rows.append((implied, o.up_ask, o.down_ask, o.up_bid, o.down_bid, 1 if m.up_wins else 0))

    if not rows:
        print("No usable samples.")
        return

    print("# Market calibration & favourite-longshot analysis\n")
    print(f"Markets: {len(markets)}   samples: {len(rows)}   "
          f"decision points: {', '.join(f'{d:g}m' for d in decisions)}")
    up_rate = sum(r[5] for r in rows) / len(rows)
    print(f"Overall realised Up rate: {up_rate:.1%}\n")

    # ---- 1. cost floor -----------------------------------------------------
    half_spreads = [(r[1] - r[3]) / 2 for r in rows]
    pair_cost = [r[1] + r[2] for r in rows]
    print("## Cost floor (what you pay just to enter)")
    print(f"  mean half-spread on the Up leg : {sum(half_spreads)/len(half_spreads):.4f} "
          f"(${sum(half_spreads)/len(half_spreads):.4f} per $1 share)")
    print(f"  mean Up ask + Down ask         : {sum(pair_cost)/len(pair_cost):.4f}  "
          f"(1.0000 would be frictionless)")
    print("  Any bias must beat this to be harvestable.\n")

    # ---- 2. calibration ----------------------------------------------------
    print("## Calibration — market implied vs realised")
    print(f"  {'implied band':>14} {'n':>5} {'implied':>9} {'realised':>9} {'95% CI':>16}  verdict")
    edges = [0.0, 0.1, 0.2, 0.35, 0.5, 0.65, 0.8, 0.9, 1.01]
    for lo, hi in zip(edges, edges[1:]):
        bucket = [r for r in rows if lo <= r[0] < hi]
        if not bucket:
            continue
        n = len(bucket)
        k = sum(r[5] for r in bucket)
        implied = sum(r[0] for r in bucket) / n
        realised = k / n
        low, high = wilson(k, n)
        # Only call it a bias if the interval excludes the implied price.
        if implied < low:
            verdict = "UNDERPRICED (Up too cheap)"
        elif implied > high:
            verdict = "OVERPRICED (Up too rich)"
        else:
            verdict = "fair (within noise)"
        print(f"  {lo:>6.2f}-{hi:<6.2f} {n:>5} {implied:>9.3f} {realised:>9.3f} "
              f"  [{low:.2f},{high:.2f}]  {verdict}")
    print()

    # ---- 3. harvest test ---------------------------------------------------
    print("## Harvest test — mechanically back the favourite / the longshot")
    print("  P&L is per $1 staked, buying at the ask and holding to resolution.")
    print(f"  {'rule':>26} {'bets':>6} {'win%':>7} {'P&L/$1':>9} {'total':>9}")
    for threshold in (0.55, 0.60, 0.70, 0.80, 0.90):
        for mode in ("favourite", "longshot"):
            wins = 0
            pnl = 0.0
            bets = 0
            for implied, up_ask, down_ask, _ub, _db, outcome in rows:
                # Pick the side by the market's own probability.
                up_is_fav = implied >= 0.5
                if mode == "favourite":
                    side_up = up_is_fav
                    prob = implied if up_is_fav else 1 - implied
                else:
                    side_up = not up_is_fav
                    prob = 1 - implied if up_is_fav else implied
                if mode == "favourite" and prob < threshold:
                    continue
                if mode == "longshot" and (1 - prob) < threshold:
                    continue
                price = up_ask if side_up else down_ask
                if not 0 < price < 1:
                    continue
                won = (outcome == 1) == side_up
                bets += 1
                wins += 1 if won else 0
                pnl += (1 / price - 1) if won else -1
            if bets:
                print(f"  {mode + ' p>=' + format(threshold, '.2f'):>26} {bets:>6} "
                      f"{100*wins/bets:>6.1f}% {pnl/bets:>+9.4f} {pnl:>+9.2f}")
    print()
    print("## Reading this")
    print("  A real edge shows up as a calibration band whose 95% interval EXCLUDES the")
    print("  implied price, in the same direction, at a size bigger than the half-spread —")
    print("  and a harvest rule with positive P&L/$1 across thresholds, not just one.")
    print(f"  With {len(markets)} markets, treat everything here as provisional.")


if __name__ == "__main__":
    main()
