# ADR 0031: Certify Shadow Adapters Without Live Authority

## Status

Accepted for Phase 2.15.

## Decision

Introduce a separate `shadow-adapter-certification` authority. It consumes only
immutable interface metadata, opaque fixture digests, normalized recorded
responses, policy/intent dry-run facts, synthetic operational observations,
and public eligibility attestations.

Certification requires complete evidence and mandatory adverse scenarios. A
positive report is an audit fact, not a placement permit, signature, credential,
wallet approval, relayer request, or deployment action. Missing or unhealthy
evidence denies certification. Identity/history equivocation halts.

The crate deliberately has no HTTP, WebSocket, RPC, wallet, cryptographic
signer, environment-secret, order-submission, or transaction-submission
dependency.

## Consequences

Adapter assumptions and failure responses become deterministic, replayable,
and independently auditable before any authenticated implementation exists.
Production integration remains unauthorized and technically absent.
