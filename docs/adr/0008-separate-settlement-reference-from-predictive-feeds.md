# ADR 0008: Separate settlement-reference from predictive feeds

- Status: Accepted
- Date: 2026-07-17

## Context

Hourly BTC and ETH market resolution depends on a named Binance spot one-hour
candle. Other Binance observations can improve a future model but are not the
resolution fact. Treating all underlying prices as interchangeable can produce
a correct prediction against the wrong oracle.

## Decision

The public reference gateway records six unauthenticated Binance Spot streams:

```text
btcusdt@kline_1h       ethusdt@kline_1h
btcusdt@aggTrade       ethusdt@aggTrade
btcusdt@bookTicker     ethusdt@bookTicker
```

It uses Binance's official market-data-only `data-stream.binance.vision`
endpoint. This endpoint exposes the same public Spot stream contracts without
supporting user data.

Only a `kline_1h` event with `x=true` becomes `FinalizedCandle` and may serve as
settlement-reference evidence. An open candle is typed `InProgressCandle`.
Aggregate trades and book tickers are always predictive observations.

Underlying prices use `QuotePriceMicros`, distinct from the `[0, 1]`
prediction-token `PriceMicros`. All decimal conversion is exact and rejects
sub-micro precision.

Book ticker messages do not carry a source event timestamp. Their event time is
the explicit sentinel `-1`; receive time is retained separately and is never
silently substituted.

Aggregate-trade IDs and book-ticker update IDs must increase within a connection
epoch but may have gaps. Finalized candles are immutable by symbol and open
time. Conflicts halt replay.

The gateway is journal-first, uses a bounded optional live channel, records all
epoch transitions, and becomes ready only after candle, aggregate-trade, and
book-ticker observations have arrived for both configured symbols. Connections
rotate before Binance's 24-hour limit.

## Consequences

- Settlement and prediction code cannot accidentally consume the same type.
- Source-time absence remains visible to freshness and research logic.
- Reconnects invalidate ephemeral predictive state but retain immutable
  finalized candles.
- This milestone adds no strategy, credential, authenticated stream, or order
  path.
