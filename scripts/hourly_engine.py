#!/usr/bin/env python3
"""Hourly BTC/ETH paper betting engine.

A research/paper engine for the hourly binary markets. It replays captured
observations, lets a pluggable strategy decide *when* and *which side* to bet
within each hour, sizes the stake from a simulated bankroll, holds to
resolution, and reports P&L. This is PAPER ONLY — simulated money, no order is
ever submitted. Use it to train and compare strategies before anything real.

Market model
------------
Each hour is a binary market. A complete set (Up + Down) redeems for $1.00.
"Up" wins if the reference (index) price is at/above the strike at the hour
boundary. Prices are fixed-point micros (1_000_000 = $1.00).

Betting Up with stake S at ask price p (dollars, 0..1):
  shares = S / p ; if Up wins each share pays $1.
  net = S*(1-p)/p on a win, or -S on a loss.
Expected value with true win prob q is S*(q - p)/p — positive iff q > p, i.e.
only bet a side when your model thinks it is *underpriced*.

No look-ahead: at time t a strategy sees only observations up to t. Volatility
for the fair-value model is estimated walk-forward from strictly earlier hours.

Usage
-----
  python3 scripts/hourly_engine.py                         # all strategies
  python3 scripts/hourly_engine.py --glob 'var/research-capture/*.jsonl'
  python3 scripts/hourly_engine.py --principal 1000 --strategy fair_value
  python3 scripts/hourly_engine.py --asset BTC --fee 0.0
"""
from __future__ import annotations

import argparse
import glob
import json
import math
import statistics
from collections import defaultdict
from dataclasses import dataclass, field

DOLLAR = 1_000_000
HOUR_MS = 3_600_000


def phi(x: float) -> float:
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))


@dataclass
class Obs:
    t: int
    s: float            # reference / index price (micros)
    k: float            # strike (micros)
    up_ask: float       # dollars (0..1)
    up_bid: float
    down_ask: float
    down_bid: float
    tau_ms: float       # time to hour expiry at this observation


@dataclass
class Market:
    asset: str
    condition: str
    obs: list           # chronological Obs
    strike: float
    up_wins: bool       # realized outcome
    start_t: int


@dataclass
class Bet:
    market: str
    asset: str
    side: str           # "UP" or "DOWN"
    price: float        # entry ask (dollars)
    stake: float        # dollars risked
    shares: float
    won: bool
    pnl: float
    t_enter: int


# --------------------------------------------------------------------------- #
# Data loading
# --------------------------------------------------------------------------- #
def expiry_ms(t: int) -> int:
    return (t // HOUR_MS + 1) * HOUR_MS


def load_markets(paths, asset_filter=None, min_span_min=20.0):
    raw = defaultdict(list)
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
                a = p.get("asset")
                if asset_filter and a != asset_filter:
                    continue
                try:
                    o = (int(r["event_time_ms"]), int(p["reference_price_micros"]),
                         int(p["target_price_micros"]),
                         int(p["up_best_ask_micros"]), int(p["up_best_bid_micros"]),
                         int(p["down_best_ask_micros"]), int(p["down_best_bid_micros"]))
                except (KeyError, ValueError):
                    continue
                raw[(a, p.get("condition_id"))].append(o)

    markets = []
    for (asset, cond), rows in raw.items():
        rows.sort()
        if (rows[-1][0] - rows[0][0]) / 60000 < min_span_min:
            continue
        strike = statistics.mode([r[2] for r in rows])
        obs = [Obs(t=r[0], s=r[1], k=r[2],
                   up_ask=r[3] / DOLLAR, up_bid=r[4] / DOLLAR,
                   down_ask=r[5] / DOLLAR, down_bid=r[6] / DOLLAR,
                   tau_ms=max(0.0, expiry_ms(r[0]) - r[0])) for r in rows]
        markets.append(Market(asset=asset, condition=cond, obs=obs, strike=strike,
                              up_wins=(rows[-1][1] >= strike), start_t=rows[0][0]))
    markets.sort(key=lambda m: m.start_t)  # chronological for walk-forward
    return markets


# Volatility estimation. The index is sampled at ~1s and prints in round units,
# so most tick-to-tick returns are exactly zero from quantisation; a naive
# estimate collapses toward zero and makes the model absurdly confident near
# expiry. Sub-sample to a coarse spacing, scale by real elapsed time, shrink
# toward a typical hourly prior while history is short, and hard-floor it.
VOL_MIN_STEP_MS = 20_000
VOL_PRIOR_HOURLY = 0.004          # ~0.4% per hour, typical for BTC/ETH
VOL_PRIOR_RATE = (VOL_PRIOR_HOURLY ** 2) / HOUR_MS
VOL_FULL_WEIGHT_MS = 20 * 60_000
VOL_FLOOR_FRACTION = 0.25


def variance_rate(markets) -> float:
    """Per-ms realized variance of index log returns, de-noised and shrunk."""
    num = den = span = 0.0
    for m in markets:
        picked = []
        for o in m.obs:
            if not picked or o.t - picked[-1].t >= VOL_MIN_STEP_MS:
                picked.append(o)
        for a, b in zip(picked, picked[1:]):
            dt = b.t - a.t
            if dt <= 0 or a.s <= 0 or b.s <= 0:
                continue
            rr = math.log(b.s / a.s)
            num += rr * rr
            den += dt
        if m.obs:
            span += m.obs[-1].t - m.obs[0].t
    if den <= 0:
        return VOL_PRIOR_RATE
    weight = min(1.0, span / VOL_FULL_WEIGHT_MS)
    blended = weight * (num / den) + (1 - weight) * VOL_PRIOR_RATE
    return max(blended, VOL_PRIOR_RATE * VOL_FLOOR_FRACTION)


# --------------------------------------------------------------------------- #
# Strategies — return None to wait, or (side, stake_fraction, price) to enter.
# --------------------------------------------------------------------------- #
class Strategy:
    name = "base"

    def reset(self, variance_rate: float):
        self.v = variance_rate

    def decide(self, hist, market):
        """hist: Obs list up to now. Return (side, frac, price) or None."""
        raise NotImplementedError


class FairValue(Strategy):
    """Bet the side the fair-value model thinks is underpriced by > threshold.

    P(Up) = Phi(ln(S/K) / (sigma*sqrt(tau))). Stake via fractional Kelly.
    Only decides once, in a window around `decision_min` minutes to expiry.
    """
    def __init__(self, threshold=0.04, decision_min=30.0, kelly=0.25, max_frac=0.05):
        self.threshold, self.decision_min = threshold, decision_min
        self.kelly, self.max_frac = kelly, max_frac
        self.name = f"fair_value(th={threshold},t={decision_min:g}m)"

    def decide(self, hist, market):
        o = hist[-1]
        # act near the chosen decision time
        if abs(o.tau_ms - self.decision_min * 60000) > 45000:
            return None
        if self.v <= 0 or o.tau_ms <= 0 or o.s <= 0 or o.k <= 0:
            return None
        # Clamp: never claim near-certainty the estimate cannot support.
        p_up = min(0.99, max(0.01, phi(math.log(o.s / o.k) / math.sqrt(self.v * o.tau_ms))))
        # buy Up if model prob beats the Up ask; else Down
        edge_up = p_up - o.up_ask
        edge_down = (1 - p_up) - o.down_ask
        if edge_up >= edge_down and edge_up > self.threshold:
            return ("UP", self._kelly(p_up, o.up_ask), o.up_ask)
        if edge_down > self.threshold:
            return ("DOWN", self._kelly(1 - p_up, o.down_ask), o.down_ask)
        return None

    def _kelly(self, q, price):
        b = (1 - price) / price if price > 0 else 0
        f = (b * q - (1 - q)) / b if b > 0 else 0
        return max(0.0, min(self.max_frac, self.kelly * f))


class Momentum(Strategy):
    """Bet the direction the index moved over the last `lookback` minutes,
    provided it is on the favorable side of the strike. Fixed fraction."""
    def __init__(self, lookback_min=10.0, min_move_bps=5.0, frac=0.02, decision_min=25.0):
        self.lookback = lookback_min * 60000
        self.min_move = min_move_bps / 10000.0
        self.frac, self.decision_min = frac, decision_min
        self.name = f"momentum(lb={lookback_min:g}m,mv={min_move_bps:g}bps)"

    def decide(self, hist, market):
        o = hist[-1]
        if abs(o.tau_ms - self.decision_min * 60000) > 45000:
            return None
        past = [h for h in hist if o.t - h.t >= self.lookback]
        if not past:
            return None
        ref = past[-1].s
        if ref <= 0:
            return None
        move = (o.s - ref) / ref
        if move > self.min_move and o.s >= o.k:
            return ("UP", self.frac, o.up_ask)
        if move < -self.min_move and o.s < o.k:
            return ("DOWN", self.frac, o.down_ask)
        return None


class Favorite(Strategy):
    """Baseline: back the market's favorite (the side priced most likely to win,
    i.e. the higher ask) once, near the decision time. Tests whether simply
    following the market's own probability pays after the spread."""
    def __init__(self, min_prob=0.60, frac=0.02, decision_min=20.0):
        self.min_prob, self.frac, self.decision_min = min_prob, frac, decision_min
        self.name = f"favorite(p>{min_prob})"

    def decide(self, hist, market):
        o = hist[-1]
        if abs(o.tau_ms - self.decision_min * 60000) > 45000:
            return None
        # The favorite is the more expensive side (higher implied win prob).
        if o.up_ask >= self.min_prob and o.up_ask > o.down_ask:
            return ("UP", self.frac, o.up_ask)
        if o.down_ask >= self.min_prob and o.down_ask > o.up_ask:
            return ("DOWN", self.frac, o.down_ask)
        return None


# --------------------------------------------------------------------------- #
# Paper engine
# --------------------------------------------------------------------------- #
@dataclass
class Result:
    strategy: str
    principal: float
    bankroll: float
    bets: list = field(default_factory=list)
    equity: list = field(default_factory=list)  # bankroll after each bet

    @property
    def n(self):
        return len(self.bets)

    def report(self) -> str:
        if not self.bets:
            return f"{self.strategy:34}  no bets taken"
        wins = sum(1 for b in self.bets if b.won)
        pnl = self.bankroll - self.principal
        ret = 100 * pnl / self.principal
        peak = self.principal
        max_dd = 0.0
        for e in self.equity:
            peak = max(peak, e)
            max_dd = max(max_dd, (peak - e) / peak)
        avg = pnl / self.n
        return (f"{self.strategy:34}  bets {self.n:4d}  win {100*wins/self.n:5.1f}%  "
                f"P&L ${pnl:+8.2f} ({ret:+6.1f}%)  end ${self.bankroll:8.2f}  "
                f"maxDD {100*max_dd:4.1f}%  avg/bet ${avg:+.3f}")


def run(markets, strategy, principal, fee, walk_forward, min_stake=1.0, prior_seed=None):
    res = Result(strategy=strategy.name, principal=principal, bankroll=principal)
    prior = list(prior_seed) if prior_seed else []
    for m in markets:
        strategy.reset(variance_rate(prior) if walk_forward else variance_rate(markets))
        # replay the hour; strategy may enter once
        entered = None
        for i in range(len(m.obs)):
            decision = strategy.decide(m.obs[:i + 1], m)
            if decision:
                entered = (decision, m.obs[i])
                break
        if entered:
            (side, frac, price), o = entered
            stake = min(res.bankroll, max(min_stake, frac * res.bankroll))
            if 0 < price < 1 and stake >= min_stake and stake <= res.bankroll:
                shares = stake / price
                won = (side == "UP" and m.up_wins) or (side == "DOWN" and not m.up_wins)
                gross = shares if won else 0.0
                pnl = gross - stake - fee * stake
                res.bankroll += pnl
                res.bets.append(Bet(market=m.condition[:10], asset=m.asset, side=side,
                                    price=price, stake=stake, shares=shares, won=won,
                                    pnl=pnl, t_enter=o.t))
                res.equity.append(res.bankroll)
        prior.append(m)
    return res


def default_strategies():
    return [
        FairValue(threshold=0.04, decision_min=30.0),
        FairValue(threshold=0.02, decision_min=15.0),
        Momentum(lookback_min=10.0, min_move_bps=5.0),
        Favorite(min_prob=0.60),
    ]


# --------------------------------------------------------------------------- #
# Training — grid search with a chronological train/test split. The point is
# NOT to find the biggest in-sample number (that is overfitting); it is to see
# whether a config that wins on the training hours *also* wins on unseen hours.
# --------------------------------------------------------------------------- #
import itertools  # noqa: E402

GRIDS = {
    "fair_value": {
        "threshold": [0.02, 0.03, 0.04, 0.05, 0.06],
        "decision_min": [45.0, 30.0, 15.0, 10.0],
        "kelly": [0.25, 0.5],
    },
    "momentum": {
        "lookback_min": [5.0, 10.0, 15.0, 20.0],
        "min_move_bps": [3.0, 5.0, 10.0, 20.0],
        "decision_min": [30.0, 20.0, 10.0],
    },
    "favorite": {
        "min_prob": [0.55, 0.60, 0.70, 0.80],
        "decision_min": [30.0, 20.0, 10.0],
    },
}
BUILDERS = {"fair_value": FairValue, "momentum": Momentum, "favorite": Favorite}


def build(family, params):
    return BUILDERS[family](**params)


def train(markets, family, train_frac, principal, fee):
    split = max(1, int(len(markets) * train_frac))
    train_m, test_m = markets[:split], markets[split:]
    if not test_m:
        print("Not enough markets to hold out a test set.")
        return

    grid = GRIDS[family]
    keys = list(grid)
    rows = []
    for values in itertools.product(*(grid[k] for k in keys)):
        params = dict(zip(keys, values))
        tr = run(train_m, build(family, params), principal, fee, walk_forward=True)
        # test uses the train hours as prior history for the volatility estimate
        te = run(test_m, build(family, params), principal, fee,
                 walk_forward=True, prior_seed=train_m)
        rows.append((params, tr, te))

    # Rank by training P&L (that is what "training" would pick).
    rows.sort(key=lambda r: r[1].bankroll, reverse=True)
    print(f"## Training: {family}  ({len(train_m)} train / {len(test_m)} test hours, "
          f"{len(rows)} configs)\n")
    print(f"  {'params':44} {'train P&L':>12} {'test P&L':>12}  verdict")
    for params, tr, te in rows[:10]:
        tr_pnl = tr.bankroll - principal
        te_pnl = te.bankroll - principal
        if tr_pnl > 0 and te_pnl > 0:
            verdict = "holds out"
        elif tr_pnl > 0 and te_pnl <= 0:
            verdict = "OVERFIT"
        else:
            verdict = "weak"
        ps = ",".join(f"{k}={v}" for k, v in params.items())
        print(f"  {ps:44} {tr_pnl:>+11.2f} {te_pnl:>+11.2f}  {verdict} "
              f"({tr.n}/{te.n} bets)")

    best = rows[0]
    holds = sum(1 for _, tr, te in rows
                if tr.bankroll > principal and te.bankroll > principal)
    print(f"\n  Best-on-train config: {best[0]}")
    print(f"  Its out-of-sample P&L: ${best[2].bankroll - principal:+.2f}")
    print(f"  Configs green on BOTH train and test: {holds}/{len(rows)}")
    print("\n  Reminder: with few hours this is noise. A real edge shows up as many")
    print("  configs holding out on hundreds of markets, not one lucky in-sample peak.")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--glob", action="append",
                    help="observation JSONL glob(s); repeatable. "
                         "Default: paper-campaign + research-capture.")
    ap.add_argument("--asset", choices=["BTC", "ETH"], help="restrict to one asset")
    ap.add_argument("--principal", type=float, default=1000.0)
    ap.add_argument("--fee", type=float, default=0.0, help="taker fee as fraction of stake")
    ap.add_argument("--min-span-min", type=float, default=20.0)
    ap.add_argument("--no-walk-forward", action="store_true")
    ap.add_argument("--strategy", choices=["fair_value", "momentum", "favorite", "all"],
                    default="all")
    ap.add_argument("--train", choices=["fair_value", "momentum", "favorite"],
                    help="grid-search a strategy family with a train/test split")
    ap.add_argument("--train-frac", type=float, default=0.6,
                    help="chronological fraction of markets used for training")
    args = ap.parse_args()

    patterns = args.glob or [
        "var/paper-campaign/paper-*.jsonl",
        "var/research-capture/*.jsonl",
    ]
    paths = sorted({p for pat in patterns for p in glob.glob(pat)})
    if not paths:
        print("No observation files found. Run capture first "
              "(scripts/run-continuous-capture.sh).")
        return

    markets = load_markets(paths, asset_filter=args.asset, min_span_min=args.min_span_min)
    if not markets:
        print("No usable markets in the data yet.")
        return

    wf = not args.no_walk_forward
    assets = sorted({m.asset for m in markets})
    print("# Hourly paper betting engine\n")
    print(f"Markets: {len(markets)}  assets: {assets}  principal: ${args.principal:.2f}  "
          f"fee: {args.fee:.3%}  vol: {'walk-forward' if wf else 'in-sample'}\n")

    if args.train:
        train(markets, args.train, args.train_frac, args.principal, args.fee)
        return

    strategies = default_strategies()
    if args.strategy != "all":
        strategies = [s for s in strategies if s.name.startswith(args.strategy)]
    print("## Strategy results (paper only — no real orders)")
    for strat in strategies:
        print("  " + run(markets, strat, args.principal, args.fee, wf).report())

    print(f"\nStatistical power: {len(markets)} independent markets. "
          f"{'Illustrative only — need hundreds for a real verdict.' if len(markets) < 200 else ''}")
    print("Grow the sample with scripts/run-continuous-capture.sh, then re-run. "
          "Paper only; wiring real execution is a separate, deliberate step you own.")


if __name__ == "__main__":
    main()
