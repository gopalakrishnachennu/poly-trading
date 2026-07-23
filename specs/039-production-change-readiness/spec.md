# Phase 2.26 Specification: Offline Production-Change Readiness Governance

## Objective

Aggregate fresh independent Phase 2.25 campaign evidence, enforce diversity and
conservative regression floors, bind the exact production-change subject, and
require distinct release, risk and operations decisions before emitting a
checksummed non-executable readiness record.

## Scope

- Phase 2.25 case results and evidence expose canonical certificate, preflight,
  plan and rollback-package subjects for downstream exact binding.
- One immutable readiness candidate binds policy, canonical campaign evidence,
  a regression baseline, exact release/binary/configuration/infrastructure/
  observability subjects, Phase 2.25 subject sets, creation time and expiry.
- Evidence validity, authority flags, freshness, independence, diversity and
  aggregate counts are recomputed from the submitted records.
- Duplicate evidence or campaign identities cannot inflate totals. Conflicting
  content under one campaign identity is an integrity failure.
- Regression floors use checked upward-rounded integer basis-point arithmetic.
- Release, risk and operations decisions bind the exact candidate, remain
  current and come from three distinct nonzero opaque operators.
- Final output is attributable when gates fail and always grants zero external
  or live-trading authority.

## Exclusions

- Credentials, keys, signatures, identity-provider integration
- Authenticated transport, cloud/Kubernetes/DNS/load-balancer clients
- Artifact upload, deployment, routing, rollback, wallet or trading activity
- Automatic production promotion from a readiness record

## Acceptance criteria

- Invalid policy, baseline, subject, evidence digest, authority-bearing evidence
  or exact-subject substitution fails closed before installation.
- Only fresh `OPERATOR_REVIEW_ELIGIBLE` evidence contributes to campaign,
  case, plan, restart and approval-set totals.
- Campaign, manifest, schedule, result-chain and plan diversity meet independent
  absolute floors after canonical deduplication.
- Every aggregate count meets both its absolute floor and conservative retained
  fraction of the sealed baseline.
- Positive readiness requires fresh affirmative release/risk/operations
  decisions over the unchanged candidate from distinct operators.
- Final records expose every missing, stale, duplicate, regression, diversity,
  decision and expiry reason and contain no executable authority.
- Commands are content-idempotent, journal-first and checkpoint-replayable;
  corruption, equivocation and post-finalization mutation halt absorbingly.
- Tests cover success, stale/ineligible/duplicate evidence, subject substitution,
  regression/diversity failure, missing/rejected/expired/same-operator decisions,
  evidence corruption, replay, sync failure and deterministic arithmetic.
- A bounded TLA+ model proves evidence, subject, regression, diversity,
  three-role control, no-authority, single-finalization and halt invariants.
