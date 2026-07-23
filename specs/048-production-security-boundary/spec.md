# Phase 3.1 Specification: Production Security Boundary

## Objective

Certify the deterministic production security boundary over one exact locally
certified Phase 3.0 report: workload identity, isolated signer policy,
Vault/KMS/HSM provider contracts, least privilege, bounded request rates,
two-person activation evidence, revocation, compromise containment and disaster
recovery—using fake providers and no secret or signature material.

## Scope

- Exact current Phase 3.0 report and durable-data subject binding.
- One workload identity contract with namespace, service account, audience,
  attestation, lifetime and rotation ceilings.
- Credentialless Vault, KMS and HSM provider boundary contracts.
- One isolated signer contract with exact purposes, resource allowlist,
  fixed-point notional ceiling, rate ceiling and arbitrary-payload denial.
- Opaque identity epochs with predecessor-bound rotation and irreversible
  revocation.
- Distinct security and operations operator evidence for dual control.
- Provider outage, signer denial, rate-limit, replay and compromise fixtures.
- Revoke-before-recovery compromise handling and inactive disaster recovery.
- Journal replay, checkpoints and create-new certification reports.

## Exclusions

- Tokens, passwords, private keys, certificates, signature bytes or seed values
- Real Vault, KMS, HSM, IAM or workload-identity provider clients
- DNS, TLS, HTTP, RPC, wallet, relayer or exchange connections
- Arbitrary payload signing, transfers, withdrawals or contract upgrades
- Strategy signing, deployment, capital, trading or order-submission authority

## Acceptance criteria

- Stale, substituted, incomplete, externally certified or authority-bearing
  Phase 3.0 evidence fails registration.
- Exactly one valid workload identity, one signer policy and one contract for
  each fake provider class are bound to the plan.
- Identity epochs are contiguous and predecessor-bound. Revocation is
  irreversible and invalidates every active claim.
- Signer policy uses exact allowed purposes/resources, conservative integer
  notional/rate ceilings, short lifetimes and denies arbitrary payloads,
  transfer, withdrawal, upgrade and strategy-direct access.
- Dual-control evidence requires two distinct opaque operators and expires
  without activating or authorizing the signer.
- Outage and rate-limit fixtures back off without automatic retry. Denial and
  replay fixtures cannot create a signature or external mutation.
- Compromise revokes before exact no-mutation recovery. Disaster recovery binds
  a distinct region and remains inactive.
- Completion requires all scenarios and all three provider classes, no active
  identity and no recovery debt. Reports grant no credential, signature,
  provider, deployment, trading, capital or submission authority.
- Tests and TLA+ cover identity epochs, least privilege, dual control, replay,
  rate limiting, compromise, recovery, no-secret/no-authority and halt absorption.
