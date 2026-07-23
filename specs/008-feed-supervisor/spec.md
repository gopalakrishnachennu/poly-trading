# Phase 1.7 Specification: Cross-Feed Supervisor

## Objective

Combine independent Polymarket and Binance snapshots into one deterministic,
fail-closed health decision with explicit freshness and clock-integrity rules.

## Requirements

1. The deterministic core receives caller-supplied nanosecond time and performs
   no wall-clock or network I/O.
2. Market snapshots expose latest source event and local receive timestamps.
3. Reference snapshots expose candle, aggregate-trade, and book-ticker receive
   timestamps independently for BTC and ETH. Timestamp state participates in
   the reference digest.
4. `READY` requires both feed states ready and every required timestamp present.
5. Enforce independent market/reference staleness budgets, maximum cross-feed
   receive skew, maximum source-event lag, and maximum future source skew.
6. Exact configured time boundaries are accepted.
7. Feed unavailable, stale, skewed, lagging, and source-future states are
   distinct, non-ready, and recoverable.
8. Local clock regression, future receive time, feed history regression, and
   digest equivocation permanently halt the supervisor.
9. A failed transition retains the last accepted feed markers.
10. Identical ordered observations produce an identical state digest through
    online application and replay.

## Acceptance criteria

- Tests cover every mode and exact boundary behavior.
- Stale and unavailable states recover after valid newer snapshots.
- Per-component reference staleness cannot be hidden by another active stream.
- Clock, sequence, epoch, receive-time, and digest-integrity failures halt.
- A halted supervisor cannot return to ready.
- Reference receive-time regression is rejected transactionally.
- Formatting, Clippy with denied warnings, workspace tests, and TLC pass.

## Exclusions

Trading decisions, fair value, signals, authenticated feeds, orders, positions,
risk allocation, signing, and external monitoring infrastructure.
