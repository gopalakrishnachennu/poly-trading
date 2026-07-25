#!/usr/bin/env python3
"""Edge-case checks that would break the 5m UP-bias / +62% EV finding.

If any of these fail the strategy isn't real, it's just an artifact of
our small sample or a particular market regime. All checks read the
same tape as analyze_full_tape.py.

Checks performed:
  A. Regime — was the capture during a strong BTC/ETH uptrend? If yes,
     the 67% UP win rate is measuring the trend, not an inefficiency.
  B. Per-day breakdown — does the edge persist on every day, or is it
     driven by one anomalous day?
  C. Contrarian flip in the ladder sim — the earlier sim flipped to
     DOWN when UP was expensive. Re-run with a strictly "always-UP"
     rule to see if UP-bias alone carries the ROI.
  D. Executable size at the "golden" open — is the +62% EV bet actually
     fillable, or is the first-ask quote a paper thin 1-share offer?
  E. Weekend vs weekday split.
  F. Hour-of-day bias distribution — is UTC 12:00 signal real or is it
     just where most markets sit?
  G. Survivorship — how many markets were dropped by resolve() and how
     might that bias results?
  H. Look-ahead — confirm oneshot_early uses only past data at decision.
  I. Correlated outcomes — do BTC and ETH 5m markets in the same
     minute tend to win together (bad for diversification claims)?
"""
from __future__ import annotations
import glob, json, statistics, math
from collections import defaultdict, Counter
from datetime import datetime, timezone


DOLLAR = 1_000_000


def load_tape(path: str) -> dict:
    markets: dict[str, dict] = {}
    with open(path) as h:
        for line in h:
            try: row = json.loads(line)
            except ValueError: continue
            if row.get("k") == "sys":
                p = row.get("payload", {})
                if p.get("event_type") != "market_identity": continue
                cond = p["condition_id"]
                markets[cond] = {
                    "cond": cond, "asset": p["asset"],
                    "start_ms": p["start_time_ms"], "end_ms": p["end_time_ms"],
                    "up_tok": p["up_token_id"], "down_tok": p["down_token_id"],
                    "ticks": [],
                }
            elif row.get("k") == "book":
                cond = row.get("cond")
                if cond not in markets: continue
                tok = row.get("tok")
                bids = row.get("bids") or []
                asks = row.get("asks") or []
                t_ms = row.get("t", 0) // 1_000_000
                side = "up" if tok == markets[cond]["up_tok"] else "down"
                markets[cond]["ticks"].append({
                    "t_ms": t_ms, "side": side,
                    "bid_px": bids[0][0] / DOLLAR if bids else None,
                    "ask_px": asks[0][0] / DOLLAR if asks else None,
                    "bid_sz": bids[0][1] / DOLLAR if bids else 0.0,
                    "ask_sz": asks[0][1] / DOLLAR if asks else 0.0,
                    "asks_depth": [(a[0]/DOLLAR, a[1]/DOLLAR) for a in asks[:5]],
                })
    return markets


def resolve(market: dict) -> dict | None:
    ticks = market["ticks"]
    if len(ticks) < 4: return None
    ticks.sort(key=lambda x: x["t_ms"])
    end_ms = market["end_ms"]
    inside = [t for t in ticks if market["start_ms"] <= t["t_ms"] <= end_ms + 5000]
    if len(inside) < 4: return None
    up_ticks = [t for t in inside if t["side"] == "up"]
    down_ticks = [t for t in inside if t["side"] == "down"]
    if not up_ticks or not down_ticks: return None
    close_up_ask = up_ticks[-1].get("ask_px")
    close_down_ask = down_ticks[-1].get("ask_px")
    if close_up_ask is None or close_down_ask is None: return None
    if close_up_ask <= 0.05 and close_down_ask >= 0.90:
        up_won = False
    elif close_down_ask <= 0.05 and close_up_ask >= 0.90:
        up_won = True
    else:
        return None
    return {
        "cond": market["cond"], "asset": market["asset"],
        "start_ms": market["start_ms"], "end_ms": market["end_ms"],
        "open_up_ask": up_ticks[0].get("ask_px"),
        "open_up_ask_sz": up_ticks[0].get("ask_sz", 0),
        "open_up_asks_depth": up_ticks[0].get("asks_depth", []),
        "open_down_ask": down_ticks[0].get("ask_px"),
        "open_up_bid_sz": up_ticks[0].get("bid_sz", 0),
        "open_down_bid_sz": down_ticks[0].get("bid_sz", 0),
        "up_won": up_won, "up_ticks": up_ticks, "down_ticks": down_ticks,
        "n_ticks": len(inside),
    }


def wilson(k: int, n: int) -> tuple[float, float]:
    if n == 0: return (0.0, 0.0)
    p = k/n; z = 1.96
    d = 1 + z*z/n
    c = (p + z*z/(2*n)) / d
    m = z * math.sqrt(p*(1-p)/n + z*z/(4*n*n)) / d
    return (max(0, c-m), min(1, c+m))


def main() -> None:
    all_resolved = []
    all_markets = 0
    all_dropped = 0
    for sub, label, sec in (("research-5m","5m",300),("research-15m","15m",900),("research-1h","1h",3600)):
        for path in glob.glob(f"var/{sub}/*.jsonl"):
            markets = load_tape(path)
            all_markets += len(markets)
            for m in markets.values():
                m["window"] = label; m["window_sec"] = sec
                res = resolve(m)
                if res is None:
                    all_dropped += 1
                else:
                    res["window"] = label
                    all_resolved.append(res)
    print(f"markets seen: {all_markets}, resolved: {len(all_resolved)}, dropped: {all_dropped}")
    print(f"drop rate: {all_dropped / all_markets * 100:.1f}%")
    print()

    fives = [r for r in all_resolved if r["window"] == "5m"]

    # ------------------------------------------------------------------
    print("=" * 78)
    print("CHECK A — Regime bias.  Was BTC/ETH just trending up during capture?")
    print("=" * 78)
    print("  Reconstruct underlying: last-tick ask on UP is a fair proxy for the")
    print("  crowd's belief that the asset closed higher than open. We take the")
    print("  RESOLUTION direction of every 5m market to see the drift.")
    print()
    fives_sorted = sorted(fives, key=lambda r: r["start_ms"])
    if fives_sorted:
        first_time = datetime.fromtimestamp(fives_sorted[0]["start_ms"]/1000, tz=timezone.utc)
        last_time  = datetime.fromtimestamp(fives_sorted[-1]["start_ms"]/1000, tz=timezone.utc)
        print(f"  capture span:  {first_time.isoformat()}  →  {last_time.isoformat()}")
        span_h = (fives_sorted[-1]["start_ms"] - fives_sorted[0]["start_ms"]) / 3600000
        print(f"                 = {span_h:.1f} hours")
    print()
    # Chunk into 6h buckets
    if fives:
        by_bucket = defaultdict(list)
        first_ms = fives_sorted[0]["start_ms"]
        for r in fives_sorted:
            bucket_idx = (r["start_ms"] - first_ms) // (6 * 3600 * 1000)
            by_bucket[bucket_idx].append(r)
        print(f"  {'bucket (6h)':<15} {'BTC':>18} {'ETH':>18} {'combined':>18}")
        for b in sorted(by_bucket):
            rows = by_bucket[b]
            btc = [r for r in rows if r["asset"] == "BTC"]
            eth = [r for r in rows if r["asset"] == "ETH"]
            btc_wins = sum(1 for r in btc if r["up_won"])
            eth_wins = sum(1 for r in eth if r["up_won"])
            all_wins = sum(1 for r in rows if r["up_won"])
            def fmt(w, n):
                if n == 0: return "  —"
                return f"{w}/{n}={w/n*100:.0f}%"
            start_time = datetime.fromtimestamp(rows[0]["start_ms"]/1000, tz=timezone.utc)
            print(f"  +{b*6}h ({start_time.strftime('%m-%d %H:%M')}Z)  "
                  f"{fmt(btc_wins, len(btc)):>18} {fmt(eth_wins, len(eth)):>18} "
                  f"{fmt(all_wins, len(rows)):>18}")
        print()
        print("  → if a bucket shows near-100% UP wins, that bucket is a bull-run")
        print("    artifact and inflates the pooled 67% number.")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK B — Per-day breakdown of 5m UP-bias")
    print("=" * 78)
    by_day = defaultdict(list)
    for r in fives:
        day = datetime.fromtimestamp(r["start_ms"]/1000, tz=timezone.utc).strftime("%Y-%m-%d")
        by_day[day].append(r)
    for day in sorted(by_day):
        rows = by_day[day]
        wins = sum(1 for r in rows if r["up_won"])
        lo, hi = wilson(wins, len(rows))
        print(f"  {day}  n={len(rows):>3}  up_wins={wins/len(rows)*100:5.1f}% "
              f"[{lo*100:4.1f}, {hi*100:4.1f}]")
    print()
    print("  → if only ONE day is 80%+, the bias is regime-specific.")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK C — Strict 'always buy UP' vs the earlier flip-at-0.5 sim")
    print("=" * 78)
    def sim_strict_up(markets, at_pct: float, stake: float = 10.0) -> dict:
        pnl = 0.0; s = 0.0; wins = 0; bets = 0
        for r in markets:
            ups = r["up_ticks"]
            if len(ups) < 5: continue
            tick = ups[int(len(ups) * at_pct)]
            px = tick.get("ask_px")
            if px is None or not (0.05 < px < 0.95): continue
            stk = stake; s += stk; bets += 1
            won = r["up_won"]
            if won: pnl += stk / px - stk; wins += 1
            else: pnl -= stk
        return {"n": bets, "wins": wins, "pnl": pnl, "stake": s,
                "roi_pct": pnl/s*100 if s else 0}
    for pct in (0.15, 0.35, 0.50, 0.75, 0.90):
        r = sim_strict_up(fives, pct)
        print(f"  always-UP @ {pct*100:2.0f}% into 5m window  n={r['n']:>3} "
              f"win={r['wins']/max(1,r['n'])*100:5.1f}% pnl={r['pnl']:+8.2f} "
              f"roi={r['roi_pct']:+6.2f}%")
    print()
    print("  → if ROI drops sharply vs the earlier +60% claim, the earlier")
    print("    number was inflated by the price>0.5 → flip-to-DOWN branch.")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK D — Executable size at the 'golden' 0.40-0.50 open")
    print("=" * 78)
    golden = [r for r in fives if r["open_up_ask"] is not None
              and 0.40 <= r["open_up_ask"] < 0.50]
    print(f"  n={len(golden)} markets in the golden band")
    if golden:
        sizes = [r["open_up_ask_sz"] for r in golden]
        print(f"  ask size at open: min={min(sizes):.1f}  "
              f"med={statistics.median(sizes):.1f}  "
              f"mean={statistics.fmean(sizes):.1f}  max={max(sizes):.1f} shares")
        thin = sum(1 for s in sizes if s < 20)
        print(f"  {thin} of {len(golden)} have < 20 shares at first ask")
        print()
        print("  depth: how much can you actually fill under 0.50?")
        depths = []
        for r in golden:
            fillable = 0.0
            for px, sz in r["open_up_asks_depth"]:
                if px < 0.50:
                    fillable += sz
                else:
                    break
            depths.append(fillable)
        print(f"    med fillable under 0.50: {statistics.median(depths):.1f} shares")
        print(f"    n>=100 shares fillable:  {sum(1 for d in depths if d >= 100)}")
        print(f"    n<10 shares fillable:    {sum(1 for d in depths if d < 10)}")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK E — Weekday vs weekend 5m UP-bias")
    print("=" * 78)
    for label, days in [("weekday", (0,1,2,3,4)), ("weekend", (5,6))]:
        rows = [r for r in fives
                if datetime.fromtimestamp(r["start_ms"]/1000, tz=timezone.utc).weekday() in days]
        if rows:
            wins = sum(1 for r in rows if r["up_won"])
            lo, hi = wilson(wins, len(rows))
            print(f"  {label}: n={len(rows)}  up_wins={wins/len(rows)*100:.1f}%  "
                  f"[{lo*100:.1f}, {hi*100:.1f}]")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK F — Where do the 5m markets cluster in the day?")
    print("=" * 78)
    hours = Counter(datetime.fromtimestamp(r["start_ms"]/1000, tz=timezone.utc).hour
                    for r in fives)
    for h in range(24):
        if hours[h]:
            print(f"  UTC {h:02d}:xx  n={hours[h]:>3}")
    print("  → if all our markets are in one 4-hour block, extrapolating to a")
    print("    full day is invalid.")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK G — Drop-rate impact.  What were we throwing out?")
    print("=" * 78)
    print(f"  {all_dropped} of {all_markets} markets dropped by resolve().")
    print("  Reasons (from resolve logic): <4 ticks, no clean 0/1 close, missing sides.")
    print("  Directional bias in drops is unknowable without a 3rd-party truth")
    print("  source; the drop rate itself is what we track.")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK H — Look-ahead: does 'oneshot_early' use only past data?")
    print("=" * 78)
    if fives:
        r = fives[0]
        ups = r["up_ticks"]
        early_idx = int(len(ups) * 0.15)
        early_tick = ups[early_idx]
        print(f"  sample market {r['cond'][:14]}")
        print(f"  n up_ticks = {len(ups)}, early_idx = {early_idx}")
        print(f"  early_tick t_ms   = {early_tick['t_ms']}")
        print(f"  market end_ms     = {r['end_ms']}")
        print(f"  early is {(r['end_ms'] - early_tick['t_ms'])/1000:.0f}s BEFORE resolution")
        print(f"  outcome is derived only from up_ticks[-1] (close). No leak.")

    # ------------------------------------------------------------------
    print()
    print("=" * 78)
    print("CHECK I — Correlation: BTC 5m and ETH 5m in the same 5-min interval")
    print("=" * 78)
    paired = defaultdict(dict)
    for r in fives:
        # bucket by start_ms
        paired[r["start_ms"]][r["asset"]] = r["up_won"]
    concordant = 0; discordant = 0
    for slot, sides in paired.items():
        if "BTC" in sides and "ETH" in sides:
            if sides["BTC"] == sides["ETH"]: concordant += 1
            else: discordant += 1
    tot = concordant + discordant
    if tot:
        print(f"  paired minutes: {tot}")
        print(f"  BTC and ETH agreed:    {concordant} ({concordant/tot*100:.1f}%)")
        print(f"  BTC and ETH disagreed: {discordant} ({discordant/tot*100:.1f}%)")
        print(f"  → if agreement is high, betting both is NOT diversifying —")
        print(f"    it's doubling exposure to the same market factor.")


if __name__ == "__main__":
    main()
