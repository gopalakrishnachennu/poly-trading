# ADR 0022: Strategies propose but never authorize

## Status

Accepted for Phase 2.6.

## Decision

The `strategy-proposal` crate is a proposal-only boundary. It captures an
immutable context from the exact coordination frame applied by `market-session`
and may convert a bounded fixed-point intent into an inert Phase 2.2
`OrderExposure`. The strategy supplies a proposal identity, but the boundary
derives the risk-order identity from the complete context and intent.

Candidate creation requires the current `ACTIVE_READY` hourly session, ready
cross-feed supervision, and authoritative Up and Down books. Degraded, expired,
unknown-token, or invalid economic input receives an attributable rejection.
Capture time must equal the applied frame time, and context validity is capped
at one second and cannot cross the session end.
Applied-frame substitution, digest equivocation, history regression, command
conflict, arithmetic failure, or durable corruption is an absorbing halt.

Each proposal identity is one-use. Commands use bounded canonical encoding,
journal-before-mutation and device sync, strict segmented replay, stable state
digests, and checksummed prefix checkpoints.

## Consequences

A future strategy can express an opportunity without gaining authority over
money or execution. The output must still pass Phase 2.2 portfolio risk, capital
reservation, Phase 2.3 intent policy, and a separately authorized execution
boundary. This phase adds no strategy alpha, credential, signer, authenticated
transport, wallet action, automatic retry, or live order submission.
