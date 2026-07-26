#!/usr/bin/env python3
"""Coinbase BTC-USD / ETH-USD spot collector — same output schema as
binance_spot.py.

Fallback venue for regions where Binance is blocked (403 "Service
unavailable from a restricted location").  Coinbase Exchange REST API is
public and does not require authentication.  Rate limit: 10 requests/second
per public endpoint, well above what we need for a 1s poll of two symbols.

Endpoint: https://api.exchange.coinbase.com/products/{product}/ticker
Returns {ask, bid, price, time, ...}.  We normalise into the shared schema
used by binance_spot.py so downstream feature-building code doesn't care
which venue produced the row (`src` field records the source).

Writes: var/research-coinbase/{symbol}-YYYY-MM-DD.jsonl
"""
from __future__ import annotations
import argparse, json, os, sys, time
import urllib.error, urllib.request
from datetime import datetime, timezone


COINBASE_TICKER = "https://api.exchange.coinbase.com/products/{product}/ticker"
DEFAULT_PRODUCTS = ("BTC-USD", "ETH-USD")
USER_AGENT = "poly-trading research read-only external market data"


def fetch_ticker(product: str, timeout: float) -> dict | None:
    url = COINBASE_TICKER.format(product=product)
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if resp.status != 200: return None
            return json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None


def parse_iso_ms(s: str) -> int | None:
    """Coinbase 'time' looks like 2026-07-26T02:03:18.477972507Z (RFC3339)."""
    try:
        # Trim beyond microseconds; Python can't parse nanoseconds directly.
        if "." in s:
            head, frac = s.split(".")
            frac = frac.rstrip("Z")[:6]  # up to 6 digits
            s = f"{head}.{frac}Z"
        s = s.replace("Z", "+00:00")
        return int(datetime.fromisoformat(s).timestamp() * 1000)
    except Exception:
        return None


def normalize(row: dict, product: str, t_recv_ms: int) -> dict | None:
    try:
        bid = float(row["bid"]); ask = float(row["ask"])
        if bid <= 0 or ask <= 0 or bid >= ask: return None
        t_evt = parse_iso_ms(row.get("time", "")) or t_recv_ms
        # Coinbase ticker doesn't publish size directly, so use trade size
        # as a rough liquidity proxy; downstream features tolerate missing sizes.
        size = float(row.get("size", 0.0) or 0.0)
        # Product like "BTC-USD" -> symbol like "BTCUSD" for consistency
        symbol = product.replace("-", "")
        return {
            "t_recv_ms": t_recv_ms, "t_event_ms": t_evt,
            "symbol": symbol,
            "bid": bid, "ask": ask,
            "mid": round((bid + ask) / 2, 8),
            "bid_sz": size, "ask_sz": size,  # unknown split; symmetric proxy
            "seq": int(row.get("trade_id", 0)),
            "src": "coinbase_rest",
        }
    except (KeyError, TypeError, ValueError):
        return None


def day_path(out_dir: str, symbol: str) -> str:
    day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    return os.path.join(out_dir, f"{symbol.lower()}-{day}.jsonl")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--products", default=",".join(DEFAULT_PRODUCTS))
    ap.add_argument("--out-dir", default="var/research-coinbase")
    ap.add_argument("--interval", type=float, default=1.0)
    ap.add_argument("--timeout", type=float, default=4.0)
    args = ap.parse_args()
    products = tuple(p.strip().upper() for p in args.products.split(",") if p.strip())
    os.makedirs(args.out_dir, exist_ok=True)
    print(f"coinbase spot collector — products={products} interval={args.interval:g}s",
          flush=True)
    print(f"  out {args.out_dir}/{{symbol}}-YYYY-MM-DD.jsonl", flush=True)
    print("  READ-ONLY: no credential, no order, no wallet.", flush=True)

    last_seq: dict[str, int] = {}
    backoff = args.interval; max_backoff = 30.0
    writes = 0
    while True:
        loop_start = time.time()
        t_recv_ms = int(loop_start * 1000)
        any_ok = False
        for product in products:
            raw = fetch_ticker(product, args.timeout)
            if raw is None: continue
            any_ok = True
            evt = normalize(raw, product, t_recv_ms)
            if evt is None: continue
            if evt["seq"] and last_seq.get(evt["symbol"]) == evt["seq"]:
                continue
            last_seq[evt["symbol"]] = evt["seq"]
            path = day_path(args.out_dir, evt["symbol"])
            try:
                with open(path, "a") as h:
                    h.write(json.dumps(evt, separators=(",", ":")) + "\n")
                writes += 1
            except OSError as err:
                print(f"  write failed for {evt['symbol']}: {err}", file=sys.stderr,
                      flush=True)
        if not any_ok:
            time.sleep(min(backoff, max_backoff))
            backoff = min(backoff * 2, max_backoff)
            continue
        backoff = args.interval
        if writes and writes % 200 == 0:
            print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] "
                  f"events written: {writes}", flush=True)
        time.sleep(max(0.0, args.interval - (time.time() - loop_start)))


if __name__ == "__main__":
    try:
        sys.exit(main() or 0)
    except KeyboardInterrupt:
        print("\nstopped.")
