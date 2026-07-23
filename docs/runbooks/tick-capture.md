# Tick-by-Tick Research Capture

The one-second paper-session projection remains an operator view. This runbook
starts the authoritative raw research capture, which persists every accepted
public event independently before any downstream delivery.

```sh
sh terminal/scripts/run-tick-capture.sh
```

It creates two checksummed journals under `var/tick-capture` by default:

```text
public-clob.journal       Polymarket CLOB market ticks
reference-market.journal  Binance reference ticks
```

Set `POLY_TICK_JOURNAL_DIR` to select a different explicit directory. Stop it
with `Ctrl-C`; the recorder closes cleanly and preserves the durable prefix.

## Captured values

### Polymarket CLOB

- Every validated `book` snapshot with full bid/ask depth.
- Every `price_change`, `best_bid_ask`, `last_trade_price` and
  `tick_size_change` event.
- Exact source event timestamp, local receive timestamp, contiguous local
  sequence, condition ID, token IDs, canonical raw JSON and checksummed
  envelope.
- Capture epoch start/sync/reconnect/shutdown events.
- Immutable hourly identity: BTC/ETH asset, event/market/question/condition
  IDs, token IDs, start/end time, market and series slugs, resolution source,
  rules description and rules fingerprint.

### Binance reference market

- Aggregate trades, best bid/ask updates and one-hour candles.
- Exact source/receive-time distinction, epoch and replay state.
- Finalized candles remain settlement evidence; predictive ticks never become
  resolution truth.

## Guarantees and limits

Each accepted event is journaled before a live consumer sees it. Oversized,
unsubscribed, malformed, unknown-type or timestamp-invalid events terminate the
capture epoch rather than being silently skipped. Disconnects force a fresh
discovery and epoch; cached books never remain authoritative across reconnects.

This is read-only data capture. It has no credential, signing, wallet, order,
capital or live-trading capability.
