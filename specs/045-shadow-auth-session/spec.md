# Phase 2.32 Specification: Offline Shadow Authenticated Sessions

## Objective

Coordinate exclusive, expiring shadow authenticated sessions over one exact
current Phase 2.31 certification while proving attestation rotation, heartbeat,
dead-man, restart revocation and ambiguity recovery without credential material,
signatures, sockets or external submission.

## Scope

- Exact current `SHADOW_CERTIFIED` Phase 2.31 plan/report binding.
- Recorded-only predecessor-bound attestation epochs.
- One exclusive opaque-owner lease with bounded lifetime.
- Monotonic healthy heartbeat evidence within lease and policy bounds.
- Dead-man expiry and unhealthy heartbeat revocation.
- Restart revocation with exact pre-restart recovery subjects.
- Ambiguity revocation and digest-bound no-mutation recovery.
- Explicit recovery that never automatically reopens a lease.
- Mandatory clean-close, rotation, dead-man, restart and ambiguity scenarios.
- Journal replay, checkpoints and create-new completion reports.

## Exclusions

- Credential, token, certificate-private-key or authorization-header values
- Cryptographic signatures, KMS/HSM/Vault or identity-provider clients
- DNS, TLS, HTTP, WebSocket, RPC, wallet, relayer or exchange clients
- Automatic reconnect, retry, lease reopening or external mutation
- Capital, reservation, trading, deployment or promotion authority

## Acceptance criteria

- Stale, substituted, non-certified or authority-bearing Phase 2.31 evidence
  fails before registration.
- Attestations are nonzero, recorded-only, time-bounded, predecessor-bound and
  retain the exact gateway/channel/token subject without secret material.
- At most one lease exists; it binds the exact plan, attestation and opaque owner
  and cannot exceed policy, plan or attestation expiry.
- Heartbeats are unique, monotonic and exact-lease-bound. Unhealthy or expired
  state revokes the lease and requires recovery.
- Restart and ambiguity revoke the active lease. Recovery binds the exact reason
  and subject, proves recorded no mutation and returns idle without reopening.
- Finalization requires all mandatory scenarios, no lease and no recovery debt.
- Commands are bounded, versioned, content-idempotent, journal-first and strictly
  recoverable. Reports and checkpoints are create-new and checksummed.
- Tests and TLA+ cover exclusivity, rotation, dead-man, restart, ambiguity,
  recovery ordering, no-secret/no-authority and absorbing halt.
