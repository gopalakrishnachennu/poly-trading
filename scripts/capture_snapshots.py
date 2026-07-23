#!/usr/bin/env python3
"""Compact, disk-sustainable research capture of validated market snapshots.

Polls the read-only terminal-projection gateway
(``GET /api/v1/terminal/snapshot``) at a low rate and appends the fields the
edge/backtest tools need to a per-UTC-day JSONL under ``var/research-capture/``.

The gateway already discovers the current hourly markets, validates fixed-point
values, enforces freshness, and fails closed. This recorder is a thin read-only
consumer of that authority — exactly like the operator terminal. It creates no
credential, order, or mutation path, and records nothing when the gateway is not
in the ``ready`` mode (so `NO_TRADE`/stale frames are never captured as data).

Each line reuses the same envelope the paper-campaign journals use, so
``analyze_paper_edge.py`` and ``backtest_fair_value.py`` read it with no changes:

    {"record": {"kind": "observation", "event_time_ms": <ms>,
                "payload": {asset, condition_id, reference_price_micros,
                            target_price_micros, up_best_*_micros,
                            down_best_*_micros}}}

At the default 15s interval a record is ~200 bytes, i.e. ~2-3 MB per day —
sustainable for the weeks of capture the backtest needs. Use Ctrl-C to stop; the
current file is always valid (one JSON object per line, flushed per write).
"""
from __future__ import annotations

import argparse
import json
import os
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone


def fetch(url: str, timeout: float) -> dict | None:
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None


def day_path(out_dir: str) -> str:
    day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    return os.path.join(out_dir, f"snapshots-{day}.jsonl")


def to_records(snap: dict, now_ms: int) -> list[dict]:
    out = []
    for a in snap.get("assets", []):
        try:
            up, down = a["up_book"], a["down_book"]
            payload = {
                "asset": a["asset"],
                "condition_id": a["condition_id"],
                "reference_price_micros": a["reference_price_micros"],
                "target_price_micros": a["target_price_micros"],
                "up_best_ask_micros": up["best_ask_micros"],
                "up_best_bid_micros": up["best_bid_micros"],
                "down_best_ask_micros": down["best_ask_micros"],
                "down_best_bid_micros": down["best_bid_micros"],
            }
        except KeyError:
            continue
        out.append({"record": {
            "campaign_id": "snapshot-capture",
            "kind": "observation",
            "event_time_ms": now_ms,
            "sequence": snap.get("sequence"),
            "snapshot_digest": snap.get("snapshot_digest"),
            "payload": payload,
        }})
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default=os.environ.get(
        "POLY_TERMINAL_API_URL", "http://127.0.0.1:8088") + "/api/v1/terminal/snapshot")
    ap.add_argument("--out-dir", default="var/research-capture")
    ap.add_argument("--interval", type=float, default=15.0, help="seconds between polls")
    ap.add_argument("--timeout", type=float, default=3.5)
    args = ap.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    print(f"capturing {args.url} every {args.interval:g}s -> {args.out_dir}/snapshots-<UTC-day>.jsonl")
    written = 0
    last_seq = None
    ready = 0
    skipped = 0
    while True:
        start = time.time()
        snap = fetch(args.url, args.timeout)
        if snap is not None and snap.get("mode") == "ready":
            seq = snap.get("sequence")
            if seq != last_seq:  # avoid duplicating an unchanged frame
                recs = to_records(snap, int(time.time() * 1000))
                if recs:
                    with open(day_path(args.out_dir), "a") as f:
                        for r in recs:
                            f.write(json.dumps(r, separators=(",", ":")) + "\n")
                        f.flush()
                    written += len(recs)
                    ready += 1
                last_seq = seq
        else:
            skipped += 1
        if (ready + skipped) % 20 == 0:
            mode = snap.get("mode") if snap else "unreachable"
            print(f"  [{datetime.now(timezone.utc).strftime('%H:%M:%SZ')}] "
                  f"records={written} ready-frames={ready} last-mode={mode}", flush=True)
        elapsed = time.time() - start
        time.sleep(max(0.0, args.interval - elapsed))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nstopped.")
