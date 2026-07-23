# Phase 1.8 Specification: Hourly Market-Session Coordinator

## Objective

Unify validated hourly identity, exact Up/Down books, settlement-reference
state, oracle contracts, cross-feed supervision, and hourly rollover in one
deterministic read-only coordinator.

## Requirements

1. Bind every registered BTC/ETH identity to an immutable resolution contract.
2. Reject and permanently halt on conflicting slot identities, overlapping
   windows, reused condition IDs, invalid contracts, or local clock regression.
3. Capture frames from immutable market replay, reference replay, actor, and
   supervisor snapshots without network or wall-clock I/O.
4. Prove supervisor provenance against exact market/reference epoch, sequence,
   and raw state digest before using its readiness result.
5. Active readiness requires the exact condition's Up and Down token books to
   exist and be authoritative, plus an exact in-progress oracle candle.
6. Exact start is active; exact end is no longer active and awaits final
   evidence. No pre-start or post-end session may be ready.
7. Finalize only from the exact immutable finalized candle and retain evidence
   while later sessions become current.
8. Select at most one current session and one next session per asset. Gaps are
   safe and produce no current session.
9. Failed integrity transitions are transactional; permanent halt is absorbing.
10. Ordered registration and coordination frames replay to the same digest.

## Acceptance criteria

- Tests cover ready/degraded states, token mismatch, source mismatch, exact
  boundaries, gaps, rollover with pending prior evidence, finalization,
  conflicts, clock regression, transactional failure, and replay equivalence.
- A bounded TLA+ model checks single-current, readiness-window, immutable-final,
  and absorbing-halt invariants.
- Formatting, Clippy with denied warnings, workspace tests, and TLC pass.

## Exclusions

Strategies, predictions, authenticated channels, orders, positions, capital
allocation, signing, split/merge transactions, and external databases.
