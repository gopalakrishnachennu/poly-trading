# Specification 002: Public Market Discovery and Capture

## Purpose

Discover active hourly BTC and ETH Up/Down markets and record bounded public CLOB
market events without credentials or trading capability.

## Requirements

### PMC-001 Series identity

Discovery uses configured series ID/slug pairs. A response whose series slug
does not match the configured ID is rejected.

Initial pairs:

```text
10114 -> btc-up-or-down-hourly
10117 -> eth-up-or-down-hourly
```

### PMC-002 Market identity

A discovered market is eligible only when event and market are active, open,
order-book enabled, accepting orders, inside the requested UTC time window, and
contain exactly `Up` and `Down` outcomes with exactly two decimal token IDs.

Condition ID and question ID must be 32-byte hexadecimal identifiers. Start and
end timestamps must describe exactly one hour. The resolution source and rules
description are mandatory and included in an immutable rules fingerprint.

### PMC-003 Bounded discovery

Use Gamma keyset pagination with bounded page count, result count, response body,
and request timeout. Cursor repetition is a hard error.

### PMC-004 Public subscription

Subscribe to `wss://ws-subscriptions-clob.polymarket.com/ws/market` using only
validated token IDs. Do not send authentication or enable custom features.

### PMC-005 Heartbeat and reconnect

Send text `PING` every 10 seconds. A disconnect ends the synchronization epoch.
Reconnect requires rediscovery and a new subscription; no cached book is treated
as current across epochs.

### PMC-006 Event validation

Accept documented `book`, `price_change`, `tick_size_change`,
`last_trade_price`, and `best_bid_ask` events. Reject unknown types, invalid
condition/token IDs, timestamps, oversized messages, and events outside the
subscription.

### PMC-007 Journal integration

Each accepted event is journaled separately with source event time, local receive
time, local source sequence, condition ID, typed public-event prefix, subscribed
asset IDs, and canonical JSON bytes.

### PMC-008 Read-only boundary

No authenticated channel, API key, wallet, order, cancellation, or signer code
may exist in this milestone.

## Acceptance criteria

- Discovery fixtures accept one valid BTC and ETH market and reject mismatched
  series, outcomes, identifiers, times, and token counts.
- Keyset pagination detects repeated cursors and respects configured limits.
- WebSocket subscription contains no auth and is deterministic.
- Object and array WebSocket messages normalize into individual events.
- Events for unsubscribed assets or conditions are rejected.
- Heartbeats and reconnect epochs are explicit.
- Captured events round-trip through the existing journal.
- Full formatting, Clippy, Rust tests, and TLC checks pass.

