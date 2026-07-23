#!/usr/bin/env python3
"""Live directional paper trader for the hourly BTC/ETH markets.

Watches the read-only projection gateway in real time and runs one or more
strategies from scripts/hourly_engine.py side by side. Each strategy keeps its
own independent book (bankroll, open positions, settled bets) over the same
live tape, so they can be compared honestly. Each book opens at most ONE
simulated position per hourly market and settles it against the realised
outcome when the hour rolls over.

This is PAPER ONLY. It holds no credential, wallet, signer or transport, places
no order, and moves no money — it records what each strategy *would* have done
on live data. Wiring real execution is a separate, deliberate step you own.

Usage
-----
  python3 scripts/live_paper_trader.py                       # a sensible slate
  python3 scripts/live_paper_trader.py --strategy all
  python3 scripts/live_paper_trader.py --strategy favorite,multi_momentum
  python3 scripts/live_paper_trader.py --principal 500 --interval 10
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
    DOLLAR, Obs, BookImbalance, FairValue, Favorite, Momentum,
    MultiMomentum, VolRegime, VOL_MIN_STEP_MS, VOL_PRIOR_RATE, VOL_FLOOR_FRACTION,
    VOL_FULL_WEIGHT_MS, expiry_ms,
)

BUILDERS = {
    "fair_value": FairValue, "momentum": Momentum, "favorite": Favorite,
    "multi_momentum": MultiMomentum, "vol_regime": VolRegime,
    "book_imbalance": BookImbalance,
}
DEFAULT_SLATE = ["fair_value", "multi_momentum", "favorite", "book_imbalance"]


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


class Book:
    """One strategy's independent simulated book."""

    def __init__(self, strategy, principal: float):
        self.strategy = strategy
        self.principal = principal
        self.bankroll = principal
        self.positions: dict[tuple, dict] = {}
        self.bets: list[dict] = []

    @property
    def wins(self) -> int:
        return sum(1 for b in self.bets if b["won"])

    def report(self) -> dict:
        return {
            "strategy": self.strategy.name,
            "principal": self.principal,
            "bankroll": round(self.bankroll, 4),
            "pnl": round(self.bankroll - self.principal, 4),
            "settled": len(self.bets),
            "wins": self.wins,
            "open_positions": [{
                "asset": p["asset"], "market": key[1][:10], "side": p["side"],
                "price": p["price"], "stake": p["stake"], "t": p["t"],
            } for key, p in self.positions.items()],
            "bets": self.bets[-40:],
        }


class LiveTrader:
    def __init__(self, strategies, principal: float, out_dir: str, min_stake: float = 1.0):
        self.books = [Book(s, principal) for s in strategies]
        self.principal = principal
        self.out_dir = out_dir
        self.min_stake = min_stake
        self.started_at = int(time.time() * 1000)
        self.markets: dict[tuple, dict] = {}
        self.index: list[tuple[int, float]] = []
        os.makedirs(out_dir, exist_ok=True)
        self.state_path = os.path.join(out_dir, "state.json")
        self.report_path = os.path.join(out_dir, "report.json")
        self._load()

    # ---------------- persistence ----------------
    def _load(self):
        try:
            with open(self.state_path) as f:
                s = json.load(f)
            by_name = {b["strategy"]: b for b in s.get("books", [])}
            restored = 0
            for book in self.books:
                saved = by_name.get(book.strategy.name)
                if not saved:
                    continue
                book.bankroll = saved.get("bankroll", book.principal)
                book.principal = saved.get("principal", book.principal)
                book.bets = saved.get("bets", [])
                restored += 1
            self.index = [tuple(x) for x in s.get("index", [])][-4000:]
            self.started_at = s.get("started_at", self.started_at)
            if restored:
                print(f"resumed {restored} live book(s)", flush=True)
        except (OSError, ValueError, KeyError, TypeError):
            pass

    def _save(self):
        tmp = self.state_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump({
                "books": [{"strategy": b.strategy.name, "principal": b.principal,
                           "bankroll": b.bankroll, "bets": b.bets[-300:]} for b in self.books],
                "index": self.index[-4000:], "started_at": self.started_at,
            }, f)
        os.replace(tmp, self.state_path)

        report = {
            "generated_at_ms": int(time.time() * 1000),
            "paper_only": True, "live": True,
            "started_at_ms": self.started_at,
            "books": [b.report() for b in self.books],
        }
        tmp = self.report_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump(report, f, indent=1)
        os.replace(tmp, self.report_path)

    # ---------------- trading ----------------
    def settle(self, key, market):
        """Resolve a finished hour for every book: Up wins if last index >= strike."""
        last = market["obs"][-1]
        up_wins = last.s >= market["strike"]
        for book in self.books:
            position = book.positions.pop(key, None)
            if not position:
                continue
            won = (position["side"] == "UP") == up_wins
            pnl = (position["shares"] if won else 0.0) - position["stake"]
            book.bankroll += pnl
            book.bets.append({
                "t": position["t"], "settled_t": last.t, "asset": position["asset"],
                "market": key[1][:10], "side": position["side"],
                "price": round(position["price"], 4), "stake": round(position["stake"], 2),
                "won": won, "pnl": round(pnl, 4), "bankroll": round(book.bankroll, 4),
            })
            print(f"  settled [{book.strategy.name}] {position['asset']} {position['side']} "
                  f"@ {position['price']:.3f} -> {'WON' if won else 'LOST'} {pnl:+.2f} "
                  f"(bankroll ${book.bankroll:.2f})", flush=True)

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
                "asset": key[0], "strike": obs.k, "obs": [],
            })
            market["obs"].append(obs)
            if len(market["obs"]) > 2000:
                market["obs"] = market["obs"][-2000:]
            self.index.append((obs.t, obs.s))

            rate = series_variance_rate(self.index)
            for book in self.books:
                if key in book.positions or any(b["market"] == key[1][:10] for b in book.bets[-8:]):
                    continue
                book.strategy.reset(rate)
                decision = book.strategy.decide(market["obs"], market)
                if not decision:
                    continue
                side, frac, price = decision
                stake = min(book.bankroll, max(self.min_stake, frac * book.bankroll))
                if not (0 < price < 1) or stake < self.min_stake or stake > book.bankroll:
                    continue
                book.positions[key] = {
                    "asset": key[0], "side": side, "price": price, "stake": stake,
                    "shares": stake / price, "t": obs.t,
                }
                print(f"  OPEN [{book.strategy.name}] {key[0]} {side} @ {price:.3f} "
                      f"stake ${stake:.2f} ({(obs.tau_ms / 60000):.0f}m to expiry)", flush=True)

        # Markets that vanished from the projection have rolled over.
        for key, market in list(self.markets.items()):
            if key in live_keys or not market["obs"]:
                continue
            self.settle(key, market)
            self.markets.pop(key, None)
        self.index = self.index[-4000:]
        self._save()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default=os.environ.get("POLY_TERMINAL_API_URL", "http://127.0.0.1:8088") + "/api/v1/terminal/snapshot")
    ap.add_argument("--strategy", default=",".join(DEFAULT_SLATE),
                    help='comma-separated strategy names, or "all"')
    ap.add_argument("--principal", type=float, default=1000.0)
    ap.add_argument("--interval", type=float, default=10.0)
    ap.add_argument("--timeout", type=float, default=4.0)
    ap.add_argument("--out-dir", default="var/live-paper")
    args = ap.parse_args()

    names = sorted(BUILDERS) if args.strategy == "all" else [n.strip() for n in args.strategy.split(",") if n.strip()]
    unknown = [n for n in names if n not in BUILDERS]
    if unknown:
        ap.error(f"unknown strategy: {', '.join(unknown)}. choose from {', '.join(sorted(BUILDERS))} or 'all'")
    strategies = [BUILDERS[n]() for n in names]

    trader = LiveTrader(strategies, args.principal, args.out_dir)
    print(f"live paper trader: {len(strategies)} book(s) @ ${args.principal:.2f} each", flush=True)
    for s in strategies:
        print(f"  - {s.name}", flush=True)
    print(f"polling {args.url} every {args.interval:g}s", flush=True)
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
