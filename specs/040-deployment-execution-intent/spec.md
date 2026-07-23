# Phase 2.27 Specification: Offline Deployment Execution-Intent Policy

## Objective

Consume one current sealed Phase 2.26 readiness record and its exact subject,
certify a credentialless isolated-executor contract against a mandatory dry-run
matrix, then derive short-lived, one-use, next-step manual handoff intents whose
operation, resource and region cannot exceed an immutable privilege ceiling.

## Scope

- Exact Phase 2.26 record, candidate and subject binding with freshness checks.
- Canonical region, operation and resource privilege ceilings without wildcards.
- Contiguous ordered steps; only the next step can receive an intent.
- Fixed dry-run cases for substitution, privilege, credential, signing,
  authenticated-transport, expiry and replay behavior.
- One-use intent issue/consume lifecycle with expiry and restart recovery.
- Journal-first transitions, checksummed checkpoints and create-new reports.
- Every output remains a simulated manual handoff and grants zero authority.

## Exclusions

- Credentials, private keys, signatures, KMS/Vault or identity-provider access
- Authenticated transport, cloud/Kubernetes clients or control-plane submission
- Deployment, traffic mutation, rollback execution, wallet or trading activity
- Automatic execution or promotion from any record emitted here

## Acceptance criteria

- Invalid, stale, non-ready, substituted or authority-bearing readiness input
  halts before registration.
- Privilege ceilings are nonempty canonical exact sets and explicitly prohibit
  wildcard, secret, admin, shell, escalation, cross-region and credential access.
- Steps are contiguous and each exact operation/resource/region is permitted.
- Certification requires every mandatory dry-run in exact order and matching
  expected/observed disposition; any external side effect halts.
- An intent is issued only after certification, for the next step, with a bounded
  lifetime no later than its plan and readiness expiry.
- Every intent is consumed at most once, in order, by a nonzero opaque manual
  handoff label; expired, replayed or substituted intents halt.
- Commands are bounded, versioned, content-idempotent, journal-first and strictly
  recoverable; corruption, gaps and post-halt events fail closed.
- Final reports are checksummed, create-new and explicitly non-executing.
- Tests and a bounded TLA+ model cover gates, expiry, replay, order, no-authority,
  recovery and absorbing halt invariants.
