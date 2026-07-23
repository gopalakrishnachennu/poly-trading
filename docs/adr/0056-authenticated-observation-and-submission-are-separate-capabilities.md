# ADR 0056: Authenticated Observation and Submission Are Separate Capabilities

- Status: Accepted
- Date: 2026-07-22

## Context

An identity accepted for a private observation channel must not imply that the
same process can reach order, cancellation or wallet mutation operations.

## Decision

Phase 3.6 requires two independent controls: submission transport and endpoint
capability are physically absent, and policy independently denies every
mutation purpose. Observation contracts allowlist event classes only.

Opaque identity labels and recorded fixtures contain no credential value.
Rotation, revocation, provider outage, ambiguity and disaster recovery are
certified without opening a real authenticated connection.

## Consequences

Local code can prove lifecycle and authority separation. Real authenticated
observation remains an environment gate requiring externally supplied identity
and target-region evidence.
