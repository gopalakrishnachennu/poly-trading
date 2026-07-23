# Phase 2.28 Specification: Offline Executor-Session Protocol

## Objective

Bind one current Phase 2.27 certification report to its exact plan and simulate
a credentialless isolated executor session with exclusive leases, bounded
request envelopes, acknowledgement/unknown outcomes, dead-man expiry, restart
recovery and mandatory reconciliation—without external submission.

## Scope

- Exact Phase 2.27 report, plan, subject, contract and completed-step binding.
- Credentialless process-isolation contract with explicit capability denials.
- Exclusive short-lived lease and monotonic heartbeat sequence.
- Contiguous request templates within the Phase 2.27 privilege ceiling.
- One active request envelope with bounded lifetime and exact provenance.
- Simulated acknowledged, rejected and unknown observations.
- Unknown, restart and dead-man paths require conservative reconciliation.
- Journal-first transitions, checksummed checkpoints and create-new dossiers.

## Exclusions

- Credentials, private keys, signatures, KMS/Vault and identity providers
- Network sockets, authenticated transport, cloud/Kubernetes clients
- External request submission, deployment, rollback or traffic mutation
- Wallet, RPC, exchange access, order submission or live trading

## Acceptance criteria

- Stale, incomplete, substituted, corrupt or authority-bearing Phase 2.27 input
  halts before registration.
- Isolation contracts deny network, credentials, signing, privilege, shell,
  filesystem escape and host namespace access.
- One exclusive unexpired lease is required to open and operate a session.
- Request templates are contiguous and exactly within the Phase 2.27 operation,
  resource and region ceiling; one request may be active at a time.
- Request envelopes are short-lived, one-use and bound to lease, session,
  process, report, plan, contract and template digests.
- Observations cannot claim credentials, signatures, authenticated transport,
  external submission or external mutation.
- Unknown outcomes, dead-man expiry and restart prevent new requests until an
  exact no-mutation reconciliation is recorded.
- Finalization requires every template resolved, no active lease/request and a
  recovered, reconciled terminal session.
- Commands are bounded, versioned, content-idempotent, journal-first and strictly
  recoverable. Dossiers and checkpoints are create-new and checksummed.
- Tests and a bounded TLA+ model cover leases, ordering, unknowns, dead-man,
  restart, reconciliation, recovery, no-authority and absorbing halt.
