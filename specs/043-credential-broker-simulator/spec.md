# Phase 2.30 Specification: Offline Credential-Broker and Signing Policy

## Objective

Consume one current Phase 2.29 transport certificate and simulate an isolated
credential-broker policy with opaque non-secret key handles, exact purpose/
subject/unit/time ceilings, dual authorization, nonce and permit uniqueness,
revocation and mandatory signer-failure fixtures without private keys or real
signatures.

## Scope

- Exact fresh Phase 2.29 certificate binding.
- Opaque attested key handles with no material, export or provider access.
- Canonical purposes, subjects and checked integer unit ceilings.
- Fixed signer failure/denial fixture matrix.
- Exact-request security and operations dual authorization.
- Short-lived one-use permits and globally unique nonces.
- Simulator-generated digest receipts containing no signature bytes.
- Irreversible handle revocation and fail-closed lifecycle.
- Journal replay, checkpoints and create-new certification reports.

## Exclusions

- Private/public key bytes, seed phrases, certificates or secret material
- KMS, HSM, Vault, cloud identity or credential-provider clients
- Cryptographic signature generation or verification
- Network, authenticated transport, external submission or deployment
- Wallet, exchange, order submission or live trading activity

## Acceptance criteria

- Invalid, stale, substituted or authority-bearing Phase 2.29 evidence fails
  before plan registration.
- Key handle identity and attestation are nonzero, immutable, non-exportable and
  explicitly contain no key material or provider access.
- Request templates are contiguous, unique and within exact purpose, subject,
  unit, aggregate-unit and plan-time ceilings.
- The complete fixed fixture matrix proves valid dry-run behavior and denials
  for wrong purpose/subject, excess units, expiry, nonce replay, revocation,
  provider failure and attestation mismatch.
- Every request requires current affirmative security and operations approvals
  from distinct nonzero opaque operators over the exact unchanged request.
- Permit lifetime is bounded; permit, request and nonce identities are one-use.
- The simulator alone creates receipts, which contain no real signature,
  credential, authentication or submission authority.
- Revocation is irreversible and forbids new permits or receipts.
- Commands are bounded, versioned, content-idempotent, journal-first and strictly
  recoverable. Reports and checkpoints are create-new and checksummed.
- Tests and TLA+ cover fixtures, dual control, bounds, replay, revocation,
  completion, no-key/no-authority and absorbing halt.
