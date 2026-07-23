# Phase 2.33 Specification: Offline Credential-Provider Adapter Certification

## Objective

Certify a deterministic credential-provider protocol over one exact current
Phase 2.32 shadow-session report while proving opaque-handle acquisition,
attestation, rotation, revocation, quota/outage behavior, split-brain denial and
disaster recovery without key material, provider credentials, cryptographic
signing, sockets, external mutation or trading authority.

## Scope

- Exact current successful Phase 2.32 report and plan binding.
- Versioned provider and opaque-handle contracts with fixed identity subjects.
- Digest-only recorded protocol fixtures and conservative dispositions.
- Monotonic handle epochs with exact predecessor binding.
- Irreversible revocation that invalidates active handle claims.
- Quota exhaustion and provider outage backoff without automatic retry.
- Split-brain detection requiring revocation and reconciliation.
- Disaster recovery into a distinct configured region with exact state binding.
- Complete fixed scenario matrix before certification.
- Journal replay, prefix checkpoints and create-new certification reports.

## Exclusions

- Secret, token, private-key, certificate-key or authorization-header values
- Real Vault, KMS, HSM, cloud identity or credential-provider clients
- Cryptographic signing, signature bytes or arbitrary payload signing
- DNS, TLS, HTTP, WebSocket, RPC, wallet, relayer or exchange clients
- Automatic retry, failover, deployment, capital or order authority

## Acceptance criteria

- Stale, substituted, incomplete or authority-bearing Phase 2.32 evidence fails
  before registration.
- Provider contracts bind exact provider, tenant, region, key purpose,
  algorithm, limits, expiry and disaster-recovery region without secret values.
- Handle acquisition and rotation produce only opaque digests. Epochs are
  contiguous and rotation names the exact predecessor.
- Revocation is irreversible. A revoked handle can never become active again.
- Quota, outage and split-brain fixtures can only deny, back off, reconcile or
  require manual recovery; they never authorize a request or automatic retry.
- Disaster recovery requires prior revocation, exact recovered epoch/state and
  a distinct configured destination region. It does not activate a handle.
- Finalization requires every fixed scenario and no active ambiguity or
  recovery debt. The report explicitly grants no signing, provider, deployment,
  trading, capital or submission authority.
- Commands are bounded, versioned, content-idempotent, journal-first and strictly
  recoverable. Reports and checkpoints are create-new and checksummed.
- Tests and TLA+ cover epochs, revocation, quota, outage, split brain, disaster
  recovery, no-secret/no-authority and absorbing halt.
