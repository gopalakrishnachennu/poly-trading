# ADR 0071: Treat BTC and ETH hourly terminal data as one atomic frame

- Status: Accepted
- Date: 2026-07-25

## Decision

The terminal merge endpoint accepts a gateway hourly frame only if it contains
one valid BTC market and one valid ETH market.  Its rollover cache stores and
replays that pair as one object; it never combines a new asset with the other
asset from an earlier frame.

## Consequences

On a partial gateway response the terminal either presents the last complete
pair as explicitly stale context or no hourly markets at all.  This may show
less data during an outage, but prevents a misleading mixed-hour, mixed-price
display.  It creates no trading or paper-execution authority.
