# Tick-by-Tick Research Capture

The one-second paper-session projection remains an operator view. This runbook
starts the authoritative raw research capture, which persists every accepted
public event independently before any downstream delivery.

```sh
sh terminal/scripts/run-tick-capture.sh
```

On a new explicit start it creates two checksummed, bounded segment directories
under `var/tick-capture` by default:

```text
public-clob-segments/       Polymarket CLOB market ticks
reference-market-segments/  Binance reference ticks
```

Each directory contains contiguous 256 MiB / 1,000,000-record journal
segments. Completed segments are immutable export inputs. The active segment
must not be exported as a final research artifact until a verified snapshot
operation has established its exact durable prefix.

Legacy `public-clob.journal` and `reference-market.journal` files are retained
as historical inputs. The launcher never migrates, truncates, renames, or
deletes them. Stop a legacy capture explicitly before starting the new
segmented layout; do not run two capture processes against the same public
feeds unless you deliberately require duplicate independent observations.

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
