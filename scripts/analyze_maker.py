#!/usr/bin/env python3
"""Maker-side simulation: can we EARN the spread instead of paying it?

Every strategy so far crossed the spread (bought at the ask) and lost roughly
the ~1% round-trip cost. This tests the opposite stance: post a passive limit
order and let someone else cross to us. If we get filled at the bid instead of
the ask we start each trade ~1% better off, which is larger than any edge we
have been able to measure.

The catch is adverse selection. You are filled precisely when someone wants to
sell to you — often because the market is about to move against you. A maker
simulation is only honest if it captures that, so this one holds every fill to
resolution and reports the realised outcome, not the theoretical spread capture.

Fill model
----------
We only have top-of-book snapshots, not the trade tape, so a fill is inferred:
posting a bid at price P is treated as filled if a later snapshot in the same
hour shows best_ask <= P — evidence that a seller was willing to transact at or
below our price. This is an approximation and is deliberately stated as one:
it ignores queue position (we assume we are first in line, which flatters the
maker) while ignoring any trades between snapshots (which penalises it).

Usage
-----
  python3 scripts/analyze_maker.py
  python3 scripts/analyze_maker.py --asset BTC --decision-min 30
"""
from __future__ import annotations

import argparse
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hourly_engine import load_markets  # noqa: E402

TICK = 0.01


def sample_index(market, minutes: float):
    target = minutes * 60_000
    best_i, best_d = None, None
    for i, o in enumerate(market.obs):
        d = abs(o.tau_ms - target)
        if best_d is None or d < best_d:
            best_i, best_d = i, d
    return best_i if best_d is not None and best_d <= 150_000 else None


def simulate(markets, minutes: float, side_rule: str):
    """Return stats for taker and several maker aggressiveness levels."""
    modes = {
        "TAKER (cross the spread)": None,
        "MAKER at best bid": 0.0,
        "MAKER at bid + 1 tick": TICK,
        "MAKER at mid": None,  # handled specially
    }
    stats = {name: {"posted": 0, "filled": 0, "wins": 0, "pnl": 0.0, "prices": []}
             for name in modes}

    for m in markets:
        i = sample_index(m, minutes)
        if i is None:
            continue
        o = m.obs[i]
        # Which side do we want? "favourite" = the side the market thinks wins.
        up_mid = (o.up_bid + o.up_ask) / 2
        want_up = up_mid >= 0.5 if side_rule == "favourite" else up_mid < 0.5
        bid = o.up_bid if want_up else o.down_bid
        ask = o.up_ask if want_up else o.down_ask
        if not (0 < bid < ask < 1):
            continue
        won = (m.up_wins == want_up)

        for name, offset in modes.items():
            s = stats[name]
            s["posted"] += 1
            if name.startswith("TAKER"):
                price = ask
                filled = True
            else:
                price = (bid + ask) / 2 if name.endswith("mid") else min(bid + (offset or 0.0), ask - TICK / 2)
                if not 0 < price < 1:
                    continue
                # Filled if a later snapshot shows a seller willing to hit our price.
                filled = any(
                    (later.up_ask if want_up else later.down_ask) <= price
                    for later in m.obs[i + 1:]
                )
            if not filled:
                continue
            s["filled"] += 1
            s["prices"].append(price)
            s["wins"] += 1 if won else 0
            s["pnl"] += (1 / price - 1) if won else -1
    return stats


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--glob", action="append")
    ap.add_argument("--asset", choices=["BTC", "ETH"])
    ap.add_argument("--min-span-min", type=float, default=20.0)
    ap.add_argument("--decision-min", type=float, default=30.0)
    ap.add_argument("--side", choices=["favourite", "underdog"], default="favourite")
    args = ap.parse_args()

    patterns = args.glob or ["var/paper-campaign/paper-*.jsonl", "var/research-capture/*.jsonl"]
    paths = sorted({p for pat in patterns for p in glob.glob(pat)})
    markets = load_markets(paths, asset_filter=args.asset, min_span_min=args.min_span_min)
    if not markets:
        print("No usable markets. Capture first.")
        return

    stats = simulate(markets, args.decision_min, args.side)
    print("# Maker vs taker simulation\n")
    print(f"Markets: {len(markets)}   post at ~{args.decision_min:g}m to expiry   "
          f"side: the {args.side}\n")
    print(f"  {'stance':>26} {'posted':>7} {'filled':>7} {'fill%':>7} "
          f"{'avg px':>7} {'win%':>7} {'P&L/fill':>9} {'P&L/post':>9}")
    for name, s in stats.items():
        if not s["posted"]:
            continue
        filled = s["filled"]
        avg = sum(s["prices"]) / filled if filled else 0.0
        per_fill = s["pnl"] / filled if filled else 0.0
        per_post = s["pnl"] / s["posted"]
        win = 100 * s["wins"] / filled if filled else 0.0
        print(f"  {name:>26} {s['posted']:>7} {filled:>7} {100*filled/s['posted']:>6.1f}% "
              f"{avg:>7.3f} {win:>6.1f}% {per_fill:>+9.4f} {per_post:>+9.4f}")

    print("\n## Reading this")
    print("  P&L/fill  — how each executed trade did. Maker should beat taker here if")
    print("              earning the spread works at all.")
    print("  P&L/post  — the honest number: unfilled orders are missed opportunities,")
    print("              and a maker only gets filled when someone wants the other side.")
    print("  If maker P&L/fill is positive but P&L/post is not, the fills are adversely")
    print("  selected: you are filled on the trades you did not want.")
    print(f"  Sample is {len(markets)} markets — provisional.")


if __name__ == "__main__":
    main()
