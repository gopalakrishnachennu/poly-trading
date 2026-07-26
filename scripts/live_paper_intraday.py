#!/usr/bin/env python3
"""Retired fixed-threshold intraday paper challenger.

Watches 15m and 1h markets via /api/windows.  Enters a simulated position
on the UP or DOWN token when its ask drops to <= BUY_THRESHOLD, exits
when bid >= SELL_THRESHOLD or when the market ends (at last known bid).

Findings that motivate this (scripts/analyze_intraday.py, n=604 markets):
  15m UP  buy<=0.25 / sell>=0.40 : +11.9% ROI on n=131
  15m DOWN buy<=0.25 / sell>=0.40: +1.3%  ROI on n=135
  1h  UP  buy<=0.25 / sell>=0.40 : +14.9% ROI on n=23
  5m  loses on both sides — excluded.

This tracker has negative out-of-sample paper P&L and is retired by default.
It cannot create simulated entries unless an operator supplies both explicit
deployment acknowledgements.  It never touches the directional trader and
does not place real orders.  Its historical simulated fills remain in
`var/live-paper/intraday-bets-YYYY-MM-DD.jsonl` so we get out-of-sample
evidence without changing it.

Usage:
    python3 scripts/live_paper_intraday.py                  # supervised
    python3 scripts/live_paper_intraday.py --stake 5 --interval 5
"""
from __future__ import annotations
import argparse, json, os, time, urllib.request, urllib.error
from datetime import datetime, timezone


DEFAULT_URL = "http://127.0.0.1:3000/api/windows"
DEFAULT_STAKE = 10.0
BUY_THRESHOLD = 0.25   # enter when ask <= this
SELL_THRESHOLD = 0.40  # exit when bid >= this
WINDOWS = ("15m", "1h")  # 5m loses in every mean-reversion variant
MAX_CONCURRENT_POSITIONS = 8  # cap so runaway losses can't spiral
DOLLAR = 1_000_000
RETIREMENT_STATUS = "RETIRED_NEGATIVE_OOS"
RETIREMENT_REASON = (
    "Fixed entry/exit thresholds have negative observed out-of-sample paper "
    "P&L and have not passed unseen-session net-P&L governance."
)
EXPERIMENT_ACKNOWLEDGEMENT = "I_UNDERSTAND_UNVALIDATED_STRATEGY"


def experiment_enabled(environ: dict[str, str] | None = None) -> bool:
    """Require two deployment-level acknowledgements before any paper entry.

    This is intentionally not a dashboard setting.  An old supervisor or a
    casual direct invocation must not silently revive a retired challenger.
    """
    values = os.environ if environ is None else environ
    return (
        values.get("POLY_ENABLE_UNVALIDATED_INTRADAY") == "1"
        and values.get("POLY_INTRADAY_EXPERIMENT_ACK") == EXPERIMENT_ACKNOWLEDGEMENT
    )


def retirement_frame() -> dict[str, object]:
    return {
        "paper_only": True,
        "strategy_status": RETIREMENT_STATUS,
        "strategy_reason": RETIREMENT_REASON,
        "new_entries_allowed": False,
        "historical_evidence_only": True,
    }


def fetch(url: str, timeout: float):
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp:
            if resp.status != 200:
                return None
            return json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None


class IntradayTracker:
    """Owns bankroll, open positions, settled-bet log for the mean-rev strategy."""

    def __init__(self, out_dir: str, principal: float, stake: float):
        os.makedirs(out_dir, exist_ok=True)
        self.out_dir = out_dir
        self.stake = stake
        self.principal = principal
        self.bankroll = principal
        self.started_at = int(time.time() * 1000)
        self.positions: dict[tuple, dict] = {}
        self.settled: list[dict] = []
        # Bankroll timeline — appended every save, capped so the file stays
        # small. 2000 points at ~5s cadence ≈ 2.8 hours of chartable history.
        self.bankroll_series: list[tuple[int, float]] = []
        self.state_path = os.path.join(out_dir, "intraday-state.json")
        self.report_path = os.path.join(out_dir, "intraday-report.json")
        self._load()

    # ---------- persistence -------------------------------------------------

    def _load(self):
        try:
            s = json.load(open(self.state_path))
        except (OSError, ValueError):
            return
        self.principal = s.get("principal", self.principal)
        self.bankroll = s.get("bankroll", self.principal)
        self.started_at = s.get("started_at_ms", self.started_at)
        self.settled = s.get("settled", [])
        self.bankroll_series = [(int(t), float(v))
                                for t, v in s.get("bankroll_series", [])[-2000:]]
        for row in s.get("positions", []):
            key = row.get("key")
            if not isinstance(key, list) or len(key) != 4:
                continue
            pos = {k: v for k, v in row.items() if k != "key"}
            self.positions[tuple(key)] = pos
        print(f"resumed: bankroll=${self.bankroll:.2f} "
              f"settled={len(self.settled)} open={len(self.positions)} "
              f"series={len(self.bankroll_series)}", flush=True)

    def _save(self):
        # Sample bankroll (mark-to-market including open positions' last-bid
        # value) into the timeline before writing state so the chart tracks
        # unrealised P&L too, not just realised bankroll.
        now_ms = int(time.time() * 1000)
        mtm = self.bankroll + sum(
            (p.get("last_bid") or p["entry_price"]) * p["shares"]
            for p in self.positions.values()
        )
        if not self.bankroll_series or now_ms - self.bankroll_series[-1][0] >= 5_000:
            self.bankroll_series.append((now_ms, round(mtm, 4)))
            self.bankroll_series = self.bankroll_series[-2000:]

        tmp = self.state_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump({
                "started_at_ms": self.started_at,
                "principal": self.principal,
                "bankroll": self.bankroll,
                # No cap: month-long history in place, monthly archive by user.
                "settled": self.settled,
                "positions": [{"key": list(k), **v} for k, v in self.positions.items()],
                "bankroll_series": self.bankroll_series,
            }, f)
        os.replace(tmp, self.state_path)

        # Per-(window, asset, side_token) rollup so the UI can show which
        # cross-section actually pays. Same pattern as the directional
        # trader's per_window strip.
        per_window: dict[str, dict[str, float | int]] = {}
        per_asset: dict[str, dict[str, float | int]] = {}
        per_cell: dict[str, dict[str, float | int]] = {}
        for b in self.settled:
            for key_name, bucket in (
                (b.get("window", "?"), per_window),
                (b.get("asset", "?"), per_asset),
                (f"{b.get('asset','?')}-{b.get('window','?')}-{b.get('side_token','?')}", per_cell),
            ):
                slot = bucket.setdefault(key_name, {"bets": 0, "wins": 0, "pnl": 0.0})
                slot["bets"] = int(slot["bets"]) + 1
                slot["wins"] = int(slot["wins"]) + (1 if b.get("won") else 0)
                slot["pnl"] = round(float(slot["pnl"]) + float(b.get("pnl", 0.0)), 4)

        report = {
            "generated_at_ms": now_ms,
            "paper_only": True, "live": True, "intraday": True,
            "started_at_ms": self.started_at,
            "principal": self.principal,
            "bankroll": round(self.bankroll, 4),
            "mark_to_market": round(mtm, 4),
            "pnl": round(self.bankroll - self.principal, 4),
            "mtm_pnl": round(mtm - self.principal, 4),
            "settled": len(self.settled),
            "wins": sum(1 for b in self.settled if b.get("won")),
            "buy_threshold": BUY_THRESHOLD, "sell_threshold": SELL_THRESHOLD,
            "windows": list(WINDOWS),
            "per_window": per_window,
            "per_asset": per_asset,
            "per_cell": per_cell,
            "bankroll_series": self.bankroll_series[-500:],  # chart-friendly
            "open_positions": [
                {"asset": p["asset"], "window": p["window"],
                 "cond": p["condition_id"][:10], "side_token": p["side_token"],
                 "entry_price": p["entry_price"], "stake": p["stake"],
                 "shares": p["shares"],
                 "opened_ms": p["opened_ms"], "last_bid": p.get("last_bid")}
                for p in self.positions.values()
            ],
            "recent_settled": self.settled[-40:],
        }
        tmp = self.report_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump(report, f, indent=1)
        os.replace(tmp, self.report_path)

    def _append_bet_log(self, bet: dict):
        day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
        path = os.path.join(self.out_dir, f"intraday-bets-{day}.jsonl")
        try:
            with open(path, "a") as h:
                h.write(json.dumps(bet, separators=(",", ":")) + "\n")
        except OSError:
            pass

    # ---------- trading loop ------------------------------------------------

    def tick(self, feed: dict):
        now_ms = int(time.time() * 1000)
        live: dict[str, dict] = {}
        for m in feed.get("markets", []):
            if m.get("window") not in WINDOWS or m.get("stale"):
                continue
            cond = m.get("cond") or m.get("condition_id")
            if cond:
                live[cond] = m

        # 1) Update last-seen bid for every open position and evaluate EXITS.
        for key in list(self.positions.keys()):
            asset, window, cond, side_token = key
            pos = self.positions[key]
            m = live.get(cond)
            if m is None:
                # Market vanished this tick.  Wait ONE more tick before
                # exiting (rollover-cache blip): only exit if the market's
                # own end_ms has passed.
                end_ms = pos.get("end_ms") or 0
                if now_ms >= end_ms:
                    # Exit at last known bid; if we never observed one, 0.
                    exit_bid = pos.get("last_bid") or 0.0
                    self._exit(key, pos, exit_bid, now_ms, "market_ended")
                continue
            book = m["up_book"] if side_token == "UP" else m["down_book"]
            bid = int(book.get("best_bid_micros", 0)) / DOLLAR
            pos["last_bid"] = bid
            if bid >= SELL_THRESHOLD:
                self._exit(key, pos, bid, now_ms, "target_hit")

        # 2) Evaluate ENTRIES on each live market x each side token.
        if len(self.positions) < MAX_CONCURRENT_POSITIONS:
            for cond, m in live.items():
                if self.bankroll < self.stake:
                    break
                if len(self.positions) >= MAX_CONCURRENT_POSITIONS:
                    break
                for side_token, book in (("UP", m["up_book"]), ("DOWN", m["down_book"])):
                    key = (m["asset"], m["window"], cond, side_token)
                    if key in self.positions:
                        continue
                    ask = int(book.get("best_ask_micros", 0)) / DOLLAR
                    bid = int(book.get("best_bid_micros", 0)) / DOLLAR
                    if not (0.02 < ask <= BUY_THRESHOLD):
                        continue
                    if self.bankroll < self.stake:
                        break
                    self._enter(key, m, side_token, ask, bid, now_ms)
                    if len(self.positions) >= MAX_CONCURRENT_POSITIONS:
                        break

        self._save()

    def _enter(self, key: tuple, market: dict, side_token: str,
               ask: float, bid: float, now_ms: int):
        asset, window, cond, _ = key
        stake = self.stake
        shares = stake / ask
        self.bankroll -= stake
        # Regime tag — recorded but NOT filtered on. Lets us analyze
        # in a week whether a regime filter (per scripts/analyze_regime.py)
        # would have improved returns out-of-sample.
        pos = {
            "asset": asset, "window": window, "condition_id": cond,
            "side_token": side_token,
            "entry_price": ask, "stake": stake, "shares": shares,
            "opened_ms": now_ms, "last_bid": bid,
            "start_ms": market.get("start_ms"), "end_ms": market.get("end_ms"),
            "reference_micros": market.get("reference_micros"),
            "open_obs": {
                "up_ask":  int(market["up_book"]["best_ask_micros"])   / DOLLAR,
                "up_bid":  int(market["up_book"]["best_bid_micros"])   / DOLLAR,
                "down_ask":int(market["down_book"]["best_ask_micros"]) / DOLLAR,
                "down_bid":int(market["down_book"]["best_bid_micros"]) / DOLLAR,
            },
        }
        self.positions[key] = pos
        print(f"  OPEN intraday {asset} {window} {side_token} @ {ask:.3f} "
              f"stake ${stake:.2f}  bankroll ${self.bankroll:.2f}", flush=True)

    def _exit(self, key: tuple, pos: dict, exit_price: float,
              now_ms: int, reason: str):
        proceeds = pos["shares"] * exit_price
        pnl = proceeds - pos["stake"]
        self.bankroll += proceeds
        del self.positions[key]
        bet = {
            "asset": pos["asset"], "window": pos["window"],
            "condition_id": pos["condition_id"],
            "side_token": pos["side_token"],
            "opened_ms": pos["opened_ms"], "closed_ms": now_ms,
            "held_ms": now_ms - pos["opened_ms"],
            "entry_price": round(pos["entry_price"], 4),
            "exit_price": round(exit_price, 4),
            "stake": round(pos["stake"], 4),
            "shares": round(pos["shares"], 6),
            "pnl": round(pnl, 4), "won": pnl > 0,
            "bankroll_after": round(self.bankroll, 4),
            "exit_reason": reason,
            "open_obs": pos.get("open_obs"),
            "start_ms": pos.get("start_ms"), "end_ms": pos.get("end_ms"),
            "logged_at_ms": now_ms,
        }
        self.settled.append(bet)
        self._append_bet_log(bet)
        print(f"  EXIT intraday {pos['asset']} {pos['window']} {pos['side_token']} "
              f"@ {exit_price:.3f} pnl {pnl:+.2f} ({reason}) "
              f"bankroll ${self.bankroll:.2f}", flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default=os.environ.get("POLY_WINDOWS_URL", DEFAULT_URL))
    ap.add_argument("--out-dir", default="var/live-paper")
    ap.add_argument("--interval", type=float, default=5.0)
    ap.add_argument("--timeout", type=float, default=4.0)
    ap.add_argument("--principal", type=float, default=1000.0)
    ap.add_argument("--stake", type=float, default=DEFAULT_STAKE)
    ap.add_argument("--retirement-check", action="store_true",
                    help="print the non-authorizing retirement state and exit")
    args = ap.parse_args()

    if args.retirement_check:
        print(json.dumps(retirement_frame(), sort_keys=True))
        return 0
    if not experiment_enabled():
        print(json.dumps(retirement_frame(), sort_keys=True), flush=True)
        print(
            "No simulated entries were created. This retired challenger requires "
            "POLY_ENABLE_UNVALIDATED_INTRADAY=1 plus an explicit deployment "
            "acknowledgement for a separately governed experiment.",
            flush=True,
        )
        return 0

    tracker = IntradayTracker(args.out_dir, args.principal, args.stake)
    print(f"paper intraday tracker starting", flush=True)
    print(f"  polling {args.url} every {args.interval:g}s", flush=True)
    print(f"  windows={WINDOWS}  buy<={BUY_THRESHOLD}  sell>={SELL_THRESHOLD}  "
          f"stake=${args.stake:.2f}  max_open={MAX_CONCURRENT_POSITIONS}", flush=True)
    print(f"PAPER ONLY — no credential, order, wallet, or capital authority.",
          flush=True)

    skipped = 0
    while True:
        start = time.time()
        feed = fetch(args.url, args.timeout)
        if feed and feed.get("available") is True and isinstance(feed.get("markets"), list):
            tracker.tick(feed)
        else:
            skipped += 1
            if skipped % 60 == 0:
                print(f"  waiting ({feed.get('reason') if feed else 'unreachable'})",
                      flush=True)
        time.sleep(max(0.0, args.interval - (time.time() - start)))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("\nstopped.")
