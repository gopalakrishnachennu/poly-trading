# ADR 0037: Require Current Fleet Binding and Three-Role Preflight

## Status

Accepted for Phase 2.21.

## Decision

Introduce a bounded offline `deployment-preflight` single writer. It consumes a
fresh current-readiness binding derived by Phase 2.20 only while an operational
dossier remains current and unrevoked. Registration seals exact regional
configuration, credentialless least-privilege limits, rollback artifacts and the
unchanged release subject.

Final readiness requires fresh approvals from release, risk and operations
roles using three distinct opaque operator identities. Finalization must supply
a renewed Phase 2.20 binding, preventing a revoked or superseded dossier from
remaining current merely because an older file still exists.

The output is evidence only. Operator labels are not authentication, signatures
or credentials, and no deployment or rollback action is represented.

## Consequences

Package substitution, privilege escalation, missing regional configuration and
stale fleet readiness become deterministic preflight failures. This phase adds
no cloud control plane, credential, signer, authenticated transport, RPC,
wallet, deployment, rollback execution or live-trading path.
