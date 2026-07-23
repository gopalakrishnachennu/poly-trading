# Phase 4.0 Specification: Read-Only Terminal Projection Gateway

## Objective

Connect the operator terminal to validated current hourly BTC/ETH market,
reference-price and public CLOB state without introducing credentials, financial
authority or fabricated fallback values.

## Scope

- Reuse validated Gamma hourly market identity discovery.
- Fetch public CLOB books for exact complementary token identifiers.
- Fetch Binance market-data-only current prices and exact hourly opening candle.
- Expose versioned read-only HTTP projections with all financial values encoded
  as decimal integer strings.
- Revalidate identity, timestamps, order-book bounds and freshness every cycle.
- Represent discovery, ready, stale and halted state explicitly.
- Drive the terminal from the gateway and retain client-side history only for
  visualization.

## Exclusions

- Credentials, authenticated endpoints, wallet state or order submission
- Live accounting balances, reservations, positions or P&L
- Strategy, risk, execution or settlement authority
- Browser-originated financial calculations
- Fabricated data when any upstream is unavailable

## Acceptance criteria

- Market selection requires exactly one current validated BTC and ETH hourly
  identity; duplicates, gaps and expired identities fail closed.
- Every CLOB response must match its requested token and expected condition.
- Prices and quantities parse through strict fixed-point types; crossed,
  one-sided, empty, excessive or malformed books are rejected.
- Reference price and target/open price match the exact symbol and hour.
- Upstream timestamps, receive timestamps and projection timestamps remain
  distinct and monotonic.
- Partial refresh failure cannot combine new data with an old complementary leg.
- Market rollover clears prior authority before publishing the new identity.
- Stale or unavailable data publishes `NO_TRADE` with an attributable reason.
- The terminal displays unavailable financial-authority fields as unavailable,
  never as zero or simulated capital.
- Tests cover malformed decimals, identity substitution, crossed books,
  staleness, partial refresh, rollover and recovery.
