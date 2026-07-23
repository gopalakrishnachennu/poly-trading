# Phase 3.8 Specification: Controlled-Production Release

## Objective

Implement deterministic, revocable production-release code eligibility with
staged fixed-point capital ceilings, independent regional health, continuous
reconciliation, expiring evidence and rehearsed incident, disaster-recovery and
rollback controls, without granting live production authority.

## Scope

- Exact current Phase 3.7 code-eligibility report binding.
- Immutable release, artifact, configuration, infrastructure, reconciliation,
  incident, disaster-recovery and rollback subjects.
- Strictly increasing fixed-point capital, exposure and loss stages.
- At least two independently identified regions with current health evidence.
- Three-person release, risk and operations control.
- Safe `NO_TRADE`, reconciliation, expiry, incident, DR, rollback and revocation.
- Journal replay, checkpoints and create-new checksummed reports.

## Exclusions

- Real capital, credentials, signing, transport, deployment or order submission
- Legal or geographic eligibility claims
- Target-environment or production completion without external evidence
- Guaranteed trades, returns, principal preservation or availability

## Acceptance criteria

- Registration rejects stale or authority-bearing Phase 3.7 evidence.
- Capital stages are contiguous, positive and strictly increasing; exposure and
  loss ceilings cannot exceed their enclosing stage limits.
- Required regions are unique, nonzero, independently current, reconciled,
  capital-floor-safe and free of unknown state.
- Three distinct opaque operators approve the exact sealed subject.
- Stale evidence cannot authorize progress and finalization rechecks freshness.
- `NO_TRADE` remains valid in every stage and requires no external mutation.
- Incident response, disaster recovery, rollback and revocation have explicit,
  deterministic evidence; revocation is irreversible.
- Final reports are code eligible only and every environment, capital,
  credential, signing, deployment, trading and submission authority is false.
- Failure/property tests and TLA+ cover stage monotonicity, regional health,
  expiry, rollback, revocation, no authority and absorbing halt.
