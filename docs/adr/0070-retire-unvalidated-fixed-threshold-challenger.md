# ADR 0070: Retire automatic fixed-threshold intraday paper entries

- Status: Accepted
- Date: 2026-07-25

## Decision

The managed stack does not automatically start the fixed `ask <= 0.25`,
`bid >= 0.40` intraday challenger. Its accumulated reports are historical
research evidence only. A deliberate deployment configuration may enable it
for a separately governed experiment only with both its explicit enable flag
and a separate exact acknowledgement. Its status remains unvalidated until it
passes the applicable unseen-session, net-P&L gate.

The managed Profit Engine uses the BTC one-hour research window by default.
Its walk-forward output is still research evidence; a non-baseline prediction
metric alone cannot authorize a paper strategy.

## Consequences

The stack cannot silently keep spending simulated challenger capital after the
challenger shows negative out-of-sample P&L, including from a stale worker that
was started before this decision. Historical evidence remains available for
analysis and is labelled historical in the terminal. This decision grants no
model, paper strategy, wallet, signing, transport, or live-trading authority.
