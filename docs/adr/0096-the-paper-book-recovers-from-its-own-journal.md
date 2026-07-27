# ADR 0096: The Paper Book Recovers From Its Own Journal

## Status

Accepted

## Context

ADR 0095 made the window engine mark a real book. It did not make that book
durable. An audit of one day's operation found three failures that all share a
root cause — the book was authoritative in memory and advisory on disk:

- Eleven positions have a stake journalled and no terminal record, because a
  restart dropped the in-memory position before anything could settle it.
- Published totals covered only the current process lifetime while presenting as
  the day's book (169 closes / +$27.32 reported against 175 / +$30.79 on disk).
- Journal and status write errors were discarded, so a failing disk would look
  identical to a quiet market.

The project's own invariants already forbid all three: every state transition
needed for recovery must be journaled, and corrupt durable state must halt
recovery rather than be silently skipped.

## Decision

- **The journal is the book.** On boot the runtime replays the current day's
  decision journal and rebuilds the ledger from its terminal records. Reported
  realized P&L, win/loss counts, and per-setup attribution are therefore
  day-scoped and identical before and after a restart.
- **Open positions are checkpointed, not remembered.** Each tick that changes
  the open book atomically rewrites `positions.json` with the position, its risk
  state, the recorded window open, and the last observed snapshot. The snapshot
  is included specifically so a window that expires while the worker is down can
  still be settled against a level we actually observed, rather than an invented
  one.
- **Recovery is all-or-nothing.** An unparseable journal line, a corrupt
  checkpoint, or a checkpoint written by another `strategy_version` halts
  startup with a named reason and a non-zero exit. Partial recovery of a
  financial book is worse than refusing to start.
- **A failed journal write stops the worker.** Continuing to evaluate against a
  journal known to be incomplete would produce evidence that cannot be replayed,
  which is the one thing this journal exists to prevent.
- **Missing accounting fields are counted, not coerced.** Terminal records from
  before Spec 091 carry no realized P&L. They are excluded from the totals and
  reported as a skipped count, because booking them as zero-P&L closes would
  silently dilute every win rate that reads the ledger.

## Consequences

A restart is no longer an accounting event. The eleven already-abandoned
positions stay unresolved and visible; this ADR deliberately does not invent
outcomes for them, since no observed closing level was ever persisted for them.

Feature history is not checkpointed. A resumed position is managed from cold
momentum, persistence and acceleration, so for the first seconds after a restart
the exit logic sees a weaker signal than it had before. Resuming with a cold
signal is strictly better than abandoning a funded position, but it means a
restart can bias a resumed position toward an earlier protective exit. Operators
should prefer restarting between windows.

Startup now performs a bounded read of one journal file. At roughly 150k records
per day this is a fraction of a second, but the cost grows with the day's
length, and a future spec may checkpoint ledger totals if that ever matters.
