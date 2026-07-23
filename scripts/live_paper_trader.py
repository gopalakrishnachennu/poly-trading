#!/usr/bin/env python3
"""Live directional paper trader for the hourly BTC/ETH markets.

Watches the read-only projection gateway in real time, runs a strategy from
scripts/hourly_engine.py against the live tape, opens at most ONE simulated
position per hourly market, and settles it against the realised outcome when the
hour rolls over. State and a GUI-readable report are persisted to
``var/live-paper/``.

This is PAPER ONLY. It holds no credential, wallet, signer or transport, places
no order, and moves no money — it records what a strategy *would* have done on
live data. Wiring real execution is a separate, deliberate step you own.

Usage
-----
  python3 scripts/live_paper_trader.py                       # fair_value, $1000
  python3 scripts/live_paper_trader.py --strategy multi_momentum --principal 500
  python3 scripts/live_paper_trader.py --interval 10
"""
from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hourly_engine import (  # noqa: E402
    DOLLAR, HOUR_MS, Obs, BookImbalance, FairValue, Favorite, Momentum,
    MultiMomentum, VolRegime, VOL_MIN_STEP_MS, VOL_PRIOR_RATE, VOL_FLOOR_FRACTION,
    VOL_FULL_WEIGHT_MS, expiry_ms,
)

BUILDERS = {
    "fair_value": FairValue, "momentum": Momentum, "favorite": Favorite,
    "multi_momentum": MultiMomentum, "vol_regime": VolRegime,
    "book_imbalance": BookImbalance,
}


def series_variance_rate(samples: list[tuple[int, float]]) -> float:
    """Per-ms realised variance from (t, index_price) samples.

    Mirrors the engine estimator: sub-sample past feed quantisation, scale by
    real elapsed time, shrink toward the prior while history is short, floor it.
    """
    picked: list[tuple[int, float]] = []
    for point in sorted(samples):
        if not picked or point[0] - picked[-1][0] >= VOL_MIN_STEP_MS:
            picked.append(point)
    num = den = 0.0
    for (t0, s0), (t1, s1) in zip(picked, picked[1:]):
        dt = t1 - t0
        if dt <= 0 or s0 <= 0 or s1 <= 0:
            continue
        r = math.log(s1 / s0)
        num += r * r
        den += dt
    if den <= 0:
        return VOL_PRIOR_RATE
    span = picked[-1][0] - picked[0][0] if len(picked) > 1 else 0
    weight = min(1.0, span / VOL_FULL_WEIGHT_MS)
    blended = weight * (num / den) + (1 - weight) * VOL_PRIOR_RATE
    return max(blended, VOL_PRIOR_RATE * VOL_FLOOR_FRACTION)


def fetch(url: str, timeout: float):
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None


def to_obs(asset: dict, now_ms: int) -> Obs | None:
    try:
        up, down = asset["up_book"], asset["down_book"]

        def size(book, side):
            try:
                return float((book.get(side) or [{}])[0].get("quantity_micros", 0))
            except (IndexError, TypeError, ValueError):
                return 0.0

        return Obs(
            t=now_ms,
            s=float(asset["reference_price_micros"]),
            k=float(asset["target_price_micros"]),
            up_ask=int(up["best_ask_micros"]) / DOLLAR,
            up_bid=int(up["best_bid_micros"]) / DOLLAR,
            down_ask=int(down["best_ask_micros"]) / DOLLAR,
            down_bid=int(down["best_bid_micros"]) / DOLLAR,
            tau_ms=max(0.0, expiry_ms(now_ms) - now_ms),
            up_bid_sz=size(up, "bids"), up_ask_sz=size(up, "asks"),
            down_bid_sz=size(down, "bids"), down_ask_sz=size(down, "asks"),
        )
    except (KeyError, ValueError, TypeError):
        return None


class LiveTrader:
    def __init__(self, strategy, principal: float, out_dir: str, min_stake: float = 1.0):
        self.strategy = strategy
        self.principal = principal
        self.bankroll = principal
        self.out_dir = out_dir
        self.min_stake = min_stake
        self.started_at = int(time.time() * 1000)
        self.markets: dict[tuple, dict] = {}   # (asset, condition) -> live state
        self.index: list[tuple[int, float]] = []  # (t, price) for volatility
        self.bets: list[dict] = []
        os.makedirs(out_dir, exist_ok=True)
        self.state_path = os.path.join(out_dir, "state.json")
        self.report_path = os.path.join(out_dir, "report.json")
        self._load()

    # ---------------- persistence ----------------
    def _load(self):
        try:
            with open(self.state_path) as f:
                s = json.load(f)
            if s.get("strategy") != self.strategy.name:
                print("strategy changed; starting a fresh live book", flush=True)
                return
            self.bankroll = s.get("bankroll", self.principal)
            self.principal = s.get("principal", self.principal)
            self.bets = s.get("bets", [])
            self.index = [tuple(x) for x in s.get("index", [])][-4000:]
            self.started_at = s.get("started_at", self.started_at)
            print(f"resumed live book: ${self.bankroll:.2f} over {len(self.bets)} settled bets", flush=True)
        except (OSError, ValueError, KeyError):
            pass

    def _save(self):
        tmp = self.state_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump({
                "strategy": self.strategy.name, "principal": self.principal,
                "bankroll": self.bankroll, "bets": self.bets[-500:],
                "index": self.index[-4000:], "started_at": self.started_at,
            }, f)
        os.replace(tmp, self.state_path)

        wins = sum(1 for b in self.bets if b["won"])
        open_positions = [{
            "asset": m["asset"], "market": key[1][:10], "side": m["position"]["side"],
            "price": m["position"]["price"], "stake": m["position"]["stake"],
            "t": m["position"]["t"],
        } for key, m in self.markets.items() if m.get("position") and not m.get("settled")]
        report = {
            "generated_at_ms": int(time.time() * 1000),
            "paper_only": True, "live": True,
            "strategy": self.strategy.name,
            "principal": self.principal, "bankroll": round(self.bankroll, 4),
            "pnl": round(self.bankroll - self.principal, 4),
            "settled": len(self.bets), "wins": wins,
            "started_at_ms": self.started_at,
            "open_positions": open_positions,
            "bets": self.bets[-40:],
        }
        tmp = self.report_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump(report, f, indent=1)
        os.replace(tmp, self.report_path)

    # ---------------- trading ----------------
    def settle(self, key, market):
        """Resolve a finished hour: Up wins if the last index >= strike."""
        if market.get("settled") or not market["obs"]:
            return
        market["settled"] = True
        last = market["obs"][-1]
        up_wins = last.s >= market["strike"]
        position = market.get("position")
        if not position:
            return
        won = (position["side"] == "UP") == up_wins
        gross = position["shares"] if won else 0.0
        pnl = gross - position["stake"]
        self.bankroll += pnl
        self.bets.append({
            "t": position["t"], "settled_t": last.t, "asset": market["asset"],
            "market": key[1][:10], "side": position["side"],
            "price": round(position["price"], 4), "stake": round(position["stake"], 2),
            "won": won, "pnl": round(pnl, 4), "bankroll": round(self.bankroll, 4),
        })
        print(f"  settled {market['asset']} {position['side']} @ {position['price']:.3f} "
              f"-> {'WON' if won else 'LOST'} {pnl:+.2f} (bankroll ${self.bankroll:.2f})", flush=True)

    def tick(self, snapshot: dict):
        now_ms = int(time.time() * 1000)
        live_keys = set()
        for asset in snapshot.get("assets", []):
            obs = to_obs(asset, now_ms)
            if obs is None:
                continue
            key = (asset.get("asset"), asset.get("condition_id"))
            live_keys.add(key)
            market = self.markets.setdefault(key, {
                "asset": key[0], "strike": obs.k, "obs": [], "position": None, "settled": False,
            })
            market["obs"].append(obs)
            if len(market["obs"]) > 2000:
                market["obs"] = market["obs"][-2000:]
            self.index.append((obs.t, obs.s))

            if market["position"] or market["settled"]:
                continue
            self.strategy.reset(series_variance_rate(self.index))
            decision = self.strategy.decide(market["obs"], market)
            if not decision:
                continue
            side, frac, price = decision
            stake = min(self.bankroll, max(self.min_stake, frac * self.bankroll))
            if not (0 < price < 1) or stake < self.min_stake or stake > self.bankroll:
                continue
            market["position"] = {
                "side": side, "price": price, "stake": stake,
                "shares": stake / price, "t": obs.t,
            }
            print(f"  OPEN {key[0]} {side} @ {price:.3f} stake ${stake:.2f} "
                  f"({(obs.tau_ms/60000):.0f}m to expiry)", flush=True)

        # Any market that vanished from the projection has rolled over: settle it.
        for key, market in list(self.markets.items()):
            if key not in live_keys and not market.get("settled"):
                self.settle(key, market)
        # Drop settled markets once resolved, keeping memory bounded.
        for key, market in list(self.markets.items()):
            if market.get("settled"):
                self.markets.pop(key, None)
        self.index = self.index[-4000:]
        self._save()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default=os.environ.get("POLY_TERMINAL_API_URL", "http://127.0.0.1:8088") + "/api/v1/terminal/snapshot")
    ap.add_argument("--strategy", choices=sorted(BUILDERS), default="fair_value")
    ap.add_argument("--principal", type=float, default=1000.0)
    ap.add_argument("--interval", type=float, default=10.0)
    ap.add_argument("--timeout", type=float, default=4.0)
    ap.add_argument("--out-dir", default="var/live-paper")
    args = ap.parse_args()

    strategy = BUILDERS[args.strategy]()
    trader = LiveTrader(strategy, args.principal, args.out_dir)
    print(f"live paper trader: {strategy.name} | principal ${args.principal:.2f} | "
          f"polling {args.url} every {args.interval:g}s", flush=True)
    print("PAPER ONLY — no credential, order, or capital authority.", flush=True)

    skipped = 0
    while True:
        start = time.time()
        snapshot = fetch(args.url, args.timeout)
        if snapshot is not None and snapshot.get("mode") == "ready":
            trader.tick(snapshot)
        else:
            skipped += 1
            if skipped % 30 == 0:
                mode = snapshot.get("mode") if snapshot else "unreachable"
                print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] waiting "
                      f"(gateway {mode})", flush=True)
        time.sleep(max(0.0, args.interval - (time.time() - start)))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nstopped.")
