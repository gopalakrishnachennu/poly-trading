#!/usr/bin/env python3
"""Live directional paper trader for all six BTC/ETH markets (5m, 15m, 1h).

Watches the unified /api/windows feed (which merges the hourly gateway
projection with the read-only 5m/15m short-window capture) and runs one or
more strategies from scripts/hourly_engine.py side by side. Each strategy
keeps its own independent book (bankroll, open positions, settled bets) over
the same live tape, so they can be compared honestly. Each book opens at
most ONE simulated position per (asset, window, market) and settles it
against the realised outcome when that market rolls over.

Short-window markets (5m/15m) don't publish an explicit strike — the trader
uses the first reference price it observes within each new window as the
strike, which is what a real algorithmic trader would do without insider
knowledge of Polymarket's precise open snapshot.

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


def to_obs(market: dict, now_ms: int, strike_micros: float | None) -> Obs | None:
    """Build an Obs from a /api/windows market row.

    The unified feed spells fields consistently across all six markets:
      reference_micros, up_book, down_book, end_ms, window ("5m"|"15m"|"1h").
    Hourly markets carry an explicit strike in target_micros; short windows
    carry None until the trader has observed a first-tick reference to
    anchor the strike from — that anchor is passed in as strike_micros.
    """
    try:
        up, down = market["up_book"], market["down_book"]
        ref = market.get("reference_micros")
        if ref is None:
            return None  # can't score a market without a reference index

        def size(book, side):
            try:
                return float((book.get(side) or [{}])[0].get("quantity_micros", 0))
            except (IndexError, TypeError, ValueError):
                return 0.0

        target = market.get("target_micros")
        strike = float(target) if target is not None else (
            float(strike_micros) if strike_micros is not None else float(ref)
        )
        end_ms = market.get("end_ms")
        tau_ms = max(0.0, float(end_ms) - now_ms) if end_ms is not None else 0.0

        return Obs(
            t=now_ms,
            s=float(ref),
            k=strike,
            up_ask=int(up["best_ask_micros"]) / DOLLAR,
            up_bid=int(up["best_bid_micros"]) / DOLLAR,
            down_ask=int(down["best_ask_micros"]) / DOLLAR,
            down_bid=int(down["best_bid_micros"]) / DOLLAR,
            tau_ms=tau_ms,
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
        # Per-window rollup so the operator can see which window(s) each strategy
        # is winning or losing on — the whole point of running all six markets.
        per_window: dict[str, dict] = {}
        for bet in self.bets:
            w = bet.get("window", "1h")
            slot = per_window.setdefault(w, {"bets": 0, "wins": 0, "pnl": 0.0})
            slot["bets"] += 1
            slot["wins"] += 1 if bet["won"] else 0
            slot["pnl"] = round(slot["pnl"] + bet["pnl"], 4)
        return {
            "strategy": self.strategy.name,
            "principal": self.principal,
            "bankroll": round(self.bankroll, 4),
            "pnl": round(self.bankroll - self.principal, 4),
            "settled": len(self.bets),
            "wins": self.wins,
            "per_window": per_window,
            "open_positions": [{
                "asset": p["asset"], "window": p.get("window", "1h"),
                "market": key[2][:10] if len(key) > 2 else key[1][:10],
                "side": p["side"], "price": p["price"], "stake": p["stake"], "t": p["t"],
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
        """Resolve a finished market for every book: Up wins if last index >= strike."""
        last = market["obs"][-1]
        up_wins = last.s >= market["strike"]
        window = market.get("window", "1h")
        for book in self.books:
            position = book.positions.pop(key, None)
            if not position:
                continue
            won = (position["side"] == "UP") == up_wins
            pnl = (position["shares"] if won else 0.0) - position["stake"]
            book.bankroll += pnl
            book.bets.append({
                "t": position["t"], "settled_t": last.t,
                "asset": position["asset"], "window": window,
                "market": key[2][:10] if len(key) > 2 else key[1][:10],
                "side": position["side"],
                "price": round(position["price"], 4), "stake": round(position["stake"], 2),
                "won": won, "pnl": round(pnl, 4), "bankroll": round(book.bankroll, 4),
            })
            print(f"  settled [{book.strategy.name}] {position['asset']} {window} "
                  f"{position['side']} @ {position['price']:.3f} -> "
                  f"{'WON' if won else 'LOST'} {pnl:+.2f} "
                  f"(bankroll ${book.bankroll:.2f})", flush=True)

    def tick(self, feed: dict):
        """Consume one /api/windows response and step every strategy."""
        now_ms = int(time.time() * 1000)
        live_keys = set()
        for m in feed.get("markets", []):
            asset = m.get("asset")
            window = m.get("window", "1h")
            cond = m.get("cond") or m.get("condition_id")
            if not asset or not cond:
                continue
            # 1h rollover-cached rows (stale=true) are held over the blip for
            # display but must not drive trading decisions — the tape is frozen.
            if m.get("stale"):
                key = (asset, window, cond)
                live_keys.add(key)
                continue
            key = (asset, window, cond)
            live_keys.add(key)

            state = self.markets.setdefault(key, {
                "asset": asset, "window": window, "strike": None, "obs": [],
                "start_ms": m.get("start_ms"), "end_ms": m.get("end_ms"),
            })
            obs = to_obs(m, now_ms, state.get("strike"))
            if obs is None:
                continue
            if state["strike"] is None:
                # First tick of this market — anchor the strike from the current
                # reference. Explicit target_micros (1h) wins; otherwise this is
                # the honest best available approximation of the open.
                state["strike"] = obs.k
            state["obs"].append(obs)
            if len(state["obs"]) > 2000:
                state["obs"] = state["obs"][-2000:]
            self.index.append((obs.t, obs.s))

            rate = series_variance_rate(self.index)
            # Scale each strategy's decision_min to this market's window length so
            # the same strategy that fires at "1/3 through a 60m window" also
            # fires at "1/3 through a 5m/15m window". Without this, strategies
            # tuned for 1h markets (decision_min=20-30min) never fire on the
            # short windows because those markets never have 20min to go.
            start_ms = state.get("start_ms") or m.get("start_ms") or 0
            end_ms = state.get("end_ms") or m.get("end_ms") or 0
            market_len_ms = (end_ms - start_ms) if (end_ms and start_ms) else 0
            scale = market_len_ms / 3_600_000 if market_len_ms > 0 else 1.0
            for book in self.books:
                mkey = cond[:10]
                # bets written by the old (hourly-only) trader have no window
                # field; treat their absence as "1h" so we don't accidentally
                # re-open a position on a market that just settled.
                if key in book.positions or any(
                    b["market"] == mkey and b.get("window", "1h") == window
                    for b in book.bets[-8:]
                ):
                    continue
                book.strategy.reset(rate)
                original_dm = getattr(book.strategy, "decision_min", None)
                if original_dm is not None:
                    book.strategy.decision_min = original_dm * scale
                try:
                    decision = book.strategy.decide(state["obs"], state)
                finally:
                    if original_dm is not None:
                        book.strategy.decision_min = original_dm
                if not decision:
                    continue
                side, frac, price = decision
                stake = min(book.bankroll, max(self.min_stake, frac * book.bankroll))
                if not (0 < price < 1) or stake < self.min_stake or stake > book.bankroll:
                    continue
                book.positions[key] = {
                    "asset": asset, "window": window, "side": side,
                    "price": price, "stake": stake,
                    "shares": stake / price, "t": obs.t,
                }
                print(f"  OPEN [{book.strategy.name}] {asset} {window} {side} @ "
                      f"{price:.3f} stake ${stake:.2f} "
                      f"({(obs.tau_ms / 60000):.1f}m to expiry)", flush=True)

        # Markets that vanished from the feed have rolled over.
        for key, state in list(self.markets.items()):
            if key in live_keys or not state["obs"] or state.get("strike") is None:
                continue
            self.settle(key, state)
            self.markets.pop(key, None)
        self.index = self.index[-4000:]
        self._save()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    default_url = os.environ.get("POLY_WINDOWS_URL", "http://127.0.0.1:3000/api/windows")
    ap.add_argument("--url", default=default_url,
                    help="unified /api/windows feed URL (all 6 markets)")
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
    print(f"polling {args.url} every {args.interval:g}s (all 6 markets: BTC/ETH x 5m/15m/1h)", flush=True)
    print("PAPER ONLY — no credential, order, or capital authority.", flush=True)

    skipped = 0
    while True:
        start = time.time()
        feed = fetch(args.url, args.timeout)
        if feed is not None and feed.get("available") is True and isinstance(feed.get("markets"), list):
            trader.tick(feed)
        else:
            skipped += 1
            if skipped % 30 == 0:
                reason = "unreachable" if feed is None else "no markets in feed"
                print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] waiting "
                      f"({reason})", flush=True)
        time.sleep(max(0.0, args.interval - (time.time() - start)))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nstopped.")
