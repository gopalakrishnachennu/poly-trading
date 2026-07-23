# Phase 1.5 Specification: Reference Market Data

## Objective

Capture and deterministically replay the exact public BTC/USDT and ETH/USDT
settlement-reference candles alongside separately typed predictive observations.

## Functional requirements

1. Accept only Binance Spot `BTCUSDT` and `ETHUSDT`.
2. Accept only UTC `1h` candles; distinguish open from finalized candles.
3. Capture aggregate trades and best bid/ask as predictive inputs only.
4. Preserve source event time and receive time independently. Represent absent
   source event time explicitly.
5. Parse quote prices into integer micros and Binance reference quantities into
   distinct 1e-8 fixed-point integers without rounding.
6. Validate stream name, embedded symbol, candle bounds, OHLC relationships,
   trade bounds, and uncrossed best prices.
7. Journal before optional bounded delivery; never silently drop an event.
8. Record start, synchronized, rotated, disconnected, and shutdown transitions.
9. Replay contiguous envelopes transactionally with source-specific monotonic
   IDs and immutable finalized candles.
10. Provide a deterministic state digest and read-only capture/replay CLI.

## Acceptance criteria

- Malformed, unsupported, crossed, imprecise, regressing, or conflicting data
  fails closed.
- Readiness requires all three feed classes for both symbols in one epoch.
- A version-2 decoder round trip preserves every fixed-point field exactly.
- Book ticker source time is absent, not replaced with local receive time.
- A failed replay transition leaves the prior state and digest unchanged.
- Workspace formatting, Clippy with denied warnings, and tests pass.
- A short public live capture replays to the same terminal digest.

## Exclusions

Strategies, model inference, authenticated Binance APIs, private data, order
submission, capital allocation, and Polymarket execution.
