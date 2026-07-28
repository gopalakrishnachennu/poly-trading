# Paused 2026-07-28 — resume notes for next week

The user is taking a break this week for other work. Everything below is
what you need to pick this back up cold, without re-deriving context.

## What was stopped, and how

`scripts/run-terminal-stack.sh stop` was run. This kills the supervisor and
every child it manages: gateway, capture, snapshot recorder, terminal
(Next.js), `btc-window-paper`, and the BTC-hourly research cycle. Confirmed
all down via `scripts/run-terminal-stack.sh status` (every line reads DOWN).

No data capture is running. No paper trading is running. Nothing is
listening on :8088 or :3000.

## How to resume

```bash
nohup scripts/run-terminal-stack.sh > var/log/stack-supervisor.log 2>&1 &
sleep 15
scripts/run-terminal-stack.sh status   # every line should read "up"
```

Then open the terminal at `http://localhost:3000` (F4 POSITIONS for the
paper book, or use the Poly Advisor MCP in `advisor/` for programmatic
queries).

## State of the paper book at pause time

- **Principal:** $1,000
- **Net realized P&L:** **+$3.05** (bankroll $1,003.05)
- This is the *recovered* number — earlier this week the book was down
  **−$413** (41% drawdown) before two fixes landed (see below). It swung
  from a 41% loss to roughly breakeven within about a day of the fixes.
- Active window policy: **version 2** (`var/strategy-control/active.json`),
  digest `54e75db9...`.

## What changed this week, in order, and why

1. **Diagnosed the −$413 drawdown** by pulling the full decision journal
   (`var/btc-window-paper/decisions-*.jsonl`) and aggregating realized P&L
   by asset/window/setup_id/side/exit-reason. Found the loss was
   concentrated in 3 specific setups, not spread evenly.

2. **Repeatability test** (the actual quant discipline requested): split
   every (setup_id, side) combination's P&L by day. Only trusted a pattern
   if it had the **same sign on two independent days**. This separated real
   structural losers from one-off noise.

3. **Disabled 3 proven repeat-negative setups** via a new `disable`
   subcommand added to the `btc-window-policy` CLI
   (`crates/btc-window-strategy/src/bin/btc_window_policy.rs`). This reuses
   the existing validated save+activate path — no hand-edited JSON, no
   manual digest computation. Disabled:
   - `5m_confirmed_mid_window_favourite` (−$60 then −$197, worsening)
   - `15m_seven_minute_acceleration` (−$84 then −$65)
   - `15m_eleven_minute_continuation` (both sides, small consistent losses)

   This produced policy version 2 (`var/strategy-control/versions/v0002.json`).
   Command used, if you ever need to reproduce the pattern for a new setup:
   ```bash
   ./target/release/btc-window-policy disable "setup_id_1,setup_id_2"
   ```

4. **Found and fixed a second, cross-cutting cost**: `EMERGENCY_PRICE_STOP`
   was the single largest loss-reason bucket even after the setup disable
   (256 fires / −$848 over 48h; still 19 fires / −$41 in the ~4h right
   after the disable — a *larger* share of remaining losses, proving it
   wasn't specific to the bad setups). The stop was firing on ordinary
   short-window noise (6-8 cent adverse moves on a 0.58-0.78 priced token).
   Widened the bands in `crates/btc-window-strategy/src/config.rs`:
   - 5m: 6/8 points -> 8/10 points
   - 15m: 8/10 points -> 10/12 points
   - 1h: unchanged
   Documented in `docs/adr/0101-widen-short-window-emergency-stop-bands.md`.
   All 47 crate tests pass; release binary rebuilt; engine restarted on it.

5. Net effect of steps 3+4: bankroll went from −$413 to +$3.05.

## What is NOT yet resolved — pick this up first

- **The stop-band widening has only ~1 day of evidence.** Before trusting
  it as fixed, re-run the same fire-count/P&L-by-reason breakdown over a
  full 24-48h under the new bands. If `EMERGENCY_PRICE_STOP` fire count
  and its P&L share dropped meaningfully, the fix worked. If not, the
  bands may need another iteration, or the mechanism needs different logic
  (e.g. momentum-based rather than fixed-distance).
- **Two 5m setups flipped negative in the ~4h window right after the
  disable** (`5m_late_continuation`, `5m_high_confidence_confirmation`),
  reversing their prior 2-day positive pattern. Per the repeatability rule
  used all week: **one bad short window is not enough evidence to act on.**
  Do NOT disable these without first checking whether the pattern repeats
  over a full day under the new (wider) stop bands — the widening itself
  may have already fixed whatever was cutting them short.
- The BTC-hourly research cycle (`var/research-cycles/`) is still at
  `NO_TRADE` — no model has passed its frozen walk-forward gate. That is
  the correct, honest state; it only changes when a genuinely validated
  model exists (session count needs to keep growing toward the ~100+
  threshold in `docs/STATUS.md`).

## How to re-run the analysis that found all of this

The pattern that worked all week, reusable for any future drawdown:

```bash
# Pull closed (settled + exited) trades from the journal
grep -h '"action":"settle"\|"action":"exit"' \
    var/btc-window-paper/decisions-*.jsonl > /tmp/closed.jsonl

# Then in Python: group by (setup_id, side), split by day (observed_at_ns),
# and only trust a pattern that has the same P&L sign on 2+ independent days.
```

Full worked version of this script is not committed (it was ad hoc), but
the field names to group on are: `asset`, `window`, `setup_id`, `side`,
`reason` (exit/settle reason), `outcome`, `realized_pnl_micros`,
`stake_micros`, `observed_at_ns`.

## Repo / GitHub

Latest tag: **v4.0.0**. Everything through this pause point is pushed to
`main` on `https://github.com/gopalakrishnachennu/poly-trading` — see the
commit that references this pause for the exact state.
