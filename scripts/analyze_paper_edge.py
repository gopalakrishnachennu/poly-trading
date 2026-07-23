#!/usr/bin/env python3
"""Complete-set arbitrage edge analysis over captured paper-campaign observations.

This is a read-only research tool. It reads the local paper-campaign JSONL
journals (under ``var/paper-campaign/`` by default, which are Git-ignored) and
quantifies whether a top-of-book complete-set arbitrage opportunity ever existed
in the captured window. It creates nothing and submits nothing.

Background
----------
Each hourly market is a binary pair (Up / Down). One "complete set"
(one Up + one Down share) redeems for exactly $1.00. Prices are fixed-point
micros where ``1_000_000`` == $1.00.

    buy_cost      = up_best_ask + down_best_ask   # pay now, redeem $1.00 later
    sell_proceeds = up_best_bid + down_best_bid   # mint a set for $1.00, sell now

    buy  gross edge = 1_000_000 - buy_cost        # profitable if > 0
    sell gross edge = sell_proceeds - 1_000_000   # profitable if > 0
    net edge        = gross - fee - slippage
    opportunity     = net edge >= minimum_locked_edge

Fee / slippage / minimum-locked-edge reserves default to the values in
``config/paper-market-policy.json`` but can be overridden on the command line.

Usage
-----
    python3 scripts/analyze_paper_edge.py
    python3 scripts/analyze_paper_edge.py --glob 'var/paper-campaign/paper-*.jsonl'
    python3 scripts/analyze_paper_edge.py --fee 1000 --slippage 500 --min-edge 1000
"""
from __future__ import annotations

import argparse
import glob
import json
import statistics
from collections import Counter

DOLLAR = 1_000_000


def usd(micros: float) -> str:
    return f"${micros / DOLLAR:.6f}"


def pct(n: int, d: int) -> str:
    return f"{(100.0 * n / d):.4f}%" if d else "n/a"


def load_policy_reserves(path: str) -> tuple[int, int, int]:
    """Return (fee, slippage, min_edge) micros from a paper-market policy file.

    Uses the maximum across assets so the threshold is the most conservative
    one the runtime would apply. Falls back to (1000, 500, 1000) if unreadable.
    """
    try:
        with open(path) as f:
            policy = json.load(f)
        assets = policy.get("assets", {})
        fee = max(int(a["fee_micros"]) for a in assets.values())
        slip = max(int(a["slippage_micros"]) for a in assets.values())
        edge = max(int(a["minimum_locked_edge_micros"]) for a in assets.values())
        return fee, slip, edge
    except Exception:
        return 1000, 500, 1000


def analyze(paths: list[str], fee: int, slip: int, min_edge: int) -> None:
    per_asset: dict[str, dict] = {}
    total = 0
    tmin = tmax = None
    buckets: Counter[int] = Counter()

    for path in paths:
        with open(path) as f:
            for line in f:
                try:
                    r = json.loads(line)["record"]
                except Exception:
                    continue
                if r.get("kind") != "observation":
                    continue
                p = r["payload"]
                try:
                    ua = int(p["up_best_ask_micros"])
                    ub = int(p["up_best_bid_micros"])
                    da = int(p["down_best_ask_micros"])
                    db = int(p["down_best_bid_micros"])
                except (KeyError, ValueError):
                    continue

                asset = p.get("asset", "?")
                t = r.get("event_time_ms")
                if t is not None:
                    tmin = t if tmin is None else min(tmin, t)
                    tmax = t if tmax is None else max(tmax, t)

                buy_cost = ua + da
                sell_proceeds = ub + db
                buy_net = (DOLLAR - buy_cost) - fee - slip
                sell_net = (sell_proceeds - DOLLAR) - fee - slip

                a = per_asset.setdefault(asset, {
                    "n": 0, "buy_cost": [], "sell_proceeds": [],
                    "buy_opp": 0, "sell_opp": 0,
                    "best_buy_net": None, "best_sell_net": None,
                })
                a["n"] += 1
                total += 1
                a["buy_cost"].append(buy_cost)
                a["sell_proceeds"].append(sell_proceeds)
                a["best_buy_net"] = buy_net if a["best_buy_net"] is None else max(a["best_buy_net"], buy_net)
                a["best_sell_net"] = sell_net if a["best_sell_net"] is None else max(a["best_sell_net"], sell_net)
                if buy_net >= min_edge:
                    a["buy_opp"] += 1
                if sell_net >= min_edge:
                    a["sell_opp"] += 1
                buckets[(buy_cost - DOLLAR) // 10_000] += 1

    if total == 0:
        print("No observation records found. Check --glob path.")
        return

    thresh = fee + slip + min_edge
    hours = (tmax - tmin) / 3_600_000 if tmin and tmax else 0.0
    print("# Complete-set arbitrage edge analysis\n")
    print(f"Observations analysed : {total:,}")
    print(f"Capture span          : {hours:.2f} hours")
    print(f"Reserves              : fee {usd(fee)} + slippage {usd(slip)} + "
          f"min locked edge {usd(min_edge)}")
    print(f"Opportunity threshold : the pair must clear {usd(thresh)} beyond "
          f"$1.00 (buy below, or sell above)\n")

    for asset in sorted(per_asset):
        a = per_asset[asset]
        n, bc, sp = a["n"], a["buy_cost"], a["sell_proceeds"]
        print(f"## {asset}  (n={n:,})")
        print(f"  buy_cost      min {usd(min(bc))}  median {usd(int(statistics.median(bc)))}  max {usd(max(bc))}")
        print(f"  sell_proceeds min {usd(min(sp))}  median {usd(int(statistics.median(sp)))}  max {usd(max(sp))}")
        print(f"  BUY  opportunities (net >= min edge): {a['buy_opp']:>7,} / {n:,}  ({pct(a['buy_opp'], n)})")
        print(f"  SELL opportunities (net >= min edge): {a['sell_opp']:>7,} / {n:,}  ({pct(a['sell_opp'], n)})")
        print(f"  best buy  net edge observed: {usd(a['best_buy_net'])} / share")
        print(f"  best sell net edge observed: {usd(a['best_sell_net'])} / share\n")

    print("## buy_cost distribution (whole cents above $1.00)")
    for c in sorted(buckets):
        print(f"  +{c:>2}c  ~${1 + c / 100:.2f}: {buckets[c]:>7,}  ({pct(buckets[c], total)})")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--glob", default="var/paper-campaign/paper-*.jsonl")
    ap.add_argument("--policy", default="config/paper-market-policy.json")
    ap.add_argument("--fee", type=int)
    ap.add_argument("--slippage", type=int)
    ap.add_argument("--min-edge", type=int)
    args = ap.parse_args()

    fee, slip, edge = load_policy_reserves(args.policy)
    if args.fee is not None:
        fee = args.fee
    if args.slippage is not None:
        slip = args.slippage
    if args.min_edge is not None:
        edge = args.min_edge

    paths = sorted(glob.glob(args.glob))
    if not paths:
        print(f"No files matched {args.glob!r}.")
        return
    analyze(paths, fee, slip, edge)


if __name__ == "__main__":
    main()
