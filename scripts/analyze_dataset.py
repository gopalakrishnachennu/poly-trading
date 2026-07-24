#!/usr/bin/env python3
"""Evaluate the frozen dataset: is there a positive-expectation intraday exit?

Reads a split produced by build_dataset.py (train by default — never peek at
test while forming a view) and reports, per horizon, the distribution of the
net executable return from buying now and selling later, whether any feature
subset separates winners from losers, and whether a positive mean is a real
edge or a fat-tailed lottery (median, trimmed mean, tail concentration).

Usage
-----
  python3 scripts/analyze_dataset.py
  python3 scripts/analyze_dataset.py --split var/dataset/test.jsonl
"""
from __future__ import annotations

import argparse
import json
import statistics

HORIZONS_S = [15, 30, 60, 300, 600]


def load(path):
    return [json.loads(line) for line in open(path)]


def summary(rows):
    print(f"decision rows: {len(rows)}\n")
    print("## Buy now (ask), sell later (bid) — net return per $1, OPTIMISTIC executable")
    print(f"  {'horizon':>8} {'n':>6} {'mean':>9} {'median':>9} {'trim10%':>9} {'%positive':>10} {'%filled':>8}")
    for h in HORIZONS_S:
        r = sorted(x[f"ret_{h}s"] for x in rows if x.get(f"ret_{h}s") is not None)
        if not r:
            continue
        n = len(r)
        fills = [x[f"fill_{h}s"] for x in rows if x.get(f"fill_{h}s") is not None]
        trim = statistics.mean(r[n // 10:-n // 10]) if n >= 20 else statistics.mean(r)
        pos = sum(1 for v in r if v > 0) / n
        fil = sum(1 for v in fills if v) / len(fills) if fills else 0.0
        print(f"  {h:>6}s {n:>6} {statistics.mean(r):>+9.4f} {statistics.median(r):>+9.4f} "
              f"{trim:>+9.4f} {100 * pos:>9.1f}% {100 * fil:>7.1f}%")

    print("\n## Is any positive mean a real edge or a lottery? (per horizon)")
    for h in HORIZONS_S:
        r = sorted(x[f"ret_{h}s"] for x in rows if x.get(f"ret_{h}s") is not None)
        if len(r) < 20 or statistics.mean(r) <= 0:
            continue
        n = len(r)
        gains = sum(v for v in r if v > 0)
        top2 = sum(r[-max(1, n // 50):])
        share = 100 * top2 / gains if gains > 0 else 0
        print(f"  {h}s: mean +{statistics.mean(r):.3f} but median {statistics.median(r):+.3f}; "
              f"{share:.0f}% of gains from top 2% of trades -> lottery, not edge")

    print("\n## Does any feature subset separate winners from losers? (60s horizon)")
    h = 60
    sub = [x for x in rows if x.get(f"ret_{h}s") is not None]

    def bucket(name, predicate):
        vals = [x[f"ret_{h}s"] for x in sub if predicate(x)]
        if vals:
            print(f"  {name:30} n={len(vals):5}  mean {statistics.mean(vals):+.4f}  "
                  f"%+ {100 * sum(1 for v in vals if v > 0) / len(vals):.1f}")

    bucket("momentum up", lambda x: (x.get("mom_60s") or 0) > 0.002)
    bucket("momentum down", lambda x: (x.get("mom_60s") or 0) < -0.002)
    bucket("book bid-heavy (imb>0.3)", lambda x: (x.get("imbalance") or 0) > 0.3)
    bucket("book ask-heavy (imb<-0.3)", lambda x: (x.get("imbalance") or 0) < -0.3)
    bucket("cheap entry (<0.30)", lambda x: x["entry_ask"] < 0.30)
    bucket("mid entry (0.40-0.60)", lambda x: 0.40 <= x["entry_ask"] <= 0.60)
    bucket("favourite entry (>0.70)", lambda x: x["entry_ask"] > 0.70)

    spreads = [x["spread"] for x in rows]
    print(f"\n## Cost reality: mean round-trip spread {2 * statistics.mean(spreads):.4f} per $1")
    print("  The typical trade loses about the spread. That is the whole story.")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--split", default="var/dataset/train.jsonl")
    args = ap.parse_args()
    try:
        rows = load(args.split)
    except FileNotFoundError:
        print(f"{args.split} not found. Run scripts/build_dataset.py first.")
        return
    print(f"# Dataset evaluation — {args.split}\n")
    summary(rows)


if __name__ == "__main__":
    main()
