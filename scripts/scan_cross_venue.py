#!/usr/bin/env python3
"""Cross-venue arbitrage scanner for binary BTC markets.

Looks for the same event priced differently on two venues. Unlike every other
strategy in this repo this is a MECHANICAL edge: if venue A sells YES and venue
B sells NO on the identical event, and the two cost less than $1.00 combined
after fees, the pair pays exactly $1.00 at resolution and the difference is
locked regardless of what Bitcoin does.

The whole difficulty is proving the events really are identical. A binary
"BTC up" contract is defined by three things:

    (underlying, strike price, resolution instant)

All three must match. Different strike reference times or different expiry
windows mean different contracts, and combining them is a directional bet
wearing an arbitrage costume — the most expensive mistake in this space.

Venues
------
Polymarket : hourly up/down, strike = the hour's opening price (read from the
             local read-only gateway, so no credentials are involved).
Kalshi     : public market data API, no credentials. Series KXBTC15M is
             "BTC price up in next 15 mins?" with an explicit floor_strike.

Read-only. This scanner places no order and holds no credential; it reports
opportunities for a human to act on.

Usage
-----
  python3 scripts/scan_cross_venue.py
  python3 scripts/scan_cross_venue.py --strike-tolerance 5 --once
"""
from __future__ import annotations

import argparse
import json
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

KALSHI = "https://api.elections.kalshi.com/trade-api/v2"
# Kalshi charges a trading fee that peaks mid-book; this is their published
# form. Ignoring it would manufacture arbitrage that does not exist.
def kalshi_fee(price: float, contracts: int = 1) -> float:
    return 0.07 * contracts * price * (1 - price)


def get(url: str, timeout: float = 12.0):
    try:
        with urllib.request.urlopen(url, timeout=timeout) as r:
            if r.status != 200:
                return None
            return json.loads(r.read())
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None


def kalshi_binaries(series: str):
    """Return normalised binary quotes: strike, expiry, yes bid/ask."""
    data = get(f"{KALSHI}/markets?series_ticker={series}&status=open&limit=50")
    out = []
    for m in (data or {}).get("markets", []):
        strike = m.get("floor_strike") or m.get("cap_strike")
        close = m.get("close_time")
        if strike is None or not close:
            continue
        book = get(f"{KALSHI}/markets/{m['ticker']}/orderbook?depth=1")
        fp = (book or {}).get("orderbook_fp") or {}
        yes = fp.get("yes_dollars") or []
        no = fp.get("no_dollars") or []
        # yes_dollars are bids to BUY yes; no_dollars are bids to BUY no.
        # Buying NO at p is selling YES at (1-p), so it defines the YES ask.
        yes_bid = max((float(p) for p, _ in yes), default=None)
        no_bid = max((float(p) for p, _ in no), default=None)
        yes_ask = (1 - no_bid) if no_bid is not None else None
        out.append({
            "venue": "kalshi", "ticker": m["ticker"], "strike": float(strike),
            "expiry": close, "yes_bid": yes_bid, "yes_ask": yes_ask,
            "title": m.get("title", ""),
        })
    return out


def polymarket_binaries(url: str):
    """Current hourly up/down markets from the local read-only gateway."""
    snap = get(f"{url}/api/v1/terminal/snapshot", timeout=5)
    if not snap or snap.get("mode") != "ready":
        return [], (snap or {}).get("reason", "gateway unreachable")
    out = []
    for a in snap.get("assets", []):
        out.append({
            "venue": "polymarket", "ticker": a.get("asset"),
            "strike": int(a["target_price_micros"]) / 1e6,
            "expiry": datetime.fromtimestamp(a["end_time_ms"] / 1000, timezone.utc)
                              .strftime("%Y-%m-%dT%H:%M:%SZ"),
            "yes_bid": int(a["up_book"]["best_bid_micros"]) / 1e6,
            "yes_ask": int(a["up_book"]["best_ask_micros"]) / 1e6,
            "title": f"{a.get('asset')} up this hour",
        })
    return out, None


def scan(kal, poly, tolerance: float):
    """Pair contracts that share an expiry instant and (near) identical strike."""
    hits = []
    for k in kal:
        for p in poly:
            if k["expiry"] != p["expiry"]:
                continue
            gap = abs(k["strike"] - p["strike"])
            if gap > tolerance:
                continue
            # Lock A: buy YES on Kalshi, buy NO on Polymarket (= sell YES there).
            for buy, sell, label in ((k, p, "YES kalshi / NO polymarket"),
                                     (p, k, "YES polymarket / NO kalshi")):
                if buy["yes_ask"] is None or sell["yes_bid"] is None:
                    continue
                cost = buy["yes_ask"] + (1 - sell["yes_bid"])
                fee = kalshi_fee(k["yes_ask"] or 0.5)
                net = 1.0 - cost - fee
                hits.append({"label": label, "cost": cost, "fee": fee, "net": net,
                             "gap": gap, "expiry": k["expiry"],
                             "k": k["ticker"], "p": p["ticker"]})
    return hits


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--gateway", default="http://127.0.0.1:8088")
    ap.add_argument("--series", default="KXBTC15M")
    ap.add_argument("--strike-tolerance", type=float, default=1.0,
                    help="max $ difference in strike to treat as the same event")
    ap.add_argument("--interval", type=float, default=30.0)
    ap.add_argument("--once", action="store_true")
    args = ap.parse_args()

    while True:
        stamp = datetime.now(timezone.utc).strftime("%H:%M:%SZ")
        kal = kalshi_binaries(args.series)
        poly, reason = polymarket_binaries(args.gateway)
        print(f"[{stamp}] kalshi={len(kal)} polymarket={len(poly)}"
              + (f"  ({reason})" if reason else ""))
        for m in kal:
            print(f"   KALSHI  {m['ticker']:34} strike ${m['strike']:>11,.2f} "
                  f"exp {m['expiry']}  yes {m['yes_bid']}/{m['yes_ask']}")
        for m in poly:
            print(f"   POLY    {m['ticker']:34} strike ${m['strike']:>11,.2f} "
                  f"exp {m['expiry']}  yes {m['yes_bid']:.3f}/{m['yes_ask']:.3f}")

        hits = scan(kal, poly, args.strike_tolerance)
        locks = [h for h in hits if h["net"] > 0]
        if locks:
            print("   *** LOCKED ARBITRAGE ***")
            for h in sorted(locks, key=lambda x: -x["net"]):
                print(f"     {h['label']}: cost {h['cost']:.4f} + fee {h['fee']:.4f} "
                      f"-> net +{h['net']:.4f} per $1  (strike gap ${h['gap']:.2f})")
        elif hits:
            best = max(hits, key=lambda x: x["net"])
            print(f"   matched {len(hits)} pair(s), none profitable "
                  f"(best net {best['net']:+.4f} per $1)")
        else:
            print("   no matched events (strike or expiry differ) — no lock possible")

        if args.once:
            return
        time.sleep(args.interval)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nstopped.")
