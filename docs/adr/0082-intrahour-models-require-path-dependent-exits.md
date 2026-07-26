# ADR 0082: Intrahour models require path-dependent executable exits

- Status: Accepted
- Date: 2026-07-26

## Decision

The momentum thesis is evaluated with the first future executable bid that
crosses a fixed take-profit, stop-loss or maximum-holding boundary. Fixed
horizons remain useful diagnostics but cannot stand in for this exit policy.
Entry books must also pass immutable ask, spread and session-time gates before a
model may score them.

## Consequences

Research now matches the proposed trade lifecycle and preserves gap risk. It may
still reject the strategy; the exit state machine improves validity, not the
odds of obtaining a green result.
