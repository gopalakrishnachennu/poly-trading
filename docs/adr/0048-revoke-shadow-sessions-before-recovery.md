# ADR 0048: Revoke Shadow Sessions Before Recovery

- Status: Accepted
- Date: 2026-07-21

## Decision

Phase 2.32 represents authentication sessions only as offline leases over
recorded attestations. Dead-man expiry, unhealthy heartbeat, process restart and
ambiguous state revoke the active lease before any recovery evidence is accepted.
Recovery binds the exact revocation subject, proves recorded no mutation and
returns the coordinator to idle. It never reopens a lease automatically.

Attestation rotation is allowed only while idle and must name the exact prior
attestation digest with a monotonically increasing epoch. The gateway, channel
and token subject cannot change inside this phase.

## Consequences

- A stale process cannot retain an authentication-session claim.
- Recovery and renewed lease acquisition remain separate transitions.
- Rotation cannot silently substitute a gateway or channel/token subject.
- No credential, signature, provider or transport capability is introduced.
