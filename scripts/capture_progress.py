#!/usr/bin/env python3
"""Report how much backtest-ready data has accumulated.

Scans observation journals (paper-campaign and/or research-capture) and counts
the independent unit that matters for validating a strategy: distinct resolved
hourly markets (one outcome per market), broken down by asset and UTC day.

A market is "resolved/usable" here if it was observed for at least --min-span
minutes (so a strike and a near-expiry close both exist).

    python3 scripts/capture_progress.py
    python3 scripts/capture_progress.py --glob 'var/research-capture/*.jsonl'
"""
from __future__ import annotations

import argparse
import glob
import json
from collections import defaultdict
from datetime import datetime, timezone

# Rough power targets for judging a directional edge vs the market mid.
TARGET_MIN = 200
TARGET_GOOD = 500


def utc_day(ms: int) -> str:
    return datetime.fromtimestamp(ms / 1000, timezone.utc).strftime("%Y-%m-%d")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--glob", action="append",
                    help="observation JSONL glob(s); repeatable. "
                         "Default: paper-campaign + research-capture.")
    ap.add_argument("--min-span", type=float, default=20.0,
                    help="minutes a market must be observed to count as resolved")
    args = ap.parse_args()

    patterns = args.glob or [
        "var/paper-campaign/paper-*.jsonl",
        "var/research-capture/*.jsonl",
    ]
    paths = sorted({p for pat in patterns for p in glob.glob(pat)})
    if not paths:
        print("No observation files found. Start capture with "
              "scripts/run-continuous-capture.sh")
        return

    markets: dict[tuple, dict] = defaultdict(lambda: {"tmin": None, "tmax": None, "asset": None})
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
                key = (p.get("asset"), p.get("condition_id"))
                t = r.get("event_time_ms")
                if t is None:
                    continue
                m = markets[key]
                m["asset"] = p.get("asset")
                m["tmin"] = t if m["tmin"] is None else min(m["tmin"], t)
                m["tmax"] = t if m["tmax"] is None else max(m["tmax"], t)

    per_asset: dict[str, int] = defaultdict(int)
    per_day: dict[str, int] = defaultdict(int)
    usable = 0
    total = len(markets)
    for m in markets.values():
        span = (m["tmax"] - m["tmin"]) / 60000 if m["tmin"] else 0
        if span >= args.min_span:
            usable += 1
            per_asset[m["asset"]] += 1
            per_day[utc_day(m["tmin"])] += 1

    print("# Capture progress\n")
    print(f"Files scanned            : {len(paths)}")
    print(f"Distinct markets seen     : {total}")
    print(f"Resolved/usable markets   : {usable}  (observed >= {args.min_span:.0f} min)\n")
    print("By asset:")
    for a in sorted(per_asset):
        print(f"  {a}: {per_asset[a]}")
    print("\nBy UTC day:")
    for d in sorted(per_day):
        print(f"  {d}: {per_day[d]}")

    pct = 100.0 * usable / TARGET_GOOD
    bar = "#" * int(min(usable, TARGET_GOOD) / TARGET_GOOD * 40)
    print(f"\nBacktest readiness (target ~{TARGET_GOOD} markets):")
    print(f"  [{bar:<40}] {usable}/{TARGET_GOOD}  ({pct:.1f}%)")
    if usable < TARGET_MIN:
        print(f"  Not enough yet — under the ~{TARGET_MIN} minimum for any signal. Keep capturing.")
    elif usable < TARGET_GOOD:
        print(f"  Enough for a first look; keep going toward ~{TARGET_GOOD} for a firmer read.")
    else:
        print("  Enough to backtest with meaningful power. Run scripts/backtest_fair_value.py.")


if __name__ == "__main__":
    main()
