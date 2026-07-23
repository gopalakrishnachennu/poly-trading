#!/usr/bin/env python3
"""Directional fair-value model + walk-forward-style backtest for hourly markets.

Read-only research tool. It builds a fair-value probability for the "Up" leg of
each hourly binary market from the live index price vs the hour strike, and
compares that model probability against the market's own prices to look for a
directional edge the complete-set arbitrage detector does not use.

Model
-----
Each hourly market resolves "Up" if the index price at the hour boundary is at
or above the strike ``K`` (the hour-open price). For an observation at time ``t``
with current index ``S`` and time-to-expiry ``tau`` (ms), model the log return to
expiry as zero-drift Gaussian with per-ms variance ``v`` (estimated per asset
from the realized index series):

    sigma_tau = sqrt(v * tau)
    P(Up) = Phi( ln(S / K) / sigma_tau )      # Phi = standard normal CDF

The market's implied Up probability is the Up mid price (micros / 1e6). A taker
"edge" exists when the model probability exceeds the ask (buy Up) or is below the
bid (sell Up) by more than a threshold.

Outcome / P&L
-------------
Realized outcome per market = (last observed index >= strike). A simulated taker
that buys the mispriced leg at the ask pays ``ask`` and receives ``1.0`` if that
leg wins, ``0`` otherwise. We take at most ONE position per market (the
independent unit is the market, not the tick) at a fixed decision point.

IMPORTANT: statistical power is bounded by the number of distinct markets, not
observations. With only a handful of hourly markets this is illustrative, not a
validation. See docs/EDGE_ANALYSIS.md.
"""
from __future__ import annotations

import argparse
import glob
import json
import math
import statistics
from collections import defaultdict

DOLLAR = 1_000_000
HOUR_MS = 3_600_000


def phi(x: float) -> float:
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))


def load(paths: list[str]):
    """Return markets: {(asset, condition): [obs...]} and per-asset index series."""
    markets: dict[tuple, list[dict]] = defaultdict(list)
    series: dict[str, list[tuple[int, float]]] = defaultdict(list)
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
                    o = {
                        "t": int(r["event_time_ms"]),
                        "S": int(p["reference_price_micros"]),
                        "K": int(p["target_price_micros"]),
                        "ua": int(p["up_best_ask_micros"]),
                        "ub": int(p["up_best_bid_micros"]),
                        "da": int(p["down_best_ask_micros"]),
                        "db": int(p["down_best_bid_micros"]),
                    }
                except (KeyError, ValueError):
                    continue
                asset = p.get("asset", "?")
                markets[(asset, p.get("condition_id"))].append(o)
                series[asset].append((o["t"], o["S"]))
    return markets, series


def variance_rate(series: list[tuple[int, float]]) -> float:
    """Per-ms realized variance of log index returns (robust to uneven spacing)."""
    s = sorted(series)
    num = 0.0
    den = 0.0
    for (t0, s0), (t1, s1) in zip(s, s[1:]):
        dt = t1 - t0
        if dt <= 0 or s0 <= 0 or s1 <= 0:
            continue
        r = math.log(s1 / s0)
        num += r * r
        den += dt
    return (num / den) if den > 0 else 0.0


def expiry_ms(t: int) -> int:
    return (t // HOUR_MS + 1) * HOUR_MS


def build_markets(paths: list[str], min_span_min: float):
    """Return chronologically-ordered usable markets with derived fields."""
    markets, _ = load(paths)
    rows = []
    for (asset, cond), obs in markets.items():
        obs.sort(key=lambda o: o["t"])
        span_min = (obs[-1]["t"] - obs[0]["t"]) / 60000
        if span_min < min_span_min:
            continue  # skip stub markets with almost no life
        strike = statistics.mode([o["K"] for o in obs])
        close = obs[-1]["S"]
        rows.append({
            "asset": asset, "cond": cond, "obs": obs,
            "strike": strike, "outcome_up": 1 if close >= strike else 0,
            "start": obs[0]["t"],
        })
    rows.sort(key=lambda r: r["start"])  # chronological for walk-forward
    return rows


def evaluate(rows, decision_tau_min: float, threshold: float, walk_forward: bool):
    """Return (calib, trades). Volatility is estimated out-of-sample when
    walk_forward is set: only index samples from strictly earlier markets feed
    the per-asset variance-rate estimate for each market."""
    prior_series: dict[str, list] = defaultdict(list)
    full_rate = {}
    if not walk_forward:
        agg: dict[str, list] = defaultdict(list)
        for r in rows:
            agg[r["asset"]].extend((o["t"], o["S"]) for o in r["obs"])
        full_rate = {a: variance_rate(s) for a, s in agg.items()}

    calib, trades = [], []
    for r in rows:
        asset, strike, outcome_up = r["asset"], r["strike"], r["outcome_up"]
        if walk_forward:
            v = variance_rate(prior_series[asset]) if prior_series[asset] else 0.0
        else:
            v = full_rate.get(asset, 0.0)

        pick = min(r["obs"], key=lambda o: abs((expiry_ms(o["t"]) - o["t"]) - decision_tau_min * 60000))
        tau = expiry_ms(pick["t"]) - pick["t"]
        S = pick["S"]
        if v > 0 and tau > 0 and S > 0 and strike > 0:
            sigma_tau = math.sqrt(v * tau)
            p_up = phi(math.log(S / strike) / sigma_tau)
            up_mid = (pick["ua"] + pick["ub"]) / 2 / DOLLAR
            calib.append((p_up, up_mid, outcome_up))
            ua, da = pick["ua"] / DOLLAR, pick["da"] / DOLLAR
            p_down = 1 - p_up
            if p_up - ua > threshold and (p_up - ua) >= (p_down - da):
                trades.append((asset, r["cond"][:10], "BUY_UP", round(p_up, 3),
                               round(up_mid, 3), outcome_up, round(outcome_up - ua, 4)))
            elif p_down - da > threshold:
                trades.append((asset, r["cond"][:10], "BUY_DOWN", round(p_up, 3),
                               round(up_mid, 3), outcome_up, round((1 - outcome_up) - da, 4)))

        # This market's ticks become training data for later markets.
        prior_series[asset].extend((o["t"], o["S"]) for o in r["obs"])
    return calib, trades


def backtest(paths: list[str], decision_tau_min: float, threshold: float,
             min_span_min: float, walk_forward: bool) -> None:
    rows = build_markets(paths, min_span_min)
    usable = len(rows)

    print("# Directional fair-value backtest\n")
    mode = "walk-forward (out-of-sample volatility)" if walk_forward else "in-sample volatility"
    print(f"Volatility mode: {mode}")
    calib, trades = evaluate(rows, decision_tau_min, threshold, walk_forward)
    print(f"Usable markets (span >= {min_span_min:.0f} min): {usable}")
    print(f"Decision point: ~{decision_tau_min:.0f} min to expiry, taker threshold {threshold:.3f}\n")

    # Calibration: does model / market probability track realized outcomes?
    print("## Calibration (independent unit = market)")
    if calib:
        n = len(calib)
        up_rate = sum(o for _, _, o in calib) / n
        brier_model = sum((p - o) ** 2 for p, _, o in calib) / n
        brier_market = sum((m - o) ** 2 for _, m, o in calib) / n
        # Directional agreement with realized outcome.
        model_dir = sum(1 for p, _, o in calib if (p >= 0.5) == (o == 1)) / n
        market_dir = sum(1 for _, m, o in calib if (m >= 0.5) == (o == 1)) / n
        print(f"  markets: {n}   realized Up rate: {up_rate:.2%}")
        print(f"  Brier score  model: {brier_model:.4f}   market-mid: {brier_market:.4f}  (lower is better)")
        print(f"  directional hit  model: {model_dir:.2%}   market-mid: {market_dir:.2%}")
    print()

    print("## Simulated taker trades (<=1 per market)")
    if trades:
        total = sum(t[-1] for t in trades)
        wins = sum(1 for t in trades if t[-1] > 0)
        print(f"  {'asset':5} {'cond':11} {'action':9} {'P(Up)':>6} {'mkt':>6} {'out':>3} {'pnl':>8}")
        for t in trades:
            print(f"  {t[0]:5} {t[1]:11} {t[2]:9} {t[3]:6.3f} {t[4]:6.3f} {t[5]:>3} {t[6]:8.4f}")
        print(f"\n  trades: {len(trades)}   winners: {wins}   total PnL: ${total:.4f}   "
              f"avg/trade: ${total/len(trades):.4f}")
    else:
        print("  No trades cleared the threshold.")
    print()
    print("## Statistical power")
    print(f"  Independent samples = usable markets = {usable}.")
    if usable < 200:
        print(f"  This is far too few to validate or reject an edge; the numbers above are")
        print(f"  illustrative of the framework only. Validation needs hundreds+ of hourly")
        print(f"  markets (weeks of capture across regimes). See docs/EDGE_ANALYSIS.md and")
        print(f"  scripts/capture_progress.py.")
    elif usable < 500:
        print(f"  Enough for a first read, not yet firm. Keep capturing toward ~500 markets.")
    else:
        print(f"  Enough markets for a meaningful read. Judge model vs market-mid Brier.")


def sweep(paths: list[str], min_span_min: float, walk_forward: bool) -> None:
    rows = build_markets(paths, min_span_min)
    print("\n## Sensitivity sweep (decision minutes-to-expiry x threshold)")
    print(f"  usable markets: {len(rows)}   (each cell: trades / total PnL / model Brier)")
    taus = [45, 30, 15, 5]
    thrs = [0.02, 0.03, 0.05, 0.08]
    header = "  tau\\thr " + "".join(f"{t:>16.2f}" for t in thrs)
    print(header)
    for tau in taus:
        cells = []
        for thr in thrs:
            calib, trades = evaluate(rows, tau, thr, walk_forward)
            pnl = sum(t[-1] for t in trades)
            brier = (sum((p - o) ** 2 for p, _, o in calib) / len(calib)) if calib else float("nan")
            cells.append(f"{len(trades):>3}/{pnl:+.2f}/{brier:.3f}")
        print(f"  {tau:>5}m  " + "".join(f"{c:>16}" for c in cells))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--glob", default="var/paper-campaign/paper-*.jsonl")
    ap.add_argument("--decision-tau-min", type=float, default=30.0,
                    help="minutes-to-expiry at which to take the decision")
    ap.add_argument("--threshold", type=float, default=0.03,
                    help="minimum model-minus-ask edge to trade")
    ap.add_argument("--min-span-min", type=float, default=20.0,
                    help="skip markets observed for fewer than this many minutes")
    ap.add_argument("--walk-forward", action="store_true",
                    help="estimate volatility out-of-sample from earlier markets only")
    ap.add_argument("--sweep", action="store_true",
                    help="also print a decision-time x threshold sensitivity grid")
    args = ap.parse_args()
    paths = sorted(glob.glob(args.glob))
    if not paths:
        print(f"No files matched {args.glob!r}.")
        return
    backtest(paths, args.decision_tau_min, args.threshold, args.min_span_min, args.walk_forward)
    if args.sweep:
        sweep(paths, args.min_span_min, args.walk_forward)


if __name__ == "__main__":
    main()
