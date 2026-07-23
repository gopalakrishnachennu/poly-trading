# Phase 2.31 Specification: Offline Submission-Gateway Certification

## Objective

Certify a deterministic shadow authenticated-envelope and submission-gateway
contract by binding one complete Phase 2.30 broker report and its exact receipt
chain to the originating Phase 2.29 endpoint and canonical request bindings,
without credentials, signature bytes, sockets or external submission.

## Scope

- Exact fresh Phase 2.29 transport-plan/certificate and Phase 2.30 plan/report binding.
- Canonical shadow envelopes over exact broker requests, receipts and transport bindings.
- Opaque channel and token binding with no credential or header material.
- Bounded envelope lifetime and globally unique idempotency identities.
- Fixed recorded gateway success, denial, backoff, ambiguity and reconciliation matrix.
- One active shadow submission with exactly-once receipt/idempotency consumption.
- Recorded accepted, rejected and unknown outcomes without external mutation claims.
- Mandatory exact no-mutation reconciliation after every unknown outcome.
- Journal replay, checkpoints and create-new certification reports.

## Exclusions

- API keys, secrets, bearer tokens, cookies or authorization-header values
- Private/public keys, cryptographic signatures or credential-provider clients
- DNS, TLS, HTTP, WebSocket, RPC, wallet, relayer or exchange clients
- Automatic retry, real authentication, order submission or external mutation
- Capital, reservation, trading, deployment or promotion authority

## Acceptance criteria

- Invalid, stale, substituted, revoked, incomplete or authority-bearing upstream
  evidence fails before registration.
- Every envelope maps one contiguous Phase 2.30 request and receipt to the same
  contiguous Phase 2.29 request binding and exact endpoint-policy digest.
- The complete receipt chain recomputes to the Phase 2.30 report before use.
- Channel, token, request, receipt and idempotency identities are nonzero,
  immutable, canonical and unique.
- The complete fixed fixture matrix proves safe behavior for valid, substituted,
  replayed, expired, rate-limited, unknown and reconciled cases.
- Staging consumes one exact receipt and idempotency identity once, creates only
  an inert simulated submission, and never exposes authentication material.
- Unknown is nonterminal and prevents another submission until exact recorded
  no-mutation reconciliation succeeds. No automatic retry exists.
- Commands are bounded, versioned, content-idempotent, journal-first and strictly
  recoverable. Reports and checkpoints are create-new and checksummed.
- Tests and TLA+ cover upstream binding, fixtures, exactly-once use, ambiguity,
  reconciliation, expiry, no-secret/no-authority and absorbing halt.
