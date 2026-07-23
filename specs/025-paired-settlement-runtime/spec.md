# Phase 2.12 Specification: Paired Settlement and Accounting Composition

## Objective

Own Phase 2.11 execution, Phase 2.1 settlement reconciliation, and the nested
Phase 2.0 ledger under one deterministic offline writer. Convert only authentic
execution handoffs into settlement intents, post only confirmed exact fills,
and retain paired capital until terminal execution and current finalized-chain
reconciliation prove release is safe.

## Requirements

1. The runtime owns one Phase 2.11 runtime and one settlement reconciler.
2. Callers cannot register a detached trade intent or provide a detached ledger
   reconciliation view.
3. Every registered intent is copied from one exact, previously unregistered
   handoff stored by the owned execution runtime.
4. `MATCHED`, `MINED`, and `RETRYING` never post accounting. `FAILED` never
   posts successful-fill accounting. Only exact `CONFIRMED` economics post.
5. Confirmed posting uses the handoff's ledger command ID, stage reservation,
   token, side, quantity, consideration, fee, and confirmed transaction hash.
6. Reconciliation frames are built from the nested authoritative ledger and a
   caller-supplied finalized paper-chain snapshot.
7. A stage can release residual reservations only when both paper orders are
   terminal, every handoff is registered, every registered trade is terminal,
   confirmed trades are posted, failed trades are unposted, and reconciliation
   is current for the exact ledger digest.
8. Residual releases are transactional across both legs. No one-leg release
   command exists at this boundary.
9. Confirmed equal complementary buy inventory may be locked only after current
   reconciliation; locking never claims merge confirmation or spendable profit.
10. Commands are bounded, canonical, content-idempotent, journal-first,
    replayable, checkpoint-verifiable, and fail closed.
11. The phase contains no credentials, signer, authenticated transport, RPC,
    wallet action, split/merge transaction, automatic retry, or live order.

## Acceptance criteria

- Tests cover authentic handoff registration, detached/duplicate rejection,
  non-confirmed non-posting, confirmed exact posting, failed trades, chain and
  ledger mismatch, current-reconciliation requirements, pair locking, unsafe
  release denial, transactional residual release, replay/checkpoint recovery,
  durable sync failure, and property invariants.
- A bounded TLA+ model proves registration origin, confirmed-only posting,
  failed non-posting, reconciliation-before-release, paired release atomicity,
  reservation retention, no live authority, and absorbing halt.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Split/merge submission, merge confirmation, redemption, credentials, signing,
authenticated adapters, RPC, wallets, automatic retry, and live trading.
