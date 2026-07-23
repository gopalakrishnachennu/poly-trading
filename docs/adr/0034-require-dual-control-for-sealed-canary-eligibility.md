# ADR 0034: Require Dual Control for Sealed Canary Eligibility

## Status

Accepted for Phase 2.18.

## Decision

Introduce `promotion-governance` as a bounded offline single writer above Phase
2.17 evidence. One immutable candidate binds a canonical set of campaign
bundles, a regression baseline, source/binary/toolchain/dependency-lock/SBOM/
configuration digests, and explicit rollback criteria.

Campaign counts, diversity and regression retention are derived from the
bundles. Repeated identities cannot inflate independent evidence. Two current
approvals are required: one risk role and one release role, held by distinct
opaque operator identities and bound to the exact candidate digest. An opaque
identifier is accountability data, not authentication or a signature.

Finalization produces an expiring canary-eligibility record with independent
reasons. Even a positive record grants no promotion, deployment, signing,
credential, transport, wallet, RPC, order, transaction, or live authority. A
later phase must introduce and separately authorize any deployment boundary.

## Consequences

Release evidence becomes deterministic, reproducible and artifact-specific.
One person, one campaign, duplicate evidence, or a regressed candidate cannot
produce positive eligibility. Governance remains entirely offline and cannot
act on the eligibility it records.
