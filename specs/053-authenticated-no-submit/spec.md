# Phase 3.6 Specification: Authenticated No-Submit Certification

## Objective

Certify the isolated authenticated-observation lifecycle while proving that
order, cancellation, wallet mutation and transaction-submission capabilities
remain physically absent and logically denied.

## Scope

- Exact current Phase 3.5 local report binding.
- Opaque workload/provider identity subjects with no credential value.
- Observation-only endpoint and event allowlist contracts.
- Predecessor-bound identity/session rotation and irreversible revocation.
- Recorded activation, observation, outage, dead-man, unknown-state,
  reconciliation and distinct-region disaster-recovery fixtures.
- Independent physical-absence and logical-denial submit gates.
- Journal replay, checkpoints and create-new reports.

## Exclusions

- Credential values, private keys, signatures or bearer tokens
- Real authenticated transport or provider contact
- Order/cancel endpoints, wallet/RPC mutation or live submission
- Deployment, capital or trading authority

## Acceptance criteria

- Registration rejects stale, substituted or authority-bearing Phase 3.5
  evidence and any contract exposing submission or secret material.
- Opaque identity epochs are contiguous and predecessor-bound; revocation is
  irreversible and clears session readiness before recovery.
- Fixture sequence is contiguous and covers activation, observation, rotation,
  revocation, outage, dead-man, unknown reconciliation, disaster recovery and
  both no-submit proofs.
- Unknown and outage cases require explicit no-mutation reconciliation and no
  automatic retry.
- Disaster recovery uses a distinct provider/region subject and returns idle.
- Physical submit capability remains absent while logical policy independently
  denies submit, cancel, transfer, withdrawal and upgrade purposes.
- Final report is locally certified only and grants no credential, signature,
  connection, deployment, capital, trading or submission authority.
- Tests and TLA+ cover lifecycle, revocation, recovery, no-submit independence,
  no mutation and absorbing halt.
