#!/usr/bin/env python3
"""Audit the immutable paper-bets log against captured market data.

Answers the question: for every bet we have taken, do we also have the
market data that produced it? Read-only; makes no changes.

Reports (per day):
  - bets in the immutable log
  - bets whose full condition_id is present in the corresponding
    research file (var/research-{window}/{asset}-{day}.jsonl)
  - bets missing a book row within [open, settle] on the market
  - bets missing an open_obs snapshot (older bets from before that field
    was captured)

Usage:
  python3 scripts/audit_paper_bets.py
  python3 scripts/audit_paper_bets.py --day 2026-07-25
"""
from __future__ import annotations
import argparse, glob, json, os, sys
from collections import defaultdict


def scan_market_days(root: str, window: str, asset: str, day: str) -> tuple[set[str], dict[str, list[int]]]:
    """Return (set of condition_ids present, dict cond->sorted list of book ns timestamps)."""
    conds: set[str] = set()
    ticks: dict[str, list[int]] = defaultdict(list)
    path = os.path.join(root, f"research-{window}", f"{asset.lower()}-{day}.jsonl")
    if not os.path.exists(path):
        return conds, ticks
    with open(path) as h:
        for line in h:
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if row.get("k") == "sys":
                cond = row.get("payload", {}).get("condition_id")
                if cond:
                    conds.add(cond)
            elif row.get("k") == "book":
                cond = row.get("cond")
                t_ns = row.get("t")
                if cond and isinstance(t_ns, int):
                    ticks[cond].append(t_ns)
    for c in ticks:
        ticks[c].sort()
    return conds, ticks


def audit(bets_root: str, market_root: str, day: str) -> dict:
    bets_path = os.path.join(bets_root, f"bets-{day}.jsonl")
    if not os.path.exists(bets_path):
        return {"day": day, "error": f"no bets log for {day} at {bets_path}"}

    bets: list[dict] = []
    with open(bets_path) as h:
        for line in h:
            try:
                bets.append(json.loads(line))
            except ValueError:
                continue

    # Cache market scans per (window, asset).
    scans: dict[tuple[str, str], tuple[set[str], dict[str, list[int]]]] = {}
    def get_scan(window: str, asset: str):
        key = (window, asset)
        if key not in scans:
            scans[key] = scan_market_days(market_root, window, asset, day)
        return scans[key]

    by_window: dict[str, dict] = defaultdict(lambda: {
        "total": 0, "with_cond_id": 0, "cond_found_in_market": 0,
        "ticks_within_bet_span": 0, "has_open_obs": 0, "has_close_obs": 0,
        "missing_examples": [],
    })

    for b in bets:
        window = b.get("window", "1h")
        asset = b.get("asset", "?")
        cond = b.get("condition_id")
        t = b.get("t")
        settled_t = b.get("settled_t")
        slot = by_window[window]
        slot["total"] += 1
        if b.get("open_obs"): slot["has_open_obs"] += 1
        if b.get("close_obs"): slot["has_close_obs"] += 1
        if not cond:
            if len(slot["missing_examples"]) < 3:
                slot["missing_examples"].append({"reason": "no condition_id", "asset": asset, "market": b.get("market"), "t": t})
            continue
        slot["with_cond_id"] += 1
        conds, ticks = get_scan(window, asset)
        if cond in conds:
            slot["cond_found_in_market"] += 1
            # book rows within [t, settled_t] (t is ms, book ticks are ns)
            if t and settled_t and cond in ticks:
                t_ns_lo = t * 1_000_000
                t_ns_hi = settled_t * 1_000_000
                if any(t_ns_lo <= x <= t_ns_hi for x in ticks[cond]):
                    slot["ticks_within_bet_span"] += 1
                else:
                    if len(slot["missing_examples"]) < 3:
                        slot["missing_examples"].append({
                            "reason": "no book row within bet span",
                            "asset": asset, "cond": cond[:16], "t": t,
                        })
        else:
            if len(slot["missing_examples"]) < 3:
                slot["missing_examples"].append({
                    "reason": "condition_id not found in market data",
                    "asset": asset, "cond": cond[:16], "window": window,
                })

    return {"day": day, "bets_path": bets_path, "bets_total": len(bets), "by_window": dict(by_window)}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bets-dir", default="var/live-paper")
    ap.add_argument("--market-root", default="var")
    ap.add_argument("--day", default=None, help="YYYY-MM-DD; default: audit every daily bets log")
    args = ap.parse_args()

    if args.day:
        days = [args.day]
    else:
        days = sorted({
            os.path.basename(p).removeprefix("bets-").removesuffix(".jsonl")
            for p in glob.glob(os.path.join(args.bets_dir, "bets-*.jsonl"))
        })
    if not days:
        print(f"no bets logs found under {args.bets_dir}", file=sys.stderr)
        sys.exit(1)

    for day in days:
        result = audit(args.bets_dir, args.market_root, day)
        print(f"\n=== {day}  ({result.get('bets_path','?')}) ===")
        if "error" in result:
            print(f"  {result['error']}")
            continue
        print(f"  bets_total: {result['bets_total']}")
        for window in ("5m", "15m", "1h"):
            slot = result["by_window"].get(window)
            if not slot:
                continue
            t = slot["total"]
            print(f"  {window:>3}: {t} bets | with_cond_id {slot['with_cond_id']}/{t} "
                  f"| cond_in_market {slot['cond_found_in_market']}/{t} "
                  f"| tick_in_span {slot['ticks_within_bet_span']}/{t} "
                  f"| open_obs {slot['has_open_obs']}/{t} "
                  f"| close_obs {slot['has_close_obs']}/{t}")
            for m in slot["missing_examples"]:
                print(f"      MISSING: {m}")


if __name__ == "__main__":
    main()
