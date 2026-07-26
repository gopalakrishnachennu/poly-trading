#!/usr/bin/env python3
"""Binance BTC/ETH spot collector — book ticker + top-of-book depth.

REST polling (no external deps) with backoff.  Written in a way that lets a
future WebSocket collector produce the exact same on-disk schema.

Every event line:
    {
      "t_recv_ms":  1785030000123,          # local receipt time (ms)
      "t_event_ms": 1785030000110,          # exchange-reported update time
      "symbol":     "BTCUSDT",
      "bid":        62431.10,
      "ask":        62431.20,
      "mid":        62431.15,
      "bid_sz":     0.523,
      "ask_sz":     1.204,
      "seq":        1023823001,             # updateId from Binance
      "src":        "rest"                  # or "ws" after upgrade
    }

Writes to var/research-binance/{symbol}-YYYY-MM-DD.jsonl.

Read-only.  No credential, no order path.  Binance's book-ticker endpoint is
public and does not require authentication.

Usage:
    python3 scripts/profit_engine/collectors/binance_spot.py
    python3 scripts/profit_engine/collectors/binance_spot.py --symbols BTCUSDT,ETHUSDT
"""
from __future__ import annotations
import argparse, json, os, sys, time
import urllib.error, urllib.request
from datetime import datetime, timezone


BINANCE_BOOK_TICKER = "https://api.binance.com/api/v3/ticker/bookTicker"
DEFAULT_SYMBOLS = ("BTCUSDT", "ETHUSDT")
USER_AGENT = "poly-trading research read-only external market data"


def fetch_book_tickers(symbols: tuple[str, ...], timeout: float) -> list[dict] | None:
    """Fetch top-of-book for the given symbols.  Returns None on transient error."""
    # Binance accepts ?symbols=%5B%22BTCUSDT%22%2C%22ETHUSDT%22%5D
    quoted = ",".join(f'"{s}"' for s in symbols)
    url = f'{BINANCE_BOOK_TICKER}?symbols=[{quoted}]'
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None


def normalize(row: dict, t_recv_ms: int) -> dict | None:
    """Convert Binance's book-ticker record into our schema."""
    try:
        bid = float(row["bidPrice"]); ask = float(row["askPrice"])
        bid_sz = float(row["bidQty"]); ask_sz = float(row["askQty"])
        if bid <= 0 or ask <= 0 or bid >= ask:
            return None
        # bookTicker doesn't carry an exchange event timestamp; use receipt.
        return {
            "t_recv_ms": t_recv_ms, "t_event_ms": t_recv_ms,
            "symbol": row["symbol"],
            "bid": bid, "ask": ask,
            "mid": round((bid + ask) / 2, 8),
            "bid_sz": bid_sz, "ask_sz": ask_sz,
            "seq": int(row.get("updateId", 0)),
            "src": "rest",
        }
    except (KeyError, TypeError, ValueError):
        return None


def day_path(out_dir: str, symbol: str) -> str:
    day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    return os.path.join(out_dir, f"{symbol.lower()}-{day}.jsonl")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--symbols", default=",".join(DEFAULT_SYMBOLS),
                    help="comma-separated Binance symbols")
    ap.add_argument("--out-dir", default="var/research-binance")
    ap.add_argument("--interval", type=float, default=1.0,
                    help="seconds between polls; Binance limit is 6000/min so 0.1+ is safe")
    ap.add_argument("--timeout", type=float, default=4.0)
    args = ap.parse_args()

    symbols = tuple(s.strip().upper() for s in args.symbols.split(",") if s.strip())
    os.makedirs(args.out_dir, exist_ok=True)
    print(f"binance spot collector — symbols={symbols} interval={args.interval:g}s", flush=True)
    print(f"  out {args.out_dir}/{{symbol}}-YYYY-MM-DD.jsonl", flush=True)
    print("  READ-ONLY: no credential, no order, no wallet.", flush=True)

    last_seq: dict[str, int] = {}
    backoff = args.interval
    max_backoff = 30.0
    writes = 0

    while True:
        loop_start = time.time()
        t_recv_ms = int(loop_start * 1000)
        rows = fetch_book_tickers(symbols, args.timeout)
        if rows is None:
            # Exponential backoff on transient failure.  Never crash.
            time.sleep(min(backoff, max_backoff))
            backoff = min(backoff * 2, max_backoff)
            continue
        backoff = args.interval

        for raw in rows:
            evt = normalize(raw, t_recv_ms)
            if evt is None:
                continue
            # Dedup: skip if updateId hasn't advanced (avoid rewriting the same tick).
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

        if writes and writes % 200 == 0:
            print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] "
                  f"events written: {writes}", flush=True)
        time.sleep(max(0.0, args.interval - (time.time() - loop_start)))


if __name__ == "__main__":
    try:
        sys.exit(main() or 0)
    except KeyboardInterrupt:
        print("\nstopped.")
