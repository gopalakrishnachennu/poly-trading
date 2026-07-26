# ADR 0086: Path challengers optimize net value, not win probability

- Status: Accepted
- Date: 2026-07-26

## Decision

The next intrahour challenger predicts exact path-dependent net P&L in integer
micros. Model selection uses realized executable development-fold P&L after
costs. Features without equivalent historical and local coverage, currently
full-depth and Polymarket trade flow, are excluded.

## Consequences

The model cannot prefer many small wins that are outweighed by fewer large
losses merely because classification accuracy is high. Passing is still only
eligibility for paper evaluation; it does not activate a strategy or authorize
orders.
