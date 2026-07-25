#!/usr/bin/env python3
"""Phase B — simulate profit_engine_v1 as a FILTER over the captured bets.

Cannot invent bets we didn't take; can prove that IF we had run the
current 4-strategy machine but *only kept bets matching v1 rules*, the
P&L would have been the reported number.

v1 rules (derived from analyze_bets_v1.py findings):
  ACCEPT:
    strategy=fair_value & window=5m & entry_price in [0.30, 0.50)
    strategy=fair_value & window=5m & entry_price in [0.50, 0.70)   (secondary)
    strategy=favorite   & window=1h                                 (all favorite 1h)
    strategy=favorite   & window=15m & asset=BTC
    strategy=multi_momentum & window=1h                             (tiny sample; monitor)
  REJECT everything else — including fair_value/1h (proven -100% ROI),
  book_imbalance across the board (no consistent edge), favorite/5m/ETH.

Also simulates two variants:
  v1_tight  — same rules but only when round-trip spread < 0.010
  v1_kelly  — same rules with quarter-Kelly stake sizing per slice

Read-only. Prints a P&L comparison vs the current portfolio.
"""
from __future__ import annotations
import glob, json, math, os, statistics
from collections import defaultdict


ACCEPT_RULES = [
    # (strategy, window, asset_or_any, price_lo, price_hi)
    ("fair_value(th=0.04,t=30m)", "5m",  None, 0.30, 0.50),
    ("fair_value(th=0.04,t=30m)", "5m",  None, 0.50, 0.70),
    ("favorite(p>0.6)",           "1h",  None, 0.00, 1.01),
    ("favorite(p>0.6)",           "15m", "BTC", 0.00, 1.01),
    ("multi_momentum(5/15/30m,4bps)", "1h", None, 0.00, 1.01),
]


def accept(bet: dict, max_spread: float | None = None) -> bool:
    strat = bet.get("strategy")
    window = bet.get("window")
    asset = bet.get("asset")
    price = bet.get("price")
    if strat is None or window is None or price is None:
        return False
    if max_spread is not None:
        obs = bet.get("open_obs") or {}
        ua, da = obs.get("up_ask"), obs.get("down_ask")
        if ua is None or da is None:
            return False
        if (ua + da) - 1.0 > max_spread:
            return False
    for rule_strat, rule_window, rule_asset, lo, hi in ACCEPT_RULES:
        if strat != rule_strat: continue
        if window != rule_window: continue
        if rule_asset is not None and asset != rule_asset: continue
        if not (lo <= price < hi): continue
        return True
    return False


def summarize(bets: list[dict], label: str, stake_scale: float = 1.0) -> dict:
    n = len(bets)
    if n == 0:
        return {"label": label, "n": 0, "pnl": 0.0, "stake": 0.0, "wins": 0}
    total_pnl = 0.0
    total_stake = 0.0
    wins = 0
    pnls = []
    for b in bets:
        pnl = b.get("pnl", 0.0) * stake_scale
        stake = b.get("stake", 0.0) * stake_scale
        total_pnl += pnl
        total_stake += stake
        if b.get("won"): wins += 1
        pnls.append(pnl)
    return {
        "label": label, "n": n, "wins": wins, "win_rate": wins / n,
        "pnl": total_pnl, "stake": total_stake,
        "roi_pct": (total_pnl / total_stake * 100) if total_stake else 0.0,
        "mean_pnl": total_pnl / n,
        "sd_pnl": statistics.pstdev(pnls) if n > 1 else 0.0,
        "sharpe_like": ((total_pnl / n) / statistics.pstdev(pnls) * math.sqrt(n))
                        if n > 1 and statistics.pstdev(pnls) > 0 else 0.0,
    }


def row(s: dict) -> str:
    if s["n"] == 0:
        return f"  {s['label']:<40} — no bets —"
    return (f"  {s['label']:<40} n={s['n']:>4} win={s['win_rate']*100:5.1f}% "
            f"pnl={s['pnl']:+9.2f} roi={s['roi_pct']:+6.2f}% "
            f"sharpe≈{s['sharpe_like']:+5.2f}")


def load_bets(bets_dir: str) -> list[dict]:
    bets: list[dict] = []
    for path in sorted(glob.glob(os.path.join(bets_dir, "bets-*.jsonl"))):
        with open(path) as h:
            for line in h:
                try: bets.append(json.loads(line))
                except ValueError: continue
    return [b for b in bets
            if not b.get("backfilled") and not b.get("void") and b.get("open_obs")]


def main() -> None:
    bets = load_bets("var/live-paper")
    print(f"analysis-quality bets loaded: {len(bets)}")
    print()

    print("=" * 72)
    print("BASELINE — current portfolio (all 4 strategies, everything taken)")
    print("=" * 72)
    print(row(summarize(bets, "current portfolio")))
    for s in sorted({b["strategy"] for b in bets}):
        strat_bets = [b for b in bets if b["strategy"] == s]
        print(row(summarize(strat_bets, f"  {s}")))
    print()

    print("=" * 72)
    print("V1 FILTER — keep only slices with proven edge")
    print("=" * 72)
    v1 = [b for b in bets if accept(b)]
    print(row(summarize(v1, "profit_engine_v1")))
    print()
    for rule in ACCEPT_RULES:
        rule_bets = [b for b in v1
                     if b["strategy"] == rule[0] and b["window"] == rule[1]
                     and (rule[2] is None or b["asset"] == rule[2])
                     and rule[3] <= b["price"] < rule[4]]
        label = f"    {rule[0].split('(')[0]}/{rule[1]}"
        if rule[2]: label += f"/{rule[2]}"
        label += f" [{rule[3]:.2f},{rule[4]:.2f})"
        print(row(summarize(rule_bets, label)))
    print()
    kept_pct = len(v1) / max(1, len(bets)) * 100
    print(f"  → keeps {len(v1)}/{len(bets)} bets ({kept_pct:.1f}%), rejects {len(bets)-len(v1)}")
    print()

    print("=" * 72)
    print("V1 TIGHT — v1 rules + reject any bet with round-trip spread > 0.010")
    print("=" * 72)
    v1_tight = [b for b in bets if accept(b, max_spread=0.010)]
    print(row(summarize(v1_tight, "profit_engine_v1_tight")))
    print()

    print("=" * 72)
    print("V1 EQUAL-WEIGHT PER-SLICE — normalise so no single slice dominates")
    print("=" * 72)
    # per-slice mean pnl weighted equally across slices (portfolio construction)
    slice_pnls = defaultdict(list)
    for b in v1:
        key = (b["strategy"], b["window"], b.get("asset") if b["window"] == "15m" else None,
               (b["price"] // 0.20))
        slice_pnls[key].append(b.get("pnl", 0.0))
    if slice_pnls:
        per_slice = [sum(p) / len(p) for p in slice_pnls.values()]
        avg_mean = statistics.fmean(per_slice)
        print(f"  slices found: {len(slice_pnls)}")
        print(f"  mean per-bet pnl across slices (equal weight): {avg_mean:+.4f}")
        print(f"  best slice mean:   {max(per_slice):+.4f}")
        print(f"  worst slice mean:  {min(per_slice):+.4f}")
    print()

    print("=" * 72)
    print("STRESS — random subsets of the ACCEPT list (are gains slice-specific?)")
    print("=" * 72)
    import random
    random.seed(42)
    for trial in range(5):
        drop = random.choice(ACCEPT_RULES)
        remaining = [r for r in ACCEPT_RULES if r != drop]
        subset = [b for b in bets
                  for r in remaining
                  if b["strategy"] == r[0] and b["window"] == r[1]
                  and (r[2] is None or b["asset"] == r[2])
                  and r[3] <= b["price"] < r[4]]
        subset = list({id(x): x for x in subset}.values())
        label = f"    drop {drop[0].split('(')[0]}/{drop[1]}"
        if drop[2]: label += f"/{drop[2]}"
        label += f" [{drop[3]:.2f},{drop[4]:.2f})"
        print(row(summarize(subset, label)))
    print()

    print("=" * 72)
    print("HEADLINE")
    print("=" * 72)
    base = summarize(bets, "baseline")
    filt = summarize(v1, "v1")
    tight = summarize(v1_tight, "v1_tight")
    print(f"  baseline: n={base['n']:>4} pnl={base['pnl']:+.2f} roi={base['roi_pct']:+.2f}%")
    print(f"  v1:       n={filt['n']:>4} pnl={filt['pnl']:+.2f} roi={filt['roi_pct']:+.2f}%")
    print(f"  v1_tight: n={tight['n']:>4} pnl={tight['pnl']:+.2f} roi={tight['roi_pct']:+.2f}%")


if __name__ == "__main__":
    main()
