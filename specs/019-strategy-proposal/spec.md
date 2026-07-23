# Phase 2.6 Specification: Deterministic Strategy-Proposal Boundary

## Objective

Create a proposal-only authority that converts a strategy's bounded intent into
an immutable portfolio-risk candidate bound to exact current hourly-session
state, without possessing capital, risk-approval, signing, or execution power.

## Requirements

1. A proposal context must be captured from the exact coordination frame last
   accepted by the market-session coordinator.
2. Context provenance binds coordinator, applied-frame, session, market,
   reference, and supervision digests plus the current session identity and
   exact Up/Down token books.
3. A context may represent degraded state, but only a current `ACTIVE_READY`
   session with authoritative two-sided books can produce a candidate.
4. Context capture time must equal the applied frame time, validity is capped at
   one second and at session end, and proposal intents use fixed-point integer
   quantity, price, partial-fill, and maximum-fee fields with explicit bounds.
5. The authority derives the risk order ID from proposal identity, context, and
   exact intent; callers cannot choose or substitute it.
6. Accepted output is only an inert Phase 2.2 `OrderExposure`. It is not a risk
   approval, capital reservation, policy permit, signature, or order.
7. Expired/stale/degraded contexts, unknown tokens, invalid economics, duplicate
   proposal identity, or capacity limits produce attributable rejection.
8. Context checksum failure, applied-frame substitution, history regression,
   digest equivocation, command conflict, overflow, or durable corruption halt.
9. Commands are versioned, bounded, content-idempotent, journaled and synced
   before mutation, strictly replayable, checkpoint-verifiable, and digest-stable.
10. No arbitrage calculation, market making, prediction, credentials, wallet,
    authenticated client, automatic retry, signing, or submission is added.

## Acceptance criteria

- Tests cover exact frame provenance, current/ready/book gates, expiry boundary,
  token binding, economic bounds, derived order identity, idempotency conflict,
  history equivocation, journal sync failure, replay/checkpoint equality, and
  stricter-intent property behavior.
- A bounded TLA+ model proves proposal cannot precede ready context, cannot
  authorize risk/reservation/execution, cannot be replayed, and halt is absorbing.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Strategy alpha, complete-set calculations, probability models, capital access,
risk approval, credentials, signing, authenticated transport, and live orders.
