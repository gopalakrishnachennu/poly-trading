#!/usr/bin/env python3
"""Intraday momentum backtest: buy cheap early, SELL before resolution.

Tests the idea of trading the share-price path within the hour instead of
holding to settlement. Enter in the trend direction on recent momentum, then
exit early at a take-profit, a stop-loss, or a time limit — capturing the price
move (buy at $0.20, sell at $0.60) rather than the $1/$0 payoff.

This is a genuinely different P&L mechanism from the resolution-hold strategies:
here you pay the spread on BOTH the entry (buy at ask) and the exit (sell at
bid), and you never see the outcome. No look-ahead: entries and exits are taken
at the first qualifying observation walking forward in time.

Usage
-----
  python3 scripts/backtest_intraday.py
  python3 scripts/backtest_intraday.py --asset BTC --lookback 10 --min-move 0.03 \
      --take-profit 0.10 --stop-loss 0.10 --entry-min 40 --exit-min 5
  python3 scripts/backtest_intraday.py --fade     # test the opposite (mean reversion)
"""
from __future__ import annotations

import argparse
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hourly_engine import load_markets  # noqa: E402


def up_mid(o):
    return (o.up_bid + o.up_ask) / 2


def run(markets, entry_min, exit_min, lookback_min, min_move, tp, sl, fade):
    trades = []
    for m in markets:
        obs = m.obs
        entered = None
        # 1) find an entry: first obs inside the entry window with a momentum signal
        for i, o in enumerate(obs):
            tau_min = o.tau_ms / 60000
            if tau_min > entry_min or tau_min < exit_min:
                continue
            past = [x for x in obs[:i + 1] if o.t - x.t >= lookback_min * 60000]
            if not past:
                continue
            move = up_mid(o) - up_mid(past[-1])       # recent move in the UP price
            if abs(move) < min_move:
                continue
            up_trend = move > 0
            long_up = up_trend if not fade else not up_trend
            if long_up and 0 < o.up_ask < 1:
                entered = (i, "UP", o.up_ask)
            elif (not long_up) and 0 < o.down_ask < 1:
                entered = (i, "DOWN", o.down_ask)
            if entered:
                break
        if not entered:
            continue

        i, side, entry_ask = entered
        exit_price = None
        # 2) walk forward to the first exit trigger
        for o in obs[i + 1:]:
            value = (o.up_bid if side == "UP" else o.down_bid)   # what we could sell for
            if value <= 0:
                continue
            if value - entry_ask >= tp or value - entry_ask <= -sl or o.tau_ms / 60000 <= exit_min:
                exit_price = value
                break
        if exit_price is None:  # never triggered: exit at the last available bid
            last = obs[-1]
            exit_price = last.up_bid if side == "UP" else last.down_bid
        pnl = exit_price - entry_ask               # dollars per share
        ret = pnl / entry_ask if entry_ask > 0 else 0.0
        trades.append((m.asset, side, entry_ask, exit_price, pnl, ret))
    return trades


def report(name, trades, principal, frac):
    if not trades:
        print(f"  {name:34} no trades")
        return
    wins = sum(1 for t in trades if t[4] > 0)
    pnl_share = sum(t[4] for t in trades)
    # Stake a fixed fraction of a fixed bankroll per trade (illustrative sizing).
    stake = principal * frac
    total = sum(stake * t[5] for t in trades)
    avg = pnl_share / len(trades)
    print(f"  {name:34} trades {len(trades):4d}  win {100*wins/len(trades):5.1f}%  "
          f"avg {avg:+.4f}/share  P&L ${total:+8.2f}  (${stake:.0f}/trade)")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--glob", action="append")
    ap.add_argument("--asset", choices=["BTC", "ETH"])
    ap.add_argument("--min-span-min", type=float, default=20.0)
    ap.add_argument("--entry-min", type=float, default=40.0, help="enter within this many minutes to expiry")
    ap.add_argument("--exit-min", type=float, default=5.0, help="force exit at this many minutes to expiry")
    ap.add_argument("--lookback", type=float, default=10.0, help="minutes to measure recent momentum")
    ap.add_argument("--min-move", type=float, default=0.03, help="min recent price move to trigger entry")
    ap.add_argument("--take-profit", type=float, default=0.10)
    ap.add_argument("--stop-loss", type=float, default=0.10)
    ap.add_argument("--principal", type=float, default=1000.0)
    ap.add_argument("--frac", type=float, default=0.05, help="fraction of bankroll staked per trade")
    ap.add_argument("--fade", action="store_true", help="test mean-reversion (fade the move) instead")
    args = ap.parse_args()

    patterns = args.glob or ["var/paper-campaign/paper-*.jsonl", "var/research-capture/*.jsonl"]
    paths = sorted({p for pat in patterns for p in glob.glob(pat)})
    markets = load_markets(paths, asset_filter=args.asset, min_span_min=args.min_span_min)
    if not markets:
        print("No usable markets. Capture first.")
        return

    print("# Intraday momentum backtest — buy early, sell before resolution\n")
    print(f"Markets: {len(markets)}   enter <= {args.entry_min:g}m, exit >= {args.exit_min:g}m to expiry")
    print(f"Signal: {args.lookback:g}m momentum > {args.min_move} | TP {args.take_profit} / SL {args.stop_loss}")
    print(f"You pay the spread on entry (ask) AND exit (bid).\n")

    trades = run(markets, args.entry_min, args.exit_min, args.lookback,
                 args.min_move, args.take_profit, args.stop_loss, args.fade)
    report("MOMENTUM (ride the move)" if not args.fade else "REVERSION (fade the move)",
           trades, args.principal, args.frac)

    # Robustness: does the sign survive different take-profit / stop-loss / lookback?
    print("\n## Robustness sweep (momentum)")
    for lb in (5.0, 10.0, 20.0):
        for tp in (0.05, 0.10, 0.20):
            t = run(markets, args.entry_min, args.exit_min, lb, args.min_move, tp, tp, False)
            if t:
                total = sum(args.principal * args.frac * x[5] for x in t)
                print(f"  lookback {lb:>4g}m  TP/SL {tp:.2f}   trades {len(t):3d}   P&L ${total:+8.2f}")
    print(f"\n  {len(markets)} markets is far too few to trust any single cell; look for the")
    print("  sign being stable across the whole grid, not one good number.")


if __name__ == "__main__":
    main()
