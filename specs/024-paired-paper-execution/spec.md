# Phase 2.11 Specification: Composed Paired Paper Execution

## Objective

Own Phase 2.10 policy and deterministically consume exact paired paper permits
through simulated submission, exchange lifecycle, fills, cancellation,
settlement handoff creation, recovery, and reservation-retention controls.

## Requirements

1. The runtime owns one Phase 2.10 policy runtime and all paired paper orders.
2. Caller policy commands cannot inject lifecycle observations; execution alone
   derives Phase 2.10 leg transitions from accepted paper actions and events.
3. Submission consumes an authentic, current, unused permit matching the exact
   owned stage, leg, candidate, and reservation.
4. Source sequence, event time, receive time, and exchange order identity are
   monotonic and immutable.
5. Partial and full fills enforce quantity, fee, cumulative, price-limit, and
   unique ledger-command bounds and emit one immutable reconciliation handoff.
6. Unknown and cancel-pending states are non-terminal and fillable. Delayed
   orders remain uncancellable until their exact boundary.
7. First-leg full match enables Phase 2.10 hedge authorization; no other state
   can bypass sequencing.
8. Both reservations remain active through submission, delay, live, partial,
   unknown, cancel-pending, full match, and every unposted handoff.
9. Commands are bounded, canonical, content-idempotent, journal-first,
   replayable, checkpoint-verifiable, and fail closed.
10. This phase is simulated and contains no credentials, signer, authenticated
    transport, wallet/RPC, split/merge, or real submission capability.

## Acceptance criteria

- Tests cover exact permit consumption, reuse denial, first/hedge sequencing,
  partial and full fills, cancel races, delayed cancellation, unknown recovery,
  handoff uniqueness, invalid fill rollback, reservation retention, safe no-fill
  abort, replay/checkpoints, sync failure, and property invariants.
- A bounded TLA+ model proves permit single-use, hedge ordering, unknown and
  cancel fillability, handoff/fill equality, reservation retention, no live
  authority, child-halt propagation, and absorbing halt.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Settlement confirmation, ledger fill posting, signatures, credentials,
authenticated adapters, automatic retries, blockchain actions, split/merge,
and live order submission.
