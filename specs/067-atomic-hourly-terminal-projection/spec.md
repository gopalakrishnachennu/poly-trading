# Phase 5.1 Specification: Atomic Hourly Terminal Projection

## Objective

Keep the terminal's BTC and ETH hourly display coherent through transient
gateway and rollover failures.  The display must never assemble a current
hourly view from a fresh asset and a stale or malformed counterpart.

## Scope

- Validate that a gateway hourly response contains exactly one structurally
  valid BTC and one structurally valid ETH market before exposing it to the
  terminal's hourly merge.
- Cache and serve the last good hourly frame only as one bounded BTC+ETH pair.
- Mark a carried-over pair as stale and expire it before either market's own
  resolution time or the bounded cache lifetime.

## Exclusions

- Changing the Rust projection authority, relaxing its validation, inventing
  market data, orders, credentials, wallet operations, paper fills, or live
  trading.

## Acceptance criteria

- A ready gateway response that is missing, duplicated, malformed or one-sided
  for either required asset does not produce a partial hourly display.
- A cached hourly display contains BTC and ETH from the same prior gateway
  frame, is visibly stale, and expires conservatively.
- A complete fresh frame replaces the cached pair atomically.
- Dashboard tests and TypeScript build continue to pass.
