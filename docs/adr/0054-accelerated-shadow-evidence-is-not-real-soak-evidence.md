# ADR 0054: Accelerated Shadow Evidence Is Not Real Soak Evidence

- Status: Accepted
- Date: 2026-07-21

## Context

Deterministic accelerated campaigns are necessary for repeatable failure and
recovery verification, but accelerated logical time is not evidence that a
target environment remained healthy for multiple real days.

## Decision

Phase 3.4 records accelerated duration and real elapsed duration separately.
Local completion may use an accelerated campaign and must set real multi-day
environment certification false. A later environment gate may consume real
evidence only when it is fresh and bound to the same artifact and configuration.

Every disruption clears readiness before recovery. Resource dimensions are
gated independently. Opaque operator labels provide accountability only; they
are neither credentials nor cryptographic signatures.

## Consequences

Tests can exhaustively cover multi-day logical schedules quickly without
fabricating uptime. Code completion cannot silently become environment
certification, deployment authority or trading authority.
