# ADR 0047: Certify Submission-Gateway Behavior Without Authentication

- Status: Accepted
- Date: 2026-07-21

## Decision

Phase 2.31 binds digest-only Phase 2.30 receipts to exact Phase 2.29 endpoint and
canonical-request evidence, then exercises the resulting envelopes exclusively
through recorded fixtures and a deterministic shadow submission state machine.
Opaque channel and token digests represent binding subjects, not token values.

Staging consumes an envelope receipt and idempotency identity exactly once but
creates only an inert simulated submission. A recorded unknown outcome remains
active and blocks progress until exact no-mutation reconciliation. There is no
automatic retry transition.

## Consequences

- Cross-layer request substitution and receipt replay are detectable offline.
- Unknown-response recovery can be proved before a transport client exists.
- No API secret, signature, authorization header or socket enters this phase.
- Certification evidence grants no authentication or submission authority.
