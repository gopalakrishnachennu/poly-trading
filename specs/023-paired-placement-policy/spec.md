# Phase 2.10 Specification: Offline Paired Placement Policy

## Objective

Consume an exact Phase 2.9 fully reserved stage and issue short-lived, inert
paper placement permissions under deterministic exchange-mode, sequencing,
expiry, and ambiguous-state controls.

## Requirements

1. One runtime owns the Phase 2.9 staging runtime and all paired policy state.
2. A permission binds the complete stage record, exact candidate, reservation,
   leg role, current normal exchange-mode observation, and validity interval.
3. The first leg alone may be permitted initially. The complementary hedge leg
   may be permitted only after an authoritative paper lifecycle observation
   records the first leg fully matched.
4. Permissions are one-use and valid for at most one second within both the
   stage freshness window and the original candidate expiry. Expiry never
   releases capital automatically.
5. Submitted, delayed, live, partially matched, unknown, fully matched, and
   hedge-active states retain both Phase 2.9 reservations.
6. Paired abort is available only when neither leg has possible fill exposure.
   The runtime invokes the owned Phase 2.9 paired abort; no one-leg release exists.
7. Source sequence and lifecycle transitions are monotonic. Equivocation,
   regression, subject substitution, child failure, or arithmetic/durable
   failure halts the complete owner.
8. Commands and decisions are bounded, canonical, digest-stable,
   content-idempotent, journal-first, replayable, and checkpoint-verifiable.
9. Permissions are paper audit facts only. This phase has no signer, secret,
   authenticated client, network, split/merge, wallet, or submission capability.

## Acceptance criteria

- Tests cover first-leg permission, hedge sequencing denial and success, stale
  mode, stage expiry, permission reuse, lifecycle ambiguity, partial fill,
  no-fill safe abort, abort denial while exposed, reservation retention,
  provenance substitution, replay/checkpoints, sync failure, and properties.
- A bounded TLA+ model proves hedge ordering, no unsafe abort, reservation
  retention under possible exposure, no signing/submission authority, child-halt
  propagation, and absorbing halt.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Actual execution, signatures, credentials, authenticated transport, exchange
clients, automatic retry, fills with financial posting, split/merge, settlement,
wallet/RPC activity, and live order submission.
