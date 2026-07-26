# ADR 0068: Build BTC hourly labels from executable L2 depth, not price touches

- Status: Accepted
- Date: 2026-07-25

## Decision

The BTC one-hour intraday research builder derives a hypothetical taker round
trip from captured L2. Entry consumes ask depth and a later exit consumes bid
depth for the exact configured quantity. It preserves event, receive, and
strategy-available time and evaluates only later available observations.

The builder does not claim passive fills as facts: public L2 cannot determine a
hypothetical queue position. It writes NO_MODEL until the configured number of
disjoint completed sessions is available.

## Consequences

Research can measure executable intrahour paths without top-of-book-touch
fills, hidden look-ahead, row-level split leakage, or in-sample promotion. Its
output cannot alter paper or live execution.
