# ADR 0046: Simulate Signing Without Key Material

- Status: Accepted
- Date: 2026-07-21

## Decision

Phase 2.30 models a signing-policy boundary using opaque accountability handles
only. Handles contain no key bytes and cannot be exported or used to contact a
provider. Every request binds a purpose, exact subject, integer units, payload,
time and globally unique nonce. Distinct security and operations approvals are
required before a short-lived one-use permit can exist.

The state machine—not a caller—produces a digest-only simulation receipt after
consuming an exact permit. It performs no cryptographic operation and emits no
signature bytes. A fixed recorded matrix covers policy denials and provider
failure. Revocation is irreversible.

## Consequences

- Signing policy and recovery can be proved without secrets or KMS access.
- A simulation receipt is never an authentication artifact.
- Real key custody and signature production remain separately unauthorized.
- Nonce replay, stale approval, over-limit request and revoked handles fail closed.
