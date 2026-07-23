# ADR 0011: Unify Hourly State Through Deterministic Read-Only Sessions

## Status

Accepted

## Context

Validated identity, reconstructed books, Binance reference state, resolution
contracts, and cross-feed health were independently correct but did not yet
form one auditable hourly lifecycle. Treating any component's readiness as a
trading-session decision would permit stale provenance, token mismatch, oracle
window mismatch, or unsafe rollover.

## Decision

Add a deterministic `market-session` coordinator. It registers immutable
non-overlapping BTC/ETH slots, captures exact Up/Down book and oracle state,
verifies the supervisor was computed from the same source snapshots, and owns
only lifecycle readiness and evidence attachment. Exact window boundaries drive
current/next selection. Ended sessions remain visible while awaiting final
evidence and while the next hour is active. Integrity conflicts halt
transactionally and halt is absorbing.

Coordination frames contain immutable decision-relevant state and can be
replayed through the same core. They do not authorize a strategy or order.

## Consequences

- A session is never ready from title/time alone.
- Supervisor readiness cannot be reused with different source snapshots.
- Rollover does not discard unresolved prior evidence.
- The system remains read-only; risk, execution, wallets, and signing stay out
  of scope.
