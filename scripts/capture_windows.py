#!/usr/bin/env python3
"""Read-only capture of Polymarket's short-window BTC/ETH markets.

Records the rolling 5-minute and 15-minute "Up or Down" markets for both BTC
and ETH to JSONL in the SAME schema the L2 exporter emits, so
scripts/build_dataset.py and scripts/analyze_dataset.py run on them unchanged.
Each window/asset stream is written under var/research-<window>/ so a dataset
can be built per window with buffers appropriate to its length.

The 1-hour markets are already captured and shown by the Rust gateway; this
tool covers the short windows the gateway does not.

A new 5m market opens every 5 minutes and a 15m market every 15 minutes, per
asset, so this accumulates hundreds of resolvable markets per day — enough to
test the short-horizon idea with real statistical power in days, not weeks.

Public, read-only. No credential, wallet, signer or order path; no order is
ever placed. Output is derived research data.

Usage
-----
  python3 scripts/capture_windows.py
  python3 scripts/capture_windows.py --interval 5 --depth 10
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
# Local gateway snapshot — the authoritative 1h source. The gateway already
# discovers, validates, and de-dupes the current-hour BTC/ETH markets; polling
# it means we don't need to re-implement date-based hourly-slug discovery here.
GATEWAY_SNAPSHOT = "http://127.0.0.1:8088/api/v1/terminal/snapshot"
UA = {"User-Agent": "poly-trading research read-only market data"}
DOLLAR = 1_000_000

# (asset, window_seconds, slug_prefix, out_subdir)
SPECS = [
    ("BTC", 300, "btc-updown-5m", "research-5m"),
    ("ETH", 300, "eth-updown-5m", "research-5m"),
    ("BTC", 900, "btc-updown-15m", "research-15m"),
    ("ETH", 900, "eth-updown-15m", "research-15m"),
]

HOURLY_ASSETS = ("BTC", "ETH")
HOURLY_SUB = "research-1h"


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
    rows = []
    for entry in side or []:
        try:
            rows.append((float(entry["price"]), float(entry["size"])))
        except (KeyError, ValueError, TypeError):
            continue
    rows.sort(key=lambda pair: pair[0], reverse=best_first_desc)
    return [[round(px * DOLLAR), round(sz * DOLLAR)] for px, sz in rows[:depth]]


def discover(slug_prefix: str, window_start: int, timeout: float):
    data = get(f"{GAMMA}{slug_prefix}-{window_start}", timeout)
    if not data:
        return None
    try:
        market = data[0]["markets"][0]
        tokens = market["clobTokenIds"]
        tokens = json.loads(tokens) if isinstance(tokens, str) else tokens
        outcomes = market["outcomes"]
        outcomes = json.loads(outcomes) if isinstance(outcomes, str) else outcomes
        return market["conditionId"], tokens[outcomes.index("Up")], tokens[outcomes.index("Down")]
    except (KeyError, IndexError, ValueError, TypeError):
        return None


def day_path(out_dir: str, asset: str) -> str:
    day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    return os.path.join(out_dir, f"{asset.lower()}-{day}.jsonl")


def load_existing_identities(out_root: str, specs) -> dict:
    """Seed the (asset, window)->(cond, up, down) map from today's files.

    Prevents rewriting a market_identity row across process restarts within
    the same window: without this, every supervisor auto-restart would append
    a duplicate identity for whatever market is currently open.
    """
    seeded: dict = {}
    for idx, (asset, step, _slug, sub) in enumerate(specs):
        out_dir = os.path.join(out_root, sub)
        path = day_path(out_dir, asset)
        if not os.path.exists(path):
            continue
        try:
            with open(path) as handle:
                for line in handle:
                    try:
                        row = json.loads(line)
                    except ValueError:
                        continue
                    if row.get("k") != "sys":
                        continue
                    payload = row.get("payload") or {}
                    if payload.get("event_type") != "market_identity":
                        continue
                    if payload.get("asset") != asset:
                        continue
                    start_ms = payload.get("start_time_ms")
                    end_ms = payload.get("end_time_ms")
                    if not isinstance(start_ms, int) or not isinstance(end_ms, int):
                        continue
                    if (end_ms - start_ms) // 1000 != step:
                        continue
                    window = start_ms // 1000
                    seeded[idx] = (window, (payload["condition_id"],
                                            payload["up_token_id"],
                                            payload["down_token_id"]))
        except OSError:
            continue
    return seeded


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out-root", default="var")
    ap.add_argument("--interval", type=float, default=5.0)
    ap.add_argument("--depth", type=int, default=10)
    ap.add_argument("--timeout", type=float, default=6.0)
    args = ap.parse_args()

    for _, _, _, sub in SPECS:
        os.makedirs(os.path.join(args.out_root, sub), exist_ok=True)
    os.makedirs(os.path.join(args.out_root, HOURLY_SUB), exist_ok=True)
    print("capturing BTC+ETH 5m/15m/1h markets every "
          f"{args.interval:g}s -> {args.out_root}/research-{{5m,15m,1h}}/", flush=True)
    print("PUBLIC READ-ONLY — no credential, wallet, signer or order path.", flush=True)

    windows: dict[int, int] = {}       # spec index -> current window start
    identities: dict[int, tuple] = {}  # spec index -> (cond, up, down)
    live: dict[int, dict] = {}         # spec index -> latest top-of-book snapshot
    latest_path = os.path.join(args.out_root, "windows-latest.json")
    rows_written = 0
    cycles = 0

    # Recover state written by an earlier process so a restart does not
    # duplicate the identity row for the market that is already open.
    seeded = load_existing_identities(args.out_root, SPECS)
    if seeded:
        for idx, (window, ident) in seeded.items():
            windows[idx] = window
            identities[idx] = ident
        print(f"seeded {len(seeded)} existing market identities from today's files", flush=True)

    def best(levels_list):
        return (levels_list[0][0] / DOLLAR) if levels_list else None

    # Track the 1h condition_id currently open per asset so we only write the
    # market_identity row once per hourly rollover (matches the 5m/15m dedup).
    hourly_current: dict[str, str] = {}

    def capture_hourly(out_root: str, asset_names) -> None:
        """Persist 1h book snapshots by reading the local projection gateway.

        Uses the same on-disk schema as 5m/15m so downstream tools that scan
        `research-*` treat all three window sizes identically. Silently no-ops
        when the gateway isn't ready — the gateway itself is fail-closed and
        will only publish valid, digest-checked books.
        """
        snap = get(GATEWAY_SNAPSHOT, args.timeout)
        if not snap or snap.get("mode") != "ready":
            return
        out_dir = os.path.join(out_root, HOURLY_SUB)
        now_ns = time.time_ns()
        for a in snap.get("assets", []):
            asset_name = a.get("asset")
            if asset_name not in asset_names:
                continue
            cond = a.get("condition_id")
            up_tok = a.get("up_book", {}).get("token_id")
            down_tok = a.get("down_book", {}).get("token_id")
            start_ms = a.get("start_time_ms")
            end_ms = a.get("end_time_ms")
            if not all((cond, up_tok, down_tok, start_ms, end_ms)):
                continue
            path = day_path(out_dir, asset_name)
            if hourly_current.get(asset_name) != cond:
                hourly_current[asset_name] = cond
                # Only write identity if not already present for this cond in today's file.
                already = False
                if os.path.exists(path):
                    try:
                        with open(path) as h:
                            for line in h:
                                if f'"condition_id":"{cond}"' in line:
                                    already = True
                                    break
                    except OSError:
                        pass
                if not already:
                    with open(path, "a") as h:
                        h.write(json.dumps({
                            "k": "sys",
                            "payload": {
                                "event_type": "market_identity", "asset": asset_name,
                                "condition_id": cond, "up_token_id": up_tok,
                                "down_token_id": down_tok,
                                "start_time_ms": start_ms, "end_time_ms": end_ms,
                            },
                        }, separators=(",", ":")) + "\n")
            # Book rows in the same schema the 5m/15m capture emits.
            def levels_from_gateway(book, key):
                out = []
                for row in book.get(key, [])[:args.depth]:
                    try:
                        px = int(row["price_micros"]); sz = int(row["quantity_micros"])
                        out.append([px, sz])
                    except (KeyError, ValueError, TypeError):
                        continue
                return out
            for token, book in ((up_tok, a["up_book"]), (down_tok, a["down_book"])):
                bids = levels_from_gateway(book, "bids")
                asks = levels_from_gateway(book, "asks")
                if not bids and not asks:
                    continue
                with open(path, "a") as h:
                    h.write(json.dumps({
                        "k": "book", "tok": token, "cond": cond,
                        "t": now_ns, "bids": bids, "asks": asks,
                    }, separators=(",", ":")) + "\n")

    while True:
        start = time.time()
        capture_hourly(args.out_root, HOURLY_ASSETS)
        for idx, (asset, step, slug_prefix, sub) in enumerate(SPECS):
            window = int(time.time()) // step * step
            out_dir = os.path.join(args.out_root, sub)
            if windows.get(idx) != window:
                found = discover(slug_prefix, window, args.timeout)
                if found:
                    windows[idx] = window
                    identities[idx] = found
                    with open(day_path(out_dir, asset), "a") as handle:
                        handle.write(json.dumps({
                            "k": "sys",
                            "payload": {
                                "event_type": "market_identity", "asset": asset,
                                "condition_id": found[0], "up_token_id": found[1],
                                "down_token_id": found[2],
                                "start_time_ms": window * 1000,
                                "end_time_ms": (window + step) * 1000,
                            },
                        }, separators=(",", ":")) + "\n")
            ident = identities.get(idx)
            if windows.get(idx) != window or not ident:
                continue
            cond, up, down = ident
            now_ns = time.time_ns()
            out_rows = []
            for token in (up, down):
                book = get(f"{CLOB}{token}", args.timeout)
                if not book:
                    continue
                bids = levels(book.get("bids"), True, args.depth)
                asks = levels(book.get("asks"), False, args.depth)
                if not bids and not asks:
                    continue
                out_rows.append({"k": "book", "tok": token, "cond": cond,
                                 "t": now_ns, "bids": bids, "asks": asks})
            if out_rows:
                with open(day_path(out_dir, asset), "a") as handle:
                    for row in out_rows:
                        handle.write(json.dumps(row, separators=(",", ":")) + "\n")
                    handle.flush()
                rows_written += len(out_rows)
                by_tok = {row["tok"]: row for row in out_rows}
                up_row, down_row = by_tok.get(up), by_tok.get(down)
                depth = lambda row, key: (row[key][:8] if row else [])  # noqa: E731
                live[idx] = {
                    "asset": asset,
                    "window": "5m" if step == 300 else "15m",
                    "cond": cond,
                    "start_ms": window * 1000,
                    "end_ms": (window + step) * 1000,
                    "up_bid": best(up_row["bids"]) if up_row else None,
                    "up_ask": best(up_row["asks"]) if up_row else None,
                    "down_bid": best(down_row["bids"]) if down_row else None,
                    "down_ask": best(down_row["asks"]) if down_row else None,
                    # Full top-of-book depth (best-first, price+size micros) so the
                    # terminal can render the same complementary book the hourly
                    # markets show. Capped to keep the snapshot small.
                    "up_bids": depth(up_row, "bids"), "up_asks": depth(up_row, "asks"),
                    "down_bids": depth(down_row, "bids"), "down_asks": depth(down_row, "asks"),
                    "t_ms": now_ns // 1_000_000,
                }

        # Small live snapshot for the terminal to read cheaply.
        snapshot = {
            "generated_at_ms": int(time.time() * 1000),
            "paper_only": True,
            "markets": [live[i] for i in sorted(live)],
        }
        tmp = latest_path + ".tmp"
        with open(tmp, "w") as handle:
            json.dump(snapshot, handle)
        os.replace(tmp, latest_path)

        cycles += 1
        if cycles % 20 == 0:
            print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] "
                  f"book rows written: {rows_written}", flush=True)
        time.sleep(max(0.0, args.interval - (time.time() - start)))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nstopped.")
