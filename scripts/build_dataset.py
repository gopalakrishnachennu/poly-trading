#!/usr/bin/env python3
"""Brick 2: build a frozen, per-decision research dataset from the L2 export.

Reads the faithful L2 JSONL produced by `export_l2` and turns it into the
dataset the intraday research plan needs, answering one question per row:

  At this second in the hourly market, if I buy this token now, can I sell it
  later at a positive net EXECUTABLE price?

For each token (the UP and DOWN leg of every hourly market) it reconstructs the
top-of-book path, samples decision points across the hour, and labels each with
the future executable exit bid at several horizons, the resulting net return
after fees, and the maximum favourable / adverse excursion.

Honesty rules (from the plan):
  * A price touching a level is never assumed to trade. Entry pays the ask,
    exit receives the bid — you always cross the spread twice.
  * This v1 labels the OPTIMISTIC executable case (best bid/ask) plus a
    fill-feasibility flag from the resting size at the touch. If even the
    optimistic net edge is not positive, no conservative model can rescue it,
    so this is the correct first cut. Full depth-walk conservative slippage is
    a follow-up (the per-level depth is present in the export).
  * The independent unit is the hourly market. Train/validation/test are split
    CHRONOLOGICALLY by market start, never by row, so the test set is genuinely
    unseen future.

Nothing here trades. Output is derived research data under var/dataset/.

Usage
-----
  python3 scripts/build_dataset.py
  python3 scripts/build_dataset.py --input var/research-l2/clob-l2.jsonl \
      --step 30 --target-shares 20 --fee 0.0
"""
from __future__ import annotations

import argparse
import bisect
import glob
import hashlib
import json
import os
import time
from array import array
from collections import defaultdict

DOLLAR = 1_000_000
HORIZONS_S = [15, 30, 60, 300, 600]


def load_stream(paths):
    """Single pass: collect market identities and compact per-token top-of-book."""
    markets: dict[str, dict] = {}
    token_side: dict[str, tuple[str, str]] = {}
    series: dict[str, dict[str, array]] = defaultdict(lambda: {
        "t": array("q"), "bb": array("q"), "ba": array("q"),
        "bbsz": array("q"), "basz": array("q"),
        "bdep": array("q"), "adep": array("q"),
    })
    for path in paths:
        with open(path) as handle:
            for line in handle:
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                kind = row.get("k")
                if kind == "sys":
                    payload = row.get("payload") or {}
                    if payload.get("event_type") != "market_identity":
                        continue
                    cond = payload.get("condition_id")
                    if not cond or cond in markets:
                        if cond:
                            token_side.setdefault(payload["up_token_id"], (cond, "UP"))
                            token_side.setdefault(payload["down_token_id"], (cond, "DOWN"))
                        continue
                    markets[cond] = {
                        "asset": payload.get("asset"),
                        "start_ms": int(payload["start_time_ms"]),
                        "end_ms": int(payload["end_time_ms"]),
                        "up": payload["up_token_id"],
                        "down": payload["down_token_id"],
                    }
                    token_side[payload["up_token_id"]] = (cond, "UP")
                    token_side[payload["down_token_id"]] = (cond, "DOWN")
                elif kind == "book":
                    tok = row.get("tok")
                    bids = row.get("bids") or []
                    asks = row.get("asks") or []
                    if not tok:
                        continue
                    s = series[tok]
                    s["t"].append(int(row["t"]) // 1_000_000)  # ns -> ms
                    s["bb"].append(bids[0][0] if bids else 0)
                    s["bbsz"].append(bids[0][1] if bids else 0)
                    s["ba"].append(asks[0][0] if asks else 0)
                    s["basz"].append(asks[0][1] if asks else 0)
                    s["bdep"].append(sum(level[1] for level in bids))
                    s["adep"].append(sum(level[1] for level in asks))
    return markets, token_side, series


def sorted_view(s):
    """Return time-sorted parallel lists for a token (venue time can invert)."""
    order = sorted(range(len(s["t"])), key=lambda i: s["t"][i])
    return {key: [col[i] for i in order] for key, col in s.items()}


def at_or_after(times, target):
    idx = bisect.bisect_left(times, target)
    return idx if idx < len(times) else None


def features_and_labels(view, i, meta, cfg):
    times = view["t"]
    t0 = times[i]
    ba0 = view["ba"][i]
    bb0 = view["bb"][i]
    if not (0 < ba0 < DOLLAR) or not (0 < bb0 <= ba0):
        return None
    entry_ask = ba0 / DOLLAR                     # optimistic taker entry (cross the spread)
    tau_ms = meta["end_ms"] - t0
    if tau_ms <= 0:
        return None

    # Momentum: mid change over the last 60s and 300s.
    mid0 = (bb0 + ba0) / 2 / DOLLAR
    def mid_ago(sec):
        j = bisect.bisect_left(times, t0 - sec * 1000) - 1
        if j < 0:
            return None
        return (view["bb"][j] + view["ba"][j]) / 2 / DOLLAR
    m60 = mid_ago(60)
    m300 = mid_ago(300)

    bbsz0 = view["bbsz"][i]
    basz0 = view["basz"][i]
    imb = (bbsz0 - basz0) / (bbsz0 + basz0) if (bbsz0 + basz0) > 0 else 0.0

    target = cfg["target_shares"] * DOLLAR       # micros of shares
    row = {
        "asset": meta["asset"],
        "side": meta["side"],
        "market": meta["cond"][:12],
        "t_ms": t0,
        "tau_ms": tau_ms,
        "tau_min": round(tau_ms / 60000, 2),
        "entry_ask": round(entry_ask, 4),
        "best_bid": round(bb0 / DOLLAR, 4),
        "spread": round((ba0 - bb0) / DOLLAR, 4),
        "mid": round(mid0, 4),
        "bid_sz": bbsz0 / DOLLAR,
        "ask_sz": basz0 / DOLLAR,
        "bid_depth": view["bdep"][i] / DOLLAR,
        "ask_depth": view["adep"][i] / DOLLAR,
        "imbalance": round(imb, 4),
        "mom_60s": round(mid0 - m60, 4) if m60 is not None else None,
        "mom_300s": round(mid0 - m300, 4) if m300 is not None else None,
        "entry_fillable": bool(basz0 >= target),  # is our size resting at the ask?
    }

    # Future executable exit bid at each horizon (optimistic = best bid).
    max_bid = bb0
    min_bid = bb0
    for sec in HORIZONS_S:
        j = at_or_after(times, t0 + sec * 1000)
        if j is None or view["t"][j] > t0 + sec * 1000 + 60_000:
            row[f"bid_{sec}s"] = None
            row[f"ret_{sec}s"] = None
            row[f"fill_{sec}s"] = None
            continue
        exit_bid = view["bb"][j] / DOLLAR
        exit_sz = view["bbsz"][j]
        row[f"bid_{sec}s"] = round(exit_bid, 4)
        row[f"ret_{sec}s"] = round(exit_bid / entry_ask - 1 - cfg["fee"], 4)
        row[f"fill_{sec}s"] = bool(exit_sz >= target)

    # MFE / MAE of the executable bid over the full holding window.
    end_j = at_or_after(times, t0 + max(HORIZONS_S) * 1000) or len(times)
    for k in range(i + 1, min(end_j + 1, len(times))):
        b = view["bb"][k]
        if b > max_bid:
            max_bid = b
        if 0 < b < min_bid:
            min_bid = b
    row["mfe"] = round(max_bid / DOLLAR / entry_ask - 1, 4)
    row["mae"] = round(min_bid / DOLLAR / entry_ask - 1, 4)
    return row


def build(paths, cfg):
    markets, token_side, series = load_stream(paths)
    rows_by_market: dict[str, list] = defaultdict(list)

    for tok, s in series.items():
        if tok not in token_side or len(s["t"]) < 10:
            continue
        cond, side = token_side[tok]
        meta = markets.get(cond)
        if not meta:
            continue
        view = sorted_view(s)
        times = view["t"]
        meta_row = {"asset": meta["asset"], "side": side, "cond": cond,
                    "end_ms": meta["end_ms"], "start_ms": meta["start_ms"]}
        # Decision points every `step` seconds, inside a buffer from the edges.
        lo = meta["start_ms"] + cfg["start_buffer_s"] * 1000
        hi = meta["end_ms"] - cfg["end_buffer_s"] * 1000
        t = max(lo, times[0])
        while t <= hi:
            i = at_or_after(times, t)
            if i is None:
                break
            if view["t"][i] <= hi:
                labelled = features_and_labels(view, i, meta_row, cfg)
                if labelled:
                    rows_by_market[cond].append(labelled)
            t += cfg["step"] * 1000
    return markets, rows_by_market


def write_split(path, rows):
    with open(path, "w") as handle:
        for row in rows:
            handle.write(json.dumps(row, separators=(",", ":")) + "\n")
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--input", default="var/research-l2/clob-l2.jsonl")
    ap.add_argument("--out-dir", default="var/dataset")
    ap.add_argument("--step", type=int, default=30, help="seconds between decision points")
    ap.add_argument("--start-buffer-s", type=int, default=120)
    ap.add_argument("--end-buffer-s", type=int, default=120)
    ap.add_argument("--target-shares", type=float, default=20.0,
                    help="shares whose resting size defines the fill-feasibility flag")
    ap.add_argument("--fee", type=float, default=0.0, help="taker fee as fraction of stake")
    args = ap.parse_args()

    paths = sorted(glob.glob(args.input))
    if not paths:
        print(f"No input matched {args.input!r}. Run the exporter first.")
        return
    cfg = {"step": args.step, "start_buffer_s": args.start_buffer_s,
           "end_buffer_s": args.end_buffer_s, "target_shares": args.target_shares,
           "fee": args.fee}

    print(f"reading {paths[0]} ...", flush=True)
    markets, rows_by_market = build(paths, cfg)
    resolved = [c for c, rows in rows_by_market.items() if rows]
    resolved.sort(key=lambda c: markets[c]["start_ms"])
    total_rows = sum(len(rows_by_market[c]) for c in resolved)
    print(f"markets with decision rows: {len(resolved)}   total decision rows: {total_rows}")
    if not resolved:
        print("No decision rows produced (need more captured hours).")
        return

    # Chronological split by market start.
    n = len(resolved)
    n_train = max(1, int(n * 0.6))
    n_val = max(0, int(n * 0.2))
    splits = {
        "train": resolved[:n_train],
        "val": resolved[n_train:n_train + n_val],
        "test": resolved[n_train + n_val:],
    }
    os.makedirs(args.out_dir, exist_ok=True)
    manifest = {"generated_at_ms": int(time.time() * 1000), "config": cfg,
                "horizons_s": HORIZONS_S, "markets_total": n, "splits": {}}
    print("\n## Frozen splits (chronological by market start)")
    for name, conds in splits.items():
        rows = [r for c in conds for r in rows_by_market[c]]
        path = os.path.join(args.out_dir, f"{name}.jsonl")
        digest = write_split(path, rows)
        span = ""
        if conds:
            span = (f"{ms_to_utc(markets[conds[0]]['start_ms'])}..{ms_to_utc(markets[conds[-1]]['start_ms'])}")
        manifest["splits"][name] = {"markets": len(conds), "rows": len(rows),
                                    "sha256": digest, "span": span}
        print(f"  {name:5}  markets {len(conds):3d}  rows {len(rows):6d}  {span}  sha {digest[:12]}")

    with open(os.path.join(args.out_dir, "manifest.json"), "w") as handle:
        json.dump(manifest, handle, indent=1)
    print(f"\nmanifest -> {os.path.join(args.out_dir, 'manifest.json')}")
    print("Immutable, chronological, no row leaks across the split boundary.")


def ms_to_utc(ms: int) -> str:
    from datetime import datetime, timezone
    return datetime.fromtimestamp(ms / 1000, timezone.utc).strftime("%m-%d %H:%MZ")


if __name__ == "__main__":
    main()
