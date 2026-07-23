# ADR 0036: Require Regional Drills and Irreversible Release Revocation

## Status

Accepted for Phase 2.20.

## Decision

Introduce `fleet-rollout-governance` as a bounded offline single writer above
Phase 2.19 reports. One sealed campaign binds exact release, artifact and
rollback digests, planned regions, fresh canonical evidence, adverse drill
thresholds, required rollback triggers and a fixed change-freeze interval.

Each planned region must have independent completion evidence. Abort drills,
rollback drills and rollback-trigger diversity are separate requirements.
Duplicate report, plan or evidence identities do not increase coverage.

A release-revocation command binds the exact subject and accountable operator.
Revocation is irreversible and prevents readiness even if all other evidence
passes. It remains valid after positive finalization, immediately invalidates
the current dossier and requires a superseding non-ready dossier. Final dossiers
are evidence only and grant no external authority.

## Consequences

Multi-region readiness and revocation behavior become reproducible before an
external fleet controller exists. This phase adds no deployment, rollback,
authentication, signing, wallet, RPC, network-control or live-trading path.
