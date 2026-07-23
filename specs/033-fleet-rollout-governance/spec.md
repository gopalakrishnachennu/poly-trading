# Phase 2.20 Specification: Offline Fleet Rollout and Release Revocation

## Objective

Aggregate independent region-bound Phase 2.19 reports, prove successful rollout
and adverse abort/rollback drills, enforce an immutable release change-freeze,
support irreversible release revocation, and emit a checksummed non-deploying
operational-readiness dossier.

## Scope

- One single writer owns one immutable fleet campaign.
- The campaign binds one release/artifact/rollback subject, a sealed change
  freeze, canonical regional evidence, required rollback-trigger diversity,
  limits, expiry, and manifest digest.
- Duplicate reports, evidence identities, plans, or regions cannot inflate
  completion or drill coverage.
- Every required region needs an independent simulated-completion report.
  Global abort and rollback drill floors and every required rollback trigger
  must be covered by authentic Phase 2.19 terminal outcomes.
- Reports must be fresh, digest-valid, non-authorizing and bound to the exact
  release artifacts and rollback policy.
- Revocation binds the exact release/artifact subject, a nonzero accountable
  operator and reason, and is irreversible.
- Revocation remains available after a positive dossier, immediately removes
  that dossier from current state, and requires a superseding non-ready dossier.
- Finalization emits attributable readiness reasons. A positive dossier still
  grants no deployment, rollback execution, credentials, transport or trading.
- Commands and state are journaled and checkpointed; dossiers are canonical,
  create-new and checksummed.

## Exclusions

- Deployment, routing, capital allocation, rollback execution
- Credentials, private keys, signatures, authentication, RPC or wallet access
- Artifact publication, cloud control planes, order or transaction submission

## Acceptance criteria

- Invalid report, manifest, freeze, release, artifact, rollback or identity
  binding fails closed before installation.
- Canonical deduplication prevents any report or plan from counting twice.
- Every required region has completion evidence; configured abort/rollback drill
  counts and trigger diversity are independently enforced.
- Stale evidence, freeze expiry and revocation are explicit readiness denials.
- Revocation is idempotent for exact content, conflicting reuse halts, and no
  subsequent command can restore readiness.
- Dossiers bind aggregate counts, region/trigger coverage, freeze and revocation
  state, and always set every external authority flag false.
- Replay/checkpoint recovery reproduces complete state; dossier corruption,
  replacement and noncanonical encoding are rejected.
- Tests cover success, missing region, duplicate inflation, stale evidence,
  missing drills/triggers, freeze boundaries, revocation, substitution,
  durability, corruption and deterministic replay.
- A bounded TLA+ model proves region/drill/freeze/revocation gating,
  irreversible revocation, no external authority and absorbing halt.
