#!/usr/bin/env python3
"""Phase A analysis — what do the captured paper bets tell us?

Read-only over var/live-paper/bets-*.jsonl. Finds:
  1. Per (strategy, window, asset) net edge with 95% Wilson CI on win rate
     and standard error on per-bet P&L.
  2. Conditional buckets: entry-price, spread, book-imbalance, and
     time-of-day slices — where does each strategy actually make money?
  3. Aggregate portfolio behaviour and dominance test — is the current
     4-strategy portfolio dominated by a subset?

Prints a compact report to stdout. Makes no changes to any file.
"""
from __future__ import annotations
import glob, json, math, os, statistics
from collections import defaultdict
from datetime import datetime, timezone


def load_bets(bets_dir: str) -> list[dict]:
    bets: list[dict] = []
    for path in sorted(glob.glob(os.path.join(bets_dir, "bets-*.jsonl"))):
        with open(path) as h:
            for line in h:
                try:
                    bets.append(json.loads(line))
                except ValueError:
                    continue
    return bets


def wilson_ci(wins: int, n: int, z: float = 1.96) -> tuple[float, float]:
    """Wilson 95% confidence interval for a proportion."""
    if n == 0:
        return (0.0, 0.0)
    p = wins / n
    denom = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denom
    margin = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denom
    return (max(0.0, centre - margin), min(1.0, centre + margin))


def summarize(bets: list[dict], label: str) -> dict:
    """Compute win-rate CI, mean/std P&L, edge per $1 staked."""
    if not bets:
        return {"label": label, "n": 0}
    n = len(bets)
    wins = sum(1 for b in bets if b.get("won") is True)
    voids = sum(1 for b in bets if b.get("void") is True)
    pnls = [b.get("pnl", 0.0) for b in bets]
    stakes = [b.get("stake", 0.0) for b in bets]
    total_pnl = sum(pnls)
    total_stake = sum(stakes)
    mean = total_pnl / n
    sd = statistics.pstdev(pnls) if n > 1 else 0.0
    lo, hi = wilson_ci(wins, n - voids) if n > voids else (0.0, 0.0)
    # per-$ edge (roi) — normalises across different stake sizes
    roi = (total_pnl / total_stake) if total_stake else 0.0
    # sharpe-ish t-stat: mean vs zero
    tstat = (mean / (sd / math.sqrt(n))) if sd > 0 and n > 1 else 0.0
    return {
        "label": label, "n": n, "voids": voids,
        "wins": wins, "win_rate": wins / max(1, n - voids),
        "win_ci95": (lo, hi),
        "total_pnl": total_pnl, "mean_pnl": mean, "sd_pnl": sd,
        "total_stake": total_stake, "roi_pct": roi * 100,
        "tstat": tstat,
    }


def row(s: dict) -> str:
    if s["n"] == 0:
        return f"  {s['label']:<48} — no bets —"
    lo, hi = s["win_ci95"]
    sig = " *" if abs(s["tstat"]) >= 2 else ("  " if abs(s["tstat"]) >= 1 else "  ")
    return (f"  {s['label']:<48} n={s['n']:>4} win={s['win_rate']*100:5.1f}% "
            f"[{lo*100:4.1f},{hi*100:4.1f}] pnl={s['total_pnl']:+8.2f} "
            f"roi={s['roi_pct']:+5.2f}% t={s['tstat']:+.2f}{sig}")


def bucket(bets: list[dict], key: str, edges: list[float], label_fmt: str) -> list[tuple[str, list[dict]]]:
    """Slice bets by a numeric key at the given edges; return [(label, bets)]."""
    out: list[tuple[str, list[dict]]] = []
    for i, lo in enumerate(edges[:-1]):
        hi = edges[i + 1]
        subset = []
        for b in bets:
            val = None
            if key == "entry_price":
                val = b.get("price")
            elif key == "spread":
                obs = b.get("open_obs") or {}
                ua = obs.get("up_ask"); da = obs.get("down_ask")
                if ua is not None and da is not None:
                    val = (ua + da) - 1.0  # complete-set cost above $1
            elif key == "book_imbalance":
                obs = b.get("open_obs") or {}
                side = b.get("side")
                if side == "UP":
                    bs = obs.get("up_bid_sz", 0); a_s = obs.get("up_ask_sz", 0)
                elif side == "DOWN":
                    bs = obs.get("down_bid_sz", 0); a_s = obs.get("down_ask_sz", 0)
                else:
                    bs = a_s = 0
                if bs + a_s > 0:
                    val = (bs - a_s) / (bs + a_s)
            elif key == "hour_utc":
                t = b.get("t")
                if t: val = datetime.fromtimestamp(t / 1000, tz=timezone.utc).hour
            if val is None:
                continue
            if lo <= val < hi:
                subset.append(b)
        out.append((label_fmt.format(lo=lo, hi=hi), subset))
    return out


def main() -> None:
    bets = load_bets("var/live-paper")
    # Filter: analysis-quality bets only (have open_obs, not backfilled void)
    good = [b for b in bets if not b.get("backfilled") and not b.get("void") and b.get("open_obs")]
    print(f"loaded {len(bets)} total bets; {len(good)} analysis-quality "
          f"(non-backfilled, non-void, with open_obs)")
    print()

    print("=" * 78)
    print("PER STRATEGY  (all windows, all assets)")
    print("=" * 78)
    strats = sorted({b["strategy"] for b in good})
    for s in strats:
        print(row(summarize([b for b in good if b["strategy"] == s], s)))
    print()

    print("=" * 78)
    print("PER (STRATEGY, WINDOW)")
    print("=" * 78)
    for s in strats:
        for w in ("5m", "15m", "1h"):
            slice_ = [b for b in good if b["strategy"] == s and b.get("window") == w]
            if slice_:
                print(row(summarize(slice_, f"{s} / {w}")))
    print()

    print("=" * 78)
    print("PER (STRATEGY, WINDOW, ASSET)")
    print("=" * 78)
    for s in strats:
        for w in ("5m", "15m", "1h"):
            for a in ("BTC", "ETH"):
                slice_ = [b for b in good if b["strategy"] == s and b.get("window") == w and b.get("asset") == a]
                if slice_:
                    print(row(summarize(slice_, f"{s} / {w} / {a}")))
    print()

    print("=" * 78)
    print("CONDITIONAL — entry price buckets (per strategy, all windows)")
    print("  Does taking a bet at a specific price band pay?")
    print("=" * 78)
    edges = [0.0, 0.3, 0.5, 0.7, 0.9, 1.01]
    for s in strats:
        s_bets = [b for b in good if b["strategy"] == s]
        for label, sub in bucket(s_bets, "entry_price", edges, "price [{lo:.2f}, {hi:.2f})"):
            if sub:
                print(row(summarize(sub, f"{s} / {label}")))
        print()

    print("=" * 78)
    print("CONDITIONAL — round-trip spread (up_ask+down_ask - 1)")
    print("  Buying complete set costs $1 + spread. Wider = more house edge.")
    print("=" * 78)
    edges = [-0.02, 0.00, 0.005, 0.01, 0.02, 0.05]
    for s in strats:
        s_bets = [b for b in good if b["strategy"] == s]
        for label, sub in bucket(s_bets, "spread", edges, "spread [{lo:.3f}, {hi:.3f})"):
            if sub:
                print(row(summarize(sub, f"{s} / {label}")))
        print()

    print("=" * 78)
    print("CONDITIONAL — order-book imbalance on the side we bet")
    print("  Positive = size resting behind us wants to buy. Should predict wins.")
    print("=" * 78)
    edges = [-1.01, -0.3, -0.1, 0.1, 0.3, 1.01]
    for s in strats:
        s_bets = [b for b in good if b["strategy"] == s]
        for label, sub in bucket(s_bets, "book_imbalance", edges, "imb [{lo:+.2f}, {hi:+.2f})"):
            if sub:
                print(row(summarize(sub, f"{s} / {label}")))
        print()

    print("=" * 78)
    print("CONDITIONAL — hour of day (UTC)")
    print("=" * 78)
    edges = list(range(0, 25))  # per hour
    for s in strats:
        s_bets = [b for b in good if b["strategy"] == s]
        for label, sub in bucket(s_bets, "hour_utc", edges, "UTC {lo:02.0f}:00"):
            if len(sub) >= 5:  # need at least 5 bets for a reading
                print(row(summarize(sub, f"{s} / {label}")))
        print()

    print("=" * 78)
    print("PORTFOLIO DOMINANCE — if we drop the losing legs, what's left?")
    print("=" * 78)
    portfolio_full = summarize(good, "full 4-strategy portfolio")
    print(row(portfolio_full))
    # Pick the (strategy, window) slices with mean_pnl > 0 AND n >= 20
    keepers = []
    for s in strats:
        for w in ("5m", "15m", "1h"):
            slice_ = [b for b in good if b["strategy"] == s and b.get("window") == w]
            summ = summarize(slice_, f"{s}/{w}")
            if summ["n"] >= 20 and summ["mean_pnl"] > 0:
                keepers.append((s, w, slice_))
    if keepers:
        filtered = [b for k in keepers for b in k[2]]
        print(row(summarize(filtered, f"filtered ({len(keepers)} winning slices)")))
        print()
        print("  the winning slices are:")
        for s, w, sl in keepers:
            summ = summarize(sl, f"{s} / {w}")
            print(f"    - {s} / {w}: n={summ['n']} pnl={summ['total_pnl']:+.2f} "
                  f"win={summ['win_rate']*100:.1f}% t={summ['tstat']:+.2f}")


if __name__ == "__main__":
    main()
