# ADR 0049: Certify the Credential-Provider Protocol Without Provider Access

- Status: Accepted
- Date: 2026-07-21

## Decision

Phase 2.33 certifies only a deterministic recorded protocol. Opaque handle
digests, attestations and fixture outcomes represent provider interactions but
cannot contain key material, provider credentials, signatures, authenticated
transport or external side effects.

Handle epochs are contiguous and predecessor-bound. Revocation is irreversible.
Provider outage and quota exhaustion never retry automatically. Split-brain
evidence revokes the current claim before recovery. Disaster recovery proves
state continuity into a configured distinct region but returns inactive; later
activation remains a separate authority.

## Consequences

- Provider semantics and failure policy can be reviewed and replayed offline.
- No repository artifact can operate a real credential provider.
- Recovery cannot silently reactivate a stale or ambiguous handle.
- Phase 3.1 must certify real provider adapters without weakening this protocol.
