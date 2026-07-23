# ADR 0039: Certify Recorded Deployment Adapters Without Control-Plane Authority

## Status

Accepted for Phase 2.23.

## Decision

Introduce an offline `deployment-adapter-certification` owner above exact Phase
2.22 completion and rollback reports. Certification requires a complete,
contiguous, digest-bound fixture matrix in every region, explicit least-
privilege denial coverage, and bounded disaster-recovery evidence covering
regional, control-plane, durable-state and artifact failures.

Recorded fixture outcomes may observe, deny, require manual execution, back off
or require reconciliation. They may not claim that a mutation, traffic shift,
failover or rollback occurred. A certified report remains evidence only and
grants no credential or external authority.

## Consequences

Adapter behavior, privilege boundaries and regional recovery become replayable
and independently auditable before any authenticated control-plane integration.
Phase 2.23 adds no network client, credentials, cloud SDK, Kubernetes client,
deployment, routing, failover, rollback execution, wallet, RPC or live-trading
path.
