# Specification 092: Durable Recovery For The Window Paper Book

## Objective

Make the window paper book survive a worker restart. No opened bet may end
without a journalled result, no reported total may silently narrow to the
current process lifetime, and no failed journal write may pass unnoticed.

## Problem

Spec 091 delivered stake, marks, realized results and attribution, but held all
of it in memory:

- `EngineState.position` and the last observed snapshot existed only in RAM, so
  a restart abandoned every open position. Eleven bets carry a stake on disk
  with no exit and no settlement, and their outcome is now unknowable.
- `PaperLedger` was constructed empty on boot and never replayed the journal, so
  the published totals meant "since this process started" while presenting as
  the book. Disk held 175 closes and +$30.79; the running worker reported 169
  and +$27.32.
- `append_decision` and `write_status` discarded their errors, so a failing disk
  would drop journal records while the worker reported healthy operation.

The first two break the rule that every state transition needed for recovery
must be journaled. The third breaks the rule that corrupt or failed durable
state must halt rather than be silently skipped.

## Scope

- Replay the current day's decision journal into the ledger at boot so realized
  P&L, win/loss counts, and per-setup attribution are day-scoped.
- Treat an unparseable journal line as fatal: halt recovery, never skip it.
- Count, rather than silently absorb, terminal records written before Spec 091
  that carry no accounting fields.
- Checkpoint open positions atomically to `positions.json` on every tick that
  changes the open book, recording the position, its risk state, the recorded
  window open, and the last observed snapshot.
- On boot, restore checkpointed positions into their engines. A condition still
  present in the feed resumes; a condition absent from the feed settles from its
  persisted last snapshot and journals the settlement.
- Treat a corrupt or version-mismatched checkpoint as fatal.
- Fail loudly on a journal write error: record it in status and stop the worker
  rather than continue trading against an incomplete journal.
- Publish recovery provenance in the status file so an operator can see how many
  closes were replayed, how many positions were resumed, and how many were
  settled on restart.

## Exclusions

- Restoring per-second feature history. Momentum, persistence and acceleration
  rebuild from live observation, so a resumed position is managed from cold
  features. This is recorded as an accepted consequence, not hidden.
- Reconciliation against the authoritative on-chain resolver.
- Backfilling the eleven already-abandoned pre-Spec-091 positions. They have no
  recoverable terminal state and must stay visibly unresolved.
- Real money, credentials, wallets, signing, or Paper Week capital authority.

## Acceptance criteria

1. A worker restarted mid-window resumes its open positions with the same
   stake, cost basis, realized P&L, and side it had before the restart.
2. A position whose window ended while the worker was down is settled from its
   persisted last snapshot, and the settlement is journalled with its outcome.
3. After a restart with no market activity, the published realized total and
   closed-trade count equal the values derived from the day's journal.
4. An unparseable journal line or a corrupt checkpoint halts startup with a
   non-zero exit and a named reason; neither is skipped.
5. A checkpoint written by a different `strategy_version` is rejected rather
   than partially applied.
6. A journal write failure stops the worker and is visible in status.
7. Terminal records lacking accounting fields are counted in status as skipped,
   not folded into the totals as zero-P&L trades.
8. No floating-point type appears in any recovered stake, price, or P&L path.
9. Unit tests cover replay, corrupt-line halt, corrupt-checkpoint halt, version
   mismatch, resume, settle-on-restart, and the pre-Spec-091 skip counter.
