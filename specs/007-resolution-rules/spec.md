# Phase 1.6 Specification: Oracle and Resolution Rules

## Objective

Bind each validated hourly BTC/ETH market to its exact finalized Binance candle
and produce deterministic, auditable outcome evidence without claiming external
Polymarket resolution.

## Requirements

1. Preserve the complete market rules description in immutable discovery
   identity and its existing rules fingerprint.
2. Accept only configured BTC and ETH hourly series with matching assets.
3. Accept only the exact Binance Spot BTC/USDT or ETH/USDT resolution URL.
4. Recognize the reviewed rule clauses: finalized `1h` candle, exact pair,
   `close >= open` means `Up`, otherwise `Down`, and no other exchange/pair.
5. Require an hour-aligned UTC start and exactly one-hour market interval.
6. Bind the Binance candle close timestamp to `market_end_ms - 1`.
7. Reject symbol, interval, open-time, or close-time mismatches.
8. Keep in-progress assessment explicitly indicative and non-final.
9. Create final evidence only from `FinalizedCandle`.
10. Make identical finalization idempotent and conflicting finalization a
    transactional failure.
11. Encode evidence with an explicit versioned, checksummed schema and stable
    digest.

## Acceptance criteria

- BTC and ETH contracts bind with the reviewed current rule text.
- Changed source, rule language, series, symbol, or time window fails closed.
- `close == open` resolves `Up`; `close < open` resolves `Down`.
- In-progress data never creates `ResolutionEvidence`.
- Evidence round trips exactly and checksum corruption is rejected.
- A conflicting final candle leaves prior state and evidence unchanged.
- Formatting, Clippy with denied warnings, workspace tests, and TLC pass.

## Exclusions

Polymarket resolution proposals, disputes, 50/50/invalid markets, on-chain
confirmation, redemption, positions, strategies, authentication, and orders.
