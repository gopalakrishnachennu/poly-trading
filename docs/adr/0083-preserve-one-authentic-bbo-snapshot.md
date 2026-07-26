# ADR 0083: Preserve one authentic BBO snapshot per time bucket

- Status: Accepted
- Date: 2026-07-26

## Decision

New BTC-hourly historical tapes select the latest usable source row per
market/token/second. Exact-timestamp ambiguity is resolved by selecting one
whole deterministic row. Bid, ask and sizes are never assembled from separate
observations. The earlier conservative-extrema tape remains immutable evidence
but is not the preferred path-label source.

## Consequences

Path exits are evaluated against books that actually existed while retaining
the two-second unknown-receive-time reserve. This removes synthetic spread
artifacts without making fills optimistic or inventing queue information.
