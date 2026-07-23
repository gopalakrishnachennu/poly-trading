# ADR 0043: Certify Credentialless Isolated-Executor Intents

- Status: Accepted
- Date: 2026-07-21

## Decision

Phase 2.27 introduces an offline single-writer authority that consumes one exact
current Phase 2.26 readiness record. It certifies an immutable executor contract
against a mandatory dry-run matrix, then emits only short-lived, one-use,
next-step manual handoff intents bounded by exact operation, resource digest and
region sets.

The executor contract must expressly prohibit credential loading, signatures,
authenticated transport, external submission, wildcard resources, secret read,
cluster administration, arbitrary execution, privilege escalation and
cross-region mutation. Dry-run observations declaring any side effect are an
absorbing integrity failure.

## Consequences

- A readiness record cannot itself become an executable command.
- Certification demonstrates interface behavior only; it does not establish
  identity, credentials or deployment authority.
- Intent replay, expiry, substitution and out-of-order use fail closed.
- A later phase must create a separately reviewed boundary before any external
  executor integration can exist.
