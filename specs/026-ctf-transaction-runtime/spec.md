# Phase 2.13 Specification: Offline CTF Transaction Simulation

## Objective

Own Phase 2.12 and deterministically simulate conditional-token split, merge,
and redemption transactions. Reserve or lock every input before submission,
keep pending and retrying value inaccessible, and recognize accounting effects
only after an authoritative simulated confirmation.

## Requirements

1. The runtime owns one Phase 2.12 paired settlement runtime and all conversion
   transaction records.
2. Split requests reserve exact confirmed collateral. Redemption requests
   reserve exact confirmed outcome tokens. Merge requests lock an exact
   confirmed complementary pair or bind an existing active pair lock.
3. Requests require current Phase 2.12 reconciliation and immutable bounded
   token, condition, quantity, payout, lock, and resolution identities.
4. Lifecycle states are `REQUESTED`, `PENDING`, `RETRYING`, `CONFIRMED`, and
   `FAILED`. Terminal facts are immutable.
5. Submission identity, source sequence, event time, receive time, and
   confirmation hash are monotonic and non-substitutable.
6. Duplicate submission reports are explicit no-ops and cannot create another
   accounting posting. An external transaction identity cannot cross requests.
7. Pending and retrying transactions retain their reservation or pair lock.
   The runtime performs no automatic retry.
8. Confirmation derives exactly one ledger command from the stored request.
   Split creates equal Up/Down quantity with deterministic combined cost;
   merge converts one active pair lock into collateral; redemption consumes
   reserved tokens for the bounded configured payout.
9. Failed split/redemption releases its reservation transactionally. Failed
   merge retains the pair lock for an explicit later recovery request.
10. Commands are fixed-point, bounded, canonical, content-idempotent,
    journal-first, replayable, checkpoint-verifiable, and fail closed.
11. The phase has no credentials, signatures, authenticated transport, RPC,
    wallet, relayer, chain submission, automatic retry, or live order.

## Acceptance criteria

- Tests cover split, merge, redemption, pending inaccessibility, retry paths,
  failure release, failed-merge lock retention, duplicate submission, identity
  conflicts, terminal mutation, exact confirmed accounting, replay/checkpoint,
  sync failure, and property invariants.
- A bounded TLA+ model proves backing-before-pending, pending retention,
  confirmed-only accounting, at-most-once posting, terminal immutability,
  failure policy, no live authority, and absorbing halt.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Real contract calls, RPC, gas/relayers, allowance changes, wallets, credentials,
signing, authenticated APIs, automatic retry, and live trading.
