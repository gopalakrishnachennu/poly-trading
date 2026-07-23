# Phase 1.10 Specification: Read-Only Integration and Soak Daemon

## Objective

Exercise the complete read-only control plane across supervised feed snapshots,
hourly session sources, durable coordination, rollover, final evidence,
checkpoint recovery, and scripted failures over many consecutive hours.

## Requirements

1. One deterministic integration owner evaluates cross-feed supervision before
   creating and durably applying a session frame.
2. Production adapters capture exact session source state from existing book
   and reference replay cores without network or state mutation.
3. Fault scripts are explicit, ordered, bounded, and divided between pre-
   supervision feed faults and post-supervision session-frame faults.
4. Recoverable faults produce explicit degraded sessions; integrity faults
   halt and never silently continue.
5. The soak runner generates deterministic BTC and ETH identities, books,
   candles, timestamps, feed sequences, rollovers, and final evidence using
   integer fixed-point values only.
6. At every tick there is at most one current session per asset. At completion,
   every generated session has immutable final evidence.
7. Runs are bounded by configured hours, ticks per hour, sessions, and faults.
8. Identical plans and scripts produce identical reports and coordinator
   digests. Restart/checkpoint recovery must match uninterrupted execution.
9. The CLI supports bounded synthetic soak and strict durable recovery/reporting.
10. Live external networking remains outside this brick and fail-closed until
    the Phase 1.5 eligible-network smoke passes.

## Acceptance criteria

- Tests cover multi-hour BTC/ETH rollover, finalization, recoverable feed
  degradation, book/candle degradation, integrity halt, deterministic reports,
  restart equivalence, invalid bounds, and capture adapter behavior.
- A formal model verifies at-most-one-current, final evidence retention,
  bounded progress, and absorbing integrity halt.
- Formatting, denied-warning Clippy, all tests, and all TLC models pass.

## Exclusions

Strategies, predictions, authenticated feeds, orders, positions, capital
allocation, wallet operations, signing, and real external network execution.
