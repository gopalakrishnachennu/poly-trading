#!/usr/bin/env python3
"""Read-only capture of Polymarket's BTC 'Up or Down 5m' markets.

Polls the public Gamma + CLOB endpoints for the currently-live 5-minute BTC
market and records both legs' order books to JSONL in the SAME schema the L2
exporter emits, so scripts/build_dataset.py and scripts/analyze_dataset.py run
on it unchanged. A new market opens every 5 minutes, so this accumulates ~288
resolvable markets/day — roughly 12x faster than the hourly capture — letting us
test the 5-minute idea with real statistical power in days, not weeks.

This is a public, read-only research recorder. It holds no credential, wallet,
signer or order path and places no order. Output is derived research data.

Schema per line (matches export_l2):
  {"k":"sys","payload":{"event_type":"market_identity","asset":"BTC",
     "condition_id":..,"up_token_id":..,"down_token_id":..,
     "start_time_ms":..,"end_time_ms":..}}
  {"k":"book","tok":..,"cond":..,"t":<ns>,"bids":[[px_micros,sz_micros]..],
     "asks":[[px_micros,sz_micros]..]}    # both best-first

Usage
-----
  python3 scripts/capture_5m.py
  python3 scripts/capture_5m.py --interval 4 --depth 10
"""
from __future__ import annotations

import argparse
import json
import os
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

GAMMA = "https://gamma-api.polymarket.com/events?slug="
CLOB = "https://clob.polymarket.com/book?token_id="
UA = {"User-Agent": "poly-trading research read-only market data"}
DOLLAR = 1_000_000
WINDOW_S = 300


def get(url: str, timeout: float):
    try:
        req = urllib.request.Request(url, headers=UA)
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None


def levels(side, best_first_desc: bool, depth: int):
    """Convert a CLOB book side to best-first [px_micros, sz_micros] rows."""
    rows = []
    for entry in side or []:
        try:
            rows.append((float(entry["price"]), float(entry["size"])))
        except (KeyError, ValueError, TypeError):
            continue
    rows.sort(key=lambda pair: pair[0], reverse=best_first_desc)
    return [[round(px * DOLLAR), round(sz * DOLLAR)] for px, sz in rows[:depth]]


def discover(window_start: int, timeout: float):
    """Return (condition_id, up_token, down_token) for a 5m window, or None."""
    data = get(f"{GAMMA}btc-updown-5m-{window_start}", timeout)
    if not data:
        return None
    try:
        market = data[0]["markets"][0]
        tokens = market["clobTokenIds"]
        tokens = json.loads(tokens) if isinstance(tokens, str) else tokens
        outcomes = market["outcomes"]
        outcomes = json.loads(outcomes) if isinstance(outcomes, str) else outcomes
        up = tokens[outcomes.index("Up")]
        down = tokens[outcomes.index("Down")]
        return market["conditionId"], up, down
    except (KeyError, IndexError, ValueError, TypeError):
        return None


def day_path(out_dir: str) -> str:
    day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    return os.path.join(out_dir, f"btc5m-{day}.jsonl")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out-dir", default="var/research-5m")
    ap.add_argument("--interval", type=float, default=4.0, help="seconds between polls")
    ap.add_argument("--depth", type=int, default=10)
    ap.add_argument("--timeout", type=float, default=6.0)
    args = ap.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    print(f"capturing BTC 5m markets every {args.interval:g}s -> {args.out_dir}/", flush=True)
    print("PUBLIC READ-ONLY — no credential, wallet, signer or order path.", flush=True)

    current_window = None
    identity = None            # (cond, up, down) for current_window
    books_written = 0
    cycles = 0

    while True:
        start = time.time()
        window = int(time.time()) // WINDOW_S * WINDOW_S

        if window != current_window:
            found = discover(window, args.timeout)
            if found:
                cond, up, down = found
                current_window = window
                identity = found
                with open(day_path(args.out_dir), "a") as handle:
                    handle.write(json.dumps({
                        "k": "sys",
                        "payload": {
                            "event_type": "market_identity", "asset": "BTC",
                            "condition_id": cond, "up_token_id": up, "down_token_id": down,
                            "start_time_ms": window * 1000, "end_time_ms": (window + WINDOW_S) * 1000,
                        },
                    }, separators=(",", ":")) + "\n")
                print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] new window "
                      f"{window} cond={cond[:10]}", flush=True)

        if identity:
            cond, up, down = identity
            now_ns = time.time_ns()
            rows = []
            for token in (up, down):
                book = get(f"{CLOB}{token}", args.timeout)
                if not book:
                    continue
                bids = levels(book.get("bids"), True, args.depth)   # best (highest) first
                asks = levels(book.get("asks"), False, args.depth)  # best (lowest) first
                if not bids and not asks:
                    continue
                rows.append({"k": "book", "tok": token, "cond": cond, "t": now_ns,
                             "bids": bids, "asks": asks})
            if rows:
                with open(day_path(args.out_dir), "a") as handle:
                    for row in rows:
                        handle.write(json.dumps(row, separators=(",", ":")) + "\n")
                    handle.flush()
                books_written += len(rows)

        cycles += 1
        if cycles % 30 == 0:
            print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] book rows written: "
                  f"{books_written}", flush=True)
        time.sleep(max(0.0, args.interval - (time.time() - start)))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nstopped.")
